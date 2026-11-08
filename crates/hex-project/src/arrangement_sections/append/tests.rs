use super::*;
use crate::Editor;

fn beat(value: u64) -> BeatTime {
    BeatTime::from_quarters(value).unwrap()
}

fn marker(editor: &mut Editor, name: &str, start: u64, end: u64) -> SectionId {
    let Created::ArrangementSection(id) = editor
        .execute(Command::AddArrangementSection {
            name: name.into(),
            color: ElementColor::Blue,
            start: beat(start),
            end: beat(end),
        })
        .unwrap()
    else {
        panic!("section fixture");
    };
    id
}

fn fixture() -> (Editor, SectionId) {
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 16).unwrap();
    editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "owned.wav".into(),
            sample_rate: 8_000,
            frames: 16_000,
        })
        .unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "midi".into(),
        })
        .unwrap()
    else {
        panic!("midi track");
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(0),
            length: beat(4),
        })
        .unwrap()
    else {
        panic!("midi clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(1),
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 90,
        })
        .unwrap();
    editor
        .execute(Command::DuplicateAliasClips(vec![clip]))
        .unwrap();
    let Created::Clip(tail) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(12),
            length: beat(4),
        })
        .unwrap()
    else {
        panic!("unmarked tail");
    };
    editor
        .execute(Command::AddMidiNote {
            clip: tail,
            start: beat(0),
            duration: beat(1),
            pitch: 72,
            channel: 0,
            velocity: 80,
        })
        .unwrap();
    let source = marker(&mut editor, "source", 0, 4);
    marker(&mut editor, "last", 4, 8);
    (Editor::new(editor.project().clone(), 8).unwrap(), source)
}

fn command(source: SectionId, start: BeatTime) -> Command {
    Command::AppendArrangementSectionCopy {
        section: source,
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
fn preview_and_atomic_copy_agree_with_and_without_a_gap() {
    for start in [8, 12] {
        let (mut editor, source) = fixture();
        let before = editor.project().snapshot();
        let edit = command(source, beat(start));
        assert!(edit.requires_section_publication());
        let preview = editor
            .project()
            .preview_arrangement_section_edit(&edit)
            .unwrap();
        assert_eq!(editor.project().snapshot(), before, "preview is read only");
        let Created::ArrangementSection(copy) = editor.execute(edit).unwrap() else {
            panic!("copied section");
        };
        let after = editor.project().snapshot();
        let project = editor.project();
        assert_eq!(
            geometry(project.arrangement_sections()),
            geometry(preview.sections())
        );
        assert_eq!(project.tempo_map(), preview.tempo_map());
        assert_ne!(copy, source);
        assert_eq!(
            project.arrangement_section(copy).unwrap().start(),
            beat(start)
        );
        assert_eq!(
            project.arrangement_section(copy).unwrap().end(),
            beat(start + 4)
        );
        for original in &before.arrangement_sections {
            let retained = project.arrangement_section(SectionId(original.id)).unwrap();
            assert_eq!(
                (retained.name(), retained.start(), retained.end()),
                (original.name.as_str(), original.start, original.end)
            );
        }
        assert_eq!(project.assets().count(), 1, "copy shares original media");
        let audio = project.tracks()[0].clips();
        assert_eq!(audio.len(), 2);
        assert_eq!(audio[0].asset(), audio[1].asset());
        assert_eq!((audio[0].start(), audio[1].start()), (0, start * 4_000));
        let midi = project.tracks()[1].midi_clips();
        assert_eq!(midi.len(), 4);
        assert_eq!(
            midi[3].start(),
            beat(16),
            "unmarked destination music shifts intact"
        );
        assert_eq!(midi[3].notes()[0].pitch(), 72);
        assert_eq!(midi[2].start(), beat(start));
        assert_eq!(midi[2].notes().len(), 1);
        assert_eq!(midi[2].notes()[0].pitch(), 60);
        assert_eq!(midi[2].notes()[0].velocity(), 90);
        let gap = project
            .arrangement_sections()
            .iter()
            .find(|m| m.name() == "gap");
        if start == 8 {
            assert!(gap.is_none());
        } else {
            let gap = gap.unwrap();
            assert_eq!(
                (gap.start(), gap.end(), gap.color()),
                (beat(8), beat(start), ElementColor::Rose)
            );
            assert_ne!(gap.id(), copy);
            assert_ne!(gap.id(), source);
        }
        assert_eq!(
            Project::from_snapshot(after.clone()).unwrap().snapshot(),
            after
        );
        assert!(editor.undo());
        let mut restored = editor.project().snapshot();
        assert!(
            restored.next_id >= after.next_id,
            "undo must not reuse allocated IDs"
        );
        restored.next_id = before.next_id;
        assert_eq!(restored, before);
        assert!(!editor.can_undo(), "one history edge owns copy and gap");
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
    }
}

#[test]
fn invalid_append_preserves_the_complete_project_and_history() {
    let (mut editor, source) = fixture();
    let before = editor.project().snapshot();
    let mut invalid_name = command(source, beat(12));
    if let Command::AppendArrangementSectionCopy { gap_name, .. } = &mut invalid_name {
        gap_name.clear();
    }
    for edit in [
        command(source, beat(7)),
        command(source, BeatTime::from_raw(u64::MAX)),
        command(SectionId(u64::MAX), beat(12)),
        invalid_name,
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
}

#[test]
fn capacity_accounts_for_both_copy_and_gap_before_mutation() {
    for count in [MAX_ARRANGEMENT_SECTIONS - 1, MAX_ARRANGEMENT_SECTIONS] {
        let mut project = Project::new(8_000).unwrap();
        project.arrangement_sections = (0..count)
            .map(|index| ArrangementSection {
                id: SectionId(index as u64 + 1),
                name: "section".into(),
                color: ElementColor::Blue,
                start: beat(index as u64),
                end: beat(index as u64 + 1),
            })
            .collect();
        project.next_id = count as u64 + 1;
        let mut editor = Editor::new(project, 1).unwrap();
        let before = editor.project().snapshot();
        let edit = command(SectionId(1), beat(count as u64 + 1));
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
}
