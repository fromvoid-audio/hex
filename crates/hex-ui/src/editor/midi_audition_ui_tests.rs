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
    "toggle note audition · selection plays up to 17 notes · live midi stays connected";
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
                    * (((beat * BeatTime::ONE.raw() as f64) - v.piano_beat_start as f64)
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
    let button_bounds = visual.debug_bounds(AUDITION).unwrap();
    assert!(visual.debug_bounds("midi-audition-speaker").is_some());
    let disabled_fill = visual.rendered_background(AUDITION).unwrap();
    let a = position(&mut visual, &editor, 1.125, 60);
    let b = position(&mut visual, &editor, 3.125, 64);
    visual.simulate_click(a, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| assert!(v.computer_keyboard.input.is_none()));
    toggle(&mut visual);
    assert_eq!(visual.debug_bounds(AUDITION), Some(button_bounds));
    assert_ne!(visual.rendered_background(AUDITION), Some(disabled_fill));
    assert!(visual.debug_bounds("midi-audition-speaker").is_some());
    editor.read_with(&visual, |v, _| assert!(v.midi_audition.enabled));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(
            v.computer_keyboard.input.is_none(),
            "enable waits for explicit selection"
        )
    });
    // A note click from Arrangement focus must retain the new piano intent.
    visual.update(|window, cx| editor.read_with(cx, |v, _| v.focus.focus(window)));
    visual.run_until_parked();
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
    assert_eq!(visual.debug_bounds(AUDITION), Some(button_bounds));
    editor.read_with(&visual, |v, _| assert!(!v.midi_audition.enabled));
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
    let [start, one, both] = marquee_points(&mut visual, &editor, false);
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(start, MouseButton::Left, shift);
    for destination in [one, both] {
        visual.simulate_mouse_move(destination, Some(MouseButton::Left), shift);
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(Arc::ptr_eq(
                &performance,
                v.computer_keyboard.input.as_ref().unwrap()
            ))
        });
    }
    visual.simulate_mouse_up(both, MouseButton::Left, shift);
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
    assert_audition_bank(&editor, &visual, &[(64, 93)]);
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
        let until = crate::test_wait::hang_deadline();
        let event = loop {
            if let Ok(event) = v.service.try_event() {
                assert_eq!(event.computer_keyboard_complete, Some(request));
                assert!(event.error.is_none(), "{:?}", event.error);
                assert!(event.playback.is_some(), "real preparation must succeed");
                break event;
            }
            assert!(std::time::Instant::now() < until, "physical_keys_defer_the_latest_selection_and_stale_preparation_or_focus_cannot_trigger: hang guard expired");
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
                        active: true,
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
        // Exercise the fast-ACK case deterministically: a completed preparation
        // may have populated notes before source loss cancels this input.
        if input.key_word(0) == Some(0) {
            input.trigger_note_audition().unwrap();
        }
        assert_ne!(input.key_word(0), Some(0));
        v.project_generation += 1;
        input
    });
    pump(&editor, &mut visual);
    assert!(pending.is_cancelled());
    // Cancellation is terminal even when a fast ACK already populated notes;
    // the raw key slots need not be zero for the renderer to remain silent.
    assert!(pending.trigger_note_audition().is_err());
    editor.read_with(&visual, |v, _| assert!(v.computer_keyboard.input.is_none()));
}

