//! Worker-side scheduling for the existing active native PCM transport.
//! Deadline priority is borrowed while audio is in flight and restored for
//! control work, idle polling, and retirement. The device callback is uninvolved.

use std::{
    sync::atomic::{
        AtomicUsize,
        Ordering::{AcqRel, Acquire},
    },
    time::{Duration, Instant},
};

use cpal::platform::CoreAudioWorkerTimeConstraint;
use hex_plugin_host::Error;

use super::{Processor, State};

// This limits the experimental time-constraint reservations. Additional
// transports keep their ordinary scheduling and may still exchange PCM.
const MAX_ACTIVE_TRANSPORTS: usize = 12;
static ACTIVE_TRANSPORTS: AtomicUsize = AtomicUsize::new(0);

pub(super) struct Deadline {
    policy: CoreAudioWorkerTimeConstraint,
    interval: Duration,
    live_until: Option<Instant>,
    admitted: bool,
    refused: bool,
}

impl Deadline {
    pub(super) fn for_native_pcm(state: &State) -> Option<Self> {
        std::env::var_os("HEX_VST3_TEST_WORKER_DEADLINE")?;
        let Some(Processor::Native(instance)) = state.processor.as_ref() else {
            return None;
        };
        let config = instance.configuration();
        let period =
            Duration::try_from_secs_f64(f64::from(config.quantum) / config.sample_rate).ok()?;
        if !(Duration::from_millis(4)..=Duration::from_millis(8)).contains(&period) {
            return None;
        }
        let policy = CoreAudioWorkerTimeConstraint::for_current_thread(
            period,
            Duration::from_micros(300),
            Duration::from_millis(4),
        )
        .ok()?;
        Some(Self {
            policy,
            interval: period * 2,
            live_until: None,
            admitted: false,
            refused: false,
        })
    }

    pub(super) fn before_poll(&mut self) -> Result<(), Error> {
        if self.live_until.is_some_and(|until| Instant::now() < until) {
            self.enter();
        } else {
            self.live_until = None;
            self.leave()?;
        }
        Ok(())
    }

    pub(super) fn enter(&mut self) {
        if self.admitted || self.refused {
            return;
        }
        let admitted = ACTIVE_TRANSPORTS.fetch_update(AcqRel, Acquire, |active| {
            (active < MAX_ACTIVE_TRANSPORTS).then_some(active + 1)
        });
        if admitted.is_err() {
            return;
        }
        self.admitted = self.policy.enter().is_ok();
        if !self.admitted {
            self.refused = true;
            ACTIVE_TRANSPORTS.fetch_sub(1, AcqRel);
        }
    }

    pub(super) fn replied(&mut self) {
        if !self.refused {
            self.live_until = Instant::now().checked_add(self.interval);
        }
    }

    pub(super) fn leave(&mut self) -> Result<(), Error> {
        if self.admitted {
            self.policy.leave().map_err(|error| {
                Error::IoAt(
                    "native transport deadline restoration",
                    std::io::Error::other(error),
                )
            })?;
            self.admitted = false;
            ACTIVE_TRANSPORTS.fetch_sub(1, AcqRel);
            // A successful Mach restore is sufficient. macOS can refuse a
            // later pthread QoS request on a manually scheduled thread.
        }
        Ok(())
    }
}

impl Drop for Deadline {
    fn drop(&mut self) {
        if self.leave().is_err() {
            // The time constraint's own drop retries restoration. The
            // temporary right and scheduling policy die with this worker.
            ACTIVE_TRANSPORTS.fetch_sub(1, AcqRel);
        }
    }
}
