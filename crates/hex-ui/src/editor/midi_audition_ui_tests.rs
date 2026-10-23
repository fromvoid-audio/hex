//! Explicit rendered input is the only audition trigger; the real worker owns preparation.
use super::creation_tests::{fixture, pump};
use gpui::{
    Entity, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, MouseButton, TestAppContext,
    VisualTestContext, point,
};
use hex_project::{BeatTime, Command, MidiNoteInsert};
use hex_session::service::Request;
use std::sync::Arc;
const AUDITION: &str =
    "toggle note audition · selection plays up to 17 notes · physical keyboard takes priority";
fn authored(
    cx: &mut TestAppContext,
) -> (
    VisualTestContext,
    Entity<super::EditorView>,
    Arc<super::super::test_environment::Environment>,
    hex_project::ClipId,
) {
    let (mut visual, editor, environment, clip) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::InsertInstrument {
            track: v.selected_track.unwrap(),
            state: hex_project::default_instrument_state().unwrap(),
        })))
    });
    pump(&editor, &mut visual);
    for (beat, pitch, velocity) in [(1, 60, 37), (3, 64, 93)] {
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::from_quarters(beat).unwrap(),
                duration: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
                pitch,
                channel: 0,
                velocity,
            })))
        });
        pump(&editor, &mut visual);
    }
    (visual, editor, environment, clip)
}
fn position(
    visual: &mut VisualTestContext,
    editor: &Entity<super::EditorView>,
    beat: f64,
    pitch: u8,
) -> gpui::Point<gpui::Pixels> {
    let bounds = visual.debug_bounds("midi-note-roll").unwrap();
    editor.read_with(visual, |v, _| {
        point(
            bounds.origin.x
                + bounds.size.width
                    * (((beat * BeatTime::ONE.raw() as f64) - v.piano_beat_start.raw() as f64)
                        / v.piano_beat_span.raw() as f64) as f32,
            bounds.origin.y
                + bounds.size.height
                    * ((f32::from(v.piano_pitch_low + v.piano_pitch_rows - 1 - pitch) + 0.5)
                        / f32::from(v.piano_pitch_rows)),
        )
    })
}
fn toggle(visual: &mut VisualTestContext) {
    let button = visual.debug_bounds(AUDITION).unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
}
#[gpui::test]
fn click_chord_and_marquee_prepare_once_with_exact_velocities_but_disabled_paint_and_ack_do_not(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, _) = authored(cx);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let a = position(&mut visual, &editor, 1.125, 60);
    let b = position(&mut visual, &editor, 3.125, 64);
    visual.simulate_click(a, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| assert!(v.computer_keyboard.input.is_none()));
    toggle(&mut visual);
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(
            v.computer_keyboard.input.is_none(),
            "enable waits for explicit selection"
        )
    });
    visual.simulate_click(b, Modifiers::default());
    pump(&editor, &mut visual);
    let (single, request) = editor.read_with(&visual, |v, _| {
        let input = v.computer_keyboard.input.as_ref().unwrap();
        assert!(input.is_note_audition());
        assert_eq!((input.key_word(0).unwrap() >> 8) & 255, 93);
        (Arc::clone(input), v.computer_keyboard.request_id)
    });
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        pump(&editor, &mut visual);
    }
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.computer_keyboard.request_id, request);
        assert!(Arc::ptr_eq(
            &single,
            v.computer_keyboard.input.as_ref().unwrap()
        ));
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };
    visual.simulate_click(a, shift);
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let input = v.computer_keyboard.input.as_ref().unwrap();
        assert_eq!((input.key_word(0).unwrap() >> 8) & 255, 37);
        assert_eq!((input.key_word(1).unwrap() >> 8) & 255, 93);
    });
    visual.simulate_keystrokes("escape");
    visual.simulate_click(b, Modifiers::default());
    pump(&editor, &mut visual);
    let request = editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_selection.len(), 1);
        v.computer_keyboard.request_id
    });
    let start = position(&mut visual, &editor, 0.5, 65);
    let end = position(&mut visual, &editor, 3.5, 59);
    visual.simulate_mouse_down(start, MouseButton::Left, shift);
    visual.simulate_mouse_move(end, Some(MouseButton::Left), shift);
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.computer_keyboard.request_id, request)
    });
    visual.simulate_mouse_up(end, MouseButton::Left, shift);
    pump(&editor, &mut visual);
    let chord = editor.read_with(&visual, |v, _| {
        assert_eq!(v.computer_keyboard.request_id, request + 1);
        assert_eq!(v.midi_selection.len(), 2);
        Arc::clone(v.computer_keyboard.input.as_ref().unwrap())
    });
    // Explicitly committing the same marquee is one new intent, never a paint trigger.
    let request = editor.read_with(&visual, |v, _| v.computer_keyboard.request_id);
    visual.simulate_mouse_down(start, MouseButton::Left, shift);
    visual.simulate_mouse_move(end, Some(MouseButton::Left), shift);
    visual.simulate_mouse_up(end, MouseButton::Left, shift);
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.computer_keyboard.request_id, request + 1)
    });
    toggle(&mut visual);
    pump(&editor, &mut visual);
    assert!(chord.is_cancelled());
    editor.read_with(&visual, |v, _| {
        assert!(v.computer_keyboard.input.is_none());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
}
#[gpui::test]
fn physical_keys_defer_the_latest_selection_and_stale_preparation_or_focus_cannot_trigger(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, _) = authored(cx);
    visual.simulate_keystrokes("m");
    pump(&editor, &mut visual);
    toggle(&mut visual);
    visual.simulate_event(KeyDownEvent {
        keystroke: Keystroke::parse("a").unwrap(),
        is_held: false,
    });
    pump(&editor, &mut visual);
    let performance = editor.read_with(&visual, |v, _| {
        Arc::clone(v.computer_keyboard.input.as_ref().unwrap())
    });
    let b = position(&mut visual, &editor, 3.125, 64);
    visual.simulate_click(b, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(Arc::ptr_eq(
            &performance,
            v.computer_keyboard.input.as_ref().unwrap()
        ));
        assert_eq!(v.computer_keyboard.physical_notes().count(), 1);
    });
    visual.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("a").unwrap(),
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(
            v.computer_keyboard
                .input
                .as_ref()
                .unwrap()
                .is_note_audition()
        )
    });
    // Hold a physical key while changing selection, so preparation cannot race
    // the focus-loss boundary before this test takes custody of its real event.
    visual.simulate_event(KeyDownEvent {
        keystroke: Keystroke::parse("a").unwrap(),
        is_held: false,
    });
    pump(&editor, &mut visual);
    let a = position(&mut visual, &editor, 1.125, 60);
    visual.simulate_click(a, Modifiers::default());
    pump(&editor, &mut visual);
    let (old, prepared) = editor.update(&mut visual, |v, _| {
        assert!(v.computer_keyboard_up("a"));
        assert_eq!(v.tick_midi_audition(), Some(true));
        let input = Arc::clone(v.computer_keyboard.input.as_ref().unwrap());
        let request = v.computer_keyboard.request_id;
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let event = loop {
            if let Ok(event) = v.service.try_event() {
                assert_eq!(event.computer_keyboard_complete, Some(request));
                assert!(event.error.is_none(), "{:?}", event.error);
                assert!(event.playback.is_some(), "real preparation must succeed");
                break event;
            }
            assert!(std::time::Instant::now() < until);
            std::thread::yield_now();
        };
        for key in 0..17 {
            assert_eq!(input.key_word(key), Some(0));
        }
        (input, event)
    });
    visual.update(|window, cx| editor.read_with(cx, |v, _| v.focus.focus(window)));
    visual.run_until_parked();
    assert!(old.is_cancelled());
    for key in 0..17 {
        assert_eq!(old.key_word(key), Some(0));
    }
    // Deliver the actual worker completion only after cancellation, then settle
    // subsequent frames: a stale completion must never resurrect or trigger it.
    editor.update(&mut visual, |v, _| v.receive(prepared));
    pump(&editor, &mut visual);
    for key in 0..17 {
        assert_eq!(old.key_word(key), Some(0));
    }
    editor.read_with(&visual, |v, _| assert!(v.computer_keyboard.input.is_none()));
}
#[gpui::test]
fn over_capacity_selection_refuses_the_whole_chord_without_project_changes(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = authored(cx);
    editor.update(&mut visual, |v, _| {
        assert!(
            v.enqueue(Request::Edit(Command::EditMidiNotesWithSourceExtent {
                clip,
                remove: Vec::new(),
                insert: (0..18)
                    .map(|index| MidiNoteInsert {
                        start: BeatTime::ZERO,
                        duration: BeatTime::ONE,
                        pitch: 48 + index,
                        channel: 0,
                        velocity: 37,
                    })
                    .collect(),
            }))
        )
    });
    pump(&editor, &mut visual);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    toggle(&mut visual);
    visual.simulate_keystrokes("cmd-a");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_selection.len() > 17);
        assert!(v.error.as_deref().unwrap().contains("17"));
        assert!(v.computer_keyboard.input.is_none());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
}

