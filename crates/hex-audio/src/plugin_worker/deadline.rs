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
    pub(super) fn for_native_pcm(state: &State) -> Option<Self> {
        let Some(Processor::Native(instance)) = state.processor.as_ref() else {
            return None;
        };
        let period = instance.deadline_period_on_worker()?;
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
            active: false,
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
