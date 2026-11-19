//! Real pointer dispatch, worker ACK and canonical history navigation checks.
use super::creation_tests::{double_down, fixture, pump};
use gpui::{Modifiers, MouseButton, TestAppContext, point, px};
use hex_project::{BeatTime, Command};
use hex_session::service::Request;

#[gpui::test]
fn middle_drag_moves_both_axes_without_editing_and_ack_keeps_blank_space(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let before = editor.read_with(&visual, |v, _| {
        let s = v.state.as_ref().unwrap();
        (s.project.snapshot(), s.revision, s.can_undo, s.can_redo)
    });
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let down = roll.center();
    let end = point(
        down.x - (roll.size.width - px(2.0)) * 0.75,
        down.y + roll.size.height * 0.25,
    );
    visual.simulate_mouse_down(down, MouseButton::Middle, Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_pan.gesture.is_some(), "middle press must capture")
    });
    visual.simulate_mouse_move(end, Some(MouseButton::Middle), Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert!(
            v.piano_beat_start > 0,
            "middle move must pan; captured={}",
            v.midi_pan.gesture.is_some()
        )
    });
    visual.run_until_parked();
    visual.simulate_mouse_up(end, MouseButton::Middle, Modifiers::default());
    visual.run_until_parked();
    let viewport = editor.read_with(&visual, |v, _| {
        assert!((v.piano_beat_start as f64 / BeatTime::ONE.raw() as f64 - 3.0).abs() < 0.000001);
        assert_eq!((v.piano_pitch_low, v.piano_pitch_rows), (54, 24));
        let s = v.state.as_ref().unwrap();
        assert_eq!(
            (s.project.snapshot(), s.revision, s.can_undo, s.can_redo),
            before
        );
        assert!(!v.edit_pending());
        (v.piano_beat_start, v.piano_beat_span, v.piano_pitch_low)
    });
    visual.simulate_mouse_down(down, MouseButton::Middle, Modifiers::default());
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::RenameTrack {
            track: v.selected_track.unwrap(),
            name: "pan ack".into(),
        })));
    });
    pump(&editor, &mut visual);
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
    }
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            (v.piano_beat_start, v.piano_beat_span, v.piano_pitch_low),
            viewport
        );
        assert_eq!(v.detail_clip_id(), Some(clip));
        assert!(
            v.midi_pan.gesture.is_none(),
            "ACK retires capture, preserving viewport"
        );
    });
    visual.simulate_mouse_up(down, MouseButton::Middle, Modifiers::default());
    // A creation beyond the short source must use the displayed beat/pitch,
    // grow only through the existing canonical command, and undo in one action.
    let blank = point(roll.origin.x + roll.size.width * 0.75, roll.center().y);
    double_down(&mut visual, blank, Modifiers::default());
    visual.simulate_mouse_up(blank, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let source = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
        assert_eq!(source.notes().len(), 1);
        assert_eq!(
            source.notes()[0].start(),
            BeatTime::from_quarters(6).unwrap()
        );
        assert_eq!(source.notes()[0].pitch(), 65);
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()
                .is_empty()
        )
    });
}

#[gpui::test]
fn pan_mode_intercepts_drawing_and_cancelled_creation_never_commits(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    double_down(&mut visual, roll.center(), Modifiers::default());
    editor.read_with(&visual, |v, _| assert!(v.midi_creation.is_some()));
    // Creation now owns its held left press; a secondary middle press cannot
    // preempt it. End that gesture before starting fresh middle navigation.
    visual.simulate_mouse_down(roll.center(), MouseButton::Middle, Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_creation.is_some());
        assert!(v.midi_pan.gesture.is_none());
        assert!(!v.edit_pending());
    });
    visual.simulate_mouse_up(roll.center(), MouseButton::Middle, Modifiers::default());
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(roll.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_down(roll.center(), MouseButton::Middle, Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_creation.is_none());
        assert!(v.midi_pan.gesture.is_some());
    });
    visual.simulate_mouse_move(
        point(roll.center().x - px(20.0), roll.center().y),
        Some(MouseButton::Middle),
        Modifiers::default(),
    );
    visual.simulate_mouse_up(roll.center(), MouseButton::Middle, Modifiers::default());
    pump(&editor, &mut visual);
    let toggle = visual
        .debug_bounds("toggle pan mode · drag to move time and pitch · middle-drag always pans")
        .unwrap();
    visual.simulate_click(toggle.center(), Modifiers::default());
    visual.simulate_keystrokes("b");
    let down = roll.center();
    let end = point(down.x - roll.size.width * 0.5, down.y - roll.size.height);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_pan.enabled);
        assert!(v.midi_pan.gesture.is_some());
        assert_eq!(v.piano_pitch_low, 24);
        assert!(v.midi_stroke.is_none());
        assert!(v.midi_creation.is_none());
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    let viewport = editor.read_with(&visual, |v, _| {
        assert!(v.midi_pan.gesture.is_none());
        (v.piano_beat_start, v.piano_pitch_low)
    });
    visual.simulate_mouse_move(roll.center(), Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(roll.center(), MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!((v.piano_beat_start, v.piano_pitch_low), viewport);
        assert!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()
                .is_empty()
        );
    });
}

