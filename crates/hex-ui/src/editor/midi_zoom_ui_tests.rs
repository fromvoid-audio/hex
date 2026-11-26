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
            v.midi_editor.piano_beat_span.raw() < 4 * BeatTime::ONE.raw(),
            "real piano wheel must zoom"
        );
        let anchor = v.midi_editor.piano_beat_start as f64
            + 0.75 * v.midi_editor.piano_beat_span.raw() as f64;
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
        assert_eq!(v.midi_editor.piano_beat_start, 0);
        assert_eq!(
            v.midi_editor.piano_beat_span,
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
    let zoom = editor.read_with(&visual, |v, _| {
        (
            v.midi_editor.piano_beat_start,
            v.midi_editor.piano_beat_span,
        )
    });
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
            (
                v.midi_editor.piano_beat_start,
                v.midi_editor.piano_beat_span
            ),
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
        assert_eq!(
            v.midi_editor.piano_beat_span,
            BeatTime::from_quarters(32).unwrap()
        )
    });
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            assert!(v.set_selected_clip(v.selected_track.unwrap(), original, window, cx));
            cx.notify();
        })
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.midi_editor.piano_beat_span,
            BeatTime::from_quarters(4).unwrap()
        )
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
            assert_eq!(
                v.midi_editor.piano_beat_span,
                BeatTime::from_quarters(4).unwrap()
            )
        });
    }
    visual.simulate_event(ScrollWheelEvent {
        position: at(0.75),
        delta: ScrollDelta::Pixels(point(px(0.0), px(20.0))),
        modifiers: cmd(),
        ..Default::default()
    });
    visual.run_until_parked();
    let viewport = editor.read_with(&visual, |v, _| {
        (
            v.midi_editor.piano_beat_start,
            v.midi_editor.piano_beat_span,
        )
    });
    double_down(&mut visual, at(0.25), Modifiers::default());
    let expected_start = BeatTime::from_raw(BeatTime::ONE.raw() * 5 / 4);
    let target_end = i128::from(expected_start.raw() + BeatTime::ONE.raw());
    let fraction = ((target_end - viewport.0) as f64 / viewport.1.raw() as f64) as f32;
    let end = point(
        roll.left() + px(1.0) + (roll.size.width - px(2.0)) * fraction,
        at(0.25).y,
    );
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    let (note_center, after) = editor.read_with(&visual, |v, _| {
        assert_eq!(
            (
                v.midi_editor.piano_beat_start,
                v.midi_editor.piano_beat_span
            ),
            viewport
        );
        let state = v.state.as_ref().unwrap();
        let notes = state.project.midi_source(clip).unwrap().notes();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].start(), expected_start);
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
        assert_eq!(
            v.midi_editor.piano_beat_span,
            BeatTime::from_quarters(4).unwrap()
        )
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
    let span = editor.read_with(&visual, |v, _| v.midi_editor.piano_beat_span);
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
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_editor.piano_beat_span, span)
    });
}

