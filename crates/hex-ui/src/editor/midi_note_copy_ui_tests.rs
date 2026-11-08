//! Real copy input keeps canonical IDs, selection ACK and history in their owners.
use super::creation_tests::{fixture, pump};
use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use hex_project::{BeatTime, ClipId, Command, MidiNote, MidiNoteInsert};
use hex_session::service::Request;

fn history_matches(actual: hex_project::Snapshot, expected: &hex_project::Snapshot) {
    // Canonical history deliberately never reuses allocated identities.
    assert!(actual.next_id >= expected.next_id);
    let mut expected = expected.clone();
    expected.next_id = actual.next_id;
    assert_eq!(actual, expected);
}
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
                        active: true,
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
                        * ((beat * BeatTime::ONE.raw() as f64 - v.piano_beat_start as f64)
                            / v.piano_beat_span.raw() as f64) as f32,
                px(1.0)
                    + (roll.size.height - px(2.0))
                        * (f32::from(v.piano_pitch_low + v.piano_pitch_rows - 1 - pitch) + 0.5)
                        / f32::from(v.piano_pitch_rows),
            )
    })
}

#[gpui::test]
fn note_activation_and_copy_keep_inactive_source_notes(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    authored(&editor, &mut visual, clip);
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_keystrokes("0");
    pump(&editor, &mut visual);
    assert!(
        notes(&editor, &visual, clip)
            .iter()
            .all(|note| !note.active())
    );
    visual.run_until_parked();
    let center = at(&editor, &mut visual, 0.625, 60);
    let scale = visual.update(|window, _| window.scale_factor());
    let x = f64::from(f32::from(center.x) * scale);
    let y = f64::from(f32::from(center.y) * scale);
    let muted: gpui::Background =
        gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Muted)).into();
    assert!(
        visual
            .rendered_filled_quads("midi-note-roll")
            .iter()
            .any(|(bounds, color)| {
                *color == muted
                    && f64::from(bounds.left()) <= x
                    && x < f64::from(bounds.right())
                    && f64::from(bounds.top()) <= y
                    && y < f64::from(bounds.bottom())
            }),
        "inactive note has a muted fill at its visible center"
    );
    visual.simulate_keystrokes("cmd-d");
    pump(&editor, &mut visual);
    let copied = notes(&editor, &visual, clip);
    assert_eq!(copied.len(), 4);
    assert!(copied.iter().all(|note| !note.active()));
    visual.simulate_keystrokes("0");
    pump(&editor, &mut visual);
    let after = notes(&editor, &visual, clip);
    assert_eq!(after.iter().filter(|note| note.active()).count(), 2);
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    assert!(
        notes(&editor, &visual, clip)
            .iter()
            .all(|note| !note.active())
    );
}

#[gpui::test]
fn note_activation_does_not_interrupt_a_held_note_gesture(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    authored(&editor, &mut visual, clip);
    visual.simulate_keystrokes("cmd-a");
    let down = at(&editor, &mut visual, 0.625, 60);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("0");
    pump(&editor, &mut visual);
    assert!(
        notes(&editor, &visual, clip)
            .iter()
            .all(|note| note.active())
    );
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    assert!(
        notes(&editor, &visual, clip)
            .iter()
            .all(|note| note.active())
    );
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
            history_matches(v.state.as_ref().unwrap().project.snapshot(), expected)
        });
    }
    for expected in history.iter().skip(1) {
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            history_matches(v.state.as_ref().unwrap().project.snapshot(), expected)
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
            history_matches(v.state.as_ref().unwrap().project.snapshot(), &before)
        });
    }
}

