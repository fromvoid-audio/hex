use hex_project::{Command, Editor, LoopRegion, Project};

#[test]
fn set_toggle_clear_and_history_are_atomic() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    editor
        .execute(Command::SetLoopRegion {
            start: 100,
            end: 101,
            enabled: true,
        })
        .unwrap();
    assert_eq!(
        editor.project().loop_region(),
        Some(LoopRegion::new(100, 101, true).unwrap())
    );
    assert!(editor.undo());
    assert_eq!(editor.project().loop_region(), None);
    assert!(editor.redo());

    editor.execute(Command::ToggleLoopRegion).unwrap();
    assert!(!editor.project().loop_region().unwrap().enabled());
    assert!(editor.undo());
    assert!(editor.project().loop_region().unwrap().enabled());

    editor.execute(Command::ClearLoopRegion).unwrap();
    assert_eq!(editor.project().loop_region(), None);
    assert!(editor.undo());
    assert_eq!(editor.project().loop_region().unwrap().end(), 101);
}

#[test]
fn invalid_region_and_missing_toggle_preserve_redo_state() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    assert!(editor.execute(Command::ToggleLoopRegion).is_err());
    assert!(!editor.can_undo());
    editor
        .execute(Command::SetLoopRegion {
            start: 2,
            end: 3,
            enabled: false,
        })
        .unwrap();
    editor.undo();
    let before = editor.project().clone();
    for (start, end) in [(5, 5), (6, 5)] {
        assert!(
            editor
                .execute(Command::SetLoopRegion {
                    start,
                    end,
                    enabled: true,
                })
                .is_err()
        );
        assert_eq!(editor.project(), &before);
        assert!(editor.can_redo());
    }
    assert!(editor.redo());
    assert_eq!(editor.project().loop_region().unwrap().start(), 2);
}

#[test]
fn loop_region_accepts_maximum_exclusive_end() {
    let region = LoopRegion::new(u64::MAX - 1, u64::MAX, true).unwrap();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    editor
        .execute(Command::SetLoopRegion {
            start: region.start(),
            end: region.end(),
            enabled: region.enabled(),
        })
        .unwrap();
    assert_eq!(editor.project().loop_region(), Some(region));
}