/// Real gutter drag changes pitch geometry; plain wheel stays horizontal and edits
/// still hit the same displayed note through the real worker/Undo path.
#[gpui::test]
fn piano_key_drag_zooms_pitch_and_preserves_note_editing(cx: &mut TestAppContext) {
    use super::creation_tests::{double_down, first_open_fixture};
    use gpui::{Modifiers, MouseButton};
    let (mut visual, editor, _environment, clip) = first_open_fixture(cx);
    let keys = visual.debug_bounds("midi-piano-keys").unwrap();
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let (prior, before) = editor.read_with(&visual, |v, _| {
        (
            v.midi_editor.piano_pitch,
            v.state.as_ref().unwrap().project.snapshot(),
        )
    });
    let grid_height = f32::from(roll.size.height - px(2.0));
    assert_eq!(keys.size.height, px(grid_height));
    assert!((f64::from(grid_height) / prior.span() - 16.0).abs() < 0.01);
    let down = keys.center();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        down + point(px(0.0), px(120.0)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert_eq!(v.midi_editor.piano_pitch, prior));

    for dx in [1.0, 120.0] {
        visual.simulate_mouse_move(
            down + point(px(dx), px(40.0)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            let expected = prior.zoom_at(grid_height, 0.5, dx).unwrap();
            assert!(v.midi_editor.piano_pitch.span() < prior.span());
            assert_eq!(v.midi_editor.piano_pitch, expected);
            assert!(
                (v.midi_editor.piano_pitch.low() + v.midi_editor.piano_pitch.span() / 2.0 - 60.0)
                    .abs()
                    < 1.0 / 4096.0
            );
            assert_eq!(
                v.midi_editor.piano_beat_span,
                BeatTime::from_quarters(4).unwrap()
            );
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
    }
    let zoomed = prior.zoom_at(grid_height, 0.5, 120.0).unwrap();
    visual.simulate_mouse_up(
        down + point(px(120.0), px(40.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(keys.center(), None, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_editor.piano_pitch_zoom.is_none())
    });
    visual.simulate_event(ScrollWheelEvent {
        position: roll.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(80.0))),
        modifiers: cmd(),
        ..Default::default()
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_editor.piano_pitch, zoomed);
        assert!(v.midi_editor.piano_beat_span.raw() < 4 * BeatTime::ONE.raw());
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
        let x = (i128::from(note.start().raw() + note.duration().raw() / 2)
            - v.midi_editor.piano_beat_start) as f64
            / v.midi_editor.piano_beat_span.raw() as f64;
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
    for (distance, bound) in [(4000.0, 48.0), (-4000.0, 10.0)] {
        visual.simulate_mouse_down(keys.center(), MouseButton::Left, Modifiers::default());
        let end = keys.center() + point(px(distance), px(0.0));
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            let rows = v.midi_editor.piano_pitch.span();
            let row_height = f64::from(grid_height) / rows;
            assert!(rows >= 3.0);
            assert!((row_height - bound).abs() < 0.01);
        });
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
    }
    visual.simulate_mouse_down(keys.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_editor.piano_pitch_zoom.is_none())
    });
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
                beat_start: v.midi_editor.piano_beat_start,
                beat_span: v.midi_editor.piano_beat_span.raw(),
                pitch: v.midi_editor.piano_pitch,
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
        // Note-aware Z leaves a 1/16-beat margin after this quarter-beat
        // source, so each note occupies 80% of the fitted viewport.
        assert_eq!(widths(v), vec![800.0, 800.0]);
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
            widths.iter().all(|w| (*w - 640.0).abs() < 0.01),
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
    use super::creation_tests::first_open_fixture;
    use gpui::Modifiers;
    for surface in ["midi-note-roll", "midi-piano-keys"] {
        let (mut visual, editor, _environment, _) = first_open_fixture(cx);
        let bounds = visual.debug_bounds(surface).unwrap();
        let keys = visual.debug_bounds("midi-piano-keys").unwrap();
        let before = editor.read_with(&visual, |v, _| {
            (
                v.midi_editor.piano_pitch,
                v.midi_editor.piano_beat_start,
                v.midi_editor.piano_beat_span,
                v.state.as_ref().unwrap().project.snapshot(),
            )
        });
        let at = point(bounds.center().x, keys.origin.y + keys.size.height * 0.75);
        visual.simulate_event(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(0.0), px(1.0))),
            modifiers: Modifiers {
                alt: true,
                ..Default::default()
            },
            ..Default::default()
        });
        visual.run_until_parked();
        let pitch_view = editor.read_with(&visual, |v, _| {
            assert!(
                v.midi_editor.piano_pitch.span() < before.0.span(),
                "tiny Option-wheel on {surface}"
            );
            assert!(before.0.span() - v.midi_editor.piano_pitch.span() < 1.0);
            let old_anchor = before.0.low() + before.0.span() * 0.25;
            let new_anchor =
                v.midi_editor.piano_pitch.low() + v.midi_editor.piano_pitch.span() * 0.25;
            assert!((new_anchor - old_anchor).abs() < 1.0 / 4096.0);
            assert_eq!(
                (
                    v.midi_editor.piano_beat_start,
                    v.midi_editor.piano_beat_span
                ),
                (before.1, before.2)
            );
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before.3);
            v.midi_editor.piano_pitch
        });
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::RenameTrack {
                track: v.selected_track.unwrap(),
                name: "ack".into()
            })))
        });
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.midi_editor.piano_pitch, pitch_view)
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
            assert_eq!(v.midi_editor.piano_pitch, pitch_view);
            assert!(
                v.midi_editor.piano_beat_span < before.2,
                "cmd-wheel zooms time"
            );
        });
    }
}

