//! Offer unsaved work left by a session that ended unexpectedly (Pulse #3990).
//! Detection is a read-only worker-side query; recovery reuses the explicit
//! open path, and declining leaves the journal to the copy-on-first-edit rule.
//! An untitled launch offers every unowned leftover slot one after another
//! (Pulse #4575); a declined foreign slot is released for a later launch.
use super::{EditorView, theme};
use gpui::{AppContext as _, Context, PromptButton, PromptLevel, Window};
use hex_session::service::{LeftoverRecovery, Request};
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
};

#[derive(Default)]
pub(super) enum State {
    #[default]
    Idle,
    /// Look for leftover journals: the untitled slots (`None`) or beside a document.
    Check {
        document: Option<PathBuf>,
        generation: u64,
    },
    Checking,
    /// Offers waiting in order. `generation` is the project they apply to, or
    /// `None` after a recovery: they then apply to the project it installs.
    Found {
        queue: VecDeque<LeftoverRecovery>,
        generation: Option<u64>,
    },
}

/// One offer being shown, plus the offers that follow it.
struct Offer {
    found: LeftoverRecovery,
    rest: VecDeque<LeftoverRecovery>,
    generation: Option<u64>,
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
    fn leftover_offer_current(&self, generation: Option<u64>) -> bool {
        generation.is_none_or(|generation| generation == self.project_generation)
            && self
                .state
                .as_ref()
                .is_none_or(|state| !state.can_undo && !state.can_redo)
    }

    /// Nothing else is opening or prompting, and the working project is known.
    /// After a recovery, its open must also have finished.
    fn leftover_offer_ready(&self, generation: Option<u64>) -> bool {
        self.state.is_some()
            && !self.opening_project
            && !self.close_prompt_open
            && !self.save_dialog_open
            && (generation.is_some()
                || (self.pending_requests.is_empty()
                    && self.worker_requests_pending == 0
                    && self.missing_media.is_none()))
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
                    match document {
                        Some(document) => {
                            hex_session::service::leftover_recovery(&untitled, Some(&document))
                                .into_iter()
                                .collect()
                        }
                        None => hex_session::service::untitled_leftovers(&untitled),
                    }
                });
                cx.spawn(async move |this, cx| {
                    let queue: VecDeque<_> = found.await.into();
                    let _ = this.update(cx, |view, cx| {
                        if matches!(view.leftover_recovery, State::Checking) {
                            view.leftover_recovery = if queue.is_empty() {
                                State::Idle
                            } else {
                                State::Found {
                                    queue,
                                    generation: Some(generation),
                                }
                            };
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
            State::Found {
                mut queue,
                generation,
            } => {
                if !self.leftover_offer_current(generation) {
                    return; // editing started or the project changed: copy-on-first-edit keeps it
                }
                if !self.leftover_offer_ready(generation) {
                    // Startup state or another dialog may still be arriving.
                    self.leftover_recovery = State::Found { queue, generation };
                    return;
                }
                let Some(found) = queue.pop_front() else {
                    return;
                };
                let offer = Offer {
                    found,
                    rest: queue,
                    generation,
                };
                cx.defer_in(window, move |view, window, cx| {
                    view.offer_leftover_recovery(offer, window, cx);
                });
            }
            other => self.leftover_recovery = other,
        }
    }

    /// Continue with the remaining offers unless something else took over.
    fn queue_leftover_offers(&mut self, rest: VecDeque<LeftoverRecovery>, generation: Option<u64>) {
        if !rest.is_empty() && matches!(self.leftover_recovery, State::Idle) {
            self.leftover_recovery = State::Found {
                queue: rest,
                generation,
            };
        }
    }

    fn offer_leftover_recovery(
        &mut self,
        offer: Offer,
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
                        view.confirm_leftover_discard(offer, window, cx);
                    });
                });
                return;
            }
            let _ = this.update(cx, |view, cx| {
                let Offer {
                    found,
                    mut rest,
                    generation,
                } = offer;
                let mut next = generation;
                if answer != Some(0) {
                    view.status = "unsaved work kept for later".into();
                } else if !view.leftover_offer_current(generation)
                    || !view.leftover_offer_ready(generation)
                {
                    view.error = Some("project changed · recovery not opened".into());
                    rest.clear(); // later offers no longer apply either
                } else if !found.unchanged() {
                    view.error =
                        Some("recovery journal changed · open it from file > recover".into());
                } else {
                    match found.recover_source() {
                        Ok(source) => {
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
                            next = None;
                        }
                        Err(reason) => {
                            view.error = Some(
                                format!("could not recover: {reason} · unsaved work kept").into(),
                            );
                        }
                    }
                }
                view.queue_leftover_offers(rest, next);
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
        offer: Offer,
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
                let Offer {
                    found,
                    mut rest,
                    generation,
                } = offer;
                if answer != Some(0) {
                    view.status = "unsaved work kept for later".into();
                } else if !view.leftover_offer_current(generation) {
                    view.error = Some("project changed · unsaved work kept".into());
                    rest.clear(); // later offers no longer apply either
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
                view.queue_leftover_offers(rest, generation);
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "leftover_recovery_tests.rs"]
mod tests;
