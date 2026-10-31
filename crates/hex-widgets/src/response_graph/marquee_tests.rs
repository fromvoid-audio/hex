//! Opt-in marquee selection through actual graph presses.
use super::*;
use gpui::{AppContext, Render, TestAppContext, VisualTestContext, size};
use std::{cell::RefCell, rc::Rc};

struct Host {
    graph: Entity<ResponseGraph>,
}
impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut slot = div().w(px(240.)).h(px(120.));
        div()
            .size_full()
            .child(gpui::AnyView::from(self.graph.clone()).cached(slot.style().clone()))
    }
}

#[derive(Debug, PartialEq)]
enum Seen {
    Node(ResponseGraphPhase, u32),
    Marquee(ResponseGraphMarqueePhase, Vec<u32>),
}

struct Fixture {
    visual: VisualTestContext,
    graph: Entity<ResponseGraph>,
    seen: Rc<RefCell<Vec<Seen>>>,
    _subscriptions: [gpui::Subscription; 2],
}

const NODES: [(u32, [f32; 2]); 3] = [(1, [0.2, 0.8]), (2, [0.3, 0.7]), (3, [0.8, 0.2])];

fn fixture(cx: &mut TestAppContext, creation: bool, selected: &[u32]) -> Fixture {
    let selected = selected.to_vec();
    let handle = cx.add_window(|_, cx| Host {
        graph: cx.new(|cx| {
            let mut graph = ResponseGraph::new(cx, "marquee-graph", crate::tests::STYLE);
            graph.set_editable(true, cx);
            graph.set_creation_enabled(creation);
            graph.set_creation_drag_enabled(creation);
            graph.set_marquee_enabled(true, cx);
            let mut model = graph.model.clone();
            model.nodes = NODES
                .iter()
                .map(|(id, position)| ResponseGraphNode {
                    id: *id,
                    position: *position,
                    enabled: true,
                    selected: selected.contains(id),
                })
                .collect();
            graph.set_model(model, crate::tests::STYLE, cx).unwrap();
            graph
        }),
    });
    let graph = handle.read_with(cx, |host, _| host.graph.clone()).unwrap();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let nodes = seen.clone();
    let bands = seen.clone();
    let subscriptions = cx.update(|cx| {
        [
            cx.subscribe(&graph, move |_, event: &ResponseGraphEvent, _| {
                nodes
                    .borrow_mut()
                    .push(Seen::Node(event.phase, event.node_id))
            }),
            cx.subscribe(&graph, move |_, event: &ResponseGraphMarqueeEvent, _| {
                bands
                    .borrow_mut()
                    .push(Seen::Marquee(event.phase, event.selected.clone()))
            }),
        ]
    });
    let visual = VisualTestContext::from_window(*handle, cx);
    visual.simulate_resize(size(px(300.), px(150.)));
    visual.run_until_parked();
    Fixture {
        visual,
        graph,
        seen,
        _subscriptions: subscriptions,
    }
}

fn at(fixture: &mut Fixture, [x, y]: [f32; 2]) -> Point<Pixels> {
    let plot = fixture
        .graph
        .read_with(&fixture.visual, |graph, _| graph.plot_bounds().unwrap());
    point(
        plot.left() + plot.size.width * x,
        plot.top() + plot.size.height * (1.0 - y),
    )
}

fn drag(fixture: &mut Fixture, from: [f32; 2], to: [f32; 2], modifiers: Modifiers) {
    let (from, to) = (at(fixture, from), at(fixture, to));
    let v = &mut fixture.visual;
    v.simulate_mouse_down(from, MouseButton::Left, modifiers);
    v.simulate_mouse_move(to, Some(MouseButton::Left), modifiers);
    v.simulate_mouse_up(to, MouseButton::Left, modifiers);
    v.run_until_parked();
}

fn marquee_commits(seen: &[Seen]) -> Vec<Vec<u32>> {
    seen.iter()
        .filter_map(|seen| match seen {
            Seen::Marquee(ResponseGraphMarqueePhase::Commit, ids) => Some(ids.clone()),
            _ => None,
        })
        .collect()
}

