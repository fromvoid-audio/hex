use super::*;
use gpui::Focusable;

fn selector(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}
fn rail(
    visual: &mut VisualTestContext,
    device: DeviceId,
    operator: usize,
) -> gpui::Bounds<gpui::Pixels> {
    visual
        .debug_bounds(selector(format!(
            "device-{}-parameter-{}-slider",
            device.value(),
            613 + operator
        )))
        .unwrap()
}

#[gpui::test]
fn primary_keytrack_rails_fit_rows_and_commit_independent_canonical_history(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    cx.update(|app| app.use_test_assets(crate::assets::HexAssets));
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    for operator in 0..4 {
        let bar = rail(&mut visual, device, operator);
        let row = visual
            .debug_bounds(selector(format!(
                "operator-row-{}-{operator}",
                device.value()
            )))
            .unwrap();
        assert_eq!(row.size.height, px(44.0));
        assert_eq!(bar.size, size(px(10.0), px(30.0)));
        assert!(bar.left() >= row.left() && bar.right() <= row.right());
        assert!(bar.top() >= row.top() && bar.bottom() <= row.bottom());
        let icon = selector(format!(
            "operator-keytrack-icon-{}-{operator}",
            device.value()
        ));
        let keyboard = visual.debug_bounds(icon).unwrap();
        assert!(keyboard.top() >= bar.bottom());
        assert!(keyboard.bottom() <= row.bottom());
        assert!(
            !visual.rendered_monochrome_colors(icon).is_empty(),
            "embedded keyboard icon is actually painted"
        );
        assert!(
            visual
                .debug_bounds(selector(format!(
                    "device-{}-parameter-{}-value",
                    device.value(),
                    613 + operator
                )))
                .is_none()
        );
        let start = bar.center();
        let end = point(start.x, start.y + px(12.0));
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending==0
            && view.pending_requests.is_empty() && view.device_gesture_barriers.is_empty()
            && view.device_gesture.snapshot().is_none()
            && view.state.as_ref().is_some_and(|s|matches!(operator_parameter_value(s,track,device,613+operator as u32),ParameterValue::Float(v) if v<100.0 && v>0.0))
        });
        let accepted = editor.read_with(cx, |view, _| {
            assert_eq!(
                view.device_panel.operator_pages[&device].selected_operator,
                0
            );
            operator_parameter_value(
                view.state.as_ref().unwrap(),
                track,
                device,
                613 + operator as u32,
            )
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().is_some_and(|s| {
                    operator_parameter_value(s, track, device, 613 + operator as u32)
                        == ParameterValue::Float(100.0)
                })
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().is_some_and(|s| {
                    operator_parameter_value(s, track, device, 613 + operator as u32) == accepted
                })
        });
        visual.run_until_parked();
    }
    // The permanent primary ratio fields stay available at the fixed endpoint.
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::SetDeviceParameter {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(613),
            value: ParameterValue::Float(0.0)
        }))
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|s| {
                operator_parameter_value(s, track, device, 613) == ParameterValue::Float(0.0)
            })
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(selector(format!(
                "device-{}-parameter-1-slider",
                device.value()
            )))
            .is_some()
    );
    open_operator_options(&mut visual, device);
    for parameter in [62, 63, 64] {
        let cell = visual
            .debug_bounds(selector(format!(
                "operator-options-panel-{}",
                device.value()
            )))
            .unwrap();
        let control = visual
            .debug_bounds(selector(format!(
                "device-{}-parameter-{parameter}",
                device.value()
            )))
            .unwrap();
        assert!(
            control.left() >= cell.left() && control.right() <= cell.right(),
            "fixed anchors fit their own cells"
        );
        assert!(
            visual
                .debug_bounds(selector(format!(
                    "device-{}-parameter-{parameter}",
                    device.value()
                )))
                .is_some()
        );
    }
}

#[gpui::test]
fn released_keytrack_fill_survives_completed_repaints_until_terminal_ack(cx: &mut TestAppContext) {
    let (_service, state, track, device) = operator_navigation_tests::operator_fixture();
    let (handle, editor) = crate::editor::clip_drag_ui_test::editor_window(
        cx,
        (*state.project).clone(),
        Arc::default(),
    );
    editor.update(cx, |view, cx| {
        view.selected_track = Some(track);
        view.device_panel_active = true;
        view.clip_view_open = true;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = rail(&mut visual, device, 3);
    let start = bounds.center();
    let end = point(start.x, start.y + px(12.0));
    let fill_id = selector(format!("device-{}-parameter-616-bar-fill", device.value()));
    let original = visual.debug_bounds(fill_id).unwrap();
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    let preview = visual.debug_bounds(fill_id).unwrap();
    assert!(preview.size.height < original.size.height);
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    for _ in 0..3 {
        editor.update(cx, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(fill_id).unwrap(), preview);
        editor.read_with(cx, |view, _| {
            assert_eq!(
                operator_parameter_value(view.state.as_ref().unwrap(), track, device, 616),
                ParameterValue::Float(100.0)
            );
            assert_eq!(
                view.device_panel.operator_pages[&device].selected_operator,
                0
            );
            assert!(view.device_gesture.has_pending_terminal());
        });
    }
}

#[gpui::test]
fn cached_keytrack_focus_cannot_edit_a_stale_or_hidden_owner(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let original = state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let entity = editor.read_with(cx, |view, _| {
        match &view.device_panel.controls[&ParameterKey {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(614),
        }] {
            ParameterControl::Float { entity, .. } => entity.clone(),
            _ => panic!("keytrack"),
        }
    });
    editor.update(cx, |view, cx| {
        view.selected_track = None;
        cx.notify();
    });
    visual.run_until_parked();
    visual.update(|window, cx| entity.focus_handle(cx).focus(window));
    visual.simulate_keystrokes("down");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert!(view.device_gesture.snapshot().is_none());
    });
}
