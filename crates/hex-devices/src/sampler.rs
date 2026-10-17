//! Original borrowed-PCM playback prerequisite for the planned Sampler device.
//!
//! This module is not a registered instrument or a claim of Simpler parity.
//! The control owner validates PCM once and retains it for every borrowed voice.
//! Rendering is deterministic, allocation-free and independent of block size.
//! See `docs/sampler.md` for host asset and reference acceptance still required.
mod envelope;
pub use envelope::Envelope;

use crate::Invalid;
use hex_lattice::StereoFrame;

/// Finite original kernel limit, separate from host resident/stream budgets.
/// At 48 kHz this is approximately six minutes of stereo PCM (128 MiB).
pub const MAX_SAMPLE_FRAMES: usize = 16 * 1024 * 1024;
pub const MAX_SLICES: usize = 128;
const FRACTION_BITS: u32 = 32;
const ONE: u128 = 1_u128 << FRACTION_BITS;

/// Half-open source-frame interval. Constructors validate against the sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub start: usize,
    pub end: usize,
}

impl Region {
    fn valid(self, frames: usize) -> bool {
        self.start < self.end && self.end <= frames
    }

    fn contains(self, other: Self) -> bool {
        self.start <= other.start && other.end <= self.end && other.start < other.end
    }
}

/// Validated finite stereo PCM; construction scans on control, never on audio.
/// No ownership handle is cloned or destroyed when a voice is copied/rendered.
#[derive(Clone, Copy, Debug)]
pub struct Sample<'a> {
    frames: &'a [StereoFrame],
    sample_rate: u32,
}

impl<'a> Sample<'a> {
    pub fn new(frames: &'a [StereoFrame], sample_rate: u32) -> Result<Self, Invalid> {
        if !(8_000..=192_000).contains(&sample_rate)
            || frames.is_empty()
            || frames.len() > MAX_SAMPLE_FRAMES
            || frames.iter().flatten().any(|value| !value.is_finite())
        {
            return Err(Invalid(
                "sample needs bounded finite pcm and a supported rate",
            ));
        }
        Ok(Self {
            frames,
            sample_rate,
        })
    }

    pub fn whole_region(self) -> Region {
        Region {
            start: 0,
            end: self.frames.len(),
        }
    }
}

/// Note-off semantics. Trigger runs to the region end, including after note-off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateMode {
    Gate,
    Trigger,
}

/// Original mode laws, pending a pinned-reference audit. Slice starts are
/// strictly increasing, include region.start, and map from base_note upwards.
/// Each selected slice stops at the next marker or region.end, without looping
/// or chromatic transposition from the selecting key.
#[derive(Clone, Copy, Debug)]
pub enum Mode<'a> {
    Classic {
        loop_region: Option<Region>,
    },
    OneShot {
        gate: GateMode,
    },
    Slice {
        starts: &'a [usize],
        base_note: u8,
        gate: GateMode,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct Settings<'a> {
    pub region: Region,
    pub mode: Mode<'a>,
    pub reverse: bool,
    pub root_note: u8,
    pub transpose_semitones: f64,
    pub detune_cents: f64,
    /// Linear gain in 0..=4. No normalization or musical limiter; only values
    /// beyond f32's finite range saturate at that representation boundary.
    pub gain: f64,
    /// Original linear balance: center preserves both source channels.
    pub pan: f64,
    /// 0 ignores velocity; 1 scales gain by velocity/127.
    pub velocity_amount: f64,
    pub envelope: Envelope,
}

impl Settings<'_> {
    pub fn new(region: Region) -> Self {
        Self {
            region,
            mode: Mode::Classic { loop_region: None },
            reverse: false,
            root_note: 60,
            transpose_semitones: 0.0,
            detune_cents: 0.0,
            gain: 1.0,
            pan: 0.0,
            velocity_amount: 1.0,
            envelope: Envelope::default(),
        }
    }
}

/// Prepared immutable configuration. PCM and slice markers are borrowed from
/// control-owned storage; no file, project ID, queue or runtime owner lives here.
#[derive(Clone, Copy, Debug)]
pub struct PreparedSampler<'a> {
    sample: Sample<'a>,
    settings: Settings<'a>,
    output_rate: u32,
    envelope: envelope::PreparedEnvelope,
}