#[gpui::test]
fn blank_drag_selects_nodes_inside_the_band_in_any_direction(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, false, &[3]);
    // Drag up-left so the band origin is its bottom-right corner.
    drag(&mut fixture, [0.4, 0.6], [0.1, 0.9], Modifiers::default());
    let seen = fixture.seen.take();
    assert_eq!(
        seen.first(),
        Some(&Seen::Marquee(
            ResponseGraphMarqueePhase::Change,
            vec![1, 2]
        ))
    );
    assert_eq!(marquee_commits(&seen), vec![vec![1, 2]]);
    assert!(
        !seen.iter().any(|seen| matches!(seen, Seen::Node(..))),
        "a marquee never creates, selects or drags a node"
    );
}

#[gpui::test]
fn shift_adds_and_cmd_toggles_against_the_press_time_selection(cx: &mut TestAppContext) {
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    let cmd = Modifiers {
        platform: true,
        ..Modifiers::default()
    };
    let mut fixture = fixture(cx, false, &[3]);
    drag(&mut fixture, [0.1, 0.9], [0.4, 0.6], shift);
    assert_eq!(marquee_commits(&fixture.seen.take()), vec![vec![1, 2, 3]]);
    // Cmd over nodes 2 and 3 toggles them against the base {3}.
    drag(&mut fixture, [0.25, 0.75], [0.9, 0.1], cmd);
    assert_eq!(marquee_commits(&fixture.seen.take()), vec![vec![2]]);
}

#[gpui::test]
fn blank_click_clears_or_creates_on_release_and_escape_restores(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, false, &[2]);
    let blank = at(&mut fixture, [0.6, 0.6]);
    fixture.visual.simulate_click(blank, Modifiers::default());
    assert_eq!(
        marquee_commits(&fixture.seen.take()),
        vec![Vec::<u32>::new()]
    );

    let mut fixture = self::fixture(cx, true, &[]);
    let blank = at(&mut fixture, [0.6, 0.6]);
    fixture
        .visual
        .simulate_mouse_down(blank, MouseButton::Left, Modifiers::default());
    assert!(
        fixture.seen.borrow().is_empty(),
        "creation waits for release so a drag can become a marquee"
    );
    fixture
        .visual
        .simulate_mouse_up(blank, MouseButton::Left, Modifiers::default());
    assert_eq!(
        fixture.seen.take(),
        vec![Seen::Node(ResponseGraphPhase::Create, 0)]
    );

    let mut fixture = self::fixture(cx, true, &[3]);
    let (from, to) = (at(&mut fixture, [0.1, 0.9]), at(&mut fixture, [0.4, 0.6]));
    let v = &mut fixture.visual;
    v.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    v.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    v.simulate_keystrokes("escape");
    v.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    v.run_until_parked();
    assert_eq!(
        fixture.seen.take(),
        vec![
            Seen::Marquee(ResponseGraphMarqueePhase::Change, vec![1, 2]),
            Seen::Marquee(ResponseGraphMarqueePhase::Cancel, vec![3]),
        ],
        "escape restores the press-time selection and release creates nothing"
    );
}

#[gpui::test]
fn node_presses_keep_their_select_and_drag_stream(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, true, &[]);
    drag(&mut fixture, [0.8, 0.2], [0.7, 0.3], Modifiers::default());
    let seen = fixture.seen.take();
    assert_eq!(
        seen.iter()
            .map(|seen| match seen {
                Seen::Node(phase, 3) => Some(*phase),
                _ => None,
            })
            .collect::<Vec<_>>(),
        [
            ResponseGraphPhase::Select,
            ResponseGraphPhase::Press,
            ResponseGraphPhase::Begin,
            ResponseGraphPhase::Change,
            ResponseGraphPhase::Commit,
            ResponseGraphPhase::Release,
        ]
        .map(Some)
    );
}
