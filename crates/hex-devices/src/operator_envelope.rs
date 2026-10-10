//! Original standalone oscillator amplitude envelopes; not a registered device.
//!
//! Levels are linear amplitudes and times are seconds. The configured decay
//! and release durations end at a relative 1e-5 cutoff, then snap to their
//! exact target. These equations are original, not measured Live transfer laws.

use crate::Invalid;

pub const OPERATOR_ENVELOPES: usize = 4;
/// Continuous curve's relative distance at its configured endpoint.
pub const RELATIVE_CUTOFF: f64 = 1.0e-5;
const MAX_SECONDS: f64 = 120.0;

/// Validated authoring values. Each oscillator owns an independent instance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OperatorEnvelope {
    initial: f64,
    peak: f64,
    sustain: f64,
    attack_seconds: f64,
    decay_seconds: f64,
    release_seconds: f64,
}

impl OperatorEnvelope {
    pub fn new(
        initial: f64,
        peak: f64,
        sustain: f64,
        attack_seconds: f64,
        decay_seconds: f64,
        release_seconds: f64,
    ) -> Result<Self, Invalid> {
        if [initial, peak, sustain]
            .iter()
            .any(|level| !level.is_finite() || !(0.0..=1.0).contains(level))
        {
            return Err(Invalid("operator envelope levels must be finite in [0, 1]"));
        }
        if [attack_seconds, decay_seconds, release_seconds]
            .iter()
            .any(|seconds| !seconds.is_finite() || !(0.0..=MAX_SECONDS).contains(seconds))
        {
            return Err(Invalid(
                "operator envelope times must be finite in [0, 120] seconds",
            ));
        }
        Ok(Self {
            initial,
            peak,
            sustain,
            attack_seconds,
            decay_seconds,
            release_seconds,
        })
    }

    /// Convert times and curve rates on control, before querying sample levels.
    pub fn prepare(self, sample_rate: u32) -> Result<PreparedOperatorEnvelope, Invalid> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(Invalid(
                "operator envelope sample rate must be between 8000 and 192000 hz",
            ));
        }
        // Validated domains bound each conversion to 23,040,000 frames.
        let frames = |seconds: f64| (seconds * f64::from(sample_rate)).round() as u64;
        let attack = frames(self.attack_seconds);
        let decay = frames(self.decay_seconds);
        let release = frames(self.release_seconds);
        let rate = -RELATIVE_CUTOFF.ln();
        Ok(PreparedOperatorEnvelope {
            initial: self.initial,
            peak: self.peak,
            sustain: self.sustain,
            attack,
            decay,
            release,
            attack_inverse: if attack == 0 {
                0.0
            } else {
                1.0 / attack as f64
            },
            decay_rate: if decay == 0 { 0.0 } else { rate / decay as f64 },
            release_rate: if release == 0 {
                0.0
            } else {
                rate / release as f64
            },
        })
    }
}

/// Heap-free immutable sample law. Absolute note age and gate duration are frames.
/// A note's gate is fixed for this pure query; an event-driven voice must latch
/// its actual NoteOff age before using the released part of this law.
#[derive(Debug, Clone, Copy)]
pub struct PreparedOperatorEnvelope {
    initial: f64,
    peak: f64,
    sustain: f64,
    attack: u64,
    decay: u64,
    release: u64,
    attack_inverse: f64,
    decay_rate: f64,
    release_rate: f64,
}

impl PreparedOperatorEnvelope {
    pub const fn attack_frames(&self) -> u64 {
        self.attack
    }
    pub const fn decay_frames(&self) -> u64 {
        self.decay
    }
    /// Envelope-only output tail; excludes feedback, filters and effects.
    pub const fn release_frames(&self) -> u64 {
        self.release
    }

    fn held_level(&self, age: u64) -> f64 {
        if age < self.attack {
            return self.initial + (self.peak - self.initial) * age as f64 * self.attack_inverse;
        }
        let decay_age = age - self.attack;
        if decay_age < self.decay {
            return self.sustain
                + (self.peak - self.sustain) * (-(decay_age as f64) * self.decay_rate).exp();
        }
        self.sustain
    }

