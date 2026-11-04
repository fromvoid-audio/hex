//! Real wheel dispatch over the rendered piano roll and keys: plain scrolls
//! pitch, horizontal or Shift pans time, and a held note gesture survives.
use super::super::midi_canvas::creation_tests::{double_down, fixture, pump};
use gpui::{
    Modifiers, MouseButton, ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext,
    point, px,
};
use hex_project::BeatTime;

fn wheel(
    visual: &mut VisualTestContext,
    at: gpui::Point<gpui::Pixels>,
    x: f32,
    y: f32,
    m: Modifiers,
) {
    visual.simulate_event(ScrollWheelEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(x), px(y))),
        modifiers: m,
        ..Default::default()
    });
    visual.run_until_parked();
}

#[gpui::test]
fn plain_vertical_wheel_scrolls_pitch_over_roll_and_keys(cx: &mut TestAppContext) {
    for surface in ["midi-note-roll", "midi-piano-keys"] {
        let (mut visual, editor, _environment, _) = fixture(cx);
        let at = visual.debug_bounds(surface).unwrap().center();
        let before = editor.read_with(&visual, |v, _| {
            (
                v.piano_pitch_low,
                v.piano_pitch_rows,
                v.piano_beat_start,
                v.piano_beat_span,
                v.state.as_ref().unwrap().project.snapshot(),
            )
        });
        // Positive delta reveals the rows above: higher pitches.
        wheel(&mut visual, at, 0.0, 120.0, Modifiers::default());
        let raised = editor.read_with(&visual, |v, _| {
            assert!(
                v.piano_pitch_low > before.0,
                "{surface}: plain wheel scrolls pitch"
            );
            assert_eq!(v.piano_pitch_rows, before.1, "{surface}: rows unchanged");
            assert_eq!(
                (v.piano_beat_start, v.piano_beat_span),
                (before.2, before.3),
                "{surface}: time unchanged"
            );
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before.4);
            v.piano_pitch_low
        });
        wheel(&mut visual, at, 0.0, -240.0, Modifiers::default());
        editor.read_with(&visual, |v, _| assert!(v.piano_pitch_low < raised));
        // Scrolling far past the bottom clamps at MIDI 0 without banking motion.
        for _ in 0..40 {
            wheel(&mut visual, at, 0.0, -400.0, Modifiers::default());
        }
        editor.read_with(&visual, |v, _| assert_eq!(v.piano_pitch_low, 0));
        wheel(&mut visual, at, 0.0, 120.0, Modifiers::default());
        editor.read_with(&visual, |v, _| assert!(v.piano_pitch_low > 0));
    }
}

/// The canvas sits inside the roll's border, so pixels map to beats within 1%.
fn beats(start: BeatTime, expected: f64) -> bool {
    (start.raw() as f64 / BeatTime::ONE.raw() as f64 - expected).abs() < 0.01 * expected
}

#[gpui::test]
fn horizontal_swipe_and_shift_wheel_pan_time_without_zoom(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _) = fixture(cx);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let width = f32::from(roll.size.width);
    let before = editor.read_with(&visual, |v, _| {
        (v.piano_pitch_low, v.piano_pitch_rows, v.piano_beat_span)
    });
    // Negative x reveals later beats: a quarter of the width is one beat here.
    wheel(
        &mut visual,
        roll.center(),
        -width / 4.0,
        0.0,
        Modifiers::default(),
    );
    editor.read_with(&visual, |v, _| {
        assert!(beats(v.piano_beat_start, 1.0), "swipe pans time");
        assert_eq!(
            (v.piano_pitch_low, v.piano_pitch_rows, v.piano_beat_span),
            before
        );
    });
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };
    wheel(&mut visual, roll.center(), 0.0, -width / 4.0, shift);
    editor.read_with(&visual, |v, _| {
        assert!(beats(v.piano_beat_start, 2.0), "shift pans time");
        assert_eq!(
            (v.piano_pitch_low, v.piano_pitch_rows, v.piano_beat_span),
            before
        );
    });
    // Panning before the start clamps at zero.
    wheel(
        &mut visual,
        roll.center(),
        4.0 * width,
        0.0,
        Modifiers::default(),
    );
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.piano_beat_start, BeatTime::ZERO)
    });
    // Over the keys only the pitch axis applies; a swipe there is ignored.
    let keys = visual.debug_bounds("midi-piano-keys").unwrap();
    wheel(
        &mut visual,
        keys.center(),
        -width,
        0.0,
        Modifiers::default(),
    );
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.piano_beat_start, BeatTime::ZERO)
    });
}

#[gpui::test]
fn held_note_gesture_survives_every_wheel_mapping_inside_the_roll(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let at = |fraction: f32| {
        point(
            roll.origin.x + roll.size.width * fraction,
            roll.origin.y + roll.size.height * 0.52,
        )
    };
    let viewport = |visual: &mut VisualTestContext| {
        editor.read_with(visual, |v, _| {
            (
                v.piano_pitch_low,
                v.piano_pitch_rows,
                v.piano_beat_start,
                v.piano_beat_span,
            )
        })
    };
    let before = viewport(&mut visual);
    double_down(&mut visual, at(0.25), Modifiers::default());
    assert!(editor.read_with(&visual, |v, _| v.midi_creation.is_some()));
    for modifiers in [
        Modifiers::default(),
        Modifiers {
            shift: true,
            ..Default::default()
        },
        Modifiers::command(),
        Modifiers {
            alt: true,
            ..Default::default()
        },
    ] {
        wheel(&mut visual, at(0.4), -60.0, 160.0, modifiers);
        assert!(
            editor.read_with(&visual, |v, _| v.midi_creation.is_some()),
            "{modifiers:?}: wheel must not cancel the held note"
        );
        assert_eq!(
            viewport(&mut visual),
            before,
            "{modifiers:?}: view stays fixed"
        );
    }
    visual.simulate_mouse_move(at(0.5), Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(at(0.5), MouseButton::Left, Modifiers::default());
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
        assert_eq!(notes.len(), 1, "the held creation commits after scrolling");
        assert!(v.midi_creation.is_none());
    });
    // Released input navigates normally again.
    wheel(&mut visual, at(0.4), 0.0, 120.0, Modifiers::default());
    assert!(viewport(&mut visual).0 > before.0);
}
