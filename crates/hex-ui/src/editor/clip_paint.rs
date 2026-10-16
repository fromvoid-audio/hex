//! Passive clip decoration painted without a layout node per grid line or note.
use super::musical_time::GridLine;
use crate::theme;
use gpui::{Bounds, Canvas, PaintQuad, Pixels, canvas, fill, point, prelude::*, px, rgb, size};

pub(super) type NoteShape = (f32, f32, u8);
#[derive(Clone, Copy)]
struct Stripe {
    fraction: f32,
    marker: bool,
    bar_start: bool,
}

fn stripes(lines: &[GridLine], start: u64, end: u64) -> Vec<Stripe> {
    if start >= end {
        return Vec::new();
    }
    lines
        .iter()
        .filter_map(|line| {
            let offset = line.frame.checked_sub(start)?;
            (offset > 0 && offset < end - start).then_some(Stripe {
                fraction: (offset as f64 / (end - start) as f64) as f32,
                marker: line.meter_marker,
                bar_start: line.bar_start,
            })
        })
        .collect()
}

fn paint_grid(
    stripes: &[Stripe],
    bounds: Bounds<Pixels>,
    scale: f32,
    mut paint: impl FnMut(PaintQuad),
) {
    if !scale.is_finite() || scale <= 0.0 {
        return;
    }
    for stripe in stripes {
        let color = theme::color(if stripe.marker {
            theme::PaletteRole::Primary
        } else {
            theme::PaletteRole::Grid
        });
        let opacity = if stripe.marker || stripe.bar_start {
            0.42
        } else {
            0.24
        };
        paint(fill(
            layout_aligned(
                Bounds::new(
                    point(
                        bounds.origin.x + bounds.size.width * stripe.fraction,
                        bounds.origin.y,
                    ),
                    size(
                        px(if stripe.marker { 1.5 } else { 1.0 }),
                        bounds.size.height,
                    ),
                ),
                scale,
            ),
            gpui::Hsla::from(rgb(color)).opacity(opacity),
        ));
    }
}

pub(super) fn grid_layer(lines: &[GridLine], start: u64, end: u64) -> Option<Canvas<()>> {
    let stripes = stripes(lines, start, end);
    if stripes.is_empty() {
        return None;
    }
    Some(
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                paint_grid(&stripes, bounds, window.scale_factor(), |quad| {
                    window.paint_quad(quad)
                });
            },
        )
        .absolute()
        .inset_0(),
    )
}

// Primary decision, 16 October 2026: align passive quads from the actual
// rounded canvas bounds. This can realign an edge by at most one physical pixel
// versus Taffy's private unrounded intermediate widths; hitboxes stay unchanged.
fn layout_aligned(bounds: Bounds<Pixels>, scale: f32) -> Bounds<Pixels> {
    let align = |value: Pixels| px((f32::from(value) * scale + 0.5).floor() / scale);
    let origin = point(align(bounds.origin.x), align(bounds.origin.y));
    let end = point(align(bounds.right()), align(bounds.bottom()));
    Bounds::new(origin, size(end.x - origin.x, end.y - origin.y))
}

fn paint_notes(
    shapes: &[NoteShape],
    bounds: Bounds<Pixels>,
    scale: f32,
    mut paint: impl FnMut(PaintQuad),
) {
    if !scale.is_finite() || scale <= 0.0 {
        return;
    }
    let color = rgb(theme::color(theme::PaletteRole::Secondary));
    for &(x, width, pitch) in shapes {
        let top = f32::from(127_u8.saturating_sub(pitch)) / 128.0;
        paint(fill(
            layout_aligned(
                Bounds::new(
                    point(
                        bounds.origin.x + bounds.size.width * x,
                        bounds.origin.y + bounds.size.height * top,
                    ),
                    size(bounds.size.width * width, px(2.0)),
                ),
                scale,
            ),
            color,
        ));
    }
}

