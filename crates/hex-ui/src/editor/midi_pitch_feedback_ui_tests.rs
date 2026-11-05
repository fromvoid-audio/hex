//! Native decoder ingress, completed paint, and the real step-entry worker terminal.
use super::*;
use crate::editor::midi_canvas::creation_tests::{fixture, pump};
use gpui::{KeyDownEvent, KeyUpEvent, Keystroke, TestAppContext, VisualTestContext};
use hex_project::{BeatTime, MidiNoteInsert};

fn send(decoder: &mut hex_midi::Decoder, input: &hex_midi::HeldNoteInput, bytes: &[u8]) {
    hex_midi::deliver_packets(
        decoder,
        2,
        hex_midi::Controllers::default(),
        [bytes],
        input,
        &|_| {},
    );
}
fn repaint(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    editor.update(&mut *visual, |view, cx| {
        if view.tick_midi_pitch_feedback() {
            cx.notify();
        }
    });
    visual.run_until_parked();
}
fn assert_lines(
    visual: &mut VisualTestContext,
    notes: &[MidiNoteInsert],
    start: u64,
    span: u64,
    low: u8,
    rows: u8,
) {
    let bounds = visual.debug_bounds("midi-note-roll").unwrap();
    let scale = gpui::ScaledPixels::from(visual.update(|window, _| window.scale_factor()));
    let color: gpui::Background = rgb(theme::color(theme::PaletteRole::Primary)).into();
    let lines: Vec<_> = visual
        .rendered_filled_quads("midi-note-roll")
        .into_iter()
        .filter(|(b, c)| c == &color && (b.size.height / scale - 2.0).abs() < 0.01)
        .collect();
    let visible: Vec<_> = notes
        .iter()
        .filter(|n| n.pitch >= low && u16::from(n.pitch) < u16::from(low) + u16::from(rows))
        .collect();
    assert_eq!(lines.len(), visible.len());
    for note in visible {
        let left = bounds.left()
            + gpui::px(1.0)
            + (bounds.size.width - gpui::px(2.0))
                * ((note.start.raw() - start) as f64 / span as f64) as f32;
        let width =
            (bounds.size.width - gpui::px(2.0)) * (note.duration.raw() as f64 / span as f64) as f32;
        let center = bounds.top()
            + gpui::px(1.0)
            + (bounds.size.height - gpui::px(2.0))
                * (f32::from(low.saturating_add(rows - 1) - note.pitch) + 0.5)
                / f32::from(rows);
        assert!(
            lines
                .iter()
                .any(|(b, _)| (b.origin.x / scale - f32::from(left)).abs() < 0.1
                    && (b.size.width / scale - f32::from(width)).abs() < 0.1
                    && (b.center().y / scale - f32::from(center)).abs() < 0.1),
            "missing {note:?}: {lines:?}"
        );
        let selector = match note.pitch {
            60 => "midi-piano-key-60",
            64 => "midi-piano-key-64",
            99 => "midi-piano-key-99",
            _ => panic!("fixture pitch"),
        };
        let key_color = visual.rendered_filled_quads(selector);
        assert!(
            key_color.iter().any(|(_, c)| c == &color),
            "held key must paint before release"
        );
    }
}

