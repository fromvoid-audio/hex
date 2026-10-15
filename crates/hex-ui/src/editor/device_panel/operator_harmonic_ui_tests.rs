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
    let from = point(
        bounds.origin.x + px(1.0),
        bounds.origin.y + bounds.size.height * 0.75,
    );
    let to = point(
        bounds.origin.x + bounds.size.width - px(1.0),
        bounds.origin.y + bounds.size.height * 0.25,
    );
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.device_gesture.snapshot().is_none()
            && view.state.as_ref().is_some_and(|s| {
                operator_parameter_value(s, track, device, 156) != ParameterValue::Float(0.0)
            })
    });
    editor.read_with(cx,|view,_|{let state=view.state.as_ref().unwrap();assert_eq!(operator_parameter_value(state,track,device,57),ParameterValue::Choice(4));for id in 93..=156{assert!(matches!(operator_parameter_value(state,track,device,id),ParameterValue::Float(v)if (0.24..=0.76).contains(&v)));}});
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .is_some_and(|s| s.project.device(device).unwrap().state() == &original)
    });
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
