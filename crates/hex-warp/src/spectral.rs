//! Linked stereo phase propagation, identity peak locking and real spectral
//! envelope reimposition for formant-preserving transposition.
use super::{
    fft::{self, Complex},
    resample::{Bandlimited, bank_bytes},
    *,
};
fn wrap(phase: f64) -> f64 {
    (phase + std::f64::consts::PI).rem_euclid(2.0 * std::f64::consts::PI) - std::f64::consts::PI
}
fn envelope(magnitudes: &[f64], resolution: u16, prefix: &mut [f64], output: &mut [f64]) {
    prefix[0] = 0.0;
    for (index, magnitude) in magnitudes.iter().enumerate() {
        prefix[index + 1] = prefix[index] + magnitude.max(1e-12).ln();
    }
    let radius = (magnitudes.len() / usize::from(resolution)).max(2);
    for (index, value) in output.iter_mut().enumerate() {
        let first = index.saturating_sub(radius);
        let end = (index + radius + 1).min(magnitudes.len());
        *value = (prefix[end] - prefix[first]) / (end - first) as f64;
    }
}
fn interpolated(values: &[f64], position: f64) -> f64 {
    let index = (position.floor() as usize).min(values.len() - 1);
    let next = (index + 1).min(values.len() - 1);
    let fraction = (position - index as f64).clamp(0.0, 1.0);
    values[index] + (values[next] - values[index]) * fraction
}
pub(super) fn render(
    request: &Request<'_>,
    pitch: f64,
    work: &mut Work<'_>,
) -> Result<Vec<Frame>, Error> {
    let n = ((request.input.sample_rate as usize * 8 / 100).next_power_of_two()).clamp(512, 8_192);
    let half = n / 2;
    let bins = half + 1;
    let hop = n / 4;
    let bytes = n
        .checked_mul(112)
        .and_then(|n| n.checked_add(bank_bytes(work.resampler)))
        .ok_or(Error::ScratchLimit)?;
    let mut accumulator = Accumulator::new(request.output_frames, n * 2 + hop + 4, work, bytes)?;
    let bank = Bandlimited::new(1.0, work)?;
    let mut analysis = [
        buffer(n, Complex::default())?,
        buffer(n, Complex::default())?,
    ];
    let mut synthesis = [
        buffer(n, Complex::default())?,
        buffer(n, Complex::default())?,
    ];
    let mut previous_phase = buffer(bins, 0.0_f64)?;
    let mut propagated = buffer(bins, 0.0_f64)?;
    let mut reference_phase = buffer(bins, 0.0_f64)?;
    let mut previous_magnitude = buffer(bins, 0.0_f64)?;
    let mut magnitude = buffer(bins, 0.0_f64)?;
    let mut spectral_envelope = buffer(bins, 0.0_f64)?;
    let mut prefix = buffer(bins + 1, 0.0_f64)?;
    let mut reference_channel = buffer(bins, 0_usize)?;
    let mut previous_channel = buffer(bins, 0_usize)?;
    let mut peaks = Vec::new();
    peaks
        .try_reserve_exact(bins)
        .map_err(|_| Error::Allocation)?;
    let window: Vec<f64> = {
        let mut result = buffer(n, 0.0)?;
        for (index, value) in result.iter_mut().enumerate() {
            *value = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * index as f64 / (n - 1) as f64).cos();
        }
        result
    };
    let pro = if let Mode::ComplexPro { formants, envelope } = request.mode {
        Some((f64::from(formants) / 100.0, envelope))
    } else {
        None
    };
    let mut center = -(half as i128);
    let mut previous_center = None;
    while center < (request.output_frames + half) as i128 {
        work.check((n * n.ilog2() as usize * 4 + n * 64) as u64)?;
        let source_center = request.map.position(center)?;
        for (index, weight) in window.iter().copied().enumerate() {
            let position = source_center + index as f64 - half as f64;
            let sample = bank.sample_float(request.input, position);
            for channel in 0..2 {
                analysis[channel][index] = Complex::new(sample[channel] * weight, 0.0);
            }
        }
        for channel in &mut analysis {
            fft::transform(channel, false)?;
        }
        let mut increase = 0.0;
        let mut previous_total = 0.0;
        for bin in 0..bins {
            let left = analysis[0][bin].magnitude();
            let right = analysis[1][bin].magnitude();
            magnitude[bin] = left.hypot(right);
            reference_channel[bin] = usize::from(right > left);
            reference_phase[bin] = analysis[reference_channel[bin]][bin].phase();
            increase += (magnitude[bin] - previous_magnitude[bin]).max(0.0);
            previous_total += previous_magnitude[bin];
        }
        let transient = previous_total > 1e-9 && increase / previous_total > 0.6;
        let analysis_hop = previous_center
            .map_or(hop as f64, |old: f64| source_center - old)
            .max(1e-9);
        for bin in 0..bins {
            let bin_frequency = 2.0 * std::f64::consts::PI * bin as f64 / n as f64;
            if previous_center.is_none()
                || transient
                || previous_channel[bin] != reference_channel[bin]
            {
                propagated[bin] = reference_phase[bin];
            } else {
                let residual =
                    wrap(reference_phase[bin] - previous_phase[bin] - bin_frequency * analysis_hop);
                let frequency = bin_frequency + residual / analysis_hop;
                propagated[bin] = wrap(propagated[bin] + frequency * pitch * hop as f64);
            }
        }
        peaks.clear();
        for bin in 1..half {
            if magnitude[bin] >= magnitude[bin - 1]
                && magnitude[bin] > magnitude[bin + 1]
                && magnitude[bin] > 1e-12
            {
                peaks.push(bin);
            }
        }
        if peaks.is_empty() {
            peaks.push(0);
        }
        if let Some((strength, resolution)) = pro
            && strength > 0.0
            && pitch != 1.0
        {
            envelope(&magnitude, resolution, &mut prefix, &mut spectral_envelope);
        }
        for channel in &mut synthesis {
            channel.fill(Complex::default());
        }
        let mut peak_index = 0;
        // Reuse the now-unneeded envelope prefix buffer for locked source phases.
        for bin in 0..bins {
            while peak_index + 1 < peaks.len()
                && bin.abs_diff(peaks[peak_index + 1]) < bin.abs_diff(peaks[peak_index])
            {
                peak_index += 1;
            }
            let peak = peaks[peak_index];
            prefix[bin] = propagated[peak] + wrap(reference_phase[bin] - reference_phase[peak]);
        }
        // Gather at fractional source-bin positions. Scattering to integer bins
        // leaves spectral holes during upward transposition and biases off-bin tones.
        for target in 0..bins {
            let source = target as f64 / pitch;
            if source > half as f64 {
                continue;
            }
            let lower = source.floor() as usize;
            let upper = (lower + 1).min(half);
            let fraction = source - lower as f64;
            let locked = prefix[lower] + wrap(prefix[upper] - prefix[lower]) * fraction;
            let correction = if let Some((strength, _)) = pro
                && strength > 0.0
                && pitch != 1.0
            {
                (strength * (spectral_envelope[target] - interpolated(&spectral_envelope, source)))
                    .clamp(-2.079_441_541_679_835_7, 2.079_441_541_679_835_7)
                    .exp()
            } else {
                1.0
            };
            for channel in 0..2 {
                let low_magnitude = analysis[channel][lower].magnitude();
                let high_magnitude = analysis[channel][upper].magnitude();
                let magnitude =
                    (low_magnitude + (high_magnitude - low_magnitude) * fraction) * correction;
                let low_delta = wrap(analysis[channel][lower].phase() - reference_phase[lower]);
                let high_delta = wrap(analysis[channel][upper].phase() - reference_phase[upper]);
                let interchannel = low_delta + wrap(high_delta - low_delta) * fraction;
                synthesis[channel][target] = Complex::polar(magnitude, locked + interchannel);
            }
        }
        for channel in &mut synthesis {
            channel[0].im = 0.0;
            channel[half].im = 0.0;
            for bin in 1..half {
                channel[n - bin] = Complex::new(channel[bin].re, -channel[bin].im);
            }
            fft::transform(channel, true)?;
        }
        let first = center - half as i128;
        for (index, weight) in window.iter().copied().enumerate() {
            accumulator.add(
                first + index as i128,
                [
                    synthesis[0][index].re / weight.max(1e-12),
                    synthesis[1][index].re / weight.max(1e-12),
                ],
                weight * weight,
            )?;
        }
        previous_phase.copy_from_slice(&reference_phase);
        previous_channel.copy_from_slice(&reference_channel);
        previous_magnitude.copy_from_slice(&magnitude);
        previous_center = Some(source_center);
        center += hop as i128;
        accumulator.finish_until((center - half as i128).max(0) as usize)?;
    }
    accumulator.finish()
}
