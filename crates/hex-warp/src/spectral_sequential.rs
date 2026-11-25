//! Sequential control-side scheduling for bounded Complex and ComplexPro spectral warp.
//!
//! Each `advance` step synthesizes one STFT hop over a caller-supplied source window
//! using the exact same linked stereo phase propagation, identity peak locking, and
//! spectral-envelope formant reimposition as the resident `spectral` renderer.
//! Checkpoints capture only the bounded overlap ring and per-bin phase/magnitude/channel
//! history, independent of total output duration.
use crate::{
    Error, Frame, GranularOverlap, GranularSource, MAX_SCRATCH_BYTES, MAX_STREAMED_OUTPUT_FRAMES,
    MAX_WORK_UNITS, Mode, Q32, Resampler, TimeMap, V2_MAX_TAPS, Work, buffer,
    fft::{self, Complex},
    granular_overlap::OverlapCheckpoint,
    granular_sequential::next_renderer_id,
    resample::Bandlimited,
};

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

/// Instance-bound snapshot of spectral phase-vocoder and overlap state.
///
/// Checkpoints own no source or filter; callers own total checkpoint admission,
/// eviction and generation validation, and must keep them off the audio callback.
pub struct SpectralCheckpoint {
    owner: u64,
    overlap: OverlapCheckpoint,
    center: i128,
    previous_center: Option<f64>,
    ready_end: u64,
    previous_phase: Vec<f64>,
    propagated: Vec<f64>,
    previous_magnitude: Vec<f64>,
    previous_channel: Vec<u8>,
}

impl SpectralCheckpoint {
    /// Output cursor captured with this checkpoint.
    pub fn position(&self) -> u64 {
        self.overlap.position()
    }

    /// Retained heap allocations, excluding this inline checkpoint value.
    pub fn backing_bytes(&self) -> Option<usize> {
        let overlap = self.overlap.backing_bytes()?;
        let phases = self
            .previous_phase
            .capacity()
            .checked_mul(std::mem::size_of::<f64>())?;
        let prop = self
            .propagated
            .capacity()
            .checked_mul(std::mem::size_of::<f64>())?;
        let mags = self
            .previous_magnitude
            .capacity()
            .checked_mul(std::mem::size_of::<f64>())?;
        let chans = self
            .previous_channel
            .capacity()
            .checked_mul(std::mem::size_of::<u8>())?;
        overlap
            .checked_add(phases)?
            .checked_add(prop)?
            .checked_add(mags)?
            .checked_add(chans)
    }
}

/// Parameters for a sequential Complex or ComplexPro spectral render.
pub struct SequentialSpectralConfig {
    pub map: TimeMap,
    pub sample_rate: u32,
    pub mode: Mode,
    pub pitch_cents: i16,
    pub resampler: Resampler,
}

/// Bounded control-side STFT phase-vocoder scheduler and overlap owner.
///
/// Each `advance` handles at most one STFT hop. Drain every completed frame
/// before advancing again. The caller supplies stable source data and scratch;
/// neither is retained here. Do not run on an audio callback.
pub struct SequentialSpectral {
    config: SequentialSpectralConfig,
    owner: u64,
    bank: Bandlimited,
    overlap: GranularOverlap,
    total: u64,
    n: usize,
    half: usize,
    bins: usize,
    hop: usize,
    pitch: f64,
    pro: Option<(f64, u16)>,
    window: Vec<f64>,
    analysis: [Vec<Complex>; 2],
    synthesis: [Vec<Complex>; 2],
    previous_phase: Vec<f64>,
    propagated: Vec<f64>,
    reference_phase: Vec<f64>,
    previous_magnitude: Vec<f64>,
    magnitude: Vec<f64>,
    envelope_buf: Vec<f64>,
    prefix: Vec<f64>,
    reference_channel: Vec<u8>,
    previous_channel: Vec<u8>,
    peaks: Vec<usize>,
    center: i128,
    previous_center: Option<f64>,
    ready_end: u64,
    max_source_frames: usize,
    poisoned: bool,
}

