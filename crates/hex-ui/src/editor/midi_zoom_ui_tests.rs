//! Real wheel dispatch and selected-source fitting, without musical edits.
use super::creation_tests::{fixture, pump};
use gpui::{ScrollDelta, ScrollWheelEvent, TestAppContext, point, px};
use hex_project::{BeatTime, Command};
use hex_session::service::Request;

/// Cmd zooms time; a plain wheel scrolls (see `midi_wheel`).
fn cmd() -> gpui::Modifiers {
    gpui::Modifiers {
        platform: true,
        ..Default::default()
    }
}

#[gpui::test]
fn wheel_zoom_anchors_pointer_and_does_not_mutate_project(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _clip) = fixture(cx);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let at = point(roll.origin.x + roll.size.width * 0.75, roll.center().y);
    visual.simulate_event(ScrollWheelEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.0), px(80.0))),
        modifiers: cmd(),
        ..Default::default()
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(
            v.piano_beat_span.raw() < 4 * BeatTime::ONE.raw(),
            "real piano wheel must zoom"
        );
        let anchor = v.piano_beat_start as f64 + 0.75 * v.piano_beat_span.raw() as f64;
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
        assert_eq!(v.piano_beat_start, 0);
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
        modifiers: cmd(),
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
    // Two explicit changes in one frame still constitute a fresh selection,
    // even though the final clip ID equals the previously rendered owner.
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            assert!(v.set_selected_clip(v.selected_track.unwrap(), original, window, cx));
            assert!(v.set_selected_clip(v.selected_track.unwrap(), selected, window, cx));
            cx.notify();
        });
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.piano_beat_span, BeatTime::from_quarters(32).unwrap())
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
            modifiers: cmd(),
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
        modifiers: cmd(),
        ..Default::default()
    });
    visual.run_until_parked();
    let viewport = editor.read_with(&visual, |v, _| (v.piano_beat_start, v.piano_beat_span));
    double_down(&mut visual, at(0.25), Modifiers::default());
    // Held cmd snaps the new note's end; it is free by default.
    visual.simulate_mouse_move(at(0.5), Some(MouseButton::Left), Modifiers::command());
    visual.simulate_mouse_up(at(0.5), MouseButton::Left, Modifiers::command());
    pump(&editor, &mut visual);
    let (note_center, after) = editor.read_with(&visual, |v, _| {
        assert_eq!((v.piano_beat_start, v.piano_beat_span), viewport);
        let state = v.state.as_ref().unwrap();
        let notes = state.project.midi_source(clip).unwrap().notes();
        assert_eq!(notes.len(), 1);
        assert_eq!(
            notes[0].start(),
            BeatTime::from_raw(BeatTime::ONE.raw() * 5 / 4)
        );
        assert_eq!(notes[0].duration(), BeatTime::ONE);
        let middle = notes[0].start().raw() + notes[0].duration().raw() / 2;
        (
            ((i128::from(middle) - viewport.0) as f64 / viewport.1.raw() as f64) as f32,
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

#[gpui::test]
fn wheel_can_zoom_out_past_source_and_ack_keeps_view(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _) = fixture(cx);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    visual.simulate_event(ScrollWheelEvent {
        position: roll.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(-80.0))),
        modifiers: cmd(),
        ..Default::default()
    });
    visual.run_until_parked();
    let span = editor.read_with(&visual, |v, _| v.piano_beat_span);
    assert!(
        span.raw() > 4 * BeatTime::ONE.raw(),
        "must see space beyond a full-source note"
    );
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::RenameTrack {
            track: v.selected_track.unwrap(),
            name: "zoom ack".into()
        })));
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| assert_eq!(v.piano_beat_span, span));
}

