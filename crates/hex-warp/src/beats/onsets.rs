//! Streaming stereo evidence, with no spectrogram or PCM side buffer.
use super::*;
use crate::fft::{self, Complex};

fn fft_length(rate: u32) -> usize {
    ((rate as usize * 16 / 1000).next_power_of_two()).clamp(128, 4096)
}
pub(super) fn scratch(rate: u32) -> usize {
    let n = fft_length(rate);
    // One FFT/window and two generations of actual stereo complex bins.
    n * (std::mem::size_of::<Complex>() + 8) + (n / 2 + 1) * std::mem::size_of::<[Complex; 2]>() * 2
}
fn insert(starts: &mut Vec<u64>, at: u64, separation: u64) -> Result<(), Error> {
    let index = starts.partition_point(|x| *x < at);
    if index > 0 && at - starts[index - 1] < separation
        || index < starts.len() && starts[index] - at < separation
    {
        return Ok(());
    }
    if starts.len() == MAX_POINTS {
        return Err(Error::Invalid("transients exceed bounded source plan"));
    }
    // The caller reserved MAX_POINTS before any analysis owner was acquired.
    starts.insert(index, at);
    Ok(())
}
fn innovation(input: Input<'_>, at: u64) -> f64 {
    let now = input.frame(i128::from(at));
    let previous = input.frame(i128::from(at) - 1);
    (0..2).map(|c| (now[c] - previous[c]).powi(2)).sum()
}
fn refine(
    input: Input<'_>,
    center: u64,
    half: u64,
    work: &mut Work<'_>,
) -> Result<Option<u64>, Error> {
    let start = center.saturating_sub(half);
    let end = center.saturating_add(half).min(input.region_length);
    let block = u64::from((input.sample_rate / 1000).max(1));
    let mut maximum = 0.0_f64;
    let mut sum = 0.0;
    for at in start..end {
        if (at - start) % block == 0 {
            work.check(block * 16)?;
        }
        let x = innovation(input, at);
        maximum = maximum.max(x);
        sum += x;
    }
    // A changing FFT leakage pattern is not a physical attack. In particular
    // steady sinusoidal innovation has peak/mean <=2. Require sample-level
    // concentration before turning spectral novelty into an onset anchor.
    let floor = sum / (end - start).max(1) as f64 * 4.0;
    if maximum <= floor.max(1e-14) {
        return Ok(None);
    }
    let threshold = (maximum * 0.10).max(floor).max(1e-14);
    // Earliest substantial change, not the frame center or a later peak in a
    // noisy attack. Both channels share this one physical onset decision.
    for at in start..end {
        if (at - start) % block == 0 {
            work.check(block * 16)?;
        }
        if innovation(input, at) >= threshold {
            return Ok(Some(at));
        }
    }
    Ok(None)
}

/// A region wrap is not itself an onset. Compare its innovation against the
/// same cyclic neighborhood without privileging either stereo channel.
pub(super) fn cycle_boundary(input: Input<'_>, work: &mut Work<'_>) -> Result<bool, Error> {
    if !input.looping {
        return Ok(true);
    }
    let count = i128::from((input.sample_rate / 500).max(1));
    work.check(count as u64 * 32)?;
    let difference = |at: i128| {
        let x = input.frame(at);
        let y = input.frame(at - 1);
        (0..2).map(|c| (x[c] - y[c]).powi(2)).sum::<f64>()
    };
    let seam = difference(0);
    let mut neighborhood = 0.0;
    for at in -count..=count {
        if at != 0 {
            neighborhood += difference(at);
        }
    }
    Ok(seam > 1e-12 && seam > neighborhood / (count * 2) as f64 * 8.0)
}

