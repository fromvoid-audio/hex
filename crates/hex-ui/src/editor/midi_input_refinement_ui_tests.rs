//! Actual grid events, canonical worker ACKs and completed paint/cursor feedback.
use super::creation_tests::{double_down, fixture, pump};
use gpui::{CursorStyle, Modifiers, MouseButton, TestAppContext, point, px};
use hex_project::{BeatTime, Command};
use hex_session::service::Request;

#[gpui::test]
fn empty_grid_drag_selects_time_and_repeated_copy_retains_ack_destination(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let beat = BeatTime::ONE.raw();
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ONE,
            duration: BeatTime::from_raw(beat / 2),
            pitch: 60,
            channel: 0,
            velocity: 93,
        })))
    });
    pump(&editor, &mut visual);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let at = |fraction| {
        point(
            roll.origin.x + roll.size.width * fraction,
            roll.origin.y + px(4.0),
        )
    };
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    visual.simulate_mouse_down(at(0.5), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(at(0.125), Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_marquee.is_none());
        assert_eq!(v.midi_time_selection_bounds(), Some((beat / 2, 2 * beat)));
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert!(!v.edit_pending());
    });
    visual.simulate_mouse_up(at(0.125), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    for n in 1..=2 {
        visual.simulate_keystrokes("cmd-d");
        pump(&editor, &mut visual);
        for _ in 0..3 {
            editor.update(&mut visual, |_, cx| cx.notify());
            visual.run_until_parked();
        }
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.midi_time_selection_bounds(),
                Some((beat / 2 + n * 3 * beat / 2, 2 * beat + n * 3 * beat / 2))
            );
            assert!(v.midi_selection.is_empty());
            assert_eq!(
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .midi_source(clip)
                    .unwrap()
                    .notes()
                    .len(),
                n as usize + 1
            );
        });
    }
    visual.simulate_keystrokes("cmd-z");
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
            2
        )
    });
    visual.simulate_keystrokes("cmd-shift-z");
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
            3
        )
    });
}

