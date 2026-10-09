use hex_project::{Command, Created, Editor, Invalid, Project};

#[test]
fn rejected_preparation_preserves_ids_redo_and_history() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    editor
        .execute(Command::AddTrack {
            name: "first".into(),
        })
        .unwrap();
    let Created::Track(second) = editor
        .execute(Command::AddTrack {
            name: "second".into(),
        })
        .unwrap()
    else {
        panic!("track expected")
    };
    assert!(editor.undo());
    let before = editor.project().clone();
    let rejected = editor.execute_validated(
        Command::AddMidiTrack {
            name: "candidate".into(),
        },
        |candidate| {
            assert_eq!(candidate.tracks().len(), 2);
            Err(Invalid("prepared graph refused"))
        },
    );
    assert!(rejected.is_err());
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());
    assert!(editor.redo());
    assert_eq!(editor.project().tracks()[1].id(), second);
    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
    let Created::Track(next) = editor
        .execute(Command::AddTrack {
            name: "accepted".into(),
        })
        .unwrap()
    else {
        panic!("track expected")
    };
    assert_eq!(next.value(), second.value() + 2);
    assert!(editor.undo());
    assert_eq!(editor.project().tracks(), before.tracks());
    assert!(editor.undo());
    assert!(editor.project().tracks().is_empty());
}

#[test]
fn rejected_prepared_undo_and_redo_preserve_project_allocator_and_stack_order() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    for name in ["first", "second", "third"] {
        editor
            .execute(Command::AddTrack { name: name.into() })
            .unwrap();
    }
    assert!(editor.undo());
    let before_undo = editor.project().snapshot();
    let can_undo = editor.can_undo();
    let can_redo = editor.can_redo();
    let rejected = editor.undo_prepared(|candidate| {
        assert_eq!(candidate.tracks().len(), 1);
        Err::<(), _>(Invalid("compile rejected undo"))
    });
    assert_eq!(rejected, Err(Invalid("compile rejected undo")));
    assert_eq!(editor.project().snapshot(), before_undo);
    assert_eq!(editor.can_undo(), can_undo);
    assert_eq!(editor.can_redo(), can_redo);

    let candidate = editor
        .undo_prepared(|candidate| Ok::<_, Invalid>(candidate.snapshot()))
        .unwrap()
        .expect("undo candidate");
    assert_eq!(candidate.tracks.len(), 1);
    assert_eq!(editor.project().tracks().len(), 1);
    assert!(editor.can_redo());

    let before_redo = editor.project().snapshot();
    let can_undo = editor.can_undo();
    let can_redo = editor.can_redo();
    let rejected = editor.redo_prepared(|candidate| {
        assert_eq!(candidate.tracks().len(), 2);
        Err::<(), _>(Invalid("compile rejected redo"))
    });
    assert_eq!(rejected, Err(Invalid("compile rejected redo")));
    assert_eq!(editor.project().snapshot(), before_redo);
    assert_eq!(editor.can_undo(), can_undo);
    assert_eq!(editor.can_redo(), can_redo);

    let candidate = editor
        .redo_prepared(|candidate| Ok::<_, Invalid>(candidate.snapshot()))
        .unwrap()
        .expect("redo candidate");
    assert_eq!(candidate.tracks.len(), 2);
    assert_eq!(editor.project().tracks().len(), 2);
    assert!(editor.redo());
    assert_eq!(editor.project().tracks().len(), 3);
}
