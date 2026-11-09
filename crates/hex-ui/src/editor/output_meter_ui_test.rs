use super::{clip_drag_ui_test::editor_window, refresh::AudioFrame};
use crate::output_meter::{HEIGHT, MeterInput, WIDTH, readout};
use gpui::{Modifiers, TestAppContext, VisualTestContext, px, size};
use hex_audio::prepared::Status;
use hex_project::Project;
use std::sync::{Arc, atomic::Ordering::Relaxed};
use std::time::Instant;

#[gpui::test]
fn changed_stream_media_status_reaches_editor_error(cx: &mut TestAppContext) {
    let _output_serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let client = output.take_client().unwrap();
    let (_, editor) = editor_window(
        cx,
        Project::new(48_000).unwrap(),
        Arc::new(Default::default()),
    );
    editor.update(cx, |view, cx| {
        view.audio = Some(crate::editor_backend::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "test output".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::VerifiedStreaming,
        });
        view.tick(cx);
        assert!(
            !view
                .error
                .as_deref()
                .is_some_and(|error| error.contains("original media changed"))
        );
        view.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .stream_source_changed
            .store(true, Relaxed);
        view.tick(cx);
        assert_eq!(
            view.error.as_deref(),
            Some("original media changed · reopen or relink it")
        );
        assert_eq!(view.status, "streamed media unavailable");
        view.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .stream_source_changed
            .store(false, Relaxed);
        view.tick(cx);
        assert_eq!(view.status, "audio ready");
        assert_eq!(view.error.as_deref(), None);
        view.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .stream_source_changed
            .store(true, Relaxed);
        view.tick(cx);
        view.status = "saving project".into();
        view.error = Some("another error".into());
        view.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .stream_source_changed
            .store(false, Relaxed);
        view.tick(cx);
        assert_eq!(view.status, "saving project");
        assert_eq!(view.error.as_deref(), Some("another error"));
        view.audio.take().unwrap().control.close();
    });
    drop(output);
    backend.close();
}

