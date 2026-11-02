//! Source-coordinate preparation, independent of PCM ownership and storage.
use super::*;
/// Logical sample channels, never a physical storage/layout declaration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleChannels {
    Mono,
    Stereo,
}
/// Bounded DSP source facts only. These numeric facts do NOT establish PCM
/// validity, storage provenance, a native lease, identity or admission authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleShape {
    frames: usize,
    sample_rate: u32,
    channels: SampleChannels,
}
impl SampleShape {
    pub fn new(frames: usize, sample_rate: u32, channels: SampleChannels) -> Result<Self, Invalid> {
        if frames == 0 || frames > MAX_SAMPLE_FRAMES || !(8_000..=192_000).contains(&sample_rate) {
            return Err(Invalid("sample shape exceeds bounded source domain"));
        }
        Ok(Self {
            frames,
            sample_rate,
            channels,
        })
    }
    pub fn frames(self) -> usize {
        self.frames
    }
    pub fn sample_rate(self) -> u32 {
        self.sample_rate
    }
    pub fn channels(self) -> SampleChannels {
        self.channels
    }
}

/// Control-prepared source-coordinate plan. Only slice markers are borrowed;
/// it stores no PCM, pointer, Arc or ownership receipt. Voice coefficients are
/// independent of both the source buffers and the marker storage.
#[derive(Clone, Copy, Debug)]
pub struct PreparedSamplerPlan<'a> {
    pub(super) sample: SampleShape,
    pub(super) settings: Settings<'a>,
    output_rate: u32,
    envelope: envelope::PreparedEnvelope,
}

impl<'a> PreparedSamplerPlan<'a> {
    pub(super) const fn output_rate(self) -> u32 {
        self.output_rate
    }

    pub fn new(
        sample: SampleShape,
        output_rate: u32,
        settings: Settings<'a>,
    ) -> Result<Self, Invalid> {
        if !(8_000..=192_000).contains(&output_rate)
            || !settings.region.valid(sample.frames)
            || settings.root_note > 127
            || !finite_range(settings.transpose_semitones, -48.0, 48.0)
            || !finite_range(settings.detune_cents, -100.0, 100.0)
            || !finite_range(settings.gain, 0.0, 4.0)
            || !finite_range(settings.pan, -1.0, 1.0)
            || !finite_range(settings.velocity_amount, 0.0, 1.0)
            || !finite_range(settings.loop_crossfade_ms, 0.0, MAX_LOOP_CROSSFADE_MS)
        {
            return Err(Invalid("sampler settings are outside their finite domain"));
        }
        match settings.mode {
            Mode::Classic {
                loop_region: Some(region),
            } if !settings.region.contains(region) => {
                return Err(Invalid(
                    "sample loop must be nonempty and inside the region",
                ));
            }
            Mode::Slice {
                starts, base_note, ..
            } => {
                if starts.is_empty()
                    || starts.len() > MAX_SLICES
                    || base_note > 127
                    || starts.len() > 128 - usize::from(base_note)
                    || starts.first() != Some(&settings.region.start)
                    || starts.last().is_none_or(|end| *end >= settings.region.end)
                    || starts.windows(2).any(|pair| pair[0] >= pair[1])
                {
                    return Err(Invalid(
                        "sample slices need ordered in-region midi-mapped markers",
                    ));
                }
            }
            _ => {}
        }
        // Static reconstruction tables are built here, on control, never on
        // first render.
        super::interpolation::warm();
        Ok(Self {
            sample,
            output_rate,
            envelope: settings.envelope.prepare(output_rate)?,
            settings,
        })
    }

    /// Control-side voice preparation. None denotes velocity zero or an
    /// unmapped slice key. Invalid MIDI bytes are refused, never truncated.
    /// The host remains responsible for polyphony, note identity and stealing.
    pub fn prepare_voice_coefficients(
        self,
        note: u8,
        velocity: u8,
    ) -> Result<Option<VoiceCoefficients>, Invalid> {
        self.prepare_voice_with_transpose(note, velocity, None)
    }

