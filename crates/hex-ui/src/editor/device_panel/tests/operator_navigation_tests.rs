use super::*;
use gpui::Focusable;
use operator_panel::preview::{Field, Switch};

pub(super) fn operator_fixture() -> (Service, Arc<ViewState>, TrackId, DeviceId) {
    let (service, _, track, _, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::RemoveInstrument { track }))
        .unwrap();
    let _ = receive_service_event(&service);
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                "hex.instrument.operator",
            )),
            before: None,
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let device = state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    (service, state, track, device)
}

fn selector(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

// The legacy fallback lets this exact regression execute on the unfixed source:
// it detects the vertical jump, rather than merely failing a renamed selector.
fn tab_bounds(
    visual: &mut VisualTestContext,
    device: DeviceId,
    page: &str,
    legacy: &str,
) -> Bounds<Pixels> {
    visual
        .debug_bounds(selector(format!("operator-page-{}-{page}", device.value())))
        .or_else(|| {
            visual.debug_bounds(selector(format!(
                "operator-page-{}-{legacy}",
                device.value()
            )))
        })
        .unwrap()
}

#[gpui::test]
fn operator_section_navigation_keeps_rendered_bounds_and_owner_fields(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_fixture();
    let original = state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .state()
        .clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1440.0), px(540.0)));
    visual.run_until_parked();
    let envelope = tab_bounds(&mut visual, device, "envelope", "amp env");
    let oscillator = tab_bounds(&mut visual, device, "oscillator", "osc");
    let rail = visual
        .debug_bounds(selector(format!(
            "operator-preview-row-{}-global",
            device.value()
        )))
        .unwrap();
    visual.simulate_click(oscillator.center(), Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        tab_bounds(&mut visual, device, "envelope", "amp env"),
        envelope,
        "envelope navigation must not move when oscillator content replaces it"
    );
    assert_eq!(
        tab_bounds(&mut visual, device, "oscillator", "osc"),
        oscillator,
        "oscillator navigation must not move when selected"
    );
    assert!(
        envelope.origin.x < oscillator.origin.x,
        "reference order is envelope left, oscillator right"
    );
    for operator in 0..4 {
        let badge = visual
            .debug_bounds(selector(format!(
                "operator-select-{}-{operator}",
                device.value()
            )))
            .unwrap();
        visual.simulate_click(badge.center(), Modifiers::default());
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.pending_requests.is_empty()
                && view.device_gesture_barriers.is_empty()
                && view.state.as_ref().is_some_and(|state| {
                    operator_parameter_value(state, track, device, 513 + operator)
                        == ParameterValue::Bool(false)
                })
        });
        // Badges now make a canonical enable edit as well as selecting context.
        // Undo that edit before testing that subsequent navigation preserves sound.
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.pending_requests.is_empty()
                && view.device_gesture_barriers.is_empty()
                && view.state.as_ref().is_some_and(|state| {
                    operator_parameter_value(state, track, device, 513 + operator)
                        == ParameterValue::Bool(true)
                })
        });
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds(selector(format!(
                    "device-{}-parameter-{}",
                    device.value(),
                    57 + operator
                )))
                .is_some(),
            "wave field belongs to selected oscillator"
        );
        visual.simulate_click(envelope.center(), Modifiers::default());
        visual.run_until_parked();
        for parameter in 33 + operator * 6..39 + operator * 6 {
            assert!(
                visual
                    .debug_bounds(selector(format!(
                        "device-{}-parameter-{parameter}-value",
                        device.value()
                    )))
                    .is_some()
            );
        }
        visual.simulate_click(oscillator.center(), Modifiers::default());
        visual.run_until_parked();
        let fixed = visual
            .debug_bounds(selector(format!(
                "device-{}-parameter-{}",
                device.value(),
                62 + operator * 3
            )))
            .unwrap();
        visual.simulate_click(fixed.center(), Modifiers::default());
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.pending_requests.is_empty()
                && view.device_gesture_barriers.is_empty()
        });
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds(selector(format!(
                    "device-{}-parameter-{}-knob",
                    device.value(),
                    63 + operator * 3
                )))
                .is_some(),
            "fixed frequency retains its canonical binding"
        );
        assert_eq!(
            tab_bounds(&mut visual, device, "envelope", "amp env"),
            envelope
        );
        assert_eq!(
            tab_bounds(&mut visual, device, "oscillator", "osc"),
            oscillator
        );
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0 && view.pending_requests.is_empty()
        });
    }
    for switch in [Switch::Filter, Switch::Pitch] {
        let switch = switch.index();
        let bounds = visual
            .debug_bounds(selector(format!(
                "operator-preview-{}-toggle-{switch}",
                device.value()
            )))
            .unwrap();
        visual.simulate_click(bounds.center(), Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            tab_bounds(&mut visual, device, "envelope", "amp env"),
            envelope
        );
        assert_eq!(
            tab_bounds(&mut visual, device, "oscillator", "osc"),
            oscillator
        );
    }
    assert!(
        visual
            .debug_bounds(selector(format!(
                "operator-page-{}-routing",
                device.value()
            )))
            .is_none()
    );
    // Click the Time tile rather than the icon, then activate the rail by keyboard.
    let time = visual
        .debug_bounds(selector(format!(
            "operator-preview-{}-{}",
            device.value(),
            Field::Time as usize
        )))
        .unwrap();
    visual.simulate_click(
        point(time.center().x, time.origin.y + px(3.0)),
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            operator_panel::OperatorPage::Routing
        )
    });
    assert_eq!(
        visual
            .debug_bounds(selector(format!(
                "operator-preview-row-{}-global",
                device.value()
            )))
            .unwrap(),
        rail
    );
    assert_eq!(
        tab_bounds(&mut visual, device, "envelope", "amp env"),
        envelope
    );
    assert_eq!(
        tab_bounds(&mut visual, device, "oscillator", "osc"),
        oscillator
    );
    visual.simulate_click(envelope.center(), Modifiers::default());
    let fine = visual
        .debug_bounds(selector(format!(
            "operator-preview-{}-{}-knob",
            device.value(),
            Field::FineB as usize
        )))
        .unwrap();
    visual.simulate_click(fine.center(), Modifiers::default());
    visual.simulate_keystrokes("tab");
    visual.run_until_parked();
    assert!(
        visual.update(|window, app| editor.read_with(app, |view, _| view
            .device_panel
            .operator_previews[&device]
            .routing_focus
            .as_ref()
            .unwrap()
            .is_focused(window))),
        "tab from a device widget discovers the routing rail"
    );
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            operator_panel::OperatorPage::Routing,
            "focused routing activation takes precedence over session launch"
        );
    });
    visual.simulate_click(envelope.center(), Modifiers::default());
    visual.simulate_click(fine.center(), Modifiers::default());
    visual.simulate_keystrokes("tab");
    visual.simulate_keystrokes("space");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            operator_panel::OperatorPage::Routing
        );
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .state(),
            &original,
            "navigation and selection preserve canonical sound state"
        );
    });
}

