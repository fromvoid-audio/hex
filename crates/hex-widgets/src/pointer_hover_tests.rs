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
        div()
            .size_full()
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
                    .tooltip(move |_, cx| {
                        tooltip_builds.set(tooltip_builds.get() + 1);
                        cx.new(|_| gpui::Empty).into()
                    }),
            )
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
    }
}

#[gpui::test]
fn consumed_drag_move_clears_hover_across_completed_repaints(cx: &mut TestAppContext) {
    let hovered = std::rc::Rc::new(std::cell::Cell::new(false));
    let tooltip_builds = std::rc::Rc::new(std::cell::Cell::new(0));
    let handle = cx.add_window(|_, _| Fixture {
        hovered: hovered.clone(),
        tooltip_builds: tooltip_builds.clone(),
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
    visual
        .executor()
        .advance_clock(std::time::Duration::from_secs(1));
    visual.run_until_parked();
    assert_eq!(
        tooltip_builds.get(),
        0,
        "held drag must not build a pending tooltip"
    );
    visual.update(|window, _| window.refresh());
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
