//! Control-side audio snapshots and redraw decisions without a window or service.
use crate::cpu_meter::CpuSample;
use crate::output_meter_logic::MeterInput;
use std::time::{Duration, Instant};

pub const DIAGNOSTIC_REFRESH: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioFrame {
    pub source: u64,
    pub cpu: CpuSample,
    pub playing: bool,
    pub position: u64,
    pub peak_left: u32,
    pub peak_right: u32,
    /// Callback-latched maximum peak bits drained since the previous poll.
    pub latched_peak_left: u32,
    pub latched_peak_right: u32,
    /// Callback-latched between-poll clip flags (`peak > 1.0`) drained since the previous poll.
    pub latched_clip: [bool; 2],
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
    pub fn meter_input(self) -> MeterInput {
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RedrawDecision {
    /// Full root `EditorView::render` is required (structural/state transition).
    pub root: bool,
    /// Playback position changed (updates isolated transport readout & playheads).
    pub position: bool,
    /// Main output peak/clip changed (updates isolated output/main meters).
    pub output_meter: bool,
}

impl RedrawDecision {
    pub fn any(self) -> bool {
        self.root || self.position || self.output_meter
    }
}

#[derive(Default)]
pub struct RedrawGate {
    rendered: Option<AudioFrame>,
    last_redraw: Option<Instant>,
}

impl RedrawGate {
    pub fn evaluate(
        &mut self,
        visible_change: bool,
        audio: Option<AudioFrame>,
        now: Instant,
    ) -> RedrawDecision {
        let Some(current) = audio else {
            let root = self.rendered.take().is_some() || visible_change;
            return RedrawDecision {
                root,
                position: root,
                output_meter: root,
            };
        };
        let previous = self.rendered;
        let mut decision = RedrawDecision {
            root: visible_change,
            position: false,
            output_meter: false,
        };
        if let Some(previous) = previous {
            decision.root |= current.playing != previous.playing
                || current.applied_revision != previous.applied_revision
                || current.deadline_misses != previous.deadline_misses
                || current.render_fault != previous.render_fault
                || current.device_fault != previous.device_fault
                || current.shutdown_complete != previous.shutdown_complete;
            decision.position = current.position != previous.position;
            if current.playing {
                decision.output_meter = current.peak_left != previous.peak_left
                    || current.peak_right != previous.peak_right
                    || current.latched_peak_left != previous.latched_peak_left
                    || current.latched_peak_right != previous.latched_peak_right
                    || current.latched_clip != previous.latched_clip;
            }
            let diagnostic_due = self
                .last_redraw
                .is_none_or(|last| now.saturating_duration_since(last) >= DIAGNOSTIC_REFRESH);
            decision.root |= diagnostic_due && current.callback_us != previous.callback_us;
        }
        if decision.any() {
            self.rendered = Some(current);
            if decision.root {
                self.last_redraw = Some(now);
            }
        } else if self.rendered.is_none() {
            // The initial window paint already shows this state.
            self.rendered = Some(current);
            self.last_redraw = Some(now);
        }
        decision
    }

    pub fn should_redraw(
        &mut self,
        visible_change: bool,
        audio: Option<AudioFrame>,
        now: Instant,
    ) -> bool {
        self.evaluate(visible_change, audio, now).any()
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
            latched_peak_left: 0,
            latched_peak_right: 0,
            latched_clip: [false; 2],
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
