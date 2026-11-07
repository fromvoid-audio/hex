use super::*;

struct BrowserDirectory(std::path::PathBuf);
impl BrowserDirectory {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hex-device-keys-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::create_dir(path.join("samples")).unwrap();
        Self(path)
    }
    fn window(
        &self,
        cx: &mut TestAppContext,
        service: Service,
        state: Arc<ViewState>,
        track: TrackId,
    ) -> (gpui::AnyWindowHandle, Entity<EditorView>) {
        let handle = *cx.add_window(|_, _| gpui::Empty);
        let editor = cx
            .update_window(handle, |_, window, cx| {
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new_for_test_with_browser_directory(window, cx, service, &self.0)
                });
                editor.update(cx, |view, cx| {
                    assert_eq!(view.browser.directory, self.0.join("samples"));
                    assert_eq!(view.browser.places.clone(), vec![self.0.join("samples")]);
                    view.state = Some(state);
                    view.selected_track = Some(track);
                    view.device_panel_active = true;
                    view.clip_view_open = true;
                    view.mode = super::super::super::EditorMode::Session;
                    cx.notify();
                });
                editor
            })
            .unwrap();
        (handle, editor)
    }
}
impl Drop for BrowserDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn title(visual: &mut VisualTestContext, device: DeviceId) -> gpui::Point<gpui::Pixels> {
    visual
        .debug_bounds(Box::leak(
            format!("device-drag-title-{}", device.value()).into_boxed_str(),
        ))
        .expect("device title")
        .center()
}
fn settled(view: &EditorView) -> bool {
    view.worker_requests_pending == 0
        && view.pending_requests.is_empty()
        && view.device_gesture.snapshot().is_none()
        && view.device_gesture_barriers.is_empty()
}

#[gpui::test]
fn device_header_backspace_keeps_real_keyboard_undo_redo_and_focus_after_reconcile(
    cx: &mut TestAppContext,
) {
    device_delete_key_history(cx, "backspace");
}

#[gpui::test]
fn device_header_x_keeps_real_keyboard_undo_redo_and_focus_after_reconcile(
    cx: &mut TestAppContext,
) {
    device_delete_key_history(cx, "x");
}

fn device_delete_key_history(cx: &mut TestAppContext, key: &str) {
    let browser = BrowserDirectory::new();
    cx.update(super::super::super::bind_editor_keys);
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let original = state.project.clone();
    let (handle, editor) = browser.window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    let point = title(&mut visual, device);
    visual.simulate_click(point, Modifiers::default());
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse(key).unwrap(),
        is_held: false,
    });
    pump_until(&editor, cx, |view| {
        settled(view)
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .is_none()
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert!(project.track(track).is_some());
        assert_eq!(
            project.track(track).unwrap().instrument(),
            original.track(track).unwrap().instrument()
        );
        assert!(view.device_panel.selected_device.is_none());
        assert!(!view.device_panel.views.contains_key(&device));
        assert!(!view.device_panel.has_control_for(device));
        assert!(
            view.device_panel.message.is_none(),
            "settled normal removal is not a stale-target warning"
        );
    });
    visual.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse(key).unwrap(),
    });
    let focus_after_removal = visual.update(|window, cx| window.focused(cx).is_some());
    visual.simulate_keystrokes("cmd-z");
    assert!(
        focus_after_removal,
        "focused device removal blurred the window before real Cmd-Z"
    );
    pump_until(&editor, cx, |view| {
        settled(view)
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .is_some()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original)
    });
    // Restoring a card never makes a still-held key remove it again.
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse(key).unwrap(),
        is_held: true,
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original)
    });
    visual.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse(key).unwrap(),
    });
    editor.update(cx, |view, _| view.last_transport_command = None);
    visual.simulate_keystrokes("space");
    editor.read_with(cx, |view, _| {
        assert!(
            view.last_transport_command.is_some(),
            "real Space reaches transport"
        )
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump_until(&editor, cx, |view| {
        settled(view)
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .is_none()
    });
}

#[gpui::test]
fn device_header_backspace_removes_instrument_without_touching_effect_or_track(
    cx: &mut TestAppContext,
) {
    let browser = BrowserDirectory::new();
    cx.update(super::super::super::bind_editor_keys);
    let (service, state, track, effect, _, _) = actor_utility_fixture();
    let original = state.project.clone();
    let instrument = original.track(track).unwrap().instrument().unwrap().id();
    let (handle, editor) = browser.window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    let point = title(&mut visual, instrument);
    visual.simulate_click(point, Modifiers::default());
    visual.simulate_keystrokes("backspace");
    pump_until(&editor, cx, |view| {
        settled(view)
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .instrument()
                .is_none()
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().project.device(effect),
            original.device(effect)
        );
        assert!(view.device_panel.message.is_none());
    });
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("instrument-device-{}", instrument.value()).into_boxed_str()
            ))
            .is_none()
    );
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view)
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .device(instrument)
                .is_some()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original)
    });
}

