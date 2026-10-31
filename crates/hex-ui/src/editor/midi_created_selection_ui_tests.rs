//! Rendered creation/stroke selection follows actual canonical identities and ACK custody.
use super::creation_tests::{fixture, pump};
use gpui::{
    Entity, Modifiers, MouseButton, MouseDownEvent, TestAppContext, VisualTestContext, point,
};
use hex_project::{BeatTime, Command, Created};
use hex_session::service::{Event, Request};
fn next(editor: &Entity<super::super::EditorView>, visual: &VisualTestContext) -> Event {
    let until = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = editor.read_with(visual, |v, _| v.service.try_event()) {
            return event;
        }
        assert!(
            std::time::Instant::now() < until,
            "next: hang guard expired"
        );
        std::thread::yield_now();
    }
}
#[gpui::test]
fn committed_draw_selects_actual_authored_ids_with_last_primary_after_later_frames(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let at = |f| point(roll.origin.x + roll.size.width * f, roll.center().y);
    visual.simulate_keystrokes("b");
    visual.simulate_mouse_down(at(0.25), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(at(0.5), Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(at(0.5), MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    let ids = editor.read_with(&visual, |v, _| {
        let mut ids: Vec<_> = v
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap()
            .notes()
            .iter()
            .map(|n| n.id())
            .collect();
        ids.sort_unstable();
        assert!(ids.len() > 1);
        assert_eq!(v.midi_selection.ids(), ids.as_slice());
        assert_eq!(v.midi_selection.primary(), ids.last().copied());
        ids
    });
    visual.simulate_mouse_move(
        roll.origin - gpui::point(gpui::px(5.0), gpui::px(5.0)),
        None,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let scale = gpui::ScaledPixels::from(visual.update(|window, _| window.scale_factor()));
    let text: gpui::Background =
        gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Text)).into();
    let accents: Vec<_> = visual
        .rendered_filled_quads("midi-note-roll")
        .into_iter()
        .filter(|(b, c)| {
            c == &text && b.size.width / scale > 3.0 && (b.size.height / scale - 2.0).abs() < 0.01
        })
        .collect();
    assert_eq!(
        accents.len(),
        2,
        "the primary note has persistent top/bottom accents without hover"
    );
    let primary_start = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .midi_note(*ids.last().unwrap())
            .unwrap()
            .start()
    });
    let expected_x = f32::from(roll.origin.x)
        + f32::from(roll.size.width)
            * (primary_start.raw() as f32 / (4 * BeatTime::ONE.raw()) as f32);
    for (bounds, _) in accents {
        assert!((bounds.origin.x / scale - expected_x).abs() < 0.05);
    }
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        pump(&editor, &mut visual);
    }
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_selection.ids(), ids.as_slice());
        assert_eq!(v.midi_selection.primary(), ids.last().copied());
    });
}
#[gpui::test]
fn cancelled_held_creation_and_draw_keep_valid_selection(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
            pitch: 60,
            channel: 0,
            velocity: 37
        })))
    });
    pump(&editor, &mut visual);
    let baseline = editor.read_with(&visual, |v, _| v.midi_selection.primary().unwrap());
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let at = point(roll.origin.x + roll.size.width * 0.5, roll.center().y);
    for draw in [false, true] {
        editor.update(&mut visual, |v, cx| {
            v.midi_draw_mode = draw;
            cx.notify();
        });
        visual.run_until_parked();
        visual.simulate_event(MouseDownEvent {
            position: at,
            button: MouseButton::Left,
            click_count: if draw { 1 } else { 2 },
            modifiers: Modifiers::default(),
            first_mouse: false,
        });
        visual.simulate_keystrokes("escape");
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.midi_selection.primary(), Some(baseline));
            assert_eq!(v.midi_selection.ids(), &[baseline]);
        });
    }
}
#[gpui::test]
fn changed_selection_before_actual_ack_cannot_be_replaced_by_created_note(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    for beat in [0, 1] {
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::from_quarters(beat).unwrap(),
                duration: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
                pitch: 60,
                channel: 0,
                velocity: 37
            })))
        });
        pump(&editor, &mut visual);
    }
    let old = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap()
            .notes()[0]
            .id()
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::from_quarters(2).unwrap(),
            duration: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
            pitch: 60,
            channel: 0,
            velocity: 37
        })))
    });
    let event = next(&editor, &visual);
    assert!(matches!(event.created, Some(Created::Note(_))));
    editor.update(&mut visual, |v, cx| {
        v.midi_selection.set_single(old);
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_selection.primary(), Some(old))
    });
}

