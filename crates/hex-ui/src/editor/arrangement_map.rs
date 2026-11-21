//! Cached, view-only clip intervals for the Main lane's arrangement miniature.
//! Project owns time/identity; section, range and automation owners retain input.
use super::{EditorView, viewport::Viewport};
use crate::{section_surface::SectionCoverage, theme};
use gpui::{
    Bounds, Div, Pixels, Rgba, ScaledPixels, Window, canvas, div, fill, point, prelude::*, px, rgb,
    size,
};

mod model;
use model::cached;
pub(super) use model::{
    Interval, MAX_ROWS, MapCache, append_track_intervals, merge, project_interval,
};

pub(super) fn view(
    view: &EditorView,
    viewport: Viewport,
    coverage: Option<SectionCoverage>,
) -> Div {
    layer(view, viewport, None, coverage)
}

pub(super) fn section(view: &EditorView, start: u64, end: u64, chamfer: f32) -> Div {
    layer(
        view,
        Viewport {
            start,
            length: end.saturating_sub(start),
        },
        Some(chamfer),
        None,
    )
}

const ROW_BATCH_CAPACITY: usize = 16;

/// Nearby spans of one row share an order. Each original quad and its
/// content mask still paint independently.
fn paint_row(
    window: &mut Window,
    color: Rgba,
    top: Pixels,
    height: Pixels,
    spans: impl Iterator<Item = (Pixels, Pixels)>,
    covers: impl Fn(Bounds<ScaledPixels>, Bounds<ScaledPixels>) -> bool,
) {
    let scale = window.scale_factor();
    let mask = window.content_mask().bounds;
    let scaled_mask = mask.scale(scale);
    let max_gap = ScaledPixels::from(2.0);
    let mut batch = [Bounds::default(); ROW_BATCH_CAPACITY];
    let mut count = 0;
    let mut first_clipped = Bounds::<ScaledPixels>::default();
    let mut right = ScaledPixels::default();

    for (left, width) in spans {
        let span = Bounds::new(point(left, top), size(width, height));
        let scaled = span.scale(scale);
        let clipped = scaled.intersect(&scaled_mask);
        if clipped.is_empty() || covers(scaled, scaled_mask) {
            continue;
        }
        let gap = clipped.left() - right;
        if count == batch.len()
            || (count > 0 && !(ScaledPixels::default() <= gap && gap <= max_gap))
        {
            paint_row_batch(
                window,
                color,
                &batch[..count],
                mask,
                scale,
                first_clipped,
                right,
            );
            count = 0;
        }
        if count == 0 {
            first_clipped = clipped;
        }
        batch[count] = span;
        count += 1;
        right = clipped.right();
    }
    paint_row_batch(
        window,
        color,
        &batch[..count],
        mask,
        scale,
        first_clipped,
        right,
    );
}

fn paint_row_batch(
    window: &mut Window,
    color: Rgba,
    batch: &[Bounds<Pixels>],
    mask: Bounds<Pixels>,
    scale: f32,
    first_clipped: Bounds<ScaledPixels>,
    clipped_right: ScaledPixels,
) {
    let paint = |window: &mut Window| {
        for &span in batch {
            window.paint_quad(fill(span, color));
        }
    };
    if let [first, .., last] = batch {
        let layer = first.union(last);
        let clipped_layer = layer.intersect(&mask);
        let reserved = clipped_layer.scale(scale);
        // The layer clips before scaling, while Scene clips the scaled
        // quads. Guard rounded outer edges before sharing one draw order.
        if !clipped_layer.is_empty()
            && !reserved.is_empty()
            && first_clipped.left() < clipped_right
            && first_clipped.top() < first_clipped.bottom()
            && reserved.left() <= first_clipped.left()
            && reserved.right() >= clipped_right
            && reserved.top() <= first_clipped.top()
            && reserved.bottom() >= first_clipped.bottom()
            && [
                reserved.left(),
                reserved.right(),
                reserved.top(),
                reserved.bottom(),
            ]
            .into_iter()
            .all(|edge| f64::from(edge).is_finite())
        {
            window.paint_layer(layer, paint);
            return;
        }
    }
    paint(window);
}

