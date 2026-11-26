//! Sequential control-side scheduling for bounded Complex and ComplexPro spectral warp.
//!
//! Each `advance` step synthesizes one STFT hop over a caller-supplied source window
//! using the shared [`crate::spectral::SpectralVocoder`] core for linked stereo phase
//! propagation, identity peak locking, and spectral-envelope formant reimposition.
//! Checkpoints capture only the bounded overlap ring and per-bin phase/magnitude/channel
//! history, independent of total output duration.
use crate::{
    Error, Frame, GranularOverlap, GranularSource, MAX_SCRATCH_BYTES, MAX_STREAMED_OUTPUT_FRAMES,
    MAX_WORK_UNITS, Mode, Q32, Resampler, TimeMap, V2_MAX_TAPS, Work, buffer, fft::Complex,
    granular_overlap::OverlapCheckpoint, granular_sequential::next_renderer_id,
    resample::Bandlimited, spectral::SpectralVocoder,
};

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
    vocoder: SpectralVocoder,
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
        let vocoder = SpectralVocoder::new(config.sample_rate, pitch, pro)?;
        let ring = vocoder.n * 2 + vocoder.hop + 4;
        let max_source_frames = vocoder.n + V2_MAX_TAPS + 4;
        let overlap = GranularOverlap::new(total, ring)?;
        let center = -(vocoder.half as i128);
        let result = Self {
            config,
            owner,
            bank,
            overlap,
            total,
            vocoder,
            center,
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
            && self.center == -(self.vocoder.half as i128)
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
        self.config
            .map
            .backing_bytes()?
            .checked_add(self.bank.backing_bytes()?)?
            .checked_add(self.overlap.backing_bytes()?)?
            .checked_add(self.vocoder.backing_bytes()?)
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
            .vocoder
            .bins
            .checked_mul(3 * std::mem::size_of::<f64>() + std::mem::size_of::<u8>())
            .ok_or(Error::ScratchLimit)?;
        let overlap_budget = effective
            .checked_sub(state_bytes)
            .ok_or(Error::ScratchLimit)?;
        let overlap = self.overlap.checkpoint(overlap_budget)?;
        let mut previous_phase = buffer(self.vocoder.bins, 0.0_f64)?;
        let mut propagated = buffer(self.vocoder.bins, 0.0_f64)?;
        let mut previous_magnitude = buffer(self.vocoder.bins, 0.0_f64)?;
        let mut previous_channel = buffer(self.vocoder.bins, 0_u8)?;
        previous_phase.copy_from_slice(&self.vocoder.previous_phase);
        propagated.copy_from_slice(&self.vocoder.propagated);
        previous_magnitude.copy_from_slice(&self.vocoder.previous_magnitude);
        previous_channel.copy_from_slice(&self.vocoder.previous_channel);
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
        if checkpoint.previous_phase.len() != self.vocoder.bins
            || checkpoint.propagated.len() != self.vocoder.bins
            || checkpoint.previous_magnitude.len() != self.vocoder.bins
            || checkpoint.previous_channel.len() != self.vocoder.bins
        {
            return Err(Error::Invalid("incompatible spectral checkpoint"));
        }
        self.overlap.refresh_checkpoint(&mut checkpoint.overlap)?;
        checkpoint.center = self.center;
        checkpoint.previous_center = self.previous_center;
        checkpoint.ready_end = self.ready_end;
        checkpoint
            .previous_phase
            .copy_from_slice(&self.vocoder.previous_phase);
        checkpoint
            .propagated
            .copy_from_slice(&self.vocoder.propagated);
        checkpoint
            .previous_magnitude
            .copy_from_slice(&self.vocoder.previous_magnitude);
        checkpoint
            .previous_channel
            .copy_from_slice(&self.vocoder.previous_channel);
        Ok(())
    }

    /// Restore a checkpoint captured by this exact renderer, clearing poison on success.
    pub fn restore(&mut self, checkpoint: &SpectralCheckpoint) -> Result<(), Error> {
        if checkpoint.owner != self.owner {
            return Err(Error::Invalid("foreign spectral checkpoint"));
        }
        if checkpoint.previous_phase.len() != self.vocoder.bins
            || checkpoint.propagated.len() != self.vocoder.bins
            || checkpoint.previous_magnitude.len() != self.vocoder.bins
            || checkpoint.previous_channel.len() != self.vocoder.bins
        {
            return Err(Error::Invalid("incompatible spectral checkpoint"));
        }
        self.overlap.restore(&checkpoint.overlap)?;
        self.center = checkpoint.center;
        self.previous_center = checkpoint.previous_center;
        self.ready_end = checkpoint.ready_end;
        self.vocoder
            .previous_phase
            .copy_from_slice(&checkpoint.previous_phase);
        self.vocoder
            .propagated
            .copy_from_slice(&checkpoint.propagated);
        self.vocoder
            .previous_magnitude
            .copy_from_slice(&checkpoint.previous_magnitude);
        self.vocoder
            .previous_channel
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
        let half = self.vocoder.half;
        let first_pos = source_center - half as f64;
        let last_pos = source_center + (self.vocoder.n - 1) as f64 - half as f64;
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
            for (index, weight) in self.vocoder.window.iter().copied().enumerate() {
                if index > 0 && index % 1_024 == 0 && cancel() {
                    return Err(Error::Cancelled);
                }
                let position = source_center + index as f64 - half as f64;
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
                    self.vocoder.analysis[channel][index] = Complex::new(value * weight, 0.0);
                }
            }
            self.vocoder
                .synthesize_hop(source_center, self.previous_center, Some(cancel))?;
            let first = self.center - half as i128;
            for (index, weight) in self.vocoder.window.iter().copied().enumerate() {
                self.overlap.add(
                    first + index as i128,
                    [
                        self.vocoder.synthesis[0][index].re / weight.max(1e-12),
                        self.vocoder.synthesis[1][index].re / weight.max(1e-12),
                    ],
                    weight * weight,
                )?;
            }
            self.vocoder.commit_hop();
            self.previous_center = Some(source_center);
            self.center += self.vocoder.hop as i128;
            self.ready_end = (self.center - half as i128)
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
