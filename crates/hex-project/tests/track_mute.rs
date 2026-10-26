use hex_project::{Command, Created, Editor, Project, TrackId, TrackSelectionIds};

fn fixture() -> (Editor, [TrackId; 3]) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let tracks = ["first", "untouched", "last"].map(|name| {
        let Created::Track(track) = editor
            .execute(Command::AddTrack { name: name.into() })
            .unwrap()
        else {
            panic!("track")
        };
        track
    });
    (Editor::new(editor.project().clone(), 8).unwrap(), tracks)
}

#[test]
fn selected_mute_unifies_mixed_states_and_restores_each_in_one_undo() {
    let (mut editor, tracks) = fixture();
    editor
        .execute(Command::Mute {
            track: tracks[2],
            muted: true,
        })
        .unwrap();
    let before = editor.project().snapshot();
    let selected = TrackSelectionIds::new(&[tracks[2], tracks[0]]).unwrap();
    editor
        .execute(Command::ToggleTracksMute(selected.clone()))
        .unwrap();
    let muted = editor.project().snapshot();
    assert!(editor.project().track(tracks[0]).unwrap().muted());
    assert!(!editor.project().track(tracks[1]).unwrap().muted());
    assert!(editor.project().track(tracks[2]).unwrap().muted());
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), muted);
    editor.execute(Command::ToggleTracksMute(selected)).unwrap();
    assert!(editor.project().tracks().iter().all(|t| !t.muted()));
}

#[test]
fn stale_member_rejects_the_whole_mute_and_preserves_redo() {
    let (mut editor, tracks) = fixture();
    editor.execute(Command::RemoveTrack(tracks[2])).unwrap();
    let mut editor = Editor::new(editor.project().clone(), 8).unwrap();
    editor
        .execute(Command::Mute {
            track: tracks[1],
            muted: true,
        })
        .unwrap();
    assert!(editor.undo());
    let before = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::ToggleTracksMute(
                TrackSelectionIds::new(&[tracks[0], tracks[2]]).unwrap()
            ))
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
    assert!(!editor.can_undo());
    assert!(editor.can_redo());
    assert!(editor.redo());
    assert!(editor.project().track(tracks[1]).unwrap().muted());
    assert!(TrackSelectionIds::new(&[]).is_err());
    assert!(TrackSelectionIds::new(&[tracks[0], tracks[0]]).is_err());
}