#[gpui::test]
fn tuning_terminal_rebases_only_the_known_unchanged_source_before_creation_ack(
    cx: &mut TestAppContext,
) {
    tuning_before_creation(cx, false);
}
#[gpui::test]
fn accepted_tuning_cancel_preserves_queued_creation_selection_without_advancing_revision(
    cx: &mut TestAppContext,
) {
    tuning_before_creation(cx, true);
}
fn tuning_before_creation(cx: &mut TestAppContext, cancel: bool) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        let tuning = v.state.as_ref().unwrap().project.tuning();
        v.tuning.client.begin(v.project_generation, tuning).unwrap();
        v.tuning.client.change(tuning.with_enabled(true)).unwrap();
        if cancel {
            v.tuning.client.cancel().unwrap();
        } else {
            v.tuning.client.finish(tuning.with_enabled(true)).unwrap();
        }
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ONE,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 93
        })));
        assert_eq!(
            v.pending_requests.len(),
            1,
            "creation waits behind the typed tuning terminal"
        );
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        assert_eq!(
            v.state.as_ref().unwrap().project.tuning().enabled(),
            !cancel
        );
        let notes = v
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap()
            .notes();
        assert_eq!(notes.len(), 1);
        assert_eq!(v.midi_selection.primary(), Some(notes[0].id()));
        assert!(v.pending_created_note_selection.is_none());
    });
}
#[gpui::test]
fn focus_loss_and_failed_creation_cannot_replace_valid_baseline(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 37
        })))
    });
    pump(&editor, &mut visual);
    let baseline = editor.read_with(&visual, |v, _| v.midi_selection.primary().unwrap());
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::from_quarters(2).unwrap(),
            duration: BeatTime::ONE,
            pitch: 64,
            channel: 0,
            velocity: 93
        })))
    });
    let event = next(&editor, &visual);
    visual.update(|window, cx| editor.read_with(cx, |v, _| v.focus.focus(window)));
    visual.run_until_parked();
    editor.update(&mut visual, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_selection.primary(), Some(baseline))
    });
    visual.update(|window, cx| editor.read_with(cx, |v, _| v.midi_focus.focus(window)));
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::from_quarters(3).unwrap(),
            duration: BeatTime::ONE,
            pitch: 65,
            channel: 0,
            velocity: 0
        })))
    });
    let failure = next(&editor, &visual);
    assert!(failure.error.is_some());
    editor.update(&mut visual, |v, cx| {
        v.receive(failure);
        cx.notify();
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_selection.primary(), Some(baseline))
    });
}

#[gpui::test]
fn source_growth_creation_uses_returned_identity_and_preserves_one_history_action(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    editor.update(&mut visual, |v, cx| {
        v.piano_beat_span = BeatTime::from_quarters(16).unwrap();
        cx.notify();
    });
    visual.run_until_parked();
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let at = point(roll.origin.x + roll.size.width * 0.5, roll.center().y);
    visual.simulate_event(MouseDownEvent {
        position: at,
        button: MouseButton::Left,
        click_count: 2,
        modifiers: Modifiers::default(),
        first_mouse: false,
    });
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    let event = next(&editor, &visual);
    let Some(Created::Notes {
        clip: created_clip,
        first,
        last,
    }) = event.created
    else {
        panic!("source growth returns a canonical range")
    };
    assert_eq!(created_clip, clip);
    assert_eq!(first, last);
    editor.update(&mut visual, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    pump(&editor, &mut visual);
    let after = editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_selection.ids(), &[last]);
        assert_eq!(v.midi_selection.primary(), Some(last));
        let source = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
        let note = v.state.as_ref().unwrap().project.midi_note(last).unwrap();
        assert!(source.source_length().raw() >= note.start().raw() + note.duration().raw());
        assert!(note.start() >= BeatTime::from_quarters(8).unwrap());
        v.state.as_ref().unwrap().project.snapshot()
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
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
        );
        assert!(v.midi_selection.is_empty());
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
    });
}
#[gpui::test]
fn unrelated_source_reply_cannot_rebase_or_claim_a_tuning_queued_creation(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 37
        })))
    });
    pump(&editor, &mut visual);
    let baseline = editor.read_with(&visual, |v, _| v.midi_selection.primary().unwrap());
    editor.update(&mut visual, |v, _| {
        let tuning = v.state.as_ref().unwrap().project.tuning();
        v.tuning.client.begin(v.project_generation, tuning).unwrap();
        v.tuning.client.change(tuning.with_enabled(true)).unwrap();
        v.tuning.client.finish(tuning.with_enabled(true)).unwrap();
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::from_quarters(3).unwrap(),
            duration: BeatTime::ONE,
            pitch: 64,
            channel: 0,
            velocity: 93
        })));
        // Adversarial real worker input: an unrelated source edit arrives before
        // the queued UI tuning protocol. Its created ID must not claim our intent.
        v.service
            .submit(Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::ONE,
                duration: BeatTime::ONE,
                pitch: 65,
                channel: 0,
                velocity: 80,
            }))
            .unwrap();
    });
    let event = next(&editor, &visual);
    assert!(matches!(event.created, Some(Created::Note(_))));
    editor.update(&mut visual, |v, cx| {
        v.receive(event);
        assert!(v.pending_created_note_selection.is_none());
        cx.notify();
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_selection.primary(), Some(baseline));
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()
                .len(),
            3
        );
    });
}