    /// NoteOff at `gate_frames` releases from that instant's held level.
    /// Zero-duration stages transition immediately. A zero gate and nonzero
    /// release emits the held level at age zero, then releases; zero release
    /// is immediately silent. No replay or hidden mutable state is involved.
    pub fn level(&self, age_frames: u64, gate_frames: u64) -> f64 {
        if age_frames < gate_frames {
            return self.held_level(age_frames);
        }
        let released = age_frames - gate_frames;
        if released >= self.release {
            return 0.0;
        }
        self.held_level(gate_frames) * (-(released as f64) * self.release_rate).exp()
    }
}

/// Four independently prepared oscillator envelopes, without a voice scheduler.
#[derive(Debug, Clone, Copy)]
pub struct PreparedOperatorEnvelopes {
    envelopes: [PreparedOperatorEnvelope; OPERATOR_ENVELOPES],
}

impl PreparedOperatorEnvelopes {
    pub fn new(
        sample_rate: u32,
        envelopes: [OperatorEnvelope; OPERATOR_ENVELOPES],
    ) -> Result<Self, Invalid> {
        let [a, b, c, d] = envelopes;
        Ok(Self {
            envelopes: [
                a.prepare(sample_rate)?,
                b.prepare(sample_rate)?,
                c.prepare(sample_rate)?,
                d.prepare(sample_rate)?,
            ],
        })
    }

    pub fn levels(&self, age_frames: u64, gate_frames: u64) -> [f64; OPERATOR_ENVELOPES] {
        std::array::from_fn(|index| self.envelopes[index].level(age_frames, gate_frames))
    }

