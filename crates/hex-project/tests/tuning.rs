use hex_project::{Command, Editor, Invalid, Project};
use hex_tuning::{Audition, ProjectTuning, Scale, Waveform};

fn tuning() -> ProjectTuning {
    ProjectTuning::new(
        true,
        Scale::default(),
        Audition::new(Waveform::Saw, 0.6, 25.0, 450.0).unwrap(),
    )
    .unwrap()
}

#[test]
fn tuning_is_project_scoped_snapshotted_and_one_history_step() {
    let original = Project::new(48_000).unwrap();
    assert_eq!(original.tuning(), ProjectTuning::default());
    let mut editor = Editor::new(original.clone(), 8).unwrap();
    editor.execute(Command::SetTuning(tuning())).unwrap();
    assert_eq!(editor.project().tuning(), tuning());
    let restored = Project::from_snapshot(editor.project().snapshot()).unwrap();
    assert_eq!(restored, *editor.project());
    assert_eq!(Project::new(48_000).unwrap().tuning(), original.tuning());
    assert!(editor.undo());
    assert_eq!(*editor.project(), original);
    assert!(!editor.can_undo());
    // A neutral no-op leaves the redo branch intact.
    editor
        .execute(Command::SetTuning(original.tuning()))
        .unwrap();
    assert!(editor.can_redo());
    assert!(editor.redo());
    assert_eq!(editor.project().tuning(), tuning());
}

#[test]
fn rejected_tuning_preparation_and_atomic_batch_preserve_project_and_history() {
    let mut editor = Editor::new(Project::new(44_100).unwrap(), 8).unwrap();
    editor.execute(Command::SetTuning(tuning())).unwrap();
    assert!(editor.undo());
    let before = editor.project().clone();
    let result = editor.execute_validated(Command::SetTuning(tuning()), |candidate| {
        assert_eq!(candidate.tuning(), tuning());
        Err::<(), _>(Invalid("prepared tuning refused"))
    });
    assert!(result.is_err());
    assert_eq!(*editor.project(), before);
    assert!(editor.can_redo());
    assert!(!editor.can_undo());
    assert!(
        editor
            .execute_many_prepared(
                vec![
                    Command::SetTuning(tuning()),
                    Command::AddTrack {
                        name: String::new()
                    },
                ],
                |_, _| Ok::<_, Invalid>(())
            )
            .is_err()
    );
    assert_eq!(*editor.project(), before);
    assert!(editor.redo());
    assert_eq!(editor.project().tuning(), tuning());
}
