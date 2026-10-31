use super::*;

fn bounds(visual: &mut VisualTestContext, prefix: &str, device: DeviceId) -> Bounds<Pixels> {
    visual
        .debug_bounds(Box::leak(
            format!("{prefix}-{}", device.value()).into_boxed_str(),
        ))
        .unwrap_or_else(|| panic!("visible {prefix} for {device:?}"))
}

pub(super) fn assert_power_paint(visual: &mut VisualTestContext, device: DeviceId, enabled: bool) {
    // Inspect completed idle paint, independently of the hover/click position.
    visual.simulate_mouse_move(point(px(0.0), px(0.0)), None, Modifiers::default());
    visual.run_until_parked();
    let selector = Box::leak(format!("device-bypass-{}", device.value()).into_boxed_str());
    let indicator =
        Box::leak(format!("device-bypass-{}-indicator", device.value()).into_boxed_str());
    let target = visual.debug_bounds(selector).expect("power hit target");
    let indicator_bounds = visual.debug_bounds(indicator).expect("power indicator");
    assert_eq!(target.size, size(px(12.0), px(12.0)));
    assert_eq!(indicator_bounds.size, size(px(8.0), px(8.0)));
    assert_eq!(indicator_bounds.center(), target.center());
    let scale = visual.update(|window, _| window.scale_factor());
    assert_eq!(
        visual
            .rendered_surface_geometry(indicator)
            .unwrap()
            .0
            .top_left,
        px(hex_widgets::TOGGLE_CORNER_RADIUS).scale(scale)
    );
    assert_eq!(
        visual.rendered_background(selector),
        None,
        "no painted surrounding box"
    );
    assert_eq!(
        visual.rendered_border(selector),
        None,
        "no surrounding border"
    );
    assert_eq!(
        visual.rendered_background(indicator),
        Some(
            rgb(theme::color(if enabled {
                theme::PaletteRole::Primary
            } else {
                theme::PaletteRole::Border
            }))
            .into()
        ),
        "canonical power state must keep its semantic fill regardless of header selection"
    );
}

#[gpui::test]
fn header_power_toggle_hover_feedback_and_cancelled_press_keep_canonical_state(
    cx: &mut TestAppContext,
) {
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let before = state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert_power_paint(&mut visual, device, true);
    let selector = Box::leak(format!("device-bypass-{}", device.value()).into_boxed_str());
    let indicator =
        Box::leak(format!("device-bypass-{}-indicator", device.value()).into_boxed_str());
    let target = visual.debug_bounds(selector).unwrap();
    let paint = visual.rendered_background(indicator);
    visual.simulate_mouse_move(target.center(), None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(visual.rendered_background(selector), None);
    assert_ne!(visual.rendered_background(indicator), paint);
    visual.simulate_mouse_down(target.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(visual.rendered_background(selector), None);
    assert_ne!(visual.rendered_background(indicator), paint);
    visual.simulate_mouse_up(
        point(px(0.0), px(0.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, before)
    });
}

fn assert_order(visual: &mut VisualTestContext, device: DeviceId, remove_prefix: &str) {
    let title = bounds(visual, "device-drag-title", device);
    let bypass = bounds(visual, "device-bypass", device);
    let save = bounds(visual, "device-preset-save", device);
    let load = bounds(visual, "device-preset-load", device);
    let remove = bounds(visual, remove_prefix, device);
    let frame = bounds(
        visual,
        if remove_prefix == "instrument-remove" {
            "instrument-device"
        } else {
            "effect-device"
        },
        device,
    );
    let inset =
        px(hex_widgets::DEVICE_FRAME_HEADER_PADDING + hex_widgets::DEVICE_FRAME_BORDER_WIDTH);
    assert_eq!(
        bypass.left() - frame.left(),
        inset,
        "toggle is the far-left control"
    );
    assert_eq!(
        frame.right() - px(device_meter::WIDTH) - remove.right(),
        inset,
        "X occupies the right inset"
    );
    assert!(
        load.right() <= save.left(),
        "load precedes save without overlap"
    );

    assert!(
        bypass.right() <= title.left(),
        "bypass must precede the name"
    );
    assert!(
        title.size.width >= px(24.0),
        "compact name retains useful space"
    );
    assert!(title.right() <= save.left().min(load.left()));
    assert!(
        save.right() <= remove.left() && load.right() <= remove.left(),
        "X is last"
    );
    for action in [bypass, save, load, remove] {
        assert!(action.size.width >= px(12.0));
        assert_eq!(action.size.height, px(12.0));
        assert!(action.top() >= title.top() && action.bottom() <= title.bottom());
    }
    for prefix in [
        "device-up",
        "device-down",
        "return-device-up",
        "return-device-down",
    ] {
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("{prefix}-{}", device.value()).into_boxed_str()
                ))
                .is_none(),
            "movement buttons removed, title drag remains"
        );
    }
}

