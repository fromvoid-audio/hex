use super::super::test_support::beat;
use super::*;
use crate::Editor;

fn fixture(source_first: bool, target_length: u64) -> (Editor, SectionId, SectionId) {
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 16).unwrap();
    editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "owned.wav".into(),
            sample_rate: 8_000,
            frames: 96_000,
        })
        .unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "midi".into(),
        })
        .unwrap()
    else {
        panic!("midi")
    };
    let source_start = if source_first { 0 } else { 8 };
    let target_start = if source_first { 8 } else { 0 };
    let mut markers = Vec::new();
    for (name, color, start, length, pitch) in [
        ("source", ElementColor::Blue, source_start, 4, 60),
        (
            "target",
            ElementColor::Rose,
            target_start,
            target_length,
            72,
        ),
        ("tail", ElementColor::Cyan, 20, 4, 84),
    ] {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: beat(start),
                length: beat(length),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: beat(0),
                duration: beat(1),
                pitch,
                channel: 0,
                velocity: 90,
            })
            .unwrap();
        let Created::ArrangementSection(id) = editor
            .execute(Command::AddArrangementSection {
                name: name.into(),
                color,
                start: beat(start),
                end: beat(start + length),
            })
            .unwrap()
        else {
            panic!("section")
        };
        markers.push(id);
    }
    (
        Editor::new(editor.project().clone(), 8).unwrap(),
        markers[0],
        markers[1],
    )
}

fn edit(source: SectionId, target: SectionId, copy: bool) -> Command {
    Command::ReplaceArrangementSection {
        source,
        target,
        copy,
    }
}
fn geometry(sections: &[ArrangementSection]) -> Vec<(&str, ElementColor, BeatTime, BeatTime)> {
    sections
        .iter()
        .map(|m| (m.name(), m.color(), m.start(), m.end()))
        .collect()
}

