//! Pointer ownership through actual graph presses, with independent wheel streams.
use super::*;
use gpui::{AppContext, Render, TestAppContext, VisualTestContext, size};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
struct Host {
    graph: Entity<ResponseGraph>,
    actions: Rc<Cell<usize>>,
    escape: Rc<Cell<usize>>,
}
impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let down = self.actions.clone();
        let up = self.actions.clone();
        let wheel = self.actions.clone();
        let escape = self.escape.clone();
        let mut slot = div().w(px(240.)).h(px(120.));
        div()
            .size_full()
            .on_key_down(move |event, _, _| {
                if event.keystroke.key == "escape" {
                    escape.set(escape.get() + 1);
                }
            })
            .child(gpui::AnyView::from(self.graph.clone()).cached(slot.style().clone()))
            .child(
                div()
                    .id("graph-other")
                    .absolute()
                    .left(px(300.))
                    .top_0()
                    .w(px(100.))
                    .h(px(120.))
                    .cursor(CursorStyle::Crosshair)
                    .on_mouse_down(MouseButton::Right, move |_, _, _| down.set(down.get() + 1))
                    .on_mouse_up(MouseButton::Left, move |_, _, _| up.set(up.get() + 1))
                    .on_scroll_wheel(move |_, _, _| wheel.set(wheel.get() + 1)),
            )
    }
}
struct Fixture {
    handle: gpui::AnyWindowHandle,
    graph: Entity<ResponseGraph>,
    actions: Rc<Cell<usize>>,
    escape: Rc<Cell<usize>>,
}
fn fixture(cx: &mut TestAppContext, creation: bool) -> Fixture {
    let actions = Rc::new(Cell::new(0));
    let escape = Rc::new(Cell::new(0));
    let handle = cx.add_window(|_, cx| Host {
        graph: cx.new(|cx| {
            let mut g = ResponseGraph::new(cx, "capture-graph", crate::tests::STYLE);
            g.set_editable(true, cx);
            if creation {
                g.set_creation_enabled(true);
                g.set_creation_drag_enabled(true);
            } else {
                let mut model = g.model.clone();
                model.nodes = vec![ResponseGraphNode {
                    id: 1,
                    position: [0.5, 0.5],
                    enabled: true,
                    selected: true,
                }];
                g.set_model(model, crate::tests::STYLE, cx).unwrap();
                g.set_scroll_nodes(vec![1], cx).unwrap();
            }
            g
        }),
        actions: actions.clone(),
        escape: escape.clone(),
    });
    let graph = handle.read_with(cx, |h, _| h.graph.clone()).unwrap();
    Fixture {
        handle: *handle,
        graph,
        actions,
        escape,
    }
}
#[gpui::test]
fn graph_pointer_isolation_preserves_outside_terminal_and_independent_wheel(
    cx: &mut TestAppContext,
) {
    let Fixture {
        handle,
        graph,
        actions,
        ..
    } = fixture(cx, false);
    let phases = Rc::new(RefCell::new(Vec::new()));
    let target = phases.clone();
    let _sub = cx.update(|cx| {
        cx.subscribe(&graph, move |_, e: &ResponseGraphEvent, _| {
            target.borrow_mut().push(e.phase)
        })
    });
    let scroll = Rc::new(RefCell::new(Vec::new()));
    let target = scroll.clone();
    let _wheel = cx.update(|cx| {
        cx.subscribe(&graph, move |_, e: &ResponseGraphScrollEvent, _| {
            target.borrow_mut().push(e.phase)
        })
    });
    let mut v = VisualTestContext::from_window(handle, cx);
    v.simulate_resize(size(px(440.), px(150.)));
    v.run_until_parked();
    let center = graph.read_with(cx, |g, _| g.plot_bounds().unwrap().center());
    let outside = point(px(340.), px(60.));
    v.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
    assert!(graph.read_with(cx, |g, _| g.press.is_some()));
    v.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
    v.simulate_event(gpui::ScrollWheelEvent {
        position: outside,
        delta: gpui::ScrollDelta::Lines(point(0., 1.)),
        ..Default::default()
    });
    assert_eq!(
        actions.get(),
        0,
        "a graph press must isolate unrelated buttons/wheel before movement"
    );
    assert!(
        scroll.borrow().is_empty(),
        "held pointers retain the existing no-wheel policy"
    );
    v.simulate_mouse_move(outside, Some(MouseButton::Left), Modifiers::default());
    v.run_until_parked();
    assert_eq!(v.rendered_cursor_style(), Some(CursorStyle::ResizeColumn));
    v.update(|w, cx| {
        w.root::<Host>()
            .unwrap()
            .unwrap()
            .update(cx, |_, cx| cx.notify())
    });
    v.run_until_parked();
    v.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    v.simulate_mouse_up(outside, MouseButton::Right, Modifiers::default());
    v.run_until_parked();
    assert_eq!(actions.get(), 0);
    assert_eq!(
        phases
            .borrow()
            .iter()
            .filter(|p| **p == ResponseGraphPhase::Commit)
            .count(),
        1
    );
    assert_eq!(
        phases.borrow().last(),
        Some(&ResponseGraphPhase::ReleaseOutside)
    );
    assert_eq!(v.rendered_cursor_style(), Some(CursorStyle::Crosshair));
    // A wheel stream has no pressed pointer lease, and its outside terminal must
    // still reach both the observing graph and the unrelated panel's bubble path.
    v.simulate_mouse_move(center, None, Modifiers::default());
    v.simulate_event(gpui::ScrollWheelEvent {
        position: center,
        delta: gpui::ScrollDelta::Lines(point(0., 1.)),
        touch_phase: gpui::TouchPhase::Moved,
        ..Default::default()
    });
    v.run_until_parked();
    assert!(graph.read_with(cx, |g, _| g.scroll.active()));
    assert!(!v.update(|w, _| w.is_pointer_dragging()));
    v.simulate_event(gpui::ScrollWheelEvent {
        position: outside,
        touch_phase: gpui::TouchPhase::Ended,
        ..Default::default()
    });
    v.run_until_parked();
    assert_eq!(
        scroll.borrow().last(),
        Some(&ResponseGraphScrollPhase::Commit)
    );
    assert_eq!(
        actions.get(),
        1,
        "an independent wheel terminal still bubbles outside"
    );
}
#[gpui::test]
fn graph_escape_keeps_its_keyboard_guard_for_node_and_creation_pointer(cx: &mut TestAppContext) {
    for (creation, dragged) in [(false, false), (false, true), (true, false), (true, true)] {
        let Fixture {
            handle,
            graph,
            actions,
            escape: parent_escape,
        } = fixture(cx, creation);
        let phases = Rc::new(RefCell::new(Vec::new()));
        let target = phases.clone();
        let _sub = cx.update(|cx| {
            cx.subscribe(&graph, move |_, e: &ResponseGraphEvent, _| {
                target.borrow_mut().push(e.phase)
            })
        });
        let mut v = VisualTestContext::from_window(handle, cx);
        v.simulate_resize(size(px(440.), px(150.)));
        v.run_until_parked();
        let center = graph.read_with(cx, |g, _| g.plot_bounds().unwrap().center());
        v.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
        if dragged {
            v.simulate_mouse_move(
                center + point(px(15.), px(10.)),
                Some(MouseButton::Left),
                Modifiers::default(),
            );
        }
        v.run_until_parked();
        assert!(graph.read_with(cx, |g, _| if dragged {
            g.drag.is_some()
        } else {
            g.press.is_some()
        }));
        v.simulate_keystrokes("escape");
        v.run_until_parked();
        assert_eq!(
            parent_escape.get(),
            0,
            "the graph's existing Escape guard must run before pointer retirement"
        );
        assert_eq!(
            phases
                .borrow()
                .iter()
                .filter(|p| **p == ResponseGraphPhase::Cancel)
                .count(),
            1
        );
        assert!(!phases.borrow().contains(&ResponseGraphPhase::Commit));
        assert!(graph.read_with(cx, |g, _| g.press.is_none() && g.drag.is_none()));
        v.simulate_mouse_up(
            point(px(340.), px(60.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(
            actions.get(),
            0,
            "cancelled pointer release must not activate another panel"
        );
    }
}

/// CPU-only node/creation pointer comparison through complete update turns.
#[gpui::test]
#[ignore = "manual optimized graph pointer/CPU-scene profile; no native acceptance"]
fn profile_graph_pointer(cx: &mut TestAppContext) {
    let smoke = std::env::var_os("HEX_GRAPH_POINTER_PROFILE_SMOKE").is_some_and(|v| v == "1");
    assert!(
        !cfg!(debug_assertions) || smoke,
        "timing requires --release; debug smoke must be explicit"
    );
    let warmup = if smoke { 1 } else { 16 };
    let samples = if smoke { 4 } else { 128 };
    for creation in [false, true] {
        let Fixture { handle, graph, .. } = fixture(cx, creation);
        let mut v = VisualTestContext::from_window(handle, cx);
        v.simulate_resize(size(px(440.), px(150.)));
        v.run_until_parked();
        let bounds = graph.read_with(cx, |g, _| g.plot_bounds().unwrap());
        let mut timings = Vec::with_capacity(samples);
        for round in 0..warmup + samples {
            v.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
            v.run_until_parked();
            assert!(graph.read_with(cx, |g, _| g.press.is_some()));
            let start = std::time::Instant::now();
            let mut at = bounds.center();
            for step in 0..8 {
                at = bounds.origin
                    + point(
                        bounds.size.width * (0.1 + ((round + step) % 7) as f32 * 0.1),
                        bounds.size.height * 0.4,
                    );
                v.simulate_mouse_move(at, Some(MouseButton::Left), Modifiers::default());
                v.run_until_parked();
            }
            let elapsed = start.elapsed().as_secs_f64() * 1_000_000.;
            v.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
            v.run_until_parked();
            if round >= warmup {
                timings.push(elapsed);
            }
        }
        timings.sort_by(f64::total_cmp);
        let percentile = |p: usize| timings[(timings.len() - 1) * p / 100];
        let scale = v.update(|w, _| w.scale_factor());
        println!(
            "GRAPH_POINTER_PROFILE creation={creation} size=440x150 scale={scale} warmup={warmup} samples={samples} debug_smoke={smoke} completed_updates_per_sample=8 headless_cpu_only=true p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3}",
            percentile(50),
            percentile(95),
            percentile(99),
            percentile(100)
        );
    }
}

#[gpui::test]
fn graph_x_removal_is_opt_in_and_does_not_consume_modified_keys(cx: &mut TestAppContext) {
    cx.update(super::bind_response_graph_keys);
    let Fixture { handle, graph, .. } = fixture(cx, false);
    let phases = Rc::new(RefCell::new(Vec::new()));
    let target = phases.clone();
    let _subscription = cx.update(|cx| {
        cx.subscribe(&graph, move |_, event: &ResponseGraphEvent, _| {
            target.borrow_mut().push(event.phase);
        })
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(440.), px(150.)));
    visual.run_until_parked();
    visual.update(|window, cx| graph.update(cx, |graph, _| graph.focus.focus(window)));
    visual.simulate_keystrokes("x");
    assert!(!phases.borrow().contains(&ResponseGraphPhase::Remove));
    graph.update(&mut visual, |graph, cx| {
        graph.set_backspace_removal(true);
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-x");
    assert!(!phases.borrow().contains(&ResponseGraphPhase::Remove));
    visual.simulate_keystrokes("x");
    assert_eq!(
        phases
            .borrow()
            .iter()
            .filter(|phase| **phase == ResponseGraphPhase::Remove)
            .count(),
        1
    );
}
