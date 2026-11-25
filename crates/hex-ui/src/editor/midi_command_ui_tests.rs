//! Focused key input through canonical note commands, worker acknowledgement and history.
use super::super::midi_canvas::creation_tests::{fixture, pump};
use gpui::TestAppContext;
use hex_project::{BeatTime, Command, MidiNoteInsert};
use hex_session::service::Request;

fn beat(value: f64) -> BeatTime {
    BeatTime::from_raw((value * BeatTime::ONE.raw() as f64) as u64)
}

#[gpui::test]
fn keyboard_single_and_group_move_grow_extent_once_and_reverse_without_shrinking(
    cx: &mut TestAppContext,
) {
    for group in [false, true] {
        let (mut visual, editor, _environment, clip) = fixture(cx);
        editor.update(&mut visual, |v, _| {
            assert!(
                v.enqueue(Request::Edit(Command::AddMidiNotes {
                    clip,
                    notes: [(3.5, 0.5, 60), (3.75, 0.25, 64)]
                        .into_iter()
                        .map(|(start, duration, pitch)| MidiNoteInsert {
                            active: true,
                            start: beat(start),
                            duration: beat(duration),
                            pitch,
                            channel: 2,
                            velocity: 93,
                        })
                        .collect(),
                }))
            );
        });
        pump(&editor, &mut visual);
        let before = editor.update(&mut visual, |v, cx| {
            let source = v
                .state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .clone();
            if group {
                v.midi_editor
                    .selection
                    .select_all(source.notes().iter().map(|n| n.id()))
                    .unwrap();
            } else {
                v.midi_editor.selection.set_single(source.notes()[0].id());
            }
            cx.notify();
            source
        });
        visual.run_until_parked();
        visual.simulate_keystrokes("right");
        editor.read_with(&visual, |v, _| {
            assert!(matches!(
                v.test_probes.last_enqueued_edit,
                Some(Command::UpdateMidiNotesWithSourceExtent { .. })
            ));
            assert_eq!(
                v.state.as_ref().unwrap().project.midi_source(clip).unwrap(),
                &before
            );
        });
        pump(&editor, &mut visual);
        let changed = editor.read_with(&visual, |v, _| {
            let project = &v.state.as_ref().unwrap().project;
            let source = project.midi_source(clip).unwrap();
            assert_eq!(source.source_length(), beat(4.25));
            assert_eq!(project.midi_clip(clip).unwrap().length(), beat(4.0));
            for (i, old) in before.notes().iter().enumerate() {
                let note = source.notes().iter().find(|n| n.id() == old.id()).unwrap();
                assert_eq!(
                    note.start().raw(),
                    old.start().raw() + if group || i == 0 { beat(0.25).raw() } else { 0 }
                );
                assert_eq!(
                    (note.duration(), note.velocity(), note.channel()),
                    (old.duration(), old.velocity(), old.channel())
                );
            }
            source.clone()
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.midi_source(clip).unwrap(),
                &before
            )
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.midi_source(clip).unwrap(),
                &changed
            )
        });
        visual.simulate_keystrokes("left");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let source = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
            assert_eq!(source.source_length(), beat(4.25));
            assert_eq!(source.notes(), before.notes());
        });
    }
}

#[gpui::test]
fn lengthen_single_and_group_notes_past_source_end_grows_source_and_undoes_atomically(
    cx: &mut TestAppContext,
) {
    for group in [false, true] {
        let (mut visual, editor, _environment, clip) = fixture(cx);
        editor.update(&mut visual, |v, _| {
            assert!(
                v.enqueue(Request::Edit(Command::AddMidiNotes {
                    clip,
                    notes: [(3.5, 0.5, 60), (3.75, 0.25, 64)]
                        .into_iter()
                        .map(|(start, duration, pitch)| MidiNoteInsert {
                            active: true,
                            start: beat(start),
                            duration: beat(duration),
                            pitch,
                            channel: 2,
                            velocity: 93,
                        })
                        .collect(),
                }))
            );
        });
        pump(&editor, &mut visual);
        let before = editor.update(&mut visual, |v, cx| {
            let source = v
                .state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .clone();
            if group {
                v.midi_editor
                    .selection
                    .select_all(source.notes().iter().map(|n| n.id()))
                    .unwrap();
            } else {
                v.midi_editor.selection.set_single(source.notes()[0].id());
            }
            cx.notify();
            source
        });
        visual.run_until_parked();
        editor.update(&mut visual, |v, cx| {
            let revision = v.state.as_ref().unwrap().revision;
            let generation = v.project_generation;
            if group {
                v.adjust_selected_group_current(
                    super::super::midi_group_drag::GroupParameter::Duration(1),
                );
            } else {
                let note = v.midi_editor.selection.primary().unwrap();
                v.update_note_field(note, revision, generation, super::NoteField::Duration(1));
            }
            cx.notify();
        });
        editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            assert!(matches!(
                v.test_probes.last_enqueued_edit,
                Some(Command::UpdateMidiNotesWithSourceExtent { .. })
            ));
        });
        pump(&editor, &mut visual);
        let changed = editor.read_with(&visual, |v, _| {
            let project = &v.state.as_ref().unwrap().project;
            let source = project.midi_source(clip).unwrap();
            assert_eq!(source.source_length(), beat(4.25));
            assert_eq!(project.midi_clip(clip).unwrap().length(), beat(4.0));
            for (i, old) in before.notes().iter().enumerate() {
                let note = source.notes().iter().find(|n| n.id() == old.id()).unwrap();
                assert_eq!(note.start(), old.start());
                assert_eq!(
                    note.duration().raw(),
                    old.duration().raw() + if group || i == 0 { beat(0.25).raw() } else { 0 }
                );
            }
            source.clone()
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.midi_source(clip).unwrap(),
                &before
            );
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.midi_source(clip).unwrap(),
                &changed
            );
        });
    }
}
