use super::EditorView;
use gpui::{AppContext, Context, PromptButton, PromptLevel, Window};

pub(super) fn requires_close_confirmation(
    unsaved: bool,
    worker_requests_pending: usize,
    local_requests_pending: usize,
    duplicate_pending: bool,
    opening_project: bool,
    export_pending: bool,
    device_gesture_pending: bool,
) -> bool {
    unsaved
        || worker_requests_pending > 0
        || local_requests_pending > 0
        || duplicate_pending
        || opening_project
        || export_pending
        || device_gesture_pending
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
        if !self.prepare_theme_window_close(window, cx) {
            return false;
        }
        let _ = super::device_panel::finalize_controls(self, cx);
        self.finalize_device_gesture_for_close();
        if self.requires_close_confirmation(cx) {
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
        if !self.prepare_theme_window_close(window, cx) {
            return;
        }
        let _ = super::device_panel::finalize_controls(self, cx);
        self.finalize_device_gesture_for_close();
        if self.requires_close_confirmation(cx) {
            self.show_close_prompt(window, cx, true);
        } else {
            self.stop();
            cx.quit();
        }
    }

    fn requires_close_confirmation(&self, cx: &Context<Self>) -> bool {
        requires_close_confirmation(
            self.state.as_ref().is_some_and(|state| state.unsaved),
            self.worker_requests_pending,
            self.pending_requests.len(),
            self.duplicate_intent.is_pending(),
            self.opening_project,
            self.export_pending,
            self.device_gesture_pending() || self.device_panel.has_pending_draft(cx),
        )
    }

    fn finalize_device_gesture_for_close(&mut self) {
        if self.device_gesture.has_pending_terminal() {
            return;
        }
        if self.device_gesture.snapshot().is_some()
            && let Err(error) = self.device_gesture.finish_current()
        {
            self.error = Some(error.into());
        }
    }

    fn show_close_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>, quit: bool) {
        self.invalidate_prepared_seek();
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
        assert!(requires_close_confirmation(
            false, 0, 0, false, false, true, false
        ));
        assert!(!requires_close_confirmation(
            false, 0, 0, false, false, false, false
        ));
        assert!(requires_close_confirmation(
            true, 0, 0, false, false, false, false
        ));
        assert!(requires_close_confirmation(
            false, 1, 0, false, false, false, false
        ));
        assert!(requires_close_confirmation(
            false, 0, 1, false, false, false, false
        ));
        assert!(requires_close_confirmation(
            false, 0, 0, true, false, false, false
        ));
        assert!(requires_close_confirmation(
            false, 0, 0, false, true, false, false
        ));
        assert!(requires_close_confirmation(
            false, 0, 0, false, false, false, true
        ));
    }
}

// Run with --features ui-test-support. These exercise the actual asynchronous
// prompt completion and window lifecycle without opening a user's project.
#[cfg(all(test, feature = "ui-test-support"))]
mod window_tests {
    use super::*;
    use gpui::{
        AnyWindowHandle, Empty, Entity, MouseButton, MouseDownEvent, MouseUpEvent, TestAppContext,
        VisualTestContext, point,
    };
    use hex_session::service::{Event, Request, Service, ViewState};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn next_service_event(service: &Service) -> Event {
        let start = Instant::now();
        loop {
            match service.try_event() {
                Ok(event) => return event,
                Err(std::sync::mpsc::TryRecvError::Empty)
                    if start.elapsed() < Duration::from_secs(2) =>
                {
                    std::thread::yield_now();
                }
                Err(error) => panic!("project worker event unavailable: {error}"),
            }
        }
    }

    fn next_editor_service_event(editor: &Entity<EditorView>, cx: &mut TestAppContext) -> Event {
        let start = Instant::now();
        loop {
            match editor.read_with(cx, |view, _| view.service.try_event()) {
                Ok(event) => return event,
                Err(std::sync::mpsc::TryRecvError::Empty)
                    if start.elapsed() < Duration::from_secs(3) =>
                {
                    std::thread::yield_now();
                }
                Err(error) => panic!("project worker event unavailable: {error}"),
            }
        }
    }

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

    fn click_custom_close(visual: &mut VisualTestContext) {
        let bounds = visual
            .debug_bounds("window-control-close")
            .expect("custom close hitbox");
        let position = point(
            bounds.origin.x + bounds.size.width / 2.0,
            bounds.origin.y + bounds.size.height / 2.0,
        );
        visual.simulate_event(MouseDownEvent {
            position,
            modifiers: Default::default(),
            button: MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        });
        visual.simulate_event(MouseUpEvent {
            position,
            modifiers: Default::default(),
            button: MouseButton::Left,
            click_count: 1,
        });
    }

