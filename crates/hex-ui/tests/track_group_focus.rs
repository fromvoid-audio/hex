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
