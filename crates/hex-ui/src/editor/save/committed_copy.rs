//! Consent and the worker receipt for a copy that excludes a refused parameter edit.
use super::*;
use hex_session::device_gesture::{GestureKey, GestureTarget};
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Intent {
    key: GestureKey,
    sequence: u64,
    revision: u64,
}

impl Intent {
    fn current(view: &EditorView) -> Option<Self> {
        if !view.device_gesture.has_stranded_commit() {
            return None;
        }
        let active = view.device_gesture.snapshot()?;
        if active.key.generation != view.project_generation
            || !matches!(
                active.target,
                GestureTarget::DeviceParameter { .. }
                    | GestureTarget::DeviceParameterPair { .. }
                    | GestureTarget::DeviceParameterSnapshot { .. }
            )
        {
            return None;
        }
        Some(Self {
            key: active.key,
            sequence: active.sequence,
            revision: view.state.as_ref()?.revision,
        })
    }
}

#[derive(Default)]
pub(in crate::editor) struct CommittedCopy {
    prompt: Option<Intent>,
    receipt: Option<(Intent, Receiver<Result<PathBuf, String>>)>,
}
impl CommittedCopy {
    pub(super) fn pending(&self) -> bool {
        self.receipt.is_some()
    }
}

impl EditorView {
    pub(in crate::editor) fn offer_committed_copy(&mut self) -> bool {
        let Some(intent) = Intent::current(self) else {
            return false;
        };
        if let SaveAdmission::Reject(reason) = self.save_admission() {
            self.error = Some(reason.into());
            return true;
        }
        self.cancel_open_after_save();
        self.committed_copy.prompt = Some(intent);
        self.save_dialog_open = true;
        true
    }

    pub(in crate::editor) fn reconcile_committed_copy_prompt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(intent) = self.committed_copy.prompt.take() else {
            return;
        };
        cx.defer_in(window, move |view, window, cx| {
            if Intent::current(view) != Some(intent) {
                view.save_dialog_open = false;
                return;
            }
            let title = theme::text("save a copy with the previous parameter setting?");
            let detail = theme::text("the parameter edit could not be committed. the copy includes your committed edits. the pending parameter stays in this window.");
            let answer = window.prompt(PromptLevel::Warning, &title, Some(&detail), &[
                PromptButton::new(theme::text("save committed copy…")),
                PromptButton::cancel(theme::text("cancel")),
            ], cx);
            let handle = window.window_handle();
            cx.spawn(async move |this, cx| {
                let answer = answer.await.ok();
                let _ = cx.update_window(handle, |_, window, cx| {
                    let _ = this.update(cx, |view, cx| {
                        view.save_dialog_open = false;
                        if window.focused(cx).is_none() { view.focus.focus(window); }
                        if answer == Some(0) {
                            if Intent::current(view) == Some(intent) {
                                view.choose_save_path(Some(intent), cx);
                            } else {
                                view.error = Some("project or pending parameter changed · copy cancelled".into());
                            }
                        } else {
                            view.status = "copy cancelled · parameter edit still pending".into();
                        }
                        cx.notify();
                    });
                });
            }).detach();
        });
    }

    pub(super) fn submit_committed_copy(&mut self, intent: Intent, path: PathBuf) {
        if Intent::current(self) != Some(intent) {
            self.error = Some("project or pending parameter changed · copy cancelled".into());
            return;
        }
        let path = match prepare_save_path(path) {
            Ok(path) => path,
            Err(error) => {
                self.error = Some(error.into());
                return;
            }
        };
        let (reply, receipt) = sync_channel(1);
        if self.enqueue_request(
            Request::SaveCommittedParameterCopy {
                path,
                revision: intent.revision,
                key: intent.key,
                sequence: intent.sequence,
                reply,
            },
            false,
        ) {
            self.committed_copy.receipt = Some((intent, receipt));
            self.error = None;
            self.status = "saving committed copy".into();
        }
    }

    pub(in crate::editor) fn poll_committed_copy(&mut self) -> bool {
        let Some((intent, receipt)) = &self.committed_copy.receipt else {
            return false;
        };
        let result = match receipt.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                Err("project copy did not complete · check the destination before retrying".into())
            }
        };
        let same_project = intent.key.generation == self.project_generation;
        self.committed_copy.receipt = None;
        if same_project {
            match result {
                Ok(path) => {
                    self.status = format!(
                        "committed copy saved · {}",
                        hex_session::service::text::verbatim(path.display())
                    );
                    self.error = None;
                }
                Err(reason) => {
                    self.status = "committed copy could not be confirmed".into();
                    self.error = Some(reason.into());
                }
            }
        }
        true
    }
}