fn layer(
    view: &EditorView,
    viewport: Viewport,
    chamfer: Option<f32>,
    coverage: Option<SectionCoverage>,
) -> Div {
    let data = view.state.as_ref().map(|state| {
        cached(
            &view.arrangement_map_cache,
            view.project_generation,
            state.revision,
            &state.project,
            view.arrangement_focus_ranges(),
            view.arrangement_revealed_track(),
        )
    });
    div()
        .debug_selector(move || {
            if chamfer.is_some() {
                "section-arrangement-map".into()
            } else {
                "main-arrangement-map".into()
            }
        })
        .absolute()
        .inset_0()
        .children(data.map(|data| {
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let Some(limit) = viewport.end() else {
                        return;
                    };
                    if data.rows.is_empty() || viewport.length == 0 {
                        return;
                    }
                    let width = f32::from(bounds.size.width);
                    let height = f32::from(bounds.size.height);
                    let top = if chamfer.is_some() {
                        super::clip_name::CLIP_NAME_HEIGHT
                    } else {
                        (height * hex_widgets::RANGE_HANDLE_FRACTION)
                            .max(super::clip_name::CLIP_NAME_HEIGHT)
                    }
                    .min(height);
                    let available = height - top;
                    if available <= 0.0 {
                        return;
                    }
                    let row_height = available / data.rows.len() as f32;
                    let pixel = 1.0 / window.scale_factor();
                    let coverage = coverage
                        .as_ref()
                        .and_then(|coverage| coverage.for_paint(window));
                    let covers = |mark, mask| {
                        coverage
                            .as_ref()
                            .is_some_and(|coverage| coverage.covers(mark, mask))
                    };
                    let color = rgb(theme::color(if chamfer.is_some() {
                        theme::PaletteRole::Background
                    } else {
                        theme::PaletteRole::Raised
                    }));
                    for (index, row) in data.rows.iter().enumerate() {
                        let first = row.partition_point(|span| span.end <= viewport.start);
                        let y = top + index as f32 * row_height;
                        let h = (row_height - pixel).max(pixel).min(height - y);
                        let edge = chamfer.map_or(width, |cut| {
                            crate::section_surface::geometry::miniature_right(
                                width,
                                height,
                                cut,
                                y + h,
                            )
                        });
                        let spans = row[first..]
                            .iter()
                            .take_while(|span| span.start < limit)
                            .filter_map(|&span| {
                                let (left, right) = project_interval(span, viewport, width)?;
                                let w = (right - left - pixel)
                                    .max(pixel)
                                    .min(right - left)
                                    .min(edge - left);
                                if w <= 0.0 {
                                    return None;
                                }
                                Some((bounds.left() + px(left), px(w)))
                            });
                        paint_row(window, color, bounds.top() + px(y), px(h), spans, covers);
                    }
                },
            )
            .size_full()
        }))
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::model::tests::project;
    use super::*;
    use hex_project::{BeatTime, Command, Editor};

    struct SectionCoverageOracle {
        left: f32,
        opacity: f32,
    }

    const ORACLE_INK: u32 = 0x6741d9;

    impl gpui::Render for SectionCoverageOracle {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            let coverage = SectionCoverage::default();
            let underlay = coverage.clone();
            let body = crate::section_surface::surface(
                (self.left, 0.5),
                48.0,
                crate::section_surface::CHAMFER,
                hex_project::ElementColor::Orange,
                false,
                false,
                Some(coverage),
            );
            div()
                .debug_selector(|| "main-miniature-coverage-oracle".into())
                .relative()
                .w(px(96.0))
                .h(px(36.0))
                .opacity(self.opacity)
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, _| {
                            let read = underlay.for_paint(window);
                            let covers = |mark, mask| {
                                read.as_ref().is_some_and(|read| read.covers(mark, mask))
                            };
                            for (y, marks) in [(24.0, &[16.0, 55.0][..]), (33.0, &[45.0][..])] {
                                paint_row(
                                    window,
                                    rgb(ORACLE_INK),
                                    bounds.top() + px(y),
                                    px(1.0),
                                    marks.iter().map(|&x| (bounds.left() + px(x), px(1.0))),
                                    covers,
                                );
                            }
                        },
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(body)
        }
    }

    #[gpui::test]
    fn main_miniature_only_omits_current_opaque_section_coverage(cx: &mut gpui::TestAppContext) {
        let handle = cx.add_window(|_, _| SectionCoverageOracle {
            left: 0.0,
            opacity: 1.0,
        });
        let root = handle.root(cx).unwrap();
        let mut visual = gpui::VisualTestContext::from_window(*handle, cx);
        let submitted_marks = |visual: &mut gpui::VisualTestContext, opacity| {
            visual.run_until_parked();
            let scale = visual.update(|window, _| window.scale_factor());
            let bounds = visual
                .debug_bounds("main-miniature-coverage-oracle")
                .unwrap();
            let ink = gpui::Background::from(rgb(ORACLE_INK)).opacity(opacity);
            let mut xs = visual
                .rendered_filled_quads("main-miniature-coverage-oracle")
                .into_iter()
                .filter(|(_, color)| *color == ink)
                .map(|(mark, _)| mark.left() - bounds.left().scale(scale))
                .collect::<Vec<_>>();
            xs.sort_unstable();
            xs.into_iter()
                .map(|x| x / ScaledPixels::from(scale))
                .collect::<Vec<_>>()
        };
        // Covered mark at 16 drops. The cutout at 45 and gap at 55 remain.
        assert_eq!(submitted_marks(&mut visual, 1.0), [45.0, 55.0]);
        root.update(&mut visual, |view, cx| {
            view.left = 0.5;
            cx.notify();
        });
        assert_eq!(submitted_marks(&mut visual, 1.0), [16.0, 45.0]);
        root.update(&mut visual, |view, cx| {
            view.opacity = 0.5;
            cx.notify();
        });
        assert_eq!(submitted_marks(&mut visual, 0.5), [16.0, 45.0, 55.0]);
    }

    struct MiniaturePaintOracle {
        batch: bool,
    }

    impl gpui::Render for MiniaturePaintOracle {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            let batch = self.batch;
            div()
                .debug_selector(|| "miniature-paint-oracle".into())
                .size_full()
                .child(
                    canvas(
                        |_, _, _| (),
                        move |_, _, window, _| {
                            let rect =
                                |x, y, w, h| Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
                            let mask = gpui::ContentMask {
                                bounds: rect(10.125, 10.25, 32.125, 8.25),
                            };
                            window.paint_quad(fill(rect(9.0, 9.0, 37.0, 11.0), rgb(0x505050)));
                            window.with_content_mask(Some(mask), |window| {
                                // Keep fractional/empty spans and overlap between short rows
                                // when the projected vertical stride is below one device pixel.
                                let clipped = [
                                    (8.0, 1.0),
                                    (10.0, 3.25),
                                    (13.75, 2.0),
                                    (15.625, 3.125),
                                    (38.625, 5.75),
                                ];
                                let long: [(f32, f32); ROW_BATCH_CAPACITY + 3] =
                                    std::array::from_fn(|i| (10.5 + i as f32 * 1.5, 1.0));
                                for (y, color, spans) in [
                                    (10.125, 0x5050aa, clipped.as_slice()),
                                    (10.4375, 0xaa5050, clipped.as_slice()),
                                    (14.5, 0xaaaa50, long.as_slice()),
                                ] {
                                    if batch {
                                        paint_row(
                                            window,
                                            rgb(color),
                                            px(y),
                                            px(0.5),
                                            spans
                                                .iter()
                                                .copied()
                                                .map(|(left, width)| (px(left), px(width))),
                                            |_, _| false,
                                        );
                                    } else {
                                        for &(left, width) in spans {
                                            window.paint_quad(fill(
                                                rect(left, y, width, 0.5),
                                                rgb(color),
                                            ));
                                        }
                                    }
                                }
                            });
                            window.paint_quad(fill(rect(13.0, 10.375, 4.0, 0.5), rgb(0x50aa50)));
                        },
                    )
                    .size_full(),
                )
        }
    }

    #[gpui::test]
    fn miniature_batches_preserve_clipped_quads_and_overlap_order(cx: &mut gpui::TestAppContext) {
        let mut scenes = Vec::new();
        for batch in [false, true] {
            let handle = cx.add_window(move |_, _| MiniaturePaintOracle { batch });
            let mut visual = gpui::VisualTestContext::from_window(*handle, cx);
            visual.run_until_parked();
            let scale = visual.update(|window, _| window.scale_factor());
            let quads = visual.rendered_filled_quads("miniature-paint-oracle");
            assert_eq!(
                quads.len(),
                10 + ROW_BATCH_CAPACITY + 3,
                "background, four spans per short row, a run over capacity and overlay"
            );
            let samples = [
                (10.0625, 10.5), // Left of the content mask.
                (10.5, 10.5),    // Overlapping miniature rows.
                (13.5, 10.5),    // Gap between spans with the later overlay.
                (14.25, 10.5),   // Overlay above both rows.
                (15.6875, 10.5), // Overlapping spans within both rows.
                (22.0, 10.5),    // Main gap outside the miniature.
                (42.5, 10.5),    // Right of the content mask.
                (42.0, 10.75),   // Near the mask edge in the later row only.
            ];
            let stacks = samples.map(|(x, y)| {
                let sample = point(px(x), px(y)).scale(scale);
                quads
                    .iter()
                    .filter(|(bounds, _)| bounds.contains(&sample))
                    .map(|(_, color)| *color)
                    .collect::<Vec<_>>()
            });
            let mut geometry = quads;
            geometry.sort_by_key(|(bounds, _)| (bounds.top(), bounds.left()));
            scenes.push((geometry, stacks));
        }
        assert_eq!(scenes[0], scenes[1]);
        assert_eq!(
            scenes[0].1.each_ref().map(Vec::len),
            [1, 3, 2, 4, 6, 1, 1, 2]
        );
    }

    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn main_lane_paints_canonical_intervals_at_the_ruler_scale(cx: &mut gpui::TestAppContext) {
        use gpui::{VisualTestContext, point, px, size};
        use std::sync::Arc;
        for has_section in [false, true] {
            let mut model = Editor::new(project(), 8).unwrap();
            if has_section {
                model
                    .execute(Command::AddArrangementSection {
                        name: "section".into(),
                        color: hex_project::ElementColor::Orange,
                        start: BeatTime::ZERO,
                        end: BeatTime::from_quarters(4).unwrap(),
                    })
                    .unwrap();
            }
            let original = model.project().snapshot();
            let (handle, editor) = super::super::clip_drag_ui_test::editor_window(
                cx,
                model.project().clone(),
                Arc::default(),
            );
            let mut visual = VisualTestContext::from_window(handle, cx);
            for dimensions in [(880.0, 540.0), (1440.0, 800.0)] {
                visual.simulate_resize(size(px(dimensions.0), px(dimensions.1)));
                for start in [0, 72_000] {
                    editor.update(&mut visual, |view, cx| {
                        view.viewport = Some(Viewport {
                            start,
                            length: 192_000,
                        });
                        cx.notify();
                    });
                    visual.run_until_parked();
                    let lane = visual.debug_bounds("main-track-lane").unwrap();
                    assert_eq!(visual.debug_bounds("main-arrangement-map"), Some(lane));
                    let ruler = visual.debug_bounds("timeline-ruler").unwrap();
                    assert_eq!(lane.left(), ruler.left());
                    assert_eq!(lane.size.width, ruler.size.width);
                    let (viewport, revision) = editor.read_with(&visual, |view, _| {
                        let state = view.state.as_ref().unwrap();
                        (view.visible_viewport(state.timeline_frames), state.revision)
                    });
                    let scale = visual.update(|window, _| window.scale_factor());
                    let color: gpui::Background = rgb(theme::color(if has_section {
                        theme::PaletteRole::Background
                    } else {
                        theme::PaletteRole::Raised
                    }))
                    .into();
                    let quads = visual.rendered_filled_quads("main-arrangement-map");
                    if start == 0 {
                        // The authored audio span is wholly inside the
                        // section when present. The Main mark under the
                        // section must be absent from the completed scene.
                        let main_top = (f32::from(lane.size.height)
                            * hex_widgets::RANGE_HANDLE_FRACTION)
                            .max(super::super::clip_name::CLIP_NAME_HEIGHT);
                        let probe = point(
                            lane.left() + lane.size.width * (72_000.0 / viewport.length as f32),
                            lane.top() + px(main_top + 2.0),
                        )
                        .scale(scale);
                        let main_color =
                            gpui::Background::from(rgb(theme::color(theme::PaletteRole::Raised)));
                        assert_eq!(
                            quads.iter().any(|(rect, color)| {
                                *color == main_color && rect.contains(&probe)
                            }),
                            !has_section,
                        );
                    }
                    let top = if has_section {
                        super::super::clip_name::CLIP_NAME_HEIGHT
                    } else {
                        (f32::from(lane.size.height) / 3.0)
                            .max(super::super::clip_name::CLIP_NAME_HEIGHT)
                    };
                    for (row, (first, last)) in [(48_000_u64, 96_000_u64), (24_000, 144_000)]
                        .into_iter()
                        .enumerate()
                    {
                        let left = first.max(viewport.start);
                        let right = last.min(viewport.end().unwrap());
                        if left >= right {
                            continue;
                        }
                        let x = f32::from(lane.left())
                            + (left - viewport.start) as f32 / viewport.length as f32
                                * f32::from(lane.size.width);
                        let y = f32::from(lane.top())
                            + top
                            + row as f32 * (f32::from(lane.size.height) - top) / 2.0;
                        assert!(
                            quads.iter().any(|(bounds, background)| {
                                *background == color
                                    // Section-local projections can differ by a fraction
                                    // of one physical pixel after layout rounding.
                                    && (f64::from(bounds.left()) - f64::from(x * scale)).abs() <= 0.5
                                    && (f64::from(bounds.top()) - f64::from(y * scale)).abs() <= 0.5
                                    && bounds.size.height > gpui::ScaledPixels::default()
                            }),
                            "missing row {row} at {x}, {y}: {quads:?}"
                        );
                    }
                    // The canvas owns no input: the existing Main-lane selection
                    // gesture still receives a click through the miniature.
                    let click = point(lane.center().x, lane.bottom() - px(3.0));
                    visual.simulate_mouse_down(
                        click,
                        gpui::MouseButton::Left,
                        gpui::Modifiers::default(),
                    );
                    visual.simulate_mouse_up(
                        click,
                        gpui::MouseButton::Left,
                        gpui::Modifiers::default(),
                    );
                    visual.run_until_parked();
                    editor.read_with(&visual, |view, _| {
                        let state = view.state.as_ref().unwrap();
                        assert_eq!(state.project.snapshot(), original);
                        assert_eq!(state.revision, revision);
                    });
                }
            }
        }
    }
}