#[gpui::test]
fn option_wheel_zooms_continuously_independent_of_time_and_respects_held_marquee(
    cx: &mut TestAppContext,
) {
    use super::creation_tests::first_open_fixture;
    use gpui::{Modifiers, MouseButton};
    let (mut visual, editor, _environment, _) = first_open_fixture(cx);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let before = editor.read_with(&visual, |v, _| {
        (
            v.midi_editor.piano_pitch,
            v.midi_editor.piano_beat_span,
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
    wheel(&mut visual, 1.0, true);
    let pitch = editor.read_with(&visual, |v, _| {
        assert!(v.midi_editor.piano_pitch.span() < before.0.span());
        assert_eq!(v.midi_editor.piano_beat_span, before.1);
        v.midi_editor.piano_pitch
    });
    wheel(&mut visual, 60.0, false);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_editor.piano_pitch, pitch);
        assert!(v.midi_editor.piano_beat_span < before.1);
    });
    let grid_height = f32::from(roll.size.height - px(2.0));
    for (delta, edge) in [(-320.0, 10.0), (320.0, 48.0)] {
        for _ in 0..4 {
            wheel(&mut visual, delta, true);
        }
        editor.read_with(&visual, |v, _| {
            let height = f64::from(grid_height) / v.midi_editor.piano_pitch.span();
            assert!((height - edge).abs() < 0.01);
        });
    }
    let saved = editor.read_with(&visual, |v, _| v.midi_editor.piano_pitch);
    editor.update(&mut visual, |v, _| {
        for (bounds, y, delta) in [
            ([0.0, 0.0, 100.0, 100.0], -1.0, 80.0),
            ([0.0, 0.0, 100.0, 100.0], 101.0, 80.0),
            ([0.0, 0.0, 100.0, 0.0], 0.0, 80.0),
            ([f32::NAN, 0.0, 100.0, 100.0], 50.0, 80.0),
            ([0.0, 0.0, 100.0, 100.0], 50.0, f32::INFINITY),
        ] {
            assert!(!v.zoom_piano_pitch_wheel_at(bounds, y, delta));
            assert_eq!(v.midi_editor.piano_pitch, saved);
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
    editor.read_with(&visual, |v, _| assert!(v.midi_editor.marquee.is_some()));
    wheel(&mut visual, -80.0, true);
    editor.read_with(&visual, |v, _| {
        assert!(
            v.midi_editor.marquee.is_some(),
            "wheel cannot cancel held selection"
        );
        assert_eq!(
            v.midi_editor.piano_pitch, saved,
            "wheel cannot zoom during capture"
        );
    });
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(roll.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    wheel(&mut visual, -80.0, true);
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_editor.marquee.is_none());
        assert!(
            v.midi_editor.piano_pitch.span() > saved.span(),
            "fresh wheel works after release"
        );
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
        v.midi_editor
            .piano_pitch
            .pitch_at_y(
                f32::from(at.y - roll.origin.y - px(1.0)),
                f32::from(roll.size.height - px(2.0)),
            )
            .unwrap()
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

#[gpui::test]
fn resized_grid_rebounds_manual_zoom_and_keeps_the_full_note_fit(cx: &mut TestAppContext) {
    use super::creation_tests::first_open_fixture;
    use gpui::{Modifiers, MouseButton};
    let (mut visual, editor, _environment, clip) = first_open_fixture(cx);
    let earlier = visual.debug_bounds("midi-piano-keys").unwrap();
    let alt = Modifiers {
        alt: true,
        ..Default::default()
    };
    visual.simulate_event(ScrollWheelEvent {
        position: earlier.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(-4000.0))),
        modifiers: alt,
        ..Default::default()
    });
    visual.run_until_parked();
    let resize_down = visual.debug_bounds("clip-detail-resize").unwrap().center();
    let resize_up = resize_down + point(px(0.0), px(50.0));
    visual.simulate_mouse_down(resize_down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(resize_up, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(resize_up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let smaller = visual.debug_bounds("midi-piano-keys").unwrap();
    assert!(smaller.size.height < earlier.size.height);
    editor.read_with(&visual, |view, _| {
        assert!(
            (f64::from(f32::from(smaller.size.height)) / view.midi_editor.piano_pitch.span()
                - 10.0)
                .abs()
                < 0.01
        );
    });

    editor.update(&mut visual, |view, _| {
        assert!(
            view.enqueue(Request::Edit(Command::AddMidiNotes {
                clip,
                notes: [0, 127]
                    .map(|pitch| hex_project::MidiNoteInsert {
                        active: true,
                        start: BeatTime::ZERO,
                        duration: BeatTime::ONE,
                        pitch,
                        channel: 0,
                        velocity: 90,
                    })
                    .into(),
            }))
        );
    });
    pump(&editor, &mut visual);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    visual.simulate_click(roll.center(), Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("z");
    visual.run_until_parked();
    let full = editor.read_with(&visual, |view, _| {
        assert!(view.midi_editor.piano_pitch_note_fit);
        assert_eq!(view.midi_editor.piano_pitch.coverage(), 0..128);
        view.midi_editor.piano_pitch
    });
    let resize_down = visual.debug_bounds("clip-detail-resize").unwrap().center();
    let resize_up = resize_down - point(px(0.0), px(30.0));
    visual.simulate_mouse_down(resize_down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(resize_up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.midi_editor.piano_pitch, full)
    });
}
