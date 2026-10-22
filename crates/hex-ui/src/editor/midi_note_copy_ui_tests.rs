//! Real copy input keeps canonical IDs, selection ACK and history in their owners.
use super::creation_tests::{fixture, pump};
use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use hex_project::{BeatTime, ClipId, Command, MidiNote, MidiNoteInsert};
use hex_session::service::Request;

fn notes(
    editor: &Entity<super::EditorView>,
    visual: &VisualTestContext,
    clip: ClipId,
) -> Vec<MidiNote> {
    editor.read_with(visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap()
            .notes()
            .to_vec()
    })
}
fn authored(editor: &Entity<super::EditorView>, visual: &mut VisualTestContext, clip: ClipId) {
    editor.update(&mut *visual, |v, _| {
        assert!(
            v.enqueue(Request::Edit(Command::AddMidiNotes {
                clip,
                notes: [(0.5, 60, 37), (1.5, 64, 93)]
                    .into_iter()
                    .map(|(start, pitch, velocity)| MidiNoteInsert {
                        start: BeatTime::from_raw((start * BeatTime::ONE.raw() as f64) as u64),
                        duration: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
                        pitch,
                        channel: 2,
                        velocity
                    })
                    .collect()
            }))
        )
    });
    pump(editor, visual);
}
fn at(
    editor: &Entity<super::EditorView>,
    visual: &mut VisualTestContext,
    beat: f64,
    pitch: u8,
) -> gpui::Point<gpui::Pixels> {
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    editor.read_with(visual, |v, _| {
        roll.origin
            + point(
                px(1.0)
                    + (roll.size.width - px(2.0))
                        * ((beat * BeatTime::ONE.raw() as f64 - v.piano_beat_start.raw() as f64)
                            / v.piano_beat_span.raw() as f64) as f32,
                px(1.0)
                    + (roll.size.height - px(2.0))
                        * (f32::from(v.piano_pitch_low + v.piano_pitch_rows - 1 - pitch) + 0.5)
                        / f32::from(v.piano_pitch_rows),
            )
    })
}
#[gpui::test]
fn repeated_keyboard_copy_tracks_only_acknowledged_new_ids_and_roundtrips(cx: &mut TestAppContext) {
    let (mut visual, editor, environment, clip) = fixture(cx);
    authored(&editor, &mut visual, clip);
    let original = notes(&editor, &visual, clip);
    visual.simulate_keystrokes("cmd-a");
    let mut history =
        vec![editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot())];
    for repeat in 1..=3 {
        let old_selected = editor.read_with(&visual, |v, _| v.midi_selection.ids().to_vec());
        visual.simulate_keystrokes("cmd-d");
        pump(&editor, &mut visual);
        visual.run_until_parked();
        let all = notes(&editor, &visual, clip);
        assert_eq!(
            all.len(),
            2 * (repeat + 1),
            "copy notes, never the Arrangement clip"
        );
        for note in &original {
            assert!(all.contains(note));
        }
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.midi_selection.len(), 2);
            assert!(
                v.midi_selection
                    .ids()
                    .iter()
                    .all(|id| !old_selected.contains(id))
            );
            for (index, id) in v.midi_selection.ids().iter().enumerate() {
                let note = all.iter().find(|n| n.id() == *id).unwrap();
                assert_eq!(
                    note.start().raw(),
                    original[index].start().raw() + repeat as u64 * BeatTime::ONE.raw() * 5 / 4
                );
                assert_eq!(
                    (
                        note.duration(),
                        note.pitch(),
                        note.channel(),
                        note.velocity()
                    ),
                    (
                        original[index].duration(),
                        original[index].pitch(),
                        original[index].channel(),
                        original[index].velocity()
                    )
                );
            }
            assert_eq!(
                v.state.as_ref().unwrap().project.tracks()[0]
                    .midi_clips()
                    .len(),
                1
            );
        });
        history
            .push(editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot()));
    }
    for expected in history.iter().rev().skip(1) {
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(&v.state.as_ref().unwrap().project.snapshot(), expected)
        });
    }
    for expected in history.iter().skip(1) {
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(&v.state.as_ref().unwrap().project.snapshot(), expected)
        });
    }
    let saved = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let path = environment.root().join("note-copies.hex");
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::SaveNew(path.clone())))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Open(path))));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), saved)
    });
}
#[gpui::test]
fn option_drag_copies_single_and_group_with_preview_cancel_and_source_growth(
    cx: &mut TestAppContext,
) {
    for group in [false, true] {
        let (mut visual, editor, _environment, clip) = fixture(cx);
        authored(&editor, &mut visual, clip);
        let originals = notes(&editor, &visual, clip);
        if group {
            visual.simulate_keystrokes("cmd-a");
        } else {
            editor.update(&mut visual, |v, cx| {
                v.midi_selection.set_single(originals[0].id());
                cx.notify();
            });
        }
        editor.update(&mut visual, |v, cx| {
            v.piano_beat_span = BeatTime::from_quarters(8).unwrap();
            cx.notify();
        });
        visual.run_until_parked();
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let down = at(&editor, &mut visual, 0.625, 60);
        let end = at(&editor, &mut visual, 4.625, 60);
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        visual.simulate_mouse_down(down, MouseButton::Left, alt);
        visual.simulate_mouse_up(down, MouseButton::Left, alt);
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        visual.simulate_mouse_down(down, MouseButton::Left, alt);
        visual.simulate_mouse_move(end, Some(MouseButton::Left), alt);
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        visual.simulate_keystrokes("escape");
        visual.simulate_mouse_up(end, MouseButton::Left, alt);
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        visual.simulate_mouse_down(down, MouseButton::Left, alt);
        visual.simulate_mouse_move(end, Some(MouseButton::Left), alt);
        visual.simulate_mouse_up(end, MouseButton::Left, alt);
        pump(&editor, &mut visual);
        visual.run_until_parked();
        let all = notes(&editor, &visual, clip);
        assert_eq!(all.len(), if group { 4 } else { 3 });
        for original in &originals {
            assert!(all.contains(original));
        }
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.midi_selection.len(), if group { 2 } else { 1 });
            let source = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
            assert_eq!(
                source.source_length().raw(),
                BeatTime::ONE.raw() * if group { 23 } else { 19 } / 4
            );
            for id in v.midi_selection.ids() {
                let copy = all.iter().find(|n| n.id() == *id).unwrap();
                let original = originals
                    .iter()
                    .find(|n| n.pitch() == copy.pitch())
                    .unwrap();
                assert_eq!(
                    copy.start().raw(),
                    original.start().raw() + 4 * BeatTime::ONE.raw()
                );
                assert_ne!(copy.id(), original.id());
            }
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
    }
}
