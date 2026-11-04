//! Console hit surfaces reuse the existing atomic audio-import worker.
use super::{
    EditorMode, EditorView,
    drag::{BrowserAudioDrag, BrowserDropDestination, ClipDrag, ClipPosition},
};
use crate::theme;
use gpui::{Context, Div, Stateful, prelude::*, rgb};
use hex_project::{Command, SceneId};
use hex_session::{
    import_batch::{AudioBatchDestination, ConsoleDestination},
    import_batch_decode::AudioBatchContext,
    service::Request,
};

impl EditorView {
    pub(super) fn console_drop_surface(
        &self,
        surface: Stateful<Div>,
        target: ConsoleDestination,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let active = self.active_browser_audio_drag.as_ref().is_some_and(|drag| {
            self.browser_audio_drag_is_current(drag)
                && drag.destination.get() == Some(BrowserDropDestination::Console(target))
        });
        let entity = cx.entity();
        surface
            .when(active, |surface| {
                surface.bg(rgb(theme::color(theme::PaletteRole::Secondary)))
            })
            .on_drag_move::<BrowserAudioDrag>(move |event, _, cx| {
                if !event.bounds.contains(&event.event.position) {
                    return;
                }
                let Some(drag) = event.dragged_item().downcast_ref::<BrowserAudioDrag>() else {
                    return;
                };
                entity.update(cx, |view, cx| {
                    drag.last_pointer.set(Some((
                        f32::from(event.event.position.x),
                        f32::from(event.event.position.y),
                    )));
                    drag.preview.set(None);
                    drag.lane_hovered.set(false);
                    drag.insertion_line_y.set(None);
                    let destination = (view.mode == EditorMode::Session
                        && view.browser_audio_drag_is_current(drag)
                        && view.console_target_exists(target))
                    .then_some(BrowserDropDestination::Console(target));
                    if drag.destination.replace(destination) != destination {
                        cx.notify();
                    }
                });
            })
            .on_drop(cx.listener(move |view, drag: &BrowserAudioDrag, _, cx| {
                view.drop_console_browser_audio(drag, target, cx);
            }))
            .on_drop(cx.listener(move |view, drag: &ClipDrag, _, cx| {
                view.drop_console_arrangement_clip(drag, target, cx);
            }))
    }

    pub(super) fn console_target_exists(&self, target: ConsoleDestination) -> bool {
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        target.validate(&state.project).is_ok()
    }

    pub(super) fn drop_console_browser_audio(
        &mut self,
        drag: &BrowserAudioDrag,
        target: ConsoleDestination,
        cx: &mut Context<Self>,
    ) {
        let current = self.mode == EditorMode::Session
            && self.browser_audio_drag_is_current(drag)
            && self.console_target_exists(target)
            && !self.edit_pending()
            && !self.save_dialog_open
            && !self.export_dialog_open
            && self.audio_batch.is_none()
            && !self.tuning.open
            && !self.theme_settings.open
            && !self.changelog.is_open()
            && !self.device_panel.has_pending_draft(cx)
            && self.tuning.client.snapshot().is_none();
        // Always retire this exact UI payload before admission. The worker's
        // AudioBatchControl, not a retained ghost, owns cancellation afterwards.
        self.clear_browser_audio_drag(drag);
        if !current {
            self.error = Some("sample drop canceled · console target or project changed".into());
            cx.notify();
            return;
        }
        let Some(state) = self.state.as_ref() else {
            cx.notify();
            return;
        };
        let context = AudioBatchContext {
            project_generation: self.project_generation,
            revision: state.revision,
            active_gesture: false,
        };
        let (request, control) = match Request::audio_batch(
            vec![drag.path.clone()],
            AudioBatchDestination::Console(target),
            context,
        ) {
            Ok(request) => request,
            Err(reason) => {
                self.error = Some(reason.into());
                cx.notify();
                return;
            }
        };
        if self.enqueue(request) {
            self.stop_browser_preview_for_drop();
            self.audio_batch = Some(control);
            self.audio_batch_progress = None;
            self.error = None;
            self.poll_audio_batch();
        }
        cx.notify();
    }

