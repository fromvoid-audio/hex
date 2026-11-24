//! Process-wide admission for the two scheduling lanes of a live instance.
//! Its permit follows the waitable native child through Pending, Prepared,
//! quiescence, restart, and failed disposal. No callback holds a permit.

use std::{
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
    time::Duration,
};

use crate::protocol::Configuration;

// The paired scheduling profile admits up to thirty-two concurrent native
// instances so multi-VST3 sessions (such as 16+ plugin projects) do not starve
// tail instances on ordinary scheduling. Further sources still load and process
// using high-QoS ordinary scheduling.
pub const MAX_DEADLINE_PAIRS: usize = 32;

// Supported low-latency hardware periods admitted for Mach time-constraint
// scheduling (500 us..=12 ms: covers 44.1/48/88.2/96 kHz at 64/128/256 frames
// and 44.1/48 kHz at 512 frames, while keeping Mach's minimum computation
// budget well within bounds).
const MIN_DEADLINE_PERIOD: Duration = Duration::from_micros(500);
const MAX_DEADLINE_PERIOD: Duration = Duration::from_millis(12);

// Only the scalar count is shared between child owners.
static RESERVED_PAIRS: AtomicUsize = AtomicUsize::new(0);

pub(super) struct Permit {
    pub period: Duration,
}

impl Permit {
    pub(super) fn reserve(config: &Configuration) -> Option<Self> {
        let period =
            Duration::try_from_secs_f64(f64::from(config.quantum) / config.sample_rate).ok()?;
        if !(MIN_DEADLINE_PERIOD..=MAX_DEADLINE_PERIOD).contains(&period) {
            return None;
        }
        RESERVED_PAIRS
            .fetch_update(Relaxed, Relaxed, |reserved| {
                (reserved < MAX_DEADLINE_PAIRS).then_some(reserved + 1)
            })
            .ok()?;
        Some(Self { period })
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        RESERVED_PAIRS.fetch_sub(1, Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::ForeignProcessorBinding as Binding;
    use std::sync::Mutex;

    static PERMIT_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn config(quantum: u16, sample_rate: f64) -> Configuration {
        Configuration {
            binding: Binding {
                root: 1,
                instance: 1,
                graph: 1,
                epoch: 1,
            },
            identity: hex_plugin_state::Identity {
                class: hex_plugin_state::ClassId::new([1; 16]).unwrap(),
                binary: hex_plugin_state::BinaryIdentity {
                    executable_sha256: [2; 32],
                    info_sha256: [3; 32],
                },
            },
            sample_rate,
            quantum,
            input_channels: 2,
            output_channels: 2,
            bundle: "/tmp/test.vst3".into(),
            executable: "Contents/MacOS/test".into(),
            restore: None,
        }
    }

    #[test]
    fn permits_admit_up_to_cap_and_release_on_drop() {
        let _serial = PERMIT_TEST_LOCK.lock().unwrap();
        let cfg = config(256, 44_100.0);
        let baseline = RESERVED_PAIRS.load(Relaxed);
        let available = MAX_DEADLINE_PAIRS.saturating_sub(baseline);
        assert!(
            available >= 16,
            "cap must admit at least 16 concurrent VST3 instances"
        );
        let mut permits = Vec::with_capacity(available);
        for _ in 0..available {
            permits.push(Permit::reserve(&cfg).expect("permit within cap"));
        }
        assert!(Permit::reserve(&cfg).is_none());
        permits.pop();
        let replacement = Permit::reserve(&cfg).expect("released permit re-admits");
        drop(replacement);
        drop(permits);
        assert_eq!(RESERVED_PAIRS.load(Relaxed), baseline);
    }

    #[test]
    fn permits_admit_all_supported_low_latency_rates_and_quanta() {
        let _serial = PERMIT_TEST_LOCK.lock().unwrap();
        for sample_rate in [44_100.0, 48_000.0, 88_200.0, 96_000.0] {
            for quantum in [64_u16, 128, 256] {
                let permit = Permit::reserve(&config(quantum, sample_rate));
                assert!(
                    permit.is_some(),
                    "expected RT deadline permit for {sample_rate} Hz @ {quantum} frames"
                );
            }
        }
        assert!(Permit::reserve(&config(512, 48_000.0)).is_some());
        assert!(Permit::reserve(&config(512, 44_100.0)).is_some());
    }

    #[test]
    fn permits_reject_periods_outside_qualified_window() {
        let _serial = PERMIT_TEST_LOCK.lock().unwrap();
        assert!(Permit::reserve(&config(16, 96_000.0)).is_none());
        assert!(Permit::reserve(&config(256, 16_000.0)).is_none());
        assert!(Permit::reserve(&config(1024, 44_100.0)).is_none());
    }
}
