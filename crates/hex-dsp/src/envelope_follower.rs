//! Sample-stepped stereo peak envelope for reusable audio-driven modulation.
//! The caller supplies the signal and owns clock, routing and transactions.
use crate::StereoFrame;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidEnvelopeFollower;

/// A pair of one-pole time constants and a normalized current envelope.
/// Input is peak-rectified across channels and bounded to 0..1. A time
/// constant of zero follows that edge immediately. Otherwise the envelope
/// covers 1 - exp(-1) of a sustained level change in one time constant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvelopeFollower {
    attack: f64,
    release: f64,
    envelope: f64,
}

impl EnvelopeFollower {
    pub fn new(
        sample_rate: u32,
        attack_seconds: f64,
        release_seconds: f64,
    ) -> Result<Self, InvalidEnvelopeFollower> {
        if sample_rate == 0
            || ![attack_seconds, release_seconds]
                .into_iter()
                .all(|seconds| seconds.is_finite() && seconds >= 0.0)
        {
            return Err(InvalidEnvelopeFollower);
        }
        let coefficient = |seconds: f64| {
            if seconds == 0.0 {
                1.0
            } else {
                -(-1.0 / seconds / f64::from(sample_rate)).exp_m1()
            }
        };
        Ok(Self {
            attack: coefficient(attack_seconds),
            release: coefficient(release_seconds),
            envelope: 0.0,
        })
    }

    /// Validate the complete borrow before advancing history or writing samples.
    /// The caller may reuse its output storage and split a signal arbitrarily.
    pub fn render(
        &mut self,
        input: &[StereoFrame],
        output: &mut [f32],
    ) -> Result<(), InvalidEnvelopeFollower> {
        if input.len() != output.len() || input.iter().flatten().any(|sample| !sample.is_finite()) {
            return Err(InvalidEnvelopeFollower);
        }
        for ([left, right], sample) in input.iter().zip(output) {
            let peak = f64::from(left.abs().max(right.abs()).min(1.0));
            let coefficient = if peak > self.envelope {
                self.attack
            } else {
                self.release
            };
            self.envelope += (peak - self.envelope) * coefficient;
            *sample = self.envelope as f32;
        }
        Ok(())
    }

    /// Seek or explicit reset clears prior signal history.
    pub fn reset(&mut self) {
        self.envelope = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_independent_exponential_laws_across_rates_and_partitions() {
        for rate in [24_000, 48_000, 96_000] {
            let attack = 4.0 / f64::from(rate);
            let release = 8.0 / f64::from(rate);
            let mut whole = EnvelopeFollower::new(rate, attack, release).unwrap();
            let mut partitioned = whole;
            let mut input = [[0.125, -0.8]; 11];
            input[5..].fill([0.0; 2]);
            let mut actual = [0.0; 11];
            let mut split = actual;
            whole.render(&input, &mut actual).unwrap();
            partitioned.render(&input[..3], &mut split[..3]).unwrap();
            partitioned.render(&input[3..], &mut split[3..]).unwrap();
            assert_eq!(actual, split);
            let attack_value = |frames: u32| 0.8 * (1.0 - (-f64::from(frames) / 4.0).exp());
            for (i, value) in actual.into_iter().enumerate() {
                let expected = if i < 5 {
                    attack_value((i + 1) as u32)
                } else {
                    attack_value(5) * (-((i - 4) as f64) / 8.0).exp()
                };
                assert!((f64::from(value) - expected).abs() < 1e-7);
            }
        }
    }

    #[test]
    fn validates_before_mutation_and_resets_silence() {
        for (rate, attack, release) in [(0, 1.0, 1.0), (1, f64::NAN, 1.0), (1, 0.0, -1.0)] {
            assert_eq!(
                EnvelopeFollower::new(rate, attack, release),
                Err(InvalidEnvelopeFollower)
            );
        }
        let mut follower = EnvelopeFollower::new(48_000, -0.0, 1.0).unwrap();
        let mut output = [0.5; 2];
        let original = follower;
        assert!(
            follower
                .render(&[[1.0; 2], [0.0, f32::INFINITY]], &mut output)
                .is_err()
        );
        assert_eq!(follower, original);
        assert_eq!(output, [0.5; 2]);
        follower.render(&[[0.5, -2.0]], &mut output[..1]).unwrap();
        assert_eq!(output[0], 1.0);
        follower.reset();
        follower.render(&[[0.0; 2]], &mut output[..1]).unwrap();
        assert_eq!(output[0], 0.0);
    }
}