#[gpui::test]
fn grid_time_capture_cancel_stale_and_stationary_click_keep_defined_owners(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, _clip) = fixture(cx);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let at = |fraction| point(roll.origin.x + roll.size.width * fraction, roll.center().y);
    let beat = BeatTime::ONE.raw();
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    visual.simulate_mouse_down(at(0.25), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at(0.75), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    for mode in 0..3 {
        visual.simulate_mouse_down(at(0.125), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(at(0.5), Some(MouseButton::Left), Modifiers::default());
        if mode == 0 {
            visual.simulate_keystrokes("escape");
        } else if mode == 1 {
            visual.update(|window, cx| editor.update(cx, |v, _| v.focus.focus(window)));
        } else {
            editor.update(&mut visual, |v, _| {
                v.piano_beat_span = BeatTime::from_quarters(8).unwrap()
            });
        }
        visual.simulate_mouse_up(at(0.5), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.midi_time_selection_bounds(), Some((beat, 3 * beat)));
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
        visual.update(|window, cx| {
            editor.update(cx, |v, cx| {
                v.piano_beat_span = BeatTime::from_quarters(4).unwrap();
                v.midi_focus.focus(window);
                cx.notify();
            })
        });
        visual.run_until_parked();
    }
    visual.simulate_click(at(0.5), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_marker, Some(2 * beat));
        assert!(v.midi_time_selection_bounds().is_none());
        assert!(
            v.error.is_none(),
            "grid click must not issue inactive-route cue: {:?}",
            v.error
        );
        assert!(!v.edit_pending());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
}

#[gpui::test]
fn creation_floors_onset_and_free_preview_commits_one_pixel_at_both_zooms(cx: &mut TestAppContext) {
    use super::midi_drag::MidiGrid;
    for draw in [false, true] {
        for (span, grid, invert) in [
            (4 * BeatTime::ONE.raw(), MidiGrid::Sixteenth, false),
            (16 * BeatTime::ONE.raw(), MidiGrid::Off, false),
            (BeatTime::ONE.raw() / 16, MidiGrid::Off, false),
            (4 * BeatTime::ONE.raw(), MidiGrid::Sixteenth, true),
        ] {
            let (mut visual, editor, _environment, clip) = fixture(cx);
            editor.update(&mut visual, |v, cx| {
                v.midi_draw_mode = draw;
                v.midi_snap_grid = grid;
                v.piano_beat_span = BeatTime::from_raw(span);
                cx.notify();
            });
            visual.run_until_parked();
            let roll = visual.debug_bounds("midi-note-roll").unwrap();
            let down = point(roll.origin.x + roll.size.width * 0.304, roll.center().y);
            let mods = Modifiers {
                platform: invert,
                ..Default::default()
            };
            if draw {
                visual.simulate_mouse_down(down, MouseButton::Left, mods);
            } else {
                double_down(&mut visual, down, mods);
            }
            visual.run_until_parked();
            let preview = editor.read_with(&visual, |v, _| {
                let note = if draw {
                    v.midi_stroke.as_ref().unwrap().shared_preview().inserted[0]
                } else {
                    v.midi_creation.as_ref().unwrap().preview
                };
                let raw =
                    (f64::from((down.x - roll.origin.x - px(1.0)) / (roll.size.width - px(2.0)))
                        * span as f64) as u64;
                let free = grid == MidiGrid::Off || invert;
                if free {
                    assert_eq!(note.start.raw(), raw);
                    let pixel = (span as f64 / f64::from(f32::from(roll.size.width - px(2.0))))
                        .ceil() as u64;
                    assert_eq!(
                        note.duration.raw(),
                        pixel.max(
                            v.state
                                .as_ref()
                                .unwrap()
                                .project
                                .tempo_map()
                                .frame_to_beat(2)
                                .unwrap()
                                .raw()
                                .max(1)
                        )
                    );
                } else {
                    assert_eq!(note.start.raw(), BeatTime::ONE.raw());
                    assert_eq!(note.duration.raw(), BeatTime::ONE.raw() / 4);
                }
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
                note
            });
            let primary: gpui::Background =
                gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Primary)).into();
            assert!(
                visual
                    .rendered_filled_quads("midi-note-roll")
                    .iter()
                    .any(|(_, color)| *color == primary)
            );
            visual.simulate_mouse_up(down, MouseButton::Left, mods);
            pump(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                let source = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
                assert_eq!(source.notes().len(), 1);
                assert_eq!(source.notes()[0].start(), preview.start);
                assert_eq!(source.notes()[0].duration(), preview.duration);
                assert!(v.error.is_none(), "{:?}", v.error);
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
    }
}

#[gpui::test]
fn copy_modifier_cursor_tracks_actual_payload_and_restores_after_cancel(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ONE,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 2,
            velocity: 93,
        })))
    });
    pump(&editor, &mut visual);
    let original = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap()
            .notes()[0]
            .clone()
    });
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let y = editor.read_with(&visual, |v, _| {
        roll.origin.y
            + roll.size.height
                * ((f32::from(v.piano_pitch_low + v.piano_pitch_rows - 1 - 60) + 0.5)
                    / f32::from(v.piano_pitch_rows))
    });
    let down = point(roll.origin.x + roll.size.width * 0.375, y);
    let end = point(down.x + roll.size.width * 0.375, y);
    let copy = Modifiers {
        alt: true,
        ..Default::default()
    };
    for mode in 0..4 {
        let cancel = mode != 3;
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        visual.simulate_modifiers_change(copy);
        visual.run_until_parked();
        assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::DragCopy));
        visual.simulate_modifiers_change(Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(CursorStyle::ClosedHand)
        );
        visual.simulate_modifiers_change(copy);
        visual.run_until_parked();
        assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::DragCopy));
        if mode == 0 {
            visual.simulate_keystrokes("escape");
        } else if mode == 1 {
            visual.update(|window, cx| editor.update(cx, |v, _| v.focus.focus(window)));
            visual.run_until_parked();
        } else if mode == 2 {
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Edit(Command::SetMidiClipSource {
                    clip,
                    source_length: BeatTime::from_quarters(4).unwrap(),
                    source_start: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
                    loop_start: BeatTime::ZERO,
                    loop_end: BeatTime::from_quarters(4).unwrap(),
                    looping: false,
                    length: BeatTime::from_quarters(4).unwrap(),
                })))
            });
            pump(&editor, &mut visual);
        }
        visual.simulate_mouse_up(end, MouseButton::Left, copy);
        pump(&editor, &mut visual);
        assert_ne!(visual.rendered_cursor_style(), Some(CursorStyle::DragCopy));
        editor.read_with(&visual, |v, _| {
            let notes = v
                .state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes();
            assert_eq!(notes.len(), if cancel { 1 } else { 2 });
            assert_eq!(notes[0], original);
            if !cancel {
                assert_ne!(notes[1].id(), original.id());
            }
        });
        visual.update(|window, cx| editor.update(cx, |v, _| v.midi_focus.focus(window)));
        visual.run_until_parked();
    }
}

