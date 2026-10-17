use hex_project::{Command, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn unimplemented_tuned_playback_refuses_before_history_commits() {
    let project = Project::new(48_000).unwrap();
    let mut editor = Editor::new(project.clone(), 8).unwrap();
    let tuning = project.tuning().with_enabled(true);
    let result = editor.execute_validated(Command::SetTuning(tuning), |candidate| {
        hex_engine::compile::project(candidate, &BTreeMap::new())
            .map(|_| ())
            .map_err(|error| hex_project::Invalid(error.0))
    });
    assert_eq!(
        result.unwrap_err().0,
        "project tuning playback is not implemented"
    );
    assert_eq!(*editor.project(), project);
    assert!(!editor.can_undo());
    // The lower-level Session preparation API must not silently bypass refusal.
    editor.execute(Command::SetTuning(tuning)).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    assert_eq!(
        hex_engine::PreparedSessionInventory::prepare(editor.project(), table, None)
            .unwrap_err()
            .0,
        "project tuning playback is not implemented"
    );
    assert!(hex_engine::compile::project(&project, &BTreeMap::new()).is_ok());
}
