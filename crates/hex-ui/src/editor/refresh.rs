use std::time::{Duration, Instant};

const DIAGNOSTIC_REFRESH: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct AudioFrame {
    pub playing: bool,
    pub position: u64,
    pub peak_left: u32,
    pub peak_right: u32,
    pub applied_revision: u64,
    pub deadline_misses: u64,
    pub callback_us: u64,
    pub render_fault: bool,
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
            return visible_change;
        };
        let previous = self.rendered;
        let mut changed = visible_change;
        if let Some(previous) = previous {
            changed |= current.playing != previous.playing
                || current.applied_revision != previous.applied_revision
                || current.deadline_misses != previous.deadline_misses
                || current.render_fault != previous.render_fault;
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
            playing,
            position: 0,
            peak_left: 0,
            peak_right: 0,
            applied_revision: 1,
            deadline_misses: 0,
            callback_us: 10,
            render_fault: false,
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