#[gpui::test]
fn operator_fine_pointer_focus_and_keyboard_follow_stable_owner(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, device) = operator_fixture();
    let revision = state.revision;
    let original = state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .state()
        .clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    for (operator, field) in [
        (1, Field::FineB),
        (2, Field::FineC),
        (3, Field::FineD),
        (0, Field::FineA),
    ] {
        let fine = visual
            .debug_bounds(selector(format!(
                "operator-preview-{}-{}-knob",
                device.value(),
                field as usize
            )))
            .unwrap();
        visual.simulate_click(fine.center(), Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.device_panel.operator_pages[&device].selected_operator, operator,
                "clicking fine must select its oscillator"
            );
            assert_eq!(
                view.device_panel.operator_pages[&device].page,
                operator_panel::OperatorPage::Envelope
            );
        });
    }
    let fine_b = editor.read_with(cx, |view, _| {
        view.device_panel.operator_previews[&device].controls
            [&operator_panel::preview::Field::FineB]
            .entity
            .clone()
    });

    visual.update(|window, app| fine_b.focus_handle(app).focus(window));
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].selected_operator, 1,
            "keyboard focus selects B without editing its value"
        )
    });
    editor.update(cx, |view, cx| {
        view.device_panel
            .operator_pages
            .get_mut(&device)
            .unwrap()
            .page = operator_panel::OperatorPage::Routing;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("up");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].selected_operator,
            1
        );
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            operator_panel::OperatorPage::Oscillator
        );
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .state(),
            &original
        );
        assert!(
            view.device_gesture.snapshot().is_none(),
            "fine remains honest view-only state"
        );
    });
    let value = visual
        .debug_bounds(selector(format!(
            "operator-preview-{}-{}-value",
            device.value(),
            Field::FineB as usize
        )))
        .unwrap();
    visual.simulate_click(value.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("12");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_previews[&device].values[Field::FineB as usize],
            12.0
        )
    });
    // Keep a cached entity alive across owner/generation changes: neither focus
    // nor its keyboard Begin may select a stale/reused instrument context.
    editor.update(cx, |view, cx| {
        view.selected_track = None;
        cx.notify();
    });
    visual.run_until_parked();
    visual.update(|window, app| fine_b.focus_handle(app).focus(window));
    visual.simulate_keystrokes("up");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_previews[&device].values[Field::FineB as usize],
            12.0,
            "hidden owner cannot receive stale numeric input"
        )
    });
    editor.update(cx, |view, cx| {
        view.selected_track = Some(track);
        view.project_generation += 1;
        view.device_panel.operator_pages.clear();
        cx.notify();
    });
    visual.run_until_parked();
    visual.update(|window, app| fine_b.focus_handle(app).focus(window));
    visual.simulate_keystrokes("up");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel
                .operator_pages
                .get(&device)
                .copied()
                .unwrap_or_default()
                .selected_operator,
            0,
            "old generation cannot select reused ids"
        )
    });
}