#[gpui::test]
fn device_header_backspace_removes_return_effect_with_one_undo(cx: &mut TestAppContext) {
    let browser = BrowserDirectory::new();
    cx.update(super::super::super::bind_editor_keys);
    let (service, state, track, return_id, device, _) = actor_return_utility_fixture();
    let original = state.project.clone();
    let (handle, editor) = browser.window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.focused_return = Some(return_id);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    let point = title(&mut visual, device);
    visual.simulate_click(point, Modifiers::default());
    visual.simulate_keystrokes("backspace");
    pump_until(&editor, cx, |view| {
        settled(view)
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .is_none()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().project.track(track),
            original.track(track)
        )
    });
    visual.run_until_parked();
    assert!(visual.update(|window, cx| window.focused(cx).is_some()));
    visual.simulate_keystrokes("cmd-z");
    pump_until(&editor, cx, |view| {
        settled(view)
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .is_some()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original)
    });
}

#[gpui::test]
fn device_backspace_preserves_numeric_and_browser_text_focus(cx: &mut TestAppContext) {
    let browser = BrowserDirectory::new();
    cx.update(super::super::super::bind_editor_keys);
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let original = state.project.clone();
    let (handle, editor) = browser.window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    let point = title(&mut visual, device);
    visual.simulate_click(point, Modifiers::default());
    let field = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-1-value", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("0.5");
    visual.simulate_keystrokes("backspace escape");
    pump_until(&editor, cx, settled);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original)
    });
    let search = visual.debug_bounds("browser-search").unwrap();
    visual.simulate_click(search.center(), Modifiers::default());
    visual.simulate_input("abc");
    visual.simulate_keystrokes("backspace");
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .is_some()
        );
        assert_eq!(view.browser.search.read(cx).text(), "ab");
        assert!(settled(view));
    });
}

#[gpui::test]
fn device_header_backspace_finalizes_valid_draft_before_removal(cx: &mut TestAppContext) {
    let browser = BrowserDirectory::new();
    cx.update(super::super::super::bind_editor_keys);
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let original = state.project.clone();
    let (handle, editor) = browser.window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    let field = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-1-value", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("6.00 db");
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        let ParameterControl::Float { entity, .. } = &view.device_panel.controls[&ParameterKey {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(1),
        }] else {
            panic!("float")
        };
        assert!(entity.read(cx).is_editing(), "numeric draft owns focus");
    });
    let point = title(&mut visual, device);
    visual.simulate_click(point, Modifiers::default());
    let expected = editor.read_with(cx, |view, cx| {
        let ParameterControl::Float { entity, .. } = &view.device_panel.controls[&ParameterKey {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(1),
        }] else {
            panic!("float")
        };
        let value = entity.read(cx).value() as f32;
        assert!((value - 10.0_f32.powf(6.0 / 20.0)).abs() < 0.0001);
        value
    });
    visual.simulate_keystrokes("backspace");
    pump_until(&editor, cx, |view| {
        settled(view)
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .is_none()
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view)
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .is_some()
    });
    editor.read_with(cx, |view, _| {
        let state = view
            .state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .state();
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                NativeDeviceClass::Effect,
                state,
                hex_lattice_contract::ParameterId::from_static(1)
            )
            .unwrap(),
            ParameterValue::Float(expected)
        );
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original)
    });
}