impl SequentialSpectral {
    /// Validate configuration and allocate only bounded STFT, filter, and overlap state.
    pub fn new(
        config: SequentialSpectralConfig,
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<Self, Error> {
        let pro = match config.mode {
            Mode::Complex => None,
            Mode::ComplexPro { formants, envelope }
                if formants <= 100 && (32..=512).contains(&envelope) =>
            {
                Some((f64::from(formants) / 100.0, envelope))
            }
            _ => return Err(Error::Invalid("invalid sequential spectral mode")),
        };
        if !(8_000..=192_000).contains(&config.sample_rate) {
            return Err(Error::Invalid("invalid sequential spectral sample rate"));
        }
        config
            .map
            .validate_stretch(config.mode, config.pitch_cents)?;
        let total = config
            .map
            .segments()
            .last()
            .ok_or(Error::Invalid("empty spectral map"))?
            .output_end;
        if total == 0 || total > MAX_STREAMED_OUTPUT_FRAMES {
            return Err(Error::Invalid("invalid sequential spectral extent"));
        }
        let pitch = 2.0_f64.powf(f64::from(config.pitch_cents) / 1_200.0);
        let n = ((config.sample_rate as usize * 8 / 100).next_power_of_two()).clamp(512, 8_192);
        let half = n / 2;
        let bins = half + 1;
        let hop = n / 4;
        let ring = n * 2 + hop + 4;
        let max_source_frames = n + V2_MAX_TAPS + 4;
        let owner = next_renderer_id()?;
        let mut work = Work {
            cancel,
            used: 0,
            limit: MAX_WORK_UNITS,
            scratch: 0,
            base_scratch: 0,
            resampler: config.resampler,
        };
        work.check(0)?;
        let bank = Bandlimited::new(1.0, &mut work)?;
        let overlap = GranularOverlap::new(total, ring)?;
        let mut window = buffer(n, 0.0_f64)?;
        for (index, value) in window.iter_mut().enumerate() {
            *value = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * index as f64 / (n - 1) as f64).cos();
        }
        let mut peaks = Vec::new();
        peaks
            .try_reserve_exact(bins)
            .map_err(|_| Error::Allocation)?;
        let result = Self {
            config,
            owner,
            bank,
            overlap,
            total,
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
            center: -(half as i128),
            previous_center: None,
            ready_end: 0,
            max_source_frames,
            poisoned: false,
        };
        let aggregate = result
            .backing_bytes()
            .and_then(|bytes| {
                max_source_frames
                    .checked_mul(std::mem::size_of::<[f64; 2]>())
                    .and_then(|scratch| bytes.checked_add(scratch))
            })
            .ok_or(Error::ScratchLimit)?;
        if aggregate > MAX_SCRATCH_BYTES {
            return Err(Error::ScratchLimit);
        }
        Ok(result)
    }

    /// A fresh renderer has not analyzed any hop or committed output.
    pub fn is_pristine(&self) -> bool {
        !self.poisoned
            && self.previous_center.is_none()
            && self.center == -(self.half as i128)
            && self.position() == 0
    }

    /// Validated complete output extent in frames.
    pub fn total_frames(&self) -> u64 {
        self.total
    }

