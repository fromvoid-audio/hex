//! Passive, revision-keyed group content. Canonical groups own no clips;
//! this projection shares descendant interval rows and never emits edits.
use super::{
    EditorView, ViewState,
    arrangement_map::{Interval, MAX_ROWS, append_track_intervals, merge, project_interval},
    viewport::Viewport,
};
use crate::theme;
use gpui::{Div, canvas, div, fill, point, prelude::*, px, rgb, size};
use hex_project::{Project, Track, TrackId};
use std::{collections::BTreeMap, rc::Rc};

#[derive(Default)]
struct Group {
    rows: Vec<Rc<[Interval]>>,
    blocks: Vec<Interval>,
}
#[derive(Default)]
pub(super) struct Cache {
    stamp: Option<(u64, u64)>,
    groups: Rc<BTreeMap<TrackId, Group>>,
}
impl Cache {
    fn read(
        &mut self,
        generation: u64,
        revision: u64,
        project: &Project,
    ) -> Rc<BTreeMap<TrackId, Group>> {
        if self.stamp != Some((generation, revision)) {
            self.groups = Rc::new(project_groups(project));
            self.stamp = Some((generation, revision));
        }
        Rc::clone(&self.groups)
    }
}
fn project_groups(project: &Project) -> BTreeMap<TrackId, Group> {
    let mut groups = BTreeMap::<TrackId, Group>::new();
    let mut ancestors = Vec::with_capacity(hex_project::MAX_GROUP_DEPTH);
    for track in project.tracks() {
        while ancestors.last().copied() != track.parent_group() && !ancestors.is_empty() {
            ancestors.pop();
        }
        if track.is_group() {
            groups.entry(track.id()).or_default();
            ancestors.push(track.id());
        } else if !ancestors.is_empty() {
            let mut intervals = Vec::new();
            append_track_intervals(project, track, &mut intervals);
            merge(&mut intervals);
            if intervals.is_empty() {
                continue;
            }
            let row: Rc<[Interval]> = intervals.into();
            for ancestor in &ancestors {
                let group = groups.entry(*ancestor).or_default();
                group.blocks.extend_from_slice(&row);
                group.rows.push(Rc::clone(&row));
            }
        }
    }
    for group in groups.values_mut() {
        merge(&mut group.blocks);
    }
    groups
}

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
                            window.paint_quad(fill(
                                gpui::Bounds::new(
                                    point(bounds.left() + px(left), bounds.top()),
                                    size(px(right - left), bounds.size.height),
                                ),
                                rgb(color),
                            ));
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
                                let w = (right - left - pixel)
                                    .max(pixel)
                                    .min(right - left)
                                    .min(width - left);
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

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{
        BeatTime, Command, Created, Editor, IDENTITY_CONVERSION, Tempo, TrackSelectionIds,
    };
    fn add(model: &mut Editor, name: &str) -> TrackId {
        let Created::Track(id) = model
            .execute(Command::AddTrack { name: name.into() })
            .unwrap()
        else {
            panic!("track")
        };
        id
    }
    fn group(model: &mut Editor, tracks: &[TrackId], name: &str) -> TrackId {
        let Created::Track(id) = model
            .execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(tracks).unwrap(),
                name: name.into(),
            })
            .unwrap()
        else {
            panic!("group")
        };
        id
    }
    fn midi(model: &mut Editor, track: TrackId, start: u64, length: u64) {
        model
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::from_quarters(start).unwrap(),
                length: BeatTime::from_quarters(length).unwrap(),
            })
            .unwrap();
    }
    fn mixed() -> (Editor, TrackId, TrackId, TrackId) {
        let mut model = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
        let a = add(&mut model, "a");
        let b = add(&mut model, "b");
        let c = add(&mut model, "c");
        let other = add(&mut model, "unrelated");
        model
            .execute(Command::SetTempoPoint {
                beat: BeatTime::from_quarters(4).unwrap(),
                tempo: Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        model
            .execute(Command::ImportPreparedAudioClip {
                track: a,
                start: 0,
                path: "/tmp/group-overview.wav".into(),
                source_sample_rate: 48_000,
                source_frames: 96_000,
                playback_frames: 96_000,
                conversion_version: IDENTITY_CONVERSION,
            })
            .unwrap();
        midi(&mut model, b, 2, 4);
        midi(&mut model, c, 8, 2);
        midi(&mut model, other, 0, 64);
        let inner = group(&mut model, &[a, b], "inner");
        let outer = group(&mut model, &[inner, c], "outer");
        (model, inner, outer, a)
    }
    #[test]
    fn nested_groups_share_rows_use_mixed_time_and_exclude_unrelated_content() {
        let (model, inner, outer, _) = mixed();
        let before = model.project().snapshot();
        let data = project_groups(model.project());
        assert_eq!(
            data[&inner].blocks,
            vec![Interval {
                start: 0,
                end: 192_000
            }]
        );
        assert_eq!(
            data[&outer].blocks,
            vec![
                Interval {
                    start: 0,
                    end: 192_000
                },
                Interval {
                    start: 288_000,
                    end: 384_000
                }
            ]
        );
        assert_eq!(data[&inner].rows.len(), 2);
        assert_eq!(data[&outer].rows.len(), 3);
        assert!(Rc::ptr_eq(&data[&inner].rows[0], &data[&outer].rows[0]));
        assert!(Rc::ptr_eq(&data[&inner].rows[1], &data[&outer].rows[1]));
        assert_eq!(model.project().snapshot(), before);
    }
    #[test]
    fn adjacent_boundaries_and_empty_groups_are_not_invented_or_joined() {
        let mut model = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let a = add(&mut model, "a");
        let empty = add(&mut model, "empty");
        midi(&mut model, a, 0, 2);
        midi(&mut model, a, 2, 2);
        midi(&mut model, a, 6, 2);
        let full = group(&mut model, &[a], "full");
        let empty = group(&mut model, &[empty], "empty");
        let data = project_groups(model.project());
        assert_eq!(
            data[&full].blocks,
            vec![
                Interval {
                    start: 0,
                    end: 48_000
                },
                Interval {
                    start: 48_000,
                    end: 96_000
                },
                Interval {
                    start: 144_000,
                    end: 192_000
                }
            ]
        );
        assert!(data[&empty].rows.is_empty() && data[&empty].blocks.is_empty());
    }
    #[test]
    fn cache_reuses_projection_and_retires_changed_or_replaced_context() {
        let (mut model, _, outer, a) = mixed();
        let mut cache = Cache::default();
        let original = cache.read(1, 1, model.project());
        assert!(Rc::ptr_eq(&original, &cache.read(1, 1, model.project())));
        let clip = model.project().track(a).unwrap().clips()[0].id();
        model.execute(Command::RemoveClip(clip)).unwrap();
        let changed = cache.read(1, 2, model.project());
        assert!(!Rc::ptr_eq(&original, &changed));
        assert_eq!(changed[&outer].blocks[0].start, 48_000);
        assert!(model.undo());
        let restored = cache.read(1, 3, model.project());
        assert_eq!(restored[&outer].blocks, original[&outer].blocks);
        assert!(cache.read(2, 3, &Project::new(48_000).unwrap()).is_empty());
    }
    #[test]
    fn hundred_track_projection_is_bounded_and_reused() {
        let mut model = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
        let mut tracks = Vec::new();
        for i in 0..100 {
            let track = add(&mut model, &format!("track {i}"));
            midi(&mut model, track, 0, 4);
            tracks.push(track);
        }
        let group = group(&mut model, &tracks, "all");
        let mut cache = Cache::default();
        let first = cache.read(1, 1, model.project());
        assert_eq!(first[&group].rows.len(), 100);
        assert_eq!(
            first[&group].blocks,
            vec![Interval {
                start: 0,
                end: 96_000
            }]
        );
        assert!(Rc::ptr_eq(&first, &cache.read(1, 1, model.project())));
    }
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
                assert_eq!(blocks.len(), 1);
                let expected_width = if viewport.start == 0 { 0.5 } else { 0.75 };
                assert!((blocks[0].0.left() / scale - f32::from(bounds.left())).abs() < 0.05);
                assert!(
                    (blocks[0].0.size.width / scale
                        - f32::from(bounds.size.width) * expected_width)
                        .abs()
                        < 0.1
                );
                assert!(
                    (blocks[0].0.size.height / scale - f32::from(bounds.size.height)).abs() < 0.05
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
