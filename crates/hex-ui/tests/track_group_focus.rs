//! Focused visibility contract using the same private projection as the UI.
#[path = "../src/editor/arrangement_focus/interval.rs"]
mod interval;
#[path = "../src/editor/track_reorder/subtree.rs"]
mod subtree;
#[path = "../src/editor/track_hierarchy_indent.rs"]
mod track_hierarchy_indent;
#[path = "../src/editor/arrangement_focus/visibility.rs"]
mod visibility;

use hex_project::{BeatTime, Command, Created, Editor, Project, TrackId, TrackSelectionIds};
use std::collections::BTreeSet;

#[test]
fn matching_tracks_keep_nested_ancestors_and_revealed_track_context() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let add = |editor: &mut Editor, name: &str| {
        let Created::Track(id) = editor
            .execute(Command::AddTrack { name: name.into() })
            .unwrap()
        else {
            panic!("track")
        };
        id
    };
    let inside = add(&mut editor, "inside");
    let outside = add(&mut editor, "outside");
    for (track, start, length) in [(inside, 2, 1), (outside, 0, 2)] {
        editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::from_quarters(start).unwrap(),
                length: BeatTime::from_quarters(length).unwrap(),
            })
            .unwrap();
    }
    let group = |editor: &mut Editor, track: TrackId| {
        let Created::Track(id) = editor
            .execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&[track]).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group")
        };
        id
    };
    let inner = group(&mut editor, inside);
    let outer = group(&mut editor, inner);
    let unrelated = group(&mut editor, outside);
    let Created::ArrangementSection(section) = editor
        .execute(Command::AddArrangementSection {
            name: "selected".into(),
            color: hex_project::ElementColor::Cyan,
            start: BeatTime::from_quarters(2).unwrap(),
            end: BeatTime::from_quarters(3).unwrap(),
        })
        .unwrap()
    else {
        panic!("section")
    };
    let project = editor.project();
    let mut indent = track_hierarchy_indent::HierarchyIndent::default();
    indent.reconcile(1, 1, project);
    assert_eq!(indent.pixels(outer), 0.0);
    assert_eq!(indent.pixels(inner), 4.0);
    assert_eq!(indent.pixels(inside), 8.0);
    assert_eq!(indent.pixels(unrelated), 0.0);
    assert_eq!(indent.pixels(outside), 4.0);
    let source =
        subtree::SourceRows::capture(project, &TrackSelectionIds::new(&[outer, inner]).unwrap())
            .unwrap();
    let index = |id| {
        project
            .tracks()
            .iter()
            .position(|track| track.id() == id)
            .unwrap()
    };
    assert!(!source.permits(index(inner), Some(outer)));
    assert!(!source.permits_group(inner));
    assert!(!source.permits(index(inside), Some(inner)));
    assert!(!source.group_ends_by(inner, index(inside)));
    assert!(source.group_ends_by(inner, index(unrelated)));
    assert!(source.group_ends_by(outer, index(unrelated)));
    assert!(
        !source.permits(index(unrelated), None),
        "the boundary after the complete subtree is a no-op"
    );
    assert!(
        source.permits(project.tracks().len(), None),
        "the group can move after an unrelated subtree"
    );
    let nested =
        subtree::SourceRows::capture(project, &TrackSelectionIds::new(&[inner]).unwrap()).unwrap();
    assert!(nested.permits_group(unrelated));
    assert!(!nested.permits_group(outer));
    assert!(
        nested.permits(index(unrelated), None),
        "unchanged order can still reparent a subtree"
    );
    assert!(
        !nested.permits(index(unrelated), Some(outer)),
        "same-parent trailing boundary is a no-op"
    );
    let mut moved = Editor::new(project.clone(), 8).unwrap();
    moved
        .execute(Command::MoveTracksBefore {
            tracks: TrackSelectionIds::new(&[inner]).unwrap(),
            before: Some(unrelated),
        })
        .unwrap();
    assert_eq!(moved.project().track(inner).unwrap().parent_group(), None);
    assert_eq!(
        moved.project().track(inside).unwrap().parent_group(),
        Some(inner)
    );
    assert!(moved.undo());
    assert_eq!(moved.project(), project);

    let sparse = subtree::SourceRows::capture(
        project,
        &TrackSelectionIds::new(&[inside, outside]).unwrap(),
    )
    .unwrap();
    assert!(
        sparse.permits(index(unrelated), None),
        "a gap between selected subtrees remains a real move"
    );
    let ranges = [(48_000, 72_000)];
    let visible = visibility::visible_tracks(project, &ranges, None, None);
    assert_eq!(visible, BTreeSet::from([outer, inner, inside]));
    let ordered: Vec<_> = project
        .tracks()
        .iter()
        .filter(|track| visible.contains(&track.id()))
        .map(|track| track.id())
        .collect();
    assert_eq!(ordered, [outer, inner, inside]);
    assert_eq!(
        visibility::visible_tracks(project, &ranges, Some(outside), None),
        BTreeSet::from([outer, inner, inside, unrelated, outside])
    );
    assert!(
        visibility::visible_tracks(project, &[], None, None).is_empty(),
        "empty ranges use the caller's unfiltered path"
    );
    let mut projection = visibility::Projection::default();
    projection.rebuild(1, 1, project, &[], None);
    let frame_revision = projection.fold_revision();
    projection.toggle_fold(inner);
    assert_ne!(
        projection.fold_revision(),
        frame_revision,
        "fold invalidates captured geometry"
    );
    assert!(projection.rebuild(1, 1, project, &[], None));
    assert_eq!(
        projection.tracks,
        BTreeSet::from([outer, inner, unrelated, outside])
    );
    assert!(!projection.tracks.contains(&inside));
    projection.toggle_fold(outer);
    projection.rebuild(1, 1, project, &[], None);
    assert_eq!(
        projection.tracks,
        BTreeSet::from([outer, unrelated, outside])
    );
    projection.toggle_fold(outer);
    projection.rebuild(1, 1, project, &[], None);
    assert!(projection.folded(inner));
    assert!(
        !projection.tracks.contains(&inside),
        "inner fold survives outer expansion"
    );
    projection.toggle_fold(inner);
    projection.rebuild(1, 1, project, &[], None);
    assert!(!projection.filtering());
    assert_eq!(projection.indices.len(), project.tracks().len());
    projection.enabled = true;
    projection.toggle_fold(outer);
    projection.rebuild(1, 1, project, &[section], None);
    assert_eq!(projection.tracks, BTreeSet::from([outer]));
    let console_tracks: Vec<_> = projection
        .console_indices
        .iter()
        .map(|index| project.tracks()[*index].id())
        .collect();
    assert_eq!(
        console_tracks,
        [outer, unrelated, outside],
        "section focus does not filter Console columns"
    );
    projection.toggle_fold(outer);
    projection.rebuild(1, 1, project, &[section], None);
    assert_eq!(projection.tracks, BTreeSet::from([outer, inner, inside]));
    projection.toggle_fold(inner);
    editor.execute(Command::UngroupTrack(inner)).unwrap();
    projection.rebuild(1, 2, editor.project(), &[section], None);
    assert!(!projection.folded(inner));
    assert!(projection.tracks.contains(&inside));
    projection.toggle_fold(outer);
    projection.rebuild(1, 2, editor.project(), &[section], None);
    assert!(projection.folded(outer));
    projection.rebuild(2, 2, editor.project(), &[section], None);
    assert!(!projection.folded(outer));
    assert!(!projection.enabled);

    indent.reconcile(1, 2, editor.project());
    assert_eq!(indent.pixels(inside), 4.0);
    assert_eq!(indent.pixels(outside), 4.0);
    editor.execute(Command::UngroupTrack(outer)).unwrap();
    // A replacement document invalidates even if revisions happen to match.
    indent.reconcile(2, 2, editor.project());
    assert_eq!(indent.pixels(inside), 0.0);
}