#[gpui::test]
fn device_header_backspace_repeat_does_not_delete_a_selected_clip_or_another_device(
    cx: &mut TestAppContext,
) {
    let browser = BrowserDirectory::new();
    cx.update(super::super::super::bind_editor_keys);
    let (service, _, track, device, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::ONE,
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::Clip(clip) = event.created.unwrap() else {
        panic!("clip")
    };
    let (handle, editor) = browser.window(cx, service, event.state, track);
    editor.update(cx, |view, cx| {
        view.mode = super::super::super::EditorMode::Arrangement;
        view.selected_clip = Some(clip);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    let point = title(&mut visual, device);
    visual.simulate_click(point, Modifiers::default());
    visual.simulate_keystrokes("backspace");
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("backspace").unwrap(),
        is_held: true,
    });
    pump_until(&editor, cx, |view| {
        settled(view)
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .is_none()
    });
    visual.run_until_parked();
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("backspace").unwrap(),
        is_held: true,
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert!(
            project
                .track(track)
                .unwrap()
                .midi_clips()
                .iter()
                .any(|item| item.id() == clip)
        );
        assert!(project.track(track).unwrap().instrument().is_some());
        assert!(settled(view));
    });
}

#[gpui::test]
fn device_backspace_refuses_pending_stale_and_empty_selection_without_falling_through(
    cx: &mut TestAppContext,
) {
    let browser = BrowserDirectory::new();
    cx.update(super::super::super::bind_editor_keys);
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let original = state.project.clone();
    let (handle, editor) = browser.window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let point = title(&mut visual, device);
    visual.simulate_click(point, Modifiers::default());
    editor.update(cx, |view, _| view.worker_requests_pending = 1);
    visual.simulate_keystrokes("backspace");
    editor.update(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original);
        view.worker_requests_pending = 0;
        view.device_panel.selected_device = None;
    });
    visual.simulate_keystrokes("backspace");
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original)
    });
    editor.update(cx, |view, _| {
        view.device_panel.selected_device = Some(device);
        view.project_generation += 1;
    });
    visual.simulate_keystrokes("backspace");
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original)
    });
}

#[gpui::test]
fn device_backspace_keeps_eq_graph_removal_local(cx: &mut TestAppContext) {
    let browser = BrowserDirectory::new();
    cx.update(super::super::super::bind_editor_keys);
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    let _ = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "eq keyboard".into(),
        }))
        .unwrap();
    let Created::Track(track) = receive_service_event(&service).created.unwrap() else {
        panic!("track")
    };
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(hex_engine::eq::EQ_TYPE_ID),
            before: None,
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let device = state.project.track(track).unwrap().devices()[0].id();
    let (handle, editor) = browser.window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    let header = title(&mut visual, device);
    visual.simulate_click(header, Modifiers::default());
    let graph = visual
        .debug_bounds(Box::leak(
            format!("eq-response-{}", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(graph.center(), Modifiers::default());
    pump_until(&editor, cx, |view| {
        settled(view)
            && hex_engine::eq::EqState::decode(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .device(device)
                    .unwrap()
                    .state(),
            )
            .unwrap()
            .nodes
            .len()
                == 1
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("0");
    editor.read_with(cx, |view, _| {
        assert!(
            !view
                .state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .bypassed()
        );
    });
    pump_until(&editor, cx, settled);
    let before_modal = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    editor.update(cx, |view, _| view.close_prompt_open = true);
    visual.simulate_keystrokes("x");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(settled(view), "a modal must not enqueue node removal");
        assert_eq!(
            view.state.as_ref().unwrap().project.snapshot(),
            before_modal
        );
    });
    editor.update(cx, |view, _| view.close_prompt_open = false);
    visual.simulate_keystrokes("x");
    editor.read_with(cx, |view, _| {
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .is_some(),
            "a focused graph must not route x to device removal"
        );
    });
    visual.simulate_keystrokes("backspace");
    pump_until(&editor, cx, |view| {
        settled(view)
            && hex_engine::eq::EqState::decode(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .device(device)
                    .expect("graph must not remove device")
                    .state(),
            )
            .unwrap()
            .nodes
            .is_empty()
    });
    visual.simulate_keystrokes("backspace");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .is_some()
        );
        assert!(view.state.as_ref().unwrap().project.track(track).is_some());
    });
}

