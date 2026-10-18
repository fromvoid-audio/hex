//! Distinct transient, pitch-synchronous and deterministic textured granulation.
use super::{
    resample::{BANK_BYTES, Bandlimited},
    *,
};

#[derive(Clone, Copy)]
struct Attack {
    output: u64,
    source: i128,
}
fn push_attack(attacks: &mut Vec<Attack>, attack: Attack) -> Result<(), Error> {
    if attacks
        .last()
        .is_some_and(|last| last.output >= attack.output)
    {
        return Ok(());
    }
    if attacks.len() >= MAX_MAP_SEGMENTS {
        return Err(Error::Invalid("transient schedule exceeds 8192 events"));
    }
    attacks.try_reserve(1).map_err(|_| Error::Allocation)?;
    attacks.push(attack);
    Ok(())
}

/// One-sided endpoint estimate, never sampling the arriving attack. A locally
/// predictable background uses its endpoint; incoherent noise uses its local
/// mean so a random last sample does not become a new decaying low-frequency
/// pulse. The predictability decision and time window are shared by stereo.
fn off_restart_baseline(
    input: Input<'_>,
    bank: &Bandlimited,
    start: i128,
    end: i128,
    pitch: f64,
    work: &mut Work<'_>,
) -> Result<[f64; 2], Error> {
    let available = (((end - start) as f64 / pitch).floor() as usize).max(1);
    let count = ((input.sample_rate / 2_000).max(2) as usize).min(available);
    work.check(count as u64 * 64)?;
    let center = (count - 1) as f64 * 0.5;
    let mut sum = [0.0; 2];
    let mut square = [0.0; 2];
    let mut slope = [0.0; 2];
    let mut denominator = 0.0;
    let mut minimum = [f64::INFINITY; 2];
    let mut maximum = [f64::NEG_INFINITY; 2];
    for i in 0..count {
        let t = i as f64 - center;
        let sample = bank.sample_segment(
            input,
            end as f64 - (count - i) as f64 * pitch,
            start,
            end - 1,
        );
        denominator += t * t;
        for c in 0..2 {
            sum[c] += sample[c];
            square[c] += sample[c] * sample[c];
            slope[c] += t * sample[c];
            minimum[c] = minimum[c].min(sample[c]);
            maximum[c] = maximum[c].max(sample[c]);
        }
    }
    let variance: f64 = (0..2)
        .map(|c| (square[c] - sum[c] * sum[c] / count as f64).max(0.0))
        .sum();
    let fitted = if denominator > 0.0 {
        slope.iter().map(|s| s * s / denominator).sum()
    } else {
        0.0
    };
    let predictable = variance - fitted <= variance * 0.05 + 1e-16;
    Ok(std::array::from_fn(|c| {
        let mean = sum[c] / count as f64;
        if predictable && denominator > 0.0 {
            (mean + slope[c] / denominator * center).clamp(minimum[c], maximum[c])
        } else {
            mean
        }
    }))
}

