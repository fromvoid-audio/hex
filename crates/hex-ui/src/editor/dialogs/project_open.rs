//! One selected Open intent across confirmation and an optional canonical save.
use super::super::EditorView;
use gpui::{AppContext, Context, PromptButton, PromptLevel, Window};
use hex_session::DocumentAudioMode;
use hex_session::snapshot_exports::SnapshotTransportError;
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
    Checking(Intent),
    Confirming(Intent),
    SaveRequested(Intent),
    Saving(Intent),
    Saved {
        intent: Intent,
        succeeded: bool,
        revision: u64,
    },
}

#[derive(Default)]
pub(in crate::editor) struct State {
    phase: Phase,
    serial: u64,
    retry_scheduled: bool,
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
    fn retry_project_open_read(&mut self, cx: &mut Context<Self>) {
        if self.project_open.retry_scheduled {
            return;
        }
        self.project_open.retry_scheduled = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(5))
                .await;
            let _ = this.update(cx, |view, cx| {
                view.project_open.retry_scheduled = false;
                cx.notify();
            });
        })
        .detach();
    }

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
        if let Phase::SaveRequested(intent) = &self.project_open.phase {
            self.request_open_save(intent.clone(), cx);
            return;
        }
        if let Phase::Saved {
            succeeded,
            revision,
            ..
        } = &self.project_open.phase
        {
            let (succeeded, revision) = (*succeeded, *revision);
            self.complete_open_save(succeeded, revision);
            if matches!(self.project_open.phase, Phase::Saved { .. }) {
                self.retry_project_open_read(cx);
            }
            return;
        }
        if let Phase::Checking(intent) = &self.project_open.phase {
            let intent = intent.clone();
            cx.defer_in(window, move |view, window, cx| {
                view.confirm_selected_open(intent, window, cx)
            });
            return;
        }
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
        let revision = match self.dialog_snapshot(|snapshot| snapshot.revision()) {
            Ok(Some(revision)) => revision,
            Ok(None) => return,
            Err(SnapshotTransportError::Busy) => {
                self.retry_project_open_read(cx);
                return;
            }
            Err(_) => {
                self.error = Some("project read is unavailable".into());
                return;
            }
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
        self.project_open.phase = Phase::Checking(intent.clone());
        cx.defer_in(window, move |view, window, cx| {
            view.confirm_selected_open(intent, window, cx)
        });
    }

    fn current_open_intent(&self, intent: &Intent) -> bool {
        self.project_generation == intent.generation
            && matches!(&self.project_open.phase,
            Phase::Checking(active) | Phase::Confirming(active) | Phase::SaveRequested(active) | Phase::Saving(active)
                | Phase::Saved { intent: active, .. } if active.serial == intent.serial)
    }

    fn confirm_selected_open(
        &mut self,
        intent: Intent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.current_open_intent(&intent)
            || !matches!(&self.project_open.phase, Phase::Checking(active) if active.serial == intent.serial)
        {
            return;
        }
        let state = match self.dialog_snapshot(|snapshot| (snapshot.revision(), snapshot.unsaved()))
        {
            Ok(state) => state,
            Err(SnapshotTransportError::Busy) => {
                self.retry_project_open_read(cx);
                return;
            }
            Err(_) => {
                self.cancel_open_after_save();
                self.error = Some("project read is unavailable".into());
                cx.notify();
                return;
            }
        };
        if state.is_none_or(|(revision, _)| revision != intent.revision) {
            self.cancel_open_after_save();
            self.error = Some("project changed · open cancelled".into());
            cx.notify();
            return;
        }
        let dirty =
            state.is_some_and(|(_, unsaved)| unsaved) || self.tuning.client.snapshot().is_some();
        if !dirty {
            self.project_open.phase = Phase::Idle;
            self.begin_open(intent.path, intent.mode);
            cx.notify();
            return;
        }
        self.project_open.phase = Phase::Confirming(intent.clone());
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
            let mut pending = Some(intent);
            loop {
                let retry = cx
                    .update_window(handle, |_, window, cx| {
                        this.update(cx, |view, cx| {
                            let Some(intent) = pending.as_ref() else {
                                return false;
                            };
                            if !view.current_open_intent(intent) {
                                return false;
                            }
                            let revision =
                                match view.dialog_snapshot(|snapshot| snapshot.revision()) {
                                    Ok(revision) => revision,
                                    Err(SnapshotTransportError::Busy) => return true,
                                    Err(_) => {
                                        view.cancel_open_after_save();
                                        view.error = Some("project read is unavailable".into());
                                        cx.notify();
                                        return false;
                                    }
                                };
                            if window.focused(cx).is_none() {
                                view.focus.focus(window);
                            }
                            let Some(intent) = pending.take() else {
                                return false;
                            };
                            if revision != Some(intent.revision) {
                                view.cancel_open_after_save();
                                view.error = Some("project changed · open cancelled".into());
                            } else {
                                match answer {
                                    Some(0) => {
                                        view.request_open_save(intent, cx);
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
                            false
                        })
                        .unwrap_or(false)
                    })
                    .unwrap_or(false);
                if !retry {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(5))
                    .await;
            }
        })
        .detach();
    }

    pub(in crate::editor) fn cancel_open_after_save(&mut self) {
        self.project_open.phase = Phase::Idle;
        self.pending_open_path = None;
        self.pending_open_mode = None;
    }

    fn request_open_save(&mut self, intent: Intent, cx: &mut Context<Self>) {
        self.project_open.phase = Phase::Saving(intent.clone());
        match self.save_project_checked(cx) {
            Ok(true) => {}
            Ok(false) => self.cancel_open_after_save(),
            Err(SnapshotTransportError::Busy) => {
                self.project_open.phase = Phase::SaveRequested(intent);
                self.retry_project_open_read(cx);
            }
            Err(_) => {
                self.cancel_open_after_save();
                self.error = Some("project read is unavailable".into());
            }
        }
    }

    pub(in crate::editor) fn complete_open_save(&mut self, succeeded: bool, revision: u64) {
        let intent = match &self.project_open.phase {
            Phase::Saving(intent) | Phase::Saved { intent, .. } => intent.clone(),
            _ => return,
        };
        let current = match self.dialog_snapshot(|snapshot| {
            !snapshot.unsaved()
                && snapshot.revision() == revision
                && snapshot.document_path().is_some()
        }) {
            Ok(current) => {
                succeeded && self.project_generation == intent.generation && current == Some(true)
            }
            Err(SnapshotTransportError::Busy) => {
                self.project_open.phase = Phase::Saved {
                    intent,
                    succeeded,
                    revision,
                };
                return;
            }
            Err(_) => {
                self.cancel_open_after_save();
                self.error = Some("project read is unavailable".into());
                return;
            }
        };
        self.cancel_open_after_save();
        if current {
            self.begin_open(intent.path, intent.mode);
        } else if succeeded {
            self.error = Some("project changed after saving · open cancelled".into());
        }
    }
}