#[test]
fn replacement_orders_lengths_and_modes_preserve_non_target_music_and_one_history_edge() {
    for source_first in [false, true] {
        for target_length in [2, 6] {
            for copy in [false, true] {
                let (mut editor, source, target) = fixture(source_first, target_length);
                let before = editor.project().snapshot();
                let command = edit(source, target, copy);
                assert!(command.requires_section_publication());
                let preview = editor
                    .project()
                    .preview_arrangement_section_edit(&command)
                    .unwrap();
                assert_eq!(editor.project().snapshot(), before);
                let created = editor.execute(command).unwrap();
                let result = if copy {
                    let Created::ArrangementSection(id) = created else {
                        panic!("copy")
                    };
                    assert_ne!(id, source);
                    assert_ne!(id, target);
                    id
                } else {
                    assert_eq!(created, Created::None);
                    source
                };
                let project = editor.project();
                let after = project.snapshot();
                assert_eq!(
                    geometry(project.arrangement_sections()),
                    geometry(preview.sections())
                );
                assert_eq!(project.tempo_map(), preview.tempo_map());
                assert!(project.arrangement_section(target).is_none());
                assert_eq!(
                    project.arrangement_sections().len(),
                    if copy { 3 } else { 2 }
                );
                let replacement = project.arrangement_section(result).unwrap();
                let start = if source_first {
                    if copy { 8 } else { 4 }
                } else {
                    0
                };
                assert_eq!(
                    (
                        replacement.name(),
                        replacement.color(),
                        replacement.start(),
                        replacement.end()
                    ),
                    ("source", ElementColor::Blue, beat(start), beat(start + 4))
                );
                let midi = project.tracks()[1].midi_clips();
                assert!(
                    !midi.iter().any(|c| c.notes()[0].pitch() == 72),
                    "target content alone is erased"
                );
                assert_eq!(
                    midi.iter().filter(|c| c.notes()[0].pitch() == 60).count(),
                    if copy { 2 } else { 1 }
                );
                assert!(midi.iter().all(|c| c.notes()[0].velocity() == 90));
                let tail = midi.iter().find(|c| c.notes()[0].pitch() == 84).unwrap();
                assert_eq!(
                    tail.start(),
                    beat(20 - target_length + if copy { 4 } else { 0 })
                );
                assert_eq!(
                    midi.iter().map(|c| c.length().raw()).sum::<u64>(),
                    beat(if copy { 12 } else { 8 }).raw()
                );
                assert_eq!(project.assets().count(), 1);
                assert_eq!(
                    project.tracks()[0]
                        .clips()
                        .iter()
                        .map(|c| c.duration())
                        .sum::<u64>(),
                    96_000 - target_length * 4_000 + if copy { 16_000 } else { 0 }
                );
                if copy {
                    let original = project.arrangement_section(source).unwrap();
                    assert_eq!(
                        original.start(),
                        beat(if source_first {
                            0
                        } else {
                            8 - target_length + 4
                        })
                    );
                }
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
    }
}

#[test]
fn self_stale_and_late_allocation_refusal_preserve_complete_state_and_history() {
    let (mut editor, source, target) = fixture(true, 2);
    let before = editor.project().snapshot();
    for copy in [false, true] {
        let preview = editor
            .project()
            .preview_arrangement_section_edit(&edit(source, source, copy))
            .unwrap();
        assert_eq!(
            geometry(preview.sections()),
            geometry(editor.project().arrangement_sections())
        );
        assert_eq!(
            editor.execute(edit(source, source, copy)).unwrap(),
            Created::None
        );
        assert_eq!(editor.project().snapshot(), before);
        assert!(!editor.can_undo());
        for command in [
            edit(SectionId(u64::MAX), target, copy),
            edit(source, SectionId(u64::MAX), copy),
        ] {
            assert!(
                editor
                    .project()
                    .preview_arrangement_section_edit(&command)
                    .is_err()
            );
            assert!(editor.execute(command).is_err());
            assert_eq!(editor.project().snapshot(), before);
            assert!(!editor.can_undo());
        }
    }
    let mut project = Project::new(8_000).unwrap();
    project.arrangement_sections = vec![
        ArrangementSection {
            id: SectionId(1),
            name: "source".into(),
            color: ElementColor::Blue,
            start: beat(0),
            end: beat(4),
        },
        ArrangementSection {
            id: SectionId(2),
            name: "target".into(),
            color: ElementColor::Rose,
            start: beat(8),
            end: beat(10),
        },
    ];
    project.next_id = u64::MAX;
    let mut editor = Editor::new(project, 1).unwrap();
    let before = editor.project().snapshot();
    assert!(
        editor
            .project()
            .preview_arrangement_section_edit(&edit(SectionId(1), SectionId(2), true))
            .is_ok()
    );
    assert_eq!(
        editor
            .execute(edit(SectionId(1), SectionId(2), true))
            .unwrap_err()
            .0,
        "project id space exhausted"
    );
    assert_eq!(
        editor.project().snapshot(),
        before,
        "failed copy restores the already deleted target"
    );
    assert!(!editor.can_undo());
}

#[test]
fn replacement_reuses_the_target_marker_slot_at_capacity() {
    let mut project = Project::new(8_000).unwrap();
    project.arrangement_sections = (0..MAX_ARRANGEMENT_SECTIONS)
        .map(|i| ArrangementSection {
            id: SectionId(i as u64 + 1),
            name: "section".into(),
            color: ElementColor::Blue,
            start: beat(i as u64),
            end: beat(i as u64 + 1),
        })
        .collect();
    project.next_id = MAX_ARRANGEMENT_SECTIONS as u64 + 1;
    let mut editor = Editor::new(project, 1).unwrap();
    let command = edit(SectionId(1), SectionId(2), true);
    let preview = editor
        .project()
        .preview_arrangement_section_edit(&command)
        .unwrap();
    editor.execute(command).unwrap();
    assert_eq!(
        editor.project().arrangement_sections().len(),
        MAX_ARRANGEMENT_SECTIONS
    );
    assert_eq!(
        geometry(editor.project().arrangement_sections()),
        geometry(preview.sections())
    );
}
