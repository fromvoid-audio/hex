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
        keystroke: gpui::Keystroke::parse("backspace").unwrap(),
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
        keystroke: gpui::Keystroke::parse("backspace").unwrap(),
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
        keystroke: gpui::Keystroke::parse("backspace").unwrap(),
        is_held: true,
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original)
    });
    visual.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("backspace").unwrap(),
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
        assert_eq!(view.browser.search.read(cx).content, "ab");
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
