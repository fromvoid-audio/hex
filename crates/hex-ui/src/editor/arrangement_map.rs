//! Cached, view-only clip intervals for the Main lane and section miniatures.
//! Project owns time/identity; section, range and automation owners retain input.
use super::{EditorView, ViewState, viewport::Viewport};
use crate::{section_surface::SectionCoverage, theme};
use gpui::{
    Bounds, Div, Pixels, Rgba, ScaledPixels, Window, canvas, div, fill, point, prelude::*, px, rgb,
    size,
};
use std::rc::Rc;

mod model;
pub(super) use model::{
    Interval, MAX_ROWS, MapCache, SectionMapCache, append_track_intervals, merge, project_interval,
};
use model::{SectionRows, cached, cached_section_rows};

enum Rows {
    Main(Rc<MapCache>),
    Section(SectionRows),
}

impl Rows {
    fn len(&self) -> usize {
        match self {
            Self::Main(data) => data.rows.len(),
            Self::Section(rows) => rows.len(),
        }
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn row(&self, index: usize) -> Option<&[Interval]> {
        match self {
            Self::Main(data) => data.rows.get(index).map(Vec::as_slice),
            Self::Section(rows) => rows.get(index).map(Box::as_ref),
        }
    }
}

pub(super) fn view(
    view: &EditorView,
    viewport: Viewport,
    coverage: Option<SectionCoverage>,
) -> Div {
    let data = view.state.as_ref().map(|state| {
        let focus = view.arrangement_focus_ranges();
        let revealed = view.arrangement_revealed_track();
        let cache = if focus.is_empty() && revealed.is_none() {
            &view.arrangement_map_cache
        } else {
            &view.focused_arrangement_map_cache
        };
        Rows::Main(cached(
            cache,
            view.project_generation,
            state.revision,
            &state.project,
            &focus,
            revealed,
        ))
    });
    paint_layer(data, viewport, None, coverage)
}

/// Render only the clips intersecting `section_range` (`[start, end)`) from
/// every track in `state.project`, independent of arranger visibility filters.
pub(super) fn section(
    view: &EditorView,
    state: &ViewState,
    section_range: (u64, u64),
    viewport: Viewport,
    chamfer: f32,
) -> Div {
    let rows = cached_section_rows(
        &view.section_map_cache,
        view.project_generation,
        state.revision,
        &state.project,
        section_range.0,
        section_range.1,
    );
    let data = (!rows.is_empty()).then_some(Rows::Section(rows));
    paint_layer(data, viewport, Some(chamfer), None)
}

/// Paint one row's miniature marks inside the enclosing paint layer so every
/// miniature quad shares a single BoundsTree draw order and GPU batch.
fn paint_row(
    window: &mut Window,
    color: Rgba,
    top: Pixels,
    height: Pixels,
    spans: impl Iterator<Item = (Pixels, Pixels)>,
    covers: impl Fn(Bounds<ScaledPixels>, Bounds<ScaledPixels>) -> bool,
) {
    let scale = window.scale_factor();
    let scaled_mask = window.content_mask().bounds.scale(scale);
    for (left, width) in spans {
        let span = Bounds::new(point(left, top), size(width, height));
        let scaled = span.scale(scale);
        let clipped = scaled.intersect(&scaled_mask);
        if clipped.is_empty() || covers(scaled, scaled_mask) {
            continue;
        }
        window.paint_quad(fill(span, color));
    }
}

fn can_reserve_layer(bounds: Bounds<Pixels>, mask: Bounds<Pixels>, scale: f32) -> bool {
    let clipped = bounds.intersect(&mask);
    let reserved = clipped.scale(scale);
    !clipped.is_empty()
        && !reserved.is_empty()
        && [
            reserved.left(),
            reserved.right(),
            reserved.top(),
            reserved.bottom(),
        ]
        .into_iter()
        .all(|edge| f64::from(edge).is_finite())
}

fn paint_layer(
    data: Option<Rows>,
    viewport: Viewport,
    chamfer: Option<f32>,
    coverage: Option<SectionCoverage>,
) -> Div {
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
                    if data.is_empty() || viewport.length == 0 {
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
                    let row_height = available / data.len() as f32;
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
                    let paint_all_rows = |window: &mut Window| {
                        for index in 0..data.len() {
                            let Some(row) = data.row(index) else {
                                continue;
                            };
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
                    };
                    let mask = window.content_mask().bounds;
                    if can_reserve_layer(bounds, mask, window.scale_factor()) {
                        window.paint_layer(bounds, paint_all_rows);
                    } else {
                        paint_all_rows(window);
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

    const LONG_ROW_SPAN_COUNT: usize = 19;

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
                                let long: [(f32, f32); LONG_ROW_SPAN_COUNT] =
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
                10 + LONG_ROW_SPAN_COUNT,
                "background, four spans per short row, the long row, and overlay"
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

    #[gpui::test]
    fn section_previews_keep_their_clips_when_q_focus_moves_to_an_empty_section(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::{Modifiers, VisualTestContext};

        let mut model = Editor::new(project(), 8).unwrap();
        for (name, start, end) in [("populated", 0, 4), ("empty", 4, 8)] {
            model
                .execute(Command::AddArrangementSection {
                    name: name.into(),
                    color: hex_project::ElementColor::Orange,
                    start: BeatTime::from_quarters(start).unwrap(),
                    end: BeatTime::from_quarters(end).unwrap(),
                })
                .unwrap();
        }
        let before = model.project().snapshot();
        let empty = model.project().arrangement_sections()[1].id();
        let (handle, editor) = super::super::clip_drag_ui_test::editor_window(
            cx,
            model.project().clone(),
            Default::default(),
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();

        let header = visual.debug_bounds("arrangement-section-header-0").unwrap();
        visual.simulate_click(header.center(), Modifiers::default());
        editor.update(&mut visual, |view, cx| {
            view.toggle_arrangement_focus();
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.arrangement_track_indices(), &[0, 1]);
        });

        let ink = gpui::Background::from(rgb(theme::color(theme::PaletteRole::Background)));
        let previews = |visual: &mut VisualTestContext, selector| {
            let section = visual.debug_bounds(selector).unwrap();
            let scale = visual.update(|window, _| window.scale_factor());
            let body_top = point(
                section.left(),
                section.top() + px(super::super::clip_name::CLIP_NAME_HEIGHT),
            )
            .scale(scale)
            .y;
            let width = section.scale(scale).size.width;
            visual
                .rendered_filled_quads(selector)
                .into_iter()
                .filter_map(|(bounds, color)| {
                    // Scene reads include ancestor backgrounds and the header.
                    (color == ink && bounds.top() >= body_top && bounds.size.width < width)
                        .then_some(bounds)
                })
                .collect::<Vec<_>>()
        };
        let populated = previews(&mut visual, "arrangement-section-0");
        assert_eq!(populated.len(), 2, "populated section audio and MIDI ink");

        let header = visual.debug_bounds("arrangement-section-header-1").unwrap();
        visual.simulate_click(header.center(), Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.sections.selected, Some(empty));
            assert!(view.arrangement_track_indices().is_empty());
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        });
        assert_eq!(previews(&mut visual, "arrangement-section-0"), populated);
        assert!(previews(&mut visual, "arrangement-section-1").is_empty());
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

    fn section_preview_quads(
        visual: &mut gpui::VisualTestContext,
        selector: &'static str,
    ) -> Vec<Bounds<ScaledPixels>> {
        let ink = gpui::Background::from(rgb(theme::color(theme::PaletteRole::Background)));
        let section = visual.debug_bounds(selector).unwrap();
        let scale = visual.update(|window, _| window.scale_factor());
        let body_top = point(
            section.left(),
            section.top() + px(super::super::clip_name::CLIP_NAME_HEIGHT),
        )
        .scale(scale)
        .y;
        let width = section.scale(scale).size.width;
        visual
            .rendered_filled_quads(selector)
            .into_iter()
            .filter_map(|(bounds, color)| {
                (color == ink && bounds.top() >= body_top && bounds.size.width < width)
                    .then_some(bounds)
            })
            .collect()
    }

    #[gpui::test]
    fn section_previews_render_only_their_own_clips_regardless_of_arranger_visibility(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::{Modifiers, VisualTestContext, point, px, size};
        use hex_project::{Created, IDENTITY_CONVERSION, Project, TrackSelectionIds, TrackTag};

        let mut model = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
        let mut tracks = Vec::new();
        for name in ["a-only", "b-only", "muted-a", "empty"] {
            let Created::Track(id) = model
                .execute(Command::AddTrack { name: name.into() })
                .unwrap()
            else {
                panic!("track");
            };
            tracks.push(id);
        }
        // Extra off-screen tracks so vertical scroll hides tracks in the arranger.
        for index in 0..10 {
            model
                .execute(Command::AddTrack {
                    name: format!("spacer {index}"),
                })
                .unwrap();
        }
        let Created::Track(bottom_a) = model
            .execute(Command::AddTrack {
                name: "bottom-a".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let Created::Track(group) = model
            .execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&[tracks[0], tracks[1]]).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group");
        };
        let bass = TrackTag::new("bass").unwrap();
        model
            .execute(Command::SetTrackTags {
                track: tracks[2],
                tags: vec![bass.clone()],
            })
            .unwrap();
        // Section 0 [0, 4) has three tracks (tracks[0], muted tracks[2], bottom_a).
        // Section 1 [4, 8) has one track (tracks[1]).
        // Section 2 [8, 12) is empty.
        model
            .execute(Command::PlaceMidiClip {
                track: tracks[0],
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(2).unwrap(),
            })
            .unwrap();
        model
            .execute(Command::PlaceMidiClip {
                track: tracks[1],
                start: BeatTime::from_quarters(4).unwrap(),
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap();
        model
            .execute(Command::ImportPreparedAudioClip {
                track: tracks[2],
                start: 0,
                path: "/tmp/section-muted.wav".into(),
                source_sample_rate: 48_000,
                source_frames: 48_000,
                playback_frames: 48_000,
                conversion_version: IDENTITY_CONVERSION,
            })
            .unwrap();
        let muted_clip = model.project().track(tracks[2]).unwrap().clips()[0].id();
        model
            .execute(Command::SetClipActive {
                clip: muted_clip,
                active: false,
            })
            .unwrap();
        model
            .execute(Command::PlaceMidiClip {
                track: bottom_a,
                start: BeatTime::from_quarters(2).unwrap(),
                length: BeatTime::from_quarters(2).unwrap(),
            })
            .unwrap();
        for (name, start, end) in [("first", 0, 4), ("second", 4, 8), ("empty", 8, 12)] {
            model
                .execute(Command::AddArrangementSection {
                    name: name.into(),
                    color: hex_project::ElementColor::Orange,
                    start: BeatTime::from_quarters(start).unwrap(),
                    end: BeatTime::from_quarters(end).unwrap(),
                })
                .unwrap();
        }
        let before = model.project().snapshot();
        let (handle, editor) = super::super::clip_drag_ui_test::editor_window(
            cx,
            model.project().clone(),
            Default::default(),
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(1280.0), px(720.0)));
        editor.update(&mut visual, |view, cx| {
            view.viewport = Some(Viewport {
                start: 0,
                length: 384_000,
            });
            cx.notify();
        });
        visual.run_until_parked();

        let scale = visual.update(|window, _| window.scale_factor());
        let first_bounds = visual.debug_bounds("arrangement-section-0").unwrap();
        let second_bounds = visual.debug_bounds("arrangement-section-1").unwrap();
        let body_height =
            f32::from(first_bounds.size.height) - super::super::clip_name::CLIP_NAME_HEIGHT;

        let first_baseline = section_preview_quads(&mut visual, "arrangement-section-0");
        let second_baseline = section_preview_quads(&mut visual, "arrangement-section-1");
        assert_eq!(
            first_baseline.len(),
            3,
            "section 0 renders its 3 tracks including muted and scrolled tracks"
        );
        assert_eq!(
            second_baseline.len(),
            1,
            "section 1 renders only its own 1 track"
        );
        assert!(section_preview_quads(&mut visual, "arrangement-section-2").is_empty());

        // Section 1 has only one track with clips, so that single row fills
        // section 1's full preview height instead of reserving blank rows for
        // the other 14 tracks in the project.
        let expected_single_h = f64::from((body_height - 1.0 / scale) * scale);
        let expected_single_top = f64::from(
            (f32::from(second_bounds.top()) + super::super::clip_name::CLIP_NAME_HEIGHT) * scale,
        );
        assert!(
            (f64::from(second_baseline[0].top()) - expected_single_top).abs() <= 0.5,
            "section 1 single row starts at body top: {:?}",
            second_baseline[0]
        );
        assert!(
            (f64::from(second_baseline[0].size.height) - expected_single_h).abs() <= 1.0,
            "section 1 single row fills body height ({expected_single_h}): {:?}",
            second_baseline[0]
        );

        // Hide tracks in the arranger via group fold, tag filter, Q focus and
        // vertical scroll. Section previews must remain unchanged throughout.
        editor.update(&mut visual, |view, cx| {
            view.arrangement_focus.projection.toggle_fold(group);
            view.reconcile_arrangement_focus();
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(!view.arrangement_track_visible(tracks[0]));
            assert!(!view.arrangement_track_visible(tracks[1]));
        });
        assert_eq!(
            section_preview_quads(&mut visual, "arrangement-section-0"),
            first_baseline
        );
        assert_eq!(
            section_preview_quads(&mut visual, "arrangement-section-1"),
            second_baseline
        );

        editor.update(&mut visual, |view, cx| {
            view.arrangement_focus
                .projection
                .set_tag_filter(Some(bass.clone()));
            view.reconcile_arrangement_focus();
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(!view.arrangement_track_visible(bottom_a));
            assert!(!view.arrangement_track_visible(tracks[1]));
            assert!(view.arrangement_track_visible(tracks[2]));
        });
        assert_eq!(
            section_preview_quads(&mut visual, "arrangement-section-0"),
            first_baseline
        );
        assert_eq!(
            section_preview_quads(&mut visual, "arrangement-section-1"),
            second_baseline
        );

        let header = visual.debug_bounds("arrangement-section-header-2").unwrap();
        visual.simulate_click(header.center(), Modifiers::default());
        editor.update(&mut visual, |view, cx| {
            view.toggle_arrangement_focus();
            view.track_scroll.set_offset(point(px(0.0), px(-600.0)));
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(view.arrangement_track_indices().is_empty());
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        });
        assert_eq!(
            section_preview_quads(&mut visual, "arrangement-section-0"),
            first_baseline
        );
        assert_eq!(
            section_preview_quads(&mut visual, "arrangement-section-1"),
            second_baseline
        );
        assert!(section_preview_quads(&mut visual, "arrangement-section-2").is_empty());
    }

    #[gpui::test]
    fn section_previews_keep_their_own_clips_during_reorder_drag_and_pending_layout(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::{Modifiers, MouseButton, VisualTestContext, point, px, size};
        use hex_project::{Created, Project};

        let mut model = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let mut tracks = Vec::new();
        for name in ["first", "second"] {
            let Created::Track(id) = model
                .execute(Command::AddTrack { name: name.into() })
                .unwrap()
            else {
                panic!("track");
            };
            tracks.push(id);
        }
        // Section 0 [0, 4) has a half-width clip [0, 2); section 1 [4, 8) has a full-width clip [4, 8).
        model
            .execute(Command::PlaceMidiClip {
                track: tracks[0],
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(2).unwrap(),
            })
            .unwrap();
        model
            .execute(Command::PlaceMidiClip {
                track: tracks[1],
                start: BeatTime::from_quarters(4).unwrap(),
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap();
        for (name, start, end) in [("half", 0, 4), ("full", 4, 8)] {
            model
                .execute(Command::AddArrangementSection {
                    name: name.into(),
                    color: hex_project::ElementColor::Orange,
                    start: BeatTime::from_quarters(start).unwrap(),
                    end: BeatTime::from_quarters(end).unwrap(),
                })
                .unwrap();
        }
        let (handle, editor) = super::super::clip_drag_ui_test::editor_window(
            cx,
            model.project().clone(),
            Default::default(),
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(1280.0), px(720.0)));
        editor.update(&mut visual, |view, cx| {
            view.viewport = Some(Viewport {
                start: 0,
                length: 288_000,
            });
            cx.notify();
        });
        visual.run_until_parked();

        let half_before = section_preview_quads(&mut visual, "arrangement-section-0");
        let full_before = section_preview_quads(&mut visual, "arrangement-section-1");
        assert_eq!(half_before.len(), 1);
        assert_eq!(full_before.len(), 1);
        assert!(
            full_before[0].size.width > half_before[0].size.width * 1.5,
            "resting sections must distinguish half ({:?}) and full ({:?}) clips",
            half_before[0],
            full_before[0]
        );

        // Drag section 0 ("half") past the right edge of section 1 ("full").
        let header0 = visual.debug_bounds("arrangement-section-header-0").unwrap();
        let section1 = visual.debug_bounds("arrangement-section-1").unwrap();
        let start_pt = point(
            header0.left() + header0.size.width * 0.75,
            header0.center().y,
        );
        let target_pt = point(section1.right() - px(2.0), header0.center().y);
        visual.simulate_mouse_down(start_pt, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(target_pt, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();

        // While dragging, "full" is now at index 0 and "half" is at index 1.
        let drag_first = section_preview_quads(&mut visual, "arrangement-section-0");
        let drag_second = section_preview_quads(&mut visual, "arrangement-section-1");
        assert_eq!(drag_first.len(), 1);
        assert_eq!(drag_second.len(), 1);
        assert!(
            drag_first[0].size.width > drag_second[0].size.width * 1.5,
            "dragged sections must carry their own clips: index0={:?}, index1={:?}",
            drag_first[0],
            drag_second[0]
        );

        // Release without completing the worker ACK: pending layout preview must
        // keep each moved section showing its own clips.
        visual.simulate_mouse_up(target_pt, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| assert!(view.sections.pending()));
        let pending_first = section_preview_quads(&mut visual, "arrangement-section-0");
        let pending_second = section_preview_quads(&mut visual, "arrangement-section-1");
        assert_eq!(pending_first.len(), 1);
        assert_eq!(pending_second.len(), 1);
        assert!(
            pending_first[0].size.width > pending_second[0].size.width * 1.5,
            "pending sections must keep their own clips: index0={:?}, index1={:?}",
            pending_first[0],
            pending_second[0]
        );
    }
}
