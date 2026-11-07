use super::*;
use gpui::Focusable;
fn selector(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}
pub(super) fn row_background(
    visual: &mut VisualTestContext,
    device: DeviceId,
    operator: usize,
) -> gpui::Point<gpui::Pixels> {
    let row = visual
        .debug_bounds(selector(format!(
            "operator-row-{}-{operator}",
            device.value()
        )))
        .unwrap();
    // The bottom inset is a real section-selection target outside every control.
    point(row.center().x, row.bottom() - px(1.0))
}

#[gpui::test]
fn working_controls_preserve_the_explicit_envelope_selection(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    for (parameter, text, expected) in [
        (11, "-12 db", 10.0_f32.powf(-12.0 / 20.0)),
        (7, "2", 2.0),
        (610, "150 hz", 150.0),
    ] {
        let field = visual
            .debug_bounds(selector(format!(
                "device-{}-parameter-{parameter}-value",
                device.value()
            )))
            .unwrap();
        visual.simulate_click(field.center(), Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input(text);
        visual.simulate_keystrokes("enter");
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending==0&&view.state.as_ref().is_some_and(|s|matches!(operator_parameter_value(s,track,device,parameter),ParameterValue::Float(v) if (v-expected).abs()<0.00001))
        });
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.device_panel.operator_pages[&device],
                operator_panel::OperatorPanelState {
                    selected_operator: 0,
                    page: operator_panel::OperatorPage::Envelope
                }
            );
            assert_eq!(
                view.device_panel.selected_device,
                Some(device),
                "the device host still owns control clicks"
            );
        });
        visual.run_until_parked();
    }
}

#[gpui::test]
fn row_background_selection_is_explicit_and_refuses_invalid_or_stale_contexts(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let original = state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let background = row_background(&mut visual, device, 3);
    visual.simulate_click(background, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device],
            operator_panel::OperatorPanelState {
                selected_operator: 3,
                page: operator_panel::OperatorPage::Envelope
            }
        );
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    let field = visual
        .debug_bounds(selector(format!(
            "device-{}-parameter-4-value",
            device.value()
        )))
        .unwrap();
    let entity = editor.read_with(cx, |view, _| {
        match &view.device_panel.controls[&ParameterKey {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(4),
        }] {
            ParameterControl::Float { entity, .. } => entity.clone(),
            _ => panic!("ratio"),
        }
    });
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid");
    let background = row_background(&mut visual, device, 0);
    visual.simulate_click(background, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].selected_operator,
            3
        );
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    assert!(visual.update(|window, app| entity.focus_handle(app).is_focused(window)));
    visual.simulate_keystrokes("escape");
    editor.update(cx, |view, cx| {
        let old = operator_panel::context::Owner::capture(view, device).unwrap();
        view.project_generation += 1;
        assert!(!operator_panel::context::select_oscillator(
            view, cx, old, 0
        ));
        assert_eq!(
            view.device_panel.operator_pages[&device].selected_operator,
            3
        );
    });
}
