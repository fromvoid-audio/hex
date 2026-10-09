use hex_project::{Command, Created, Editor, Project, TrackId};

fn setup() -> (Editor, [TrackId; 3]) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let tracks = std::array::from_fn(|index| {
        match editor
            .execute(Command::AddTrack {
                name: format!("track {index}"),
            })
            .unwrap()
        {
            Created::Track(id) => id,
            result => panic!("unexpected add result: {result:?}"),
        }
    });
    (editor, tracks)
}

#[test]
fn additive_and_exclusive_solo_resolve_from_canonical_state() {
    let (mut editor, [first, second, third]) = setup();
    editor
        .execute(Command::ToggleSolo {
            track: first,
            exclusive: false,
        })
        .unwrap();
    editor
        .execute(Command::ToggleSolo {
            track: second,
            exclusive: false,
        })
        .unwrap();
    assert!(editor.project().track(first).unwrap().soloed());
    assert!(editor.project().track(second).unwrap().soloed());
    assert!(!editor.project().track(third).unwrap().soloed());

    editor
        .execute(Command::ToggleSolo {
            track: third,
            exclusive: true,
        })
        .unwrap();
    assert!(!editor.project().track(first).unwrap().soloed());
    assert!(!editor.project().track(second).unwrap().soloed());
    assert!(editor.project().track(third).unwrap().soloed());

    editor
        .execute(Command::ToggleSolo {
            track: first,
            exclusive: false,
        })
        .unwrap();
    editor
        .execute(Command::ToggleSolo {
            track: third,
            exclusive: false,
        })
        .unwrap();
    assert!(editor.project().track(first).unwrap().soloed());
    assert!(!editor.project().track(third).unwrap().soloed());
    assert!(!editor.project().track(second).unwrap().soloed());
}

#[test]
fn toggle_off_preserves_other_solos_and_invalid_ids_keep_history_unchanged() {
    let (mut editor, [first, second, stale]) = setup();
    editor.execute(Command::RemoveTrack(stale)).unwrap();
    for track in [first, second] {
        editor
            .execute(Command::ToggleSolo {
                track,
                exclusive: false,
            })
            .unwrap();
    }
    editor.undo();
    let before = editor.project().clone();
    assert!(
        editor
            .execute(Command::ToggleSolo {
                track: stale,
                exclusive: true,
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());
    editor.redo();
    assert!(editor.project().track(first).unwrap().soloed());
    assert!(editor.project().track(second).unwrap().soloed());

    editor
        .execute(Command::ToggleSolo {
            track: second,
            exclusive: true,
        })
        .unwrap();
    assert!(editor.project().track(first).unwrap().soloed());
    assert!(!editor.project().track(second).unwrap().soloed());
}

#[test]
fn removing_a_soloed_track_and_undo_restore_the_exact_solo_state() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "only track".into(),
        })
        .unwrap()
    else {
        panic!("track result");
    };
    editor
        .execute(Command::ToggleSolo {
            track,
            exclusive: false,
        })
        .unwrap();
    let before = editor.project().clone();
    editor.execute(Command::RemoveTrack(track)).unwrap();
    assert!(editor.project().track(track).is_none());
    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
    assert!(editor.project().track(track).unwrap().soloed());
}