    /// Maximum envelope release duration, not complete instrument quiescence.
    pub fn maximum_tail_frames(&self) -> u64 {
        self.envelopes
            .iter()
            .map(PreparedOperatorEnvelope::release_frames)
            .max()
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Independent reference uses powers of the declared cutoff rather than
    // the prepared reciprocal/exponential implementation.
    fn oracle(levels: [f64; 3], times: [u64; 3], age: u64, gate: u64) -> f64 {
        let [initial, peak, sustain] = levels;
        let [attack, decay, release] = times;
        let held = |at: u64| {
            if at < attack {
                initial * (1.0 - at as f64 / attack as f64) + peak * at as f64 / attack as f64
            } else if at - attack < decay {
                sustain + (peak - sustain) * 0.00001_f64.powf((at - attack) as f64 / decay as f64)
            } else {
                sustain
            }
        };
        if age < gate {
            held(age)
        } else if age - gate >= release {
            0.0
        } else {
            held(gate) * 0.00001_f64.powf((age - gate) as f64 / release as f64)
        }
    }

    fn prepared(levels: [f64; 3], times: [u64; 3]) -> PreparedOperatorEnvelope {
        OperatorEnvelope::new(
            levels[0],
            levels[1],
            levels[2],
            times[0] as f64 / 8000.0,
            times[1] as f64 / 8000.0,
            times[2] as f64 / 8000.0,
        )
        .expect("valid original envelope")
        .prepare(8000)
        .expect("valid sample rate")
    }

    #[test]
    fn full_envelope_matches_independent_oracle_for_noteoff_in_every_stage() {
        for levels in [[0.25, 1.0, 0.4], [0.9, 0.3, 0.8], [0.0, 0.0, 0.0]] {
            let times = [8, 13, 17];
            let envelope = prepared(levels, times);
            for gate in [0, 1, 7, 8, 9, 20, 21, 40] {
                for age in 0..80 {
                    let actual = envelope.level(age, gate);
                    let expected = oracle(levels, times, age, gate);
                    assert!(
                        (actual - expected).abs() <= 1e-12,
                        "age={age}, gate={gate}: {actual} != {expected}"
                    );
                    assert!((0.0..=1.0).contains(&actual));
                }
                assert_eq!(envelope.level(gate, gate), envelope.held_level(gate));
                assert_eq!(envelope.level(gate + 17, gate), 0.0);
            }
        }
    }

    #[test]
    fn zero_duration_stages_and_zero_gate_have_defined_instant_transitions() {
        let levels = [0.2, 0.8, 0.3];
        for attack in [0, 4] {
            for decay in [0, 5] {
                for release in [0, 6] {
                    let times = [attack, decay, release];
                    let envelope = prepared(levels, times);
                    for gate in [0, 1, 4, 5, 9, 12] {
                        for age in 0..25 {
                            assert!(
                                (envelope.level(age, gate) - oracle(levels, times, age, gate))
                                    .abs()
                                    <= 1e-12
                            );
                        }
                    }
                }
            }
        }
        assert_eq!(prepared(levels, [4, 5, 6]).level(0, 0), 0.2);
        assert_eq!(prepared(levels, [0, 5, 6]).level(0, 0), 0.8);
        assert_eq!(prepared(levels, [0, 0, 6]).level(0, 0), 0.3);
        assert_eq!(prepared(levels, [4, 5, 0]).level(0, 0), 0.0);
    }

    #[test]
    fn frame_rounding_and_maximum_release_are_bounded_at_supported_rates() {
        for rate in [8_000, 44_100, 48_000, 192_000] {
            let envelope =
                OperatorEnvelope::new(0.0, 1.0, 0.5, 2.5 / rate as f64, 0.49 / rate as f64, 120.0)
                    .expect("valid times")
                    .prepare(rate)
                    .expect("supported rate");
            assert_eq!(envelope.attack_frames(), 3);
            assert_eq!(envelope.decay_frames(), 0);
            assert_eq!(envelope.release_frames(), 120 * u64::from(rate));
            assert_eq!(envelope.level(u64::MAX, 0), 0.0);
            assert!(envelope.level(u64::MAX, u64::MAX).is_finite());
        }
    }

    #[test]
    fn malformed_authoring_values_and_rates_are_refused() {
        let base = [0.2, 0.8, 0.3, 0.01, 0.02, 0.03];
        for index in 0..6 {
            for invalid in [
                f64::NAN,
                f64::INFINITY,
                f64::NEG_INFINITY,
                -0.01,
                if index < 3 { 1.01 } else { 120.01 },
            ] {
                let mut values = base;
                values[index] = invalid;
                assert!(
                    OperatorEnvelope::new(
                        values[0], values[1], values[2], values[3], values[4], values[5]
                    )
                    .is_err()
                );
            }
        }
        for rate in [0, 7_999, 192_001, u32::MAX] {
            assert!(
                OperatorEnvelope::new(0.0, 1.0, 0.0, 0.0, 0.0, 0.0)
                    .expect("valid config")
                    .prepare(rate)
                    .is_err()
            );
        }
    }

    #[test]
    fn four_envelopes_remain_independent_under_partitioned_and_reordered_queries() {
        let levels = [
            [0.2, 0.9, 0.4],
            [0.1, 0.6, 0.3],
            [0.8, 0.2, 0.7],
            [0.0, 1.0, 0.0],
        ];
        let times = [[5, 9, 11], [3, 7, 17], [0, 13, 5], [11, 0, 23]];
        let configs = std::array::from_fn(|i| {
            OperatorEnvelope::new(
                levels[i][0],
                levels[i][1],
                levels[i][2],
                times[i][0] as f64 / 8000.0,
                times[i][1] as f64 / 8000.0,
                times[i][2] as f64 / 8000.0,
            )
            .expect("valid bank entry")
        });
        let bank = PreparedOperatorEnvelopes::new(8000, configs).expect("valid bank");
        assert_eq!(bank.maximum_tail_frames(), 23);
        let whole: Vec<_> = (0..80).map(|age| bank.levels(age, 7)).collect();
        for chunks in [1, 3, 16, 79] {
            let partitioned: Vec<_> = (0..80)
                .collect::<Vec<_>>()
                .chunks(chunks)
                .flat_map(|chunk| chunk.iter().map(|&age| bank.levels(age, 7)))
                .collect();
            assert_eq!(partitioned, whole);
        }
        for age in (0..80).rev() {
            assert_eq!(bank.levels(age, 7), whole[age as usize]);
            for (i, actual) in bank.levels(age, 7).into_iter().enumerate() {
                assert!((actual - oracle(levels[i], times[i], age, 7)).abs() <= 1e-12);
            }
        }
    }
}
