//! Worker-side scheduling for the existing active native PCM transport.
//! Deadline priority is borrowed while audio is in flight and restored
//! non-fatally when idle, on heavy non-routine control turns, and on retirement.
//! Routine periodic control polls keep the active transport policy intact.
//! The device callback is uninvolved.

use cpal::platform::CoreAudioWorkerTimeConstraint;
use std::io::Write as _;
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
        let computation = period
            .mul_f64(0.35)
            .min(Duration::from_millis(1))
            .min(constraint / 2);
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
    pub(super) fn after_empty_poll(&mut self, now: Instant) {
        if self.live_until.is_none_or(|until| now >= until) {
            self.live_until = None;
            self.leave();
        }
    }

    pub(super) fn remaining_live_window(&self, now: Instant) -> Option<Duration> {
        self.live_until
            .and_then(|until| until.checked_duration_since(now))
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

    /// Restore standard thread policy non-fatally. A kernel refusal logs a
    /// diagnostic warning and disables further policy churn on this thread
    /// without revoking or killing the running VST3 plugin.
    pub(super) fn leave(&mut self) {
        if self.active {
            let result = self.policy.leave().map_err(|error| error.to_string());
            self.finish_leave(result);
        }
    }

    fn finish_leave(&mut self, result: Result<(), String>) {
        self.active = false;
        if let Err(error) = result {
            self.refused = true;
            self.live_until = None;
            let _ = writeln!(
                std::io::stderr().lock(),
                "vst3 worker warning: native transport deadline leave failed: {error}"
            );
        }
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
        deadline.after_empty_poll(expires - period);
        assert!(deadline.active);

        deadline.after_empty_poll(expires);
        assert!(
            !deadline.active,
            "an expired empty poll must restore priority"
        );
        assert!(deadline.live_until.is_none());
    }

    #[test]
    fn policy_window_covers_all_low_latency_rates_and_quanta_and_respects_mach_bounds() {
        for sample_rate in [44_100.0_f64, 48_000.0, 88_200.0, 96_000.0] {
            for quantum in [64_u16, 128, 256] {
                let period = Duration::from_secs_f64(f64::from(quantum) / sample_rate);
                let (computation, constraint, interval) = Deadline::policy_window(period);
                assert!(
                    computation >= Duration::from_micros(50),
                    "{sample_rate} Hz @ {quantum}: computation {computation:?} below Mach minimum"
                );
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
                    .unwrap_or_else(|e| panic!("{sample_rate} Hz @ {quantum}: {e}")),
                    interval,
                    live_until: None,
                    active: false,
                    refused: false,
                };
                deadline.enter();
                assert!(
                    deadline.active,
                    "{sample_rate} Hz @ {quantum}: Mach RT policy enter refused"
                );
                deadline.replied();
                let expires = deadline.live_until.unwrap();
                // Transient multi-quantum gap keeps deadline priority instead of demoting mid-stream.
                deadline.after_empty_poll(expires - period * 5);
                assert!(deadline.active);
                deadline.after_empty_poll(expires);
                assert!(!deadline.active);
                assert!(!deadline.refused);
            }
        }
    }

    #[test]
    fn failed_policy_restoration_is_non_fatal_and_disables_further_churn() {
        let period = Duration::from_micros(2_667);
        let (computation, constraint, interval) = Deadline::policy_window(period);
        let mut deadline = Deadline {
            policy: CoreAudioWorkerTimeConstraint::for_current_thread(
                period,
                computation,
                constraint,
            )
            .unwrap(),
            interval,
            live_until: Some(Instant::now() + interval),
            active: true,
            refused: false,
        };
        deadline.finish_leave(Err("simulated non-KERN_SUCCESS policy restore".into()));
        assert!(!deadline.active);
        assert!(deadline.refused);
        assert!(deadline.live_until.is_none());
        // Subsequent enter/replied/after_empty_poll remain no-ops without panicking or failing.
        deadline.enter();
        assert!(!deadline.active);
        deadline.replied();
        assert!(deadline.live_until.is_none());
        deadline.after_empty_poll(Instant::now());
        assert!(!deadline.active);
    }
}
