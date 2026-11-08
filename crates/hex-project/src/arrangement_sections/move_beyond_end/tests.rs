use super::super::test_support::{beat, fixture};
use super::*;
use crate::Editor;

fn command(section: SectionId, start: BeatTime) -> Command {
    Command::MoveArrangementSectionBeyondEnd {
        section,
        start,
        gap_name: "gap".into(),
        gap_color: ElementColor::Rose,
    }
}

fn geometry(sections: &[ArrangementSection]) -> Vec<(&str, ElementColor, BeatTime, BeatTime)> {
    sections
        .iter()
        .map(|m| (m.name(), m.color(), m.start(), m.end()))
        .collect()
}

#[test]
fn first_last_and_sole_moves_preserve_music_and_match_preview_in_one_history_edge() {
    for mode in 0..3 {
        let (original, first) = fixture();
        let mut project = original.project().clone();
        let section = if mode == 1 {
            project.arrangement_sections()[1].id()
        } else {
            first
        };
        if mode == 2 {
            project.arrangement_sections.retain(|m| m.id == first);
        }
        let mut editor = Editor::new(project, 8).unwrap();
        let before = editor.project().snapshot();
        let edit = command(section, beat(12));
        assert!(edit.requires_section_publication());
        let preview = editor
            .project()
            .preview_arrangement_section_edit(&edit)
            .unwrap();
        assert_eq!(editor.project().snapshot(), before, "preview is read only");
        assert_eq!(editor.execute(edit).unwrap(), Created::None);
        let project = editor.project();
        let after = project.snapshot();
        assert_eq!(
            geometry(project.arrangement_sections()),
            geometry(preview.sections())
        );
        assert_eq!(project.tempo_map(), preview.tempo_map());
        let moved = project.arrangement_section(section).unwrap();
        let old = before
            .arrangement_sections
            .iter()
            .find(|m| m.id == section.value())
            .unwrap();
        assert_eq!(
            (moved.name(), moved.color(), moved.start(), moved.end()),
            (old.name.as_str(), old.color, beat(12), beat(16))
        );
        let gap = project
            .arrangement_sections()
            .iter()
            .find(|m| m.name() == "gap")
            .unwrap();
        assert_eq!(
            (gap.start(), gap.end(), gap.color()),
            (
                beat(if mode == 2 { 0 } else { 4 }),
                beat(12),
                ElementColor::Rose
            )
        );
        assert!(
            before
                .arrangement_sections
                .iter()
                .all(|m| m.id != gap.id().value())
        );
        for marker in &before.arrangement_sections {
            assert!(project.arrangement_section(SectionId(marker.id)).is_some());
        }
        assert_eq!(project.assets().count(), 1);
        let audio = project.tracks()[0].clips();
        assert_eq!(audio.len(), 1);
        assert_eq!(audio[0].id().value(), before.tracks[0].clips[0].id);
        assert_eq!(audio[0].start(), if mode == 1 { 0 } else { 48_000 });
        let midi = project.tracks()[1].midi_clips();
        assert_eq!(midi.len(), 3);
        assert_eq!(
            midi.iter().map(|c| c.length().raw()).sum::<u64>(),
            beat(12).raw()
        );
        let tail = midi.iter().find(|c| c.notes()[0].pitch() == 72).unwrap();
        assert_eq!(
            tail.start(),
            beat(8),
            "unmarked music remains in the marked gap"
        );
        let placed = midi.iter().find(|c| c.start() == beat(12)).unwrap();
        assert_eq!(
            (placed.notes()[0].pitch(), placed.notes()[0].velocity()),
            (60, 90)
        );
        assert_eq!(
            Project::from_snapshot(after.clone()).unwrap().snapshot(),
            after
        );
        assert!(editor.undo());
        let mut restored = editor.project().snapshot();
        assert!(restored.next_id >= after.next_id);
        restored.next_id = before.next_id;
        assert_eq!(restored, before);
        assert!(!editor.can_undo());
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
    }
}

#[test]
fn invalid_inputs_and_late_gap_allocation_refusal_preserve_complete_state() {
    let (mut editor, source) = fixture();
    let before = editor.project().snapshot();
    let mut bad_name = command(source, beat(12));
    if let Command::MoveArrangementSectionBeyondEnd { gap_name, .. } = &mut bad_name {
        gap_name.clear();
    }
    for edit in [
        command(source, beat(7)),
        command(source, beat(8)),
        command(source, BeatTime::from_raw(u64::MAX)),
        command(SectionId(u64::MAX), beat(12)),
        bad_name,
    ] {
        assert!(
            editor
                .project()
                .preview_arrangement_section_edit(&edit)
                .is_err()
        );
        assert!(editor.execute(edit).is_err());
        assert_eq!(editor.project().snapshot(), before);
        assert!(!editor.can_undo());
    }
    let mut exhausted = editor.project().clone();
    exhausted.next_id = u64::MAX;
    let mut editor = Editor::new(exhausted, 1).unwrap();
    let before = editor.project().snapshot();
    let edit = command(source, beat(12));
    assert!(
        editor
            .project()
            .preview_arrangement_section_edit(&edit)
            .is_ok()
    );
    assert_eq!(
        editor.execute(edit).unwrap_err().0,
        "project id space exhausted"
    );
    assert_eq!(
        editor.project().snapshot(),
        before,
        "late refusal rolls back the music permutation too"
    );
    assert!(!editor.can_undo());
}

#[test]
fn capacity_reserves_only_one_new_gap_marker() {
    for count in [MAX_ARRANGEMENT_SECTIONS - 1, MAX_ARRANGEMENT_SECTIONS] {
        let mut project = Project::new(8_000).unwrap();
        project.arrangement_sections = (0..count)
            .map(|i| ArrangementSection {
                id: SectionId(i as u64 + 1),
                name: "section".into(),
                color: ElementColor::Blue,
                start: beat(i as u64),
                end: beat(i as u64 + 1),
            })
            .collect();
        project.next_id = count as u64 + 1;
        let mut editor = Editor::new(project, 1).unwrap();
        let before = editor.project().snapshot();
        let edit = command(SectionId(1), beat(count as u64 + 4));
        if count == MAX_ARRANGEMENT_SECTIONS {
            assert!(
                editor
                    .project()
                    .preview_arrangement_section_edit(&edit)
                    .is_err()
            );
            assert!(editor.execute(edit).is_err());
            assert_eq!(editor.project().snapshot(), before);
            assert!(!editor.can_undo());
        } else {
            let preview = editor
                .project()
                .preview_arrangement_section_edit(&edit)
                .unwrap();
            editor.execute(edit).unwrap();
            assert_eq!(
                editor.project().arrangement_sections().len(),
                MAX_ARRANGEMENT_SECTIONS
            );
            assert_eq!(
                geometry(editor.project().arrangement_sections()),
                geometry(preview.sections())
            );
        }
    }
}
