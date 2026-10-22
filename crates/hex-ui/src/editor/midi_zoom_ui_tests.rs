//! Real wheel dispatch and selected-source fitting, without musical edits.
use super::creation_tests::{fixture, pump};
use gpui::{ScrollDelta, ScrollWheelEvent, TestAppContext, point, px};
use hex_project::{BeatTime, Command};
use hex_session::service::Request;

#[gpui::test]
fn wheel_zoom_anchors_pointer_and_does_not_mutate_project(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _clip) = fixture(cx);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let at = point(roll.origin.x + roll.size.width * 0.75, roll.center().y);
    visual.simulate_event(ScrollWheelEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.0), px(80.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(
            v.piano_beat_span.raw() < 4 * BeatTime::ONE.raw(),
            "real piano wheel must zoom"
        );
        let anchor = v.piano_beat_start.raw() as f64 + 0.75 * v.piano_beat_span.raw() as f64;
        assert!((anchor / BeatTime::ONE.raw() as f64 - 3.0).abs() < 0.01);
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
}

#[gpui::test]
fn newly_selected_clip_fits_once_and_worker_ack_keeps_manual_zoom(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, original) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::PlaceMidiClip {
            track: v.selected_track.unwrap(),
            start: BeatTime::from_quarters(8).unwrap(),
            length: BeatTime::from_quarters(32).unwrap(),
        })));
    });
    pump(&editor, &mut visual);
    let selected = editor.read_with(&visual, |v, _| {
        assert_ne!(v.selected_clip, Some(original));
        assert_eq!(v.piano_beat_start, BeatTime::ZERO);
        assert_eq!(
            v.piano_beat_span,
            BeatTime::from_quarters(32).unwrap(),
            "selecting a long clip must fit its editable source"
        );
        v.selected_clip.unwrap()
    });
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    visual.simulate_event(ScrollWheelEvent {
        position: roll.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(80.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let zoom = editor.read_with(&visual, |v, _| (v.piano_beat_start, v.piano_beat_span));
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::RenameTrack {
            track: v.selected_track.unwrap(),
            name: "ack".into()
        })))
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.selected_clip, Some(selected));
        assert_eq!(
            (v.piano_beat_start, v.piano_beat_span),
            zoom,
            "same clip ACK must preserve manual zoom"
        );
    });
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            assert!(v.set_selected_clip(v.selected_track.unwrap(), original, window, cx));
            cx.notify();
        })
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.piano_beat_span, BeatTime::from_quarters(4).unwrap())
    });
}

#[gpui::test]
fn wheel_accumulates_precise_motion_and_creation_hits_after_zoom(cx: &mut TestAppContext) {
    use super::creation_tests::double_down;
    use gpui::{Modifiers, MouseButton};
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let at = |fraction| {
        point(
            roll.origin.x + roll.size.width * fraction,
            roll.origin.y + roll.size.height * 0.52,
        )
    };
    for _ in 0..3 {
        visual.simulate_event(ScrollWheelEvent {
            position: at(0.75),
            delta: ScrollDelta::Pixels(point(px(0.0), px(20.0))),
            ..Default::default()
        });
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.piano_beat_span, BeatTime::from_quarters(4).unwrap())
        });
    }
    visual.simulate_event(ScrollWheelEvent {
        position: at(0.75),
        delta: ScrollDelta::Pixels(point(px(0.0), px(20.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let viewport = editor.read_with(&visual, |v, _| (v.piano_beat_start, v.piano_beat_span));
    double_down(&mut visual, at(0.25), Modifiers::default());
    visual.simulate_mouse_move(at(0.5), Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(at(0.5), MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    let (note_center, after) = editor.read_with(&visual, |v, _| {
        assert_eq!((v.piano_beat_start, v.piano_beat_span), viewport);
        let state = v.state.as_ref().unwrap();
        let notes = state.project.midi_source(clip).unwrap().notes();
        assert_eq!(notes.len(), 1);
        assert_eq!(
            notes[0].start(),
            BeatTime::from_raw(BeatTime::ONE.raw() * 3 / 2)
        );
        assert_eq!(
            notes[0].duration(),
            BeatTime::from_raw(BeatTime::ONE.raw() * 3 / 4)
        );
        let middle = notes[0].start().raw() + notes[0].duration().raw() / 2;
        (
            ((middle - viewport.0.raw()) as f64 / viewport.1.raw() as f64) as f32,
            state.project.snapshot(),
        )
    });
    double_down(&mut visual, at(note_center), Modifiers::default());
    visual.simulate_mouse_up(at(note_center), MouseButton::Left, Modifiers::default());
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
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
    });
    // Selection absence must retire the old view owner, rather than keeping its zoom.
    editor.update(&mut visual, |v, cx| {
        v.selected_clip = None;
        cx.notify();
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            assert!(v.set_selected_clip(v.selected_track.unwrap(), clip, window, cx));
            cx.notify();
        })
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.piano_beat_span, BeatTime::from_quarters(4).unwrap())
    });
}
