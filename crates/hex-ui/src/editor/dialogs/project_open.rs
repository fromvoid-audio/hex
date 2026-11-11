//! One captured New/Open destination across preparation, confirmation and a canonical save.
use super::super::EditorView;
use crate::theme;
use gpui::{AppContext, Context, PromptButton, PromptLevel, Window};
use hex_session::DocumentAudioMode;
use std::path::PathBuf;

#[derive(Clone)]
enum Destination {
    New,
    Open(PathBuf),
}

#[derive(Clone)]
struct Intent {
    destination: Destination,
    mode: DocumentAudioMode,
    generation: u64,
    revision: u64,
    serial: u64,
}

impl Intent {
    fn action_label(&self) -> &'static str {
        match self.destination {
            Destination::New => "new project",
            Destination::Open(_) => "open",
        }
    }
}

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Picker {
        generation: u64,
        serial: u64,
    },
    PreparingNew {
        mode: DocumentAudioMode,
        generation: u64,
        serial: u64,
    },
    Confirming(Intent),
    Saving(Intent),
}

#[derive(Default)]
pub(in crate::editor) struct State {
    phase: Phase,
    serial: u64,
}

impl State {
    pub(in crate::editor) fn busy(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }
    pub(in crate::editor) fn permits_save(&self) -> bool {
        matches!(self.phase, Phase::Saving(_))
    }
    fn next(&mut self) -> Option<u64> {
        self.serial = self.serial.checked_add(1)?;
        Some(self.serial)
    }
}

impl EditorView {
    fn next_open_serial(&mut self) -> Option<u64> {
        let serial = self.project_open.next();
        if serial.is_none() {
            self.cancel_open_after_save();
            self.error = Some("project request identity limit reached".into());
        }
        serial
    }

    pub(in crate::editor) fn start_open_picker(&mut self) -> Option<(u64, u64)> {
        if self.opening_project
            || self.close_prompt_open
            || self.saving_project
            || self.save_dialog_open
            || self.gain_recorder.blocks_edits()
            || self.project_open.busy()
            || self.pending_open_path.is_some()
        {
            self.error =
                Some("finish the current project action before opening another project".into());
            return None;
        }
        let serial = self.next_open_serial()?;
        let generation = self.project_generation;
        self.project_open.phase = Phase::Picker { generation, serial };
        Some((generation, serial))
    }

    pub(in crate::editor) fn open_picker_is_current(&self, generation: u64, serial: u64) -> bool {
        self.project_generation == generation
            && matches!(self.project_open.phase,
            Phase::Picker { generation: active_generation, serial: active_serial }
                if active_generation == generation && active_serial == serial)
    }

    pub(in crate::editor) fn finish_open_picker(&mut self, generation: u64, serial: u64) -> bool {
        let matches = matches!(self.project_open.phase, Phase::Picker { generation: active_generation, serial: active_serial } if active_generation == generation && active_serial == serial);
        if !matches {
            return false;
        }
        self.project_open.phase = Phase::Idle;
        if self.project_generation != generation {
            self.status = "open cancelled because project changed".into();
            return false;
        }
        true
    }

    pub(in crate::editor) fn queue_new_project(&mut self, mode: DocumentAudioMode) {
        let Some(serial) = self.next_open_serial() else {
            return;
        };
        self.cancel_duplicate_chain();
        self.project_open.phase = Phase::PreparingNew {
            mode,
            generation: self.project_generation,
            serial,
        };
        self.status = "preparing new project".into();
        self.error = None;
    }

    /// A refused admitted edit/finalization cannot be followed by a new-document
    /// prompt that would silently leave that work behind.
    pub(in crate::editor) fn cancel_new_preparation(&mut self) -> bool {
        if !matches!(self.project_open.phase, Phase::PreparingNew { .. }) {
            return false;
        }
        self.cancel_open_after_save();
        self.status = "new project cancelled".into();
        true
    }

