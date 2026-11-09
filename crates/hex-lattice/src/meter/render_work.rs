//! Opt-in scalar timing publication, independent of sample peaks. One render
//! owner writes; one control owner changes enablement. Each snapshot is one
//! atomic word: epoch, sequence and hundredths of the audio-buffer budget.
//! Epoch matching rejects a sample that races disable/re-enable. Readers never
//! spin and no endpoint owns heap, clock, graph or processor resources.
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};

#[derive(Debug, Default)]
pub struct RenderWorkMeter {
    request: AtomicU32,
    sample: AtomicU64,
}

/// A sampled wall-time fraction of the rendered audio duration. This is not
/// process CPU, worker CPU, physical latency, or proof of an audio dropout.
/// This is the last sampled render, not a continuous maximum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderWorkSample {
    pub epoch: u16,
    pub sequence: u16,
    pub hundredths_percent: u32,
}

impl RenderWorkMeter {
    /// Control only; repeated requests preserve freshness. A changed request
    /// advances the epoch so old or racing samples cannot become fresh zeroes.
    pub fn set_enabled(&self, enabled: bool) {
        let previous = self.request.load(Relaxed);
        if (previous & 1 != 0) == enabled {
            return;
        }
        let epoch = ((previous >> 1) as u16).wrapping_add(1).max(1);
        self.request
            .store((u32::from(epoch) << 1) | u32::from(enabled), Relaxed);
    }

    /// Capture once before timing; a disabled endpoint requires no clock read.
    pub fn requested_epoch(&self) -> Option<u16> {
        let request = self.request.load(Relaxed);
        (request & 1 != 0).then_some((request >> 1) as u16)
    }

    /// Single render writer. The caller supplies disjoint measured scopes for
    /// this owner and the complete rendered duration, excluding build/pre-roll.
    /// Invalid duration is unavailable, never a fabricated zero-cost sample.
    pub fn publish(&self, epoch: u16, elapsed_ns: u64, frames: usize, rate: u32) {
        if frames == 0 || rate == 0 || self.requested_epoch() != Some(epoch) {
            return;
        }
        let load = (u128::from(elapsed_ns) * u128::from(rate) * 10_000
            / (frames as u128 * 1_000_000_000))
            .min(u128::from(u32::MAX)) as u32;
        let sequence = ((self.sample.load(Relaxed) >> 32) as u16).wrapping_add(1);
        self.sample.store(
            (u64::from(epoch) << 48) | (u64::from(sequence) << 32) | u64::from(load),
            Relaxed,
        );
    }

    /// Non-destructive, bounded snapshot; no coherent graph/publication claim.
    /// The caller must correlate the containing observation with applied audio.
    pub fn sample(&self) -> Option<RenderWorkSample> {
        let epoch = self.requested_epoch()?;
        let sample = self.sample.load(Relaxed);
        if (sample >> 48) as u16 != epoch || self.requested_epoch() != Some(epoch) {
            return None;
        }
        Some(RenderWorkSample {
            epoch,
            sequence: (sample >> 32) as u16,
            hundredths_percent: sample as u32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enablement_duration_and_epoch_reject_unknown_and_late_work() {
        let meter = RenderWorkMeter::default();
        assert_eq!(meter.requested_epoch(), None);
        assert_eq!(meter.sample(), None);
        meter.set_enabled(true);
        let epoch = meter.requested_epoch().unwrap();
        assert_eq!(meter.sample(), None);
        meter.publish(epoch, 500_000, 0, 48_000);
        meter.publish(epoch, 500_000, 48, 0);
        assert_eq!(meter.sample(), None);
        meter.publish(epoch, 500_000, 48, 48_000);
        let sample = meter.sample().unwrap();
        assert_eq!(sample.hundredths_percent, 5000);
        meter.set_enabled(true);
        assert_eq!(meter.sample(), Some(sample));
        meter.publish(epoch, 2_000_000, 48, 48_000);
        assert_eq!(meter.sample().unwrap().hundredths_percent, 20_000);
        assert_ne!(meter.sample().unwrap().sequence, sample.sequence);
        meter.set_enabled(false);
        assert_eq!(meter.sample(), None);
        meter.set_enabled(true);
        meter.publish(epoch, 0, 48, 48_000);
        assert_eq!(meter.sample(), None);
        meter.publish(meter.requested_epoch().unwrap(), 0, 48, 48_000);
        assert_eq!(meter.sample().unwrap().hundredths_percent, 0);
    }
}
