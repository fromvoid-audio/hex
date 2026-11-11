//! Cached, view-only clip intervals for the Main lane's arrangement miniature.
//! Project owns time/identity; section, range and automation owners retain input.
use super::{EditorView, viewport::Viewport};
use crate::theme;
use gpui::{Div, canvas, div, fill, point, prelude::*, px, rgb, size};

mod model;
use model::cached;
pub(super) use model::{
    Interval, MAX_ROWS, MapCache, append_track_intervals, merge, project_interval,
};

pub(super) fn view(view: &EditorView, viewport: Viewport) -> Div {
    layer(view, viewport, None)
}

pub(super) fn section(view: &EditorView, start: u64, end: u64, chamfer: f32) -> Div {
    layer(
        view,
        Viewport {
            start,
            length: end.saturating_sub(start),
        },
        Some(chamfer),
    )
}

fn layer(view: &EditorView, viewport: Viewport, chamfer: Option<f32>) -> Div {
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
                    let color = rgb(theme::color(if chamfer.is_some() {
                        theme::PaletteRole::Background
                    } else {
                        theme::PaletteRole::Raised
                    }));
                    for (index, row) in data.rows.iter().enumerate() {
                        let first = row.partition_point(|span| span.end <= viewport.start);
                        let y = top + index as f32 * row_height;
                        let h = (row_height - pixel).max(pixel).min(height - y);
                        for &span in row[first..].iter().take_while(|span| span.start < limit) {
                            if let Some((left, right)) = project_interval(span, viewport, width) {
                                let edge = chamfer.map_or(width, |cut| {
                                    crate::section_surface::miniature_right(
                                        width,
                                        height,
                                        cut,
                                        y + h,
                                    )
                                });
                                let w = (right - left - pixel)
                                    .max(pixel)
                                    .min(right - left)
                                    .min(edge - left);
                                if w <= 0.0 {
                                    continue;
                                }
                                window.paint_quad(fill(
                                    gpui::Bounds::new(
                                        point(bounds.left() + px(left), bounds.top() + px(y)),
                                        size(px(w), px(h)),
                                    ),
                                    color,
                                ));
                            }
                        }
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
