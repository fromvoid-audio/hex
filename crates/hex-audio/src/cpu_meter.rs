//! Bounded callback load telemetry. Percentages measure callback work divided by
//! the current buffer period; they do not measure scheduler or device overruns.
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};

/// Cumulative callback work and budget, plus peak load in hundredths of a percent.
#[derive(Default)]
pub struct CpuMeter {
    elapsed: AtomicU64,
    budget: AtomicU64,
    peak: AtomicU32,
    frames: AtomicU32,
    rate: AtomicU32,
}

impl CpuMeter {
    /// Called by the output callback after timing its work. No allocation or locks.
    pub(crate) fn record(&self, elapsed_ns: u64, frames: usize, rate: u32) {
        let Some(deadline) = u64::try_from(frames)
            .ok()
            .and_then(|n| n.checked_mul(1_000_000_000))
            .and_then(|n| n.checked_div(u64::from(rate)))
            .filter(|n| *n != 0)
        else {
            return;
        };
        let load = elapsed_ns.saturating_mul(10_000) / deadline;
        let load = load.min(u64::from(u32::MAX)) as u32;
        self.elapsed.fetch_add(elapsed_ns, Relaxed);
        self.budget.fetch_add(deadline, Relaxed);
        self.peak.fetch_max(load, Relaxed);
        self.frames
            .store(frames.min(u32::MAX as usize) as u32, Relaxed);
        self.rate.store(rate, Relaxed);
    }

    /// Best-effort independent observations; samples may straddle callbacks.
    pub fn snapshot(&self) -> CpuSample {
        CpuSample {
            elapsed: self.elapsed.load(Relaxed),
            budget: self.budget.load(Relaxed),
            peak: self.peak.load(Relaxed),
            frames: self.frames.load(Relaxed),
            rate: self.rate.load(Relaxed),
        }
    }

    /// Reset the peak without resetting other users' monotonic overload counter.
    pub fn reset_peak(&self) {
        self.peak.swap(0, Relaxed);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuSample {
    pub elapsed: u64,
    pub budget: u64,
    pub peak: u32,
    pub frames: u32,
    pub rate: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn load_uses_each_buffers_period_and_peak_is_resettable() {
        let meter = CpuMeter::default();
        meter.record(1_000_000, 480, 48_000);
        assert_eq!(
            meter.snapshot(),
            CpuSample {
                elapsed: 1_000_000,
                budget: 10_000_000,
                peak: 1000,
                frames: 480,
                rate: 48_000
            }
        );
        meter.record(2_000_000, 96, 48_000);
        assert_eq!(meter.snapshot().peak, 10_000);
        meter.record(250_000, 96, 48_000);
        assert_eq!(meter.snapshot().elapsed, 3_250_000);
        assert_eq!(meter.snapshot().budget, 14_000_000);
        meter.reset_peak();
        assert_eq!(meter.snapshot().peak, 0);
        meter.record(1, 0, 48_000);
        assert_eq!(meter.snapshot().peak, 0);
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn callback_measurement_does_not_allocate_or_free() {
        let meter = CpuMeter::default();
        let measured = allocation_counter::measure(|| {
            for _ in 0..256 {
                meter.record(1_000_000, 256, 44_100);
            }
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }

    #[test]
    #[ignore = "optimized telemetry microbenchmark; not a native deadline test"]
    fn measurement_overhead() {
        use std::hint::black_box;
        use std::time::Instant;
        const ITERATIONS: u64 = 1_000_000;
        let meter = CpuMeter::default();
        for _ in 0..3 {
            let start = Instant::now();
            for i in 0..ITERATIONS {
                black_box((i, 256_usize, 44_100_u32));
            }
            let baseline = start.elapsed();
            let start = Instant::now();
            for i in 0..ITERATIONS {
                black_box(&meter).record(black_box(i), black_box(256), black_box(44_100));
            }
            let measured = start.elapsed();
            eprintln!(
                "cpu-meter iterations={ITERATIONS} frames=256 rate=44100 baseline_ns={} instrumented_ns={}",
                baseline.as_nanos(),
                measured.as_nanos()
            );
            black_box(meter.snapshot());
        }
    }
}
