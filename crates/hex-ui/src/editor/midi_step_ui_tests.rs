//! Real focused key input, canonical worker ACK, source growth and history.
use super::creation_tests::{fixture, pump};
use gpui::{KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, TestAppContext};
use hex_project::BeatTime;
use hex_session::service::Request;

const STEP: &str = "toggle step entry · enable computer keyboard with m · hold notes and press right · no notes inserts a rest";

#[gpui::test]
fn held_chord_steps_across_ack_release_rest_growth_and_history(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    visual.simulate_keystrokes("m");
    visual.run_until_parked();
    let button = visual.debug_bounds(STEP).unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    for key in ["a", "d"] {
        visual.simulate_event(KeyDownEvent {
            keystroke: Keystroke::parse(key).unwrap(),
            is_held: false,
        });
    }
    for index in 0..2 {
        visual.simulate_keystrokes("right");
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.midi_marker,
                Some(index * BeatTime::ONE.raw() / 4),
                "acceptance does not advance before ACK"
            );
        });
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let notes = v
                .state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes();
            assert_eq!(notes.len(), (index as usize + 1) * 2);
            assert_eq!(v.midi_marker, Some((index + 1) * BeatTime::ONE.raw() / 4));
            assert_eq!(v.computer_keyboard.physical_notes().count(), 2);
            for note in notes {
                assert_eq!(note.velocity(), 100);
                assert_eq!(note.duration().raw(), BeatTime::ONE.raw() / 4);
            }
        });
    }
    let authored = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    visual.simulate_keystrokes("left");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_marker, Some(BeatTime::ONE.raw() / 4));
        assert_eq!(
            v.state.as_ref().unwrap().project.snapshot(),
            authored,
            "step-mode left navigates rather than moving selected notes"
        );
    });
    editor.update(&mut visual, |v, _| {
        v.midi_marker = Some(BeatTime::ONE.raw() / 2)
    });
    // Octave changes do not change the identity of already held keys; a modified
    // key-up still releases the original physical key and its old pitch.
    visual.simulate_keystrokes("x");
    visual.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("shift-a").unwrap(),
    });
    visual.simulate_keystrokes("right");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let notes = v
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap()
            .notes();
        assert_eq!(notes.len(), 5);
        assert_eq!(notes.last().unwrap().pitch(), 64);
    });
    visual.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("d").unwrap(),
    });
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    visual.simulate_keystrokes("right");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().project.snapshot(),
            before,
            "rest has no project edit"
        );
        assert_eq!(v.midi_marker, Some(BeatTime::ONE.raw()));
        assert!(!v.edit_pending());
    });
    // The same canonical extent command grows a short source once for the chord.
    editor.update(&mut visual, |v, cx| {
        v.midi_marker = Some(4 * BeatTime::ONE.raw());
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_event(KeyDownEvent {
        keystroke: Keystroke::parse("a").unwrap(),
        is_held: false,
    });
    visual.simulate_keystrokes("right");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let source = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
        assert_eq!(source.notes().len(), 6);
        assert_eq!(
            source.source_length().raw(),
            4 * BeatTime::ONE.raw() + BeatTime::ONE.raw() / 4
        );
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let actual = v.state.as_ref().unwrap().project.snapshot();
        let mut expected = before.clone();
        // History restores musical state; the identity allocator deliberately never reuses IDs.
        assert!(actual.next_id > expected.next_id);
        expected.next_id = actual.next_id;
        assert_eq!(actual, expected);
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()
                .len(),
            6
        )
    });
    editor.update(&mut visual, |v, _| v.stop());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.computer_keyboard.physical_notes().count(), 0)
    });
}

#[gpui::test]
fn failed_or_stale_step_cannot_advance_and_focus_loss_releases_physical_input(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    visual.simulate_keystrokes("m");
    visual.run_until_parked();
    let button = visual.debug_bounds(STEP).unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    visual.simulate_event(KeyDownEvent {
        keystroke: Keystroke::parse("a").unwrap(),
        is_held: false,
    });
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let cursor = u64::MAX - (BeatTime::ONE.raw() / 4) + 1;
    editor.update(&mut visual, |v, cx| {
        v.midi_marker = Some(cursor);
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("right");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(
            v.error.is_some(),
            "overflow must fail before submitting an edit"
        );
        assert_eq!(v.midi_marker, Some(cursor));
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
    editor.update(&mut visual, |v, cx| {
        v.midi_marker = Some(0);
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("right");
    editor.update(&mut visual, |v, _| {
        v.midi_marker = Some(2 * BeatTime::ONE.raw());
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.midi_marker,
            Some(2 * BeatTime::ONE.raw()),
            "late receipt cannot replace a newer cursor"
        );
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()
                .len(),
            1
        );
    });
    visual.update(|window, cx| editor.read_with(cx, |v, _| v.focus.focus(window)));
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.computer_keyboard.physical_notes().count(), 0)
    });
}
