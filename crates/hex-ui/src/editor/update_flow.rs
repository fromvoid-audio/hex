//! Editor intent for one explicitly selected local update. The desktop owns
//! helper custody; the editor owns save and ordinary close decisions.
use super::EditorView;
use crate::{UpdateAction, UpdateCommand, UpdateReady, UpdateStatus};
use gpui::{Context, Window};
use hex_session::service::{Request, SaveCorrelation};

#[derive(Default)]
pub(super) struct State {
    next_id: u64,
    phase: Phase,
    ready: Option<UpdateReady>,
    available: bool,
    reported_result: bool,
}

#[derive(Clone, Copy, Default)]
enum Phase {
    #[default]
    Idle,
    Selecting {
        id: u64,
        action: UpdateAction,
    },
    Armed {
        id: u64,
    },
    Saving {
        id: u64,
        correlation: Option<SaveCorrelation>,
    },
    Preparing {
        id: u64,
        saved: Option<SaveCorrelation>,
        discard: bool,
        generation: u64,
        revision: u64,
    },
}

impl EditorView {
    pub(super) fn select_update(&mut self, action: UpdateAction, cx: &mut Context<Self>) {
        if !matches!(self.update_flow.phase, Phase::Idle) {
            self.error = Some("finish the current update action".into());
            return;
        }
        let Some(client) = cx.try_global::<crate::UpdateClient>().cloned() else {
            self.error = Some("local updates are unavailable in this build".into());
            return;
        };
        let Some(id) = self.update_flow.next_id.checked_add(1) else {
            self.error = Some("update request limit reached".into());
            return;
        };
        self.update_flow.next_id = id;
        if client.send(UpdateCommand::Select(id)).is_err() {
            self.error = Some("update worker is unavailable".into());
            return;
        }
        self.update_flow.phase = Phase::Selecting { id, action };
        self.status = "checking update".into();
        self.error = None;
        cx.notify();
    }

    fn cancel_update(&mut self, id: u64, cx: &Context<Self>) {
        if let Some(client) = cx.try_global::<crate::UpdateClient>() {
            let _ = client.send(UpdateCommand::Cancel(id));
        }
        self.update_flow.ready.take();
        self.update_flow.phase = Phase::Idle;
        // An OS termination waiting on this preparation does not quit.
        super::close::quit_refused(cx);
    }

    pub(super) fn cancel_requested_update(&mut self, cx: &mut Context<Self>) {
        let id = match self.update_flow.phase {
            Phase::Idle => return,
            Phase::Selecting { id, .. }
            | Phase::Armed { id }
            | Phase::Saving { id, .. }
            | Phase::Preparing { id, .. } => id,
        };
        self.cancel_update(id, cx);
        self.status = "update cancelled".into();
        cx.notify();
    }

    pub(super) fn update_armed(&self) -> bool {
        matches!(self.update_flow.phase, Phase::Armed { .. })
    }