/// Real gutter drag changes pitch geometry; plain wheel stays horizontal and edits
/// still hit the same displayed note through the real worker/Undo path.
#[gpui::test]
fn piano_key_drag_zooms_pitch_and_preserves_note_editing(cx: &mut TestAppContext) {
    use super::creation_tests::double_down;
    use gpui::{Modifiers, MouseButton};
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let keys = visual.debug_bounds("midi-piano-keys").unwrap();
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    visual.simulate_mouse_down(keys.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(keys.center().x, keys.center().y - px(120.0)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!((v.piano_pitch_low, v.piano_pitch_rows), (54, 12));
        assert_eq!(v.piano_beat_span, BeatTime::from_quarters(4).unwrap());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
    // Release outside the gutter retires capture, including later pointer moves.
    visual.simulate_mouse_up(
        point(roll.center().x, keys.center().y - px(120.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(keys.center(), None, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert!(v.piano_pitch_zoom.is_none()));
    visual.simulate_event(ScrollWheelEvent {
        position: roll.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(80.0))),
        modifiers: cmd(),
        ..Default::default()
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.piano_pitch_rows, 12);
        assert!(v.piano_beat_span.raw() < 4 * BeatTime::ONE.raw());
    });
    let at = point(
        roll.origin.x + roll.size.width * 0.25,
        roll.origin.y + roll.size.height * 0.52,
    );
    double_down(&mut visual, at, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
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
        assert_eq!(notes.len(), 1);
        assert_eq!(
            notes[0].pitch(),
            59,
            "created pitch must match the enlarged key row"
        );
    });
    let note_at = editor.read_with(&visual, |v, _| {
        let note = &v
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap()
            .notes()[0];
        let x = (i128::from(note.start().raw() + note.duration().raw() / 2) - v.piano_beat_start)
            as f64
            / v.piano_beat_span.raw() as f64;
        point(roll.origin.x + roll.size.width * x as f32, at.y)
    });
    double_down(&mut visual, note_at, Modifiers::default());
    visual.simulate_mouse_up(note_at, MouseButton::Left, Modifiers::default());
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
    // Extreme drags remain bounded by the MIDI range.
    for (distance, rows) in [(-4000.0, 1), (4000.0, 128)] {
        visual.simulate_mouse_down(keys.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(keys.center().x, keys.center().y + px(distance)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.piano_pitch_rows, rows);
            assert!(u16::from(v.piano_pitch_low) + u16::from(rows) <= 128);
        });
        visual.simulate_mouse_up(keys.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
    }
    visual.simulate_mouse_down(keys.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert!(v.piano_pitch_zoom.is_none()));
}

#[gpui::test]
fn short_source_long_arrangement_notes_visibly_shrink_when_zooming_out(cx: &mut TestAppContext) {
    use super::super::midi_scene::{MidiScene, SceneKey, pixel_bounds};
    use super::creation_tests::double_down;
    use gpui::{Modifiers, MouseButton};
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let quarter = BeatTime::from_raw(BeatTime::ONE.raw() / 4);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::SetMidiClipSource {
            clip,
            source_length: quarter,
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: quarter,
            looping: false,
            length: BeatTime::from_raw(BeatTime::ONE.raw() * 7 / 2),
        })))
    });
    pump(&editor, &mut visual);
    for pitch in [59, 63] {
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: quarter,
                pitch,
                channel: 0,
                velocity: 100,
            })))
        });
        pump(&editor, &mut visual);
    }
    let fit = visual.debug_bounds("midi-zoom-fit").unwrap();
    visual.simulate_click(fit.center(), Modifiers::default());
    visual.run_until_parked();
    let widths = |v: &super::super::EditorView| {
        let state = v.state.as_ref().unwrap();
        let scene = MidiScene::build(
            SceneKey {
                generation: v.project_generation,
                revision: state.revision,
                clip,
                beat_start: v.piano_beat_start,
                beat_span: v.piano_beat_span.raw(),
                pitch_low: v.piano_pitch_low,
                pitch_rows: v.piano_pitch_rows,
            },
            state.project.midi_source(clip).unwrap().notes(),
        );
        scene
            .notes()
            .iter()
            .map(|n| {
                let (left, right) = pixel_bounds(*n, 1000.0).unwrap();
                right - left
            })
            .collect::<Vec<_>>()
    };
    let before = editor.read_with(&visual, |v, _| {
        assert_eq!(widths(v), vec![1000.0, 1000.0]);
        v.state.as_ref().unwrap().project.snapshot()
    });
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    visual.simulate_event(ScrollWheelEvent {
        position: roll.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(-80.0))),
        modifiers: cmd(),
        ..Default::default()
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        let widths = widths(v);
        assert_eq!(widths.len(), 2);
        assert!(
            widths.iter().all(|w| (*w - 800.0).abs() < 0.01),
            "paint geometry must shrink, got {widths:?}"
        );
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
    // Creation in revealed space must use its actual cell, never alias the
    // source's final tick. Merely zooming above still left the project unchanged.
    let blank = point(roll.origin.x + roll.size.width * 0.95, roll.center().y);
    double_down(&mut visual, blank, Modifiers::default());
    visual.simulate_mouse_up(blank, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let source = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
        assert_eq!(source.notes().len(), 3);
        assert_eq!(source.notes()[2].start(), quarter);
        assert!(source.source_length() > quarter);
    });
}

