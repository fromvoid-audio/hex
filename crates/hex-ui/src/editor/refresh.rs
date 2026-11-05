use crate::output_meter::MeterInput;
use hex_audio::prepared::Status;
use std::sync::atomic::Ordering::{Acquire, Relaxed};
use std::time::{Duration, Instant};

const DIAGNOSTIC_REFRESH: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct AudioFrame {
    pub source: u64,
    pub cpu: hex_audio::cpu_meter::CpuSample,
    pub playing: bool,
    pub position: u64,
    pub peak_left: u32,
    pub peak_right: u32,
    pub applied_revision: u64,
    pub deadline_misses: u64,
    /// Callbacks that held streamed playback silent mid-play (disk overload).
    pub stream_starved: u64,
    pub callback_us: u64,
    pub render_fault: bool,
    pub device_fault: bool,
    pub shutdown_complete: bool,
}

impl AudioFrame {
    /// One control-side observation of the actual callback status. Channel peaks
    /// remain independent relaxed loads, with no age or coherent-pair guarantee.
    pub(super) fn capture(status: &Status) -> Self {
        Self {
            source: 0,
            cpu: status.cpu_meter.snapshot(),
            playing: status.playing.load(Relaxed),
            position: status.position.load(Relaxed),
            peak_left: status.peak_left.load(Relaxed),
            peak_right: status.peak_right.load(Relaxed),
            applied_revision: status.applied_revision.load(Relaxed),
            deadline_misses: status.deadline_misses.load(Relaxed),
            stream_starved: status.stream_starved_callbacks.load(Relaxed),
            callback_us: status.callback_max_ns.load(Relaxed) / 1_000,
            render_fault: status.render_fault.load(Relaxed),
            device_fault: status.device_fault.load(Relaxed),
            shutdown_complete: status.shutdown_complete.load(Acquire),
        }
    }

    pub(super) fn meter_input(self) -> MeterInput {
        if self.shutdown_complete {
            MeterInput::Offline
        } else if self.device_fault || self.render_fault {
            MeterInput::Fault
        } else if !self.playing {
            MeterInput::Stopped
        } else {
            MeterInput::Last {
                left: f32::from_bits(self.peak_left),
                right: f32::from_bits(self.peak_right),
            }
        }
    }
}

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
        let availability_changed = self.output_meter_frame.is_some() != frame.is_some();
        self.output_meter_frame = frame;
        let overload_changed = self
            .disk_overload
            .observe(frame.map(|frame| (frame.source, frame.stream_starved)), now);
        self.redraw_gate.should_redraw(
            visible_change || availability_changed || overload_changed || cpu_changed,
            frame,
            now,
        )
    }
}

#[derive(Default)]
pub(super) struct RedrawGate {
    rendered: Option<AudioFrame>,
    last_redraw: Option<Instant>,
}

impl RedrawGate {
    pub fn should_redraw(
        &mut self,
        visible_change: bool,
        audio: Option<AudioFrame>,
        now: Instant,
    ) -> bool {
        let Some(current) = audio else {
            return self.rendered.take().is_some() || visible_change;
        };
        let previous = self.rendered;
        let mut changed = visible_change;
        if let Some(previous) = previous {
            changed |= current.playing != previous.playing
                || current.applied_revision != previous.applied_revision
                || current.deadline_misses != previous.deadline_misses
                || current.render_fault != previous.render_fault
                || current.device_fault != previous.device_fault
                || current.shutdown_complete != previous.shutdown_complete;
            changed |= current.position != previous.position;
            if current.playing {
                changed |= current.peak_left != previous.peak_left
                    || current.peak_right != previous.peak_right;
            }
            let diagnostic_due = self
                .last_redraw
                .is_none_or(|last| now.saturating_duration_since(last) >= DIAGNOSTIC_REFRESH);
            changed |= diagnostic_due && current.callback_us != previous.callback_us;
        }
        if changed {
            self.rendered = Some(current);
            self.last_redraw = Some(now);
        } else if self.rendered.is_none() {
            // The initial window paint already shows this state.
            self.rendered = Some(current);
            self.last_redraw = Some(now);
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioFrame, DIAGNOSTIC_REFRESH, RedrawGate};
    use std::time::{Duration, Instant};

    fn frame(playing: bool) -> AudioFrame {
        AudioFrame {
            source: 1,
            cpu: Default::default(),
            playing,
            position: 0,
            peak_left: 0,
            peak_right: 0,
            applied_revision: 1,
            deadline_misses: 0,
            stream_starved: 0,
            callback_us: 10,
            render_fault: false,
            device_fault: false,
            shutdown_complete: false,
        }
    }

    #[test]
    fn unchanged_stopped_state_does_not_redraw_at_timer_rate() {
        let start = Instant::now();
        let mut gate = RedrawGate::default();
        assert!(!gate.should_redraw(false, Some(frame(false)), start));
        for tick in 1..=62 {
            assert!(!gate.should_redraw(
                false,
                Some(frame(false)),
                start + Duration::from_millis(16 * tick)
            ));
        }
    }

    #[test]
    fn playback_position_and_meter_changes_refresh() {
        let start = Instant::now();
        let mut gate = RedrawGate::default();
        gate.should_redraw(false, Some(frame(true)), start);
        let mut moving = frame(true);
        moving.position = 128;
        moving.peak_left = 0.25_f32.to_bits();
        assert!(gate.should_redraw(false, Some(moving), start + Duration::from_millis(16)));
    }

    #[test]
    fn paused_seek_and_stopped_reset_refresh_immediately() {
        let start = Instant::now();
        let mut gate = RedrawGate::default();
        gate.should_redraw(false, Some(frame(false)), start);

        let mut seeked = frame(false);
        seeked.position = 24_000;
        assert!(gate.should_redraw(false, Some(seeked), start + Duration::from_millis(16)));

        let mut stopped = frame(false);
        assert!(gate.should_redraw(false, Some(stopped), start + Duration::from_millis(32)));
        stopped.callback_us += 1;
        assert!(!gate.should_redraw(false, Some(stopped), start + Duration::from_millis(48)));
    }

    #[test]
    fn events_errors_and_critical_diagnostics_refresh_immediately() {
        let start = Instant::now();
        let mut gate = RedrawGate::default();
        gate.should_redraw(false, Some(frame(false)), start);
        assert!(gate.should_redraw(true, Some(frame(false)), start + Duration::from_millis(16)));

        let mut fault = frame(false);
        fault.render_fault = true;
        assert!(gate.should_redraw(false, Some(fault), start + Duration::from_millis(32)));

        let mut misses = fault;
        misses.deadline_misses = 1;
        assert!(gate.should_redraw(false, Some(misses), start + Duration::from_millis(48)));
    }

    #[test]
    fn callback_diagnostics_are_bounded_while_stopped() {
        let start = Instant::now();
        let mut gate = RedrawGate::default();
        gate.should_redraw(false, Some(frame(false)), start);
        for tick in 1..62 {
            let mut changing = frame(false);
            changing.callback_us += tick;
            assert!(!gate.should_redraw(
                false,
                Some(changing),
                start + Duration::from_millis(16 * tick)
            ));
        }
        let mut latest = frame(false);
        latest.callback_us += 62;
        assert!(gate.should_redraw(false, Some(latest), start + DIAGNOSTIC_REFRESH));
        latest.callback_us += 1;
        assert!(!gate.should_redraw(
            false,
            Some(latest),
            start + DIAGNOSTIC_REFRESH + Duration::from_millis(16)
        ));
    }
}

#[cfg(test)]
mod meter_snapshot_tests {
    use super::*;

