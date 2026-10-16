use super::*;

#[gpui::test]
fn rendered_harmonic_stroke_is_atomic_and_shift_preserves_other_columns(cx: &mut TestAppContext) {
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
    let initial = receive_service_event(&service).state;
    let device = initial
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let original = initial.project.device(device).unwrap().state().clone();
    let (handle, editor) = actor_editor_window(cx, service, initial, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let tab = Box::leak(format!("operator-page-{}-oscillator", device.value()).into_boxed_str());
    let bounds = visual.debug_bounds(tab).unwrap();
    visual.simulate_click(bounds.center(), Modifiers::default());
    visual.run_until_parked();
    let graph_id = Box::leak(format!("operator-harmonics-{}-0", device.value()).into_boxed_str());
    let bounds = visual
        .debug_bounds(graph_id)
        .expect("64-column painter is visible");
    assert!(bounds.size.height >= px(80.0));
    assert!(bounds.size.width >= px(200.0));
    // The painter owns the upper display; its tools live below stationary tabs.
    let tools = visual.debug_bounds("operator-harmonic-tools").unwrap();
    let tab_bounds = visual.debug_bounds(tab).unwrap();
    assert!(bounds.bottom() <= tab_bounds.top());
    assert!(tools.top() >= tab_bounds.bottom());
    let preview = visual.debug_bounds("operator-waveform-preview").unwrap();
    assert!(preview.top() >= tab_bounds.bottom());
    assert!(preview.size.width >= px(48.0));
    assert!(preview.size.height >= px(12.0));
    let envelope = visual.debug_bounds("operator-envelope-fields").unwrap();
    let oscillator = visual.debug_bounds("operator-oscillator-fields").unwrap();
    assert!(envelope.right() <= oscillator.left());
    assert!(envelope.top() >= tab_bounds.bottom());
    assert!(oscillator.top() >= tab_bounds.bottom());
    let field = |parameter| {
        Box::leak(format!("operator-field-{}-{parameter}", device.value()).into_boxed_str())
            as &'static str
    };
    let phase = visual.debug_bounds(field(3)).unwrap();
    let velocity = visual.debug_bounds(field(74)).unwrap();
    assert!(
        phase.right() <= velocity.left(),
        "phase and velocity cells must not collide"
    );
    for parameter in [3, 74, 78] {
        let text = editor.read_with(cx, |view, app| {
            let ParameterControl::Float { entity, .. } = &view.device_panel.controls
                [&ParameterKey {
                    device,
                    parameter: hex_lattice_contract::ParameterId::from_static(parameter),
                }]
            else {
                panic!("numeric oscillator field");
            };
            entity.read(app).display_text().to_owned()
        });
        assert!(text.ends_with('%'));
        let value_id = Box::leak(
            format!("device-{}-parameter-{parameter}-value", device.value()).into_boxed_str(),
        );
        let cell = visual.debug_bounds(value_id).unwrap();
        let width = visual.update(|window, _| {
            window
                .text_system()
                .shape_line(
                    text.clone().into(),
                    px(control_style().font_size),
                    &[gpui::TextRun {
                        len: text.len(),
                        font: gpui::font(theme::FONT_FAMILY),
                        color: gpui::rgb(0xffffff).into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                )
                .width
        });
        assert!(
            width <= cell.size.width - px(8.0),
            "formatted {text:?} must fit its own value field"
        );
    }
    let range = |count| {
        Box::leak(format!("operator-range-{}-0-{count}", device.value()).into_boxed_str())
            as &'static str
    };
    let ranges = [16, 32, 64].map(|count| visual.debug_bounds(range(count)).unwrap());
    for button in ranges {
        assert!(button.left() >= bounds.right());
        assert!(button.top() >= bounds.top() && button.bottom() <= bounds.bottom());
    }
    assert!(ranges[0].bottom() <= ranges[1].top());
    assert!(ranges[1].bottom() <= ranges[2].top());
    let from = point(
        bounds.origin.x + px(1.0),
        bounds.origin.y + bounds.size.height * 0.75,
    );
    let to = point(
        bounds.origin.x + bounds.size.width - px(1.0),
        bounds.origin.y + bounds.size.height * 0.25,
    );
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.device_gesture.snapshot().is_none()
            && view.state.as_ref().is_some_and(|s| {
                operator_parameter_value(s, track, device, 156) != ParameterValue::Float(0.0)
            })
    });
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(
            operator_parameter_value(state, track, device, 57),
            ParameterValue::Choice(4)
        );
        for parameter in 93..=156 {
            assert!(matches!(
                operator_parameter_value(state, track, device, parameter),
                ParameterValue::Float(value) if (0.24..=0.76).contains(&value)
            ));
        }
    });
    // A redraw must refresh the painter's values, not just the canonical project.
    // Keyboard adjustment of the last painted bar observes that rendered value.
    visual.run_until_parked();
    let painted = editor.read_with(cx, |view, _| {
        operator_parameter_value(view.state.as_ref().unwrap(), track, device, 156)
    });
    visual.simulate_keystrokes("up");
    pump_until(&editor, cx, |view| {
        view.device_gesture.snapshot().is_none()
            && view
                .state
                .as_ref()
                .is_some_and(|state| operator_parameter_value(state, track, device, 156) != painted)
    });
    editor.read_with(cx, |view, _| {
        assert!(
            matches!(
                operator_parameter_value(view.state.as_ref().unwrap(), track, device, 156),
                ParameterValue::Float(value) if (0.75..=0.77).contains(&value)
            ),
            "keyboard edit must continue from the painted bar after redraw"
        );
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .is_some_and(|state| operator_parameter_value(state, track, device, 156) == painted)
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .is_some_and(|s| s.project.device(device).unwrap().state() == &original)
    });
    visual.run_until_parked();
    let envelope_tab =
        Box::leak(format!("operator-page-{}-envelope", device.value()).into_boxed_str());
    let envelope_tab_bounds = visual.debug_bounds(envelope_tab).unwrap();
    visual.simulate_click(envelope_tab_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds(field(3)).unwrap(), phase);
    assert_eq!(visual.debug_bounds(field(74)).unwrap(), velocity);
    assert_eq!(visual.debug_bounds(tab).unwrap(), tab_bounds);
    visual.simulate_click(tab_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    let bounds = visual.debug_bounds(graph_id).unwrap();
    let from = point(
        bounds.origin.x + bounds.size.width * 0.02,
        bounds.origin.y + bounds.size.height * 0.5,
    );
    let to = point(
        bounds.origin.x + bounds.size.width * 0.8,
        bounds.origin.y + bounds.size.height * 0.2,
    );
    let modifiers = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(from, MouseButton::Left, modifiers);
    visual.simulate_mouse_move(to, MouseButton::Left, modifiers);
    visual.simulate_mouse_up(to, MouseButton::Left, modifiers);
    pump_until(&editor, cx, |view| {
        view.device_gesture.snapshot().is_none()
            && view.state.as_ref().is_some_and(|s| {
                operator_parameter_value(s, track, device, 94) != ParameterValue::Float(0.0)
            })
    });
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        for id in 95..=156 {
            assert_eq!(
                operator_parameter_value(state, track, device, id),
                ParameterValue::Float(0.0)
            );
        }
    });
}

