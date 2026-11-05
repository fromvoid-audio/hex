//! Insertion navigation is local view state, separate from notes and transport.
use super::creation_tests::{fixture, pump};
use gpui::{Modifiers, MouseButton, TestAppContext, point};
use hex_project::{BeatTime, Command};
use hex_session::service::Request;

#[gpui::test]
fn empty_roll_arrows_move_the_snapped_cursor_without_editing(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _) = fixture(cx);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    visual.simulate_click(
        point(roll.origin.x + roll.size.width * 0.4, roll.center().y),
        Modifiers::default(),
    );
    visual.run_until_parked();
    let (cursor, step) = editor.read_with(&visual, |v, _| {
        (v.midi_marker.unwrap(), v.midi_snap_grid.movement_step())
    });
    assert_eq!(cursor % step, 0);
    visual.simulate_keystrokes("right");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_marker, Some(cursor + step));
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert!(!v.edit_pending());
    });
    visual.simulate_keystrokes("left");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert_eq!(v.midi_marker, Some(cursor)));
    // Clicking again owns the cursor and remains independent of vertical zoom.
    editor.update(&mut visual, |v, cx| {
        v.piano_pitch_rows = 12;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_down(
        roll.origin + point(gpui::px(1.0), gpui::px(20.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_up(
        roll.origin + point(gpui::px(1.0), gpui::px(20.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    visual.simulate_keystrokes("left");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_marker, Some(BeatTime::ZERO.raw()));
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
}

#[gpui::test]
fn cursor_navigation_keeps_note_nudging_and_resets_on_a_new_clip(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ONE,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })));
    });
    pump(&editor, &mut visual);
    let (cursor, note, step) = editor.read_with(&visual, |v, _| {
        (
            v.midi_marker,
            v.midi_selection.primary().unwrap(),
            v.midi_snap_grid.movement_step(),
        )
    });
    visual.simulate_keystrokes("right");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_marker, cursor, "selected notes retain arrow editing");
        let source = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
        assert_eq!(
            source
                .notes()
                .iter()
                .find(|n| n.id() == note)
                .unwrap()
                .start()
                .raw(),
            BeatTime::ONE.raw() + step
        );
    });
    editor.update(&mut visual, |v, cx| {
        v.midi_selection.clear();
        v.midi_marker = Some(4 * BeatTime::ONE.raw() - step);
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("right");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        let cursor = v.midi_marker.unwrap();
        assert!(i128::from(cursor) >= v.piano_beat_start);
        assert!(i128::from(cursor) < v.piano_beat_start + i128::from(v.piano_beat_span.raw()));
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .source_length(),
            BeatTime::from_quarters(4).unwrap()
        );
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::PlaceMidiClip {
            track: v.selected_track.unwrap(),
            start: BeatTime::from_quarters(8).unwrap(),
            length: BeatTime::from_quarters(4).unwrap(),
        })));
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, cx| {
        let track = v
            .state
            .as_ref()
            .unwrap()
            .project
            .track(v.selected_track.unwrap())
            .unwrap();
        v.selected_clip = Some(
            track
                .midi_clips()
                .iter()
                .find(|c| c.id() != clip)
                .unwrap()
                .id(),
        );
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert_eq!(v.midi_marker, Some(0)));
}
