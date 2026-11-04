use hex_project::{BeatTime, Command, Created, Editor, ElementColor, Project};

fn section(editor: &mut Editor, color: ElementColor, start: u64) -> hex_project::SectionId {
    let Created::ArrangementSection(id) = editor
        .execute(Command::AddArrangementSection {
            name: "part".into(),
            color,
            start: BeatTime::from_quarters(start).unwrap(),
            end: BeatTime::from_quarters(start + 1).unwrap(),
        })
        .unwrap()
    else {
        panic!("section identity")
    };
    id
}

#[test]
fn duplicates_use_unused_colors_in_preview_history_and_both_entry_points() {
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 32).unwrap();
    let source = section(&mut editor, ElementColor::Cyan, 0);
    let target = section(&mut editor, ElementColor::Red, 2);
    for command in [
        Command::DuplicateArrangementSection(source),
        Command::DuplicateArrangementSectionAt {
            section: source,
            target,
            after: true,
        },
    ] {
        let before = editor.project().clone();
        let preview = before.preview_arrangement_section_edit(&command).unwrap();
        let Created::ArrangementSection(copy) = editor.execute(command).unwrap() else {
            panic!("copy identity")
        };
        let color = editor.project().arrangement_section(copy).unwrap().color();
        assert!(
            !before
                .arrangement_sections()
                .iter()
                .any(|s| s.color() == color)
        );
        assert_eq!(
            preview
                .sections()
                .iter()
                .find(|s| s.id().value() == 0)
                .unwrap()
                .color(),
            color
        );
        assert_eq!(
            editor
                .project()
                .arrangement_section(source)
                .unwrap()
                .color(),
            ElementColor::Cyan
        );
        let after = editor.project().snapshot();
        assert!(editor.undo());
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
    }
}

#[test]
fn exhausted_palette_refuses_without_mutating_project_or_history() {
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 32).unwrap();
    let mut first = None;
    for (i, color) in ElementColor::ALL.into_iter().enumerate() {
        let id = section(&mut editor, color, (i * 2) as u64);
        first.get_or_insert(id);
    }
    let before = editor.project().clone();
    let command = Command::DuplicateArrangementSection(first.unwrap());
    assert!(before.preview_arrangement_section_edit(&command).is_err());
    let error = editor.execute(command).unwrap_err();
    assert!(error.to_string().contains("color"));
    assert_eq!(editor.project(), &before);
    assert!(editor.undo());
    assert_eq!(
        editor.project().arrangement_sections().len(),
        ElementColor::ALL.len() - 1
    );
}
