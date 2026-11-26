//! Linked stereo phase propagation, identity peak locking and real spectral
//! envelope reimposition for formant-preserving transposition.
//!
//! Owns the shared STFT phase-vocoder core (`SpectralVocoder`) used by both
//! resident [`render`] and bounded sequential [`crate::SequentialSpectral`],
//! guaranteeing bit-identical synthesis across both paths.
use super::{
    fft::{self, Complex},
    resample::{Bandlimited, bank_bytes},
    *,
};

pub(crate) fn window_size(sample_rate: u32) -> usize {
    ((sample_rate as usize * 8 / 100).next_power_of_two()).clamp(512, 8_192)
}

fn wrap(phase: f64) -> f64 {
    (phase + std::f64::consts::PI).rem_euclid(2.0 * std::f64::consts::PI) - std::f64::consts::PI
}

fn spectral_envelope(magnitudes: &[f64], resolution: u16, prefix: &mut [f64], output: &mut [f64]) {
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

/// Shared bounded STFT phase-vocoder buffers and per-hop synthesis core.
pub(crate) struct SpectralVocoder {
    pub(crate) n: usize,
    pub(crate) half: usize,
    pub(crate) bins: usize,
    pub(crate) hop: usize,
    pitch: f64,
    pro: Option<(f64, u16)>,
    pub(crate) window: Vec<f64>,
    pub(crate) analysis: [Vec<Complex>; 2],
    pub(crate) synthesis: [Vec<Complex>; 2],
    pub(crate) previous_phase: Vec<f64>,
    pub(crate) propagated: Vec<f64>,
    reference_phase: Vec<f64>,
    pub(crate) previous_magnitude: Vec<f64>,
    magnitude: Vec<f64>,
    envelope_buf: Vec<f64>,
    prefix: Vec<f64>,
    reference_channel: Vec<u8>,
    pub(crate) previous_channel: Vec<u8>,
    peaks: Vec<usize>,
}

impl SpectralVocoder {
    pub(crate) fn new(
        sample_rate: u32,
        pitch: f64,
        pro: Option<(f64, u16)>,
    ) -> Result<Self, Error> {
        let n = window_size(sample_rate);
        let half = n / 2;
        let bins = half + 1;
        let hop = n / 4;
        let mut window = buffer(n, 0.0_f64)?;
        for (index, value) in window.iter_mut().enumerate() {
            *value = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * index as f64 / (n - 1) as f64).cos();
        }
        let mut peaks = Vec::new();
        peaks
            .try_reserve_exact(bins)
            .map_err(|_| Error::Allocation)?;
        Ok(Self {
            n,
            half,
            bins,
            hop,
            pitch,
            pro,
            window,
            analysis: [
                buffer(n, Complex::default())?,
                buffer(n, Complex::default())?,
            ],
            synthesis: [
                buffer(n, Complex::default())?,
                buffer(n, Complex::default())?,
            ],
            previous_phase: buffer(bins, 0.0_f64)?,
            propagated: buffer(bins, 0.0_f64)?,
            reference_phase: buffer(bins, 0.0_f64)?,
            previous_magnitude: buffer(bins, 0.0_f64)?,
            magnitude: buffer(bins, 0.0_f64)?,
            envelope_buf: buffer(bins, 0.0_f64)?,
            prefix: buffer(bins + 1, 0.0_f64)?,
            reference_channel: buffer(bins, 0_u8)?,
            previous_channel: buffer(bins, 0_u8)?,
            peaks,
        })
    }

    pub(crate) fn backing_bytes(&self) -> Option<usize> {
        let add_vec = |acc: usize, cap: usize, elem: usize| -> Option<usize> {
            acc.checked_add(cap.checked_mul(elem)?)
        };
        let mut total = add_vec(0, self.window.capacity(), std::mem::size_of::<f64>())?;
        for buf in &self.analysis {
            total = add_vec(total, buf.capacity(), std::mem::size_of::<Complex>())?;
        }
        for buf in &self.synthesis {
            total = add_vec(total, buf.capacity(), std::mem::size_of::<Complex>())?;
        }
        for buf in [
            &self.previous_phase,
            &self.propagated,
            &self.reference_phase,
            &self.previous_magnitude,
            &self.magnitude,
            &self.envelope_buf,
            &self.prefix,
        ] {
            total = add_vec(total, buf.capacity(), std::mem::size_of::<f64>())?;
        }
        total = add_vec(
            total,
            self.reference_channel.capacity(),
            std::mem::size_of::<u8>(),
        )?;
        total = add_vec(
            total,
            self.previous_channel.capacity(),
            std::mem::size_of::<u8>(),
        )?;
        total = add_vec(total, self.peaks.capacity(), std::mem::size_of::<usize>())?;
        Some(total)
    }

    /// Analyze windowed input already staged in `self.analysis` and produce time-domain
    /// synthesis frames in `self.synthesis`.
    pub(crate) fn synthesize_hop(
        &mut self,
        source_center: f64,
        previous_center: Option<f64>,
        mut cancel: Option<&mut dyn FnMut() -> bool>,
    ) -> Result<(), Error> {
        for channel in &mut self.analysis {
            fft::transform_real_forward(channel)?;
        }
        if let Some(check) = cancel.as_deref_mut()
            && check()
        {
            return Err(Error::Cancelled);
        }
        let mut increase = 0.0;
        let mut previous_total = 0.0;
        for bin in 0..self.bins {
            let left = self.analysis[0][bin].magnitude();
            let right = self.analysis[1][bin].magnitude();
            self.magnitude[bin] = left.hypot(right);
            let ref_ch = usize::from(right > left);
            self.reference_channel[bin] = ref_ch as u8;
            self.reference_phase[bin] = self.analysis[ref_ch][bin].phase();
            increase += (self.magnitude[bin] - self.previous_magnitude[bin]).max(0.0);
            previous_total += self.previous_magnitude[bin];
        }
        let transient = previous_total > 1e-9 && increase / previous_total > 0.6;
        let analysis_hop = previous_center
            .map_or(self.hop as f64, |old| source_center - old)
            .max(1e-9);
        for bin in 0..self.bins {
            let bin_frequency = 2.0 * std::f64::consts::PI * bin as f64 / self.n as f64;
            if previous_center.is_none()
                || transient
                || self.previous_channel[bin] != self.reference_channel[bin]
            {
                self.propagated[bin] = self.reference_phase[bin];
            } else {
                let residual = wrap(
                    self.reference_phase[bin]
                        - self.previous_phase[bin]
                        - bin_frequency * analysis_hop,
                );
                let frequency = bin_frequency + residual / analysis_hop;
                self.propagated[bin] =
                    wrap(self.propagated[bin] + frequency * self.pitch * self.hop as f64);
            }
        }
        self.peaks.clear();
        for bin in 1..self.half {
            if self.magnitude[bin] >= self.magnitude[bin - 1]
                && self.magnitude[bin] > self.magnitude[bin + 1]
                && self.magnitude[bin] > 1e-12
            {
                self.peaks.push(bin);
            }
        }
        if self.peaks.is_empty() {
            self.peaks.push(0);
        }
        if let Some((strength, resolution)) = self.pro
            && strength > 0.0
            && self.pitch != 1.0
        {
            spectral_envelope(
                &self.magnitude,
                resolution,
                &mut self.prefix,
                &mut self.envelope_buf,
            );
        }
        for channel in &mut self.synthesis {
            channel.fill(Complex::default());
        }
        let mut peak_index = 0;
        // Reuse the now-unneeded envelope prefix buffer for locked source phases.
        for bin in 0..self.bins {
            while peak_index + 1 < self.peaks.len()
                && bin.abs_diff(self.peaks[peak_index + 1]) < bin.abs_diff(self.peaks[peak_index])
            {
                peak_index += 1;
            }
            let peak = self.peaks[peak_index];
            self.prefix[bin] = self.propagated[peak]
                + wrap(self.reference_phase[bin] - self.reference_phase[peak]);
        }
        // Gather at fractional source-bin positions. Scattering to integer bins
        // leaves spectral holes during upward transposition and biases off-bin tones.
        for target in 0..self.bins {
            let source = target as f64 / self.pitch;
            if source > self.half as f64 {
                continue;
            }
            let lower = source.floor() as usize;
            let upper = (lower + 1).min(self.half);
            let fraction = source - lower as f64;
            let locked =
                self.prefix[lower] + wrap(self.prefix[upper] - self.prefix[lower]) * fraction;
            let correction = if let Some((strength, _)) = self.pro
                && strength > 0.0
                && self.pitch != 1.0
            {
                (strength * (self.envelope_buf[target] - interpolated(&self.envelope_buf, source)))
                    .clamp(-2.079_441_541_679_835_7, 2.079_441_541_679_835_7)
                    .exp()
            } else {
                1.0
            };
            for channel in 0..2 {
                let low_magnitude = self.analysis[channel][lower].magnitude();
                let high_magnitude = self.analysis[channel][upper].magnitude();
                let magnitude =
                    (low_magnitude + (high_magnitude - low_magnitude) * fraction) * correction;
                let low_delta =
                    wrap(self.analysis[channel][lower].phase() - self.reference_phase[lower]);
                let high_delta =
                    wrap(self.analysis[channel][upper].phase() - self.reference_phase[upper]);
                let interchannel = low_delta + wrap(high_delta - low_delta) * fraction;
                self.synthesis[channel][target] = Complex::polar(magnitude, locked + interchannel);
            }
        }
        for channel in &mut self.synthesis {
            channel[0].im = 0.0;
            channel[self.half].im = 0.0;
            fft::transform_real_inverse(channel)?;
        }
        if let Some(check) = cancel
            && check()
        {
            return Err(Error::Cancelled);
        }
        Ok(())
    }

    /// Commit current hop's reference phase, channel and magnitude history.
    pub(crate) fn commit_hop(&mut self) {
        self.previous_phase.copy_from_slice(&self.reference_phase);
        self.previous_channel
            .copy_from_slice(&self.reference_channel);
        self.previous_magnitude.copy_from_slice(&self.magnitude);
    }
}