fn marquee_points(
    visual: &mut VisualTestContext,
    editor: &Entity<super::EditorView>,
    velocity: bool,
) -> [gpui::Point<gpui::Pixels>; 3] {
    if velocity {
        let b = visual.debug_bounds("midi-velocity-lane").unwrap();
        [
            point(b.left() + b.size.width * 0.125, b.top() + gpui::px(2.0)),
            point(b.left() + b.size.width * 0.375, b.top() + gpui::px(54.0)),
            point(b.left() + b.size.width * 0.875, b.top() + gpui::px(54.0)),
        ]
    } else {
        [
            position(visual, editor, 0.5, 65),
            position(visual, editor, 1.5, 59),
            position(visual, editor, 3.5, 59),
        ]
    }
}
#[gpui::test]
fn configured_controller_keeps_its_root_during_note_and_marquee_audition(cx: &mut TestAppContext) {
    use hex_engine::{
        PreparedRevision, SineTable,
        computer_keyboard::{ComputerKeyboardInput, LiveNoteEvent},
        playback::Playback,
    };

    let (mut visual, editor, _environment, _) = authored(cx);
    // Settle the Arrangement-to-detail focus handoff before admitting input.
    visual.update(|window, cx| editor.read_with(cx, |v, _| v.midi_focus.focus(window)));
    visual.run_until_parked();
    let input = Arc::new(ComputerKeyboardInput::default().with_live_notes().unwrap());
    let mut playback = editor.update(&mut visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        let track = v.selected_track.unwrap();
        let revision = Arc::new(
            PreparedRevision::prepare(
                &state.project,
                &Default::default(),
                Arc::new(SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
        playback
            .prepare_computer_keyboard(&state.project, track, Arc::clone(&input))
            .unwrap();
        v.computer_keyboard.target = Some((v.project_generation, state.revision, track));
        v.computer_keyboard.input = Some(Arc::clone(&input));
        v.computer_keyboard.prepared = true;
        v.computer_keyboard.suspended = false;
        v.set_test_audition_port();
        playback
    });
    let live = input.live_notes().unwrap();
    let activation = input.begin_live_notes_activation_on_control().unwrap();
    let mut render = || {
        let mut frames = [[0.0; 2]; 128];
        playback.render(&mut frames).unwrap();
        frames
    };
    let assert_route = |visual: &VisualTestContext, step| {
        editor.read_with(visual, |v, _| {
            assert!(
                v.controller_wants_live_notes(),
                "{step}: controller deselected, {:?}",
                v.error
            );
            assert!(
                !input.is_cancelled(),
                "{step}: input cancelled, {:?}",
                v.error
            );
            assert!(
                v.computer_keyboard.prepared,
                "{step}: input no longer prepared, {:?}",
                v.error
            );
            assert!(
                v.computer_keyboard
                    .input
                    .as_ref()
                    .is_some_and(|root| Arc::ptr_eq(root, &input)),
                "{step}: input replaced"
            );
        })
    };
    let a = position(&mut visual, &editor, 1.125, 60);
    visual.simulate_click(a, Modifiers::default());
    pump(&editor, &mut visual);
    assert_route(&visual, "disabled note");
    assert_eq!(render(), [[0.0; 2]; 128]);

    toggle(&mut visual);
    assert_route(&visual, "enabled toggle");
    let b = position(&mut visual, &editor, 3.125, 64);
    visual.simulate_click(b, Modifiers::default());
    pump(&editor, &mut visual);
    assert_route(&visual, "enabled note");
    assert!(render().iter().flatten().any(|sample| *sample != 0.0));

    for _ in 0..128 {
        render();
    }
    assert_eq!(render(), [[0.0; 2]; 128]);

    let [start, _, both] = marquee_points(&mut visual, &editor, false);
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };
    visual.simulate_mouse_down(start, MouseButton::Left, shift);
    visual.simulate_mouse_move(both, Some(MouseButton::Left), shift);
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(Arc::ptr_eq(
            v.computer_keyboard.input.as_ref().unwrap(),
            &input
        ));
        assert!(v.midi_selection.len() >= 2);
    });
    assert!(
        render().iter().flatten().any(|sample| *sample != 0.0),
        "marquee preview"
    );
    live.publish(
        activation,
        LiveNoteEvent::NoteOn {
            channel: 2,
            pitch: 67,
            velocity: 91,
        },
    )
    .unwrap();
    assert!(render().iter().flatten().any(|sample| *sample != 0.0));
    visual.simulate_mouse_up(both, MouseButton::Left, shift);
    toggle(&mut visual);
    for _ in 0..64 {
        render();
    }
    assert!(
        render().iter().flatten().any(|sample| *sample != 0.0),
        "held hardware note"
    );
    live.publish(
        activation,
        LiveNoteEvent::NoteOff {
            channel: 2,
            pitch: 67,
            velocity: 0,
        },
    )
    .unwrap();
    for _ in 0..64 {
        render();
    }
    assert_eq!(render(), [[0.0; 2]; 128]);
    assert!(live.same_current_activation(activation));
    assert!(!input.is_cancelled());
}