#[gpui::test]
fn output_meter_status_snapshot_reaches_actual_editor_at_minimum_size(cx: &mut TestAppContext) {
    let (handle, editor) = editor_window(
        cx,
        Project::new(48_000).unwrap(),
        Arc::new(Default::default()),
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(960.0), px(600.0)));
    visual.run_until_parked();
    let slot = visual
        .debug_bounds("output-meter")
        .expect("actual editor meter");
    assert_eq!(slot.size, size(px(WIDTH), px(HEIGHT)));
    let revision = visual.debug_bounds("editor-revision-status").unwrap();
    let diagnostics = visual.debug_bounds("editor-audio-diagnostics").unwrap();
    assert!(revision.right() <= slot.left());
    assert!(slot.right() <= diagnostics.left());
    assert!(slot.left() >= px(0.0) && diagnostics.right() <= px(960.0));
    let bar = visual.debug_bounds("editor-status-bar").unwrap();
    assert!(slot.top() >= bar.top() - px(1.0) && slot.bottom() <= bar.bottom() + px(1.0));
    editor.read_with(cx, |view, _| assert!(view.output_meter_frame.is_none()));

    // Actual Status-boundary fixtures, not simulated CPAL/native delivery.
    let status = Status::default();
    status.playing.store(true, Relaxed);
    for (left, right, text, height, above) in [
        (0.0, 0.000_001_f32, "last l -inf r <-60 dbfs", 0.0, false),
        (0.1, 1.0, "last l -20.0 r 0.0 dbfs", 16.0 * 2.0 / 3.0, false),
        (2.0, 0.5, "last l +6.0! r -6.0 dbfs", 16.0, true),
        (0.0, 0.0, "last l -inf r -inf dbfs", 0.0, true),
        (f32::NAN, -1.0, "last l invalid r invalid dbfs", 0.0, true),
    ] {
        status.peak_left.store(left.to_bits(), Relaxed);
        status.peak_right.store(right.to_bits(), Relaxed);
        let frame = AudioFrame::capture(&status);
        editor.update(cx, |view, cx| {
            assert!(view.observe_audio_frame(Some(frame), false, Instant::now()));
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(
                readout(view.output_meter_frame.unwrap().meter_input()),
                text
            )
        });
        assert_eq!(visual.debug_bounds("output-meter").unwrap(), slot);
        assert_eq!(
            visual.debug_bounds("editor-audio-diagnostics").unwrap(),
            diagnostics
        );
        let fill = visual.debug_bounds("output-meter-left-fill").unwrap();
        assert!((f32::from(fill.size.height) - height).abs() < 1.0);
        assert_eq!(
            visual.debug_bounds("output-meter-left-above").is_some(),
            above
        );
    }
    let revision_before = editor.read_with(cx, |view, _| view.state().map(|state| state.revision));
    let cap = visual
        .debug_bounds("output-meter-left-above")
        .expect("held clipping warning");
    visual.simulate_click(cap.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("output-meter-left-above").is_none());
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state().map(|state| state.revision), revision_before);
        assert_eq!(
            readout(view.output_meter_frame.unwrap().meter_input()),
            "last l invalid r invalid dbfs"
        );
    });
    status.render_fault.store(true, Relaxed);
    let frame = AudioFrame::capture(&status);
    editor.update(cx, |view, cx| {
        assert!(view.observe_audio_frame(Some(frame), false, Instant::now()));
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(frame.meter_input(), MeterInput::Fault);
    assert_eq!(
        visual
            .debug_bounds("output-meter-left-fill")
            .unwrap()
            .size
            .height,
        px(0.0)
    );
    editor.update(cx, |view, cx| {
        assert!(view.observe_audio_frame(None, false, Instant::now()));
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(view.output_meter_frame.is_none()));
    assert_eq!(visual.debug_bounds("output-meter").unwrap(), slot);
}

#[gpui::test]
fn stream_starvation_lights_compact_disk_overload_until_hold_expires(cx: &mut TestAppContext) {
    let (handle, editor) = editor_window(
        cx,
        Project::new(48_000).unwrap(),
        Arc::new(Default::default()),
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(960.0), px(600.0)));
    visual.run_until_parked();
    let slot = visual.debug_bounds("editor-disk-overload").unwrap();
    let meter = visual.debug_bounds("output-meter").unwrap();
    let diagnostics = visual.debug_bounds("editor-audio-diagnostics").unwrap();
    assert_eq!(slot.size.width, px(super::render::DISK_OVERLOAD_WIDTH));
    assert!(meter.right() <= slot.left() && slot.right() <= diagnostics.left());
    assert!(visual.debug_bounds("editor-disk-overload-lit").is_none());

    // Actual Status-boundary fixture: the callback counter, not simulated delivery.
    let status = Status::default();
    status.playing.store(true, Relaxed);
    let start = Instant::now();
    let mut observe = |status: &Status, source: u64, at: Instant| {
        let mut frame = AudioFrame::capture(status);
        frame.source = source;
        let redraw = editor.update(&mut visual, |view, cx| {
            let redraw = view.observe_audio_frame(Some(frame), false, at);
            cx.notify();
            redraw
        });
        visual.run_until_parked();
        let lit = visual.debug_bounds("editor-disk-overload-lit");
        assert_eq!(visual.debug_bounds("editor-disk-overload").unwrap(), slot);
        assert_eq!(
            visual.debug_bounds("editor-audio-diagnostics").unwrap(),
            diagnostics
        );
        (redraw, lit)
    };
    assert!(observe(&status, 1, start).1.is_none());
    status.stream_starved_callbacks.fetch_add(4, Relaxed);
    let (redraw, lit) = observe(&status, 1, start);
    assert!(redraw);
    let lit = lit.expect("starvation lights the indicator");
    assert!(lit.left() >= slot.left() && lit.right() <= slot.right());
    // Still lit just before the hold expires; expiry is its own redraw.
    let hold = super::disk_overload::HOLD;
    let (redraw, lit) = observe(
        &status,
        1,
        start + hold - std::time::Duration::from_millis(1),
    );
    assert!(!redraw && lit.is_some());
    let (redraw, lit) = observe(&status, 1, start + hold);
    assert!(redraw && lit.is_none());

    // The next output can already have a larger count at its first poll.
    let replacement = Status::default();
    replacement.playing.store(true, Relaxed);
    replacement.stream_starved_callbacks.store(9, Relaxed);
    let later = start + hold;
    status.stream_starved_callbacks.fetch_add(1, Relaxed);
    assert!(observe(&status, 1, later).1.is_some());
    let (redraw, lit) = observe(&replacement, 2, later);
    assert!(redraw && lit.is_none());
    replacement.stream_starved_callbacks.fetch_add(1, Relaxed);
    assert!(observe(&replacement, 2, later).1.is_some());
    editor.read_with(cx, |view, _| assert_eq!(view.disk_overload.count(), 10));
}
