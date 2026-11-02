//! Original source-coordinate sampler kernel and explicit empty catalog state.
//!
//! The empty catalog sentinel is silent and owns no asset or substitute PCM; malformed or
//! source-bearing state without its actual source must refuse preparation. Loaded playback
//! uses a registered mutable factory over the host's exact original-rate inventory. Ordinary
//! playback retains that configuration in each runtime lease, including outgoing voices; the
//! obsolete private constructor keeps its separately closed source path. Source regions are nonempty half-open
//! frame intervals. Pitch/rate conversion, reverse, Classic loops (with an optional seam crossfade), One-Shot/Slice gate laws and
//! envelope ages share the same bounded scalar preparation. Voices borrow immutable PCM without
//! per-note scan, copy or owning handle; sample queries remain deterministic across block sizes.
//! These original laws do not establish Live parity or native deadline acceptance.
mod binding;
pub mod catalog;
pub mod control;
mod envelope;
mod interpolation;
mod plan;
mod runtime;
pub mod state;
mod voice;
pub use binding::{BorrowedSamplerVoice, BoundSamplerPlan};
pub use envelope::Envelope;
pub use plan::{PreparedSamplerPlan, SampleChannels, SampleShape};
pub use runtime::{MAX_SAMPLER_RUNTIME_VOICES, PreparedSamplerRuntime, SamplerRuntime};
pub use voice::{Voice, VoiceCoefficients};

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
    /// Classic loop crossfade in milliseconds of original source time, within
    /// 0..=[`MAX_LOOP_CROSSFADE_MS`]. Zero (the default) is off and keeps the
    /// hard loop seam bit-identical. Otherwise the last crossfade frames before
    /// the loop end blend linearly into the same-length pre-roll ending at the
    /// loop start, so the wrap is continuous. The effective length is limited
    /// to the loop length and the pre-roll available in the playback direction;
    /// no pre-roll (loop start at the region edge) means no crossfade.
    pub loop_crossfade_ms: f64,
}

/// Authored loop-crossfade upper bound, in milliseconds of source time.
pub const MAX_LOOP_CROSSFADE_MS: f64 = 10_000.0;

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
            loop_crossfade_ms: 0.0,
        }
    }
}

/// Original validated stereo adapter over the nonowning source-coordinate plan.
/// No source owner or native storage/admission authority is created here.
#[derive(Clone, Copy, Debug)]
pub struct PreparedSampler<'a> {
    sample: Sample<'a>,
    plan: PreparedSamplerPlan<'a>,
}
impl<'a> PreparedSampler<'a> {
    pub fn new(
        sample: Sample<'a>,
        output_rate: u32,
        settings: Settings<'a>,
    ) -> Result<Self, Invalid> {
        let shape = SampleShape::new(
            sample.frames.len(),
            sample.sample_rate,
            SampleChannels::Stereo,
        )?;
        Ok(Self {
            sample,
            plan: PreparedSamplerPlan::new(shape, output_rate, settings)?,
        })
    }
    pub fn prepare_voice(self, note: u8, velocity: u8) -> Result<Option<Voice<'a>>, Invalid> {
        self.plan
            .prepare_voice_coefficients(note, velocity)
            .map(|voice| {
                voice.map(|coefficients| Voice {
                    frames: self.sample.frames,
                    coefficients,
                })
            })
    }
}

fn finite_range(value: f64, min: f64, max: f64) -> bool {
    value.is_finite() && (min..=max).contains(&value)
}

#[cfg(test)]
mod interpolation_tests;
#[cfg(test)]
mod lifetime_tests;
#[cfg(test)]
mod plan_tests;
#[cfg(test)]
mod tests;
