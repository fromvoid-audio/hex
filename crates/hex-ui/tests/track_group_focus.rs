//! Focused visibility contract using the same private projection as the UI.
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
    let project = editor.project();
    let ranges = [(48_000, 72_000)];
    let visible = visibility::visible_tracks(project, &ranges, None);
    assert_eq!(visible, BTreeSet::from([outer, inner, inside]));
    let ordered: Vec<_> = project
        .tracks()
        .iter()
        .filter(|track| visible.contains(&track.id()))
        .map(|track| track.id())
        .collect();
    assert_eq!(ordered, [outer, inner, inside]);
    assert_eq!(
        visibility::visible_tracks(project, &ranges, Some(outside)),
        BTreeSet::from([outer, inner, inside, unrelated, outside])
    );
    assert!(
        visibility::visible_tracks(project, &[], None).is_empty(),
        "empty ranges use the caller's unfiltered path"
    );
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
