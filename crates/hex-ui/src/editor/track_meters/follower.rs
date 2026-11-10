//! Display follower for the existing pre-conversion final-output status peaks.
//! Transport stop does not mute preview or keyboard input. These independently
//! reported samples have unknown age; no new audio telemetry is inferred here.
use super::{decay_factor, decay_level};
use crate::editor::refresh_state::AudioFrame;
use std::time::Instant;

#[derive(Default)]
pub(super) struct State {
    source: Option<u64>,
    peak: [f32; 2],
    updated: Option<Instant>,
}

impl State {
    pub(super) fn level(&self) -> [f32; 2] {
        self.peak
    }

    pub(super) fn observe(&mut self, frame: Option<AudioFrame>, now: Instant) -> bool {
        let before = self.peak;
        let Some(frame) =
            frame.filter(|f| !f.shutdown_complete && !f.device_fault && !f.render_fault)
        else {
            *self = Self::default();
            return before != self.peak;
        };
        if self.source != Some(frame.source) {
            *self = Self {
                source: Some(frame.source),
                ..Self::default()
            };
        }
        let decay = decay_factor(&mut self.updated, now);
        let peaks = [frame.peak_left, frame.peak_right].map(f32::from_bits);
        self.peak = std::array::from_fn(|channel| {
            let peak = peaks[channel];
            let peak = if peak.is_finite() && peak >= 0.0 {
                peak
            } else {
                0.0
            };
            peak.max(decay_level(self.peak[channel], decay))
        });
        before != self.peak
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    use hex_audio::prepared::Status;
    use std::sync::atomic::Ordering::Relaxed;
    use std::time::Duration;

    fn frame(source: u64, peak: [f32; 2]) -> AudioFrame {
        let status = Status::default();
        status.peak_left.store(peak[0].to_bits(), Relaxed);
        status.peak_right.store(peak[1].to_bits(), Relaxed);
        let mut frame = AudioFrame::capture(&status);
        frame.source = source;
        frame
    }

    #[test]
    fn stopped_transport_output_decays_at_the_track_rate_and_new_output_drops_old_peaks() {
        let mut state = State::default();
        let now = Instant::now();
        let preview = frame(1, [2.0, 0.5]);
        assert!(!preview.playing);
        assert!(state.observe(Some(preview), now));
        assert_eq!(state.level(), [2.0, 0.5]);
        state.observe(Some(frame(1, [0.0; 2])), now + Duration::from_secs(1));
        for (actual, expected) in state.level().into_iter().zip([2.0, 0.5]) {
            assert!((actual - expected * 10_f32.powf(-12.0 / 20.0)).abs() < 1e-6);
        }
        state.observe(Some(frame(1, [0.0; 2])), now + Duration::from_secs(10));
        assert_eq!(state.level(), [0.0; 2]);
        state.observe(Some(preview), now + Duration::from_secs(11));
        state.observe(Some(frame(2, [0.1, 0.2])), now + Duration::from_secs(11));
        assert_eq!(state.level(), [0.1, 0.2]);
        state.observe(Some(frame(3, [f32::NAN, f32::INFINITY])), now);
        assert_eq!(state.level(), [0.0; 2]);
    }

    #[test]
    fn missing_faulted_and_closed_outputs_clear_the_follower_immediately() {
        let now = Instant::now();
        for flag in 0..4 {
            let mut state = State::default();
            let mut failed = frame(1, [2.0; 2]);
            state.observe(Some(failed), now);
            failed.device_fault = flag == 0;
            failed.render_fault = flag == 1;
            failed.shutdown_complete = flag == 2;
            assert!(state.observe((flag != 3).then_some(failed), now));
            assert_eq!(state.level(), [0.0; 2]);
        }
    }
}
