//! Main-meter rendered integration coverage.
use crate::editor::refresh_state::AudioFrame;
use std::time::Instant;

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use hex_audio::prepared::Status;
    use std::sync::atomic::Ordering::Relaxed;

    fn frame(source: u64, peak: [f32; 2]) -> AudioFrame {
        let status = Status::default();
        status.peak_left.store(peak[0].to_bits(), Relaxed);
        status.peak_right.store(peak[1].to_bits(), Relaxed);
        let mut frame = AudioFrame::capture(&status);
        frame.source = source;
        frame
    }

    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn actual_main_meter_uses_shared_geometry_in_both_views_and_clears_clipping(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::editor::{
            EditorMode,
            clip_drag_ui_test::{audio_project, editor_window},
        };
        use gpui::{Modifiers, VisualTestContext, px, size};
        let (project, first, _, _) = audio_project();
        let (handle, editor) = editor_window(cx, project, Default::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(960.0), px(600.0)));
        let now = Instant::now();
        let before = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.snapshot()
        });
        for mode in [EditorMode::Arrangement, EditorMode::Session] {
            editor.update(&mut visual, |view, cx| {
                view.observe_audio_frame(Some(frame(2, [0.0; 2])), false, now);
                cx.notify();
            });
            visual.run_until_parked();
            editor.update(&mut visual, |view, cx| {
                view.mode = mode;
                view.selected_track = Some(first);
                // A missing track observation must not erase final preview peaks.
                view.observe_audio_frame(Some(frame(1, [2.0, 0.25])), false, now);
                view.track_meters.clear();
                cx.notify();
            });
            visual.run_until_parked();
            let meter = visual
                .debug_bounds("main-output-meter")
                .expect("main stereo well");
            let ordinary = visual
                .debug_bounds(Box::leak(
                    format!("track-output-meter-{}", first.value()).into_boxed_str(),
                ))
                .unwrap();
            assert_eq!(meter.size.width, ordinary.size.width);
            let owner = visual
                .debug_bounds(if mode == EditorMode::Arrangement {
                    "main-track-header"
                } else {
                    "console-main-mixer-column"
                })
                .unwrap();
            assert!(meter.left() >= owner.left() && meter.right() <= owner.right());
            assert!(meter.top() >= owner.top() && meter.bottom() <= owner.bottom());
            let left = visual.debug_bounds("main-output-meter-left-fill").unwrap();
            let right = visual.debug_bounds("main-output-meter-right-fill").unwrap();
            assert!(left.size.height > right.size.height);
            let cap = visual
                .debug_bounds("main-output-meter-left-above")
                .expect("clipping cap");
            visual.simulate_click(cap.center(), Modifiers::default());
            visual.run_until_parked();
            assert!(
                visual
                    .debug_bounds("main-output-meter-left-above")
                    .is_none()
            );
            editor.read_with(&visual, |view, _| {
                assert_eq!(view.selected_track, Some(first));
                assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before)
            });
        }
        editor.update(&mut visual, |view, cx| {
            let mut failed = frame(1, [2.0; 2]);
            failed.render_fault = true;
            view.observe_audio_frame(Some(failed), false, now);
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(
            visual
                .debug_bounds("main-output-meter-left-fill")
                .unwrap()
                .size
                .height,
            px(0.0)
        );
    }
}
