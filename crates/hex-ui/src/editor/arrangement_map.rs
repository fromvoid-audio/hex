//! Cached, view-only clip intervals for the Main lane's arrangement miniature.
//! Project owns time/identity; section, range and automation owners retain input.
use super::{EditorView, viewport::Viewport};
use crate::theme;
use gpui::{Div, canvas, div, fill, point, prelude::*, px, rgb, size};
use hex_project::{BeatTime, Project, TrackId};
use std::{cell::RefCell, rc::Rc};

pub(super) const MAX_ROWS: usize = 32;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Interval {
    pub(super) start: u64,
    pub(super) end: u64,
}

pub(super) struct MapCache {
    generation: u64,
    revision: u64,
    focus: Rc<[(u64, u64)]>,
    revealed: Option<TrackId>,
    rows: Vec<Vec<Interval>>,
}

pub(super) fn merge(row: &mut Vec<Interval>) {
    row.sort_unstable_by_key(|span| (span.start, span.end));
    let mut count = 0;
    for read in 0..row.len() {
        let span = row[read];
        if span.start >= span.end {
            continue;
        }
        if count > 0 && row[count - 1].end > span.start {
            row[count - 1].end = row[count - 1].end.max(span.end);
        } else {
            row[count] = span;
            count += 1;
        }
    }
    row.truncate(count);
}

fn project_rows(
    project: &Project,
    focus: &[(u64, u64)],
    revealed: Option<TrackId>,
) -> Vec<Vec<Interval>> {
    let visible = |track: &&hex_project::Track| {
        revealed == Some(track.id())
            || super::arrangement_focus::track_intersects_sections(project, track, focus)
    };
    let count = project.tracks().iter().filter(visible).count();
    let mut rows = vec![Vec::new(); count.min(MAX_ROWS)];
    if rows.is_empty() {
        return rows;
    }
    let row_count = rows.len();
    for (index, track) in project.tracks().iter().filter(visible).enumerate() {
        let row = &mut rows[index * row_count / count];
        append_track_intervals(project, track, row);
    }
    for row in &mut rows {
        merge(row);
    }
    rows
}

/// Canonical clip containers, independent of view filters and preview data.
pub(super) fn append_track_intervals(
    project: &Project,
    track: &hex_project::Track,
    row: &mut Vec<Interval>,
) {
    for clip in track.clips() {
        if let Some(end) = clip.start().checked_add(clip.duration()) {
            row.push(Interval {
                start: clip.start(),
                end,
            });
        }
    }
    for clip in track.midi_clips() {
        let Some(end) = clip.start().raw().checked_add(clip.length().raw()) else {
            continue;
        };
        if let (Ok(start), Ok(end)) = (
            project.tempo_map().beat_to_frame(clip.start()),
            project.tempo_map().beat_to_frame(BeatTime::from_raw(end)),
        ) {
            row.push(Interval { start, end });
        }
    }
}

fn cached(
    cache: &RefCell<Option<Rc<MapCache>>>,
    generation: u64,
    revision: u64,
    project: &Project,
    focus: Rc<[(u64, u64)]>,
    revealed: Option<TrackId>,
) -> Rc<MapCache> {
    let mut slot = cache.borrow_mut();
    if let Some(current) = slot.as_ref().filter(|data| {
        data.generation == generation
            && data.revision == revision
            && data.focus == focus
            && data.revealed == revealed
    }) {
        return Rc::clone(current);
    }
    let next = Rc::new(MapCache {
        generation,
        revision,
        rows: project_rows(project, &focus, revealed),
        focus,
        revealed,
    });
    *slot = Some(Rc::clone(&next));
    next
}