    /// Project sample rate configured on this renderer.
    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    /// Retained heap allocations, excluding this inline object.
    pub fn backing_bytes(&self) -> Option<usize> {
        let mut total = self
            .config
            .map
            .backing_bytes()?
            .checked_add(self.bank.backing_bytes()?)?
            .checked_add(self.overlap.backing_bytes()?)?;
        let add_vec = |acc: usize, cap: usize, elem: usize| -> Option<usize> {
            acc.checked_add(cap.checked_mul(elem)?)
        };
        total = add_vec(total, self.window.capacity(), std::mem::size_of::<f64>())?;
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

    /// Capture exact phase-vocoder and overlap state, including completed but undrained frames.
    pub fn checkpoint(&self, budget: usize) -> Result<SpectralCheckpoint, Error> {
        if self.poisoned {
            return Err(Error::Invalid("spectral renderer is poisoned"));
        }
        let available = MAX_SCRATCH_BYTES
            .checked_sub(self.backing_bytes().ok_or(Error::ScratchLimit)?)
            .and_then(|n| {
                self.max_source_frames
                    .checked_mul(std::mem::size_of::<[f64; 2]>())
                    .and_then(|scratch| n.checked_sub(scratch))
            })
            .ok_or(Error::ScratchLimit)?;
        let effective = budget.min(available);
        let state_bytes = self
            .bins
            .checked_mul(3 * std::mem::size_of::<f64>() + std::mem::size_of::<u8>())
            .ok_or(Error::ScratchLimit)?;
        let overlap_budget = effective
            .checked_sub(state_bytes)
            .ok_or(Error::ScratchLimit)?;
        let overlap = self.overlap.checkpoint(overlap_budget)?;
        let mut previous_phase = buffer(self.bins, 0.0_f64)?;
        let mut propagated = buffer(self.bins, 0.0_f64)?;
        let mut previous_magnitude = buffer(self.bins, 0.0_f64)?;
        let mut previous_channel = buffer(self.bins, 0_u8)?;
        previous_phase.copy_from_slice(&self.previous_phase);
        propagated.copy_from_slice(&self.propagated);
        previous_magnitude.copy_from_slice(&self.previous_magnitude);
        previous_channel.copy_from_slice(&self.previous_channel);
        let checkpoint = SpectralCheckpoint {
            owner: self.owner,
            overlap,
            center: self.center,
            previous_center: self.previous_center,
            ready_end: self.ready_end,
            previous_phase,
            propagated,
            previous_magnitude,
            previous_channel,
        };
        if checkpoint
            .backing_bytes()
            .is_none_or(|bytes| bytes > effective)
        {
            return Err(Error::ScratchLimit);
        }
        Ok(checkpoint)
    }

    /// Whether a checkpoint belongs to this exact renderer instance.
    pub fn owns_checkpoint(&self, checkpoint: &SpectralCheckpoint) -> bool {
        checkpoint.owner == self.owner
    }

    /// Replace an owned checkpoint in place without allocating new storage.
    pub fn refresh_checkpoint(&self, checkpoint: &mut SpectralCheckpoint) -> Result<(), Error> {
        if self.poisoned {
            return Err(Error::Invalid("spectral renderer is poisoned"));
        }
        if !self.owns_checkpoint(checkpoint) {
            return Err(Error::Invalid("foreign spectral checkpoint"));
        }
        if checkpoint.previous_phase.len() != self.bins
            || checkpoint.propagated.len() != self.bins
            || checkpoint.previous_magnitude.len() != self.bins
            || checkpoint.previous_channel.len() != self.bins
        {
            return Err(Error::Invalid("incompatible spectral checkpoint"));
        }
        self.overlap.refresh_checkpoint(&mut checkpoint.overlap)?;
        checkpoint.center = self.center;
        checkpoint.previous_center = self.previous_center;
        checkpoint.ready_end = self.ready_end;
        checkpoint
            .previous_phase
            .copy_from_slice(&self.previous_phase);
        checkpoint.propagated.copy_from_slice(&self.propagated);
        checkpoint
            .previous_magnitude
            .copy_from_slice(&self.previous_magnitude);
        checkpoint
            .previous_channel
            .copy_from_slice(&self.previous_channel);
        Ok(())
    }

    /// Restore a checkpoint captured by this exact renderer, clearing poison on success.
    pub fn restore(&mut self, checkpoint: &SpectralCheckpoint) -> Result<(), Error> {
        if checkpoint.owner != self.owner {
            return Err(Error::Invalid("foreign spectral checkpoint"));
        }
        if checkpoint.previous_phase.len() != self.bins
            || checkpoint.propagated.len() != self.bins
            || checkpoint.previous_magnitude.len() != self.bins
            || checkpoint.previous_channel.len() != self.bins
        {
            return Err(Error::Invalid("incompatible spectral checkpoint"));
        }
        self.overlap.restore(&checkpoint.overlap)?;
        self.center = checkpoint.center;
        self.previous_center = checkpoint.previous_center;
        self.ready_end = checkpoint.ready_end;
        self.previous_phase
            .copy_from_slice(&checkpoint.previous_phase);
        self.propagated.copy_from_slice(&checkpoint.propagated);
        self.previous_magnitude
            .copy_from_slice(&checkpoint.previous_magnitude);
        self.previous_channel
            .copy_from_slice(&checkpoint.previous_channel);
        self.poisoned = false;
        Ok(())
    }

    /// Maximum caller-owned source scratch required for any one STFT hop.
    pub fn max_source_frames(&self) -> usize {
        self.max_source_frames
    }

    /// Number of already complete frames that must be drained before advancing.
    pub fn available(&self) -> usize {
        (self.ready_end - self.overlap.position()) as usize
    }

    /// Exact number of frames successfully committed to caller output.
    pub fn position(&self) -> u64 {
        self.overlap.position()
    }

    /// A poisoned state cannot advance or drain until a valid checkpoint is restored.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// All frames have been drained.
    pub fn is_complete(&self) -> bool {
        !self.poisoned && self.position() == self.total
    }

    /// Analyze and synthesize at most one STFT hop.
    ///
    /// Source or scratch errors before overlap accumulation leave renderer state
    /// unchanged. Any error during synthesis/accumulation, including cancellation,
    /// poisons the renderer until restored from a valid checkpoint.
    pub fn advance<S: GranularSource>(
        &mut self,
        source: &mut S,
        scratch: &mut [[f64; 2]],
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<bool, S::Error> {
        if self.poisoned {
            return Err(Error::Invalid("spectral renderer is poisoned").into());
        }
        if self.available() != 0 {
            return Err(Error::Invalid("spectral output must be drained first").into());
        }
        if self.is_complete() {
            return Ok(false);
        }
        if cancel() {
            return Err(Error::Cancelled.into());
        }
        let source_center = self
            .config
            .map
            .position(self.center)
            .map_err(S::Error::from)?;
        let first_pos = source_center - self.half as f64;
        let last_pos = source_center + (self.n - 1) as f64 - self.half as f64;
        let first_int = ((first_pos * Q32 as f64).round() as i128).div_euclid(Q32);
        let last_int = ((last_pos * Q32 as f64).round() as i128).div_euclid(Q32);
        let start = first_int - (V2_MAX_TAPS as i128 / 2) - 1;
        let end = last_int + (V2_MAX_TAPS as i128 / 2) + 2;
        let count =
            usize::try_from(end - start).map_err(|_| S::Error::from(Error::ScratchLimit))?;
        if count > self.max_source_frames
            || scratch.len() < count
            || self
                .backing_bytes()
                .and_then(|bytes| {
                    scratch
                        .len()
                        .checked_mul(std::mem::size_of::<[f64; 2]>())
                        .and_then(|n| bytes.checked_add(n))
                })
                .is_none_or(|bytes| bytes > MAX_SCRATCH_BYTES)
        {
            return Err(Error::ScratchLimit.into());
        }
        let window_slice = &mut scratch[..count];
        source.window(start, window_slice)?;

        let result = (|| -> Result<(), Error> {
            if cancel() {
                return Err(Error::Cancelled);
            }
            for (index, weight) in self.window.iter().copied().enumerate() {
                if index > 0 && index % 1_024 == 0 && cancel() {
                    return Err(Error::Cancelled);
                }
                let position = source_center + index as f64 - self.half as f64;
                let mut missing = false;
                let sample = self
                    .bank
                    .sample_with((position * Q32 as f64).round() as i128, |at| {
                        match at
                            .checked_sub(start)
                            .and_then(|offset| usize::try_from(offset).ok())
                            .and_then(|i| window_slice.get(i))
                        {
                            Some(frame) => *frame,
                            None => {
                                missing = true;
                                [0.0; 2]
                            }
                        }
                    });
                if missing {
                    return Err(Error::Invalid("spectral source window missed a filter tap"));
                }
                for (channel, value) in sample.iter().copied().enumerate() {
                    self.analysis[channel][index] = Complex::new(value * weight, 0.0);
                }
            }
            for channel in &mut self.analysis {
                fft::transform(channel, false)?;
            }
            if cancel() {
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
            let analysis_hop = self
                .previous_center
                .map_or(self.hop as f64, |old| source_center - old)
                .max(1e-9);
            for bin in 0..self.bins {
                let bin_frequency = 2.0 * std::f64::consts::PI * bin as f64 / self.n as f64;
                if self.previous_center.is_none()
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
            for bin in 0..self.bins {
                while peak_index + 1 < self.peaks.len()
                    && bin.abs_diff(self.peaks[peak_index + 1])
                        < bin.abs_diff(self.peaks[peak_index])
                {
                    peak_index += 1;
                }
                let peak = self.peaks[peak_index];
                self.prefix[bin] = self.propagated[peak]
                    + wrap(self.reference_phase[bin] - self.reference_phase[peak]);
            }
            for target in 0..self.bins {
                let src = target as f64 / self.pitch;
                if src > self.half as f64 {
                    continue;
                }
                let lower = src.floor() as usize;
                let upper = (lower + 1).min(self.half);
                let fraction = src - lower as f64;
                let locked =
                    self.prefix[lower] + wrap(self.prefix[upper] - self.prefix[lower]) * fraction;
                let correction = if let Some((strength, _)) = self.pro
                    && strength > 0.0
                    && self.pitch != 1.0
                {
                    (strength * (self.envelope_buf[target] - interpolated(&self.envelope_buf, src)))
                        .clamp(-2.079_441_541_679_835_7, 2.079_441_541_679_835_7)
                        .exp()
                } else {
                    1.0
                };
                for channel in 0..2 {
                    let low_magnitude = self.analysis[channel][lower].magnitude();
                    let high_magnitude = self.analysis[channel][upper].magnitude();
                    let mag =
                        (low_magnitude + (high_magnitude - low_magnitude) * fraction) * correction;
                    let low_delta =
                        wrap(self.analysis[channel][lower].phase() - self.reference_phase[lower]);
                    let high_delta =
                        wrap(self.analysis[channel][upper].phase() - self.reference_phase[upper]);
                    let interchannel = low_delta + wrap(high_delta - low_delta) * fraction;
                    self.synthesis[channel][target] = Complex::polar(mag, locked + interchannel);
                }
            }
            for channel in &mut self.synthesis {
                channel[0].im = 0.0;
                channel[self.half].im = 0.0;
                for bin in 1..self.half {
                    channel[self.n - bin] = Complex::new(channel[bin].re, -channel[bin].im);
                }
                fft::transform(channel, true)?;
            }
            if cancel() {
                return Err(Error::Cancelled);
            }
            let first = self.center - self.half as i128;
            for (index, weight) in self.window.iter().copied().enumerate() {
                self.overlap.add(
                    first + index as i128,
                    [
                        self.synthesis[0][index].re / weight.max(1e-12),
                        self.synthesis[1][index].re / weight.max(1e-12),
                    ],
                    weight * weight,
                )?;
            }
            self.previous_phase.copy_from_slice(&self.reference_phase);
            self.previous_channel
                .copy_from_slice(&self.reference_channel);
            self.previous_magnitude.copy_from_slice(&self.magnitude);
            self.previous_center = Some(source_center);
            self.center += self.hop as i128;
            self.ready_end = (self.center - self.half as i128)
                .max(0)
                .min(i128::from(self.total)) as u64;
            Ok(())
        })();

        if let Err(error) = result {
            self.poisoned = true;
            return Err(error.into());
        }
        Ok(true)
    }

    /// Drain at most `available()` frames. If normalization fails, `position()`
    /// is the exact committed prefix and the renderer is poisoned.
    pub fn drain(&mut self, output: &mut [Frame]) -> Result<(), Error> {
        if self.poisoned {
            return Err(Error::Invalid("spectral renderer is poisoned"));
        }
        if output.len() > self.available() {
            return Err(Error::Invalid("spectral drain exceeds completed frames"));
        }
        if let Err(error) = self.overlap.drain(output) {
            self.poisoned = true;
            return Err(error);
        }
        Ok(())
    }
}
