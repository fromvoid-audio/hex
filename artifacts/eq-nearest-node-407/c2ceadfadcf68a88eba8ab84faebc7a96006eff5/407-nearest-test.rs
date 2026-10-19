
#[gpui::test]
fn eq_review_nearest_unsupported_node_blocks_nearby_cut_and_bubbles(cx: &mut TestAppContext) {
    let mut completed = 0;
    for kind in [hex_engine::eq::NodeKind::Bell, hex_engine::eq::NodeKind::Notch] {
        let cx = &mut cx.new_app();
        let (handle, editor, _root, device) = node_window(cx, 0.02);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let bell = plot(&mut visual, device, 0.5, 0.5);
        visual.simulate_click(bell, Modifiers::default());
        pump_until(&editor, cx, |view| settled(view) && read(view, device).nodes.len() == 2);
        visual.simulate_resize(size(px(880.0), px(650.0)));
        visual.run_until_parked();
        let width = visual.debug_bounds(selector("eq-response", device)).unwrap().size.width - px(14.0);
        let frequency = 20.0 * 1000.0_f32.powf(0.02 + 4.0 / f32::from(width));
        let id = editor.read_with(cx, |view, _| read(view, device).nodes[1].id);
        for (parameter, value) in [
            (hex_engine::eq::NodeParameter::Kind, ParameterValue::Choice(kind as u32)),
            (hex_engine::eq::NodeParameter::Frequency, ParameterValue::Float(frequency)),
        ] {
            editor.update(cx, |view, _| assert!(view.enqueue(Request::SetDeviceParameter {
                device, parameter: id.parameter(parameter).unwrap(), value,
            })));
            pump_until(&editor, cx, settled);
        }
        visual.run_until_parked();
        let before = editor.read_with(cx, |view, _| read(view, device));
        let cut_y = (before.nodes[0].q / 0.1).ln() / (18.0_f32 / 0.1).ln();
        // Align both real node heights: notch Q uses the cut axis; Bell gain
        // defaults to zero, matching the node created at the plot midpoint.
        let x = (frequency / 20.0).ln() / 1000.0_f32.ln();
        let y = if kind == hex_engine::eq::NodeKind::Bell { 0.5 } else { cut_y };
        let position = plot(&mut visual, device, x, y);
        let cut_position = plot(&mut visual, device, 0.02, cut_y);
        let delta = position - cut_position;
        let distance = (f32::from(delta.x).powi(2) + f32::from(delta.y).powi(2)).sqrt();
        assert!(distance > 0.0 && distance < 8.0, "actual nodes must overlap hit radii: {distance}");
        wheel(&mut visual, position, gpui::ScrollDelta::Lines(point(0.0, 1.0)), gpui::TouchPhase::Started, false);
        end_wheel(&mut visual, position);
        pump_until(&editor, cx, settled);
        assert_eq!(editor.read_with(cx, |view, _| read(view, device)), before,
            "nearest unsupported {kind:?} must not edit the obscured cut");
        assert!(editor.read_with(cx, |view, _| view.device_gesture.snapshot().is_none()));
        editor.update(cx, |view, cx| {
            let scroll = &view.device_panel.chain_window.scroll;
            assert!(scroll.max_offset().width > px(48.0));
            scroll.set_offset(point(-scroll.max_offset().width / 2.0, px(0.0)));
            cx.notify();
        });
        visual.run_until_parked();
        let position = plot(&mut visual, device, x, y);
        let offset = editor.read_with(cx, |view, _| view.device_panel.chain_window.scroll.offset());
        wheel(&mut visual, position, gpui::ScrollDelta::Pixels(point(px(-12.0), px(24.0))), gpui::TouchPhase::Moved, false);
        assert_ne!(editor.read_with(cx, |view, _| view.device_panel.chain_window.scroll.offset()), offset,
            "unsupported nearest must bubble diagonal wheel to actual outer chain");
        assert_eq!(editor.read_with(cx, |view, _| read(view, device)), before);
        completed += 1;
    }
    assert_eq!(completed, 2, "both Bell and Notch overlap cases executed");
}