    pub(in crate::editor) fn reconcile_project_open(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let queued_new = match self.project_open.phase {
            Phase::PreparingNew {
                mode,
                generation,
                serial,
            } => Some((mode, generation, serial)),
            _ => None,
        };
        if (self.project_open.busy() && queued_new.is_none())
            || (queued_new.is_none() && self.pending_open_path.is_none())
            || self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
            || self.device_gesture_pending()
            || self.tuning.client.has_pending_requests()
            || self.saving_project
            || self.save_dialog_open
            || self.close_prompt_open
        {
            return;
        }
        let Some(revision) = self.state.as_ref().map(|state| state.revision) else {
            return;
        };
        let intent = if let Some((mode, generation, serial)) = queued_new {
            if generation != self.project_generation {
                self.cancel_open_after_save();
                self.status = "new project cancelled because project changed".into();
                return;
            }
            Intent {
                destination: Destination::New,
                mode,
                generation,
                revision,
                serial,
            }
        } else {
            let Some(path) = self.pending_open_path.take() else {
                return;
            };
            let Some(serial) = self.next_open_serial() else {
                cx.notify();
                return;
            };
            Intent {
                destination: Destination::Open(path),
                mode: self
                    .pending_open_mode
                    .take()
                    .unwrap_or(DocumentAudioMode::Resident),
                generation: self.project_generation,
                revision,
                serial,
            }
        };
        self.project_open.phase = Phase::Confirming(intent.clone());
        cx.defer_in(window, move |view, window, cx| {
            view.confirm_selected_open(intent, window, cx)
        });
    }

    fn begin_selected_project(&mut self, intent: Intent, cx: &mut Context<Self>) {
        match intent.destination {
            Destination::New => self.begin_new_project(intent.mode),
            Destination::Open(path) => self.begin_open(path, intent.mode, cx),
        }
    }

    fn current_open_intent(&self, intent: &Intent) -> bool {
        self.project_generation == intent.generation
            && matches!(&self.project_open.phase,
            Phase::Confirming(active) | Phase::Saving(active) if active.serial == intent.serial)
    }

