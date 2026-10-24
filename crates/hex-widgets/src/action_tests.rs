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
                [("pill", "ext", 40.0), ("circle", "▶", 20.0)]
                    .into_iter()
                    .enumerate()
                    .map(|(index, (id, label, width))| {
                        round_action_surface(id, STYLE, 0x111111)
                            .debug_selector(move || id.into())
                            .w(px(width))
                            .h(px(20.0))
                            .child(label)
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.clicks[index] += 1;
                                cx.notify();
                            }))
                    }),
            )
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
        assert_eq!(bounds.size.height, px(20.0));
        assert_eq!(bounds.size.width, px(if index == 0 { 40.0 } else { 20.0 }));
        let idle = visual.rendered_background(selector);
        let geometry = visual.rendered_surface_geometry(selector).unwrap();
        let scale = visual.update(|window, _| window.scale_factor());
        assert_eq!(geometry.0.top_left, px(10.0).scale(scale));
        for count in 1..=2 {
            visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            view.read_with(cx, |view, _| assert_eq!(view.clicks[index], count - 1));
            assert_ne!(visual.rendered_background(selector), idle);
            visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(point(px(100.0), px(60.0)), None, Modifiers::default());
            visual.run_until_parked();
            view.read_with(cx, |view, _| assert_eq!(view.clicks[index], count));
            assert_eq!(visual.rendered_background(selector), idle);
            assert_eq!(visual.rendered_surface_geometry(selector), Some(geometry));
        }
    }
}