fn assert_audition_bank(
    editor: &Entity<super::EditorView>,
    visual: &VisualTestContext,
    expected: &[(u8, u8)],
) -> Arc<hex_engine::computer_keyboard::ComputerKeyboardInput> {
    editor.read_with(visual, |v, _| {
        let input = v
            .computer_keyboard
            .input
            .as_ref()
            .expect("real worker audition input");
        assert!(input.is_note_audition());
        let bank = (0..hex_engine::computer_keyboard::MAX_NOTE_AUDITION_NOTES)
            .filter_map(|slot| {
                let word = input.key_word(slot).unwrap();
                if word == 0 {
                    None
                } else {
                    assert!((1..=128).contains(&(word & 255)) && word >> 16 != 0);
                    assert!((1..=127).contains(&((word >> 8) as u8)));
                    Some(((word as u8) - 1, (word >> 8) as u8))
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(bank, expected);
        Arc::clone(input)
    })
}
#[gpui::test]
fn held_marquees_select_and_audition_only_new_contacts_without_release_or_repaint_retrigger(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = authored(cx);
    let (baseline, first, before) = editor.read_with(&visual, |v, _| {
        let source = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
        (
            source.notes()[1].id(),
            source.notes()[0].id(),
            v.state.as_ref().unwrap().project.snapshot(),
        )
    });
    toggle(&mut visual);
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    for velocity in [false, true] {
        editor.update(&mut visual, |v, cx| {
            v.midi_selection.set_single(baseline);
            cx.notify();
        });
        visual.run_until_parked();
        let [start, one, both] = marquee_points(&mut visual, &editor, velocity);
        visual.simulate_mouse_down(start, MouseButton::Left, shift);
        visual.simulate_mouse_move(one, Some(MouseButton::Left), shift);
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.midi_selection.ids(), [first, baseline]);
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
        pump(&editor, &mut visual); // Real preparation completes while capture stays held.
        let first_input = assert_audition_bank(&editor, &visual, &[(60, 37)]);
        let request = editor.read_with(&visual, |v, _| v.computer_keyboard.request_id);
        for _ in 0..3 {
            visual.simulate_mouse_move(
                one + point(gpui::px(1.0), gpui::px(0.0)),
                Some(MouseButton::Left),
                shift,
            );
            editor.update(&mut visual, |_, cx| cx.notify());
            visual.run_until_parked();
            pump(&editor, &mut visual);
        }
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.computer_keyboard.request_id, request);
            assert!(v.midi_marquee.is_some() || v.velocity_marquee_active());
            assert!(Arc::ptr_eq(
                &first_input,
                v.computer_keyboard.input.as_ref().unwrap()
            ));
        });
        visual.simulate_mouse_move(both, Some(MouseButton::Left), shift);
        pump(&editor, &mut visual);
        let second_input = assert_audition_bank(&editor, &visual, &[(64, 93)]);
        assert!(first_input.is_cancelled());
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.computer_keyboard.request_id, request + 1)
        });
        visual.simulate_mouse_move(one, Some(MouseButton::Left), shift);
        pump(&editor, &mut visual);
        assert!(
            second_input.is_cancelled(),
            "leaving revokes the finite chord even when that pitch belongs to the additive baseline"
        );
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.computer_keyboard.request_id, request + 1)
        });
        visual.simulate_mouse_move(both, Some(MouseButton::Left), shift);
        pump(&editor, &mut visual);
        let reentered = assert_audition_bank(&editor, &visual, &[(64, 93)]);
        visual.simulate_mouse_up(both, MouseButton::Left, shift);
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.computer_keyboard.request_id, request + 2);
            assert!(Arc::ptr_eq(
                &reentered,
                v.computer_keyboard.input.as_ref().unwrap()
            ));
            assert_eq!(v.midi_selection.ids(), [first, baseline]);
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
    }
}
#[gpui::test]
fn canceling_held_marquee_revokes_unreceived_real_preparation_and_restores_baseline(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, _) = authored(cx);
    let baseline = editor.read_with(&visual, |v, _| v.midi_selection.shared_ids());
    toggle(&mut visual);
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    for velocity in [false, true] {
        let [start, one, both] = marquee_points(&mut visual, &editor, velocity);
        visual.simulate_mouse_down(start, MouseButton::Left, shift);
        visual.simulate_mouse_move(one, Some(MouseButton::Left), shift);
        visual.run_until_parked();
        let input = editor.update(&mut visual, |v, _| {
            assert_eq!(v.tick_midi_audition(), Some(true));
            assert!(v.edit_pending());
            Arc::clone(v.computer_keyboard.input.as_ref().unwrap())
        });
        visual.simulate_mouse_move(both, Some(MouseButton::Left), shift);
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert!(v.edit_pending());
            assert!(
                v.midi_marquee.is_some() || v.velocity_marquee_active(),
                "its own pending preparation must not cancel live capture"
            );
        });
        // Deliberately leave the actual Service terminal unreceived until after cancel.
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        assert!(input.is_cancelled());
        pump(&editor, &mut visual);
        visual.simulate_mouse_up(one, MouseButton::Left, shift);
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.midi_selection.ids(), baseline.as_ref());
            assert!(v.computer_keyboard.input.is_none());
            assert!(!v.velocity_marquee_active());
            assert!(v.midi_marquee.is_none());
        });
    }
}
#[gpui::test]
fn marquee_audition_off_is_silent_and_new_contact_overflow_refuses_whole_batch(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = authored(cx);
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    let [start, one, _] = marquee_points(&mut visual, &editor, false);
    visual.simulate_mouse_down(start, MouseButton::Left, shift);
    visual.simulate_mouse_move(one, Some(MouseButton::Left), shift);
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_selection.len(), 2);
        assert!(v.computer_keyboard.input.is_none());
        assert_eq!(v.computer_keyboard.request_id, 0);
    });
    visual.simulate_mouse_up(one, MouseButton::Left, shift);
    visual.run_until_parked();
    editor.update(&mut visual, |v, _| {
        assert!(
            v.enqueue(Request::Edit(Command::AddMidiNotes {
                clip,
                notes: (36..54)
                    .map(|pitch| MidiNoteInsert {
                        active: true,
                        start: BeatTime::ONE,
                        duration: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
                        pitch,
                        channel: 0,
                        velocity: 100
                    })
                    .collect()
            }))
        )
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, cx| {
        v.piano_pitch_low = 24;
        v.piano_pitch_rows = 72;
        cx.notify();
    });
    visual.run_until_parked();
    toggle(&mut visual);
    let start = position(&mut visual, &editor, 0.5, 54);
    let end = position(&mut visual, &editor, 1.5, 35);
    visual.simulate_mouse_down(start, MouseButton::Left, shift);
    visual.simulate_mouse_move(end, Some(MouseButton::Left), shift);
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_selection.len(), 18);
        assert!(v.computer_keyboard.input.is_none());
        assert_eq!(v.computer_keyboard.request_id, 0);
        assert!(v.error.as_deref().unwrap().contains("at most 17"));
    });
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(end, MouseButton::Left, shift);
    visual.run_until_parked();
}