    fn confirm_selected_open(
        &mut self,
        intent: Intent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.current_open_intent(&intent) {
            return;
        }
        if self
            .state
            .as_ref()
            .is_none_or(|state| state.revision != intent.revision)
        {
            self.cancel_open_after_save();
            self.error =
                Some(format!("project changed · {} cancelled", intent.action_label()).into());
            cx.notify();
            return;
        }
        let dirty = self.state.as_ref().is_some_and(|state| state.unsaved)
            || self.tuning.client.snapshot().is_some();
        if !dirty {
            self.project_open.phase = Phase::Idle;
            self.begin_selected_project(intent, cx);
            cx.notify();
            return;
        }
        let (title, detail) = match &intent.destination {
            Destination::New => (
                "save changes before creating a new project?",
                "save keeps the current project before creating a new one. don't save discards unsaved changes after the new project is prepared.",
            ),
            Destination::Open(_) => (
                "save changes before opening another project?",
                "save keeps the current project before opening the selected file. don't save discards unsaved changes only after the selected file loads successfully.",
            ),
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &theme::text(title),
            Some(&theme::text(detail)),
            &[
                PromptButton::new(theme::text("save")),
                PromptButton::new(theme::text("don't save")),
                PromptButton::cancel(theme::text("cancel")),
            ],
            cx,
        );
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let answer = answer.await.ok();
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = this.update(cx, |view, cx| {
                    if !view.current_open_intent(&intent) {
                        return;
                    }
                    if window.focused(cx).is_none() {
                        view.focus.focus(window);
                    }
                    if view
                        .state
                        .as_ref()
                        .is_none_or(|state| state.revision != intent.revision)
                    {
                        view.cancel_open_after_save();
                        view.error = Some(
                            format!("project changed · {} cancelled", intent.action_label()).into(),
                        );
                    } else {
                        match answer {
                            Some(0) => {
                                view.project_open.phase = Phase::Saving(intent);
                                view.save_project(cx);
                                if !view.saving_project && !view.save_dialog_open {
                                    view.cancel_open_after_save();
                                }
                            }
                            Some(1) => {
                                view.project_open.phase = Phase::Idle;
                                view.begin_selected_project(intent, cx);
                            }
                            _ => {
                                view.cancel_open_after_save();
                                view.status = format!("{} cancelled", intent.action_label());
                            }
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(in crate::editor) fn cancel_open_after_save(&mut self) {
        self.project_open.phase = Phase::Idle;
        self.pending_open_path = None;
        self.pending_open_mode = None;
    }

    pub(in crate::editor) fn complete_open_save(
        &mut self,
        succeeded: bool,
        revision: u64,
        cx: &mut Context<Self>,
    ) {
        let Phase::Saving(intent) = &self.project_open.phase else {
            return;
        };
        let current = succeeded
            && self.project_generation == intent.generation
            && self.state.as_ref().is_some_and(|state| {
                !state.unsaved && state.revision == revision && state.document_path.is_some()
            });
        let intent = intent.clone();
        self.cancel_open_after_save();
        if current {
            self.begin_selected_project(intent, cx);
        } else if succeeded {
            self.error = Some(
                format!(
                    "project changed after saving · {} cancelled",
                    intent.action_label()
                )
                .into(),
            );
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project};
    use hex_session::{
        device_gesture::{GestureAck, GesturePhase, GestureTarget, ParameterValue},
        service::Request,
    };
    use std::sync::Arc;

    #[gpui::test]
    fn format_upgrade_refused_device_terminal_retires_save_before_open_intent(
        cx: &mut gpui::TestAppContext,
    ) {
        let mut project = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        let Created::Track(track) = project
            .execute(Command::AddTrack {
                name: "unsaved track".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let (handle, editor) = crate::editor::clip_drag_ui_test::editor_window(
            cx,
            project.project().clone(),
            Arc::default(),
        );
        editor.update(cx, |view, _| {
            let state = view.state.as_ref().unwrap();
            let revision = state.revision;
            view.state = Some(Arc::new(hex_session::service::ViewState {
                canonical_audio_source: state.canonical_audio_source.clone(),
                comparison_audio: state.comparison_audio.clone(),
                project: Arc::clone(&state.project),
                waveforms: state.waveforms.clone(),
                source_overviews: state.source_overviews.clone(),
                audio_clip_waveforms: state.audio_clip_waveforms.clone(),
                midi_preview: Arc::clone(&state.midi_preview),
                gain_preview: state.gain_preview.clone(),
                timeline_frames: state.timeline_frames,
                can_undo: state.can_undo,
                can_redo: state.can_redo,
                revision,
                unsaved: true,
                document_path: Some(PathBuf::from("original.hex")),
            }));
            view.project_open.phase = Phase::Saving(Intent {
                destination: Destination::Open(PathBuf::from("selected-next.hex")),
                mode: DocumentAudioMode::Resident,
                generation: view.project_generation,
                revision,
                serial: 1,
            });
            let key = view
                .device_gesture
                .begin(
                    view.project_generation,
                    GestureTarget::TrackGain { track },
                    ParameterValue::Float(1.0),
                )
                .unwrap();
            view.device_gesture.next_request().unwrap();
            assert!(view.acknowledge_device_gesture(&GestureAck {
                key,
                phase: GesturePhase::Begin,
                sequence: 0,
                accepted: true,
                preview_retained: false,
                source_lost: false,
                reason: None,
            }));
            let accepted = view.device_gesture.snapshot().unwrap();
            assert!(
                view.enqueue_device_barrier(Request::SaveCurrentWithFormatUpgrade {
                    project_generation: view.project_generation,
                    path: PathBuf::from("original.hex"),
                })
            );
            view.saving_project = true;
            let terminal = view.device_gesture.next_request().unwrap();
            view.device_gesture.mark_submitted();
            assert!(view.project_open.permits_save());
            assert!(matches!(
                view.device_gesture_barriers.front(),
                Some(Request::SaveCurrentWithFormatUpgrade { .. })
            ));
            assert!(view.acknowledge_device_gesture_with_snapshot(
                &GestureAck {
                    key,
                    phase: GesturePhase::Commit,
                    sequence: terminal.sequence(),
                    accepted: false,
                    preview_retained: true,
                    source_lost: false,
                    reason: Some("audio publication queue full".into()),
                },
                Some(accepted)
            ));
            assert!(!view.saving_project);
            assert!(
                !view.project_open.busy(),
                "a later save must not open the abandoned selection"
            );
            assert!(view.device_gesture_barriers.is_empty());
            assert!(view.pending_requests.is_empty());
            assert!(view.state.as_ref().unwrap().unsaved);
        });
        cx.update_window(handle, |_, window, _| window.remove_window())
            .unwrap();
    }
}
