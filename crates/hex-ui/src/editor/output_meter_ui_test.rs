use super::{clip_drag_ui_test::editor_window, refresh::AudioFrame};
use crate::output_meter::{HEIGHT, MeterInput, WIDTH, readout};
use gpui::{TestAppContext, VisualTestContext, px, size};
use hex_audio::prepared::Status;
use hex_project::Project;
use std::sync::{Arc, atomic::Ordering::Relaxed};
use std::time::Instant;

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
        (0.0, 0.000_001, "last l -inf r <-60 dbfs", 0.0, false),
        (0.1, 1.0, "last l -20.0 r 0.0 dbfs", 16.0 * 2.0 / 3.0, false),
        (2.0, 0.5, "last l +6.0! r -6.0 dbfs", 16.0, true),
        (0.0, 0.0, "last l -inf r -inf dbfs", 0.0, false),
        (f32::NAN, -1.0, "last l invalid r invalid dbfs", 0.0, false),
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
