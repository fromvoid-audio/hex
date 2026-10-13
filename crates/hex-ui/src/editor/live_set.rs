use super::EditorView;
use gpui::{AppContext, Context, PathPromptOptions, PromptButton, PromptLevel, Window};
use hex_session::service::Request;

impl EditorView {
    pub(super) fn live_set_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !super::device_panel::finalize_controls(self, cx) {
            return;
        }
        if self.opening_project
            || self.close_prompt_open
            || self.save_dialog_open
            || self.saving_project
            || self.edit_pending()
            || self.export_pending
        {
            self.error = Some("finish the pending project operation before importing".into());
            cx.notify();
            return;
        }
        let Some(state) = self.state.as_ref() else {
            self.error = Some("project is still loading".into());
            cx.notify();
            return;
        };
        if state.unsaved {
            self.error = Some("save current work before importing a live set".into());
            cx.notify();
            return;
        }
        self.opening_project = true;
        let answer = window.prompt(PromptLevel::Warning, "import live set as a new project?", Some("the current saved project will be replaced in this window. only the supported arrangement subset is imported. live pan sound is unverified; choose hex balance only if you accept that difference. unsupported musical behavior is refused."), &[PromptButton::cancel("cancel"), PromptButton::new("import · neutral pan only"), PromptButton::new("import · accept hex balance")], cx);
        let handle = window.window_handle();
        let editor = cx.entity();
        cx.spawn(async move |_, cx| {
            let choice = answer.await.ok();
            let _ = cx.update_window(handle, |_, _, cx| {
                editor.update(cx, |view, cx| {
                    if matches!(choice, Some(1 | 2)) {
                        view.live_set_picker(choice == Some(2), cx);
                    } else {
                        view.opening_project = false;
                    }
                    cx.notify();
                });
            });
        })
        .detach();
        cx.notify();
    }

    fn live_set_picker(&mut self, accept_hex_balance: bool, cx: &mut Context<Self>) {
        let Some(state) = self.state.as_ref() else {
            self.opening_project = false;
            return;
        };
        let revision = state.revision;
        let project_generation = self.project_generation;
        let response = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("import live set · choose .als".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = response.await;
            let _ = this.update(cx, |view, cx| {
                view.opening_project = false;
                match picked {
                    Ok(Ok(Some(mut paths))) if !paths.is_empty() => {
                        if let Some(path) = paths.pop() {
                            if view.project_generation != project_generation {
                                view.error = Some(
                                    "live-set import cancelled because project changed".into(),
                                );
                            } else if view.enqueue(Request::ImportLiveSet {
                                path,
                                accept_hex_balance,
                                project_generation,
                                revision,
                            }) {
                                view.opening_project = true;
                                view.pending_audio_commands.clear();
                                view.pending_seek_ticket = None;
                                view.pending_session_ticket = None;
                                view.play_intent.clear();
                                view.cancel_duplicate_chain();
                                view.status = "importing live set · editing paused".into();
                            }
                        }
                    }
                    Ok(Err(_)) => view.error = Some("live-set file picker failed".into()),
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use hex_session::service::Service;
    use std::time::{Duration, Instant};

    #[gpui::test]
    fn consent_cancel_preserves_project_and_dirty_work_refuses_import(cx: &mut TestAppContext) {
        let service = Service::new(48_000, 8, super::super::MAX_RESIDENT_FRAMES).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let initial = loop {
            if let Ok(event) = service.try_event() {
                break event;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        };
        let original = initial.state.project.clone();
        let window = cx.add_window(|window, cx| {
            let mut view = EditorView::new(window, cx, service, Err("offline test".into()));
            view.state = Some(initial.state);
            view
        });
        window
            .update(cx, |view, window, cx| view.live_set_dialog(window, cx))
            .unwrap();
        assert!(cx.has_pending_prompt());
        window
            .update(cx, |view, _, _| assert!(view.opening_project))
            .unwrap();
        cx.simulate_prompt_answer("cancel");
        cx.run_until_parked();
        window
            .update(cx, |view, window, cx| {
                assert!(!view.opening_project);
                assert_eq!(view.state.as_ref().unwrap().project, original);
                let state = view.state.as_ref().unwrap();
                view.state = Some(std::sync::Arc::new(hex_session::service::ViewState {
                    project: state.project.clone(),
                    waveforms: state.waveforms.clone(),
                    midi_preview: state.midi_preview.clone(),
                    gain_preview: Ok(std::sync::Arc::new(
                        hex_engine::GainAutomationPreview::default(),
                    )),
                    timeline_frames: state.timeline_frames,
                    can_undo: state.can_undo,
                    can_redo: state.can_redo,
                    revision: state.revision,
                    unsaved: true,
                    document_path: None,
                }));
                view.live_set_dialog(window, cx);
                assert!(!view.opening_project);
                assert!(view.error.as_ref().unwrap().contains("save current work"));
            })
            .unwrap();
        assert!(!cx.has_pending_prompt());
    }
}
