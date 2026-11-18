//! Blank-space gestures and group move/delete through the real EQ graph and worker.
use super::*;

const SHIFT: Modifiers = Modifiers {
    control: false,
    alt: false,
    shift: true,
    platform: false,
    function: false,
};
const SHIFT_CMD: Modifiers = Modifiers {
    platform: true,
    ..SHIFT
};
const NODE_X: [f32; 3] = [0.3, 0.45, 0.8];

/// Three bell nodes created by double-clicks at known graph positions.
fn three_nodes(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    graph_layout_tests::BrowserRoot,
    DeviceId,
    EqState,
) {
    let (service, state, track, device) = fixture(false);
    let (handle, editor, root) = graph_layout_tests::bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    for (count, x) in NODE_X.into_iter().enumerate() {
        let at = plot(&mut visual, device, x, 0.5);
        eq_double_click(&mut visual, at);
        pump_until(&editor, cx, |view| {
            settled(view) && read(view, device).nodes.len() == count + 1
        });
    }
    let before = editor.read_with(cx, |view, _| read(view, device));
    (handle, editor, root, device, before)
}

fn marquee(
    visual: &mut VisualTestContext,
    device: DeviceId,
    from: [f32; 2],
    to: [f32; 2],
    modifiers: Modifiers,
) {
    let from = plot(visual, device, from[0], from[1]);
    let to = plot(visual, device, to[0], to[1]);
    visual.simulate_mouse_down(from, MouseButton::Left, modifiers);
    visual.simulate_mouse_move(to, Some(MouseButton::Left), modifiers);
    visual.simulate_mouse_up(to, MouseButton::Left, modifiers);
    visual.run_until_parked();
}

fn group(editor: &gpui::Entity<EditorView>, cx: &mut TestAppContext, device: DeviceId) -> Vec<u32> {
    editor.read_with(cx, |view, _| {
        let mut ids: Vec<_> = eq_panel::selection_for_test(view, device)
            .1
            .iter()
            .map(|id| id.value())
            .collect();
        ids.sort_unstable();
        ids
    })
}

#[gpui::test]
fn eq_blank_drag_selects_nodes_shift_cmd_toggles_and_group_delete_undoes_in_one_step(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _root, device, before) = three_nodes(cx);
    let ids: Vec<u32> = before.nodes.iter().map(|node| node.id.value()).collect();
    let mut visual = VisualTestContext::from_window(handle, cx);
    let plain = Modifiers::default();
    marquee(&mut visual, device, [0.5, 0.7], [0.25, 0.3], plain);
    assert_eq!(group(&editor, cx, device), ids[..2]);
    editor.read_with(cx, |view, _| {
        assert_eq!(read(view, device), before, "selection authors nothing");
        assert!(settled(view));
    });
    marquee(&mut visual, device, [0.75, 0.7], [0.85, 0.3], SHIFT_CMD);
    assert_eq!(group(&editor, cx, device), ids);
    let blank = plot(&mut visual, device, 0.6, 0.7);
    visual.simulate_click(blank, plain);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(eq_panel::selection_for_test(view, device), (None, vec![]));
        assert_eq!(read(view, device), before, "a blank click authors nothing");
        assert!(settled(view));
    });
    marquee(&mut visual, device, [0.75, 0.7], [0.85, 0.3], plain);
    editor.read_with(cx, |view, _| {
        assert_eq!(
            eq_panel::selection_for_test(view, device),
            (Some(before.nodes[2].id), vec![])
        );
    });
    marquee(&mut visual, device, [0.25, 0.7], [0.5, 0.3], plain);
    visual.simulate_keystrokes("backspace");
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes.len() == 1
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(read(view, device).nodes[0].id.value(), ids[2]);
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == before
    });
}

#[gpui::test]
fn eq_group_drag_moves_every_selected_node_and_undoes_in_one_step(cx: &mut TestAppContext) {
    let (handle, editor, _root, device, before) = three_nodes(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    marquee(
        &mut visual,
        device,
        [0.25, 0.7],
        [0.5, 0.3],
        Modifiers::default(),
    );
    assert_eq!(group(&editor, cx, device).len(), 2);
    let start = plot(&mut visual, device, NODE_X[0], 0.5);
    let end = plot(&mut visual, device, NODE_X[0] + 0.05, 0.65);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) != before
    });
    let after = editor.read_with(cx, |view, _| read(view, device));
    let gain = |state: &EqState, index: usize| state.nodes[index].gain_db;
    let frequency = |state: &EqState, index: usize| state.nodes[index].frequency_hz;
    let raised = gain(&after, 0) - gain(&before, 0);
    assert!(raised > 5.0, "first node gain rose by {raised} dB");
    assert!(
        (gain(&after, 1) - gain(&before, 1) - raised).abs() < 0.05,
        "grouped nodes share the same gain delta"
    );
    // Equal horizontal travel on a log axis keeps the frequency ratio.
    let ratio = frequency(&after, 0) / frequency(&before, 0);
    assert!(ratio > 1.0);
    assert!((frequency(&after, 1) / frequency(&before, 1) - ratio).abs() < 0.01);
    assert_eq!(after.nodes[2], before.nodes[2], "unselected node stays put");
    assert_eq!(
        group(&editor, cx, device).len(),
        2,
        "a drag keeps the group"
    );
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == before
    });
}

#[gpui::test]
fn eq_double_click_drag_creates_one_node_at_release_and_undoes_in_one_step(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _root, device, before) = three_nodes(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let start = plot(&mut visual, device, 0.6, 0.8);
    let end = plot(&mut visual, device, 0.7, 0.625);
    eq_double_click_down(&mut visual, start);
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(read(view, device), before, "creation is still held");
        assert!(view.device_gesture.snapshot().is_some());
    });
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes.len() == before.nodes.len() + 1
    });
    editor.read_with(cx, |view, _| {
        let after = read(view, device);
        assert_eq!(&after.nodes[..before.nodes.len()], &before.nodes);
        let created = after.nodes.last().unwrap();
        assert!((created.frequency_hz - 20.0 * 1000.0_f32.powf(0.7)).abs() < 0.1);
        assert!((created.gain_db - 6.0).abs() < 0.01);
        assert_eq!(
            eq_panel::selection_for_test(view, device),
            (Some(created.id), vec![])
        );
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == before
    });
}
