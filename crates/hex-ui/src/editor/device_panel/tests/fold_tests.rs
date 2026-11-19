use super::*;

fn selector(name: &str, device: DeviceId) -> &'static str {
    Box::leak(format!("{name}-{}", device.value()).into_boxed_str())
}

fn title(visual: &mut VisualTestContext, device: DeviceId) -> gpui::Point<gpui::Pixels> {
    visual
        .debug_bounds(selector("device-drag-title", device))
        .unwrap()
        .center()
}

fn double_click(visual: &mut VisualTestContext, device: DeviceId) {
    let point = title(visual, device);
    visual.simulate_click(point, Modifiers::default());
    visual.run_until_parked();
    eq_ui_tests::eq_double_click(visual, point);
    visual.run_until_parked();
}

#[gpui::test]
fn device_fold_shortcuts_and_double_click_apply_to_the_selected_group(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, _, track, first, _, _) = actor_utility_fixture();
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(hex_devices::utility::UTILITY_TYPE_ID),
            before: None,
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let project = Arc::clone(&state.project);
    let instrument = project.track(track).unwrap().instrument().unwrap().id();
    let second = project.track(track).unwrap().devices()[1].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1800.0), px(800.0)));
    visual.run_until_parked();
    let first_title = title(&mut visual, first);
    let second_title = title(&mut visual, second);
    visual.simulate_click(first_title, Modifiers::default());
    visual.simulate_click(
        second_title,
        Modifiers {
            platform: true,
            ..Default::default()
        },
    );
    let assert_bodies = |visual: &mut VisualTestContext, expanded: bool| {
        for device in [first, second] {
            assert_eq!(
                visual
                    .debug_bounds(selector("device-body", device))
                    .is_some(),
                expanded
            );
            if !expanded {
                let bounds = visual
                    .debug_bounds(selector("effect-device", device))
                    .unwrap();
                assert_eq!(bounds.size.width, px(fold::CARD_WIDTH));
            }
            title(visual, device);
        }
    };
    visual.simulate_keystrokes("-");
    visual.run_until_parked();
    assert_bodies(&mut visual, false);
    visual.simulate_keystrokes("+");
    visual.run_until_parked();
    assert_bodies(&mut visual, true);

    // The first ordinary click would normally reduce the group to one.
    double_click(&mut visual, first);
    assert_bodies(&mut visual, false);
    double_click(&mut visual, second);
    assert_bodies(&mut visual, true);
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.is_selected(first));
        assert!(view.device_panel.is_selected(second));
        assert!(!view.device_panel.is_selected(instrument));
        assert!(view.device_panel.collapsed.is_empty());
        assert_eq!(view.state.as_ref().unwrap().project, project);
        assert_eq!(view.worker_requests_pending, 0);
    });
}

#[gpui::test]
fn device_fold_return_header_respects_numeric_focus_and_restores_body(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, bus, device, parameter) = actor_return_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.selected_track = None;
        view.focused_return = Some(bus);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    let field = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-1-value", device.value()).into_boxed_str(),
        ))
        .expect("return numeric field");
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a -");
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(selector("device-body", device))
            .is_some()
    );
    editor.read_with(cx, |view, cx| {
        let ParameterControl::Float { entity, .. } =
            &view.device_panel.controls[&ParameterKey { device, parameter }]
        else {
            panic!("return numeric control");
        };
        assert_eq!(entity.read(cx).display_text(), "-");
    });
    visual.simulate_keystrokes("escape");
    let header = title(&mut visual, device);
    visual.simulate_click(header, Modifiers::default());
    visual.simulate_keystrokes("-");
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(selector("device-body", device))
            .is_none()
    );
    visual.simulate_keystrokes("+");
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(selector("device-body", device))
            .is_some()
    );
}
