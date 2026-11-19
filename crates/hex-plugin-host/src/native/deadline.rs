//! Process-wide admission for the two scheduling lanes of a live instance.
//! Its permit follows the waitable native child through Pending, Prepared,
//! quiescence, restart, and failed disposal. No callback holds a permit.

use std::{
    sync::atomic::{AtomicUsize, Ordering::AcqRel},
    time::Duration,
};

use crate::protocol::Configuration;

// The paired scheduling profile is qualified with twelve independent native
// effects. Further sources still load and process using ordinary scheduling.
const MAX_DEADLINE_PAIRS: usize = 12;

static RESERVED_PAIRS: AtomicUsize = AtomicUsize::new(0);

pub(super) struct Permit {
    pub period: Duration,
}

impl Permit {
    pub(super) fn reserve(config: &Configuration) -> Option<Self> {
        let period =
            Duration::try_from_secs_f64(f64::from(config.quantum) / config.sample_rate).ok()?;
        if !(Duration::from_millis(4)..=Duration::from_millis(8)).contains(&period) {
            return None;
        }
        RESERVED_PAIRS
            .fetch_update(AcqRel, AcqRel, |reserved| {
                (reserved < MAX_DEADLINE_PAIRS).then_some(reserved + 1)
            })
            .ok()?;
        Some(Self { period })
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        RESERVED_PAIRS.fetch_sub(1, AcqRel);
    }
}