    /// Project absolute note Hz is relative to the authored A440 12-TET root.
    /// Uniform retuning therefore moves the root itself. Slice keys continue
    /// selecting source regions rather than pitches.
    pub fn prepare_tuned_voice_coefficients(
        self,
        note: u8,
        velocity: u8,
        tuning: &hex_lattice::TuningTable,
    ) -> Result<Option<VoiceCoefficients>, Invalid> {
        let frequency = tuning
            .frequency(note)
            .map_err(|_| Invalid("sampler tuning note unavailable"))?;
        let root = hex_lattice::TuningTable::default()
            .frequency(self.settings.root_note)
            .map_err(|_| Invalid("sampler tuning root unavailable"))?;
        let transpose = 12.0 * (frequency / root).log2();
        self.prepare_voice_with_transpose(note, velocity, Some(transpose))
    }

    fn prepare_voice_with_transpose(
        self,
        note: u8,
        velocity: u8,
        tuned_transpose: Option<f64>,
    ) -> Result<Option<VoiceCoefficients>, Invalid> {
        if note > 127 || velocity > 127 {
            return Err(Invalid("sample note and velocity must be seven-bit midi"));
        }
        if velocity == 0 {
            return Ok(None);
        }
        let settings = self.settings;
        let (region, loop_region, gate, transpose) = match settings.mode {
            Mode::Classic { loop_region } => (
                settings.region,
                loop_region,
                GateMode::Gate,
                tuned_transpose.unwrap_or(f64::from(note) - f64::from(settings.root_note)),
            ),
            Mode::OneShot { gate } => (
                settings.region,
                None,
                gate,
                tuned_transpose.unwrap_or(f64::from(note) - f64::from(settings.root_note)),
            ),
            Mode::Slice {
                starts,
                base_note,
                gate,
            } => {
                let Some(index) = note.checked_sub(base_note).map(usize::from) else {
                    return Ok(None);
                };
                let Some(&start) = starts.get(index) else {
                    return Ok(None);
                };
                let end = starts
                    .get(index + 1)
                    .copied()
                    .unwrap_or(settings.region.end);
                (Region { start, end }, None, gate, 0.0)
            }
        };
        let semitones = transpose + settings.transpose_semitones + settings.detune_cents / 100.0;
        let step = (f64::from(self.sample.sample_rate) / f64::from(self.output_rate)
            * 2_f64.powf(semitones / 12.0)
            * ONE as f64)
            .round();
        // The validated pitch/rate domains fit comfortably in Q32.32. Keep
        // this check explicit before float-to-integer conversion.
        if !step.is_finite() || !(1.0..=(u64::MAX / 2) as f64).contains(&step) {
            return Err(Invalid("sample playback rate exceeds fixed-point range"));
        }
        let velocity_gain =
            1.0 - settings.velocity_amount + settings.velocity_amount * f64::from(velocity) / 127.0;
        let gain = settings.gain * velocity_gain;
        let channel_gain = [
            gain * (1.0 - settings.pan.max(0.0)),
            gain * (1.0 + settings.pan.min(0.0)),
        ];
        let loop_offsets = loop_region.map(|loop_region| {
            if settings.reverse {
                (region.end - loop_region.end, region.end - loop_region.start)
            } else {
                (
                    loop_region.start - region.start,
                    loop_region.end - region.start,
                )
            }
        });
        // Source-time milliseconds become whole source frames here, limited to
        // the loop length and the pre-roll before the loop start in playback
        // (offset) order. The bounded ms domain keeps this product finite.
        let loop_crossfade = loop_offsets.map_or(0, |(start, end)| {
            let frames = (settings.loop_crossfade_ms * f64::from(self.sample.sample_rate) / 1000.0)
                .round() as usize;
            frames.min(end - start).min(start)
        });
        Ok(Some(VoiceCoefficients {
            expected_frames: self.sample.frames,
            channels: self.sample.channels,
            region,
            loop_offsets,
            loop_crossfade,
            reverse: settings.reverse,
            step: step as u64,
            kernel: super::interpolation::Kernel::for_rate(step / ONE as f64),
            channel_gain,
            gate,
            envelope: self.envelope,
        }))
    }
}