#[gpui::test]
fn device_backspace_preserves_invalid_numeric_draft_and_device(cx: &mut TestAppContext) {
    let browser = BrowserDirectory::new();
    cx.update(super::super::super::bind_editor_keys);
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let original = state.project.clone();
    let (handle, editor) = browser.window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    enter_typed_value(
        &mut visual,
        device,
        hex_lattice_contract::ParameterId::from_static(1),
        "invalid",
    );
    let header = title(&mut visual, device);
    visual.simulate_click(header, Modifiers::default());
    visual.simulate_keystrokes("backspace");
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.state.as_ref().unwrap().project, original);
        let ParameterControl::Float { entity, .. } = &view.device_panel.controls[&ParameterKey {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(1),
        }] else {
            panic!("float")
        };
        assert!(entity.read(cx).is_editing());
        assert!(settled(view));
    });
}

#[gpui::test]
fn device_delete_review_refused_press_cannot_turn_into_held_removal(cx: &mut TestAppContext) {
    refused_device_delete_key(cx, "backspace");
}

#[gpui::test]
fn device_x_refusal_survives_other_key_release_and_repeat(cx: &mut TestAppContext) {
    refused_device_delete_key(cx, "x");
}

fn refused_device_delete_key(cx: &mut TestAppContext, key: &str) {
    let browser = BrowserDirectory::new();
    cx.update(super::super::super::bind_editor_keys);
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let (handle, editor) = browser.window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    let point = title(&mut visual, device);
    visual.simulate_click(point, Modifiers::default());
    // Real actor work is pending at the first event, not a forged view counter.
    // Dispatch before leaving this window update so its completion cannot drain first.
    visual.update(|window, cx| {
        editor.update(cx, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::RenameTrack {
                track,
                name: "completed while key held".into(),
            })));
            assert!(view.worker_requests_pending > 0);
        });
        window.dispatch_input(
            gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                keystroke: gpui::Keystroke::parse(key).unwrap(),
                is_held: false,
            }),
            cx,
        );
        editor.read_with(cx, |view, _| {
            assert!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .device(device)
                    .is_some()
            );
            assert_eq!(
                view.error.as_deref(),
                Some("finish pending edits before removing the selected device")
            );
        });
    });
    pump_until(&editor, cx, settled);
    visual.run_until_parked();
    let after_edit = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    if key == "x" {
        // A release of a different key cannot retire x's refusal custody.
        visual.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("backspace").unwrap(),
        });
    }
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse(key).unwrap(),
        is_held: true,
    });
    pump_until(&editor, cx, settled);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().project,
            after_edit,
            "refused first press must remain refused while held after pending work completes"
        );
    });
    visual.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse(key).unwrap(),
    });
    visual.simulate_keystrokes(key);
    pump_until(&editor, cx, |view| {
        settled(view)
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .is_none()
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-z");
    pump_until(&editor, cx, |view| {
        settled(view)
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .is_some()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, after_edit)
    });
}

