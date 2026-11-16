//! Connect control-side audio observations to editor redraw state.
use std::time::Instant;

pub(super) use super::refresh_state::{AudioFrame, RedrawGate};

impl super::EditorView {
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
}
