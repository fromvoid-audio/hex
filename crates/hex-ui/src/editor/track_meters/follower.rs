//! Display follower for the pre-conversion final-output status peaks and
//! callback-latched between-poll maxima/clip flags. Transport stop does not
//! mute preview or keyboard input.
use super::{StereoMeterState, decay_factor};
use crate::editor::refresh_state::AudioFrame;
use crate::output_meter::MeterReading;
use std::time::Instant;

#[derive(Default)]
pub(super) struct State {
    source: Option<u64>,
    meter: StereoMeterState,
    updated: Option<Instant>,
}

impl State {
    #[cfg(all(test, feature = "ui-unit-tier"))]
    pub(super) fn level(&self) -> [f32; 2] {
        self.meter.level()
    }

    pub(super) fn reading(&self) -> MeterReading {
        self.meter.reading()
    }

    pub(super) fn observe(&mut self, frame: Option<AudioFrame>, now: Instant) -> bool {
        let before = self.meter.reading();
        let Some(frame) =
            frame.filter(|f| !f.shutdown_complete && !f.device_fault && !f.render_fault)
        else {
            *self = Self::default();
            return before != self.meter.reading();
        };
        if self.source != Some(frame.source) {
            *self = Self {
                source: Some(frame.source),
                ..Self::default()
            };
        }
        let decay = decay_factor(&mut self.updated, now);
        let raw = [frame.peak_left, frame.peak_right].map(f32::from_bits);
        let latched = [frame.latched_peak_left, frame.latched_peak_right].map(f32::from_bits);
        let peaks = std::array::from_fn(|channel| {
            let r = if raw[channel].is_finite() && raw[channel] >= 0.0 {
                raw[channel]
            } else {
                0.0
            };
            let l = if latched[channel].is_finite() && latched[channel] >= 0.0 {
                latched[channel]
            } else {
                0.0
            };
            r.max(l)
        });
        let changed = self.meter.observe(peaks, frame.latched_clip, decay, now);
        changed || before != self.meter.reading()
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
        let mut frame = crate::editor::refresh_state::capture_audio_frame(&status);
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
        assert_eq!(state.reading().peak_hold, [2.0, 0.5]);
        assert_eq!(state.reading().clipped, [true, false]);
        state.observe(Some(frame(1, [0.0; 2])), now + Duration::from_secs(1));
        for (actual, expected) in state.level().into_iter().zip([2.0, 0.5]) {
            assert!((actual - expected * 10_f32.powf(-12.0 / 20.0)).abs() < 1e-6);
        }
        // Within the 1.5s hold window, peak_hold stays pinned at the transient maximum.
        assert_eq!(state.reading().peak_hold, [2.0, 0.5]);
        state.observe(Some(frame(1, [0.0; 2])), now + Duration::from_secs(10));
        assert_eq!(state.level(), [0.0; 2]);
        assert_eq!(state.reading().peak_hold, [0.0; 2]);
        state.observe(Some(preview), now + Duration::from_secs(11));
        state.observe(Some(frame(2, [0.1, 0.2])), now + Duration::from_secs(11));
        assert_eq!(state.level(), [0.1, 0.2]);
        state.observe(Some(frame(3, [f32::NAN, f32::INFINITY])), now);
        assert_eq!(state.level(), [0.0; 2]);
    }

    #[test]
    fn follower_preserves_callback_latched_between_poll_peak_and_clip() {
        let status = Status::default();
        status.playing.store(true, Relaxed);
        // Transient clip callback followed by quiet callback before UI poll.
        status.record_output_peaks([1.6, 0.2]);
        status.record_output_peaks([0.05, 0.1]);
        let mut captured = crate::editor::refresh_state::capture_audio_frame(&status);
        captured.source = 1;
        let mut state = State::default();
        let now = Instant::now();
        assert!(state.observe(Some(captured), now));
        assert_eq!(
            state.reading(),
            MeterReading {
                level: [1.6, 0.2],
                peak_hold: [1.6, 0.2],
                clipped: [true, false],
            }
        );
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