pub(super) fn detect(
    input: Input<'_>,
    starts: &mut Vec<u64>,
    work: &mut Work<'_>,
) -> Result<(), Error> {
    let n = fft_length(input.sample_rate);
    let half = n / 2;
    let bins = half + 1;
    let hop = n / 4;
    let mut fft = buffer(n, Complex::default())?;
    let mut previous = buffer(bins, [Complex::default(); 2])?;
    let mut older = buffer(bins, [Complex::default(); 2])?;
    let mut window = buffer(n, 0.0_f64)?;
    for (i, value) in window.iter_mut().enumerate() {
        *value = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / (n - 1) as f64).cos();
    }
    let mut mean = [0.0_f64; 4];
    let mut deviation = [0.0_f64; 4];
    let mut last_score = 0.0;
    let mut before_score = 0.0;
    let mut center = 0_u64;
    let mut count = 0;
    while center < input.region_length {
        work.check((n * (n.ilog2() as usize * 12 + 16) + bins * 40) as u64)?;
        let mut increase = [0.0; 4];
        let mut magnitude = [0.0; 4];
        let mut old_magnitude = [0.0; 4];
        let mut phase_error = [0.0; 4];
        for channel in 0..2 {
            for (i, value) in fft.iter_mut().enumerate() {
                let at = i128::from(center) + i as i128 - half as i128;
                // Region edges are separate from transient evidence. Never
                // inspect unrelated media, including through a loop wrap here.
                let x = if at >= 0 && at < i128::from(input.region_length) {
                    input.frame(at)[channel]
                } else {
                    0.0
                };
                *value = Complex::new(x * window[i], 0.0);
            }
            fft::transform(&mut fft, false)?;
            for bin in 0..bins {
                let hz = bin as f64 * f64::from(input.sample_rate) / n as f64;
                let band = if hz < 200.0 {
                    0
                } else if hz < 2000.0 {
                    1
                } else if hz < 6000.0 {
                    2
                } else {
                    3
                };
                let now = fft[bin];
                let old = previous[bin][channel];
                let oldest = older[bin][channel];
                let m = now.magnitude();
                let p = old.magnitude();
                increase[band] += (m - p).max(0.0);
                magnitude[band] += m;
                old_magnitude[band] += p;
                let denominator = p * oldest.magnitude();
                if denominator > 1e-20 {
                    let predicted = old.mul(old).mul(Complex::new(oldest.re, -oldest.im));
                    phase_error[band] += (now.re - predicted.re / denominator)
                        .hypot(now.im - predicted.im / denominator);
                }
                older[bin][channel] = old;
                previous[bin][channel] = now;
            }
        }
        let mut score = 0.0_f64;
        for band in 0..4 {
            let total = magnitude[band] + old_magnitude[band];
            let flux = if total > 1e-7 * n as f64 {
                increase[band] / total * (0.75 + 0.25 * (phase_error[band] / total).min(1.0))
            } else {
                0.0
            };
            let threshold = (0.15 + mean[band] * 1.5 + deviation[band] * 3.0).min(0.70);
            score = score.max(flux - threshold);
            let delta = flux - mean[band];
            mean[band] += 0.05 * delta;
            deviation[band] += 0.05 * (delta.abs() - deviation[band]);
        }
        if count >= 3
            && last_score > 0.0
            && last_score >= before_score
            && last_score > score
            && let Some(at) = refine(input, center - hop as u64, half as u64, work)?
        {
            work.check(starts.len() as u64)?;
            insert(starts, at, u64::from((input.sample_rate / 1000).max(1)))?;
        }
        before_score = last_score;
        last_score = score;
        count += 1;
        center = center.checked_add(hop as u64).ok_or(Error::WorkLimit)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn physical(source: &[Frame], rate: u32) -> (Vec<u64>, u64) {
        let mut starts = Vec::with_capacity(MAX_POINTS);
        starts.push(0);
        let mut cancel = || false;
        let mut work = Work {
            cancel: &mut cancel,
            used: 0,
            scratch: 0,
            base_scratch: 0,
            resampler: Resampler::V1,
        };
        work.scratch(scratch(rate) + MAX_POINTS * 8).unwrap();
        detect(
            Input {
                frames: source,
                sample_rate: rate,
                region_start: 0,
                region_length: source.len() as u64,
                phase: 0,
                looping: false,
                envelope: &[],
            },
            &mut starts,
            &mut work,
        )
        .unwrap();
        (starts, work.used)
    }
    #[test]
    fn spectral_physical_onsets_detect_quiet_antiphase_percussion_above_bass_without_grid_attacks()
    {
        for rate in [16_000, 44_100, 48_000, 96_000] {
            let at = rate as usize / 4;
            let mut source: Vec<_> = (0..rate as usize / 2)
                .map(|i| {
                    let x = (std::f64::consts::TAU * 55.0 * i as f64 / f64::from(rate)).sin()
                        as f32
                        * 0.4;
                    [x, -x]
                })
                .collect();
            let (stable, _) = physical(&source, rate);
            assert_eq!(
                stable,
                [0],
                "sustained bass is not an attack: {rate} {stable:?}"
            );
            for (i, frame) in source
                .iter_mut()
                .enumerate()
                .skip(at)
                .take(rate as usize / 50)
            {
                let age = (i - at) as f64 / f64::from(rate);
                let noise = (((i as u64 * 1_664_525 + 1_013_904_223) ^ (i as u64 * 7_919)) % 65_521)
                    as f64
                    / 32_760.0
                    - 1.0;
                let hat = noise * (-250.0 * age).exp() * 0.08;
                frame[0] += hat as f32;
                frame[1] -= hat as f32;
            }
            let (attacks, work) = physical(&source, rate);
            eprintln!("spectral onset rate={rate} attacks={attacks:?} work={work}");
            assert!(
                attacks
                    .iter()
                    .any(|x| x.abs_diff(at as u64) <= u64::from(rate / 1000)),
                "quiet antiphase percussion missing {rate}: {attacks:?}"
            );
            assert!(
                attacks.len() <= 3,
                "one burst may not manufacture dense anchors"
            );
        }
    }
}