pub(super) fn render(
    request: &Request<'_>,
    pitch: f64,
    work: &mut Work<'_>,
) -> Result<Vec<Frame>, Error> {
    let n = window_size(request.input.sample_rate);
    let half = n / 2;
    let hop = n / 4;
    let bytes = n
        .checked_mul(112)
        .and_then(|n| n.checked_add(bank_bytes(work.resampler)))
        .ok_or(Error::ScratchLimit)?;
    let mut accumulator = Accumulator::new(request.output_frames, n * 2 + hop + 4, work, bytes)?;
    let bank = Bandlimited::new(1.0, work)?;
    let pro = if let Mode::ComplexPro { formants, envelope } = request.mode {
        Some((f64::from(formants) / 100.0, envelope))
    } else {
        None
    };
    let mut vocoder = SpectralVocoder::new(request.input.sample_rate, pitch, pro)?;
    let mut center = -(half as i128);
    let mut previous_center = None;
    while center < (request.output_frames + half) as i128 {
        work.check((n * n.ilog2() as usize * 4 + n * 64) as u64)?;
        let source_center = request.map.position(center)?;
        for (index, weight) in vocoder.window.iter().copied().enumerate() {
            let position = source_center + index as f64 - half as f64;
            let sample = bank.sample_float(request.input, position);
            for (channel, value) in sample.iter().copied().enumerate() {
                vocoder.analysis[channel][index] = Complex::new(value * weight, 0.0);
            }
        }
        vocoder.synthesize_hop(source_center, previous_center, None)?;
        let first = center - half as i128;
        for (index, weight) in vocoder.window.iter().copied().enumerate() {
            accumulator.add(
                first + index as i128,
                [
                    vocoder.synthesis[0][index].re / weight.max(1e-12),
                    vocoder.synthesis[1][index].re / weight.max(1e-12),
                ],
                weight * weight,
            )?;
        }
        vocoder.commit_hop();
        previous_center = Some(source_center);
        center += hop as i128;
        accumulator.finish_until((center - half as i128).max(0) as usize)?;
    }
    accumulator.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SliceSource<'a> {
        frames: &'a [Frame],
    }

    impl GranularSource for SliceSource<'_> {
        type Error = Error;

        fn window(&mut self, start: i128, output: &mut [[f64; 2]]) -> Result<(), Self::Error> {
            for (offset, slot) in output.iter_mut().enumerate() {
                *slot = self.frame(start + offset as i128)?;
            }
            Ok(())
        }

        fn frame(&mut self, index: i128) -> Result<[f64; 2], Self::Error> {
            let sample = if index < 0 || index >= self.frames.len() as i128 {
                [0.0; 2]
            } else {
                let frame = self.frames[index as usize];
                [f64::from(frame[0]), f64::from(frame[1])]
            };
            Ok(sample)
        }
    }

    fn verify_bit_identical(mode: Mode, pitch_cents: i16) {
        let sample_rate = 16_000;
        let len = 4_096usize;
        let frames: Vec<Frame> = (0..len)
            .map(|i| {
                let t = i as f64 / f64::from(sample_rate);
                let l = (2.0 * std::f64::consts::PI * 220.0 * t).sin() * 0.5
                    + (2.0 * std::f64::consts::PI * 660.0 * t).sin() * 0.25;
                let r = (2.0 * std::f64::consts::PI * 330.0 * t).cos() * 0.45
                    + (2.0 * std::f64::consts::PI * 880.0 * t).sin() * 0.2;
                [l as f32, r as f32]
            })
            .collect();
        let output_frames = 5_120usize;
        let map = TimeMap::new(
            vec![MapSegment {
                output_start: 0,
                output_end: output_frames as u64,
                source_start_q32: 0,
                source_end_q32: i128::from(len as u64) * Q32,
            }],
            output_frames,
        )
        .unwrap();
        let input = Input {
            frames: &frames,
            sample_rate,
            region_start: 0,
            region_length: len as u64,
            phase: 0,
            looping: false,
            envelope: &[],
        };
        let request = Request {
            attacks: &[],
            input,
            map: &map,
            output_frames,
            mode,
            pitch_cents,
        };
        let mut never_cancel = || false;
        let resident =
            crate::render_with_resampler(&request, Resampler::V2, &mut never_cancel).unwrap();

        let mut seq = SequentialSpectral::new(
            SequentialSpectralConfig {
                map,
                sample_rate,
                mode,
                pitch_cents,
                resampler: Resampler::V2,
            },
            &mut never_cancel,
        )
        .unwrap();
        let mut source = SliceSource { frames: &frames };
        let mut scratch = vec![[0.0_f64; 2]; seq.max_source_frames()];
        let mut sequential = Vec::with_capacity(output_frames);
        while !seq.is_complete() {
            let _ = seq
                .advance(&mut source, &mut scratch, &mut never_cancel)
                .unwrap();
            let avail = seq.available();
            if avail > 0 {
                let start = sequential.len();
                sequential.resize(start + avail, [0.0; 2]);
                seq.drain(&mut sequential[start..]).unwrap();
            }
        }
        assert_eq!(resident.frames.len(), sequential.len());
        for (idx, (a, b)) in resident.frames.iter().zip(&sequential).enumerate() {
            assert_eq!(
                [a[0].to_bits(), a[1].to_bits()],
                [b[0].to_bits(), b[1].to_bits()],
                "sample mismatch at frame {idx} for {mode:?}"
            );
        }
    }

    #[test]
    fn complex_resident_and_sequential_outputs_are_bit_identical() {
        verify_bit_identical(Mode::Complex, 300);
    }

    #[test]
    fn complex_pro_resident_and_sequential_outputs_are_bit_identical() {
        verify_bit_identical(
            Mode::ComplexPro {
                formants: 80,
                envelope: 128,
            },
            -400,
        );
    }
}