#[gpui::test]
fn rendered_harmonic_range_and_saved_values_drive_keyboard_edits(cx: &mut TestAppContext) {
    let (service, _, track, device) = operator_navigation_tests::operator_fixture();
    let mut state = None;
    for (parameter, value) in [
        (57, ParameterValue::Choice(4)),
        (349, ParameterValue::Choice(0)),
        (108, ParameterValue::Float(0.5)),
        (156, ParameterValue::Float(0.8)),
    ] {
        service
            .submit(Request::SetDeviceParameter {
                device,
                parameter: hex_lattice_contract::ParameterId::from_static(parameter),
                value,
            })
            .unwrap();
        state = Some(receive_service_event(&service).state);
    }
    let (handle, editor) = actor_editor_window(cx, service, state.unwrap(), track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let tab = Box::leak(format!("operator-page-{}-oscillator", device.value()).into_boxed_str());
    let bounds = visual.debug_bounds(tab).unwrap();
    visual.simulate_click(bounds.center(), Modifiers::default());
    visual.run_until_parked();
    let graph_id = Box::leak(format!("operator-harmonics-{}-0", device.value()).into_boxed_str());
    let bounds = visual.debug_bounds(graph_id).unwrap();
    // Focus the last visible partial without changing its existing amplitude.
    visual.simulate_click(
        point(bounds.right() - px(1.0), bounds.center().y),
        Modifiers::default(),
    );
    pump_until(&editor, cx, |view| view.device_gesture.snapshot().is_none());
    visual.run_until_parked();
    for _ in 0..64 {
        visual.simulate_keystrokes("right");
    }
    visual.simulate_keystrokes("up");
    pump_until(&editor, cx, |view| {
        view.device_gesture.snapshot().is_none()
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 108) != ParameterValue::Float(0.5)
            })
    });
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert!(matches!(
            operator_parameter_value(state, track, device, 108),
            ParameterValue::Float(value) if (value - 0.51).abs() < 0.00001
        ));
        assert_eq!(
            operator_parameter_value(state, track, device, 156),
            ParameterValue::Float(0.8)
        );
    });
    let mut previous = 0;
    for (count, choice) in [(32, 1), (64, 2), (16, 0)] {
        let button =
            Box::leak(format!("operator-range-{}-0-{count}", device.value()).into_boxed_str());
        let bounds = visual.debug_bounds(button).unwrap();
        visual.simulate_click(bounds.center(), Modifiers::default());
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().is_some_and(|state| {
                    operator_parameter_value(state, track, device, 349)
                        == ParameterValue::Choice(choice)
                })
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().is_some_and(|state| {
                    operator_parameter_value(state, track, device, 349)
                        == ParameterValue::Choice(previous)
                })
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().is_some_and(|state| {
                    operator_parameter_value(state, track, device, 349)
                        == ParameterValue::Choice(choice)
                })
        });
        editor.read_with(cx, |view, _| {
            assert_eq!(
                operator_parameter_value(view.state.as_ref().unwrap(), track, device, 156),
                ParameterValue::Float(0.8)
            );
        });
        visual.run_until_parked();
        previous = choice;
    }
}

