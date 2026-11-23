//! Worker-side scheduling for the existing active native PCM transport.
//! Deadline priority is borrowed while audio is in flight and restored for
//! control work, idle polling, and retirement. The device callback is uninvolved.

use cpal::platform::CoreAudioWorkerTimeConstraint;
use hex_plugin_host::Error;
use std::time::{Duration, Instant};

use super::{Processor, State};

pub(super) struct Deadline {
    policy: CoreAudioWorkerTimeConstraint,
    interval: Duration,
    live_until: Option<Instant>,
    active: bool,
    refused: bool,
}

impl Deadline {
    fn policy_window(period: Duration) -> (Duration, Duration, Duration) {
        let constraint = period.mul_f64(0.75).min(Duration::from_millis(4));
        let computation = Duration::from_millis(1).min(constraint / 2);
        // Retain deadline priority across transient multi-quantum gaps (up to
        // the six-quantum concealment bound and QUIET_TRANSPORT window) so a
        // brief hiccup does not demote the transport worker mid-stream.
        let interval = (period * 8).max(super::QUIET_TRANSPORT);
        (computation, constraint, interval)
    }

    pub(super) fn for_native_pcm(state: &State) -> Option<Self> {
        let Some(Processor::Native(instance)) = state.processor.as_ref() else {
            return None;
        };
        let period = instance.deadline_period_on_worker()?;
        let (computation, constraint, interval) = Self::policy_window(period);
        let policy =
            CoreAudioWorkerTimeConstraint::for_current_thread(period, computation, constraint)
                .ok()?;
        Some(Self {
            policy,
            interval,
            live_until: None,
            active: false,
            refused: false,
        })
    }

    pub(super) fn before_poll(&mut self) {
        if self.live_until.is_some_and(|until| Instant::now() < until) {
            self.enter();
        }
    }

    // A stall can outlast the live window while audio is queued. Check for
    // input at the current priority before treating that gap as idle.
    pub(super) fn after_empty_poll(&mut self, now: Instant) -> Result<(), Error> {
        if self.live_until.is_none_or(|until| now >= until) {
            self.live_until = None;
            self.leave()?;
        }
        Ok(())
    }

    pub(super) fn enter(&mut self) {
        if !self.active && !self.refused {
            self.active = self.policy.enter().is_ok();
            self.refused = !self.active;
        }
    }

    pub(super) fn replied(&mut self) {
        if !self.refused {
            self.live_until = Instant::now().checked_add(self.interval);
        }
    }

    pub(super) fn leave(&mut self) -> Result<(), Error> {
        if self.active {
            self.policy.leave().map_err(|error| {
                Error::IoAt(
                    "native transport deadline restoration",
                    std::io::Error::other(error),
                )
            })?;
            self.active = false;
            // A successful Mach restore is sufficient. macOS can refuse a
            // later pthread QoS request on a manually scheduled thread.
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_live_wait_keeps_deadline_priority_until_the_worker_checks_for_audio() {
        let period = Duration::from_micros(5_805);
        let mut deadline = Deadline {
            policy: CoreAudioWorkerTimeConstraint::for_current_thread(
                period,
                Duration::from_micros(300),
                Duration::from_millis(4),
            )
            .unwrap(),
            interval: period * 2,
            live_until: Some(Instant::now()),
            active: false,
            refused: false,
        };
        deadline.enter();
        assert!(
            deadline.active,
            "worker priority must be admitted for this test"
        );

        deadline.before_poll();
        assert!(
            deadline.active,
            "priority must survive until the input poll"
        );

        // A received request renews the live period. A subsequent empty poll
        // within that period keeps the worker ready for the next request.
        deadline.replied();
        let expires = deadline.live_until.unwrap();
        deadline.after_empty_poll(expires - period).unwrap();
        assert!(deadline.active);

        deadline.after_empty_poll(expires).unwrap();
        assert!(
            !deadline.active,
            "an expired empty poll must restore priority"
        );
        assert!(deadline.live_until.is_none());
    }

    #[test]
    fn policy_window_covers_concealment_gaps_and_respects_mach_bounds() {
        for period in [
            Duration::from_millis(4),
            Duration::from_micros(5_333),
            Duration::from_micros(5_805),
            Duration::from_millis(8),
        ] {
            let (computation, constraint, interval) = Deadline::policy_window(period);
            assert!(computation >= Duration::from_micros(500));
            assert!(computation <= constraint / 2);
            assert!(constraint <= period);
            assert!(interval >= super::super::QUIET_TRANSPORT);
            assert!(interval >= period * 8);
            let mut deadline = Deadline {
                policy: CoreAudioWorkerTimeConstraint::for_current_thread(
                    period,
                    computation,
                    constraint,
                )
                .unwrap(),
                interval,
                live_until: None,
                active: false,
                refused: false,
            };
            deadline.enter();
            assert!(deadline.active);
            deadline.replied();
            let expires = deadline.live_until.unwrap();
            // A three-quantum transient gap (~17.4 ms at 44.1 kHz / 256) must
            // keep deadline priority instead of demoting mid-stream.
            deadline.after_empty_poll(expires - period * 5).unwrap();
            assert!(deadline.active);
            deadline.after_empty_poll(expires).unwrap();
            assert!(!deadline.active);
        }
    }
}