#[gpui::test]
fn option_wheel_zooms_pitch_under_pointer_and_preserves_time_and_history(cx: &mut TestAppContext) {
    use gpui::Modifiers;
    for surface in ["midi-note-roll", "midi-piano-keys"] {
        let (mut visual, editor, _environment, _) = fixture(cx);
        let bounds = visual.debug_bounds(surface).unwrap();
        let before = editor.read_with(&visual, |v, _| {
            (
                v.piano_pitch_low,
                v.piano_pitch_rows,
                v.piano_beat_start,
                v.piano_beat_span,
                v.state.as_ref().unwrap().project.snapshot(),
            )
        });
        let at = point(
            bounds.center().x,
            bounds.origin.y + bounds.size.height * 0.75,
        );
        visual.simulate_event(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(0.0), px(80.0))),
            modifiers: Modifiers {
                alt: true,
                ..Default::default()
            },
            ..Default::default()
        });
        visual.run_until_parked();
        let pitch_view = editor.read_with(&visual, |v, _| {
            assert!(
                v.piano_pitch_rows < before.1,
                "Option-wheel must enlarge pitch rows over {surface}"
            );
            let old_pitch = f32::from(before.0) + f32::from(before.1) * 0.25;
            let new_pitch = f32::from(v.piano_pitch_low) + f32::from(v.piano_pitch_rows) * 0.25;
            assert!((new_pitch - old_pitch).abs() <= 0.5);
            assert_eq!(
                (v.piano_beat_start, v.piano_beat_span),
                (before.2, before.3)
            );
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before.4);
            (v.piano_pitch_low, v.piano_pitch_rows)
        });
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::RenameTrack {
                track: v.selected_track.unwrap(),
                name: "ack".into()
            })))
        });
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!((v.piano_pitch_low, v.piano_pitch_rows), pitch_view)
        });
        let roll = visual.debug_bounds("midi-note-roll").unwrap();
        visual.simulate_event(ScrollWheelEvent {
            position: roll.center(),
            delta: ScrollDelta::Pixels(point(px(0.0), px(80.0))),
            modifiers: cmd(),
            ..Default::default()
        });
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!((v.piano_pitch_low, v.piano_pitch_rows), pitch_view);
            assert!(v.piano_beat_span < before.3, "cmd-wheel zooms time");
        });
    }
}

