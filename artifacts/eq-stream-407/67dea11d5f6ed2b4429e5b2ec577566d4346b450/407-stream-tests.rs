
#[gpui::test]
fn eq_review_stream_boundary_wheel_outer_hits_but_not_blank_or_outside(cx: &mut TestAppContext) {
    let (handle, editor, _root, device) = node_window(cx, 0.02);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let initial = editor.read_with(cx, |view, _| read(view, device));
    let mid_q = initial.nodes[0].q;
    for (frequency, q, x, y, outward) in [
        (20.0, mid_q, 0.0, 0.5, point(px(-4.0), px(0.0))),
        (20000.0, mid_q, 1.0, 0.5, point(px(4.0), px(0.0))),
        (initial.nodes[0].frequency_hz, 0.1, 0.02, 0.0, point(px(0.0), px(4.0))),
        (initial.nodes[0].frequency_hz, 18.0, 0.02, 1.0, point(px(0.0), px(-4.0))),
    ] {
        let mut before = initial.clone();
        before.nodes[0].frequency_hz = frequency;
        before.nodes[0].q = q;
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Edit(Command::ReplaceDeviceState { device, state: before.encode().unwrap() }))));
        pump_until(&editor, cx, |view| settled(view) && read(view, device) == before);
        visual.run_until_parked();
        let center = plot(&mut visual, device, x, y);
        let hit = center + outward;
        wheel(&mut visual, hit, gpui::ScrollDelta::Lines(point(0.0, 1.0)), gpui::TouchPhase::Started, false);
        end_wheel(&mut visual, hit);
        pump_until(&editor, cx, settled);
        let mut expected = before.clone(); expected.nodes[0].slope = 1;
        assert_eq!(editor.read_with(cx, |view, _| read(view, device)), expected, "outer visible half frequency{frequency}/q{q}");
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| settled(view) && read(view, device) == before);
        for outside in [center + outward.map(|value| value * 2.5), hit + point(px(24.0), px(24.0))] {
            wheel(&mut visual, outside, gpui::ScrollDelta::Lines(point(0.0, 1.0)), gpui::TouchPhase::Started, false);
            end_wheel(&mut visual, outside);
            pump_until(&editor, cx, settled);
            assert_eq!(editor.read_with(cx, |view, _| read(view, device)), before, "blank/outside full radius must not clamp into node");
        }
    }
}

#[gpui::test]
fn eq_review_stream_physical_pause_keeps_one_history_until_end_including_zero_start(cx: &mut TestAppContext) {
    let mut completed = 0;
    for zero_start in [false, true] {
        let cx = &mut cx.new_app();
        let (handle, editor, _root, device) = node_window(cx, 0.02);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let before = editor.read_with(cx, |view, _| read(view, device));
        let graph = editor.read_with(cx, |view, _| eq_panel::graph_for_test(view, device));
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::<hex_widgets::ResponseGraphScrollEvent>::new()));
        let captured = events.clone();
        let _subscription = graph.update(cx, |_, cx| cx.subscribe(&graph, move |_, _, event: &hex_widgets::ResponseGraphScrollEvent, _| captured.borrow_mut().push(*event)));
        let position = plot(&mut visual, device, 0.02, 0.5);
        wheel(&mut visual, position, gpui::ScrollDelta::Lines(point(0.0, if zero_start {0.0} else {1.0})), gpui::TouchPhase::Started, false);
        if zero_start {
            assert!(editor.read_with(cx, |view, _| view.device_gesture.snapshot().is_none()));
            assert!(events.borrow().is_empty(), "zeroY start must not begin/consume a parameter edit");
        }
        wheel(&mut visual, position, gpui::ScrollDelta::Lines(point(0.0, 1.0)), gpui::TouchPhase::Moved, false);
        let first_slope = if zero_start {1} else {2};
        pump_until(&editor, cx, |view| view.worker_requests_pending == 0 && view.pending_requests.is_empty() && read(view, device).nodes[0].slope == first_slope);
        cx.executor().advance_clock(std::time::Duration::from_millis(300));
        visual.run_until_parked();
        assert!(editor.read_with(cx, |view, _| view.device_gesture.snapshot().is_some()), "physical fingers still held after >250ms, zero_start={zero_start}");
        assert_eq!(events.borrow().iter().filter(|e| e.phase == hex_widgets::ResponseGraphScrollPhase::Commit).count(), 0);
        wheel(&mut visual, position, gpui::ScrollDelta::Lines(point(0.0, 1.0)), gpui::TouchPhase::Moved, false);
        end_wheel(&mut visual, position);
        pump_until(&editor, cx, settled);
        let after = editor.read_with(cx, |view, _| read(view, device));
        assert_eq!(after.nodes[0].slope, first_slope + 1);
        let events = events.borrow();
        assert_eq!(events.iter().filter(|e| e.phase == hex_widgets::ResponseGraphScrollPhase::Begin).count(), 1);
        assert_eq!(events.iter().filter(|e| e.phase == hex_widgets::ResponseGraphScrollPhase::Commit).count(), 1);
        assert!(events.iter().all(|event| event.gesture_id == events[0].gesture_id));
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| settled(view) && read(view, device) == before);
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
        pump_until(&editor, cx, |view| settled(view) && read(view, device) == after);
        completed += 1;
    }
    assert_eq!(completed, 2);
}