fn pointer_remove_retains_keyboard(cx: &mut TestAppContext, kind: u8) {
    let browser = BrowserDirectory::new();
    cx.update(super::super::super::bind_editor_keys);
    let (service, state, track, return_id, device) = if kind == 2 {
        let (service, state, track, owner, device, _) = actor_return_utility_fixture();
        (service, state, track, Some(owner), device)
    } else {
        let (service, state, track, effect, _, _) = actor_utility_fixture();
        let device = if kind == 1 {
            state
                .project
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .id()
        } else {
            effect
        };
        (service, state, track, None, device)
    };
    let original = state.project.clone();
    let (handle, editor) = browser.window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.focused_return = return_id;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    let point = title(&mut visual, device);
    visual.simulate_click(point, Modifiers::default());
    let prefix = match kind {
        0 => "device-remove",
        1 => "instrument-remove",
        _ => "return-device-remove",
    };
    let remove = visual
        .debug_bounds(Box::leak(
            format!("{prefix}-{}", device.value()).into_boxed_str(),
        ))
        .expect("remove control");
    visual.simulate_click(remove.center(), Modifiers::default());
    pump_until(&editor, cx, |view| {
        let project = &view.state.as_ref().unwrap().project;
        settled(view)
            && if kind == 1 {
                project.track(track).unwrap().instrument().is_none()
            } else {
                project.device(device).is_none()
            }
    });
    editor.read_with(cx, |view, _| {
        let mut expected_sources = original.snapshot().native_sources;
        if kind == 1 {
            let source_id = original.track(track).unwrap().source_id().unwrap().value();
            let source = expected_sources
                .iter_mut()
                .find(|source| source.id == source_id)
                .expect("instrument source remains in the inventory");
            assert_eq!(source.receiver, Some(track.value()));
            source.receiver = None;
        }
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .snapshot()
                .native_sources,
            expected_sources
        );
    });
    visual.run_until_parked();
    let focused = visual.update(|window, cx| window.focused(cx).is_some());
    visual.simulate_keystrokes("cmd-z");
    assert!(
        focused,
        "pointer X retired the focused card and blurred the editor before real Cmd-Z"
    );
    pump_until(&editor, cx, |view| {
        settled(view) && view.state.as_ref().unwrap().project == original
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original)
    });
    editor.update(cx, |view, _| view.last_transport_command = None);
    visual.simulate_keystrokes("space");
    editor.read_with(cx, |view, _| {
        assert!(
            view.last_transport_command.is_some(),
            "real Space remains routed to transport"
        )
    });
}

#[gpui::test]
fn device_delete_review_pointer_track_effect_keeps_keyboard(cx: &mut TestAppContext) {
    pointer_remove_retains_keyboard(cx, 0);
}
#[gpui::test]
fn device_delete_review_pointer_instrument_keeps_keyboard(cx: &mut TestAppContext) {
    pointer_remove_retains_keyboard(cx, 1);
}
#[gpui::test]
fn device_delete_review_pointer_return_keeps_keyboard(cx: &mut TestAppContext) {
    pointer_remove_retains_keyboard(cx, 2);
}

fn refused_release_respects_new_focus(cx: &mut TestAppContext, text: bool) {
    let browser = BrowserDirectory::new();
    cx.update(super::super::super::bind_editor_keys);
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let instrument = state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let (handle, editor) = browser.window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    let point = title(&mut visual, device);
    visual.simulate_click(point, Modifiers::default());
    visual.update(|window, cx| {
        editor.update(cx, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::RenameTrack {
                track,
                name: "release focus guard".into(),
            })))
        });
        window.dispatch_input(
            gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                keystroke: gpui::Keystroke::parse("backspace").unwrap(),
                is_held: false,
            }),
            cx,
        );
    });
    pump_until(&editor, cx, settled);
    visual.run_until_parked();
    if text {
        visual.update(|window, cx| {
            editor.read_with(cx, |view, cx| view.browser.search.read(cx).focus(window))
        });
        visual.simulate_input("keep");
    } else {
        let point = title(&mut visual, instrument);
        visual.simulate_click(point, Modifiers::default());
    }
    visual.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("backspace").unwrap(),
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.read_with(cx, |view, cx| {
            assert!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .device(device)
                    .is_some()
            );
            if text {
                assert!(
                    view.browser
                        .search
                        .read(cx)
                        .focus_handle()
                        .is_focused(window)
                );
            } else {
                assert_eq!(view.device_panel.selected_device, Some(instrument));
                assert!(
                    view.device_panel
                        .chain_window
                        .device_is_focused(instrument, window)
                );
            }
        })
    });
    if text {
        visual.simulate_keystrokes("backspace");
        editor.read_with(cx, |view, cx| {
            assert_eq!(view.browser.search.read(cx).text(), "kee");
            assert!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .device(device)
                    .is_some()
            );
        });
        let point = title(&mut visual, device);
        visual.simulate_click(point, Modifiers::default());
        visual.simulate_keystrokes("backspace");
        pump_until(&editor, cx, settled);
        editor.read_with(cx, |view, _| {
            assert!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .device(device)
                    .is_none(),
                "release in text followed by a fresh card press must not retain a stale refusal"
            )
        });
    }
}
#[gpui::test]
fn device_delete_release_preserves_new_text_focus(cx: &mut TestAppContext) {
    refused_release_respects_new_focus(cx, true);
}
#[gpui::test]
fn device_delete_release_does_not_rearm_old_card_after_selection_changes(cx: &mut TestAppContext) {
    refused_release_respects_new_focus(cx, false);
}