#[gpui::test]
fn decoder_and_computer_chord_paint_matches_right_arrow_after_real_ack(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let input = Arc::new(hex_midi::HeldNoteInput::default());
    editor.update(&mut visual, |view, cx| {
        view.midi_step.enabled = true;
        view.controller_input.channel = 2;
        view.controller_input.held_notes = Some(Arc::clone(&input));
        view.controller_input.target = view.controller_target();
        view.piano_pitch_low = 48;
        view.piano_pitch_rows = 36;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("m");
    visual.simulate_event(KeyDownEvent {
        keystroke: Keystroke::parse("a").unwrap(),
        is_held: false,
    });
    let mut decoder = hex_midi::Decoder::default();
    send(&mut decoder, &input, &[0x92, 60, 37, 64, 93, 99, 88]);
    repaint(&editor, &mut visual);
    let before = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    for (cursor, start, span, low, rows) in [
        (BeatTime::ONE.raw(), 0, 4 * BeatTime::ONE.raw(), 48, 36),
        (
            2 * BeatTime::ONE.raw(),
            BeatTime::ONE.raw(),
            2 * BeatTime::ONE.raw(),
            58,
            12,
        ),
        (
            2 * BeatTime::ONE.raw(),
            BeatTime::ONE.raw(),
            2 * BeatTime::ONE.raw(),
            0,
            128,
        ),
    ] {
        editor.update(&mut visual, |view, cx| {
            view.midi_marker = Some(cursor);
            view.piano_beat_start = i128::from(start);
            view.piano_beat_span = BeatTime::from_raw(span);
            view.piano_pitch_low = low;
            view.piano_pitch_rows = rows;
            cx.notify();
        });
        visual.run_until_parked();
        let chord = editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
            assert!(!view.edit_pending());
            view.midi_step_preview()
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
        });
        assert_eq!(
            chord
                .iter()
                .map(|n| (n.pitch, n.velocity, n.channel))
                .collect::<Vec<_>>(),
            [(60, 100, 0), (64, 93, 2), (99, 88, 2)]
        );
        assert_lines(&mut visual, &chord, start, span, low, rows);
    }
    let preview = editor.read_with(&visual, |v, _| {
        v.midi_step_preview()
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
    });
    visual.simulate_keystrokes("right");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let source = view
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap();
        assert_eq!(
            source
                .notes()
                .iter()
                .map(|n| MidiNoteInsert {
                    active: true,
                    start: n.start(),
                    duration: n.duration(),
                    pitch: n.pitch(),
                    channel: n.channel(),
                    velocity: n.velocity()
                })
                .collect::<Vec<_>>(),
            preview
        );
    });
    visual.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("a").unwrap(),
    });
    send(&mut decoder, &input, &[0xb2, 123, 0]);
    repaint(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.midi_step_preview().iter().all(Option::is_none));
        assert!(!view.midi_live_pitches().into_iter().any(|p| p));
    });
}

#[gpui::test]
fn live_feedback_clears_on_release_context_and_endpoint_cancellation(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _) = fixture(cx);
    let input = Arc::new(hex_midi::HeldNoteInput::default());
    editor.update(&mut visual, |view, cx| {
        view.controller_input.channel = 2;
        view.controller_input.held_notes = Some(Arc::clone(&input));
        view.controller_input.target = view.controller_target();
        cx.notify();
    });
    visual.run_until_parked();
    let mut decoder = hex_midi::Decoder::default();
    send(&mut decoder, &input, &[0x93, 60, 90]); // wrong channel
    repaint(&editor, &mut visual);
    editor.read_with(&visual, |v, _| assert!(!v.midi_live_pitches()[60]));
    send(&mut decoder, &input, &[0x92, 60, 90]);
    repaint(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_live_pitches()[60]);
        assert!(v.midi_step_preview().iter().all(Option::is_none));
    });
    let toggle = visual.debug_bounds("toggle step entry · enable computer keyboard with m · hold notes and press right · no notes inserts a rest").unwrap();
    visual.simulate_click(toggle.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    // Enabling step entry keeps its existing fresh-press floor.
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_step_preview().iter().all(Option::is_none))
    });
    send(&mut decoder, &input, &[0x82, 60, 0, 0x92, 60, 90]);
    repaint(&editor, &mut visual);
    editor.read_with(&visual, |v, _| assert!(v.midi_step_preview()[60].is_some()));
    visual.simulate_click(toggle.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_step_preview().iter().all(Option::is_none));
    });
    assert_lines(&mut visual, &[], 0, 1, 0, 128);
    // Toolbar focus transitions retain the existing fresh-press policy.
    send(&mut decoder, &input, &[0x82, 60, 0, 0x92, 60, 90]);
    repaint(&editor, &mut visual);
    editor.read_with(&visual, |v, _| assert!(v.midi_live_pitches()[60]));
    send(&mut decoder, &input, &[0x82, 60, 0]);
    repaint(&editor, &mut visual);
    editor.read_with(&visual, |v, _| assert!(!v.midi_live_pitches()[60]));
    send(&mut decoder, &input, &[0x92, 60, 90]);
    repaint(&editor, &mut visual);
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            v.focus.focus(window);
            cx.notify();
        })
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert!(!v.midi_live_pitches()[60]));
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            v.midi_focus.focus(window);
            cx.notify();
        })
    });
    visual.run_until_parked();
    send(&mut decoder, &input, &[0x92, 64, 90]);
    repaint(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(!v.midi_live_pitches()[60]);
        assert!(v.midi_live_pitches()[64]);
    });
    input.cancel();
    repaint(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(!v.midi_live_pitches().into_iter().any(|p| p))
    });
}
