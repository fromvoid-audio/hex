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
fn period<E: From<Error>>(
    sample_rate: u32,
    read: &mut impl FnMut(i128) -> Result<[f64; 2], E>,
    center: f64,
    channel: usize,
    work: &mut Work<'_>,
) -> Result<Option<f64>, E> {
    let step = (sample_rate / 8_000).max(1) as i128;
    let minimum = (sample_rate / 2_000).max(2) as usize / step as usize;
    let maximum = ((sample_rate / 50) as usize / step as usize).min(200);
    let minimum = minimum.max(2).min(maximum.saturating_sub(1));
    let mut correlations = [0.0_f64; 202];
    let center = center.round() as i128;
    let width = 256;
    work.check(((maximum - minimum + 1) * width * 2) as u64)
        .map_err(E::from)?;
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
            let x = read(at)?[channel];
            let y = read(at - lag as i128 * step)?[channel];
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
fn similarity_anchor<E: From<Error>>(
    sample_rate: u32,
    read: &mut impl FnMut(i128) -> Result<[f64; 2], E>,
    desired: f64,
    expected: f64,
    channel: usize,
    window: usize,
    work: &mut Work<'_>,
) -> Result<f64, E> {
    let range = (window / 4).min(256) as i64;
    let step = (sample_rate / 8_000).max(1) as i64;
    let mut best = desired;
    let mut score = f64::NEG_INFINITY;
    work.check((range as u64 * 2 / step as u64 + 1) * 64)
        .map_err(E::from)?;
    for offset in (-range..=range).step_by(step as usize) {
        let candidate = desired + offset as f64;
        let mut xy = 0.0;
        let mut xx = 0.0;
        let mut yy = 0.0;
        for i in 0..64 {
            let delta = (i as i128 - 32) * i128::from(step);
            let x = read(candidate.round() as i128 + delta)?[channel];
            let y = read(expected.round() as i128 + delta)?[channel];
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
/// Parameters for one Tones or Texture grain's control-side analysis.
/// Positions are unwrapped region-relative source frames; `previous_anchor`
/// must be the prior grain's committed result. The caller owns this state.
#[derive(Clone, Copy, Debug)]
pub struct GrainAnalysis {
    pub sample_rate: u32,
    pub mode: Mode,
    pub pitch_cents: i16,
    pub center: i128,
    pub desired: f64,
    pub previous_anchor: Option<f64>,
    pub grain_index: u64,
    pub reference_channel: usize,
}

/// A grain's source anchor and window length, and analysis work charged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrainDecision {
    pub anchor: f64,
    pub length: usize,
    pub work_units: u64,
}

/// Analyze a single grain using source samples resolved by the caller.
///
/// The reader receives unwrapped integral region-relative frame positions and
/// must apply the source's phase, looping, gain and out-of-region silence, just
/// as the resident input does. It should use preloaded bounded windows; this
/// function may request the same position repeatedly and performs no I/O itself.
/// It requests at most 167,552 frames per grain; cancellation is checked at
/// analysis phase boundaries, not between individual sample requests. A reader
/// may use a bounded demand-filled cache instead of preloading all windows.
/// The reader error type must accept warp errors; source errors are returned
/// unchanged so callers can retain their specific I/O or custody context.
/// A reader error or cancellation returns no decision, though reader side effects
/// are not rolled back. Construction and analysis belong on a control worker,
/// never the audio callback. Source identity and custody remain with the caller.
pub fn analyze_grain<E: From<Error>>(
    input: GrainAnalysis,
    read: &mut impl FnMut(i128) -> Result<[f64; 2], E>,
    cancel: &mut dyn FnMut() -> bool,
) -> Result<GrainDecision, E> {
    let (grain_ms, texture) = match input.mode {
        Mode::Tones { grain_ms } if (8..=200).contains(&grain_ms) => (grain_ms, None),
        Mode::Texture {
            grain_ms,
            fluctuation,
            seed,
        } if (8..=200).contains(&grain_ms) && fluctuation <= 100 => {
            (grain_ms, Some((fluctuation, seed)))
        }
        _ => return Err(Error::Invalid("invalid granular analysis mode").into()),
    };
    // Map metadata is bounded to 2^40 source frames; allow room for
    // extrapolated edge grains and preceding anchors without float saturation.
    let valid_position = |value: f64| value.is_finite() && value.abs() <= (1_u64 << 42) as f64;
    if !(8_000..=192_000).contains(&input.sample_rate)
        || !(-4_800..=4_800).contains(&input.pitch_cents)
        || input.center.unsigned_abs() > u128::from(MAX_STREAMED_OUTPUT_FRAMES) + 38_400
        || !valid_position(input.desired)
        || input
            .previous_anchor
            .is_some_and(|value| !valid_position(value))
        || input.reference_channel > 1
    {
        return Err(Error::Invalid("invalid granular analysis parameters").into());
    }
    let nominal = (u64::from(input.sample_rate) * u64::from(grain_ms) / 1_000).max(16) as usize;
    let pitch = 2.0_f64.powf(f64::from(input.pitch_cents) / 1_200.0);
    let mut work = Work {
        cancel,
        used: 0,
        limit: MAX_WORK_UNITS,
        scratch: 0,
        base_scratch: 0,
        resampler: Resampler::V1,
    };
    work.check(0).map_err(E::from)?;
    let (anchor, length) = analyze(
        input.sample_rate,
        nominal,
        texture,
        pitch,
        input.center,
        input.desired,
        input.previous_anchor,
        input.grain_index,
        input.reference_channel,
        read,
        &mut work,
    )?;
    Ok(GrainDecision {
        anchor,
        length,
        work_units: work.used,
    })
}

#[allow(clippy::too_many_arguments)]
fn analyze<E: From<Error>>(
    sample_rate: u32,
    nominal: usize,
    texture: Option<(u8, u64)>,
    pitch: f64,
    center: i128,
    desired: f64,
    previous: Option<f64>,
    grain_index: u64,
    channel: usize,
    read: &mut impl FnMut(i128) -> Result<[f64; 2], E>,
    work: &mut Work<'_>,
) -> Result<(f64, usize), E> {
    let hop = (nominal / 4).max(1);
    if let Some((fluctuation, seed)) = texture {
        let random = (hash(seed ^ grain_index) >> 11) as f64 / ((1_u64 << 53) as f64) * 2.0 - 1.0;
        let desired = desired + random * f64::from(fluctuation) / 100.0 * nominal as f64 * 0.5;
        let anchor = if let Some(previous) = previous {
            similarity_anchor(
                sample_rate,
                read,
                desired,
                previous + pitch * hop as f64,
                channel,
                nominal,
                work,
            )?
        } else {
            desired
        };
        Ok((anchor, nominal))
    } else if let Some(period) = period(sample_rate, read, desired, channel, work)? {
        let correction =
            (pitch * center as f64 - desired + period * 0.5).rem_euclid(period) - period * 0.5;
        let length = (period * (nominal as f64 / period).round().max(2.0))
            .round()
            .clamp(nominal as f64 * 0.5, nominal as f64 * 2.0) as usize;
        Ok((desired + correction, length.max(2)))
    } else {
        let expected = previous.map_or(desired, |anchor: f64| anchor + pitch * hop as f64);
        Ok((
            similarity_anchor(sample_rate, read, desired, expected, channel, nominal, work)?,
            nominal,
        ))
    }
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
    let scratch = ring
        .checked_mul(24)
        .and_then(|bytes| bytes.checked_add(extra))
        .ok_or(Error::ScratchLimit)?;
    work.scratch(scratch)?;
    let mut output = buffer(request.output_frames, [0.0; 2])?;
    let mut accumulator = GranularOverlap::new(request.output_frames as u64, ring)?;
    let bank = Bandlimited::new(pitch, work)?;
    let channel = request.input.reference_channel()?;
    let mut center = -(half_limit as i128);
    let mut grain_index = 0_u64;
    let mut previous = None;
    while center < (request.output_frames + half_limit) as i128 {
        let desired = request.map.position(center)?;
        let mut read = |position| Ok::<_, Error>(request.input.frame(position));
        let (anchor, length) = analyze(
            request.input.sample_rate,
            nominal,
            texture,
            pitch,
            center,
            desired,
            previous,
            grain_index,
            channel,
            &mut read,
            work,
        )?;
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
        drain_output(&mut accumulator, &mut output, finished)?;
    }
    let end = output.len();
    drain_output(&mut accumulator, &mut output, end)?;
    Ok(output)
}

fn drain_output(
    accumulator: &mut GranularOverlap,
    output: &mut [Frame],
    end: usize,
) -> Result<(), Error> {
    let start = usize::try_from(accumulator.position())
        .map_err(|_| Error::Invalid("granular output cursor overflow"))?;
    let end = end.min(output.len());
    if start >= end {
        return Ok(());
    }
    let destination = output
        .get_mut(start..end)
        .ok_or(Error::Invalid("granular output range is invalid"))?;
    accumulator.drain(destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_overlap_matches_existing_accumulator_sample_for_sample() {
        let mut cancel = || false;
        let mut work = Work {
            cancel: &mut cancel,
            used: 0,
            limit: MAX_WORK_UNITS,
            scratch: 0,
            base_scratch: 0,
            resampler: Resampler::V1,
        };
        let mut previous = Accumulator::new(1024, 64, &mut work, 0).unwrap();
        let mut bounded = GranularOverlap::new(1024, 64).unwrap();
        let mut output = [[0.0; 2]; 1024];
        for start in (0..1024).step_by(8) {
            for i in 0..32 {
                let frame = (start + i) as i128;
                let sample = [(frame as f64 * 0.017).sin(), (frame as f64 * 0.031).cos()];
                let weight = (i + 1) as f64 / 33.0;
                previous.add(frame, sample, weight).unwrap();
                bounded.add(frame, sample, weight).unwrap();
            }
            let end = (start + 8).min(output.len());
            previous.finish_until(end).unwrap();
            bounded.drain(&mut output[start..end]).unwrap();
        }
        assert_eq!(output.as_slice(), previous.finish().unwrap());
    }

    #[test]
    fn incremental_reference_matches_resident_loop_phase_and_gain() {
        let frames: Vec<_> = (0..8192)
            .map(|i| [(i as f32 * 0.021).sin(), (i as f32 * 0.037).cos() * 0.8])
            .collect();
        let envelope = [
            GainPoint {
                source_frame: 0,
                gain_q16: 32_768,
            },
            GainPoint {
                source_frame: 4000,
                gain_q16: 98_304,
            },
        ];
        for (length, looping, phase) in [(31, false, 0), (4096, false, 0), (8000, true, 97)] {
            let input = Input {
                frames: &frames,
                sample_rate: 16_000,
                region_start: 8,
                region_length: length,
                phase,
                looping,
                envelope: &envelope,
            };
            let count = length.min(4096);
            let stride = (length / count).max(1);
            let mut energy = [0.0; 2];
            for index in 0..count {
                let frame = input.frame(i128::from(index * stride));
                for channel in 0..2 {
                    energy[channel] += frame[channel] * frame[channel];
                }
            }
            assert_eq!(
                input.reference_channel().unwrap(),
                usize::from(energy[1] > energy[0])
            );
        }
    }

    #[test]
    #[ignore = "explicit optimized reference-scan and PCM profile"]
    fn reference_scan_and_granular_pcm_profile() {
        use std::hint::black_box;
        use std::time::Instant;
        let frames: Vec<_> = (0..8192)
            .map(|i| {
                [
                    (i as f32 * 0.021).sin() * 0.25,
                    (i as f32 * 0.037).cos() * 0.75,
                ]
            })
            .collect();
        let envelope = [
            GainPoint {
                source_frame: 0,
                gain_q16: 32_768,
            },
            GainPoint {
                source_frame: 4096,
                gain_q16: 65_536,
            },
        ];
        let input = Input {
            frames: &frames,
            sample_rate: 16_000,
            region_start: 8,
            region_length: 8000,
            phase: 97,
            looping: true,
            envelope: &envelope,
        };
        let map = TimeMap::new(
            vec![MapSegment {
                output_start: 0,
                output_end: 1024,
                source_start_q32: 0,
                source_end_q32: 768 * Q32,
            }],
            1024,
        )
        .unwrap();
        for law in [Resampler::V1, Resampler::V2] {
            for mode in [
                Mode::Tones { grain_ms: 8 },
                Mode::Texture {
                    grain_ms: 8,
                    fluctuation: 35,
                    seed: 42,
                },
            ] {
                let request = Request {
                    input,
                    map: &map,
                    output_frames: 1024,
                    pitch_cents: 300,
                    mode,
                    attacks: &[],
                };
                let mut samples = Vec::new();
                let mut pcm = Vec::new();
                for iteration in 0..8 {
                    let started = Instant::now();
                    pcm = render_with_resampler(&request, law, &mut || false)
                        .unwrap()
                        .frames;
                    if iteration >= 3 {
                        samples.push(started.elapsed().as_nanos());
                    }
                }
                samples.sort_unstable();
                let hash = pcm
                    .iter()
                    .flatten()
                    .fold(0xcbf29ce484222325u64, |h, sample| {
                        (h ^ u64::from(sample.to_bits())).wrapping_mul(0x100000001b3)
                    });
                eprintln!(
                    "granular_pcm law={law:?} mode={mode:?} frames=1024 warmup=3 samples=5 p50_ns={} hash={hash:016x}",
                    samples[2]
                );
            }
        }
        for _ in 0..5 {
            let start = Instant::now();
            for _ in 0..100 {
                black_box(input.reference_channel().unwrap());
            }
            eprintln!(
                "reference_scan rate=16000 region=8000 samples=4096 iterations=100 elapsed_ns={}",
                start.elapsed().as_nanos()
            );
        }
    }
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
            limit: MAX_WORK_UNITS,
            scratch: 0,
            base_scratch: 0,
            resampler: Resampler::V1,
        };
        let expected = grains(&request, 1.0, &mut cached).unwrap();
        let mut cancel = || false;
        let mut fallback = Work {
            cancel: &mut cancel,
            used: 0,
            limit: MAX_WORK_UNITS,
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
                limit: MAX_WORK_UNITS,
                scratch: 0,
                base_scratch: 0,
                resampler: Resampler::V1,
            };
            let measured = period(
                input.sample_rate,
                &mut |position| Ok::<_, Error>(input.frame(position)),
                2_000.0,
                input.reference_channel().unwrap(),
                &mut work,
            )
            .unwrap()
            .unwrap();
            assert!((16_000.0 / measured - frequency).abs() < frequency * 0.01);
        }
    }
}