#[gpui::test]
fn device_zero_rapid_toggles_remain_ordered_before_worker_responses(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let point = title(&mut visual, device);
    visual.simulate_click(point, Modifiers::default());
    visual.simulate_keystrokes("c");
    visual.simulate_keystrokes("c");
    editor.read_with(cx, |view, _| {
        assert_eq!(view.worker_requests_pending, 2);
        assert!(view.error.is_none());
    });
    super::header_actions_tests::assert_power_paint(&mut visual, device, true);
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, _| {
        assert!(
            !view
                .state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .bypassed()
        );
    });
    super::header_actions_tests::assert_power_paint(&mut visual, device, true);
}

#[gpui::test]
fn device_zero_toggles_focused_effect_and_instrument_with_history_and_save(
    cx: &mut TestAppContext,
) {
    let browser = BrowserDirectory::new();
    cx.update(super::super::super::bind_editor_keys);
    let (service, _, track, effect, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::ONE,
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::Clip(clip) = event.created.unwrap() else {
        panic!("clip")
    };
    let state = event.state;
    let original = state.project.clone();
    let instrument = original.track(track).unwrap().instrument().unwrap().id();
    let (handle, editor) = browser.window(cx, service, state, track);
    editor.update(cx, |view, _| view.selected_clip = Some(clip));
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    for device in [effect, instrument] {
        let point = title(&mut visual, device);
        visual.simulate_click(point, Modifiers::default());
        visual.simulate_keystrokes("a");
        pump_until(&editor, cx, settled);
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, original);
            assert!(
                !view.automation_view.enabled,
                "a focused card keeps its own context"
            );
        });
        visual.simulate_keystrokes("c");
        pump_until(&editor, cx, settled);
        let changed = editor.read_with(cx, |view, _| {
            let actual = &view.state.as_ref().unwrap().project;
            let mut expected = Editor::new((*original).clone(), 8).unwrap();
            expected
                .execute(Command::SetDeviceBypass {
                    device,
                    bypassed: true,
                })
                .unwrap();
            assert_eq!(
                actual.as_ref(),
                expected.project(),
                "0 toggles only the focused device"
            );
            assert_eq!(view.device_panel.selected_device, Some(device));
            assert_eq!(view.selected_clip, Some(clip));
            actual.clone()
        });
        visual.run_until_parked();
        super::header_actions_tests::assert_power_paint(&mut visual, device, false);
        visual.simulate_keystrokes("cmd-z");
        pump_until(&editor, cx, settled);
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, original)
        });
        visual.run_until_parked();
        super::header_actions_tests::assert_power_paint(&mut visual, device, true);
        visual.simulate_keystrokes("cmd-shift-z");
        pump_until(&editor, cx, settled);
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, changed)
        });
        visual.run_until_parked();
        super::header_actions_tests::assert_power_paint(&mut visual, device, false);
        visual.simulate_keystrokes("0");
        pump_until(&editor, cx, settled);
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, original)
        });
        visual.run_until_parked();
    }
    visual.simulate_keystrokes("0");
    pump_until(&editor, cx, settled);
    let expected = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    let path = browser.0.join("bypassed.hex");
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::SaveNew(path.clone())))
    });
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, _| assert!(view.error.is_none()));
    let reopened = Service::new(48_000, 8, 8_000_000).unwrap();
    let _ = receive_service_event(&reopened);
    reopened.submit(Request::Open(path)).unwrap();
    let event = receive_service_event(&reopened);
    assert!(event.error.is_none());
    assert_eq!(event.state.project, expected);
    let (handle, _) = browser.window(cx, reopened, event.state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for device in [effect, instrument] {
        super::header_actions_tests::assert_power_paint(
            &mut visual,
            device,
            !expected.device(device).unwrap().bypassed(),
        );
    }
}