#[gpui::test]
fn clip_owner_wins_over_track_selection_and_stop_or_source_loss_cannot_retrigger(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = authored(cx);
    let owner = editor.read_with(&visual, |v, _| v.selected_track.unwrap());
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::InsertTrackWithDevice {
            name: "other".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })));
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, cx| {
        v.selected_clip = Some(clip);
        v.clip_view_open = true;
        v.selected_track = v
            .state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .iter()
            .find(|t| t.id() != owner)
            .map(|t| t.id());
        cx.notify();
    });
    visual.run_until_parked();
    toggle(&mut visual);
    let a = position(&mut visual, &editor, 1.125, 60);
    visual.simulate_click(a, Modifiers::default());
    pump(&editor, &mut visual);
    let input = editor.read_with(&visual, |v, _| {
        assert_eq!(v.computer_keyboard.target.unwrap().2, owner);
        let input = v.computer_keyboard.input.as_ref().unwrap();
        assert_ne!(input.key_word(0), Some(0));
        Arc::clone(input)
    });
    editor.update(&mut visual, |v, _| v.stop_computer_keyboard());
    pump(&editor, &mut visual);
    assert!(input.is_cancelled());
    editor.read_with(&visual, |v, _| assert!(v.computer_keyboard.input.is_none()));
    visual.simulate_click(a, Modifiers::default());
    let pending = editor.update(&mut visual, |v, cx| {
        v.tick(cx);
        let input = Arc::clone(v.computer_keyboard.input.as_ref().unwrap());
        v.project_generation += 1;
        input
    });
    pump(&editor, &mut visual);
    assert!(pending.is_cancelled());
    assert_eq!(pending.key_word(0), Some(0));
    editor.read_with(&visual, |v, _| assert!(v.computer_keyboard.input.is_none()));
}
