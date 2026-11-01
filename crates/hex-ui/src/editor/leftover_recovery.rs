//! Offer unsaved work left by a session that ended unexpectedly (Pulse #3990).
//! Detection is a read-only worker-side query; recovery reuses the explicit
//! open path, and declining leaves the journal to the copy-on-first-edit rule.
use super::{EditorView, theme};
use gpui::{AppContext as _, Context, PromptButton, PromptLevel, Window};
use hex_session::service::{LeftoverRecovery, Request};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Default)]
pub(super) enum State {
    #[default]
    Idle,
    /// Look for a leftover journal: the untitled slot (`None`) or beside a document.
    Check {
        document: Option<PathBuf>,
        generation: u64,
    },
    Checking,
    Found {
        found: LeftoverRecovery,
        generation: u64,
    },
}

impl EditorView {
    /// Arm one check for the current slot; a later open replaces it.
    pub(super) fn check_leftover_recovery(&mut self, document: Option<PathBuf>) {
        if self.service.recovery_slot().is_some() {
            self.leftover_recovery = State::Check {
                document,
                generation: self.project_generation,
            };
        }
    }

    /// The offer still applies: same project, and no edits made yet. Once
    /// editing starts, copy-on-first-edit keeps the journal instead.
    fn leftover_offer_current(&self, generation: u64) -> bool {
        generation == self.project_generation
            && self
                .state
                .as_ref()
                .is_none_or(|state| !state.can_undo && !state.can_redo)
    }

    /// Nothing else is opening or prompting, and the working project is known.
    fn leftover_offer_ready(&self) -> bool {
        self.state.is_some()
            && !self.opening_project
            && !self.close_prompt_open
            && !self.save_dialog_open
    }

    pub(super) fn reconcile_leftover_recovery(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match std::mem::take(&mut self.leftover_recovery) {
            State::Check {
                document,
                generation,
            } => {
                let Some(untitled) = self.service.recovery_slot().map(PathBuf::from) else {
                    return;
                };
                self.leftover_recovery = State::Checking;
                let found = cx.background_executor().spawn(async move {
                    hex_session::service::leftover_recovery(&untitled, document.as_deref())
                });
                cx.spawn(async move |this, cx| {
                    let found = found.await;
                    let _ = this.update(cx, |view, cx| {
                        if matches!(view.leftover_recovery, State::Checking) {
                            view.leftover_recovery = match found {
                                Some(found) => State::Found { found, generation },
                                None => State::Idle,
                            };
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
            State::Found { found, generation } => {
                if !self.leftover_offer_current(generation) {
                    return; // editing started or the project changed: copy-on-first-edit keeps it
                }
                if !self.leftover_offer_ready() {
                    // Startup state or another dialog may still be arriving.
                    self.leftover_recovery = State::Found { found, generation };
                    return;
                }
                cx.defer_in(window, move |view, window, cx| {
                    view.offer_leftover_recovery(found, generation, window, cx);
                });
            }
            other => self.leftover_recovery = other,
        }
    }

    fn offer_leftover_recovery(
        &mut self,
        found: LeftoverRecovery,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = theme::text("recover unsaved work?");
        let detail = theme::text(
            "hex found unsaved changes from a session that ended unexpectedly. recover opens them in place of the current project. keep for later leaves them untouched; your next edit keeps a copy beside the recovery journal.",
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            &title,
            Some(&detail),
            &[
                PromptButton::new(theme::text("recover")),
                PromptButton::new(theme::text("discard…")),
                PromptButton::cancel(theme::text("keep for later")),
            ],
            cx,
        );
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let answer = answer.await.ok();
            if answer == Some(1) {
                let _ = cx.update_window(handle, |_, window, cx| {
                    let _ = this.update(cx, |view, cx| {
                        view.confirm_leftover_discard(found, generation, window, cx);
                    });
                });
                return;
            }
            let _ = this.update(cx, |view, cx| {
                if answer != Some(0) {
                    view.status = "unsaved work kept for later".into();
                } else if !view.leftover_offer_current(generation) || !view.leftover_offer_ready() {
                    view.error = Some("project changed · recovery not opened".into());
                } else if !found.unchanged() {
                    view.error =
                        Some("recovery journal changed · open it from file > recover".into());
                } else {
                    let source = found.source.clone();
                    view.open_checking_missing_media(
                        source.clone(),
                        hex_session::DocumentAudioMode::Resident,
                        None,
                        Request::OpenWith {
                            source,
                            relinks: BTreeMap::new(),
                        },
                        cx,
                    );
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl EditorView {
    /// Discard needs its own confirmation. It sets the journal aside into a
    /// kept-copies folder rather than deleting it, and says where.
    fn confirm_leftover_discard(
        &mut self,
        found: LeftoverRecovery,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = theme::text("discard unsaved work?");
        let detail = theme::text(
            "hex stops offering these changes and moves them into a kept-copies folder beside the recovery journal. you can still open them with file > recover kept session….",
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            &title,
            Some(&detail),
            &[
                PromptButton::new(theme::text("discard")),
                PromptButton::cancel(theme::text("cancel")),
            ],
            cx,
        );
        cx.spawn(async move |this, cx| {
            let answer = answer.await.ok();
            let _ = this.update(cx, |view, cx| {
                if answer != Some(0) {
                    view.status = "unsaved work kept for later".into();
                } else if !view.leftover_offer_current(generation) {
                    view.error = Some("project changed · unsaved work kept".into());
                } else {
                    match found.discard() {
                        Ok(kept) => {
                            view.status = format!(
                                "unsaved work set aside at {}",
                                hex_session::service::text::verbatim(kept.display())
                            );
                        }
                        Err(reason) => {
                            view.error = Some(
                                format!("could not discard: {reason} · unsaved work kept").into(),
                            );
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "leftover_recovery_tests.rs"]
mod tests;
