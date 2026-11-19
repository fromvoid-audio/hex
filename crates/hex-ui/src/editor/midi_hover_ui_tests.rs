//! Passive pointer feedback through real rendered note geometry and Service ACKs.
use super::creation_tests::{double_down, fixture, pump};
use gpui::{CursorStyle, Modifiers, MouseButton, TestAppContext, point, px};
use hex_project::{BeatTime, Command};
use hex_session::service::Request;

#[gpui::test]
fn passive_note_hover_tracks_both_edges_and_clears_without_editing(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let down = point(roll.origin.x + roll.size.width * 0.25, roll.center().y);
    double_down(&mut visual, down, Modifiers::default());
    visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    let (before, selected, pitch, start, duration, low, rows) =
        editor.read_with(&visual, |v, _| {
            let state = v.state.as_ref().unwrap();
            let note = &state.project.midi_source(clip).unwrap().notes()[0];
            (
                state.project.snapshot(),
                v.midi_selection.shared_ids(),
                note.pitch(),
                note.start(),
                note.duration(),
                v.piano_pitch_low,
                v.piano_pitch_rows,
            )
        });
    let left =
        roll.origin.x + roll.size.width * (start.raw() as f32 / (4 * BeatTime::ONE.raw()) as f32);
    let width = roll.size.width * (duration.raw() as f32 / (4 * BeatTime::ONE.raw()) as f32);
    let y = roll.origin.y
        + roll.size.height * ((f32::from(low + rows - 1 - pitch) + 0.5) / f32::from(rows));
    for (x, cursor) in [
        (left + width * 0.5, CursorStyle::OpenHand),
        (left + px(1.0), CursorStyle::ResizeLeft),
        (left + width - px(1.0), CursorStyle::ResizeRight),
    ] {
        visual.simulate_mouse_move(point(x, y), None, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(visual.rendered_cursor_style(), Some(cursor));
        visual.simulate_mouse_down(point(x, y), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let active_cursor = if cursor == CursorStyle::OpenHand {
            CursorStyle::ClosedHand
        } else {
            cursor
        };
        assert_eq!(visual.rendered_cursor_style(), Some(active_cursor));
        visual.simulate_mouse_up(point(x, y), MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
    }
    visual.simulate_mouse_move(
        roll.origin - point(px(2.0), px(2.0)),
        None,
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert!(v.midi_hover.is_none()));
    visual.simulate_mouse_move(
        roll.origin + point(px(8.0), px(8.0)),
        None,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(!matches!(
        visual.rendered_cursor_style(),
        Some(CursorStyle::OpenHand | CursorStyle::ResizeLeft | CursorStyle::ResizeRight)
    ));
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert_eq!(v.midi_selection.ids(), selected.as_ref());
        assert!(!v.edit_pending());
    });

    // A real clip replacement at the same pointer position must not retain a target.
    visual.simulate_mouse_move(point(left + width * 0.5, y), None, Modifiers::default());
    visual.run_until_parked();
    editor.update(&mut visual, |v, _| {
        let track = v.selected_track.unwrap();
        assert!(v.enqueue(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::from_quarters(4).unwrap(),
            length: BeatTime::from_quarters(4).unwrap(),
        })));
    });
    pump(&editor, &mut visual);
    assert!(!matches!(
        visual.rendered_cursor_style(),
        Some(CursorStyle::OpenHand | CursorStyle::ResizeLeft | CursorStyle::ResizeRight)
    ));
}

#[gpui::test]
fn tiny_note_hover_and_hits_follow_time_pitch_zoom_and_draw_mode(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::from_quarters(2).unwrap(),
            duration: BeatTime::from_raw(BeatTime::ONE.raw() / 512),
            pitch: 65,
            channel: 0,
            velocity: 100,
        })));
    });
    pump(&editor, &mut visual);
    let (before, note) = editor.read_with(&visual, |v, _| {
        (
            v.state.as_ref().unwrap().project.snapshot(),
            v.midi_selection.primary().unwrap(),
        )
    });
    editor.update(&mut visual, |v, cx| {
        // The unselected halo must still select/move. Explicitly selected
        // tiny-edge resizing is exercised separately by resize_tests.
        v.midi_selection.clear();
        v.piano_beat_span = BeatTime::from_quarters(16).unwrap();
        cx.notify();
    });
    visual.run_until_parked();
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let tiny = editor.read_with(&visual, |v, _| {
        point(
            roll.origin.x + roll.size.width * (2.0 / 16.0),
            roll.origin.y
                + roll.size.height
                    * ((f32::from(v.piano_pitch_low + v.piano_pitch_rows - 1 - 65) + 0.5)
                        / f32::from(v.piano_pitch_rows)),
        )
    });
    visual.simulate_mouse_move(tiny, None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::OpenHand));
    visual.simulate_mouse_down(tiny, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_selection.primary(), Some(note));
        assert!(matches!(
            v.midi_note_gesture.unwrap().kind,
            super::midi_drag::GestureKind::Move { .. }
        ));
    });
    visual.simulate_mouse_up(tiny, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, cx| {
        v.piano_beat_span = BeatTime::from_raw(BeatTime::ONE.raw() / 16);
        v.piano_beat_start = i128::from(2 * BeatTime::ONE.raw() - v.piano_beat_span.raw() / 4);
        v.piano_pitch_low = 59;
        v.piano_pitch_rows = 12;
        cx.notify();
    });
    visual.run_until_parked();
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let edge = point(
        roll.origin.x + roll.size.width * 0.25 + px(1.0),
        roll.origin.y + roll.size.height * (5.5 / 12.0),
    );
    visual.simulate_mouse_move(edge, None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(CursorStyle::ResizeLeft)
    );
    editor.update(&mut visual, |v, cx| {
        v.midi_draw_mode = true;
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::Crosshair));
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert_eq!(v.midi_selection.primary(), Some(note));
    });
}
