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
    snapshot_exports::{
        SnapshotProjectRead, SnapshotTransportError, ui_consumers::SnapshotSessionCellContentRead,
    },
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
                    let Ok(current) = view.browser_audio_drag_current(drag) else {
                        return;
                    };
                    let Ok(exists) = view.console_target_exists_checked(target) else {
                        return;
                    };
                    drag.last_pointer.set(Some((
                        f32::from(event.event.position.x),
                        f32::from(event.event.position.y),
                    )));
                    drag.preview.set(None);
                    drag.lane_hovered.set(false);
                    drag.insertion_line_y.set(None);
                    let destination = (view.mode == EditorMode::Session && current && exists)
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

    fn console_target_exists_in(
        project: SnapshotProjectRead<'_>,
        target: ConsoleDestination,
    ) -> bool {
        let (track, before) = match target {
            ConsoleDestination::Cell { track, scene } => (track, Some(scene)),
            ConsoleDestination::InsertScene { track, before } => (track, before),
        };
        project.track(track).is_some() && before.is_none_or(|scene| project.scene(scene).is_some())
    }

    pub(super) fn console_target_exists_checked(
        &self,
        target: ConsoleDestination,
    ) -> Result<bool, SnapshotTransportError> {
        let Some(state) = self.state.as_ref() else {
            return Ok(false);
        };
        state.read_with(|snapshot| Self::console_target_exists_in(snapshot.model(), target))
    }

    fn drop_console_browser_audio(
        &mut self,
        drag: &BrowserAudioDrag,
        target: ConsoleDestination,
        cx: &mut Context<Self>,
    ) {
        let Ok(drag_current) = self.browser_audio_drag_current(drag) else {
            return;
        };
        let target_facts = match self.state.as_ref() {
            Some(state) => match state.read_with(|snapshot| {
                (
                    Self::console_target_exists_in(snapshot.model(), target),
                    snapshot.revision(),
                )
            }) {
                Ok(facts) => Some(facts),
                Err(_) => return,
            },
            None => None,
        };
        let current = self.mode == EditorMode::Session
            && drag_current
            && target_facts.is_some_and(|(exists, _)| exists)
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
        let Some((_, revision)) = target_facts else {
            cx.notify();
            return;
        };
        let context = AudioBatchContext {
            project_generation: self.project_generation,
            revision,
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
            self.audio_batch = Some(control);
            self.audio_batch_progress = None;
            self.error = None;
            self.poll_audio_batch();
        }
        cx.notify();
    }

    fn drop_console_arrangement_clip(
        &mut self,
        drag: &ClipDrag,
        target: ConsoleDestination,
        cx: &mut Context<Self>,
    ) {
        let Ok(drag_current) = self.drag_start_current(drag) else {
            return;
        };
        let current = self.mode == EditorMode::Session && drag_current && !self.edit_pending();
        let target_state = match self.state.as_ref() {
            Some(state) => match state.read_with(|snapshot| {
                let project = snapshot.model();
                let empty = match target {
                    ConsoleDestination::Cell { track, scene } => matches!(
                        project.session_cell(track, scene),
                        SnapshotSessionCellContentRead::Empty
                    ),
                    ConsoleDestination::InsertScene { .. } => false,
                };
                (Self::console_target_exists_in(project, target), empty)
            }) {
                Ok(facts) => facts,
                Err(_) => return,
            },
            None => (false, false),
        };
        // This receiver owns one Console clip, not a mapping of a selection.
        // Release the shared group borrow before retiring its exact UI payload.
        let multiple = drag.group.borrow().ids.len() > 1;
        if multiple {
            self.clear_clip_drag(drag.project_generation, drag.clip);
            self.error = Some("selected clips require an arrangement target".into());
            cx.notify();
            return;
        }
        let command = if current
            && target_state.0
            && target_state.1
            && let ConsoleDestination::Cell { track, scene } = target
            && track == drag.source_track
        {
            Some(match drag.position {
                ClipPosition::Audio { .. } => Command::CopyArrangementAudioClipToSession {
                    source: drag.clip,
                    scene,
                },
                ClipPosition::Midi { .. } => Command::CopyArrangementMidiToSession {
                    source: drag.clip,
                    scene,
                },
            })
        } else {
            None
        };
        self.clear_clip_drag(drag.project_generation, drag.clip);
        if let Some(command) = command {
            self.enqueue(Request::Edit(command));
        } else {
            self.error = Some(
                "clip copy requires an existing empty console cell on its source track".into(),
            );
        }
        cx.notify();
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
