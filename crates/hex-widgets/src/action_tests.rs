use super::*;
use gpui::{
    Context, Modifiers, MouseButton, Render, TestAppContext, VisualTestContext, Window, point, size,
};

const STYLE: ActionStyle = ActionStyle {
    background: 0x303030,
    text: 0xe0e0e0,
    hover_background: 0xee9944,
    hover_text: Some(0x202020),
};
const PRESSED: ActionPressStyle = ActionPressStyle {
    background: 0xffbb66,
    text: 0x202020,
};
struct Fixture {
    clicks: [u32; 2],
}
impl Render for Fixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_start()
            .gap(px(4.0))
            .children(
                [("pill", "ext", 40.0), ("circle", "▶", ROUND_ACTION_SIZE)]
                    .into_iter()
                    .enumerate()
                    .map(|(index, (id, label, width))| {
                        round_action_surface(id, STYLE, 0x111111, PRESSED)
                            .debug_selector(move || id.into())
                            .w(px(width))
                            .child(label)
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.clicks[index] += 1;
                                cx.notify();
                            }))
                    }),
            )
            .child(
                pressed_action_surface(action("run".into(), STYLE).id("regular"), PRESSED)
                    .debug_selector(|| "regular".into())
                    .w(px(40.0))
                    .h(px(ROUND_ACTION_SIZE)),
            )
            .child(
                crate::device_action("×".into(), STYLE)
                    .id("device-action")
                    .debug_selector(|| "device-action".into()),
            )
            .child(
                crate::device_segment("a".into(), STYLE)
                    .id("segment")
                    .debug_selector(|| "segment".into()),
            )
    }
}

#[gpui::test]
fn real_action_primitives_are_fully_round_while_segments_keep_readout_corners(
    cx: &mut TestAppContext,
) {
    let handle = cx.add_window(|_, _| Fixture { clicks: [0; 2] });
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.simulate_resize(size(px(200.0), px(80.0)));
    visual.run_until_parked();
    let scale = visual.update(|window, _| window.scale_factor());
    for (selector, height, radius) in [
        ("regular", ROUND_ACTION_SIZE, ROUND_ACTION_SIZE / 2.0),
        (
            "device-action",
            crate::DEVICE_HEADER_ACTION_SIZE,
            crate::DEVICE_HEADER_ACTION_SIZE / 2.0,
        ),
        (
            "segment",
            crate::DEVICE_HEADER_ACTION_SIZE,
            CONTROL_CORNER_RADIUS,
        ),
    ] {
        assert_eq!(
            visual.debug_bounds(selector).unwrap().size.height,
            px(height)
        );
        assert_eq!(
            visual
                .rendered_surface_geometry(selector)
                .unwrap()
                .0
                .top_left,
            px(radius).scale(scale)
        );
    }
}
#[gpui::test]
fn pill_and_circle_execute_once_per_release_without_latching(cx: &mut TestAppContext) {
    let handle = cx.add_window(|_, _| Fixture { clicks: [0; 2] });
    let view = handle.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.simulate_resize(size(px(140.0), px(80.0)));
    visual.simulate_mouse_move(point(px(100.0), px(60.0)), None, Modifiers::default());
    visual.run_until_parked();
    for (index, selector) in ["pill", "circle"].into_iter().enumerate() {
        let bounds = visual.debug_bounds(selector).unwrap();
        assert_eq!(bounds.size.height, px(ROUND_ACTION_SIZE));
        assert_eq!(
            bounds.size.width,
            px(if index == 0 { 40.0 } else { ROUND_ACTION_SIZE })
        );
        let idle = visual.rendered_background(selector);
        let geometry = visual.rendered_surface_geometry(selector).unwrap();
        let scale = visual.update(|window, _| window.scale_factor());
        assert_eq!(
            geometry.0.top_left,
            px(ROUND_ACTION_SIZE / 2.0).scale(scale)
        );
        for count in 1..=2 {
            visual.simulate_mouse_move(bounds.center(), None, Modifiers::default());
            visual.run_until_parked();
            let hover = visual.rendered_background(selector);
            visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            view.read_with(cx, |view, _| assert_eq!(view.clicks[index], count - 1));
            assert_ne!(visual.rendered_background(selector), hover);
            assert_eq!(
                visual.rendered_background(selector),
                Some(rgb(PRESSED.background).into())
            );
            visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(point(px(100.0), px(60.0)), None, Modifiers::default());
            visual.run_until_parked();
            view.read_with(cx, |view, _| assert_eq!(view.clicks[index], count));
            assert_eq!(visual.rendered_background(selector), idle);
            assert_eq!(visual.rendered_surface_geometry(selector), Some(geometry));
        }
    }
}