#[gpui::test]
fn idle_hover_selected_note_paint_are_distinct_on_completed_frames(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ONE,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })))
    });
    pump(&editor, &mut visual);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let y = editor.read_with(&visual, |v, _| {
        roll.origin.y
            + roll.size.height
                * ((f32::from(v.piano_pitch_low + v.piano_pitch_rows - 1 - 60) + 0.5)
                    / f32::from(v.piano_pitch_rows))
    });
    let at = point(roll.origin.x + roll.size.width * 0.375, y);
    editor.update(&mut visual, |v, cx| {
        v.midi_selection.clear();
        cx.notify();
    });
    visual.simulate_mouse_move(
        roll.origin - point(px(2.0), px(2.0)),
        None,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let scale = gpui::ScaledPixels::from(visual.update(|window, _| window.scale_factor()));
    let count = |visual: &mut gpui::VisualTestContext, role, height: f32| {
        let color: gpui::Background = gpui::rgb(crate::theme::color(role)).into();
        visual
            .rendered_filled_quads("midi-note-roll")
            .iter()
            .filter(|(b, c)| {
                *c == color
                    && b.size.width / scale > 10.0
                    && (b.size.height / scale - height).abs() < 0.01
            })
            .count()
    };
    assert!(
        count(&mut visual, crate::theme::PaletteRole::Background, 1.0) >= 2,
        "idle borders are dark"
    );
    assert_eq!(count(&mut visual, crate::theme::PaletteRole::Text, 1.0), 0);
    visual.simulate_mouse_move(at, None, Modifiers::default());
    visual.run_until_parked();
    assert!(
        count(&mut visual, crate::theme::PaletteRole::SecondaryText, 1.0) >= 2,
        "hover has a subtle outline"
    );
    assert_eq!(count(&mut visual, crate::theme::PaletteRole::Text, 2.0), 0);
    visual.simulate_click(at, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        count(&mut visual, crate::theme::PaletteRole::Text, 2.0),
        2,
        "primary selection keeps its strong accent under hover"
    );
    assert_eq!(
        count(&mut visual, crate::theme::PaletteRole::SecondaryText, 1.0),
        0
    );
}

#[gpui::test]
fn free_held_creation_resizes_from_exact_anchor_and_escape_discards_it(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    editor.update(&mut visual, |v, cx| {
        v.midi_snap_grid = super::midi_drag::MidiGrid::Off;
        cx.notify();
    });
    visual.run_until_parked();
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let down = point(roll.origin.x + roll.size.width * 0.304, roll.center().y);
    for cancel in [true, false] {
        double_down(&mut visual, down, Modifiers::default());
        let start = editor.read_with(&visual, |v, _| {
            v.midi_creation.as_ref().unwrap().preview.start
        });
        // Free mode responds to even one pixel of motion, then cannot invert
        // the note when dragged left across its fixed onset.
        visual.simulate_mouse_move(
            down + point(px(1.0), px(0.0)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        visual.simulate_mouse_move(
            down - point(px(20.0), px(0.0)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        editor.read_with(&visual, |v, _| {
            let note = v.midi_creation.as_ref().unwrap().preview;
            assert_eq!(note.start, start);
            assert_eq!(note.duration.raw(), v.midi_minimum_creation_gate().unwrap());
        });
        let end = down + point(px(83.0), px(0.0));
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        let preview = editor.read_with(&visual, |v, _| v.midi_creation.as_ref().unwrap().preview);
        assert_eq!(preview.start, start);
        assert!(preview.duration.raw() > BeatTime::ONE.raw() / 4);
        if cancel {
            visual.simulate_keystrokes("escape");
        }
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
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
            assert_eq!(notes.len(), usize::from(!cancel));
            if !cancel {
                assert_eq!(notes[0].start(), start);
                assert_eq!(notes[0].duration(), preview.duration);
            }
        });
    }
}