#[gpui::test]
fn console_copy_keeps_arrangement_isolated_and_invalid_empty_requests_do_not_edit(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, arrangement) = fixture(cx);
    authored(&editor, &mut visual, arrangement);
    let original = notes(&editor, &visual, arrangement);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddScene {
            name: "copy scene".into()
        })))
    });
    pump(&editor, &mut visual);
    let scene = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.scenes()[0].id()
    });
    editor.update(&mut visual, |v, _| {
        assert!(
            v.enqueue(Request::Edit(Command::CopyArrangementMidiToSession {
                source: arrangement,
                scene
            }))
        )
    });
    pump(&editor, &mut visual);
    let clip = editor.update(&mut visual, |v, cx| {
        let track = v.selected_track.unwrap();
        let hex_project::SessionCellContent::Midi(clip) = v
            .state
            .as_ref()
            .unwrap()
            .project
            .session_cell(track, scene)
            .unwrap()
        else {
            panic!("midi cell")
        };
        let clip = clip.id();
        v.mode = super::super::EditorMode::Session;
        v.selected_scene = Some(scene);
        v.selected_cell = Some((track, scene));
        v.selected_clip = Some(clip);
        v.piano_view_clip = None;
        v.midi_selection.clear();
        cx.notify();
        clip
    });
    visual.run_until_parked();
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    visual.simulate_keystrokes("cmd-d");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    editor.update(&mut visual, |v, cx| {
        let note = v
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap()
            .notes()[0]
            .clone();
        let sample = super::super::midi_group_drag::GroupNote {
            id: note.id(),
            active: note.active(),
            start: note.start().raw(),
            duration: note.duration().raw(),
            pitch: note.pitch(),
            channel: note.channel(),
            velocity: note.velocity(),
        };
        use super::super::midi_note_copy::insertions;
        assert!(insertions(&[], 1).is_err());
        assert!(insertions(&[sample], u64::MAX).is_err());
        for invalid in [
            super::super::midi_group_drag::GroupNote {
                duration: 0,
                ..sample
            },
            super::super::midi_group_drag::GroupNote {
                pitch: 128,
                ..sample
            },
            super::super::midi_group_drag::GroupNote {
                channel: 16,
                ..sample
            },
            super::super::midi_group_drag::GroupNote {
                velocity: 0,
                ..sample
            },
        ] {
            assert!(insertions(&[invalid], 1).is_err());
        }
        v.midi_selection.set_single(note.id());
        v.midi_snap_grid = super::super::midi_drag::MidiGrid::Off;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-d");
    pump(&editor, &mut visual);
    visual.run_until_parked();
    assert_eq!(notes(&editor, &visual, arrangement), original);
    let all = notes(&editor, &visual, clip);
    assert_eq!(all.len(), 3);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_selection.len(), 1);
        let copy = all
            .iter()
            .find(|n| Some(n.id()) == v.midi_selection.primary())
            .unwrap();
        assert_eq!(copy.start().raw(), BeatTime::ONE.raw() * 3 / 4);
        assert_eq!(copy.velocity(), 37);
    });
}

#[gpui::test]
fn copy_collision_keeps_left_prefix_and_focus_or_real_revision_cancels_preview(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
            duration: BeatTime::from_quarters(2).unwrap(),
            pitch: 60,
            channel: 2,
            velocity: 93
        })))
    });
    pump(&editor, &mut visual);
    let original = notes(&editor, &visual, clip)[0].clone();
    let down = at(&editor, &mut visual, 1.0, 60);
    let end = at(&editor, &mut visual, 2.0, 60);
    let alt = Modifiers {
        alt: true,
        ..Default::default()
    };
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    visual.simulate_mouse_down(down, MouseButton::Left, alt);
    visual.simulate_mouse_move(end, Some(MouseButton::Left), alt);
    visual.update(|window, cx| editor.update(cx, |v, _| v.focus.focus(window)));
    visual.run_until_parked();
    visual.simulate_mouse_up(end, MouseButton::Left, alt);
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    visual.update(|window, cx| editor.update(cx, |v, _| v.midi_focus.focus(window)));
    visual.simulate_mouse_down(down, MouseButton::Left, alt);
    visual.simulate_mouse_move(end, Some(MouseButton::Left), alt);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 72,
            channel: 0,
            velocity: 64
        })))
    });
    pump(&editor, &mut visual);
    let after_external =
        editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    visual.simulate_mouse_up(end, MouseButton::Left, alt);
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after_external)
    });
    editor.update(&mut visual, |v, cx| {
        v.midi_selection.set_single(original.id());
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_down(down, MouseButton::Left, alt);
    visual.simulate_mouse_move(end, Some(MouseButton::Left), alt);
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after_external)
    });
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let scale = gpui::ScaledPixels::from(visual.update(|window, _| window.scale_factor()));
    let prefix_left = f32::from(roll.origin.x + px(1.0) + (roll.size.width - px(2.0)) / 8.0);
    let prefix_width = f32::from((roll.size.width - px(2.0)) / 4.0);
    let primary: gpui::Background =
        gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Primary)).into();
    assert!(
        visual
            .rendered_filled_quads("midi-note-roll")
            .iter()
            .any(|(bounds, color)| color == &primary
                && (bounds.origin.x / scale - prefix_left).abs() < 1.0
                && (bounds.size.width / scale - prefix_width).abs() < 1.0),
        "held copy paints the surviving original prefix"
    );
    visual.simulate_mouse_up(end, MouseButton::Left, alt);
    pump(&editor, &mut visual);
    visual.run_until_parked();
    let all = notes(&editor, &visual, clip);
    let prefix = all.iter().find(|n| n.id() == original.id()).unwrap();
    assert_eq!(prefix.duration(), BeatTime::ONE);
    let copy = all
        .iter()
        .find(|n| n.pitch() == 60 && n.id() != original.id())
        .unwrap();
    assert_eq!(copy.start().raw(), BeatTime::ONE.raw() * 3 / 2);
    assert_eq!(copy.duration(), original.duration());
    assert_eq!(copy.velocity(), 93);
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        history_matches(
            v.state.as_ref().unwrap().project.snapshot(),
            &after_external,
        )
    });
}