#[gpui::test]
fn pan_bounds_and_focus_cancel_are_view_only_at_pitch_and_time_extremes(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _clip) = fixture(cx);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    for (span, rows) in [
        (BeatTime::ONE.raw() / 16, 1),
        (16 * BeatTime::ONE.raw(), 128),
    ] {
        editor.update(&mut visual, |v, cx| {
            v.piano_beat_span = BeatTime::from_raw(span);
            v.piano_beat_start = 0;
            v.piano_pitch_low = 0;
            v.piano_pitch_rows = rows;
            cx.notify();
        });
        visual.run_until_parked();
        for (delta, expected_low, expected_start) in [
            (1.0e30, 128 - rows, -i128::from(u64::MAX)),
            (-1.0e30, 0, i128::from(u64::MAX - span)),
        ] {
            editor.update(&mut visual, |v, cx| {
                v.piano_beat_start = 0;
                cx.notify();
            });
            visual.run_until_parked();
            visual.simulate_mouse_down(roll.center(), MouseButton::Middle, Modifiers::default());
            let end = roll.center() + point(px(delta), px(delta));
            visual.simulate_mouse_move(end, Some(MouseButton::Middle), Modifiers::default());
            visual.run_until_parked();
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.piano_pitch_low, expected_low);
                assert_eq!(v.piano_beat_start, expected_start);
            });
            visual.simulate_mouse_up(end, MouseButton::Middle, Modifiers::default());
            visual.run_until_parked();
        }
    }
    visual.simulate_mouse_down(roll.center(), MouseButton::Middle, Modifiers::default());
    visual.update(|window, cx| editor.update(cx, |v, _| v.focus.focus(window)));
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert!(v.midi_pan.gesture.is_none()));
    visual.simulate_mouse_down(roll.center(), MouseButton::Middle, Modifiers::default());
    editor.update(&mut visual, |v, cx| {
        v.selected_clip = None;
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(
            v.midi_pan.gesture.is_none(),
            "retire capture even when no midi canvas is rendered"
        )
    });
}

#[gpui::test]
fn panned_note_and_velocity_hits_align_and_draw_can_grow_a_fully_blank_view(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::from_quarters(2).unwrap(),
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 93,
        })))
    });
    pump(&editor, &mut visual);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let down = roll.center();
    let end = point(
        down.x - roll.size.width * 0.25,
        down.y + roll.size.height * 0.25,
    );
    visual.simulate_mouse_down(down, MouseButton::Middle, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Middle), Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Middle, Modifiers::default());
    visual.run_until_parked();
    let note_id = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap()
            .notes()[0]
            .id()
    });
    let note_at = point(
        roll.origin.x + roll.size.width * 0.375,
        roll.origin.y + roll.size.height * (17.5 / 24.0),
    );
    visual.simulate_click(note_at, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_selection.primary(), Some(note_id))
    });
    editor.update(&mut visual, |v, cx| {
        v.midi_selection.clear();
        cx.notify();
    });
    visual.run_until_parked();
    let lane = visual.debug_bounds("midi-velocity-lane").unwrap();
    let velocity_at = point(
        lane.origin.x + lane.size.width * 0.25,
        lane.origin.y + lane.size.height * (1.0 - 93.0 / 127.0),
    );
    visual.simulate_click(velocity_at, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_selection.primary(), Some(note_id))
    });
    editor.update(&mut visual, |v, cx| {
        v.pan_piano(true);
        v.pan_piano(true);
        v.pan_piano(true);
        v.midi_draw_mode = true;
        cx.notify();
    });
    visual.run_until_parked();
    let before_length = editor.read_with(&visual, |v, _| {
        let length = v
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap()
            .source_length();
        assert!(v.piano_beat_start >= i128::from(length.raw()));
        length
    });
    visual.simulate_mouse_down(roll.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(roll.center(), MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let source = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
        assert_eq!(source.notes().len(), 2);
        assert!(source.source_length() > before_length);
        assert!(
            source
                .notes()
                .iter()
                .any(|note| note.start() >= before_length)
        );
    });
}
