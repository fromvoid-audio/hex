//! Sequential control-side scheduling for bounded granular reconstruction.
use crate::{
    Error, Frame, GrainAnalysis, GrainSynthesis, GranularOverlap, MAX_SCRATCH_BYTES,
    MAX_STREAMED_OUTPUT_FRAMES, Mode, Q32, Resampler, TimeMap, analyze_grain,
};

/// Logical source access for a single sequential render. Both methods must
/// resolve the same unwrapped region-relative positions, including the source's
/// phase, loop, gain and out-of-region silence. The caller binds a stable source
/// identity/generation to the renderer and owns all I/O, scratch, custody and
/// cancellation within source calls. A window call may partially write on error.
pub trait GranularSource {
    type Error: From<Error>;
    fn frame(&mut self, position: i128) -> Result<[f64; 2], Self::Error>;
    fn window(&mut self, start: i128, output: &mut [[f64; 2]]) -> Result<(), Self::Error>;
}

/// Parameters for a sequential Tones or Texture render.
pub struct SequentialGranularConfig {
    pub map: TimeMap,
    pub sample_rate: u32,
    pub mode: Mode,
    pub pitch_cents: i16,
    pub resampler: Resampler,
    pub reference_channel: usize,
}

/// Bounded control-side grain scheduler and overlap owner.
///
/// Each advance handles at most one grain. Drain every completed frame before
/// advancing again. The caller supplies stable source data and scratch; neither
/// is retained here. No seek or source replacement is supported. Do not run on
/// an audio callback or a worker solely responsible for active playback pages.
pub struct SequentialGranular {
    config: SequentialGranularConfig,
    synth: GrainSynthesis,
    overlap: GranularOverlap,
    total: u64,
    nominal: usize,
    hop: usize,
    center: i128,
    index: u64,
    previous: Option<f64>,
    ready_end: u64,
    max_source_frames: usize,
    poisoned: bool,
}

impl SequentialGranular {
    /// Validate configuration and allocate only bounded filter and overlap state.
    pub fn new(
        config: SequentialGranularConfig,
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<Self, Error> {
        let grain_ms = match config.mode {
            Mode::Tones { grain_ms } if (8..=200).contains(&grain_ms) => grain_ms,
            Mode::Texture {
                grain_ms,
                fluctuation,
                ..
            } if (8..=200).contains(&grain_ms) && fluctuation <= 100 => grain_ms,
            _ => return Err(Error::Invalid("invalid sequential granular mode")),
        };
        if !(8_000..=192_000).contains(&config.sample_rate) || config.reference_channel > 1 {
            return Err(Error::Invalid("invalid sequential granular source"));
        }
        config
            .map
            .validate_stretch(config.mode, config.pitch_cents)?;
        let total = config
            .map
            .segments()
            .last()
            .ok_or(Error::Invalid("empty granular map"))?
            .output_end;
        if total == 0 || total > MAX_STREAMED_OUTPUT_FRAMES {
            return Err(Error::Invalid("invalid sequential granular extent"));
        }
        let nominal =
            (u64::from(config.sample_rate) * u64::from(grain_ms) / 1_000).max(16) as usize;
        let hop = (nominal / 4).max(1);
        let ring = nominal * 2 + hop + 4;
        let pitch = 2.0_f64.powf(f64::from(config.pitch_cents) / 1_200.0);
        // Full source interval spans at most 2*nominal at this pitch, plus
        // filter halo and rounding allowance, independent of output duration.
        let max_source_frames = (2.0 * nominal as f64 * pitch).ceil() as usize + 520;
        let synth = GrainSynthesis::new(config.pitch_cents, config.resampler, cancel)?;
        let overlap = GranularOverlap::new(total, ring)?;
        let result = Self {
            config,
            synth,
            overlap,
            total,
            nominal,
            hop,
            center: -(nominal as i128),
            index: 0,
            previous: None,
            ready_end: 0,
            max_source_frames,
            poisoned: false,
        };
        let aggregate = result
            .backing_bytes()
            .and_then(|n| {
                max_source_frames
                    .checked_mul(std::mem::size_of::<[f64; 2]>())
                    .and_then(|scratch| n.checked_add(scratch))
            })
            .ok_or(Error::ScratchLimit)?;
        if aggregate > MAX_SCRATCH_BYTES {
            return Err(Error::ScratchLimit);
        }
        Ok(result)
    }

    /// Retained map, filter and overlap allocations; excludes the inline object.
    pub fn backing_bytes(&self) -> Option<usize> {
        self.config
            .map
            .backing_bytes()?
            .checked_add(self.synth.filter_backing_bytes()?)?
            .checked_add(self.overlap.backing_bytes()?)
    }

    /// Maximum caller-owned source scratch required for any one grain.
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
    /// A poisoned state must be discarded; no retry or drain is permitted.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }
    /// All frames have been drained.
    pub fn is_complete(&self) -> bool {
        !self.poisoned && self.position() == self.total
    }

    /// Analyze and reconstruct at most one grain. Returns false only when all
    /// output is complete. Source or scratch errors before accumulation leave
    /// renderer state unchanged; scratch and source side effects may remain.
    /// Any reconstruction error, including cancellation before its first sample,
    /// poisons the renderer, which must be discarded.
    /// The caller must not advance while completed frames remain undrained.
    pub fn advance<S: GranularSource>(
        &mut self,
        source: &mut S,
        scratch: &mut [[f64; 2]],
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<bool, S::Error> {
        if self.poisoned {
            return Err(Error::Invalid("granular renderer is poisoned").into());
        }
        if self.available() != 0 {
            return Err(Error::Invalid("granular output must be drained first").into());
        }
        if self.is_complete() {
            return Ok(false);
        }
        let desired = self
            .config
            .map
            .position_q32(self.center)
            .map_err(S::Error::from)? as f64
            / Q32 as f64;
        let decision = analyze_grain(
            GrainAnalysis {
                sample_rate: self.config.sample_rate,
                mode: self.config.mode,
                pitch_cents: self.config.pitch_cents,
                center: self.center,
                desired,
                previous_anchor: self.previous,
                grain_index: self.index,
                reference_channel: self.config.reference_channel,
            },
            &mut |position| source.frame(position),
            cancel,
        )?;
        let range = self
            .synth
            .required_source_range(decision.anchor, decision.length)
            .map_err(S::Error::from)?;
        let count = usize::try_from(range.end - range.start)
            .map_err(|_| S::Error::from(Error::ScratchLimit))?;
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
        let window = &mut scratch[..count];
        source.window(range.start, window)?;
        if let Err(error) = self.synth.add_grain(
            self.center,
            decision,
            range.start,
            window,
            &mut self.overlap,
            cancel,
        ) {
            self.poisoned = true;
            return Err(error.into());
        }
        self.previous = Some(decision.anchor);
        self.center += self.hop as i128;
        self.index = self.index.wrapping_add(1);
        self.ready_end = (self.center - self.nominal as i128)
            .max(0)
            .min(i128::from(self.total)) as u64;
        Ok(true)
    }

    /// Drain at most `available()` frames. If normalization fails, `position()`
    /// is the exact committed prefix and the renderer is poisoned.
    pub fn drain(&mut self, output: &mut [Frame]) -> Result<(), Error> {
        if self.poisoned {
            return Err(Error::Invalid("granular renderer is poisoned"));
        }
        if output.len() > self.available() {
            return Err(Error::Invalid("granular drain exceeds completed frames"));
        }
        if let Err(error) = self.overlap.drain(output) {
            self.poisoned = true;
            return Err(error);
        }
        Ok(())
    }
}
