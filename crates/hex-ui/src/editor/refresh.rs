//! Connect control-side audio observations to editor redraw state.
use gpui::Context;
use std::time::Instant;

pub(super) use super::refresh_state::{AudioFrame, RedrawGate, capture_audio_frame};

impl super::EditorView {
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn observe_audio_frame(
        &mut self,
        frame: Option<AudioFrame>,
        visible_change: bool,
        now: Instant,
    ) -> bool {
        let cpu_changed = self.cpu_meter.observe(
            frame
                .filter(|f| !f.shutdown_complete && !f.device_fault)
                .map(|f| (f.source, f.cpu, f.deadline_misses)),
            now,
        );
        let main_changed = self.track_meters.observe_main(frame, now);
        let availability_changed = self.output_meter_frame.is_some() != frame.is_some();
        self.output_meter_frame = frame;
        let overload_changed = self
            .disk_overload
            .observe(frame.map(|frame| (frame.source, frame.stream_starved)), now);
        self.redraw_gate.should_redraw(
            visible_change
                || main_changed
                || availability_changed
                || overload_changed
                || cpu_changed,
            frame,
            now,
        )
    }

    /// Tick-path audio observation that routes 60 Hz position, CPU, output-meter,
    /// and track/Console meter updates to isolated GPUI entities instead of
    /// calling `cx.notify()` on root `EditorView`.
    pub(super) fn observe_audio_frame_for_tick(
        &mut self,
        frame: Option<AudioFrame>,
        visible_change: bool,
        track_meters_changed: bool,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> bool {
        let cpu_changed = self.cpu_meter.observe(
            frame
                .filter(|f| !f.shutdown_complete && !f.device_fault)
                .map(|f| (f.source, f.cpu, f.deadline_misses)),
            now,
        );
        let main_changed = self.track_meters.observe_main(frame, now);
        let availability_changed = self.output_meter_frame.is_some() != frame.is_some();
        self.output_meter_frame = frame;
        let overload_changed = self
            .disk_overload
            .observe(frame.map(|frame| (frame.source, frame.stream_starved)), now);
        let decision = self.redraw_gate.evaluate(
            visible_change || availability_changed || overload_changed,
            frame,
            now,
        );

        let notify_isolated = !decision.root;
        if decision.position || decision.root {
            self.sync_playhead_and_position_subviews(cx, notify_isolated);
        }
        if cpu_changed || decision.root {
            self.sync_cpu_meter_subview(cx, notify_isolated);
        }
        if main_changed || decision.output_meter || availability_changed || decision.root {
            self.sync_output_meter_subview(cx, notify_isolated);
        }
        if track_meters_changed || main_changed || decision.root {
            self.sync_live_meters_subview(cx, notify_isolated);
        }

        let inline_meters_changed =
            !self.uses_sibling_playback_overlay() && (track_meters_changed || main_changed);
        decision.root
            || inline_meters_changed
            || (decision.position && self.has_inline_playhead_panel())
    }
}