    #[gpui::test]
    fn custom_close_button_removes_a_clean_editor_window(cx: &mut TestAppContext) {
        if !cfg!(target_os = "macos") {
            return;
        }
        let (handle, editor) = dirty_replaced_window(cx);
        editor.update(cx, |view, cx| {
            if let Some(state) = view.state.as_ref() {
                view.state = Some(Arc::new(ViewState {
                    project: Arc::clone(&state.project),
                    waveforms: state.waveforms.clone(),
                    midi_preview: Arc::clone(&state.midi_preview),
                    timeline_frames: state.timeline_frames,
                    can_undo: state.can_undo,
                    can_redo: state.can_redo,
                    revision: state.revision,
                    unsaved: false,
                    document_path: state.document_path.clone(),
                }));
            }
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        click_custom_close(&mut visual);
        visual.run_until_parked();
        assert!(cx.windows().is_empty());
    }

    #[gpui::test]
    fn custom_close_button_keeps_cancelled_dirty_window_and_discards_on_second_close(
        cx: &mut TestAppContext,
    ) {
        if !cfg!(target_os = "macos") {
            return;
        }
        let (handle, _) = dirty_replaced_window(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        click_custom_close(&mut visual);
        assert!(cx.has_pending_prompt());
        click_custom_close(&mut visual);
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("cancel");
        cx.run_until_parked();
        assert!(!cx.has_pending_prompt());
        assert_eq!(cx.windows().len(), 1);

        visual.run_until_parked();
        click_custom_close(&mut visual);
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("discard");
        cx.run_until_parked();
        assert!(cx.windows().is_empty());
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

    #[gpui::test]
    fn clean_base_device_draft_is_committed_before_cancel_and_discard_prompt(
        cx: &mut TestAppContext,
    ) {
        let bootstrap = cx.add_window(|_, _| Empty);
        let handle = *bootstrap;
        let service = Service::new(48_000, 32, super::super::MAX_RESIDENT_FRAMES)
            .expect("disposable service");
        let _ = next_service_event(&service);
        service
            .submit(Request::Edit(hex_project::Command::AddTrack {
                name: "device draft".into(),
            }))
            .expect("add track");
        let track = next_service_event(&service).state.project.tracks()[0].id();
        let type_id =
            hex_session::device_gesture::DeviceTypeId::new("hex.utility").expect("type id");
        service
            .submit(Request::AddNativeDevice {
                track,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: Box::new(type_id),
                before: None,
            })
            .expect("add utility");
        let device_event = next_service_event(&service);
        let device = device_event.state.project.tracks()[0].devices()[0].id();
        let path =
            std::env::temp_dir().join(format!("hex-clean-device-draft-{}.hex", std::process::id()));
        service
            .submit(Request::SaveNew(path.clone()))
            .expect("save clean base");
        let device_event = next_service_event(&service);
        assert!(!device_event.state.unsaved);
        let editor = cx
            .update_window(handle, |_, window, cx| {
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new(window, cx, service, Err("test offline output".into()))
                });
                super::super::install_close_guard(window, cx, editor.downgrade());
                editor.update(cx, |view, cx| {
                    let state = &device_event.state;
                    view.state = Some(Arc::new(ViewState {
                        project: Arc::clone(&state.project),
                        waveforms: state.waveforms.clone(),
                        midi_preview: Arc::clone(&state.midi_preview),
                        timeline_frames: state.timeline_frames,
                        can_undo: false,
                        can_redo: false,
                        revision: state.revision,
                        unsaved: false,
                        document_path: None,
                    }));
                    view.begin_device_gesture(
                        hex_session::device_gesture::GestureTarget::DeviceParameter {
                            device,
                            parameter: hex_session::device_gesture::ParameterId::new(1)
                                .expect("gain parameter"),
                        },
                        hex_session::device_gesture::ParameterValue::Float(1.0),
                    )
                    .expect("begin clean-base gesture");
                    view.change_device_gesture(hex_session::device_gesture::ParameterValue::Float(
                        2.0,
                    ))
                    .expect("draft parameter change");
                    cx.notify();
                });
                editor
            })
            .expect("device editor window");
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                assert!(!view.request_window_close(window, cx));
            });
        })
        .expect("close with draft");
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("cancel");
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1);
        editor.read_with(cx, |view, _| {
            assert!(view.state.as_ref().is_some_and(|state| !state.unsaved));
            let draft = view.device_gesture.snapshot().expect("cancel keeps draft");
            assert_eq!(
                draft.target,
                hex_session::device_gesture::GestureTarget::DeviceParameter {
                    device,
                    parameter: hex_session::device_gesture::ParameterId::new(1)
                        .expect("gain parameter"),
                }
            );
            assert_eq!(
                draft.value,
                hex_session::device_gesture::ParameterValue::Float(2.0)
            );
        });
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                assert!(!view.request_window_close(window, cx));
            });
        })
        .expect("second close");
        cx.simulate_prompt_answer("discard");
        cx.run_until_parked();
        assert!(cx.windows().is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[gpui::test]
    fn failed_gesture_compile_refuses_deferred_save_and_export(cx: &mut TestAppContext) {
        let service = Service::new(48_000, 64, super::super::MAX_RESIDENT_FRAMES)
            .expect("disposable service");
        let _ = next_service_event(&service);
        service
            .submit(Request::Edit(hex_project::Command::AddTrack {
                name: "compile-limited chain".into(),
            }))
            .expect("add track");
        let mut state = next_service_event(&service).state;
        let track = state.project.tracks()[0].id();
        let type_id =
            hex_session::device_gesture::DeviceTypeId::new("hex.utility").expect("type id");
        let mut devices = Vec::new();
        for _ in 0..24 {
            service
                .submit(Request::AddNativeDevice {
                    track,
                    class: hex_engine::NativeDeviceClass::Effect,
                    type_id: Box::new(type_id),
                    before: None,
                })
                .expect("add utility");
            let event = next_service_event(&service);
            devices.push(
                event.state.project.tracks()[0]
                    .devices()
                    .last()
                    .unwrap()
                    .id(),
            );
            state = event.state;
        }
        let gain = hex_session::device_gesture::ParameterId::new(1).expect("gain parameter");
        let invalid_value = hex_session::device_gesture::ParameterValue::Float(56.234_13);
        let mut rejected_device = None;
        for device in devices {
            service
                .submit(Request::SetDeviceParameter {
                    device,
                    parameter: gain,
                    value: invalid_value,
                })
                .expect("probe device capacity");
            let event = next_service_event(&service);
            if event.error.is_some() {
                rejected_device = Some(device);
                state = event.state;
                break;
            }
            state = event.state;
        }
        let device = rejected_device.expect("dense chain rejects one candidate");
        let save_path =
            std::env::temp_dir().join(format!("hex-refused-gesture-{}.hex", std::process::id()));
        let export_path =
            std::env::temp_dir().join(format!("hex-refused-gesture-{}.wav", std::process::id()));
        let _ = std::fs::remove_file(&save_path);
        let _ = std::fs::remove_file(&export_path);

        let bootstrap = cx.add_window(|_, _| Empty);
        let handle = *bootstrap;
        let editor = cx
            .update_window(handle, |_, window, cx| {
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new(window, cx, service, Err("test offline output".into()))
                });
                editor.update(cx, |view, cx| {
                    view.state = Some(Arc::clone(&state));
                    view.begin_device_gesture(
                        hex_session::device_gesture::GestureTarget::DeviceParameter {
                            device,
                            parameter: gain,
                        },
                        hex_session::device_gesture::ParameterValue::Float(1.0),
                    )
                    .expect("begin parameter gesture");
                    cx.notify();
                });
                editor
            })
            .expect("device editor window");

        editor.update(cx, |view, _| assert!(view.pump_device_gesture()));
        let begin = next_editor_service_event(&editor, cx);
        let begin_ack = begin.gesture_ack.expect("begin acknowledgement");
        assert!(begin_ack.accepted);
        editor.update(cx, |view, _| {
            assert!(view.acknowledge_device_gesture(&begin_ack))
        });

        editor.update(cx, |view, _| {
            view.change_device_gesture(invalid_value)
                .expect("stage invalid final value");
            assert!(view.pump_device_gesture());
        });
        let preview = next_editor_service_event(&editor, cx);
        let preview_ack = preview.gesture_ack.expect("preview acknowledgement");
        assert!(!preview_ack.accepted);
        editor.update(cx, |view, _| {
            assert!(view.acknowledge_device_gesture(&preview_ack))
        });

        editor.update(cx, |view, _| {
            assert!(view.enqueue_device_barrier(Request::SaveNew(save_path.clone())));
            assert!(view.enqueue_device_barrier(Request::Export(export_path.clone())));
            view.saving_project = true;
            view.export_pending = true;
            assert!(view.pump_device_gesture());
        });
        let commit = next_editor_service_event(&editor, cx);
        let commit_ack = commit.gesture_ack.expect("commit acknowledgement");
        assert!(!commit_ack.accepted);
        editor.update(cx, |view, _| {
            assert!(view.acknowledge_device_gesture(&commit_ack));
            assert!(!view.saving_project);
            assert!(!view.export_pending);
            assert!(view.device_gesture_barriers.is_empty());
            assert!(view.pending_requests.is_empty());
            assert!(view.error.is_some());
        });

        assert!(
            !save_path.exists(),
            "failed gesture must not save old value"
        );
        assert!(
            !export_path.exists(),
            "failed gesture must not export old value"
        );
        assert!(matches!(
            editor.read_with(cx, |view, _| view.service.try_export_event()),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        cx.update_window(handle, |_, window, _| window.remove_window())
            .expect("remove test window");
    }
}
