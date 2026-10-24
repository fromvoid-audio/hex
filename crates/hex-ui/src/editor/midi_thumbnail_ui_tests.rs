//! Completed scene parity with the former one-quad-at-a-time submission.
use super::*;
use gpui::{
    Context, IntoElement, Render, TestAppContext, VisualTestContext, Window, canvas, div, point,
    prelude::*, px, rgba,
};

struct Thumbnail {
    batched: bool,
    width: f32,
    height: f32,
    shapes: Vec<(f32, f32, u8)>,
}
impl Render for Thumbnail {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let batched = self.batched;
        let shapes = self.shapes.clone();
        div().size_full().relative().child(
            div()
                .debug_selector(|| "thumbnail-batch-oracle".into())
                .absolute()
                .left(px(13.25))
                .top(px(21.75))
                .w(px(self.width))
                .h(px(self.height))
                .overflow_hidden()
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, _| {
                            // External paint before and after notes must retain
                            // its per-overlap-cell alpha compositing sequence.
                            window.paint_quad(gpui::fill(bounds, rgba(0x3366ff80)));
                            if batched {
                                paint_note_shapes(bounds, &shapes, window);
                            } else {
                                for &(x, width, pitch) in &shapes {
                                    if let Some(note) =
                                        paint_bounds(bounds, x, width, pitch, window.scale_factor())
                                    {
                                        window.paint_quad(gpui::fill(note, gpui::rgb(0x000000)));
                                    }
                                }
                            }
                            let mut overlay = bounds;
                            overlay.size.width /= 2.0;
                            window.paint_quad(gpui::fill(overlay, rgba(0xff663380)));
                        },
                    )
                    .absolute()
                    .left(px(-3.25))
                    .top(px(-1.75))
                    .w(px(self.width + 6.5))
                    .h(px(self.height + 3.5)),
                ),
        )
    }
}

type Quad = (gpui::Bounds<gpui::ScaledPixels>, gpui::Background);

fn assert_same_scene(legacy: &[Quad], batched: &[Quad]) {
    // Preserve exact clipped payloads and multiplicity, allowing disjoint
    // primitives to reorder. Then compare the actual finished painter sequence
    // in every elementary overlap cell, including translucent external paint.
    let mut remaining = batched.to_vec();
    for quad in legacy {
        let index = remaining
            .iter()
            .position(|item| item == quad)
            .expect("same clipped quad");
        remaining.remove(index);
    }
    assert!(remaining.is_empty());
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for (bounds, _) in legacy.iter().chain(batched) {
        xs.extend([f64::from(bounds.left()), f64::from(bounds.right())]);
        ys.extend([f64::from(bounds.top()), f64::from(bounds.bottom())]);
    }
    xs.sort_by(f64::total_cmp);
    ys.sort_by(f64::total_cmp);
    xs.dedup();
    ys.dedup();
    for x in xs.windows(2) {
        for y in ys.windows(2) {
            let sample = point(
                gpui::ScaledPixels::from(((x[0] + x[1]) * 0.5) as f32),
                gpui::ScaledPixels::from(((y[0] + y[1]) * 0.5) as f32),
            );
            let sequence = |quads: &[Quad]| {
                quads
                    .iter()
                    .filter(|(bounds, _)| bounds.contains(&sample))
                    .map(|(_, color)| color.clone())
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                sequence(legacy),
                sequence(batched),
                "painter order at {sample:?}"
            );
        }
    }
}

#[gpui::test]
fn batched_notes_preserve_completed_clipping_payloads_and_alpha_order(cx: &mut TestAppContext) {
    let mut shapes: Vec<_> = (0..65)
        .map(|index| (index as f32 / 65.0, 0.4 / 65.0, (index * 2 % 128) as u8))
        .collect();
    shapes.extend([
        (0.0, 0.2, 0),
        (0.0, 0.2, 127),
        (0.05, 0.2, 60),
        (0.05, 0.2, 60),
        (0.95, 0.05, 64),
        (0.5, 0.25, 65),
        (f32::NAN, 0.2, 60),
    ]);
    let handle = cx.add_window(|_, _| Thumbnail {
        batched: false,
        width: 127.5,
        height: 50.0,
        shapes,
    });
    let root = handle.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*handle, cx);
    for (width, height) in [(127.5, 50.0), (4.0, 7.0), (280.0, 250.0)] {
        visual.update(|window, cx| {
            root.update(cx, |root, cx| {
                root.batched = false;
                root.width = width;
                root.height = height;
                cx.notify();
            });
            window.draw(cx).clear();
        });
        let legacy = visual.rendered_filled_quads("thumbnail-batch-oracle");
        visual.update(|window, cx| {
            root.update(cx, |root, cx| {
                root.batched = true;
                cx.notify();
            });
            window.draw(cx).clear();
        });
        let batched = visual.rendered_filled_quads("thumbnail-batch-oracle");
        assert_same_scene(&legacy, &batched);
    }
    visual.update(|window, _| window.remove_window());
}
