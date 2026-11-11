//! Actual consumer surfaces share compact painted rails; domain/input behavior
//! remains covered by the existing Arrangement and clip-editor worker tests.
use super::*;
use gpui::{TestAppContext, VisualTestContext, px, size};

fn surface(visual: &mut VisualTestContext, selector: &'static str, height: f32) {
    assert_eq!(
        visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("missing {selector}"))
            .size
            .height,
        px(height)
    );
    assert_eq!(
        visual.rendered_background(selector),
        Some(gpui::rgb(theme::color(theme::PaletteRole::Surface)).into()),
        "shared surface: {selector}",
    );
    let (_, widths, _) = visual.rendered_border(selector).unwrap();
    assert!(widths.bottom > gpui::ScaledPixels::default());
}

#[gpui::test]
fn arrangement_audio_and_midi_paint_the_same_compact_timeline_surfaces(cx: &mut TestAppContext) {
    {
        let cx = &mut cx.new_app();
        let (mut visual, editor, _environment, _) = midi_canvas::creation_tests::fixture(cx);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        for width in [1280.0, 960.0] {
            visual.simulate_resize(size(px(width), px(800.0)));
            visual.run_until_parked();
            surface(&mut visual, "arrangement-time-ruler", 24.0);
            surface(&mut visual, "arrangement-loop-rail-row", 18.0);
            surface(&mut visual, "midi-source-ruler", 24.0);
            surface(&mut visual, "midi-loop-ruler", 18.0);
        }
        assert!(visual.debug_bounds("midi-loop-start-marker").is_some());
        assert!(visual.debug_bounds("midi-loop-end-marker").is_some());
        editor.update(&mut visual, |v, cx| {
            v.piano_beat_start = i128::from(hex_project::BeatTime::ONE.raw());
            v.piano_beat_span = hex_project::BeatTime::from_quarters(2).unwrap();
            cx.notify();
        });
        visual.run_until_parked();
        assert!(visual.debug_bounds("midi-loop-start-marker").is_none());
        assert!(visual.debug_bounds("midi-loop-end-marker").is_none());
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
    }
    {
        let cx = &mut cx.new_app();
        let (_files, handle, editor) = clip_drag_ui_test::upper_rail_owned_fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(&mut visual, |v, cx| {
            let track = &v.state.as_ref().unwrap().project.tracks()[0];
            v.selected_track = Some(track.id());
            v.selected_clip = Some(track.clips()[0].id());
            v.clip_view_open = true;
            v.device_panel_active = false;
            cx.notify();
        });
        visual.simulate_resize(size(px(1280.0), px(800.0)));
        visual.run_until_parked();
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        surface(&mut visual, "audio-events-time-ruler", 24.0);
        surface(&mut visual, "audio-loop-ruler", 18.0);
        editor.update(&mut visual, |v, cx| {
            v.audio_event_editor.source_view = true;
            cx.notify();
        });
        visual.run_until_parked();
        surface(&mut visual, "arrangement-audio-time-ruler", 24.0);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
    }
}

#[gpui::test]
fn loop_endpoints_exactly_at_viewport_edges_remain_painted(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = midi_canvas::creation_tests::fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::SetMidiClipSource {
            clip,
            source_length: hex_project::BeatTime::from_quarters(4).unwrap(),
            source_start: hex_project::BeatTime::ZERO,
            loop_start: hex_project::BeatTime::from_quarters(2).unwrap(),
            loop_end: hex_project::BeatTime::from_quarters(4).unwrap(),
            looping: true,
            length: hex_project::BeatTime::from_quarters(4).unwrap(),
        })));
    });
    midi_canvas::creation_tests::pump(&editor, &mut visual);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    for (first, marker, other, at_right) in [
        (0, "midi-loop-start-marker", "midi-loop-end-marker", true),
        (4, "midi-loop-end-marker", "midi-loop-start-marker", false),
    ] {
        editor.update(&mut visual, |v, cx| {
            v.piano_beat_start =
                i128::from(hex_project::BeatTime::from_quarters(first).unwrap().raw());
            v.piano_beat_span = hex_project::BeatTime::from_quarters(2).unwrap();
            cx.notify();
        });
        visual.run_until_parked();
        let strip = visual.debug_bounds("midi-loop-ruler").unwrap();
        let cap = visual
            .debug_bounds(marker)
            .unwrap_or_else(|| panic!("missing {marker}"));
        assert!(cap.left() >= strip.left() && cap.right() <= strip.right());
        assert!(cap.top() >= strip.top() && cap.bottom() <= strip.bottom());
        assert_eq!(
            if at_right { cap.right() } else { cap.left() },
            if at_right {
                strip.right()
            } else {
                strip.left()
            }
        );
        assert!(!visual.rendered_quad_paint_layers(marker, false).is_empty());
        assert!(visual.debug_bounds(other).is_none());
    }
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
}