    #[test]
    fn output_meter_capture_preserves_actual_status_bits_and_unknown_age() {
        let status = Status::default();
        status.playing.store(true, Relaxed);
        for (left, right, text) in [
            (0.0, 0.000_001, "last l -inf r <-60 dbfs"),
            (0.001, 0.1, "last l -60.0 r -20.0 dbfs"),
            (1.0, 2.0, "last l 0.0 r +6.0! dbfs"),
            (-1.0, f32::INFINITY, "last l invalid r invalid dbfs"),
            (f32::NAN, 0.5, "last l invalid r -6.0 dbfs"),
        ] {
            status.peak_left.store(left.to_bits(), Relaxed);
            status.peak_right.store(right.to_bits(), Relaxed);
            let snapshot = AudioFrame::capture(&status);
            assert_eq!(snapshot.peak_left, left.to_bits());
            assert_eq!(snapshot.peak_right, right.to_bits());
            assert_eq!(crate::output_meter::readout(snapshot.meter_input()), text);
            assert!(
                crate::output_meter::description(snapshot.meter_input()).contains("age unknown")
            );
            // A producer update cannot silently alter an already cached frame.
            status.peak_right.store(0, Relaxed);
            assert_eq!(crate::output_meter::readout(snapshot.meter_input()), text);
        }
    }

    #[test]
    fn output_meter_stop_fault_shutdown_and_redraw_do_not_invent_freshness() {
        let status = Status::default();
        let start = Instant::now();
        let mut gate = RedrawGate::default();
        let stopped = AudioFrame::capture(&status);
        assert_eq!(stopped.meter_input(), MeterInput::Stopped);
        gate.should_redraw(false, Some(stopped), start);
        status.playing.store(true, Relaxed);
        status.peak_left.store(2.0_f32.to_bits(), Relaxed);
        let overload = AudioFrame::capture(&status);
        assert!(gate.should_redraw(false, Some(overload), start));
        assert!(!gate.should_redraw(false, Some(overload), start + DIAGNOSTIC_REFRESH * 10));
        assert_eq!(
            crate::output_meter::readout(overload.meter_input()),
            "last l +6.0! r -inf dbfs"
        );
        status.peak_left.store(0, Relaxed);
        let quiet = AudioFrame::capture(&status);
        assert!(gate.should_redraw(false, Some(quiet), start));
        assert_eq!(
            crate::output_meter::readout(quiet.meter_input()),
            "last l -inf r -inf dbfs"
        );
        for fault in [&status.device_fault, &status.render_fault] {
            fault.store(true, Relaxed);
            let failed = AudioFrame::capture(&status);
            assert_eq!(failed.meter_input(), MeterInput::Fault);
            assert!(gate.should_redraw(false, Some(failed), start));
            fault.store(false, Relaxed);
            assert!(gate.should_redraw(false, Some(AudioFrame::capture(&status)), start));
        }
        status
            .shutdown_complete
            .store(true, std::sync::atomic::Ordering::Release);
        let offline = AudioFrame::capture(&status);
        assert_eq!(offline.meter_input(), MeterInput::Offline);
        assert!(gate.should_redraw(false, Some(offline), start));
        assert!(gate.should_redraw(false, None, start));
        assert!(!gate.should_redraw(false, None, start));
    }
}