pub(super) fn beats(
    request: &Request<'_>,
    pitch: f64,
    work: &mut Work<'_>,
) -> Result<Vec<Frame>, Error> {
    if matches!(
        request.mode,
        Mode::Beats {
            tail: Tail::Forward,
            decay: 100,
            ..
        }
    ) {
        return crate::beats::render(request, pitch, work);
    }
    let Mode::Beats {
        division_frames,
        tail,
        decay,
    } = request.mode
    else {
        return Err(Error::Invalid("transient mode mismatch"));
    };
    let mut starts = Vec::new();
    starts
        .try_reserve(MAX_POINTS)
        .map_err(|_| Error::Allocation)?;
    starts.push(0_u64);
    if !request.attacks.is_empty() {
        starts.extend_from_slice(&request.attacks[1..]);
    } else if let Some(division) = division_frames {
        if division == 0 {
            return Err(Error::Invalid("beat division must be positive"));
        }
        let mut source = division;
        while source < request.input.region_length {
            if starts.len() == MAX_POINTS {
                return Err(Error::Invalid(
                    "beat divisions exceed 1024 attacks per source region",
                ));
            }
            starts.push(source);
            source = source
                .checked_add(division)
                .ok_or(Error::Invalid("beat division overflow"))?;
        }
    } else {
        let minimum_separation = u64::from((request.input.sample_rate / 200).max(1));
        // Seed from the first refractory interval so sustained material cannot
        // masquerade as an onset while the long envelope warms from silence.
        // The release is time based, independent of source sample rate.
        let warmup = request.input.region_length.min(minimum_separation).max(1);
        let mut slow = 0.0;
        for frame in 0..warmup {
            if frame % 1_024 == 0 {
                work.check(1_024)?;
            }
            let sample = request.input.frame(i128::from(frame));
            slow += (sample[0] * sample[0] + sample[1] * sample[1]) / warmup as f64;
        }
        let coefficient = 1.0 - (-1.0 / (f64::from(request.input.sample_rate) * 0.020)).exp();
        let mut last = 0;
        for frame in 0..request.input.region_length {
            if frame % 1_024 == 0 {
                work.check(1_024)?;
            }
            let sample = request.input.frame(i128::from(frame));
            let energy = sample[0] * sample[0] + sample[1] * sample[1];
            if frame >= last + minimum_separation && energy > 1e-12 && energy > slow * 8.0 {
                if starts.len() == MAX_POINTS {
                    return Err(Error::Invalid(
                        "transients exceed 1024 attacks per source region",
                    ));
                }
                starts.push(frame);
                last = frame;
            }
            slow += (energy - slow) * coefficient;
        }
    }
    let first = request
        .map
        .segments()
        .first()
        .ok_or(Error::Invalid("missing transient map"))?
        .source_start_q32
        .div_euclid(Q32);
    let end = request
        .map
        .segments()
        .last()
        .ok_or(Error::Invalid("missing transient map"))?
        .source_end_q32
        .div_euclid(Q32);
    let region = i128::from(request.input.region_length);
    let first_cycle = first.div_euclid(region).max(0);
    let last_cycle = end.div_euclid(region);
    let cycles = last_cycle
        .checked_sub(first_cycle)
        .and_then(|n| n.checked_add(1))
        .ok_or(Error::Invalid("transient cycle overflow"))?;
    if cycles > MAX_MAP_SEGMENTS as i128 {
        return Err(Error::Invalid(
            "transient loop density exceeds bounded schedule",
        ));
    }
    let mut attacks = Vec::new();
    for cycle in first_cycle..=last_cycle {
        work.check(starts.len() as u64)?;
        for source in &starts {
            let source = cycle * region + i128::from(*source);
            if let Some(output) = request.map.output_for_source(source * Q32) {
                push_attack(&mut attacks, Attack { output, source })?;
            }
        }
    }
    if attacks.first().is_none_or(|event| event.output != 0) {
        attacks.try_reserve(1).map_err(|_| Error::Allocation)?;
        attacks.insert(
            0,
            Attack {
                output: 0,
                source: first,
            },
        );
    }
    let mut output = buffer(request.output_frames, [0.0; 2])?;
    let bank = Bandlimited::new(pitch, work)?;
    work.scratch(BANK_BYTES + MAX_POINTS * 8 + MAX_MAP_SEGMENTS * std::mem::size_of::<Attack>())?;
    let channel = request.input.reference_channel();
    for (index, event) in attacks.iter().enumerate() {
        let stop = attacks
            .get(index + 1)
            .map_or(request.output_frames as u64, |event| event.output)
            .min(request.output_frames as u64);
        let source_end = attacks.get(index + 1).map_or_else(
            || ((event.source.div_euclid(region) + 1) * region).min(end),
            |event| event.source,
        );
        let length = (source_end - event.source).max(1) as f64;
        let midpoint = event.source + (length * 0.5) as i128;
        let search = (request.input.sample_rate / 200).min((length * 0.25) as u32) as i128;
        let mut tail_start = midpoint;
        let mut smallest = f64::INFINITY;
        for position in midpoint - search..=midpoint + search {
            let a = request.input.frame(position)[channel];
            let b = request.input.frame(position + 1)[channel];
            let score = if a.signum() != b.signum() {
                a.abs() + b.abs()
            } else {
                a.abs() + b.abs() + 1e6
            };
            if score < smallest {
                smallest = score;
                tail_start = position;
            }
        }
        work.check((search as u64 * 2 + 1).saturating_mul(2))?;
        let tail_begin = (tail_start - event.source).max(0) as f64;
        let tail_length = (length - tail_begin).max(1.0);
        // Equal-gain overlap preserves DC and stereo balance. The two reads
        // stay in the middle/tail; the attack is never a sustain-loop source.
        // Limit overlap for short segments, and quote both reconstruction reads.
        let overlap =
            (f64::from(request.input.sample_rate) * 0.002 * pitch).min(tail_length * 0.25);
        let duration = stop - event.output;
        let native_duration = length / pitch;
        let changed_duration = (duration as f64 - native_duration).abs() > 1e-6;
        let sounding_end = if tail == Tail::Off {
            duration.min(native_duration.ceil() as u64)
        } else {
            duration
        };
        let after_off_gap = tail == Tail::Off
            && index > 0
            && attacks.get(index - 1).is_some_and(|previous| {
                let native = ((event.source - previous.source) as f64 / pitch).ceil() as u64;
                event.output - previous.output > native
            });
        let onset = if after_off_gap {
            (u64::from(request.input.sample_rate) / 500).min(sounding_end)
        } else {
            0
        };
        let baseline = if onset >= 2 {
            off_restart_baseline(
                request.input,
                &bank,
                attacks[index - 1].source,
                event.source,
                pitch,
                work,
            )?
        } else {
            [0.0; 2]
        };
        // A sustained segment joins the NEXT ORIGINAL pre-attack trajectory,
        // not zero. Finish before its attack so the attack itself is neither
        // faded nor replayed. Off with an actual silence gap and authored
        // shortening remain separate from this unshortened sustain repair.
        let join = if changed_duration
            && decay == 100
            && attacks.get(index + 1).is_some()
            && (tail != Tail::Off || duration as f64 <= native_duration)
        {
            (u64::from(request.input.sample_rate) / 500).min(duration / 4)
        } else {
            0
        };
        // A short safety ramp at an actual cut/stop is distinct from the
        // authored shortening envelope. Do not alter exact unity playback.
        let release = if changed_duration && join == 0 {
            (u64::from(request.input.sample_rate) / 1_000).min(sounding_end / 4)
        } else {
            0
        };
        let sample_at = |position: f64| {
            bank.sample_segment(
                request.input,
                event.source as f64 + position,
                event.source,
                event.source + length as i128 - 1,
            )
        };
        for frame in event.output..stop {
            if frame % 1_024 == 0 {
                work.check(1_024 * 64)?;
            }
            let age = (frame - event.output) as f64;
            let mut position = age * pitch;
            if tail == Tail::Off && position >= length {
                continue;
            }
            let mut sample = if tail == Tail::Forward
                && duration as f64 > native_duration
                && position >= length - overlap
            {
                let phase = (position - (length - overlap)).rem_euclid(tail_length - overlap);
                if phase < overlap {
                    // The regular block charge covers one reconstruction.
                    // Charge the second only where an overlap really reads it;
                    // Off and single-read spans retain their existing budget.
                    work.check(64)?;
                    let outgoing = sample_at(length - overlap + phase);
                    let incoming = sample_at(tail_begin + phase);
                    let x = phase / overlap;
                    let weight = x * x * (3.0 - 2.0 * x);
                    std::array::from_fn(|channel| {
                        outgoing[channel] * (1.0 - weight) + incoming[channel] * weight
                    })
                } else {
                    sample_at(tail_begin + phase)
                }
            } else {
                if tail == Tail::Alternating && position > length - 1.0 {
                    let span = (length - 1.0 - tail_begin).max(0.0);
                    position = if span > 0.0 {
                        let phase = (position - (length - 1.0)).rem_euclid(2.0 * span);
                        if phase < span {
                            length - 1.0 - phase
                        } else {
                            tail_begin + phase - span
                        }
                    } else {
                        length - 1.0
                    };
                }
                sample_at(position)
            };
            let envelope = (-12.0 * (100 - decay) as f64 / 100.0 * age
                / (stop - event.output).max(1) as f64)
                .exp();
            let remaining = sounding_end.saturating_sub(frame - event.output + 1);
            let safety = if release > 0 && remaining < release {
                let x = remaining as f64 / release as f64;
                x * x * (3.0 - 2.0 * x)
            } else {
                1.0
            };
            let restart_weight = if onset >= 2 && age < onset as f64 {
                let x = age / (onset - 1) as f64;
                1.0 - x * x * x * (10.0 + x * (-15.0 + 6.0 * x))
            } else {
                0.0
            };
            for (c, channel) in sample.iter_mut().enumerate() {
                *channel = (*channel - baseline[c] * restart_weight) * envelope * safety;
            }
            let until_next = stop - frame;
            if join > 0 && until_next <= join {
                work.check(64)?;
                // Clamp the reconstruction to the outgoing source interval:
                // no following attack may leak through the sinc filter.
                let incoming = bank.sample_segment(
                    request.input,
                    source_end as f64 - until_next as f64 * pitch,
                    event.source,
                    source_end - 1,
                );
                let x = (join - until_next + 1) as f64 / join as f64;
                let weight = x * x * x * (10.0 + x * (-15.0 + 6.0 * x));
                sample = std::array::from_fn(|channel| {
                    sample[channel] * (1.0 - weight) + incoming[channel] * weight
                });
            }
            if let Some(destination) = output.get_mut(frame as usize) {
                *destination = finite(sample)?;
            }
        }
    }
    Ok(output)
}

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
    let mut accumulator = Accumulator::new(
        request.output_frames,
        half_limit * 2 + hop + 4,
        work,
        BANK_BYTES + 202 * 8,
    )?;
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
            accumulator.add(frame, bank.sample_float(request.input, position), window)?;
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
            };
            let measured = period(input, 2_000.0, input.reference_channel(), &mut work)
                .unwrap()
                .unwrap();
            assert!((16_000.0 / measured - frequency).abs() < frequency * 0.01);
        }
    }
}
