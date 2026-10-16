use super::*;

#[gpui::test]
fn device_chain_navigation_pan_overview_release_escape_and_resize(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, mut state, track, _, _, _) = actor_utility_fixture();
    for _ in 0..8 {
        service
            .submit(Request::AddNativeDevice {
                track,
                class: NativeDeviceClass::Effect,
                type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                    "hex.utility",
                )),
                before: None,
            })
            .expect("append utility");
        state = receive_service_event(&service).state;
    }
    let card_selector = Box::leak(
        format!(
            "device-drop-card-{}",
            state.project.track(track).expect("track").devices()[2]
                .id()
                .value()
        )
        .into_boxed_str(),
    );
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();
    let rail = visual.debug_bounds("device-chain-scroll").expect("rail");
    let card_before = visual
        .debug_bounds(card_selector)
        .expect("visible device card");
    let start = rail.center();
    // Begin over a device, then leave the rail while holding the middle button.
    visual.simulate_mouse_down(start, MouseButton::Middle, Modifiers::default());
    let outside = point(start.x - px(200.0), rail.origin.y - px(20.0));
    visual.simulate_mouse_move(outside, Some(MouseButton::Middle), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.navigation.scroll.offset().x, px(-200.0));
        assert!(
            view.last_enqueued_edit.is_none(),
            "panning must not edit the project"
        );
    });
    let card_after = visual
        .debug_bounds(card_selector)
        .expect("panned device card");
    assert_eq!(card_after.origin.x, card_before.origin.x - px(200.0));
    visual.simulate_mouse_up(outside, MouseButton::Middle, Modifiers::default());
    visual.simulate_mouse_move(start, None, Modifiers::default());
    visual.run_until_parked();
    editor.update(cx, |view, _| {
        assert!(
            !view.device_panel.navigation.cancel(),
            "outside release ends capture"
        )
    });
    let overview = visual
        .debug_bounds("device-chain-overview")
        .expect("overview");
    assert!(
        overview.right() <= px(880.0),
        "navigator fits minimum window"
    );
    assert!(
        visual
            .debug_bounds("device-panel-bottom-toggle")
            .expect("bottom toggle")
            .right()
            <= px(880.0)
    );
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: overview.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-75.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.navigation.scroll.offset().x, px(-275.0))
    });
    let right = point(overview.right() - px(1.0), overview.center().y);
    visual.simulate_mouse_down(right, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(right, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let scroll = &view.device_panel.navigation.scroll;
        assert_eq!(
            scroll.offset().x,
            -scroll.max_offset().width,
            "click reaches final device"
        );
    });
    // Drag the focus frame all the way back to the beginning.
    visual.simulate_mouse_down(right, MouseButton::Left, Modifiers::default());
    let left = point(overview.origin.x - px(100.0), overview.center().y);
    visual.simulate_mouse_move(left, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(left, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.navigation.scroll.offset().x, px(0.0))
    });
    visual.simulate_mouse_down(start, MouseButton::Middle, Modifiers::default());
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_move(outside, Some(MouseButton::Middle), Modifiers::default());
    visual.simulate_mouse_up(outside, MouseButton::Middle, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.navigation.scroll.offset().x, px(0.0))
    });
    visual.simulate_mouse_down(right, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(right, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let old_max = editor.read_with(cx, |view, _| {
        view.device_panel.navigation.scroll.max_offset().width
    });
    visual.simulate_resize(size(px(1800.0), px(760.0)));
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let scroll = &view.device_panel.navigation.scroll;
        assert!(
            scroll.max_offset().width < old_max,
            "wider viewport reduces overflow"
        );
        assert_eq!(
            scroll.offset().x,
            -scroll.max_offset().width,
            "resize clamps focus at chain end"
        );
    });
}

#[gpui::test]
fn device_chain_bottom_toggle_preserves_return_owner_and_short_chain_is_safe(
    cx: &mut TestAppContext,
) {
    let (service, mut state, track, return_track, _, _) = actor_return_utility_fixture();
    for _ in 0..8 {
        service
            .submit(Request::Edit(Command::InsertReturnDevice {
                return_track,
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                )
                .expect("utility"),
            }))
            .expect("append return device");
        state = receive_service_event(&service).state;
    }
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            assert!(view.focus_return_details(return_track, window, cx));
            cx.notify();
        });
    })
    .expect("focus return");
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.simulate_resize(size(px(2100.0), px(760.0)));
    visual.run_until_parked();
    let toggle = visual
        .debug_bounds("device-panel-bottom-toggle")
        .expect("bottom toggle")
        .center();
    for expected_open in [false, true] {
        visual.simulate_mouse_down(toggle, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(toggle, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(view.clip_view_open, expected_open);
            assert_eq!(view.focused_return, Some(return_track));
        });
        assert_eq!(
            visual.debug_bounds("device-chain-scroll").is_some(),
            expected_open
        );
        assert!(visual.debug_bounds("device-chain-overview").is_some());
    }
    let rail = visual
        .debug_bounds("device-chain-scroll")
        .expect("short rail");
    visual.simulate_mouse_down(rail.center(), MouseButton::Middle, Modifiers::default());
    let far = point(rail.center().x - px(10000.0), rail.center().y);
    visual.simulate_mouse_move(far, Some(MouseButton::Middle), Modifiers::default());
    visual.simulate_mouse_up(far, MouseButton::Middle, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.navigation.scroll.offset().x, px(0.0))
    });
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();
    let rail = visual
        .debug_bounds("device-chain-scroll")
        .expect("overflowing return rail");
    visual.simulate_mouse_down(rail.center(), MouseButton::Middle, Modifiers::default());
    let moved = point(rail.center().x - px(100.0), rail.center().y);
    visual.simulate_mouse_move(moved, Some(MouseButton::Middle), Modifiers::default());
    visual.simulate_mouse_up(moved, MouseButton::Middle, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.navigation.scroll.offset().x, px(-100.0))
    });
    // Changing to the empty regular chain retires the return navigation identity.
    editor.update(cx, |view, cx| {
        view.focused_return = None;
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.owner,
            Some(DevicePanelOwner::Track(track))
        );
        assert_eq!(view.device_panel.navigation.scroll.offset().x, px(0.0));
    });
}