#[gpui::test]
fn clip_switch_before_delayed_ack_preserves_the_new_context_selection(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 37
        })))
    });
    pump(&editor, &mut visual);
    let original = editor.read_with(&visual, |v, _| v.midi_selection.primary().unwrap());
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::PlaceMidiClip {
            track: v.selected_track.unwrap(),
            start: BeatTime::from_quarters(4).unwrap(),
            length: BeatTime::from_quarters(4).unwrap()
        })))
    });
    pump(&editor, &mut visual);
    let other = editor.read_with(&visual, |v, _| v.detail_clip_id().unwrap());
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip: other,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 64,
            channel: 0,
            velocity: 93
        })))
    });
    pump(&editor, &mut visual);
    let other_note = editor.read_with(&visual, |v, _| v.midi_selection.primary().unwrap());
    editor.update(&mut visual, |v, cx| {
        v.selected_clip = Some(clip);
        v.midi_selection.set_single(original);
        cx.notify();
    });
    visual.run_until_parked();
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::from_quarters(2).unwrap(),
            duration: BeatTime::ONE,
            pitch: 65,
            channel: 0,
            velocity: 80
        })))
    });
    let event = next(&editor, &visual);
    editor.update(&mut visual, |v, cx| {
        v.selected_clip = Some(other);
        v.midi_selection.set_single(other_note);
        cx.notify();
    });
    visual.run_until_parked();
    editor.update(&mut visual, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.detail_clip_id(), Some(other));
        assert_eq!(v.midi_selection.ids(), &[other_note]);
    });
}
#[gpui::test]
fn rapid_batches_use_each_real_canonical_range_and_malformed_ranges_preserve_baseline(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let mut previous = Vec::new();
    let mut first_batch = Vec::new();
    for offset in [0, 2] {
        editor.update(&mut visual, |v, _| {
            assert!(
                v.enqueue(Request::Edit(Command::AddMidiNotes {
                    clip,
                    notes: [1, 0]
                        .into_iter()
                        .map(|index| hex_project::MidiNoteInsert {
                            active: true,
                            start: BeatTime::from_quarters(offset + index).unwrap(),
                            duration: BeatTime::ONE,
                            pitch: 60 + index as u8,
                            channel: 0,
                            velocity: 37
                        })
                        .collect()
                }))
            )
        });
        let event = next(&editor, &visual);
        let Some(Created::Notes { first, last, .. }) = event.created else {
            panic!("canonical batch range")
        };
        editor.update(&mut visual, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        pump(&editor, &mut visual);
        previous = editor.read_with(&visual, |v, _| {
            assert_eq!(v.midi_selection.ids(), &[first, last]);
            assert_eq!(v.midi_selection.primary(), Some(last));
            v.midi_selection.ids().to_vec()
        });
        if offset == 0 {
            first_batch = previous.clone();
        }
    }
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::EditMidiNotes {
            clip,
            remove: vec![first_batch[1]],
            insert: vec![hex_project::MidiNoteInsert {
                active: true,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 67,
                channel: 0,
                velocity: 93
            }],
        })))
    });
    let mut malformed = next(&editor, &visual);
    let last = match malformed.created {
        Some(Created::Notes { last, .. }) => last,
        _ => panic!("range"),
    };
    // Claim a missing, retired ID between real source identities. The whole
    // malformed range must refuse without changing the valid selected baseline.
    malformed.created = Some(Created::Notes {
        clip,
        first: first_batch[0],
        last,
    });
    editor.update(&mut visual, |v, cx| {
        v.receive(malformed);
        cx.notify();
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_selection.ids(), previous.as_slice());
        assert!(v.error.as_deref().unwrap().contains("incomplete"));
    });
}
