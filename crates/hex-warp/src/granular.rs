//! Pitch-synchronous and deterministic textured granulation. Beats owns its plan.
use super::{
    resample::{Bandlimited, FrameCache, bank_bytes},
    *,
};

fn hash(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e3779b97f4a7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}
fn period(
    input: Input<'_>,
    center: f64,
    channel: usize,
    work: &mut Work<'_>,
) -> Result<Option<f64>, Error> {
    let step = (input.sample_rate / 8_000).max(1) as i128;
    let minimum = (input.sample_rate / 2_000).max(2) as usize / step as usize;
    let maximum = ((input.sample_rate / 50) as usize / step as usize).min(200);
    let minimum = minimum.max(2).min(maximum.saturating_sub(1));
    let mut correlations = [0.0_f64; 202];
    let center = center.round() as i128;
    let width = 256;
    work.check(((maximum - minimum + 1) * width * 2) as u64)?;
    for (lag, correlation) in correlations
        .iter_mut()
        .enumerate()
        .take(maximum + 1)
        .skip(minimum)
    {
        let mut xy = 0.0;
        let mut xx = 0.0;
        let mut yy = 0.0;
        for index in 0..width {
            let at = center + (index as i128 - width as i128 / 2) * step;
            let x = input.frame(at)[channel];
            let y = input.frame(at - lag as i128 * step)[channel];
            xy += x * y;
            xx += x * x;
            yy += y * y;
        }
        *correlation = if xx * yy > 1e-20 {
            xy / (xx * yy).sqrt()
        } else {
            0.0
        };
    }
    let selected = (minimum + 1..maximum).find(|&lag| {
        correlations[lag] > 0.75
            && correlations[lag] >= correlations[lag - 1]
            && correlations[lag] > correlations[lag + 1]
    });
    let Some(lag) = selected else { return Ok(None) };
    let left = correlations[lag - 1];
    let middle = correlations[lag];
    let right = correlations[lag + 1];
    let denominator = left - 2.0 * middle + right;
    let adjustment = if denominator.abs() > 1e-12 {
        (0.5 * (left - right) / denominator).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    Ok(Some((lag as f64 + adjustment) * step as f64))
}
fn similarity_anchor(
    input: Input<'_>,
    desired: f64,
    expected: f64,
    channel: usize,
    window: usize,
    work: &mut Work<'_>,
) -> Result<f64, Error> {
    let range = (window / 4).min(256) as i64;
    let step = (input.sample_rate / 8_000).max(1) as i64;
    let mut best = desired;
    let mut score = f64::NEG_INFINITY;
    work.check((range as u64 * 2 / step as u64 + 1) * 64)?;
    for offset in (-range..=range).step_by(step as usize) {
        let candidate = desired + offset as f64;
        let mut xy = 0.0;
        let mut xx = 0.0;
        let mut yy = 0.0;
        for i in 0..64 {
            let delta = (i as i128 - 32) * i128::from(step);
            let x = input.frame(candidate.round() as i128 + delta)[channel];
            let y = input.frame(expected.round() as i128 + delta)[channel];
            xy += x * y;
            xx += x * x;
            yy += y * y;
        }
        let correlation = if xx * yy > 1e-20 {
            xy / (xx * yy).sqrt()
        } else {
            -1.0
        };
        let penalty = offset.unsigned_abs() as f64 * 1e-7;
        if correlation - penalty > score {
            score = correlation - penalty;
            best = candidate;
        }
    }
    Ok(best)
}
pub(super) fn grains(
    request: &Request<'_>,
    pitch: f64,
    work: &mut Work<'_>,
) -> Result<Vec<Frame>, Error> {
    let (grain_ms, texture) = match request.mode {
        Mode::Tones { grain_ms } => (grain_ms, None),
        Mode::Texture {
            grain_ms,
            fluctuation,
            seed,
        } => (grain_ms, Some((fluctuation, seed))),
        _ => return Err(Error::Invalid("grain mode mismatch")),
    };
    let nominal =
        (u64::from(request.input.sample_rate) * u64::from(grain_ms) / 1_000).max(16) as usize;
    let half_limit = nominal;
    let hop = (nominal / 4).max(1);
    let ring = half_limit * 2 + hop + 4;
    let baseline_extra = bank_bytes(work.resampler) + 202 * 8;
    let cache_fits = |bytes: usize| {
        ring.checked_mul(24)
            .and_then(|n| n.checked_add(baseline_extra))
            .and_then(|n| n.checked_add(work.base_scratch))
            .and_then(|n| n.checked_add(bytes))
            .is_some_and(|n| n <= MAX_SCRATCH_BYTES)
    };
    // The cache is optional: preserve admission and rendering when the original
    // scratch budget fits but the cache does not, or its allocation fails.
    let mut frame_cache = if cache_fits(FrameCache::minimum_bytes()) {
        FrameCache::try_new(request.input)
            .filter(|cache| cache.allocated_bytes().is_some_and(cache_fits))
    } else {
        None
    };
    let extra = baseline_extra
        + frame_cache
            .as_ref()
            .and_then(FrameCache::allocated_bytes)
            .unwrap_or(0);
    let mut accumulator = Accumulator::new(request.output_frames, ring, work, extra)?;
    let bank = Bandlimited::new(pitch, work)?;
    let channel = request.input.reference_channel();
    let mut center = -(half_limit as i128);
    let mut grain_index = 0_u64;
    let mut previous = None;
    while center < (request.output_frames + half_limit) as i128 {
        let desired = request.map.position(center)?;
        let (anchor, length) = if let Some((fluctuation, seed)) = texture {
            let random =
                (hash(seed ^ grain_index) >> 11) as f64 / ((1_u64 << 53) as f64) * 2.0 - 1.0;
            {
                let desired =
                    desired + random * f64::from(fluctuation) / 100.0 * nominal as f64 * 0.5;
                let anchor = if let Some(previous) = previous {
                    similarity_anchor(
                        request.input,
                        desired,
                        previous + pitch * hop as f64,
                        channel,
                        nominal,
                        work,
                    )?
                } else {
                    desired
                };
                (anchor, nominal)
            }
        } else if let Some(period) = period(request.input, desired, channel, work)? {
            let correction =
                (pitch * center as f64 - desired + period * 0.5).rem_euclid(period) - period * 0.5;
            let length = (period * (nominal as f64 / period).round().max(2.0))
                .round()
                .clamp(nominal as f64 * 0.5, nominal as f64 * 2.0)
                as usize;
            (desired + correction, length.max(2))
        } else {
            let expected = previous.map_or(desired, |anchor: f64| anchor + pitch * hop as f64);
            (
                similarity_anchor(request.input, desired, expected, channel, nominal, work)?,
                nominal,
            )
        };
        work.check(length as u64 * 64)?;
        previous = Some(anchor);
        let first = center - (length / 2) as i128;
        for offset in 0..length {
            let frame = first + offset as i128;
            let window = 0.5
                - 0.5 * (2.0 * std::f64::consts::PI * offset as f64 / (length - 1) as f64).cos();
            let position = anchor + (frame - center) as f64 * pitch;
            let sample = match frame_cache.as_mut() {
                Some(cache) => bank.sample_float_cached(position, cache),
                None => bank.sample_float(request.input, position),
            };
            accumulator.add(frame, sample, window)?;
        }
        center += hop as i128;
        grain_index = grain_index.wrapping_add(1);
        let finished = (center - half_limit as i128).max(0) as usize;
        accumulator.finish_until(finished)?;
    }
    accumulator.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_falls_back_when_original_scratch_budget_is_full() {
        let frames: Vec<_> = (0..256)
            .map(|i| [i as f32 / 256.0, -(i as f32) / 256.0])
            .collect();
        let input = Input {
            frames: &frames,
            sample_rate: 8000,
            region_start: 0,
            region_length: 256,
            phase: 0,
            looping: false,
            envelope: &[],
        };
        let map = TimeMap::new(
            vec![MapSegment {
                output_start: 0,
                output_end: 256,
                source_start_q32: 0,
                source_end_q32: 256 * Q32,
            }],
            256,
        )
        .unwrap();
        let request = Request {
            attacks: &[],
            input,
            map: &map,
            output_frames: 256,
            pitch_cents: 0,
            mode: Mode::Texture {
                grain_ms: 20,
                fluctuation: 10,
                seed: 4,
            },
        };
        let baseline = (160 * 2 + 40 + 4) * 24 + bank_bytes(Resampler::V1) + 202 * 8;
        let mut cancel = || false;
        let mut cached = Work {
            cancel: &mut cancel,
            used: 0,
            scratch: 0,
            base_scratch: 0,
            resampler: Resampler::V1,
        };
        let expected = grains(&request, 1.0, &mut cached).unwrap();
        let mut cancel = || false;
        let mut fallback = Work {
            cancel: &mut cancel,
            used: 0,
            scratch: 0,
            base_scratch: MAX_SCRATCH_BYTES - baseline,
            resampler: Resampler::V1,
        };
        let actual = grains(&request, 1.0, &mut fallback).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(cached.used, fallback.used);
        assert_eq!(fallback.scratch, MAX_SCRATCH_BYTES);
    }
    #[test]
    fn period_analysis_tracks_pitch_without_cancelling_antiphase_stereo() {
        for frequency in [220.0, 880.0] {
            let frames: Vec<_> = (0..4_000)
                .map(|i| {
                    let sample =
                        (2.0 * std::f64::consts::PI * frequency * i as f64 / 16_000.0).sin() as f32;
                    [sample, -sample]
                })
                .collect();
            let input = Input {
                frames: &frames,
                sample_rate: 16_000,
                region_start: 0,
                region_length: 4_000,
                phase: 0,
                looping: false,
                envelope: &[],
            };
            let mut cancel = || false;
            let mut work = Work {
                cancel: &mut cancel,
                used: 0,
                scratch: 0,
                base_scratch: 0,
                resampler: Resampler::V1,
            };
            let measured = period(input, 2_000.0, input.reference_channel(), &mut work)
                .unwrap()
                .unwrap();
            assert!((16_000.0 / measured - frequency).abs() < frequency * 0.01);
        }
    }
}