impl<'a> PreparedSampler<'a> {
    pub fn new(
        sample: Sample<'a>,
        output_rate: u32,
        settings: Settings<'a>,
    ) -> Result<Self, Invalid> {
        if !(8_000..=192_000).contains(&output_rate)
            || !settings.region.valid(sample.frames.len())
            || settings.root_note > 127
            || !finite_range(settings.transpose_semitones, -48.0, 48.0)
            || !finite_range(settings.detune_cents, -100.0, 100.0)
            || !finite_range(settings.gain, 0.0, 4.0)
            || !finite_range(settings.pan, -1.0, 1.0)
            || !finite_range(settings.velocity_amount, 0.0, 1.0)
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
    pub fn prepare_voice(self, note: u8, velocity: u8) -> Result<Option<Voice<'a>>, Invalid> {
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
                f64::from(note) - f64::from(settings.root_note),
            ),
            Mode::OneShot { gate } => (
                settings.region,
                None,
                gate,
                f64::from(note) - f64::from(settings.root_note),
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
        Ok(Some(Voice {
            frames: self.sample.frames,
            region,
            loop_offsets,
            reverse: settings.reverse,
            step: step as u64,
            channel_gain,
            gate,
            envelope: self.envelope,
        }))
    }
}

fn finite_range(value: f64, min: f64, max: f64) -> bool {
    value.is_finite() && (min..=max).contains(&value)
}

/// Copyable borrowed voice. Gate duration and age use output frames. Releasing
/// a gated voice freezes its ADS level at note-off and ramps to zero; source
/// playback (and a Classic loop) continues during release. Age queries may be
/// made in any order, enabling seek/block-independent original render fixtures.
#[derive(Clone, Copy, Debug)]
pub struct Voice<'a> {
    frames: &'a [StereoFrame],
    region: Region,
    loop_offsets: Option<(usize, usize)>,
    reverse: bool,
    step: u64,
    channel_gain: [f64; 2],
    gate: GateMode,
    envelope: envelope::PreparedEnvelope,
}

impl Voice<'_> {
    /// Zero fixed buffering/lookahead; source resampling and envelope timing
    /// do not introduce a block of host latency.
    pub const fn algorithmic_latency_frames(&self) -> u32 {
        0
    }

    pub const fn release_frames(&self) -> u64 {
        match self.gate {
            GateMode::Gate => self.envelope.release_frames(),
            GateMode::Trigger => 0,
        }
    }

    /// Length in output frames for a nonlooping source, excluding envelope
    /// truncation. A loop is unbounded until its gate/release ends. Trigger
    /// voices must be retained to this natural end even after MIDI note-off.
    pub fn natural_frames(&self) -> Option<u64> {
        if self.loop_offsets.is_some() {
            return None;
        }
        let length = (self.region.end - self.region.start) as u128 * ONE;
        u64::try_from(length.div_ceil(u128::from(self.step))).ok()
    }

    pub fn sample(&self, age_frames: u64, gate_frames: u64) -> StereoFrame {
        let envelope = match self.gate {
            GateMode::Gate => self.envelope.sample(age_frames, gate_frames),
            GateMode::Trigger => self.envelope.held(age_frames),
        };
        if envelope == 0.0 {
            return [0.0; 2];
        }
        // u64 * u64 fits u128 exactly, even at u64::MAX age. Integer modulo
        // preserves loop phase where conversion of age to f64 would lose it.
        let mut position = u128::from(age_frames) * u128::from(self.step);
        if let Some((start, end)) = self.loop_offsets {
            let end = end as u128 * ONE;
            let start = start as u128 * ONE;
            if position >= end {
                position = start + (position - end) % (end - start);
            }
        } else if position >= (self.region.end - self.region.start) as u128 * ONE {
            return [0.0; 2];
        }
        let index = (position >> FRACTION_BITS) as usize;
        let mut next = index + 1;
        if let Some((start, end)) = self.loop_offsets {
            if next == end {
                next = start;
            }
        }
        next = next.min(self.region.end - self.region.start - 1);
        let source_index = |index| {
            if self.reverse {
                self.region.end - 1 - index
            } else {
                self.region.start + index
            }
        };
        let (Some(a), Some(b)) = (
            self.frames.get(source_index(index)),
            self.frames.get(source_index(next)),
        ) else {
            return [0.0; 2];
        };
        let fraction = (position % ONE) as f64 / ONE as f64;
        // Interpolate/gain in f64 so finite f32 endpoints do not overflow the
        // subtraction. Saturation at the representation boundary avoids NaN/Inf
        // from otherwise valid finite PCM and the bounded gain domain.
        std::array::from_fn(|channel| {
            let value = (f64::from(a[channel]) * (1.0 - fraction)
                + f64::from(b[channel]) * fraction)
                * self.channel_gain[channel]
                * envelope;
            value.clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32
        })
    }

    /// Fill caller-owned output. Overflowed ages are defined silence; they do
    /// not wrap to frame zero. No cursor, heap, ownership or block-size state.
    pub fn render(&self, start_age: u64, gate_frames: u64, output: &mut [StereoFrame]) {
        for (offset, frame) in output.iter_mut().enumerate() {
            *frame = u64::try_from(offset)
                .ok()
                .and_then(|offset| start_age.checked_add(offset))
                .map_or([0.0; 2], |age| self.sample(age, gate_frames));
        }
    }
}

#[cfg(test)]
mod tests;