#[gpui::test]
fn device_zero_uses_return_and_main_owner_without_changing_track_or_chain(cx: &mut TestAppContext) {
    let browser = BrowserDirectory::new();
    cx.update(super::super::super::bind_editor_keys);
    let (service, _, track, return_id, return_device, _) = actor_return_utility_fixture();
    service
        .submit(Request::Edit(Command::InsertMainDevice {
            before: None,
            state: hex_engine::default_native_device_state(
                hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
            )
            .unwrap(),
        }))
        .unwrap();
    let state = receive_service_event(&service).state;
    let main_device = state.project.main_devices()[0].id();
    let original = state.project.clone();
    let (handle, editor) = browser.window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    for (owner, device) in [
        (DevicePanelOwner::Return(return_id), return_device),
        (DevicePanelOwner::Main, main_device),
    ] {
        editor.update(cx, |view, cx| {
            view.selected_track = None;
            view.focused_return = if matches!(owner, DevicePanelOwner::Return(_)) {
                Some(return_id)
            } else {
                None
            };
            view.device_panel.main_selected =
                (owner == DevicePanelOwner::Main).then_some(view.project_generation);
            cx.notify();
        });
        visual.run_until_parked();
        let point = title(&mut visual, device);
        visual.simulate_click(point, Modifiers::default());
        visual.simulate_keystrokes("0");
        pump_until(&editor, cx, settled);
        let changed = editor.read_with(cx, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            let mut expected = Editor::new((*original).clone(), 8).unwrap();
            expected
                .execute(Command::SetDeviceBypass {
                    device,
                    bypassed: true,
                })
                .unwrap();
            assert_eq!(project.as_ref(), expected.project());
            assert_eq!(view.device_panel.owner, Some(owner));
            assert_eq!(view.device_panel.selected_device, Some(device));
            project.clone()
        });
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-z");
        pump_until(&editor, cx, settled);
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, original)
        });
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-shift-z");
        pump_until(&editor, cx, settled);
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, changed)
        });
        visual.run_until_parked();
        visual.simulate_keystrokes("0");
        pump_until(&editor, cx, settled);
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, original)
        });
    }
}

