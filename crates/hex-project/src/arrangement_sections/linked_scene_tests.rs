//! Focused boundary tests for canonical linked Arrangement/Console scenes (#3442).

use crate::{BeatTime, Command, Created, Editor, ElementColor, Project, SceneId, SectionId};

fn b(quarters: u64) -> BeatTime {
    BeatTime::from_quarters(quarters).unwrap()
}

#[test]
fn place_repeat_rename_split_unlink_and_delete_follow_linked_scene_contract_with_one_step_undo() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Scene(chorus) = editor
        .execute(Command::AddScene {
            name: "chorus".into(),
        })
        .unwrap()
    else {
        panic!("expected scene");
    };
    // Empty scene defaults to 4 bars (16 quarters).
    assert_eq!(
        editor.project().default_scene_placement_length(chorus),
        b(16)
    );

    // Place chorus twice on the Arrangement Main track (repeated placements).
    let Created::ArrangementSection(first) = editor
        .execute(Command::PlaceLinkedScene {
            scene: chorus,
            start: b(0),
            end: b(8),
            color: ElementColor::Blue,
        })
        .unwrap()
    else {
        panic!("first placement");
    };
    let Created::ArrangementSection(second) = editor
        .execute(Command::PlaceLinkedScene {
            scene: chorus,
            start: b(16),
            end: b(24),
            color: ElementColor::Rose,
        })
        .unwrap()
    else {
        panic!("second placement");
    };
    assert_eq!(
        editor.project().arrangement_section(first).unwrap().name(),
        "chorus"
    );
    assert_eq!(
        editor
            .project()
            .arrangement_section(first)
            .unwrap()
            .linked_scene(),
        Some(chorus)
    );
    assert_eq!(
        editor.project().linked_sections_for_scene(chorus).count(),
        2
    );

    // Renaming the Console scene propagates to all linked Arrangement placements in one Undo step.
    editor
        .execute(Command::RenameScene {
            scene: chorus,
            name: "hook".into(),
        })
        .unwrap();
    assert_eq!(
        editor.project().arrangement_section(first).unwrap().name(),
        "hook"
    );
    assert_eq!(
        editor.project().arrangement_section(second).unwrap().name(),
        "hook"
    );
    assert!(editor.undo());
    assert_eq!(
        editor.project().arrangement_section(first).unwrap().name(),
        "chorus"
    );
    assert_eq!(
        editor.project().arrangement_section(second).unwrap().name(),
        "chorus"
    );

    // Renaming one linked Arrangement placement propagates to the Console scene and sibling placement.
    editor
        .execute(Command::RenameArrangementSection {
            section: first,
            name: "refrain".into(),
        })
        .unwrap();
    assert_eq!(editor.project().scenes()[0].name(), "refrain");
    assert_eq!(
        editor.project().arrangement_section(second).unwrap().name(),
        "refrain"
    );
    assert!(editor.undo());
    assert_eq!(editor.project().scenes()[0].name(), "chorus");
    assert_eq!(
        editor.project().arrangement_section(second).unwrap().name(),
        "chorus"
    );
    assert!(editor.redo());
    assert_eq!(editor.project().scenes()[0].name(), "refrain");

    // Splitting a linked placement retains the link on both left and right halves.
    let Created::ArrangementSection(split_right) = editor
        .execute(Command::SplitArrangementSection {
            section: first,
            at: b(4),
        })
        .unwrap()
    else {
        panic!("split right");
    };
    assert_eq!(
        editor
            .project()
            .arrangement_section(first)
            .unwrap()
            .linked_scene(),
        Some(chorus)
    );
    assert_eq!(
        editor
            .project()
            .arrangement_section(split_right)
            .unwrap()
            .linked_scene(),
        Some(chorus)
    );
    assert_eq!(
        editor.project().linked_sections_for_scene(chorus).count(),
        3
    );

    // Explicit unlink clears only that placement's link and undoes cleanly.
    editor
        .execute(Command::UnlinkSectionScene(split_right))
        .unwrap();
    assert_eq!(
        editor
            .project()
            .arrangement_section(split_right)
            .unwrap()
            .linked_scene(),
        None
    );
    assert!(
        editor
            .execute(Command::UnlinkSectionScene(split_right))
            .is_err()
    );
    assert!(editor.undo());
    assert_eq!(
        editor
            .project()
            .arrangement_section(split_right)
            .unwrap()
            .linked_scene(),
        Some(chorus)
    );

    // Deleting one linked placement leaves the Console scene and sibling placements intact.
    editor
        .execute(Command::RemoveArrangementSection(split_right))
        .unwrap();
    assert_eq!(editor.project().scenes().len(), 1);
    assert_eq!(
        editor.project().linked_sections_for_scene(chorus).count(),
        2
    );

    // Deleting the Console scene unlinks surviving Arrangement placements without deleting them.
    editor.execute(Command::RemoveScene(chorus)).unwrap();
    assert!(editor.project().scenes().is_empty());
    assert_eq!(editor.project().arrangement_sections().len(), 2);
    assert_eq!(
        editor
            .project()
            .arrangement_section(first)
            .unwrap()
            .linked_scene(),
        None
    );
    assert_eq!(
        editor
            .project()
            .arrangement_section(second)
            .unwrap()
            .linked_scene(),
        None
    );
    // One Undo step restores the Console scene and every linked placement.
    assert!(editor.undo());
    assert_eq!(editor.project().scenes().len(), 1);
    assert_eq!(
        editor
            .project()
            .arrangement_section(first)
            .unwrap()
            .linked_scene(),
        Some(chorus)
    );
    assert_eq!(
        editor
            .project()
            .arrangement_section(second)
            .unwrap()
            .linked_scene(),
        Some(chorus)
    );
}

#[test]
fn linked_scene_validation_refuses_missing_scene_overlap_and_dangling_snapshot() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "verse".into(),
        })
        .unwrap()
    else {
        panic!("scene");
    };
    let before = editor.project().snapshot();

    // Missing scene ID is refused without mutating history.
    assert!(
        editor
            .execute(Command::PlaceLinkedScene {
                scene: SceneId(9_999),
                start: b(0),
                end: b(8),
                color: ElementColor::Blue,
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);

    // Valid placement succeeds, overlapping placement is refused.
    editor
        .execute(Command::PlaceLinkedScene {
            scene,
            start: b(0),
            end: b(8),
            color: ElementColor::Blue,
        })
        .unwrap();
    assert!(
        editor
            .execute(Command::PlaceLinkedScene {
                scene,
                start: b(4),
                end: b(12),
                color: ElementColor::Rose,
            })
            .is_err()
    );
    assert!(
        editor
            .execute(Command::UnlinkSectionScene(SectionId(9_999)))
            .is_err()
    );

    // Dangling linked_scene in a snapshot is rejected by Project::from_snapshot.
    let mut corrupt = editor.project().snapshot();
    corrupt.arrangement_sections[0].linked_scene = Some(9_999);
    assert!(Project::from_snapshot(corrupt).is_err());

    let mut zero_link = editor.project().snapshot();
    zero_link.arrangement_sections[0].linked_scene = Some(0);
    assert!(Project::from_snapshot(zero_link).is_err());
}
