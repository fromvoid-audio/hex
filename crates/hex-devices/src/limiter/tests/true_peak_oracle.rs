//! Independent finite-record cardinal reconstruction for limiter tests.
//!
//! The record is zero outside its supplied f32 samples. A finite phase grid
//! bounds only the positions it visits; it is not a bound for all phases or
//! evidence about a physical converter.
use hex_lattice::StereoFrame;
use std::{f64::consts::PI, num::NonZeroUsize};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Peak {
    pub value: f64,
    pub position: f64,
    pub lane: usize,
}

/// Reconstruct both lanes at one finite position. Reject nonfinite PCM or a
/// nonfinite position instead of allowing a meaningless oracle result.
pub(super) fn at(frames: &[StereoFrame], position: f64) -> Option<[f64; 2]> {
    if !position.is_finite() || !finite(frames) {
        return None;
    }
    Some(at_valid(frames, position))
}

/// Search both lanes at evenly spaced phases between the first and final
/// samples, including both endpoints. Empty or nonfinite records have no peak.
pub(super) fn grid_peak(frames: &[StereoFrame], phases: NonZeroUsize) -> Option<Peak> {
    if frames.is_empty() || !finite(frames) {
        return None;
    }
    let mut peak = Peak {
        value: 0.0,
        position: 0.0,
        lane: 0,
    };
    for index in 0..frames.len() {
        let count = if index + 1 == frames.len() {
            1
        } else {
            phases.get()
        };
        for phase in 0..count {
            let position = index as f64 + phase as f64 / phases.get() as f64;
            for (lane, value) in at_valid(frames, position).into_iter().enumerate() {
                if value.abs() > peak.value {
                    peak = Peak {
                        value: value.abs(),
                        position,
                        lane,
                    };
                }
            }
        }
    }
    Some(peak)
}

fn finite(frames: &[StereoFrame]) -> bool {
    frames.iter().flatten().all(|sample| sample.is_finite())
}

fn at_valid(frames: &[StereoFrame], position: f64) -> [f64; 2] {
    let floor = position.floor();
    let fraction = position - floor;
    if fraction == 0.0 {
        return if floor >= 0.0 {
            frames
                .get(floor as usize)
                .copied()
                .unwrap_or([0.0; 2])
                .map(f64::from)
        } else {
            [0.0; 2]
        };
    }
    // A representable f64 with a fractional part has magnitude below 2^52,
    // so its integer floor is representable in i64. The alternating-sine
    // identity avoids a trigonometric call per input sample.
    let base = floor as i64;
    let scale = (PI * fraction).sin() / PI;
    let mut sum = [0.0_f64; 2];
    let mut correction = [0.0_f64; 2];
    for (index, frame) in frames.iter().enumerate() {
        let sign = if ((base as u64) ^ (index as u64)) & 1 == 0 {
            1.0
        } else {
            -1.0
        };
        let weight = sign * scale / (position - index as f64);
        for lane in 0..2 {
            let adjusted = f64::from(frame[lane]) * weight - correction[lane];
            let next = sum[lane] + adjusted;
            correction[lane] = (next - sum[lane]) - adjusted;
            sum[lane] = next;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analytic_two_sample_peak_and_integer_positions_calibrate_the_oracle() {
        let frames = [[1.0, -1.0], [1.0, -1.0]];
        assert_eq!(at(&frames, 0.0), Some([1.0, -1.0]));
        assert_eq!(at(&frames, 1.0), Some([1.0, -1.0]));
        assert_eq!(at(&frames, -1.0), Some([0.0, 0.0]));
        let midpoint = at(&frames, 0.5).unwrap();
        assert!((midpoint[0] - 4.0 / PI).abs() < 1e-12);
        assert!((midpoint[1] + 4.0 / PI).abs() < 1e-12);
        let outside = at(&frames, -0.5).unwrap();
        assert!((outside[0] - 4.0 / (3.0 * PI)).abs() < 1e-12);
        let peak = grid_peak(&frames, NonZeroUsize::new(32).unwrap()).unwrap();
        assert!((peak.value - 4.0 / PI).abs() < 1e-12);
        assert_eq!(peak.position, 0.5);
        assert_eq!(peak.lane, 0);
    }

    #[test]
    fn finite_record_grid_finds_an_intersample_overshoot_inside_silence() {
        let mut frames = vec![[0.0; 2]; 256];
        frames[120] = [1.0, 0.0];
        frames[121] = [1.0, 0.0];
        let peak = grid_peak(&frames, NonZeroUsize::new(16).unwrap()).unwrap();
        assert!((peak.value - 4.0 / PI).abs() < 1e-12);
        assert_eq!(peak.position, 120.5);
        assert_eq!(peak.lane, 0);
    }

    #[test]
    fn alternating_identity_matches_direct_sinc_and_rejects_invalid_records() {
        let frames = [[0.25, -0.5], [-0.75, 0.125], [0.5, 0.75], [-0.125, -1.0]];
        for position in [-1.75_f64, -0.5, 0.125, 1.375, 2.875, 4.25] {
            let actual = at(&frames, position).unwrap();
            for lane in 0..2 {
                let direct: f64 = frames
                    .iter()
                    .enumerate()
                    .map(|(index, frame)| {
                        let delta = position - index as f64;
                        f64::from(frame[lane]) * (PI * delta).sin() / (PI * delta)
                    })
                    .sum();
                assert!((actual[lane] - direct).abs() < 1e-12);
            }
        }
        assert!(at(&frames, f64::NAN).is_none());
        assert!(at(&[[f32::INFINITY, 0.0]], 0.5).is_none());
        assert!(grid_peak(&[], NonZeroUsize::new(8).unwrap()).is_none());
        assert!(grid_peak(&[[f32::NAN, 0.0]], NonZeroUsize::new(8).unwrap()).is_none());
    }
}