#[gpui::test]
fn option_wheel_accumulates_independently_clamps_and_respects_held_marquee(
    cx: &mut TestAppContext,
) {
    use gpui::{Modifiers, MouseButton};
    let (mut visual, editor, _environment, _) = fixture(cx);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let before = editor.read_with(&visual, |v, _| {
        (
            v.piano_pitch_rows,
            v.piano_beat_span,
            v.state.as_ref().unwrap().project.snapshot(),
        )
    });
    let wheel = |visual: &mut gpui::VisualTestContext, delta, alt| {
        visual.simulate_event(ScrollWheelEvent {
            position: roll.center(),
            delta: ScrollDelta::Pixels(point(px(0.0), px(delta))),
            modifiers: Modifiers {
                alt,
                platform: !alt,
                ..Default::default()
            },
            ..Default::default()
        });
        visual.run_until_parked();
    };
    wheel(&mut visual, 20.0, false);
    for _ in 0..3 {
        wheel(&mut visual, 20.0, true);
    }
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            (v.piano_pitch_rows, v.piano_beat_span),
            (before.0, before.1)
        )
    });
    wheel(&mut visual, 20.0, true);
    let rows = editor.read_with(&visual, |v, _| {
        assert!(v.piano_pitch_rows < before.0);
        assert_eq!(v.piano_beat_span, before.1);
        v.piano_pitch_rows
    });
    wheel(&mut visual, 60.0, false);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.piano_pitch_rows, rows);
        assert!(v.piano_beat_span < before.1);
    });
    for _ in 0..12 {
        wheel(&mut visual, -320.0, true);
    }
    editor.read_with(&visual, |v, _| {
        assert_eq!((v.piano_pitch_low, v.piano_pitch_rows), (0, 128))
    });
    for _ in 0..12 {
        wheel(&mut visual, 320.0, true);
    }
    editor.read_with(&visual, |v, _| assert_eq!(v.piano_pitch_rows, 1));
    editor.update(&mut visual, |v, _| {
        let saved = (
            v.piano_pitch_low,
            v.piano_pitch_rows,
            v.piano_pitch_wheel_accum,
        );
        for (bounds, y, delta) in [
            ([0.0, 0.0, 100.0, 100.0], -1.0, 80.0),
            ([0.0, 0.0, 100.0, 100.0], 101.0, 80.0),
            ([0.0, 0.0, 100.0, 0.0], 0.0, 80.0),
            ([f32::NAN, 0.0, 100.0, 100.0], 50.0, 80.0),
            ([0.0, 0.0, 100.0, 100.0], 50.0, f32::INFINITY),
        ] {
            assert!(!v.zoom_piano_pitch_wheel_at(bounds, y, delta));
            assert_eq!(
                (
                    v.piano_pitch_low,
                    v.piano_pitch_rows,
                    v.piano_pitch_wheel_accum
                ),
                saved
            );
        }
    });
    visual.simulate_mouse_down(
        roll.center(),
        MouseButton::Left,
        Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    visual.simulate_mouse_move(
        point(roll.center().x + px(15.0), roll.center().y + px(10.0)),
        Some(MouseButton::Left),
        Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    editor.read_with(&visual, |v, _| assert!(v.midi_marquee.is_some()));
    wheel(&mut visual, -80.0, true);
    editor.read_with(&visual, |v, _| {
        assert!(
            v.midi_marquee.is_some(),
            "wheel cannot cancel held selection"
        );
        assert_eq!(v.piano_pitch_rows, 1, "wheel cannot zoom during capture");
    });
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(roll.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    wheel(&mut visual, -80.0, true);
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_marquee.is_none());
        assert!(v.piano_pitch_rows > 1, "fresh wheel works after release");
    });
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before.2)
    });
}

#[gpui::test]
fn option_wheel_pitch_geometry_creates_the_displayed_note_through_worker_and_undo(
    cx: &mut TestAppContext,
) {
    use super::creation_tests::double_down;
    use gpui::{Modifiers, MouseButton};
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let at = point(
        roll.origin.x + roll.size.width * 0.25,
        roll.origin.y + roll.size.height * 0.75,
    );
    visual.simulate_event(ScrollWheelEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.0), px(80.0))),
        modifiers: Modifiers {
            alt: true,
            ..Default::default()
        },
        ..Default::default()
    });
    visual.run_until_parked();
    let expected_pitch = editor.read_with(&visual, |v, _| {
        v.piano_pitch_low + v.piano_pitch_rows
            - 1
            - (f32::from(v.piano_pitch_rows) * 0.75).floor() as u8
    });
    double_down(&mut visual, at, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
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
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].pitch(), expected_pitch);
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

#[path = "midi_pitch_capture_ui_tests.rs"]
mod capture_tests;