    pub(super) fn drop_console_arrangement_clip(
        &mut self,
        drag: &ClipDrag,
        target: ConsoleDestination,
        cx: &mut Context<Self>,
    ) {
        let current = self.mode == EditorMode::Session
            && self.console_clip_drag_owned(drag)
            && self.drag_start_is_current(drag)
            && !self.edit_pending();
        // This receiver owns one Console clip, not a mapping of a selection.
        // Release the shared group borrow before retiring its exact UI payload.
        let multiple = drag.group.borrow().ids.len() > 1;
        if multiple {
            self.retire_console_clip_drag(drag);
            self.error = Some("selected clips require an arrangement target".into());
            cx.notify();
            return;
        }
        if let ConsoleDestination::NewTrack {
            before_track,
            scene,
        } = target
        {
            let unsupported_source = current && !self.console_column_clip_valid(drag);
            let valid = current
                && self.console_target_exists(target)
                && self.console_column_clip_valid(drag)
                && !self.save_dialog_open
                && !self.export_dialog_open
                && !self.export_pending
                && self.audio_batch.is_none()
                && !self.tuning.open
                && !self.theme_settings.open
                && !self.changelog.is_open()
                && !self.device_panel.has_pending_draft(cx)
                && self.tuning.client.snapshot().is_none();
            self.retire_console_clip_drag(drag);
            if valid {
                let gain = match self.track_defaults.captured_gain() {
                    Ok(gain) => gain,
                    Err(error) => {
                        self.error = Some(error.into());
                        cx.notify();
                        return;
                    }
                };
                if self.enqueue(Request::CopyArrangementClipToConsoleTrack {
                    initial_gain: Some(gain),
                    project_generation: drag.project_generation,
                    revision: drag.revision,
                    source: drag.clip,
                    before_track,
                    scene,
                }) {
                    self.error = None;
                }
            } else {
                self.error = Some(if unsupported_source {
                    "processed arrangement audio cannot yet copy to console · keep it in arrangement"
                } else {
                    "clip drop canceled · console target changed or editing is busy"
                }.into());
            }
            cx.notify();
            return;
        }
        let command = if current
            && let ConsoleDestination::Cell { track, scene } = target
            && track == drag.source_track
            && self.console_target_exists(target)
        {
            self.state.as_ref().and_then(|state| match drag.position {
                ClipPosition::Audio { .. }
                    if state.project.session_cell(track, scene).is_none() =>
                {
                    Some(Command::CopyArrangementAudioClipToSession {
                        source: drag.clip,
                        scene,
                    })
                }
                ClipPosition::Midi { .. } if state.project.session_cell(track, scene).is_none() => {
                    Some(Command::CopyArrangementMidiToSession {
                        source: drag.clip,
                        scene,
                    })
                }
                _ => None,
            })
        } else {
            None
        };
        self.retire_console_clip_drag(drag);
        if let Some(command) = command {
            self.enqueue(Request::Edit(command));
        } else {
            self.error = Some(
                "clip copy requires an existing empty console cell on its source track".into(),
            );
        }
        cx.notify();
    }

    pub(super) fn console_clip_drag_owned(&self, drag: &ClipDrag) -> bool {
        self.active_clip_drag.as_ref().is_some_and(|active| {
            active.project_generation == drag.project_generation
                && active.revision == drag.revision
                && active.clip == drag.clip
                && std::rc::Rc::ptr_eq(&active.group, &drag.group)
        })
    }

    // A delayed receiver may carry the same source ID as a newer pointer drag.
    // Retire that payload alone; only the exact current group may clear input.
    pub(super) fn retire_console_clip_drag(&mut self, drag: &ClipDrag) {
        if self.console_clip_drag_owned(drag) {
            self.clear_clip_drag(drag.project_generation, drag.clip);
        } else {
            drag.retire();
        }
    }

    pub(super) fn insert_console_scene(
        &mut self,
        before: Option<SceneId>,
        generation: u64,
        revision: u64,
        cx: &mut Context<Self>,
    ) {
        if self.mode != EditorMode::Session
            || self.project_generation != generation
            || self.edit_pending()
            || self.state.as_ref().is_none_or(|state| {
                state.revision != revision
                    || before.is_some_and(|id| {
                        !state.project.scenes().iter().any(|scene| scene.id() == id)
                    })
            })
        {
            self.error = Some("scene insertion canceled · project or anchor changed".into());
            cx.notify();
            return;
        }
        let name = format!(
            "scene {}",
            self.state
                .as_ref()
                .map_or(1, |state| state.project.scenes().len() + 1)
        );
        self.enqueue(Request::Edit(Command::InsertScene { name, before }));
        cx.notify();
    }
}
