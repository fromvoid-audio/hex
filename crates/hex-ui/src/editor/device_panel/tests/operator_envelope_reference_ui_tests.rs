use super::*;

#[gpui::test]
fn other_envelopes_refresh_without_stealing_selection_and_reuse_repaint_geometry(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, mut state, track, device) = operator_navigation_tests::operator_fixture();
    for operator in 0..4 {
        service
            .submit(Request::SetDeviceParameter {
                device,
                parameter: hex_lattice_contract::ParameterId::from_static(36 + operator * 6),
                value: ParameterValue::Float(0.1 + operator as f32),
            })
            .unwrap();
        state = receive_service_event(&service).state;
    }
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let read = |visual: &mut VisualTestContext, selected| {
        editor.read_with(visual, |view, cx| {
            operator_panel::envelope::displayed_references(view, device, selected, cx).unwrap()
        })
    };
    for selected in 0..4 {
        let background =
            operator_context_selection_tests::row_background(&mut visual, device, selected);
        visual.simulate_click(background, Modifiers::default());
        visual.run_until_parked();
        let curves = read(&mut visual, selected);
        assert_eq!(curves.len(), 3);
        let others: Vec<_> = (0..4).filter(|osc| *osc != selected).collect();
        for (curve, other) in curves.iter().zip(others) {
            assert_eq!(
                curve.color,
                theme::oscillator_identity_color(other).unwrap()
            );
            assert_eq!(curve.opacity, 0.28);
            assert!(curve.points.len() > 10);
        }
        assert_ne!(curves[0].points, curves[1].points);
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        let repainted = read(&mut visual, selected);
        for (before, after) in curves.iter().zip(&repainted) {
            assert!(std::sync::Arc::ptr_eq(&before.points, &after.points));
        }
    }
    let background = operator_context_selection_tests::row_background(&mut visual, device, 0);
    visual.simulate_click(background, Modifiers::default());
    visual.run_until_parked();
    let before = read(&mut visual, 0);
    let revision = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().revision);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::SetDeviceParameter {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(42),
            value: ParameterValue::Float(8.0)
        }))
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.state.as_ref().unwrap().revision == revision + 1
    });
    visual.run_until_parked();
    let changed = read(&mut visual, 0);
    assert_ne!(before[0].points, changed[0].points);
    assert_eq!(before[1..], changed[1..]);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].selected_operator,
            0
        )
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.state.as_ref().unwrap().revision == revision + 2
    });
    visual.run_until_parked();
    assert_eq!(read(&mut visual, 0), before);
    editor.update(&mut visual, |view, _| {
        assert!(operator_panel::toggle_enable(view, device, 1))
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && operator_parameter_value(view.state.as_ref().unwrap(), track, device, 514)
                == ParameterValue::Bool(false)
    });
    visual.run_until_parked();
    let disabled = read(&mut visual, 0);
    assert_eq!(disabled[0].opacity, 0.12);
    assert_eq!(disabled[0].points, before[0].points);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].selected_operator,
            0
        )
    });
}
