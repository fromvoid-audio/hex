use super::*;

fn selector(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

#[gpui::test]
fn operator_replacement_and_navigation_preserve_untouched_rounded_values(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, _, _, _) = actor_utility_fixture();
    let sine = state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.browser.section = crate::editor::browser::BrowserSection::Instruments;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let source = visual
        .debug_bounds("browser-device-instrument-hex.instrument.operator")
        .unwrap();
    let down = source.center();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let card = visual
        .debug_bounds(selector(format!("device-drop-card-{}", sine.value())))
        .unwrap();
    let destination = point(card.left() + px(24.0), card.center().y);
    visual.simulate_mouse_move(destination, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(destination, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .state()
            .type_id()
            .as_str()
            == Ok("hex.instrument.operator")
    });
    visual.run_until_parked();
    let device = editor.read_with(cx, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id()
    });
    let ratio = hex_lattice_contract::ParameterId::from_static(1);
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::SetDeviceParameter {
            device,
            parameter: ratio,
            value: ParameterValue::Float(1.2345)
        }));
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    visual.run_until_parked();
    let (original, revision) = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        (state.project.clone(), state.revision)
    });
    for page in ["oscillator", "envelope", "oscillator"] {
        let tab = visual
            .debug_bounds(selector(format!("operator-page-{}-{page}", device.value())))
            .unwrap();
        visual.simulate_click(tab.center(), Modifiers::default());
        visual.run_until_parked();
    }
    let field = visual
        .debug_bounds(selector(format!(
            "device-{}-parameter-1-value",
            device.value()
        )))
        .unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        let ParameterControl::Float { entity, .. } = &view.device_panel.controls[&ParameterKey {
            device,
            parameter: ratio,
        }] else {
            panic!("ratio")
        };
        assert!(entity.read(cx).is_editing());
        assert_eq!(entity.read(cx).display_text(), "1.23");
    });
    let envelope = visual
        .debug_bounds(selector(format!(
            "operator-page-{}-envelope",
            device.value()
        )))
        .unwrap();
    visual.simulate_click(envelope.center(), Modifiers::default());
    visual.run_until_parked();
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture_barriers.is_empty()
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().revision,
            revision,
            "untouched rounded readout must not author a new revision"
        );
        assert_eq!(
            view.state.as_ref().unwrap().project,
            original,
            "view navigation preserves exact canonical sound state and history"
        );
        assert!(view.device_gesture.snapshot().is_none());
    });
    // Enter on an untouched draft follows the same exact-value contract as a
    // page-change barrier. Explicitly typing the displayed value is an edit.
    visual.simulate_click(field.center(), Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, revision)
    });
    enter_typed_value(&mut visual, device, ratio, "1.23");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    pump_until(&editor, cx, |view| {
        view.state.as_ref().unwrap().revision > revision
            && view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.revision, revision + 1);
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                NativeDeviceClass::Instrument,
                state
                    .project
                    .track(track)
                    .unwrap()
                    .instrument()
                    .unwrap()
                    .state(),
                ratio
            )
            .unwrap(),
            ParameterValue::Float(1.23)
        );
    });
    visual.simulate_keystrokes("cmd-z");
    visual.run_until_parked();
    pump_until(&editor, cx, |view| {
        view.state.as_ref().unwrap().project == original && view.worker_requests_pending == 0
    });
    visual.run_until_parked();
}
