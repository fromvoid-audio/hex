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
fn duplicates_keep_palette_colors_in_preview_history_and_both_entry_points() {
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
        assert!(ElementColor::ALL.contains(&color));
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
fn exhausted_palette_allows_repeated_creation_and_both_duplicate_entry_points() {
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 64).unwrap();
    let mut ids = Vec::new();
    for (i, color) in ElementColor::ALL.into_iter().enumerate() {
        ids.push(section(&mut editor, color, (i * 2) as u64));
    }
    // Explicit creation also permits repeated colors after every hue is used.
    section(&mut editor, ElementColor::Cyan, 24);
    for round in 0..12 {
        let command = if round % 2 == 0 {
            Command::DuplicateArrangementSection(ids[0])
        } else {
            Command::DuplicateArrangementSectionAt {
                section: ids[0],
                target: ids[1],
                after: round % 3 == 0,
            }
        };
        let before = editor.project().snapshot();
        let preview = editor
            .project()
            .preview_arrangement_section_edit(&command)
            .unwrap();
        let Created::ArrangementSection(copy) = editor.execute(command).unwrap() else {
            panic!("copy identity")
        };
        let copied = editor.project().arrangement_section(copy).unwrap();
        assert!(ElementColor::ALL.contains(&copied.color()));
        assert_eq!(copied.name(), "part");
        assert_eq!(
            copied.end().raw() - copied.start().raw(),
            BeatTime::ONE.raw()
        );
        assert_eq!(
            editor
                .project()
                .arrangement_section(ids[0])
                .unwrap()
                .color(),
            ElementColor::Gray
        );
        assert_eq!(
            preview
                .sections()
                .iter()
                .find(|s| s.id().value() == 0)
                .unwrap()
                .color(),
            copied.color()
        );
        let after = editor.project().snapshot();
        assert!(editor.undo());
        let mut undone = editor.project().snapshot();
        undone.next_id = before.next_id;
        assert_eq!(undone, before);
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
    }
}