#[gpui::test]
fn hiding_harmonic_field_cancels_stroke_and_keeps_the_preview(cx: &mut TestAppContext) {
    let (service, initial, track, device) = operator_navigation_tests::operator_fixture();
    let original = initial.project.device(device).unwrap().state().clone();
    let (handle, editor) = actor_editor_window(cx, service, initial, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let tab = Box::leak(format!("operator-page-{}-oscillator", device.value()).into_boxed_str());
    let tab_bounds = visual.debug_bounds(tab).unwrap();
    visual.simulate_click(tab_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    let graph = Box::leak(format!("operator-harmonics-{}-0", device.value()).into_boxed_str());
    let bounds = visual.debug_bounds(graph).unwrap();
    visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| view.device_gesture.snapshot().is_some());
    // A page change can also be delivered by navigation rather than mouse-up.
    editor.update(cx, |view, cx| {
        view.device_panel
            .operator_pages
            .get_mut(&device)
            .unwrap()
            .page = operator_panel::OperatorPage::Envelope;
        cx.notify();
    });
    visual.run_until_parked();
    pump_until(&editor, cx, |view| {
        view.device_gesture.snapshot().is_none() && view.worker_requests_pending == 0
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .state(),
            &original
        );
    });
    assert!(visual.debug_bounds(graph).is_none());
    assert!(visual.debug_bounds("operator-waveform-preview").is_some());
    visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_click(tab_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    let bounds = visual.debug_bounds(graph).unwrap();
    visual.simulate_click(bounds.center(), Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.device_gesture.snapshot().is_none()
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 125) != ParameterValue::Float(0.0)
            })
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.project.device(device).unwrap().state() == &original)
    });
}