#[gpui::test]
fn explicit_note_actions_adopt_live_membership_and_retire_both_pointer_owners(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = authored(cx);
    let (baseline, before) = editor.read_with(&visual, |v, _| {
        (
            v.midi_selection.primary().unwrap(),
            v.state.as_ref().unwrap().project.snapshot(),
        )
    });
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    for group in [false, true] {
        for velocity in [false, true] {
            editor.update(&mut visual, |v, cx| {
                if group {
                    v.midi_selection.set_single(baseline);
                } else {
                    v.midi_selection.clear();
                }
                cx.notify();
            });
            visual.run_until_parked();
            let [start, one, _] = marquee_points(&mut visual, &editor, velocity);
            visual.simulate_mouse_down(start, MouseButton::Left, shift);
            visual.simulate_mouse_move(one, Some(MouseButton::Left), shift);
            visual.run_until_parked();
            visual.simulate_keystrokes("right");
            pump(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.midi_selection.len(), if group { 2 } else { 1 });
                assert!(v.midi_marquee.is_none());
                assert!(!v.velocity_marquee_active());
                let notes = v
                    .state
                    .as_ref()
                    .unwrap()
                    .project
                    .midi_source(clip)
                    .unwrap()
                    .notes();
                assert_eq!(
                    notes.iter().map(|n| n.start().raw()).collect::<Vec<_>>(),
                    [
                        5 * BeatTime::ONE.raw() / 4,
                        if group {
                            13 * BeatTime::ONE.raw() / 4
                        } else {
                            3 * BeatTime::ONE.raw()
                        }
                    ]
                );
            });
            visual.simulate_mouse_up(one, MouseButton::Left, shift);
            editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
            pump(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
            });
            // Even a selection action that keeps identical IDs/primary retires capture.
            visual.simulate_keystrokes("cmd-a");
            visual.run_until_parked();
            visual.simulate_mouse_down(start, MouseButton::Left, shift);
            visual.simulate_mouse_move(one, Some(MouseButton::Left), shift);
            visual.run_until_parked();
            visual.simulate_keystrokes("cmd-a");
            visual.run_until_parked();
            editor.read_with(&visual, |v, _| {
                assert!(v.midi_marquee.is_none());
                assert!(!v.velocity_marquee_active());
                assert_eq!(v.midi_selection.len(), 2);
            });
            visual.simulate_mouse_up(one, MouseButton::Left, shift);
            visual.run_until_parked();
        }
    }
}