pub(super) fn note_layer(shapes: Vec<NoteShape>) -> Option<Canvas<()>> {
    if shapes.is_empty() {
        return None;
    }
    Some(
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                paint_notes(&shapes, bounds, window.scale_factor(), |quad| {
                    window.paint_quad(quad)
                });
            },
        )
        .absolute()
        .inset_0(),
    )
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::{Context, IntoElement, Render, TestAppContext, VisualTestContext, Window, div};
    use std::{cell::RefCell, rc::Rc};

    struct PaintOracle {
        extent: (f32, f32, f32),
        grid: Rc<RefCell<Vec<PaintQuad>>>,
        notes: Rc<RefCell<Vec<PaintQuad>>>,
    }
    const NOTES: [NoteShape; 5] = [
        (0.0, 0.005, 127),
        (0.25, 0.2, 60),
        (0.75, 0.25, 0),
        (0.213, 0.319, 64),
        (0.619, 0.271, 11),
    ];
    fn grid() -> Vec<Stripe> {
        vec![
            Stripe {
                fraction: 0.25,
                marker: false,
                bar_start: false,
            },
            Stripe {
                fraction: 0.5,
                marker: false,
                bar_start: true,
            },
            Stripe {
                fraction: 0.75,
                marker: true,
                bar_start: true,
            },
        ]
    }
    impl Render for PaintOracle {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let grid_capture = self.grid.clone();
            let note_capture = self.notes.clone();
            // Paint the new batches first. Scene lookup of matching bounds then
            // observes the independently laid-out legacy Div fill and mask.
            let mut legacy_grid = div().absolute().inset_0().child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let mut output = grid_capture.borrow_mut();
                        output.clear();
                        paint_grid(&grid(), bounds, window.scale_factor(), |quad| {
                            output.push(quad.clone());
                            window.paint_quad(quad);
                        });
                    },
                )
                .absolute()
                .inset_0(),
            );
            for (index, stripe) in grid().into_iter().enumerate() {
                let role = if stripe.marker {
                    theme::PaletteRole::Primary
                } else {
                    theme::PaletteRole::Grid
                };
                legacy_grid = legacy_grid.child(
                    div()
                        .debug_selector(move || format!("legacy-grid-{index}"))
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(gpui::relative(stripe.fraction))
                        .w(px(if stripe.marker { 1.5 } else { 1.0 }))
                        .bg(rgb(theme::color(role)))
                        .opacity(if stripe.marker || stripe.bar_start {
                            0.42
                        } else {
                            0.24
                        }),
                );
            }
            let mut clip = div()
                .debug_selector(|| "oracle-clip".to_owned())
                .relative()
                .ml(px(self.extent.2))
                .mt(px(5.25))
                .w(px(self.extent.0))
                .h(px(self.extent.1))
                .border_1()
                .border_color(rgb(0))
                .overflow_hidden()
                .opacity(0.68)
                .children([legacy_grid])
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, _| {
                            let mut output = note_capture.borrow_mut();
                            output.clear();
                            paint_notes(&NOTES, bounds, window.scale_factor(), |quad| {
                                output.push(quad.clone());
                                window.paint_quad(quad);
                            });
                        },
                    )
                    .absolute()
                    .inset_0(),
                );
            for (index, (x, width, pitch)) in NOTES.into_iter().enumerate() {
                clip = clip.child(
                    div()
                        .debug_selector(move || format!("legacy-note-{index}"))
                        .absolute()
                        .left(gpui::relative(x))
                        .top(gpui::relative(f32::from(127 - pitch) / 128.0))
                        .w(gpui::relative(width))
                        .h(px(2.0))
                        .bg(rgb(theme::color(theme::PaletteRole::Secondary))),
                );
            }
            clip
        }
    }
    #[test]
    fn physical_edge_oracle_covers_one_and_two_scale_and_negative_offsets() {
        let input = Bounds::new(point(px(1.25), px(2.25)), size(px(9.3), px(4.25)));
        assert_eq!(
            layout_aligned(input, 1.0),
            Bounds::new(point(px(1.0), px(2.0)), size(px(10.0), px(5.0)))
        );
        assert_eq!(
            layout_aligned(input, 2.0),
            Bounds::new(point(px(1.5), px(2.5)), size(px(9.0), px(4.0)))
        );
        let negative = Bounds::new(point(px(-1.25), px(-2.25)), input.size);
        assert_eq!(
            layout_aligned(negative, 2.0),
            Bounds::new(point(px(-1.0), px(-2.0)), size(px(9.0), px(4.0)))
        );
    }

    #[test]
    fn empty_or_hidden_decorations_have_no_layout_element() {
        let lines = [0, 100, 200].map(|frame| GridLine {
            frame,
            bar_start: false,
            meter_marker: false,
        });
        assert!(grid_layer(&lines, 0, 100).is_none());
        assert!(grid_layer(&lines, 100, 100).is_none());
        assert!(grid_layer(&lines, 0, 200).is_some());
        assert!(note_layer(Vec::new()).is_none());
    }

    #[gpui::test]
    fn batches_keep_legacy_edges_within_one_physical_pixel_and_preserve_colors_and_clip_mask(
        cx: &mut TestAppContext,
    ) {
        let grid = Rc::new(RefCell::new(Vec::new()));
        let notes = Rc::new(RefCell::new(Vec::new()));
        let handle = cx.add_window(|_, _| PaintOracle {
            extent: (13.2, 44.0, 0.0),
            grid: grid.clone(),
            notes: notes.clone(),
        });
        let root = handle.entity(cx).expect("oracle root");
        let mut visual = VisualTestContext::from_window(*handle, cx);
        for extent in [
            (9.3, 44.0, 0.0),
            (13.2, 44.0, 0.0),
            (97.3, 44.0, 2.25),
            (137.6, 88.0, 3.5),
            (743.0, 44.0, -12.0),
        ] {
            root.update(cx, |view, cx| {
                view.extent = extent;
                cx.notify();
            });
            visual.run_until_parked();
            for (prefix, captured) in [("legacy-grid", &grid), ("legacy-note", &notes)] {
                assert_eq!(
                    captured.borrow().len(),
                    if prefix == "legacy-grid" {
                        3
                    } else {
                        NOTES.len()
                    }
                );
                for (index, quad) in captured.borrow().iter().enumerate() {
                    let selector = Box::leak(format!("{prefix}-{index}").into_boxed_str());
                    let legacy = visual.debug_bounds(selector).expect("legacy shape");
                    for (actual, expected) in [
                        (quad.bounds.origin.x, legacy.origin.x),
                        (quad.bounds.origin.y, legacy.origin.y),
                        (quad.bounds.right(), legacy.right()),
                        (quad.bounds.bottom(), legacy.bottom()),
                    ] {
                        assert!(
                            (f32::from(actual - expected)
                                * visual.update(|window, _| window.scale_factor()))
                            .abs()
                                <= 1.0001,
                            "{prefix}-{index}: batched {:?}, legacy {legacy:?}",
                            quad.bounds
                        );
                    }
                    if !legacy.is_empty() {
                        assert_eq!(
                            Some(quad.background.opacity(0.68)),
                            visual.rendered_background(selector)
                        );
                    } else {
                        assert!(visual.rendered_background(selector).is_none());
                    }
                }
            }
            let (_, mask) = visual
                .rendered_surface_geometry("legacy-note-2")
                .expect("painted clipped bottom note");
            let clip = visual.debug_bounds("oracle-clip").expect("outer clip");
            let scale = visual.update(|window, _| window.scale_factor());
            let clip = clip.scale(scale);
            assert!(mask.size.width <= clip.size.width);
            assert!(mask.size.height <= clip.size.height);
        }
    }
}
