//! Marquee membership is live view state; cancellation restores its valid baseline.
use super::creation_tests::{fixture, pump};
use gpui::{Modifiers, MouseButton, TestAppContext, point, px};
use hex_project::{BeatTime, Command};
use hex_session::service::Request;

#[gpui::test]
fn marquee_selects_on_contact_and_escape_restores_its_owned_baseline(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    for (beat, pitch) in [(1, 60), (3, 64)] {
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::from_quarters(beat).unwrap(),
                duration: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
                pitch,
                channel: 0,
                velocity: 100,
            })));
        });
        pump(&editor, &mut visual);
    }
    let (before, first, baseline, low, rows) = editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        (
            state.project.snapshot(),
            state.project.midi_source(clip).unwrap().notes()[0].id(),
            v.midi_selection.primary().unwrap(),
            v.piano_pitch_low,
            v.piano_pitch_rows,
        )
    });
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let top = roll.origin.y + roll.size.height * (f32::from(low + rows - 1 - 60) / f32::from(rows));
    let start = point(roll.origin.x + roll.size.width * 0.15, top - px(1.0));
    let end = point(
        roll.origin.x + roll.size.width * 0.4,
        top + roll.size.height / f32::from(rows) + px(1.0),
    );
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };
    for cancel in [true, false] {
        visual.simulate_mouse_down(start, MouseButton::Left, shift);
        visual.simulate_mouse_move(end, Some(MouseButton::Left), shift);
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.midi_selection.ids(),
                &[first, baseline],
                "contact selects before pointer release"
            );
            assert_eq!(
                v.midi_marquee.as_ref().unwrap().highlighted().as_ref(),
                &[first, baseline]
            );
        });
        if cancel {
            visual.simulate_keystrokes("escape");
        }
        visual.simulate_mouse_up(end, MouseButton::Left, shift);
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            if cancel {
                assert_eq!(v.midi_selection.ids(), &[baseline]);
            } else {
                assert_eq!(v.midi_selection.ids(), &[first, baseline]);
            }
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            assert!(!v.edit_pending());
        });
    }

    editor.update(&mut visual, |v, cx| {
        v.midi_selection.set_single(first);
        cx.notify();
    });
    visual.run_until_parked();

    let row64 =
        roll.origin.y + roll.size.height * (f32::from(low + rows - 1 - 64) / f32::from(rows));
    let second_start = point(roll.origin.x + roll.size.width * 0.65, row64 - px(1.0));
    let second_end = point(
        roll.origin.x + roll.size.width * 0.9,
        row64 + roll.size.height / f32::from(rows) + px(1.0),
    );
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };
    visual.simulate_mouse_down(second_start, MouseButton::Left, shift);
    visual.simulate_mouse_move(second_end, Some(MouseButton::Left), shift);
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_selection.ids(), &[first, baseline]);
        assert_eq!(
            v.midi_marquee.as_ref().unwrap().highlighted().as_ref(),
            &[first, baseline]
        );
    });
    // Backtracking retracts hits instead of accumulating every crossed note.
    visual.simulate_mouse_move(
        second_start + point(px(1.0), px(1.0)),
        Some(MouseButton::Left),
        shift,
    );
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.midi_marquee.as_ref().unwrap().highlighted().as_ref(),
            &[first]
        );
    });
    editor.read_with(&visual, |v, _| assert_eq!(v.midi_selection.ids(), &[first]));
    // Release must use its actual position even without a final motion event.
    visual.simulate_mouse_up(second_end, MouseButton::Left, shift);
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_selection.ids(), &[first, baseline])
    });

    visual.simulate_mouse_down(start, MouseButton::Left, shift);
    visual.simulate_mouse_move(end, Some(MouseButton::Left), shift);
    visual.run_until_parked();
    editor.update(&mut visual, |v, cx| {
        v.midi_selection.set_single(baseline);
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_up(end, MouseButton::Left, shift);
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.midi_selection.ids(),
            &[baseline],
            "an older preview cannot replace newer selection"
        );
    });
    visual.simulate_keystrokes("cmd-a");
    visual.run_until_parked();

    visual.simulate_mouse_down(start, MouseButton::Left, shift);
    visual.simulate_mouse_move(end, Some(MouseButton::Left), shift);
    visual.run_until_parked();
    editor.update(&mut visual, |v, cx| {
        v.piano_pitch_rows = 12;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_up(end, MouseButton::Left, shift);
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_marquee.is_none());
        assert_eq!(
            v.midi_selection.ids(),
            &[first, baseline],
            "stale geometry must not commit a preview"
        );
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
}

#[gpui::test]
fn empty_grid_drag_selects_notes_without_a_modifier(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    for (beat, pitch) in [(1, 60), (3, 64)] {
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::from_quarters(beat).unwrap(),
                duration: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
                pitch,
                channel: 0,
                velocity: 100,
            })));
        });
        pump(&editor, &mut visual);
    }
    let (first, baseline, low, rows) = editor.read_with(&visual, |view, _| {
        let notes = view
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap()
            .notes();
        (
            notes[0].id(),
            notes[1].id(),
            view.piano_pitch_low,
            view.piano_pitch_rows,
        )
    });
    editor.update(&mut visual, |view, cx| {
        view.midi_selection.set_single(baseline);
        cx.notify();
    });
    visual.run_until_parked();
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let top = roll.origin.y + roll.size.height * (f32::from(low + rows - 1 - 60) / f32::from(rows));
    let start = point(roll.origin.x + roll.size.width * 0.15, top - px(1.0));
    let end = point(
        roll.origin.x + roll.size.width * 0.4,
        top + roll.size.height / f32::from(rows) + px(1.0),
    );
    for cancel in [true, false] {
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(view.midi_marquee.is_some());
            assert!(view.midi_time_selection.gesture.is_none());
            assert_eq!(view.midi_selection.ids(), &[first]);
        });
        if cancel {
            visual.simulate_keystrokes("escape");
        }
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            let expected = if cancel { baseline } else { first };
            assert_eq!(view.midi_selection.ids(), &[expected]);
        });
    }
}
