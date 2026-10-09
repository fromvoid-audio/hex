use hex_project::{Command, Created, Editor, Project};

#[test]
fn insert_hybrid_track_preserves_order_and_is_one_undo_step() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 16).expect("editor");
    let Created::Track(first) = editor
        .execute(Command::AddTrack {
            name: "first".into(),
        })
        .expect("first")
    else {
        panic!("track");
    };
    let Created::Track(second) = editor
        .execute(Command::AddTrack {
            name: "second".into(),
        })
        .expect("second")
    else {
        panic!("track");
    };
    let Created::Track(inserted) = editor
        .execute(Command::InsertTrack {
            name: "middle".into(),
            before_track: Some(second),
        })
        .expect("insert")
    else {
        panic!("track");
    };
    assert_eq!(
        editor
            .project()
            .tracks()
            .iter()
            .map(|track| track.id())
            .collect::<Vec<_>>(),
        vec![first, inserted, second]
    );
    let track = editor.project().track(inserted).expect("new hybrid track");
    assert!(track.clips().is_empty());
    assert!(track.midi_clips().is_empty());
    assert!(editor.undo());
    assert!(editor.project().track(inserted).is_none());
    assert!(editor.redo());
    assert_eq!(editor.project().tracks()[1].id(), inserted);
    editor
        .execute(Command::RemoveTrack(second))
        .expect("remove anchor");
    let snapshot = editor.project().clone();
    assert!(
        editor
            .execute(Command::InsertTrack {
                name: "stale".into(),
                before_track: Some(second)
            })
            .is_err()
    );
    assert_eq!(editor.project(), &snapshot);
    let Created::Track(appended) = editor
        .execute(Command::InsertTrack {
            name: "last".into(),
            before_track: None,
        })
        .expect("append")
    else {
        panic!("track");
    };
    assert_eq!(
        editor.project().tracks().last().map(|track| track.id()),
        Some(appended)
    );
}
