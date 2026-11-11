//! Passive, revision-keyed group content. Canonical groups own no clips;
//! this projection shares descendant interval rows and never emits edits.
use super::{
    EditorView, ViewState,
    arrangement_map::{MAX_ROWS, project_interval},
    viewport::Viewport,
};
use crate::theme;
use gpui::{Div, canvas, div, fill, point, prelude::*, px, rgb, size};
use hex_project::Track;

mod model;
pub(super) use model::Cache;

pub(super) fn view(view: &EditorView, state: &ViewState, track: &Track, viewport: Viewport) -> Div {
    let data = view.group_overview_cache.borrow_mut().read(
        view.project_generation,
        state.revision,
        &state.project,
    );
    let color = theme::section_colors(track.color(), false, false).0;
    let track = track.id();
    let populated = data
        .get(&track)
        .is_some_and(|group| !group.blocks.is_empty());
    div()
        .debug_selector(move || format!("group-overview-{}", track.value()))
        .absolute()
        .inset_0()
        .opacity(if view.automation_view.enabled {
            0.22
        } else {
            1.0
        })
        .children(populated.then(|| {
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let Some(group) = data.get(&track) else {
                        return;
                    };
                    let Some(limit) = viewport.end() else {
                        return;
                    };
                    let width = f32::from(bounds.size.width);
                    let height = f32::from(bounds.size.height);
                    if !width.is_finite()
                        || !height.is_finite()
                        || width <= 0.0
                        || height <= 0.0
                        || viewport.length == 0
                    {
                        return;
                    }
                    for &block in group
                        .blocks
                        .iter()
                        .skip_while(|block| block.end <= viewport.start)
                        .take_while(|block| block.start < limit)
                    {
                        if let Some((left, right)) = project_interval(block, viewport, width) {
                            crate::section_surface::paint(
                                gpui::Bounds::new(
                                    point(bounds.left() + px(left), bounds.top()),
                                    size(px(right - left), bounds.size.height),
                                ),
                                if block.end <= limit {
                                    crate::section_surface::CHAMFER
                                } else {
                                    0.0
                                },
                                color,
                                None,
                                window,
                            );
                        }
                    }
                    if group.rows.is_empty() {
                        return;
                    }
                    let pixel = 1.0 / window.scale_factor();
                    let inset = 2.0_f32.min(height / 4.0);
                    let available = height - 2.0 * inset;
                    let slots = group
                        .rows
                        .len()
                        .min(MAX_ROWS)
                        .min((available / (2.0 * pixel)).floor() as usize)
                        .max(1);
                    let row_height = available / slots as f32;
                    let ink = rgb(theme::color(theme::PaletteRole::Background));
                    for (index, row) in group.rows.iter().enumerate() {
                        let y = inset + (index * slots / group.rows.len()) as f32 * row_height;
                        let h = (row_height - pixel).max(pixel).min(height - inset - y);
                        let first = row.partition_point(|span| span.end <= viewport.start);
                        for &span in row[first..].iter().take_while(|span| span.start < limit) {
                            if let Some((left, right)) = project_interval(span, viewport, width) {
                                // A compressed row stays inside its containing block's cut.
                                let block = group.blocks.get(
                                    group
                                        .blocks
                                        .partition_point(|block| block.end <= span.start),
                                );
                                let edge = block
                                    .and_then(|block| {
                                        project_interval(*block, viewport, width).map(|(l, r)| {
                                            l + crate::section_surface::miniature_right(
                                                r - l,
                                                height,
                                                if block.end <= limit {
                                                    crate::section_surface::CHAMFER
                                                } else {
                                                    0.0
                                                },
                                                y + h,
                                            )
                                        })
                                    })
                                    .unwrap_or(right);
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
                                    ink,
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
    use super::*;
    use hex_project::{BeatTime, Command, TrackSelectionIds};
    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn native_group_preview_tracks_viewport_content_removal_and_undo(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::editor::track_reorder_ui_test::{live_tracks, receive};
        use gpui::{Entity, VisualTestContext};
        use hex_session::service::Request;
        let (mut visual, view, ids) = live_tracks(cx, 2, false);
        let edit = |visual: &mut VisualTestContext, view: &Entity<EditorView>, command| {
            view.update(visual, |v, _| assert!(v.enqueue(Request::Edit(command))));
            receive(visual, view);
        };
        edit(
            &mut visual,
            &view,
            Command::PlaceMidiClip {
                track: ids[0],
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(8).unwrap(),
            },
        );
        edit(
            &mut visual,
            &view,
            Command::PlaceMidiClip {
                track: ids[1],
                start: BeatTime::ONE,
                length: BeatTime::from_quarters(2).unwrap(),
            },
        );
        edit(
            &mut visual,
            &view,
            Command::GroupTracks {
                tracks: TrackSelectionIds::new(&ids).unwrap(),
                name: "rhythm".into(),
            },
        );
        let (group, color, clips, original) = view.read_with(&visual, |v, _| {
            let project = &v.state.as_ref().unwrap().project;
            let track = &project.tracks()[0];
            (
                track.id(),
                theme::section_colors(track.color(), false, false).0,
                ids.iter()
                    .map(|id| project.track(*id).unwrap().midi_clips()[0].id())
                    .collect::<Vec<_>>(),
                project.snapshot(),
            )
        });
        let selector =
            Box::leak(format!("group-overview-{}", group.value()).into_boxed_str()) as &'static str;
        for width in [880.0, 1440.0] {
            for viewport in [
                Viewport {
                    start: 0,
                    length: 384_000,
                },
                Viewport {
                    start: 48_000,
                    length: 192_000,
                },
                Viewport {
                    start: 0,
                    length: 96_000,
                },
            ] {
                visual.simulate_resize(size(px(width), px(800.0)));
                view.update(&mut visual, |v, cx| {
                    v.viewport = Some(viewport);
                    cx.notify();
                });
                visual.run_until_parked();
                let bounds = visual.debug_bounds(selector).unwrap();
                let scale =
                    gpui::ScaledPixels::from(visual.update(|window, _| window.scale_factor()));
                let fill_color: gpui::Background = rgb(color).into();
                let blocks: Vec<_> = visual
                    .rendered_filled_quads(selector)
                    .into_iter()
                    .filter(|(_, c)| c == &fill_color)
                    .collect();
                let clipped_end = viewport.length == 96_000;
                assert_eq!(blocks.len(), if clipped_end { 1 } else { 2 });
                let expected_width = if clipped_end {
                    1.0
                } else if viewport.start == 0 {
                    0.5
                } else {
                    0.75
                };
                assert!((blocks[0].0.left() / scale - f32::from(bounds.left())).abs() < 0.05);
                assert!(
                    (blocks[0].0.size.width / scale
                        - f32::from(bounds.size.width) * expected_width)
                        .abs()
                        < 0.1
                );
                assert!(
                    (blocks[0].0.size.height / scale
                        - (f32::from(bounds.size.height)
                            - if clipped_end {
                                0.0
                            } else {
                                crate::section_surface::CHAMFER
                            }))
                    .abs()
                        < 0.05
                );
                let ink: gpui::Background =
                    rgb(theme::color(theme::PaletteRole::Background)).into();
                assert_eq!(
                    visual
                        .rendered_filled_quads(selector)
                        .into_iter()
                        .filter(|(b, c)| c == &ink
                            && b.size.height / scale < f32::from(bounds.size.height) / 2.0
                            && b.top() / scale > f32::from(bounds.top()) + 1.0
                            && b.bottom() / scale < f32::from(bounds.bottom()) - 1.0
                            && b.size.width / scale > 2.0)
                        .count(),
                    2
                );
                view.read_with(&visual, |v, _| {
                    assert_eq!(v.state.as_ref().unwrap().project.snapshot(), original)
                });
            }
        }
        for clip in clips {
            edit(&mut visual, &view, Command::RemoveMidiClip(clip));
        }
        visual.run_until_parked();
        let fill_color: gpui::Background = rgb(color).into();
        assert!(
            !visual
                .rendered_filled_quads(selector)
                .into_iter()
                .any(|(_, c)| c == fill_color)
        );
        for _ in 0..2 {
            view.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
            receive(&mut visual, &view);
        }
        visual.run_until_parked();
        assert!(
            visual
                .rendered_filled_quads(selector)
                .into_iter()
                .any(|(_, c)| c == fill_color)
        );
        view.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), original)
        });
    }
}