#[gpui::test]
fn real_project_replacement_cancels_live_marquee_without_reviving_old_selection_or_input(
    cx: &mut TestAppContext,
) {
    for velocity in [false, true] {
        let (mut visual, editor, _environment, _) = authored(cx);
        toggle(&mut visual);
        let [start, one, _] = marquee_points(&mut visual, &editor, velocity);
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(start, MouseButton::Left, shift);
        visual.simulate_mouse_move(one, Some(MouseButton::Left), shift);
        pump(&editor, &mut visual);
        let input = assert_audition_bank(&editor, &visual, &[(60, 37)]);
        let generation = editor.update(&mut visual, |v, _| {
            let generation = v.project_generation;
            assert!(v.enqueue(Request::NewWithAudioMode {
                mode: hex_session::DocumentAudioMode::Resident,
                reservation: None
            }));
            generation
        });
        pump(&editor, &mut visual);
        assert!(input.is_cancelled());
        visual.simulate_mouse_up(one, MouseButton::Left, shift);
        for _ in 0..3 {
            editor.update(&mut visual, |_, cx| cx.notify());
            visual.run_until_parked();
            pump(&editor, &mut visual);
        }
        editor.read_with(&visual, |v, _| {
            assert!(v.project_generation > generation);
            assert!(v.state.as_ref().unwrap().project.tracks().is_empty());
            assert!(v.midi_selection.is_empty());
            assert!(v.midi_marquee.is_none());
            assert!(!v.velocity_marquee_active());
            assert!(v.computer_keyboard.input.is_none());
        });
    }
}
