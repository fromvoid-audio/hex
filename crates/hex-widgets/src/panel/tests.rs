use super::*;
use gpui::{
    AppContext, Bounds, Context, Empty, Modifiers, MouseButton, Pixels, Render, TestAppContext,
    VisualTestContext, Window, canvas, point, size,
};
use std::{cell::Cell, rc::Rc};

struct Fixture {
    mask: Rc<Cell<Bounds<Pixels>>>,
    clicks: Rc<Cell<usize>>,
}

impl Render for Fixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mask = self.mask.clone();
        let clicks = self.clicks.clone();
        let content = div().relative().size_full().child(
            div()
                .absolute()
                .left(px(-20.0))
                .top(px(-20.0))
                .size(px(2000.0))
                .bg(rgb(0xff0000))
                .on_mouse_down(MouseButton::Left, move |_, _, _| {
                    clicks.set(clicks.get() + 1)
                })
                .child(
                    canvas(
                        |_, _, _| (),
                        move |_, _, window, _| mask.set(window.content_mask().bounds),
                    )
                    .size_full(),
                ),
        );
        panel_frame(
            PanelStyle {
                surface: 0x202020,
                border: 0x363636,
            },
            content,
        )
        .debug_selector(|| "panel".into())
        .size_full()
    }
}

#[gpui::test]
fn rounded_panel_contains_oversized_paint_and_hits_at_both_sizes(cx: &mut TestAppContext) {
    let mask = Rc::new(Cell::new(Bounds::default()));
    let clicks = Rc::new(Cell::new(0));
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    cx.update_window(handle, |_, window, cx| {
        window.replace_root(cx, |_, _| Fixture {
            mask: mask.clone(),
            clicks: clicks.clone(),
        });
    })
    .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    for (width, height) in [(420.0, 160.0), (36.0, 68.0)] {
        visual.simulate_resize(size(px(width), px(height)));
        visual.run_until_parked();
        let panel = visual.debug_bounds("panel").unwrap();
        let scale = visual.update(|window, _| window.scale_factor());
        let (corners, _) = visual.rendered_surface_geometry("panel").unwrap();
        for radius in [
            corners.top_left,
            corners.top_right,
            corners.bottom_left,
            corners.bottom_right,
        ] {
            assert_eq!(radius, px(4.0).scale(scale));
        }
        assert_eq!(
            visual.rendered_border("panel").unwrap().0,
            rgb(0x363636).into()
        );
        let inner = mask.get();
        assert_eq!(inner.origin, panel.origin + point(px(4.0), px(4.0)));
        assert_eq!(inner.size, panel.size - size(px(8.0), px(8.0)));
        let before = clicks.get();
        for at in [
            panel.origin + point(px(1.0), px(1.0)),
            panel.bottom_right() - point(px(1.0), px(1.0)),
        ] {
            visual.simulate_click(at, Modifiers::default());
            assert_eq!(
                clicks.get(),
                before,
                "surround excludes descendant hitboxes"
            );
        }
        visual.simulate_click(panel.center(), Modifiers::default());
        assert_eq!(clicks.get(), before + 1);
    }
}
