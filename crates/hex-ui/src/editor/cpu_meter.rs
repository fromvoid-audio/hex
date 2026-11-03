//! UI-side smoothing and overload hold for the audio callback load meter.
use hex_audio::cpu_meter::CpuSample;
use std::time::{Duration, Instant};

const INTERVAL: Duration = Duration::from_millis(100);
const HOLD: Duration = Duration::from_secs(3);

#[derive(Default)]
pub(super) struct CpuMeter {
    source: Option<u64>,
    last_update: Option<Instant>,
    average: Option<f64>,
    sample: CpuSample,
    baseline: u64,
    count: u64,
    lit_until: Option<Instant>,
    shown: bool,
}

impl CpuMeter {
    pub fn observe(&mut self, input: Option<(u64, CpuSample, u64)>, now: Instant) -> bool {
        let Some((source, sample, misses)) = input else {
            let changed = self.source.is_some();
            *self = Self::default();
            return changed;
        };
        let source_changed = self.source != Some(source);
        if source_changed {
            *self = Self {
                source: Some(source),
                baseline: misses,
                ..Self::default()
            };
        }
        let previous_shown = self.shown;
        let previous_count = self.count;
        self.count = misses.saturating_sub(self.baseline);
        if self.count > previous_count {
            self.lit_until = now.checked_add(HOLD);
        }
        self.shown = self.lit_until.is_some_and(|until| now < until);
        let due = self
            .last_update
            .is_none_or(|last| now.saturating_duration_since(last) >= INTERVAL);
        if !due {
            return source_changed
                || previous_shown != self.shown
                || (self.shown && self.count != previous_count);
        }
        self.last_update = Some(now);
        let previous_label = self.label();
        let previous_sample = self.sample;
        self.sample = sample;
        let elapsed = sample.elapsed.wrapping_sub(previous_sample.elapsed);
        let budget = sample.budget.wrapping_sub(previous_sample.budget);
        if sample.frames != 0 && sample.rate != 0 && budget != 0 {
            let value = elapsed as f64 * 100.0 / budget as f64;
            self.average = Some(self.average.map_or(value, |old| old * 0.7 + value * 0.3));
        }
        source_changed
            || previous_label != self.label()
            || previous_sample.peak != sample.peak
            || previous_sample.frames != sample.frames
            || previous_sample.rate != sample.rate
            || previous_shown != self.shown
            || (self.shown && self.count != previous_count)
    }

    pub fn reset(&mut self, misses: u64) {
        self.baseline = misses;
        self.count = 0;
        self.sample.peak = 0;
        self.lit_until = None;
        self.shown = false;
    }

    pub fn label(&self) -> String {
        self.average.map_or_else(
            || "cpu —".to_owned(),
            |value| format!("cpu {:.0}%", value.min(9999.0)),
        )
    }

    pub fn warning(&self) -> bool {
        self.shown
    }

    pub fn fraction(&self) -> f32 {
        self.average.unwrap_or(0.0).clamp(0.0, 100.0) as f32 / 100.0
    }

    pub fn description(&self) -> String {
        if self.average.is_none() {
            return "audio callback load · output unavailable".into();
        }
        format!(
            "audio callback load · peak {:.1}% · {} frames · {} hz · {} callbacks over budget · click to reset peak and count",
            f64::from(self.sample.peak) / 100.0,
            self.sample.frames,
            self.sample.rate,
            self.count
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(elapsed: u64, budget: u64, peak: u32) -> CpuSample {
        CpuSample {
            elapsed,
            budget,
            peak,
            frames: 256,
            rate: 44_100,
        }
    }
    #[test]
    fn smoothing_throttling_peak_reset_and_overload_hold() {
        let now = Instant::now();
        let mut meter = CpuMeter::default();
        assert_eq!(meter.label(), "cpu —");
        assert!(meter.observe(Some((1, sample(10, 100, 1000), 0)), now));
        assert_eq!(meter.label(), "cpu 10%");
        assert!(!meter.observe(
            Some((1, sample(60, 200, 5000), 0)),
            now + Duration::from_millis(20)
        ));
        assert!(meter.observe(Some((1, sample(60, 200, 5000), 1)), now + INTERVAL));
        assert_eq!(meter.label(), "cpu 22%");
        assert!(meter.warning());
        assert!(meter.description().contains("1 callbacks over budget"));
        assert!(meter.observe(Some((1, sample(110, 300, 5000), 1)), now + INTERVAL + HOLD));
        assert!(!meter.warning());
        meter.reset(1);
        assert!(meter.description().contains("0 callbacks over budget"));
        assert!(meter.observe(None, now + INTERVAL + HOLD));
        assert_eq!(meter.label(), "cpu —");
    }
    #[test]
    fn replacement_output_resets_the_old_peak_and_miss_count() {
        let now = Instant::now();
        let mut meter = CpuMeter::default();
        meter.observe(Some((1, sample(100, 100, 10_000), 3)), now);
        meter.observe(Some((1, sample(200, 200, 10_000), 4)), now + INTERVAL);
        meter.observe(Some((2, sample(5, 100, 500), 0)), now + INTERVAL * 2);
        assert_eq!(meter.label(), "cpu 5%");
        assert!(!meter.warning());
        assert!(meter.description().contains("0 callbacks over budget"));
    }
}
