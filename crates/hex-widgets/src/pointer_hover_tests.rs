//! Completed-scene regression for hover while a raw capture handler consumes movement.
use gpui::{
    Context, IntoElement, Modifiers, MouseButton, MouseMoveEvent, Render, TestAppContext,
    VisualTestContext, Window, canvas, div, point, prelude::*, px, rgb, size,
};

#[derive(Default)]
struct Fixture {
    hovered: std::rc::Rc<std::cell::Cell<bool>>,
    tooltip_builds: std::rc::Rc<std::cell::Cell<usize>>,
}
impl Render for Fixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let hovered = self.hovered.clone();
        let tooltip_builds = self.tooltip_builds.clone();
        let callback_color = if self.hovered.get() {
            0xff0000
        } else {
            0x111111
        };
        div()
            .size_full()
            .child(
                canvas(
                    |_, _, _| (),
                    |_, _, window, _| {
                        window.on_mouse_event(|event: &MouseMoveEvent, phase, _, cx| {
                            if phase.capture() && event.pressed_button.is_some() {
                                cx.stop_propagation();
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .id("hover-target")
                    .debug_selector(|| "hover-target".into())
                    .absolute()
                    .left(px(100.))
                    .top(px(20.))
                    .size(px(60.))
                    .bg(rgb(0x111111))
                    .hover(|style| style.bg(rgb(0xff0000)))
                    .on_hover(move |value, _, _| hovered.set(*value))
                    .hoverable_tooltip(move |_, cx| {
                        tooltip_builds.set(tooltip_builds.get() + 1);
                        cx.new(|_| TooltipFixture).into()
                    }),
            )
            .child(
                div()
                    .id("callback-target")
                    .debug_selector(|| "callback-target".into())
                    .absolute()
                    .left(px(200.))
                    .top(px(20.))
                    .size(px(60.))
                    .bg(rgb(callback_color)),
            )
    }
}

#[gpui::test]
fn consumed_drag_move_clears_hover_across_completed_repaints(cx: &mut TestAppContext) {
    let hovered = std::rc::Rc::new(std::cell::Cell::new(false));
    let tooltip_builds = std::rc::Rc::new(std::cell::Cell::new(0));
    let handle = cx.add_window(|_, cx| {
        CachedHoverRoot(cx.new(|_| Fixture {
            hovered: hovered.clone(),
            tooltip_builds: tooltip_builds.clone(),
        }))
    });
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.simulate_resize(size(px(320.), px(120.)));
    visual.simulate_mouse_move(point(px(120.), px(40.)), None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_background("hover-target"),
        Some(rgb(0xff0000).into())
    );
    assert!(hovered.get());
    visual.simulate_mouse_down(
        point(px(20.), px(40.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(
        point(px(120.), px(40.)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_background("hover-target"),
        Some(rgb(0x111111).into())
    );
    assert!(
        !hovered.get(),
        "consumed move must retire the hover callback state"
    );
    assert_eq!(
        visual.rendered_background("callback-target"),
        Some(rgb(0x111111).into()),
        "callback-derived paint must clear on the first completed drag frame"
    );
    visual
        .executor()
        .advance_clock(std::time::Duration::from_secs(1));
    visual.run_until_parked();
    assert_eq!(
        tooltip_builds.get(),
        0,
        "held drag must not build a pending tooltip"
    );
    visual.update(|window, cx| {
        let root = window.root::<CachedHoverRoot>().unwrap().unwrap();
        root.update(cx, |_, cx| cx.notify());
    });
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_background("hover-target"),
        Some(rgb(0x111111).into())
    );
    visual.simulate_mouse_up(
        point(px(120.), px(40.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_background("hover-target"),
        Some(rgb(0xff0000).into())
    );
}

#[gpui::test]
fn hover_recovers_after_button_loss_escape_and_window_deactivation(cx: &mut TestAppContext) {
    let handle = cx.add_window(|_, _| Fixture::default());
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.simulate_resize(size(px(320.), px(120.)));
    for terminal in 0..3 {
        visual.update(|window, _| window.activate_window());
        visual.simulate_mouse_down(
            point(px(20.), px(40.)),
            MouseButton::Right,
            Modifiers::default(),
        );
        visual.simulate_mouse_move(
            point(px(120.), px(40.)),
            Some(MouseButton::Right),
            Modifiers::default(),
        );
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_background("hover-target"),
            Some(rgb(0x111111).into())
        );
        match terminal {
            0 => visual.simulate_mouse_move(point(px(120.), px(40.)), None, Modifiers::default()),
            1 => visual.simulate_keystrokes("escape"),
            _ => visual.deactivate_window(),
        }
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_background("hover-target"),
            Some(rgb(0xff0000).into())
        );
    }
}

struct DropFixture {
    drops: std::rc::Rc<std::cell::Cell<usize>>,
}
impl Render for DropFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let drops = self.drops.clone();
        div()
            .size_full()
            .child(
                div()
                    .id("source")
                    .absolute()
                    .left(px(10.))
                    .top(px(20.))
                    .size(px(60.))
                    .on_drag((), |_, _, _, cx| cx.new(|_| gpui::Empty)),
            )
            .child(
                div()
                    .id("target")
                    .debug_selector(|| "drop-target".into())
                    .absolute()
                    .left(px(100.))
                    .top(px(20.))
                    .size(px(60.))
                    .bg(rgb(0x111111))
                    .hover(|s| s.bg(rgb(0xff0000)))
                    .drag_over::<()>(|style, _, _, _| style.bg(rgb(0x00ff00)))
                    .on_drop(move |_: &(), _, _| drops.set(drops.get() + 1)),
            )
    }
}

#[gpui::test]
fn pointer_hover_suppression_preserves_typed_drop_feedback_and_delivery(cx: &mut TestAppContext) {
    let drops = std::rc::Rc::new(std::cell::Cell::new(0));
    let handle = cx.add_window(|_, _| DropFixture {
        drops: drops.clone(),
    });
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.simulate_resize(size(px(320.), px(120.)));
    visual.simulate_mouse_down(
        point(px(20.), px(40.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(
        point(px(30.), px(40.)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    visual.simulate_mouse_move(
        point(px(120.), px(40.)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_background("drop-target"),
        Some(rgb(0x00ff00).into())
    );
    visual.simulate_mouse_up(
        point(px(120.), px(40.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert_eq!(drops.get(), 1);
    assert_eq!(
        visual.rendered_background("drop-target"),
        Some(rgb(0xff0000).into())
    );
}

struct TooltipFixture;
impl Render for TooltipFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("tooltip")
            .debug_selector(|| "drag-tooltip".into())
            .size(px(20.))
            .bg(rgb(0xff00ff))
    }
}

#[gpui::test]
fn a_visible_hoverable_tooltip_is_hidden_on_drag_without_waiting_for_its_hide_delay(
    cx: &mut TestAppContext,
) {
    let handle = cx.add_window(|_, _| Fixture::default());
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.simulate_resize(size(px(320.), px(120.)));
    visual.simulate_mouse_move(point(px(120.), px(40.)), None, Modifiers::default());
    visual.run_until_parked();
    visual
        .executor()
        .advance_clock(std::time::Duration::from_secs(1));
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("drag-tooltip").is_some(),
        "ordinary hover must show the tooltip first"
    );
    visual.simulate_mouse_down(
        point(px(120.), px(40.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    // Remain inside the already visible hoverable tooltip. Its normal policy
    // deliberately keeps it open here, even when its source loses hover.
    visual.simulate_mouse_move(
        point(px(132.), px(50.)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("drag-tooltip").is_none(),
        "drag suppression must bypass hoverable retention and hide delay"
    );
}

struct CachedContent(std::rc::Rc<std::cell::Cell<usize>>);
impl Render for CachedContent {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.0.set(self.0.get() + 1);
        div().size_full().bg(rgb(0x111111))
    }
}
struct CacheRoot(gpui::Entity<CachedContent>);
impl Render for CacheRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut layout = div().w(px(60.)).h(px(60.));
        div()
            .size_full()
            .child(gpui::AnyView::from(self.0.clone()).cached(layout.style().clone()))
    }
}

#[gpui::test]
fn ordinary_pointer_click_preserves_unrelated_cached_content(cx: &mut TestAppContext) {
    use gpui::AppContext;
    let renders = std::rc::Rc::new(std::cell::Cell::new(0));
    let content = cx.new(|_| CachedContent(renders.clone()));
    let handle = cx.add_window(|_, _| CacheRoot(content));
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.simulate_resize(size(px(320.), px(120.)));
    visual.run_until_parked();
    let before = renders.get();
    assert!(before > 0);
    visual.simulate_click(point(px(200.), px(40.)), Modifiers::default());
    // Exercise a completed parent repaint, rather than merely checking that
    // a click produced no scene. The unrelated cached child should replay.
    visual.update(|window, cx| {
        let root = window.root::<CacheRoot>().unwrap().unwrap();
        root.update(cx, |_, cx| cx.notify());
    });
    visual.run_until_parked();
    assert_eq!(renders.get(), before);
}

struct CachedHoverRoot(gpui::Entity<Fixture>);
impl Render for CachedHoverRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut layout = div().size_full();
        div()
            .size_full()
            .child(gpui::AnyView::from(self.0.clone()).cached(layout.style().clone()))
    }
}

struct GroupContent;
impl Render for GroupContent {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("group-target")
            .debug_selector(|| "cached-group-target".into())
            .size_full()
            .bg(rgb(0x111111))
            .group_hover("hover-parent", |style| style.bg(rgb(0xff0000)))
    }
}
struct GroupRoot(gpui::Entity<GroupContent>);
impl Render for GroupRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut layout = div().w(px(60.)).h(px(60.));
        div()
            .size_full()
            .child(
                canvas(
                    |_, _, _| (),
                    |_, _, window, _| {
                        window.on_mouse_event(|event: &MouseMoveEvent, phase, _, cx| {
                            if phase.capture() && event.pressed_button.is_some() {
                                cx.stop_propagation();
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .id("hover-parent")
                    .group("hover-parent")
                    .absolute()
                    .left(px(10.))
                    .top(px(20.))
                    .w(px(200.))
                    .h(px(60.))
                    .child(gpui::AnyView::from(self.0.clone()).cached(layout.style().clone())),
            )
    }
}

#[gpui::test]
fn cached_group_hover_recovers_after_parent_only_repaint_during_drag(cx: &mut TestAppContext) {
    use gpui::AppContext;
    let content = cx.new(|_| GroupContent);
    let handle = cx.add_window(|_, _| GroupRoot(content));
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.simulate_resize(size(px(320.), px(120.)));
    let at = point(px(180.), px(40.)); // In the group, outside its cached child.
    visual.simulate_mouse_move(at, None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_background("cached-group-target"),
        Some(rgb(0xff0000).into())
    );
    visual.simulate_mouse_down(
        point(px(300.), px(40.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(at, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_background("cached-group-target"),
        Some(rgb(0x111111).into())
    );
    visual.update(|window, cx| {
        let root = window.root::<GroupRoot>().unwrap().unwrap();
        root.update(cx, |_, cx| cx.notify());
    });
    visual.run_until_parked();
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_background("cached-group-target"),
        Some(rgb(0xff0000).into())
    );
}

#[path = "pointer_hover_profile.rs"]
mod profile;

#[gpui::test]
fn deactivation_clears_drag_without_synthesizing_hover_entry(cx: &mut TestAppContext) {
    let hovered = std::rc::Rc::new(std::cell::Cell::new(false));
    let handle = cx.add_window(|_, _| Fixture {
        hovered: hovered.clone(),
        ..Default::default()
    });
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.simulate_resize(size(px(320.), px(120.)));
    visual.update(|window, _| window.activate_window());
    visual.simulate_mouse_down(
        point(px(20.), px(40.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(
        point(px(120.), px(40.)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(!hovered.get());
    visual.deactivate_window();
    visual.run_until_parked();
    assert!(
        !hovered.get(),
        "focus loss must not trigger hover-driven file preparation"
    );
    visual.update(|window, _| window.activate_window());
    visual.simulate_mouse_move(point(px(120.), px(40.)), None, Modifiers::default());
    visual.run_until_parked();
    assert!(hovered.get());
}
