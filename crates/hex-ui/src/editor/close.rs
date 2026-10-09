use super::EditorView;
use gpui::{AppContext, Context, PromptButton, PromptLevel, Window};

pub(super) fn requires_close_confirmation(
    unsaved: bool,
    worker_requests_pending: usize,
    local_requests_pending: usize,
    duplicate_pending: bool,
    opening_project: bool,
    export_pending: bool,
) -> bool {
    unsaved
        || worker_requests_pending > 0
        || local_requests_pending > 0
        || duplicate_pending
        || opening_project
        || export_pending
}

impl EditorView {
    pub(super) fn request_window_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.close_prompt_open {
            return false;
        }
        if self.requires_close_confirmation() {
            self.show_close_prompt(window, cx, false);
            false
        } else {
            self.stop();
            true
        }
    }

    pub(super) fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.close_prompt_open {
            return;
        }
        if self.requires_close_confirmation() {
            self.show_close_prompt(window, cx, true);
        } else {
            self.stop();
            cx.quit();
        }
    }

    fn requires_close_confirmation(&self) -> bool {
        requires_close_confirmation(
            self.state.as_ref().is_some_and(|state| state.unsaved),
            self.worker_requests_pending,
            self.pending_requests.len(),
            self.duplicate_intent.is_pending(),
            self.opening_project,
            self.export_pending,
        )
    }

    fn show_close_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>, quit: bool) {
        self.close_prompt_open = true;
        let answer = window.prompt(
            PromptLevel::Warning,
            "discard unsaved work?",
            Some(if self.export_pending {
                "pending edits or project changes will be discarded. an export may not finish."
            } else {
                "pending edits or project changes will be discarded."
            }),
            &[PromptButton::cancel("cancel"), PromptButton::new("discard")],
            cx,
        );
        // The window can retain its bootstrap handle type after replace_root.
        // Update the editor entity directly instead of downcasting that
        // handle, otherwise the dialog opens but its answer is never consumed.
        let handle = window.window_handle();
        // Keep the prompt's editor alive until its answer is handled. Closing
        // the confirmed window must not depend on a weak entity update succeeding.
        let editor = cx.entity();
        cx.spawn(async move |_, cx| {
            let discard = answer.await.ok() == Some(1);
            if let Err(error) = cx.update_window(handle, |_, window, cx| {
                editor.update(cx, |view, cx| {
                    view.close_prompt_open = false;
                    if discard {
                        view.stop();
                    } else {
                        cx.notify();
                    }
                });
                if discard {
                    window.remove_window();
                    if quit {
                        cx.quit();
                    }
                }
            }) {
                // A window that was already removed needs no second close.
                // Other lifecycle failures must be observable during diagnosis.
                eprintln!("hex: could not handle close prompt: {error}");
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::requires_close_confirmation;

    #[test]
    fn close_confirmation_covers_dirty_and_pending_work() {
        assert!(requires_close_confirmation(false, 0, 0, false, false, true));
        assert!(!requires_close_confirmation(
            false, 0, 0, false, false, false
        ));
        assert!(requires_close_confirmation(true, 0, 0, false, false, false));
        assert!(requires_close_confirmation(
            false, 1, 0, false, false, false
        ));
        assert!(requires_close_confirmation(
            false, 0, 1, false, false, false
        ));
        assert!(requires_close_confirmation(false, 0, 0, true, false, false));
        assert!(requires_close_confirmation(false, 0, 0, false, true, false));
    }
}

// Run with --features ui-test-support. These exercise the actual asynchronous
// prompt completion and window lifecycle without opening a user's project.
#[cfg(all(test, feature = "ui-test-support"))]
mod window_tests {
    use super::*;
    use gpui::{AnyWindowHandle, Empty, Entity, TestAppContext};
    use hex_session::service::{Service, ViewState};
    use std::sync::Arc;

    fn dirty_replaced_window(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<EditorView>) {
        let bootstrap = cx.add_window(|_, _| Empty);
        let handle = *bootstrap;
        let editor = cx
            .update_window(handle, |_, window, cx| {
                let service = Service::new(48_000, 32, super::super::MAX_RESIDENT_FRAMES)
                    .expect("disposable service");
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new(window, cx, service, Err("test offline output".into()))
                });
                super::super::install_close_guard(window, cx, editor.downgrade());
                editor.update(cx, |view, cx| {
                    view.state = Some(Arc::new(ViewState {
                        project: Arc::new(hex_project::Project::new(48_000).expect("test project")),
                        waveforms: Default::default(),
                        midi_preview: Arc::new(Default::default()),
                        timeline_frames: 384_000,
                        can_undo: false,
                        can_redo: false,
                        revision: 1,
                        unsaved: true,
                        document_path: None,
                    }));
                    cx.notify();
                });
                editor
            })
            .expect("bootstrap window");
        (handle, editor)
    }

    #[gpui::test]
    fn cancel_then_discard_closes_replaced_bootstrap_window(cx: &mut TestAppContext) {
        let (handle, editor) = dirty_replaced_window(cx);
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                assert!(!view.request_window_close(window, cx));
            });
        })
        .expect("dirty window");
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("cancel");
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1);
        editor.read_with(cx, |view, _| {
            assert!(!view.close_prompt_open);
            assert!(view.state.as_ref().is_some_and(|state| state.unsaved));
            assert_eq!(view.worker_requests_pending, 0);
        });
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                assert!(!view.request_window_close(window, cx));
                // A second close while the sheet is open must not create a second prompt.
                assert!(!view.request_window_close(window, cx));
            });
        })
        .expect("cancel preserved window");
        cx.simulate_prompt_answer("discard");
        cx.run_until_parked();
        assert!(!cx.has_pending_prompt());
        assert!(cx.windows().is_empty());
        assert!(cx.update_window(handle, |_, _, _| ()).is_err());
    }

    #[gpui::test]
    fn quit_cancel_then_discard_removes_confirmed_window(cx: &mut TestAppContext) {
        let (handle, editor) = dirty_replaced_window(cx);
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| view.request_quit(window, cx));
        })
        .expect("dirty window");
        cx.simulate_prompt_answer("cancel");
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1);
        assert!(!editor.read_with(cx, |view, _| view.close_prompt_open));
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| view.request_quit(window, cx));
        })
        .expect("cancel preserved window");
        cx.simulate_prompt_answer("discard");
        cx.run_until_parked();
        assert!(!cx.has_pending_prompt());
        assert!(cx.windows().is_empty());
    }
}