fn assert_action_rounding(visual: &mut VisualTestContext, device: DeviceId, remove_prefix: &str) {
    let scale = visual.update(|window, _| window.scale_factor());
    for prefix in ["device-preset-save", "device-preset-load", remove_prefix] {
        let selector = Box::leak(format!("{prefix}-{}", device.value()).into_boxed_str());
        assert_eq!(
            visual
                .rendered_surface_geometry(selector)
                .unwrap()
                .0
                .top_left,
            px(hex_widgets::DEVICE_HEADER_ACTION_SIZE / 2.0).scale(scale),
            "production one-shot header action is circular at its established density"
        );
    }
}

#[gpui::test]
fn visible_production_actions_are_circular_at_existing_header_density(cx: &mut TestAppContext) {
    let (service, state, track, effect, _, _) = actor_utility_fixture();
    let instrument = state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let (handle, _) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    // Paint is clipped at the viewport; compact layout is checked separately.
    visual.simulate_resize(size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    assert_action_rounding(&mut visual, effect, "device-remove");
    assert_action_rounding(&mut visual, instrument, "instrument-remove");
}

#[gpui::test]
fn shared_header_orders_specialized_and_descriptor_panels_at_compact_widths(
    cx: &mut TestAppContext,
) {
    let (service, state, track, effect, _, _) = actor_utility_fixture();
    let instrument = state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let (handle, _) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for width in [1280.0, 880.0] {
        visual.simulate_resize(size(px(width), px(540.0)));
        visual.run_until_parked();
        assert_order(&mut visual, effect, "device-remove");
        assert_order(&mut visual, instrument, "instrument-remove");
    }
    let (service, state, track, operator) = operator_navigation_tests::operator_fixture();
    let (handle, _) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();
    assert_order(&mut visual, operator, "instrument-remove");
}

#[gpui::test]
fn shared_header_return_order_bypass_remove_and_history_keep_owner(cx: &mut TestAppContext) {
    let (service, state, track, return_id, device, _) = actor_return_utility_fixture();
    let before = state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.focused_return = Some(return_id);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();
    assert_order(&mut visual, device, "return-device-remove");
    assert_power_paint(&mut visual, device, true);
    let bypass = bounds(&mut visual, "device-bypass", device);
    visual.simulate_click(bypass.center(), Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .bypassed()
    });
    assert_power_paint(&mut visual, device, false);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.selected_device, Some(device));
        assert!(view.active_native_device_drag.is_none());
        assert_eq!(
            view.state.as_ref().unwrap().project.track(track),
            before.track(track)
        );
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.state.as_ref().unwrap().project == before
    });
    visual.run_until_parked();
    assert_power_paint(&mut visual, device, true);
    let remove = bounds(&mut visual, "return-device-remove", device);
    visual.simulate_click(remove.center(), Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .is_none()
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.state.as_ref().unwrap().project == before
    });
}

#[gpui::test]
fn shared_header_track_effect_and_instrument_actions_preserve_history_without_drag(
    cx: &mut TestAppContext,
) {
    let (service, state, track, effect, _, _) = actor_utility_fixture();
    let before = state.project.clone();
    let instrument = state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for (device, remove_prefix) in [(effect, "device-remove"), (instrument, "instrument-remove")] {
        assert_power_paint(&mut visual, device, true);
        let bypass = bounds(&mut visual, "device-bypass", device);
        visual.simulate_click(bypass.center(), Modifiers::default());
        pump_until(&editor, cx, |view| {
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .bypassed()
        });
        assert_power_paint(&mut visual, device, false);
        editor.read_with(cx, |view, _| {
            assert_eq!(view.device_panel.selected_device, Some(device));
            assert!(view.active_native_device_drag.is_none());
            let other = if device == effect { instrument } else { effect };
            assert!(
                !view
                    .state
                    .as_ref()
                    .unwrap()
                    .project
                    .device(other)
                    .unwrap()
                    .bypassed()
            );
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            view.state.as_ref().unwrap().project == before
        });
        visual.run_until_parked();
        assert_power_paint(&mut visual, device, true);
        let remove = bounds(&mut visual, remove_prefix, device);
        visual.simulate_click(remove.center(), Modifiers::default());
        pump_until(&editor, cx, |view| {
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .ordered_device_chain()
                .all(|node| match node {
                    hex_project::TrackDeviceRef::Instrument(item)
                    | hex_project::TrackDeviceRef::Effect(item) => item.id() != device,
                })
        });
        editor.read_with(cx, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            assert_eq!(project.device(device).is_some(), device == instrument);
            let mut expected_sources = before.snapshot().native_sources;
            if device == instrument {
                let source_id = before.track(track).unwrap().source_id().unwrap().value();
                let source = expected_sources
                    .iter_mut()
                    .find(|source| source.id == source_id)
                    .expect("instrument source remains in the inventory");
                assert_eq!(source.receiver, Some(track.value()));
                source.receiver = None;
            }
            assert_eq!(
                project.snapshot().native_sources,
                expected_sources,
                "removing an instrument unlinks only its source receiver"
            );
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            view.state.as_ref().unwrap().project == before
        });
        visual.run_until_parked();
    }
}