#[gpui::test]
fn device_zero_preserves_numeric_and_browser_typing_and_modified_keys(cx: &mut TestAppContext) {
    let browser = BrowserDirectory::new();
    cx.update(super::super::super::bind_editor_keys);
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let original = state.project.clone();
    let (handle, editor) = browser.window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    let point = title(&mut visual, device);
    visual.simulate_click(point, Modifiers::default());
    visual.simulate_keystrokes("cmd-0 alt-0 ctrl-0 shift-0");
    let field = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-1-value", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a 0");
    editor.read_with(cx, |view, cx| {
        let ParameterControl::Float { entity, .. } = &view.device_panel.controls[&ParameterKey {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(1),
        }] else {
            panic!("numeric control")
        };
        assert!(entity.read(cx).is_editing());
        assert_eq!(entity.read(cx).display_text(), "0");
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    visual.simulate_keystrokes("x");
    editor.read_with(cx, |view, cx| {
        let ParameterControl::Float { entity, .. } = &view.device_panel.controls[&ParameterKey {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(1),
        }] else {
            panic!("numeric control")
        };
        assert_eq!(entity.read(cx).display_text(), "0x");
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    visual.simulate_keystrokes("escape");
    let search = visual.debug_bounds("browser-search").unwrap();
    visual.simulate_click(search.center(), Modifiers::default());
    visual.simulate_input("find");
    visual.simulate_keystrokes("0");
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "find0");
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
}

#[gpui::test]
fn device_zero_refuses_retired_hidden_pending_and_unselected_card_handlers(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::super::bind_editor_keys);
    for refusal in 0..10 {
        let browser = BrowserDirectory::new();
        let (service, state, track, device, _, _) = actor_utility_fixture();
        let (handle, editor) = browser.window(cx, service, state, track);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(1600.0), px(700.0)));
        visual.run_until_parked();
        let point = title(&mut visual, device);
        visual.simulate_click(point, Modifiers::default());
        editor.update(cx, |view, _| match refusal {
            0 => view.worker_requests_pending = 1,
            1 => view.device_panel.selected_device = None,
            2 => view.project_generation += 1,
            3 => view.device_panel_active = false,
            4 => view.clip_view_open = false,
            5 => view.selected_track = None,
            6 => view.opening_project = true,
            7 => view.device_panel.owner = None,
            8 | 9 => {
                let state = view.state.as_ref().unwrap();
                let mut changed = Editor::new((*state.project).clone(), 8).unwrap();
                let command = if refusal == 8 {
                    Command::RemoveDevice(device)
                } else {
                    Command::ReplaceDeviceState {
                        device,
                        state: hex_engine::eq::EqState::default().encode().unwrap(),
                    }
                };
                changed.execute(command).unwrap();
                view.state = Some(Arc::new(ViewState {
                    canonical_audio_source: None,
                    comparison_audio: None,
                    project: Arc::new(changed.project().clone()),
                    waveforms: state.waveforms.clone(),
                    source_overviews: state.source_overviews.clone(),
                    audio_clip_waveforms: state.audio_clip_waveforms.clone(),
                    midi_preview: state.midi_preview.clone(),
                    gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
                    timeline_frames: state.timeline_frames,
                    can_undo: state.can_undo,
                    can_redo: state.can_redo,
                    revision: state.revision + 1,
                    unsaved: true,
                    document_path: state.document_path.clone(),
                }));
            }
            _ => unreachable!(),
        });
        let expected = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
        // Dispatch before repaint: the old card handler must inspect live identity.
        visual.simulate_keystrokes("0");
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.state.as_ref().unwrap().project,
                expected,
                "refusal {refusal}"
            );
            assert!(view.pending_requests.is_empty());
            assert!(view.device_gesture_barriers.is_empty());
            assert_eq!(view.worker_requests_pending, usize::from(refusal == 0));
        });
    }
}

#[gpui::test]
fn device_zero_orders_valid_parameter_draft_before_bypass_with_separate_undo(
    cx: &mut TestAppContext,
) {
    let browser = BrowserDirectory::new();
    cx.update(super::super::super::bind_editor_keys);
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let original = state.project.clone();
    let (handle, editor) = browser.window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    let field = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-1-value", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("6.00 db");
    let point = title(&mut visual, device);
    visual.simulate_click(point, Modifiers::default());
    visual.simulate_keystrokes("0");
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, _| {
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .bypassed()
        )
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-z");
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, _| {
        let item = view.state.as_ref().unwrap().project.device(device).unwrap();
        assert!(!item.bypassed());
        let ParameterValue::Float(value) = hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Effect,
            item.state(),
            hex_lattice_contract::ParameterId::from_static(1),
        )
        .unwrap() else {
            panic!("gain")
        };
        assert!((value - 10.0_f32.powf(6.0 / 20.0)).abs() < 0.0001);
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-z");
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original)
    });
}