#[path = "../src/editor/arrangement_focus/fixture.rs"]
mod fixture;
use fixture::model;
use visibility::Projection as State;

#[test]
fn projection_uses_cached_section_union_and_half_open_boundaries() {
    let f = model();
    let mut state = State::default();
    assert!(state.rebuild(1, 1, f.model.project(), &f.sections[..1], None));
    assert!(!state.generation_changed(1));
    assert!(state.generation_changed(2));
    state.enabled = true;
    assert!(state.rebuild(1, 1, f.model.project(), &f.sections[..1], None));
    assert_eq!(state.indices, [1, 3]);
    assert_eq!(
        state.clips,
        BTreeSet::from([f.clips[0], f.clips[2], f.clips[4], f.clips[5]])
    );
    assert!(
        !state.rebuild(1, 1, f.model.project(), &f.sections[..1], None),
        "unchanged paints must not scan the project again"
    );
    assert!(!state.rebuild(1, 1, f.model.project(), &f.sections[..1], None));
    assert!(state.rebuild(
        1,
        1,
        f.model.project(),
        &[f.sections[0], f.sections[2]],
        None
    ));
    assert_eq!(state.indices, [1, 3]);
    assert_eq!(
        state.clips,
        BTreeSet::from([f.clips[0], f.clips[2], f.clips[4], f.clips[5]])
    );
    assert!(!state.clips.contains(&f.clips[3]), "gap is excluded");
    assert!(state.rebuild(1, 1, f.model.project(), &f.sections[1..2], None));
    assert!(
        state.indices.is_empty(),
        "audio ending at the section start is excluded"
    );
    assert!(state.rebuild(1, 1, f.model.project(), &[], None));
    assert!(state.enabled, "no selection leaves the mode armed");
    assert_eq!(state.indices, [0, 1, 2, 3, 4]);
    assert!(state.rebuild(2, 1, f.model.project(), &f.sections[..1], None));
    assert!(
        !state.enabled,
        "replacement document must start in the normal view"
    );
    assert_eq!(state.indices, [0, 1, 2, 3, 4]);
}
#[test]
fn new_track_exception_expires_without_repinning_on_history_or_focus_changes() {
    let mut f = model();
    let mut state = State::default();
    state.enabled = true;
    state.rebuild(1, 1, f.model.project(), &f.sections[..1], None);
    let Created::Track(first) = f
        .model
        .execute(Command::AddTrack {
            name: "first new".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    state.reveal_created_track(first);
    state.rebuild(1, 2, f.model.project(), &f.sections[..1], Some(first));
    assert!(state.tracks.contains(&first));
    assert!(!state.rebuild(1, 2, f.model.project(), &f.sections[..1], Some(first)));
    // Losing the active track invalidates the cached rows at the same revision.
    assert!(state.rebuild(1, 2, f.model.project(), &f.sections[..1], Some(f.tracks[1])));
    assert!(!state.tracks.contains(&first));
    state.rebuild(1, 2, f.model.project(), &f.sections[..1], Some(first));
    assert!(
        !state.tracks.contains(&first),
        "selection cannot repin an empty track"
    );

    state.reveal_created_track(first);
    let Created::Track(second) = f
        .model
        .execute(Command::AddTrack {
            name: "second new".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    state.reveal_created_track(second);
    state.rebuild(1, 3, f.model.project(), &f.sections[..1], Some(second));
    assert!(state.tracks.contains(&second));
    assert!(!state.tracks.contains(&first));
    assert!(f.model.undo());
    state.rebuild(1, 4, f.model.project(), &f.sections[..1], Some(second));
    assert!(f.model.redo());
    state.rebuild(1, 5, f.model.project(), &f.sections[..1], Some(second));
    assert!(
        !state.tracks.contains(&second),
        "redo does not revive stale visibility"
    );

    state.reveal_created_track(second);
    state.rebuild(1, 6, f.model.project(), &f.sections[1..2], Some(second));
    assert!(!state.tracks.contains(&second));
    state.rebuild(1, 6, f.model.project(), &f.sections[..1], Some(second));
    assert!(!state.tracks.contains(&second));
    state.reveal_created_track(second);
    f.model
        .execute(Command::SetArrangementSectionRange {
            section: f.sections[0],
            start: BeatTime::ZERO,
            end: BeatTime::ONE,
        })
        .unwrap();
    state.rebuild(1, 7, f.model.project(), &f.sections[..1], Some(second));
    assert!(
        !state.tracks.contains(&second),
        "editing the focused range retires the exception"
    );
    state.reveal_created_track(second);
    state.enabled = false;
    state.rebuild(1, 7, f.model.project(), &f.sections[..1], Some(second));
    state.enabled = true;
    state.rebuild(1, 7, f.model.project(), &f.sections[..1], Some(second));
    assert!(!state.tracks.contains(&second));
    state.reveal_created_track(second);
    state.rebuild(2, 7, f.model.project(), &f.sections[..1], Some(second));
    state.enabled = true;
    state.rebuild(2, 7, f.model.project(), &f.sections[..1], Some(second));
    assert!(
        !state.tracks.contains(&second),
        "document identities cannot retain an exception"
    );
}

#[test]
fn section_projection_follows_canonical_tempo_range_and_removal() {
    let mut f = model();
    let mut state = State::default();
    state.enabled = true;
    state.rebuild(1, 1, f.model.project(), &f.sections[..1], None);
    f.model
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: hex_project::Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    state.rebuild(1, 2, f.model.project(), &f.sections[..1], None);
    assert_eq!(state.ranges.as_ref(), &[(96_000, 144_000)]);
    assert_eq!(state.indices, [3]);
    f.model
        .execute(Command::SetArrangementSectionRange {
            section: f.sections[0],
            start: BeatTime::ZERO,
            end: BeatTime::ONE,
        })
        .unwrap();
    state.rebuild(1, 3, f.model.project(), &f.sections[..1], None);
    assert_eq!(state.indices, [1, 2]);
    f.model
        .execute(Command::RemoveArrangementSection(f.sections[0]))
        .unwrap();
    state.rebuild(1, 4, f.model.project(), &f.sections[..1], None);
    assert!(state.enabled);
    assert_eq!(state.indices, [0, 1, 2, 3, 4]);
}

#[test]
fn tag_and_section_filters_intersect_only_in_arrangement_and_never_change_project() {
    let mut f = model();
    let tag = hex_project::TrackTag::new("drums").unwrap();
    for track in &f.tracks[..2] {
        f.model
            .execute(Command::SetTrackTags {
                track: *track,
                tags: vec![tag.clone()],
            })
            .unwrap();
    }
    let before = f.model.project().snapshot();
    let mut projection = State::default();
    projection.enabled = true;
    projection.set_tag_filter(Some(tag));
    projection.rebuild(1, 1, f.model.project(), &f.sections[..1], None);
    assert_eq!(projection.indices, [1]);
    assert_eq!(projection.console_indices, [0, 1]);
    assert_eq!(f.model.project().snapshot(), before);
    f.model
        .execute(Command::SetTrackTags {
            track: f.tracks[1],
            tags: Vec::new(),
        })
        .unwrap();
    projection.rebuild(1, 2, f.model.project(), &f.sections[..1], None);
    assert!(projection.indices.is_empty());
    assert_eq!(projection.console_indices, [0]);
}

#[test]
fn gap_queries_preserve_group_closure_and_ignore_ordinary_fold_or_tag_filtering() {
    let mut f = model();
    let mut child = f.tracks[3];
    let mut groups = Vec::new();
    for _ in 0..2 {
        let Created::Track(group) = f
            .model
            .execute(Command::GroupTracks {
                tracks: hex_project::TrackSelectionIds::new(&[child]).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group")
        };
        groups.push(group);
        child = group;
    }
    let project = f.model.project();
    let index = |id| {
        project
            .tracks()
            .iter()
            .position(|track| track.id() == id)
            .unwrap()
    };
    let mut state = State::default();
    state.enabled = true;
    state.rebuild(1, 1, project, &[f.sections[0]], None);
    let p = &mut state;
    assert_eq!(
        p.indices
            .iter()
            .map(|index| project.tracks()[*index].id())
            .collect::<Vec<_>>(),
        [f.tracks[1], groups[1], groups[0], f.tracks[3]]
    );
    assert!(p.hidden_before(index(f.tracks[1])));
    assert!(p.hidden_before(index(groups[1])));
    assert!(!p.hidden_before(index(groups[0])));
    assert!(!p.hidden_before(index(f.tracks[3])));
    assert!(p.hidden_after(index(f.tracks[3])));
    p.toggle_fold(groups[1]);
    p.rebuild(1, 1, project, &[f.sections[0]], None);
    assert_eq!(
        p.indices
            .iter()
            .map(|index| project.tracks()[*index].id())
            .collect::<Vec<_>>(),
        [f.tracks[1], groups[1]]
    );
    assert!(p.hidden_after(index(groups[1])));
    assert!(
        !p.hidden_before(index(groups[0])),
        "a hidden child has no rendered edge"
    );
    p.enabled = false;
    p.rebuild(1, 1, project, &[f.sections[0]], None);
    for i in 0..project.tracks().len() {
        assert!(!p.hidden_before(i) && !p.hidden_after(i));
    }
    p.set_tag_filter(Some(hex_project::TrackTag::new("no matches").unwrap()));
    p.rebuild(1, 1, project, &[], None);
    assert!(p.indices.is_empty());
    assert!(
        !p.all_tracks_hidden(),
        "tag-only hiding has no section-focus cue"
    );
    p.enabled = true;
    p.rebuild(1, 1, project, &[], None);
    assert!(
        !p.all_tracks_hidden(),
        "armed focus without a section has no cue"
    );
}

#[test]
fn gap_queries_deduplicate_adjacent_rows_and_handle_all_hidden_or_all_visible() {
    let mut f = model();
    f.model
        .execute(Command::PlaceMidiClip {
            track: f.tracks[2],
            start: hex_project::BeatTime::from_quarters(2).unwrap(),
            length: hex_project::BeatTime::ONE,
        })
        .unwrap();
    let mut state = State::default();
    state.enabled = true;
    state.rebuild(1, 1, f.model.project(), &[f.sections[0]], None);
    assert_eq!(state.indices, [1, 2, 3]);
    assert!(state.hidden_before(1));
    assert!(!state.hidden_before(2) && !state.hidden_before(3));
    assert!(!state.hidden_after(1) && !state.hidden_after(2));
    assert!(state.hidden_after(3));
    for track in [f.tracks[0], f.tracks[4]] {
        f.model
            .execute(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::from_quarters(2).unwrap(),
                length: hex_project::BeatTime::ONE,
            })
            .unwrap();
    }
    state.rebuild(1, 2, f.model.project(), &[f.sections[0]], None);
    assert_eq!(state.indices, [0, 1, 2, 3, 4]);
    for i in 0..5 {
        assert!(!state.hidden_before(i) && !state.hidden_after(i));
    }
    assert!(!state.all_tracks_hidden());
    state.rebuild(1, 2, f.model.project(), &[f.sections[1]], None);
    assert!(state.indices.is_empty());
    assert!(state.all_tracks_hidden());
    assert!(!state.hidden_before(usize::MAX) && !state.hidden_after(usize::MAX));
}

#[cfg(test)]
mod source_tag_projection_tests {
    use super::visibility::*;
    use hex_project::{Command, Created, Editor, Project, TrackTag};
    #[test]
    fn tag_filter_is_view_only_cached_clearable_and_generation_bound() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let mut tracks = Vec::new();
        for name in ["kick", "hat", "bass"] {
            let Created::Track(track) = editor
                .execute(Command::AddTrack { name: name.into() })
                .unwrap()
            else {
                panic!("track")
            };
            tracks.push(track);
        }
        let drums = TrackTag::new("drums").unwrap();
        for track in &tracks[..2] {
            editor
                .execute(Command::SetTrackTags {
                    track: *track,
                    tags: vec![drums.clone()],
                })
                .unwrap();
        }
        let canonical = editor.project().clone();
        let mut projection = Projection::default();
        projection.rebuild(1, 1, &canonical, &[], None);
        projection.set_tag_filter(Some(drums));
        assert!(projection.rebuild(1, 1, &canonical, &[], None));
        assert_eq!(projection.indices, vec![0, 1]);
        assert_eq!(projection.console_indices, vec![0, 1]);
        let ptr = projection.indices.as_ptr();
        for _ in 0..1000 {
            assert!(!projection.rebuild(1, 1, &canonical, &[], None));
        }
        assert_eq!(projection.indices.as_ptr(), ptr);
        projection.set_tag_filter(Some(TrackTag::new("no match").unwrap()));
        projection.rebuild(1, 1, &canonical, &[], None);
        assert!(projection.indices.is_empty());
        assert!(projection.console_indices.is_empty());
        projection.set_tag_filter(None);
        projection.rebuild(1, 1, &canonical, &[], None);
        assert_eq!(projection.indices, vec![0, 1, 2]);
        projection.set_tag_filter(Some(TrackTag::new("drums").unwrap()));
        projection.rebuild(2, 1, &canonical, &[], None);
        assert!(projection.tag_filter().is_none());
        assert_eq!(projection.indices, vec![0, 1, 2]);
        assert_eq!(editor.project(), &canonical);
    }
    #[test]
    fn hundred_track_filter_and_cached_repaint_cost_remain_separate() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let tag = TrackTag::new("drums").unwrap();
        for index in 0..100 {
            let Created::Track(track) = editor
                .execute(Command::AddTrack {
                    name: format!("track {index}"),
                })
                .unwrap()
            else {
                panic!("track")
            };
            if index % 2 == 0 {
                editor
                    .execute(Command::SetTrackTags {
                        track,
                        tags: vec![tag.clone()],
                    })
                    .unwrap();
            }
        }
        let project = editor.project();
        let mut projection = Projection::default();
        projection.rebuild(1, 1, project, &[], None);
        let before = std::time::Instant::now();
        for _ in 0..10000 {
            assert!(!std::hint::black_box(&mut projection).rebuild(1, 1, project, &[], None));
        }
        let unfiltered = before.elapsed();
        projection.set_tag_filter(Some(tag));
        let before = std::time::Instant::now();
        projection.rebuild(1, 1, project, &[], None);
        let rebuild = before.elapsed();
        assert_eq!(projection.indices.len(), 50);
        let before = std::time::Instant::now();
        for _ in 0..10000 {
            assert!(!std::hint::black_box(&mut projection).rebuild(1, 1, project, &[], None));
        }
        eprintln!(
            "tag filter100tracks unoptimized: rebuild {rebuild:?}; cached10000 unfiltered {unfiltered:?}, filtered {:?}",
            before.elapsed()
        );
    }
}

#[cfg(test)]
mod source_hierarchy_indent_tests {
    use super::track_hierarchy_indent::*;
    use hex_project::{Command, Created, Editor, ElementColor, Project, TrackSelectionIds};

    #[test]
    fn nested_ancestors_follow_canonical_order_and_refresh_colours() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let mut ids = Vec::new();
        for name in ["a", "b", "c"] {
            let Created::Track(id) = editor
                .execute(Command::AddTrack { name: name.into() })
                .unwrap()
            else {
                panic!("track")
            };
            ids.push(id);
        }
        let Created::Track(inner) = editor
            .execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&ids[..2]).unwrap(),
                name: "inner".into(),
            })
            .unwrap()
        else {
            panic!("inner group")
        };
        let Created::Track(outer) = editor
            .execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&[inner, ids[2]]).unwrap(),
                name: "outer".into(),
            })
            .unwrap()
        else {
            panic!("outer group")
        };
        editor
            .execute(Command::SetTrackColor {
                track: outer,
                color: ElementColor::Red,
            })
            .unwrap();
        editor
            .execute(Command::SetTrackColor {
                track: inner,
                color: ElementColor::Blue,
            })
            .unwrap();
        let mut projection = HierarchyIndent::default();
        projection.reconcile(1, 1, editor.project());
        assert!(projection.ancestors(outer).is_empty());
        assert_eq!(projection.ancestors(inner), &[(outer, ElementColor::Red)]);
        assert_eq!(
            projection.ancestors(ids[0]),
            &[(outer, ElementColor::Red), (inner, ElementColor::Blue)]
        );
        assert_eq!(projection.pixels(ids[0]), 8.0);
        editor
            .execute(Command::SetTrackColor {
                track: outer,
                color: ElementColor::Green,
            })
            .unwrap();
        projection.reconcile(1, 2, editor.project());
        assert_eq!(
            projection.ancestors(ids[0])[0],
            (outer, ElementColor::Green)
        );
    }
}
