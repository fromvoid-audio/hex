//! One selected Open intent across confirmation and an optional canonical save.
use super::super::EditorView;
use gpui::{AppContext, Context, PromptButton, PromptLevel, Window};
use hex_session::DocumentAudioMode;
use std::path::PathBuf;

#[derive(Clone)]
struct Intent {
    path: PathBuf,
    mode: DocumentAudioMode,
    generation: u64,
    revision: u64,
    serial: u64,
}

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Picker {
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
            self.error = Some("open request identity limit reached".into());
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

    pub(in crate::editor) fn reconcile_project_open(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.project_open.busy()
            || self.pending_open_path.is_none()
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
        let Some(path) = self.pending_open_path.take() else {
            return;
        };
        let Some(serial) = self.next_open_serial() else {
            cx.notify();
            return;
        };
        let intent = Intent {
            path,
            mode: self
                .pending_open_mode
                .take()
                .unwrap_or(DocumentAudioMode::Resident),
            generation: self.project_generation,
            revision,
            serial,
        };
        self.project_open.phase = Phase::Confirming(intent.clone());
        cx.defer_in(window, move |view, window, cx| {
            view.confirm_selected_open(intent, window, cx)
        });
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
            self.error = Some("project changed · open cancelled".into());
            cx.notify();
            return;
        }
        let dirty = self.state.as_ref().is_some_and(|state| state.unsaved)
            || self.tuning.client.snapshot().is_some();
        if !dirty {
            self.project_open.phase = Phase::Idle;
            self.begin_open(intent.path, intent.mode);
            cx.notify();
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            "save changes before opening another project?",
            Some("save keeps the current project before opening the selected file. don't save discards unsaved changes only after the selected file loads successfully."),
            &[PromptButton::new("save"), PromptButton::new("don't save"), PromptButton::cancel("cancel")],
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
                        view.error = Some("project changed · open cancelled".into());
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
                                view.begin_open(intent.path, intent.mode);
                            }
                            _ => {
                                view.cancel_open_after_save();
                                view.status = "open cancelled".into();
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

    pub(in crate::editor) fn complete_open_save(&mut self, succeeded: bool, revision: u64) {
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
            self.begin_open(intent.path, intent.mode);
        } else if succeeded {
            self.error = Some("project changed after saving · open cancelled".into());
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
            let state = Arc::get_mut(view.state.as_mut().unwrap()).unwrap();
            state.unsaved = true;
            state.document_path = Some(PathBuf::from("original.hex"));
            view.project_open.phase = Phase::Saving(Intent {
                path: PathBuf::from("selected-next.hex"),
                mode: DocumentAudioMode::Resident,
                generation: view.project_generation,
                revision: state.revision,
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
