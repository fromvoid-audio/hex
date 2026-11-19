use hex_project::{Command, Created, Editor, MAX_ARRANGEMENT_LOCATORS, Project};

#[test]
fn locator_edits_keep_identity_order_and_exact_history() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let mut before = editor.project().snapshot();
    let Created::ArrangementLocator(locator) = editor
        .execute(Command::AddArrangementLocator { frame: 24_000 })
        .unwrap()
    else {
        panic!("add locator")
    };
    let added = editor.project().snapshot();
    assert_eq!(
        editor
            .execute(Command::AddArrangementLocator { frame: 24_000 })
            .unwrap(),
        Created::None
    );
    assert_eq!(editor.project().snapshot(), added);
    assert!(editor.undo());
    // Undo never recycles allocated project identities.
    before.next_id = added.next_id;
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), added);

    editor
        .execute(Command::AddArrangementLocator { frame: 48_000 })
        .unwrap();
    let before = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::MoveArrangementLocator {
                locator,
                frame: 48_000
            })
            .is_err()
    );
    assert!(
        editor
            .execute(Command::AddArrangementLocator { frame: u64::MAX })
            .is_err()
    );
    assert!(
        editor
            .execute(Command::RenameArrangementLocator {
                locator,
                name: Some("bad\nname".into())
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);

    for command in [
        Command::MoveArrangementLocator {
            locator,
            frame: 60_000,
        },
        Command::RenameArrangementLocator {
            locator,
            name: Some("Bridge".into()),
        },
        Command::RenameArrangementLocator {
            locator,
            name: Some(" ".into()),
        },
        Command::RemoveArrangementLocator(locator),
    ] {
        let before = editor.project().snapshot();
        editor.execute(command).unwrap();
        let after = editor.project().snapshot();
        assert_ne!(after, before);
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
    }
    assert!(editor.project().arrangement_locator(locator).is_none());
}

#[test]
fn locator_count_and_rate_conversion_preserve_bounds() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    for index in 0..MAX_ARRANGEMENT_LOCATORS as u64 {
        editor
            .execute(Command::AddArrangementLocator { frame: index * 480 })
            .unwrap();
    }
    let expected = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::AddArrangementLocator { frame: 1 })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), expected);
    let converted = editor.project().at_sample_rate(96_000).unwrap();
    for (before, after) in editor
        .project()
        .arrangement_locators()
        .iter()
        .zip(converted.arrangement_locators())
    {
        assert_eq!(before.id(), after.id());
        assert_eq!(before.frame() * 2, after.frame());
    }
    let mut excess = expected;
    excess
        .arrangement_locators
        .push(excess.arrangement_locators[0].clone());
    assert!(Project::from_snapshot(excess).is_err());
}

#[test]
fn section_appearance_uses_position_and_structural_edits_move_the_landmarks() {
    use hex_project::{BeatTime, ElementColor};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let beat = |n| BeatTime::from_quarters(n).unwrap();
    let Created::ArrangementSection(verse) = editor
        .execute(Command::AddArrangementSection {
            name: "Verse".into(),
            start: beat(1),
            end: beat(3),
            color: ElementColor::Orange,
        })
        .unwrap()
    else {
        panic!("section")
    };
    let Created::ArrangementSection(other) = editor
        .execute(Command::AddArrangementSection {
            name: "Outro".into(),
            start: beat(4),
            end: beat(6),
            color: ElementColor::Blue,
        })
        .unwrap()
    else {
        panic!("section")
    };
    for frame in [12_000, 30_000, 55_000, 72_000] {
        editor
            .execute(Command::AddArrangementLocator { frame })
            .unwrap();
    }
    let named = editor.project().arrangement_locators()[1].id();
    editor
        .execute(Command::RenameArrangementLocator {
            locator: named,
            name: Some("Hit".into()),
        })
        .unwrap();
    let expected = |editor: &Editor| {
        editor
            .project()
            .arrangement_locator_appearances()
            .into_iter()
            .map(|locator| (locator.frame, locator.name, locator.color))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        expected(&editor),
        [
            (12_000, "locator 1".into(), None),
            (30_000, "Hit".into(), Some(ElementColor::Orange)),
            (55_000, "Verse.2".into(), Some(ElementColor::Orange)),
            (72_000, "locator 2".into(), None),
        ]
    );
    editor
        .execute(Command::SetArrangementSectionColor {
            section: verse,
            color: ElementColor::Blue,
        })
        .unwrap();
    assert_eq!(expected(&editor)[2].2, Some(ElementColor::Blue));
    editor
        .execute(Command::MoveArrangementSectionMarker {
            section: verse,
            start: beat(2),
        })
        .unwrap();
    assert_eq!(expected(&editor)[1], (30_000, "Hit".into(), None));
    assert_eq!(expected(&editor)[2].1, "Verse.1");
    assert!(editor.undo());

    editor
        .execute(Command::DuplicateArrangementSection(verse))
        .unwrap();
    let locators = editor.project().arrangement_locators();
    assert_eq!(
        locators.iter().map(|l| l.frame()).collect::<Vec<_>>(),
        [12_000, 30_000, 55_000, 78_000, 103_000, 120_000]
    );
    assert_ne!(locators[3].id(), named);
    assert_eq!(locators[3].name(), Some("Hit"));
    assert!(editor.undo());

    editor
        .execute(Command::ReorderSectionAfter {
            section: verse,
            after: other,
        })
        .unwrap();
    assert_eq!(
        editor.project().arrangement_locator(named).unwrap().frame(),
        102_000
    );
    assert!(editor.undo());
    editor
        .execute(Command::RemoveArrangementSection(verse))
        .unwrap();
    assert_eq!(expected(&editor)[1], (30_000, "Hit".into(), None));
    assert!(editor.undo());
    editor
        .execute(Command::DeleteArrangementSection(verse))
        .unwrap();
    assert!(editor.project().arrangement_locator(named).is_none());
}