pub(super) fn project_interval(
    span: Interval,
    viewport: Viewport,
    width: f32,
) -> Option<(f32, f32)> {
    let limit = viewport.end()?;
    if viewport.length == 0 || !width.is_finite() || width <= 0.0 {
        return None;
    }
    let start = span.start.max(viewport.start);
    let end = span.end.min(limit);
    if start >= end {
        return None;
    }
    let x = |frame: u64| {
        ((frame - viewport.start) as f64 / viewport.length as f64 * f64::from(width)) as f32
    };
    Some((x(start), x(end)))
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, IDENTITY_CONVERSION, Tempo};
    fn project() -> Project {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let mut tracks = Vec::new();
        for name in ["audio", "midi"] {
            let Created::Track(id) = editor
                .execute(Command::AddTrack { name: name.into() })
                .unwrap()
            else {
                panic!("track");
            };
            tracks.push(id);
        }
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::ZERO,
                tempo: Tempo::new(120_000_000).unwrap(),
            })
            .unwrap();
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::from_quarters(2).unwrap(),
                tempo: Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        editor
            .execute(Command::ImportPreparedAudioClip {
                track: tracks[0],
                start: 48_000,
                path: "/tmp/main-map.wav".into(),
                source_sample_rate: 48_000,
                source_frames: 48_000,
                playback_frames: 48_000,
                conversion_version: IDENTITY_CONVERSION,
            })
            .unwrap();
        editor
            .execute(Command::PlaceMidiClip {
                track: tracks[1],
                start: BeatTime::ONE,
                length: BeatTime::from_quarters(3).unwrap(),
            })
            .unwrap();
        editor.project().clone()
    }
    #[test]
    fn canonical_rows_convert_midi_tempo_and_focus_without_changing_project() {
        let project = project();
        let before = project.snapshot();
        assert_eq!(
            project_rows(&project, &[], None),
            vec![
                vec![Interval {
                    start: 48_000,
                    end: 96_000
                }],
                vec![Interval {
                    start: 24_000,
                    end: 144_000
                }]
            ]
        );
        assert_eq!(project_rows(&project, &[(0, 24_001)], None).len(), 1);
        assert_eq!(
            project_rows(&project, &[(0, 24_001)], Some(project.tracks()[0].id())).len(),
            2
        );
        assert_eq!(project.snapshot(), before);
    }
    #[test]
    fn overlapping_ranges_merge_but_adjacent_clips_remain_distinct() {
        let mut row = vec![
            Interval { start: 4, end: 6 },
            Interval { start: 0, end: 5 },
            Interval { start: 6, end: 9 },
            Interval { start: 10, end: 10 },
        ];
        merge(&mut row);
        assert_eq!(
            row,
            vec![Interval { start: 0, end: 6 }, Interval { start: 6, end: 9 }]
        );
    }
    #[test]
    fn exact_view_projection_clips_and_rejects_invalid_geometry() {
        let viewport = Viewport {
            start: 100,
            length: 100,
        };
        assert_eq!(
            project_interval(
                Interval {
                    start: 125,
                    end: 175
                },
                viewport,
                200.0
            ),
            Some((50.0, 150.0))
        );
        assert_eq!(
            project_interval(
                Interval {
                    start: 50,
                    end: 250
                },
                viewport,
                200.0
            ),
            Some((0.0, 200.0))
        );
        for width in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                project_interval(
                    Interval {
                        start: 100,
                        end: 200
                    },
                    viewport,
                    width
                ),
                None
            );
        }
        assert_eq!(
            project_interval(
                Interval {
                    start: 200,
                    end: 250
                },
                viewport,
                100.0
            ),
            None
        );
        assert_eq!(
            project_interval(
                Interval { start: 1, end: 2 },
                Viewport {
                    start: u64::MAX,
                    length: 1
                },
                100.0
            ),
            None
        );
    }
    #[test]
    fn cache_reuses_pan_independent_data_and_invalidates_owner_context() {
        let project = project();
        let cache = RefCell::new(None);
        let focus: Rc<[(u64, u64)]> = Rc::from([]);
        let first = cached(&cache, 1, 1, &project, Rc::clone(&focus), None);
        assert!(Rc::ptr_eq(
            &first,
            &cached(&cache, 1, 1, &project, Rc::clone(&focus), None)
        ));
        let second = cached(&cache, 1, 2, &project, Rc::clone(&focus), None);
        assert!(!Rc::ptr_eq(&first, &second));
        let third = cached(&cache, 2, 2, &project, Rc::clone(&focus), None);
        assert!(!Rc::ptr_eq(&second, &third));
        let fourth = cached(&cache, 2, 2, &project, Rc::from([(0, 24_001)]), None);
        assert!(!Rc::ptr_eq(&third, &fourth));
        assert!(project_rows(&Project::new(48_000).unwrap(), &[], None).is_empty());
    }
    #[test]
    fn revisioned_edits_and_undo_restore_canonical_ranges() {
        let mut model = Editor::new(project(), 8).unwrap();
        let track = model.project().tracks()[0].id();
        let clip = model.project().tracks()[0].clips()[0].id();
        let cache = RefCell::new(None);
        let original = cached(&cache, 1, 1, model.project(), Rc::from([]), None);
        model.execute(Command::RemoveClip(clip)).unwrap();
        let removed = cached(&cache, 1, 2, model.project(), Rc::from([]), None);
        assert!(removed.rows[0].is_empty());
        assert!(model.undo());
        let restored = cached(&cache, 1, 3, model.project(), Rc::from([]), None);
        assert_eq!(restored.rows, original.rows);
        model
            .execute(Command::SetTrackColor {
                track,
                color: hex_project::ElementColor::Violet,
            })
            .unwrap();
        let recolored = cached(&cache, 1, 4, model.project(), Rc::from([]), None);
        assert_eq!(recolored.rows, restored.rows);
        assert!(!Rc::ptr_eq(&recolored, &restored));
    }
    #[test]
    fn dense_projects_are_bounded_without_losing_the_bottom_track() {
        let mut model = Editor::new(Project::new(48_000).unwrap(), 64).unwrap();
        for index in 0..64 {
            let Created::Track(track) = model
                .execute(Command::AddTrack {
                    name: index.to_string(),
                })
                .unwrap()
            else {
                panic!("track")
            };
            model
                .execute(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::ZERO,
                    length: BeatTime::ONE,
                })
                .unwrap();
        }
        let rows = project_rows(model.project(), &[], None);
        assert_eq!(rows.len(), MAX_ROWS);
        assert!(
            rows.iter()
                .all(|row| row.len() == 1 && row[0].start == 0 && row[0].end == 24_000)
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
