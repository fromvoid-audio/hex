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

#[test]
fn inserted_default_track_accepts_initial_gain_atomically_and_undoes_once() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(anchor) = editor
        .execute(Command::InsertDefaultTrack { before_track: None })
        .unwrap()
    else {
        panic!("track")
    };
    let original = editor.project().clone();
    for gain in [f32::NAN, f32::INFINITY, -0.01, 2.01] {
        assert!(
            editor
                .execute(Command::InsertDefaultTrackWithGain {
                    before_track: Some(anchor),
                    gain
                })
                .is_err()
        );
        assert_eq!(editor.project(), &original);
    }
    let Created::Track(inserted) = editor
        .execute(Command::InsertDefaultTrackWithGain {
            before_track: Some(anchor),
            gain: 0.251_188_64,
        })
        .unwrap()
    else {
        panic!("track")
    };
    let track = editor.project().track(inserted).unwrap();
    assert_eq!(track.gain(), 0.251_188_64);
    assert_eq!(track.name(), "track 2");
    assert!(track.name_is_automatic());
    assert_eq!(editor.project().tracks()[0].id(), inserted);
    assert_eq!(editor.project().track(anchor).unwrap().gain(), 1.0);
    let after = editor.project().snapshot();
    assert!(editor.undo());
    let mut expected = original.snapshot();
    expected.next_id = editor.project().snapshot().next_id;
    assert_eq!(editor.project().snapshot(), expected);
    assert!(
        editor
            .execute(Command::InsertDefaultTrackWithGain {
                before_track: Some(inserted),
                gain: 0.25,
            })
            .is_err(),
        "stale insertion anchor must refuse"
    );
    assert!(
        editor
            .execute(Command::InsertDefaultTrackWithGain {
                before_track: None,
                gain: f32::NAN,
            })
            .is_err(),
        "invalid gain must not clear redo"
    );
    assert_eq!(editor.project().snapshot(), expected);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}