    pub(super) fn update_label(&self) -> Option<&'static str> {
        match self.update_flow.phase {
            Phase::Idle if self.update_flow.available => Some("update available"),
            Phase::Selecting { .. } => Some("checking update"),
            Phase::Armed { .. } => Some("update on quit"),
            Phase::Saving { .. } => Some("saving for update"),
            Phase::Preparing { .. } => Some("preparing update"),
            Phase::Idle => None,
        }
    }

    pub(super) fn update_blocks_close(&self) -> bool {
        matches!(
            self.update_flow.phase,
            Phase::Selecting { .. } | Phase::Saving { .. } | Phase::Preparing { .. }
        )
    }

    pub(super) fn update_preparing(&self) -> bool {
        matches!(self.update_flow.phase, Phase::Preparing { .. })
    }

    pub(super) fn prepare_update_quit(&mut self, discard: bool, cx: &mut Context<Self>) -> bool {
        let Phase::Armed { id } = self.update_flow.phase else {
            return false;
        };
        self.begin_update_prepare(id, None, discard, cx);
        true
    }

    fn begin_update_prepare(
        &mut self,
        id: u64,
        saved: Option<SaveCorrelation>,
        discard: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(revision) = self.state.as_ref().map(|state| state.revision) else {
            self.cancel_update(id, cx);
            self.error = Some("project is still loading".into());
            return;
        };
        let action = if saved.is_some() {
            UpdateAction::SaveAndRestart
        } else {
            UpdateAction::InstallOnQuit
        };
        let sent = cx
            .try_global::<crate::UpdateClient>()
            .is_some_and(|client| client.send(UpdateCommand::Prepare { id, action }).is_ok());
        if sent {
            self.update_flow.phase = Phase::Preparing {
                id,
                saved,
                discard,
                generation: self.project_generation,
                revision,
            };
            self.status = "preparing update".into();
        } else {
            self.cancel_update(id, cx);
            self.error = Some("update worker is unavailable".into());
        }
        cx.notify();
    }

    /// Correlate the existing save/Save As/format-confirmation path when it is
    /// part of the current update intent. Existing pending edits must settle
    /// first, so no untagged queued save can be mistaken for this one.
    pub(super) fn submit_update_save(&mut self, request: &Request) -> Option<bool> {
        let Phase::Saving {
            id,
            correlation: None,
        } = self.update_flow.phase
        else {
            return None;
        };
        if !matches!(
            request,
            Request::SaveCurrent
                | Request::SaveNew(_)
                | Request::SaveCurrentWithFormatUpgrade { .. }
        ) {
            return None;
        }
        if self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
            || self.device_gesture_pending()
            || self.gain_recorder.blocks_edits()
            || self.midi_recorder.blocks_edits()
            || self.export_pending
            || self.project_open.busy()
            || self.pending_open_path.is_some()
            || self.close_prompt_open
            || self.duplicate_intent.is_pending()
            || self.pending_plugin_effect.is_some()
            || self.tuning.client.snapshot().is_some()
        {
            self.error = Some("finish pending edits before saving and restarting".into());
            return Some(false);
        }
        let Some(state) = self.state.as_ref() else {
            self.error = Some("project is still loading".into());
            return Some(false);
        };
        let correlation = SaveCorrelation {
            request_id: id,
            project_generation: self.project_generation,
            revision: state.revision,
        };
        let request = match request {
            Request::SaveCurrent => Request::SaveCurrent,
            Request::SaveNew(path) => Request::SaveNew(path.clone()),
            Request::SaveCurrentWithFormatUpgrade {
                project_generation,
                path,
            } => Request::SaveCurrentWithFormatUpgrade {
                project_generation: *project_generation,
                path: path.clone(),
            },
            _ => return None,
        };
        match self.service.submit_correlated_save(request, correlation) {
            Ok(()) => {
                self.worker_submitted();
                self.update_flow.phase = Phase::Saving {
                    id,
                    correlation: Some(correlation),
                };
                Some(true)
            }
            Err(_) => {
                self.error = Some("could not start update save".into());
                Some(false)
            }
        }
    }

    pub(super) fn update_save_terminal(
        &mut self,
        correlation: Option<SaveCorrelation>,
        success: bool,
        confirmation_required: bool,
        cx: &mut Context<Self>,
    ) {
        let Phase::Saving {
            id,
            correlation: Some(expected),
        } = self.update_flow.phase
        else {
            return;
        };
        if correlation != Some(expected) {
            return;
        }
        if confirmation_required && self.save_dialog_open {
            self.update_flow.phase = Phase::Saving {
                id,
                correlation: None,
            };
            return;
        }
        if success
            && self.project_generation == expected.project_generation
            && self.state.as_ref().is_some_and(|state| {
                state.revision == expected.revision
                    && !state.unsaved
                    && state.document_path.is_some()
            })
        {
            self.begin_update_prepare(id, Some(expected), false, cx);
        } else {
            self.cancel_update(id, cx);
            if self.error.is_none() {
                self.error = Some("save did not complete · update cancelled".into());
            }
        }
    }

    pub(super) fn poll_update(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(client) = cx.try_global::<crate::UpdateClient>().cloned() else {
            return false;
        };
        if let Some(ready) = client.take_ready() {
            if matches!(self.update_flow.phase, Phase::Preparing { id, .. } if id == ready.id) {
                self.update_flow.ready = Some(ready);
            } else {
                drop(ready);
            }
            return true;
        }
        if matches!(self.update_flow.phase, Phase::Idle) {
            if !self.update_flow.reported_result
                && let UpdateStatus::PreviousResult { message, failed } = client.status()
            {
                if failed {
                    self.error = Some(message.into());
                } else {
                    self.status = message.into();
                }
                self.update_flow.reported_result =
                    client.send(UpdateCommand::DismissResult).is_ok();
                return true;
            }
            let available = client.status() == UpdateStatus::Available;
            if self.update_flow.available != available {
                self.update_flow.available = available;
                return true;
            }
        }
        match (&self.update_flow.phase, client.status()) {
            (Phase::Selecting { id, action }, UpdateStatus::Selected(selected))
                if *id == selected =>
            {
                let (id, action) = (*id, *action);
                match action {
                    UpdateAction::InstallOnQuit => {
                        self.update_flow.phase = Phase::Armed { id };
                        self.status = "update selected · install on quit".into();
                    }
                    UpdateAction::SaveAndRestart => {
                        self.update_flow.phase = Phase::Saving {
                            id,
                            correlation: None,
                        };
                        self.save_project(cx);
                        if !self.saving_project && !self.save_dialog_open {
                            self.cancel_update(id, cx);
                        }
                    }
                }
                true
            }
            (
                Phase::Saving {
                    id,
                    correlation: None,
                },
                _,
            ) if !self.saving_project && !self.save_dialog_open => {
                let id = *id;
                self.cancel_update(id, cx);
                true
            }
            (
                Phase::Selecting { id, .. } | Phase::Preparing { id, .. },
                UpdateStatus::Failed(failed, error),
            ) if *id == failed || failed == 0 => {
                let id = *id;
                self.cancel_update(id, cx);
                self.error = Some(error.to_lowercase().into());
                true
            }
            _ => false,
        }
    }

    pub(super) fn reconcile_update_ready(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ready) = self.update_flow.ready.take() else {
            return;
        };
        cx.defer_in(window, move |view, window, cx| {
            let Phase::Preparing {
                id,
                saved,
                discard,
                generation,
                revision,
            } = view.update_flow.phase
            else {
                return;
            };
            if ready.id != id {
                return;
            }
            let no_pending = !super::close::requires_close_confirmation(
                false,
                view.worker_requests_pending,
                view.pending_requests.len(),
                view.duplicate_intent.is_pending(),
                view.opening_project,
                view.export_pending,
                view.device_gesture_pending()
                    || view.pending_plugin_effect.is_some()
                    || view.device_panel.has_pending_draft(cx)
                    || view.tuning.client.snapshot().is_some(),
            ) && !view.project_open.busy()
                && view.pending_open_path.is_none()
                && !view.save_dialog_open
                && !view.close_prompt_open
                && !view.automation_close_preflight.blocks_update_approval()
                && !window.is_pointer_dragging()
                && !view.gain_recorder.blocks_edits()
                && !view.midi_recorder.blocks_edits();
            let same_document = view.project_generation == generation
                && view
                    .state
                    .as_ref()
                    .is_some_and(|state| state.revision == revision);
            let state_valid = same_document
                && match saved {
                    Some(saved) => {
                        view.project_generation == saved.project_generation
                            && view.state.as_ref().is_some_and(|state| {
                                state.revision == saved.revision
                                    && !state.unsaved
                                    && state.document_path.is_some()
                            })
                    }
                    None => discard || view.state.as_ref().is_some_and(|state| !state.unsaved),
                };
            if !no_pending || !state_valid {
                drop(ready);
                view.cancel_update(id, cx);
                view.error = Some("project changed · update cancelled".into());
                cx.notify();
                return;
            }
            if !view.prepare_theme_window_close(window, cx) {
                drop(ready);
                view.cancel_update(id, cx);
                view.error = Some("theme save in progress · update cancelled".into());
                cx.notify();
                return;
            }
            view.cancel_browser_sample_playback();
            view.stop();
            match ready.approval.approve() {
                Ok(()) => {
                    window.remove_window();
                    super::close::quit_confirmed(cx);
                }
                Err(error) => {
                    view.cancel_update(id, cx);
                    view.error = Some(error.to_lowercase().into());
                    cx.notify();
                }
            }
        });
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::{AnyWindowHandle, AppContext, Empty, Entity, TestAppContext};
    use hex_session::service::Service;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::{Duration, Instant};

    struct Approval(Arc<AtomicBool>);
    impl crate::UpdateApproval for Approval {
        fn approve(self: Box<Self>) -> Result<(), String> {
            self.0.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    #[gpui::test]
    fn changed_project_refuses_ready_helper_without_approval(cx: &mut TestAppContext) {
        let service = Service::new(48_000, 32, 8_000_000).unwrap();
        let deadline = crate::test_wait::hang_deadline();
        loop {
            if let Ok(event) = service.try_event() {
                assert!(event.error.is_none());
                break;
            }
            assert!(Instant::now() < deadline, "session fixture did not start");
            std::thread::yield_now();
        }
        let path =
            std::env::temp_dir().join(format!("hex-update-ui-save-{}.hex", std::process::id()));
        service.submit(Request::SaveNew(path.clone())).unwrap();
        let state = loop {
            if let Ok(event) = service.try_event() {
                assert!(event.save_complete && event.error.is_none());
                break event.state;
            }
            assert!(Instant::now() < deadline, "session fixture did not save");
            std::thread::yield_now();
        };
        assert!(!state.unsaved && state.document_path.is_some());
        let approved = Arc::new(AtomicBool::new(false));
        let visual = cx.add_window(|_, _| Empty);
        let handle: AnyWindowHandle = *visual;
        let marker = approved.clone();
        let editor: Entity<EditorView> = cx
            .update_window(handle, |_, window, cx| {
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new(window, cx, service, Err("offline fixture".into()))
                });
                editor.update(cx, |view, cx| {
                    view.state = Some(state);
                    let actual = view.state.as_ref().unwrap().revision;
                    view.update_flow.phase = Phase::Preparing {
                        id: 1,
                        saved: Some(SaveCorrelation {
                            request_id: 1,
                            project_generation: view.project_generation,
                            revision: actual + 1,
                        }),
                        discard: false,
                        generation: view.project_generation,
                        revision: actual,
                    };
                    view.update_flow.ready = Some(UpdateReady {
                        id: 1,
                        approval: Box::new(Approval(marker)),
                    });
                    view.reconcile_update_ready(window, cx);
                });
                editor
            })
            .unwrap();
        cx.run_until_parked();
        assert!(!approved.load(Ordering::SeqCst));
        assert!(editor.read_with(cx, |view, _| matches!(view.update_flow.phase, Phase::Idle)));
        let marker = approved.clone();
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                let revision = view.state.as_ref().unwrap().revision;
                view.update_flow.phase = Phase::Preparing {
                    id: 2,
                    saved: Some(SaveCorrelation {
                        request_id: 2,
                        project_generation: view.project_generation,
                        revision,
                    }),
                    discard: false,
                    generation: view.project_generation,
                    revision,
                };
                view.mark_theme_save_pending_for_update_test();
                view.update_flow.ready = Some(UpdateReady {
                    id: 2,
                    approval: Box::new(Approval(marker)),
                });
                view.reconcile_update_ready(window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        assert!(
            !approved.load(Ordering::SeqCst),
            "pending theme save must not approve shutdown"
        );
        assert!(editor.read_with(cx, |view, _| matches!(view.update_flow.phase, Phase::Idle)));
        std::fs::remove_file(path).unwrap();
    }

    #[gpui::test]
    fn save_restart_waits_for_its_save_and_ready_helper(cx: &mut TestAppContext) {
        let service = Service::new(48_000, 32, 8_000_000).unwrap();
        let deadline = crate::test_wait::hang_deadline();
        while service.try_event().is_err() {
            assert!(
                std::time::Instant::now() < deadline,
                "save_restart_waits_for_its_save_and_ready_helper: hang guard expired"
            );
            std::thread::yield_now();
        }
        let path =
            std::env::temp_dir().join(format!("hex-update-ui-success-{}.hex", std::process::id()));
        service.submit(Request::SaveNew(path.clone())).unwrap();
        let state = loop {
            if let Ok(event) = service.try_event() {
                assert!(event.save_complete && event.error.is_none());
                break event.state;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "save_restart_waits_for_its_save_and_ready_helper: hang guard expired"
            );
            std::thread::yield_now();
        };
        let (client, worker) = crate::update_channel();
        cx.update(|cx| crate::install_update_client(cx, client));
        let visual = cx.add_window(|_, _| Empty);
        let handle: AnyWindowHandle = *visual;
        let editor: Entity<EditorView> = cx
            .update_window(handle, |_, window, cx| {
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new(window, cx, service, Err("offline fixture".into()))
                });
                editor.update(cx, |view, cx| {
                    view.state = Some(state);
                    view.select_update(UpdateAction::SaveAndRestart, cx);
                });
                editor
            })
            .unwrap();
        assert_eq!(
            worker.commands.try_recv().unwrap(),
            UpdateCommand::Select(1)
        );
        worker.publish(UpdateStatus::Selected(1));
        let command = loop {
            editor.update(cx, |view, cx| {
                view.tick(cx);
            });
            if let Ok(command) = worker.commands.try_recv() {
                break command;
            }
            assert!(
                Instant::now() < deadline,
                "save did not reach update preparation"
            );
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(
            command,
            UpdateCommand::Prepare {
                id: 1,
                action: UpdateAction::SaveAndRestart
            }
        );
        let approved = Arc::new(AtomicBool::new(false));
        assert!(!approved.load(Ordering::SeqCst));
        worker
            .deliver(UpdateReady {
                id: 1,
                approval: Box::new(Approval(approved.clone())),
            })
            .unwrap_or_else(|_| panic!("fixture delivery"));
        worker.publish(UpdateStatus::Ready(1));
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.tick(cx);
                view.reconcile_update_ready(window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        assert!(approved.load(Ordering::SeqCst));
        std::fs::remove_file(path).unwrap();
    }
}
