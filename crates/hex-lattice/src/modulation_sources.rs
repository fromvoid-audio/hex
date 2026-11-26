//! Transactional sample generation for a host-owned bank of modulators.
//!
//! Construction and destruction are control-side. The bank owns fixed source
//! state and preallocated output buffers, but no clock, graph, transport,
//! scheduling, source registry or publication authority. The enclosing graph
//! begins one transaction, renders as many bounded chunks as needed, and then
//! commits or abandons it with the rest of the audio block. Each chunk's samples
//! remain available until the next render. The host splits longer segments and
//! defines whether bypass, stop, seek, loops and retrigger advance or reset a
//! source; this helper never infers those events from block boundaries.
//! Tempo sources preserve phase while following sample-local host tempo. One
//! preallocated cache shares clock reads across sources; absent clock values use
//! the prepared tempo. Rate/mode retargeting follows the same block transaction
//! as phase, and graph inheritance retains the new graph's prepared timing mode.
use crate::{
    NativeInstrumentClock, NativeInstrumentEvent, SourceModulationParameter,
    modulation_adsr::Adsr,
    modulation_ahd_release::AhdRelease,
    modulation_note_counter::NoteCounter,
    modulation_note_events::validate_events,
    modulation_note_gate::NoteGate,
    modulation_ramp::{ModulationRampCurve, Ramp},
    modulation_source_routes::PreparedSourceRoute,
    modulation_steps::{ModulationStepDirection, Steps, TransportSteps},
};
use hex_dsp::{EnvelopeFollower, LfoRate, LfoWaveform, ModulationLfo, RandomHold, StereoFrame};
use hex_lattice_contract::{DeviceContractError as Error, ModulationOutputId, ModulationSourceId};

pub const MAX_MODULATION_SOURCE_SOURCES: usize = 32;
pub const MAX_MODULATION_SOURCE_STORAGE_BYTES: usize = 262_144;
pub const MAX_MODULATION_SOURCE_CHUNK_FRAMES: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Timing {
    Free(LfoRate),
    Tempo {
        beats_per_cycle: f64,
        fallback: LfoRate,
    },
}
impl Timing {
    fn rate(self, sample_rate: f64, tempo_micro_bpm: u32) -> Result<LfoRate, Error> {
        match self {
            Self::Free(rate) => Ok(rate),
            Self::Tempo {
                beats_per_cycle,
                fallback,
            } => {
                if tempo_micro_bpm == 0 {
                    return Ok(fallback);
                }
                LfoRate::tempo(
                    sample_rate,
                    f64::from(tempo_micro_bpm) / 1_000_000.0,
                    beats_per_cycle,
                )
                .map_err(|_| Error::InvalidParameterValue)
            }
        }
    }
}

fn render_rate_route(
    oscillator: &mut ModulationLfo,
    output: &mut [f32],
    base_rate: impl Fn(usize) -> Result<LfoRate, Error>,
    contribution: impl Fn(usize) -> f64,
) -> Result<(), Error> {
    for (frame, sample) in output.iter_mut().enumerate() {
        let base_rate = base_rate(frame)?;
        let depth = contribution(frame).clamp(-1.0, 1.0);
        let rate = if depth == 0.0 {
            base_rate
        } else {
            base_rate
                .scaled((4.0 * depth).exp2())
                .map_err(|_| Error::InvalidParameterValue)?
        };
        oscillator.set_rate(rate);
        *sample = oscillator.next_sample();
    }
    Ok(())
}

/// Arithmetic performed after applying modulation to each signed input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModulationMathOperation {
    Multiply,
    Add,
    Subtract,
    Minimum,
    Maximum,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MathControls {
    inputs: [f32; 2],
    operation: ModulationMathOperation,
}
impl MathControls {
    fn sample(self, offsets: [f64; 2]) -> f32 {
        let [a, b] = std::array::from_fn(|index| {
            (f64::from(self.inputs[index]) + 2.0 * offsets[index]).clamp(-1.0, 1.0)
        });
        match self.operation {
            ModulationMathOperation::Multiply => a * b,
            ModulationMathOperation::Add => a + b,
            ModulationMathOperation::Subtract => a - b,
            ModulationMathOperation::Minimum => a.min(b),
            ModulationMathOperation::Maximum => a.max(b),
        }
        .clamp(-1.0, 1.0) as f32
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MixControls {
    inputs: [f32; 2],
    blend: f32,
}

impl MixControls {
    fn sample(self, offsets: [f64; 3]) -> f32 {
        let a = (f64::from(self.inputs[0]) + 2.0 * offsets[0]).clamp(-1.0, 1.0);
        let b = (f64::from(self.inputs[1]) + 2.0 * offsets[1]).clamp(-1.0, 1.0);
        let blend = (f64::from(self.blend) + offsets[2]).clamp(0.0, 1.0);
        (a * (1.0 - blend) + b * blend) as f32
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum SourceState {
    Lfo {
        oscillator: ModulationLfo,
        timing: Timing,
    },
    Follower(EnvelopeFollower),
    AudioRate,
    Macro(f32),
    Math(MathControls),
    Mix(MixControls),
    NoteCounter(NoteCounter),
    Ramp(Ramp),
    Adsr(Adsr),
    AhdRelease(AhdRelease),
    Random(RandomHold),
    Steps(Steps),
    TransportSteps(TransportSteps),
}

impl SourceState {
    fn tempo_timing(self) -> Option<Timing> {
        match self {
            Self::Lfo {
                timing: timing @ Timing::Tempo { .. },
                ..
            } => Some(timing),
            _ => None,
        }
    }

    fn inherit_history(&mut self, previous: Self) -> bool {
        match (self, previous) {
            (
                Self::Lfo { oscillator, .. },
                Self::Lfo {
                    oscillator: old, ..
                },
            ) => oscillator.inherit_phase(&old),
            (Self::Follower(follower), Self::Follower(old)) => follower.inherit_envelope(&old),
            // The candidate's authored value is complete. There is no
            // temporal history to carry from the prior graph.
            (Self::Macro(_), Self::Macro(_))
            | (Self::Math(_), Self::Math(_))
            | (Self::Mix(_), Self::Mix(_)) => {}
            (Self::AudioRate, Self::AudioRate) => {}
            (Self::NoteCounter(counter), Self::NoteCounter(old)) => counter.inherit(old),
            (Self::Ramp(ramp), Self::Ramp(old)) => ramp.inherit(old),
            (Self::Adsr(envelope), Self::Adsr(old)) => envelope.inherit(old),
            (Self::AhdRelease(envelope), Self::AhdRelease(old)) => envelope.inherit(old),
            (Self::Random(random), Self::Random(old)) => random.inherit_history(&old),
            (Self::Steps(steps), Self::Steps(old)) => steps.inherit(old),
            (Self::TransportSteps(steps), Self::TransportSteps(old)) => steps.inherit(old),
            _ => return false,
        }
        true
    }
}

/// A source prepared on control with validated parameters and initial state.
#[derive(Clone, Copy, Debug)]
pub struct PreparedModulationSource {
    pub id: ModulationSourceId,
    output: ModulationOutputId,
    initial: SourceState,
    sample_rate: f64,
}
impl PreparedModulationSource {
    /// Select an output from the owning provider's declared inventory.
    /// Existing one-output constructors use MAIN.
    pub const fn with_output(mut self, output: ModulationOutputId) -> Self {
        self.output = output;
        self
    }

    pub(crate) const fn key(&self) -> (ModulationSourceId, ModulationOutputId) {
        (self.id, self.output)
    }

    /// Validate the target against the prepared source class and timing mode.
    pub fn supports_modulation_parameter(&self, parameter: SourceModulationParameter) -> bool {
        matches!(
            (parameter, self.initial),
            (SourceModulationParameter::MacroValue, SourceState::Macro(_))
                | (
                    SourceModulationParameter::MathA | SourceModulationParameter::MathB,
                    SourceState::Math(_)
                )
                | (
                    SourceModulationParameter::MixA
                        | SourceModulationParameter::MixB
                        | SourceModulationParameter::MixBlend,
                    SourceState::Mix(_)
                )
                | (
                    SourceModulationParameter::FreeLfoRate,
                    SourceState::Lfo {
                        timing: Timing::Free(_),
                        ..
                    }
                )
                | (
                    SourceModulationParameter::TempoLfoRate,
                    SourceState::Lfo {
                        timing: Timing::Tempo { .. },
                        ..
                    }
                )
        )
    }

    /// True when rendering needs a causal pre-device audio tap.
    pub fn reads_audio_input(&self) -> bool {
        matches!(
            self.initial,
            SourceState::Follower(_) | SourceState::AudioRate
        )
    }

    /// True when rendering needs the instrument owner's ordered note events.
    pub fn reads_note_events(&self) -> bool {
        matches!(
            self.initial,
            SourceState::NoteCounter(_)
                | SourceState::Ramp(_)
                | SourceState::AhdRelease(_)
                | SourceState::Adsr(_)
        )
    }

    pub(crate) fn reads_note_gate(&self) -> bool {
        matches!(self.initial, SourceState::Adsr(_))
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Prepare a free-running source without exposing DSP internals to the host.
    pub fn free(
        id: ModulationSourceId,
        sample_rate: f64,
        frequency_hz: f64,
        phase: f64,
    ) -> Result<Self, Error> {
        Self::free_with_waveform(id, sample_rate, frequency_hz, phase, LfoWaveform::Sine)
    }

    /// Prepare a free-running oscillator with an explicit output shape.
    pub fn free_with_waveform(
        id: ModulationSourceId,
        sample_rate: f64,
        frequency_hz: f64,
        phase: f64,
        waveform: LfoWaveform,
    ) -> Result<Self, Error> {
        let rate = hex_dsp::LfoRate::free(sample_rate, frequency_hz)
            .map_err(|_| Error::InvalidParameterValue)?;
        let lfo = ModulationLfo::new_with_waveform(rate, phase, waveform)
            .map_err(|_| Error::InvalidParameterValue)?;
        Ok(Self {
            id,
            output: ModulationOutputId::MAIN,
            sample_rate,
            initial: SourceState::Lfo {
                oscillator: lfo,
                timing: Timing::Free(rate),
            },
        })
    }

    /// Prepare a tempo-synced source using quarter-note beats per cycle.
    pub fn tempo(
        id: ModulationSourceId,
        sample_rate: f64,
        beats_per_minute: f64,
        beats_per_cycle: f64,
        phase: f64,
    ) -> Result<Self, Error> {
        Self::tempo_with_waveform(
            id,
            sample_rate,
            beats_per_minute,
            beats_per_cycle,
            phase,
            LfoWaveform::Sine,
        )
    }

    /// Prepare a tempo-synced oscillator with an explicit output shape.
    pub fn tempo_with_waveform(
        id: ModulationSourceId,
        sample_rate: f64,
        beats_per_minute: f64,
        beats_per_cycle: f64,
        phase: f64,
        waveform: LfoWaveform,
    ) -> Result<Self, Error> {
        let rate = hex_dsp::LfoRate::tempo(sample_rate, beats_per_minute, beats_per_cycle)
            .map_err(|_| Error::InvalidParameterValue)?;
        let lfo = ModulationLfo::new_with_waveform(rate, phase, waveform)
            .map_err(|_| Error::InvalidParameterValue)?;
        Ok(Self {
            id,
            output: ModulationOutputId::MAIN,
            sample_rate,
            initial: SourceState::Lfo {
                oscillator: lfo,
                timing: Timing::Tempo {
                    beats_per_cycle,
                    fallback: rate,
                },
            },
        })
    }

    /// A follower consumes the owning processor's pre-device audio for each
    /// frame. The graph supplies the borrowed signal during rendering.
    pub fn envelope_follower(
        id: ModulationSourceId,
        sample_rate: u32,
        attack_seconds: f64,
        release_seconds: f64,
    ) -> Result<Self, Error> {
        let follower = EnvelopeFollower::new(sample_rate, attack_seconds, release_seconds)
            .map_err(|_| Error::InvalidParameterValue)?;
        Ok(Self {
            id,
            output: ModulationOutputId::MAIN,
            initial: SourceState::Follower(follower),
            sample_rate: f64::from(sample_rate),
        })
    }

    /// A memoryless bipolar source reads the pre-device signal at each sample.
    /// Its mono tap averages both stereo channels and bounds the result to ±1.
    pub fn audio_rate(id: ModulationSourceId, sample_rate: u32) -> Result<Self, Error> {
        if sample_rate == 0 {
            return Err(Error::InvalidParameterValue);
        }
        Ok(Self {
            id,
            output: ModulationOutputId::MAIN,
            initial: SourceState::AudioRate,
            sample_rate: f64::from(sample_rate),
        })
    }

    /// A manual source holds its finite normalized value at each sample.
    pub fn macro_value(
        id: ModulationSourceId,
        sample_rate: u32,
        value: f32,
    ) -> Result<Self, Error> {
        if sample_rate == 0 || !(0.0..=1.0).contains(&value) {
            return Err(Error::InvalidParameterValue);
        }
        Ok(Self {
            id,
            output: ModulationOutputId::MAIN,
            initial: SourceState::Macro(value),
            sample_rate: f64::from(sample_rate),
        })
    }

    /// Prepare signed A and B controls with one bipolar output. Incoming
    /// offsets are normalized to each input's full range before arithmetic.
    pub fn math(
        id: ModulationSourceId,
        sample_rate: u32,
        inputs: [f32; 2],
        operation: ModulationMathOperation,
    ) -> Result<Self, Error> {
        if !(8_000..=192_000).contains(&sample_rate)
            || inputs.iter().any(|value| !(-1.0..=1.0).contains(value))
        {
            return Err(Error::InvalidParameterValue);
        }
        Ok(Self {
            id,
            output: ModulationOutputId::MAIN,
            initial: SourceState::Math(MathControls { inputs, operation }),
            sample_rate: f64::from(sample_rate),
        })
    }

    /// Crossfade between signed A and B at a normalized position. The
    /// prepared graph clamps each input after summing its incoming routes.
    pub fn mix(
        id: ModulationSourceId,
        sample_rate: u32,
        inputs: [f32; 2],
        blend: f32,
    ) -> Result<Self, Error> {
        if !(8_000..=192_000).contains(&sample_rate)
            || inputs.iter().any(|value| !(-1.0..=1.0).contains(value))
            || !(0.0..=1.0).contains(&blend)
        {
            return Err(Error::InvalidParameterValue);
        }
        Ok(Self {
            id,
            output: ModulationOutputId::MAIN,
            initial: SourceState::Mix(MixControls { inputs, blend }),
            sample_rate: f64::from(sample_rate),
        })
    }

    /// Advance by a positive increment at each Note On and wrap after
    /// `steps`. The first onset advances from step zero; note releases
    /// do not change the held step.
    pub fn note_counter(
        id: ModulationSourceId,
        sample_rate: u32,
        steps: u16,
        increment: u16,
        bipolar: bool,
    ) -> Result<Self, Error> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(Error::InvalidParameterValue);
        }
        Ok(Self {
            id,
            output: ModulationOutputId::MAIN,
            initial: SourceState::NoteCounter(NoteCounter::new(steps, increment, bipolar)?),
            sample_rate: f64::from(sample_rate),
        })
    }

    /// Restart at every note onset. A one-shot reaches the endpoint
    /// exactly at `duration_frames`; a loop restarts at that same offset.
    pub fn note_ramp(
        id: ModulationSourceId,
        sample_rate: u32,
        duration_frames: u32,
        curve: ModulationRampCurve,
        falling: bool,
        looping: bool,
    ) -> Result<Self, Error> {
        if !(8_000..=192_000).contains(&sample_rate) || duration_frames > 60 * sample_rate {
            return Err(Error::InvalidParameterValue);
        }
        let duration =
            std::num::NonZeroU32::new(duration_frames).ok_or(Error::InvalidParameterValue)?;
        Ok(Self {
            id,
            output: ModulationOutputId::MAIN,
            initial: SourceState::Ramp(Ramp::new(duration, curve, falling, looping)),
            sample_rate: f64::from(sample_rate),
        })
    }

    /// Trigger an attack, hold and decay envelope on note release. A zero
    /// duration skips that segment. Single-trigger mode ignores releases
    /// until the active envelope has emitted its last sample.
    pub fn ahd_on_release(
        id: ModulationSourceId,
        sample_rate: u32,
        durations: [u32; 3],
        curves: [ModulationRampCurve; 2],
        single_trigger: bool,
    ) -> Result<Self, Error> {
        if !(8_000..=192_000).contains(&sample_rate)
            || durations == [0; 3]
            || durations.iter().any(|&frames| frames > 60 * sample_rate)
        {
            return Err(Error::InvalidParameterValue);
        }
        Ok(Self {
            id,
            output: ModulationOutputId::MAIN,
            initial: SourceState::AhdRelease(AhdRelease::new(durations, curves, single_trigger)),
            sample_rate: f64::from(sample_rate),
        })
    }

    /// Prepare an aggregate note-gate envelope. Durations are attack,
    /// decay and release in frames. A zero duration skips its segment.
    pub fn adsr(
        id: ModulationSourceId,
        sample_rate: u32,
        durations: [u32; 3],
        sustain: f32,
        single_trigger: bool,
    ) -> Result<Self, Error> {
        if !(8_000..=192_000).contains(&sample_rate)
            || !(0.0..=1.0).contains(&sustain)
            || durations.iter().any(|&frames| frames > 60 * sample_rate)
        {
            return Err(Error::InvalidParameterValue);
        }
        Ok(Self {
            id,
            output: ModulationOutputId::MAIN,
            initial: SourceState::Adsr(Adsr::new(durations, sustain, single_trigger)),
            sample_rate: f64::from(sample_rate),
        })
    }

    /// Prepare a signed step pattern. Every step lasts exactly the given
    /// number of admitted frames. The signed value -32767 means -1, and
    /// 32767 means +1. A one-step pattern holds its value in every direction.
    pub fn steps(
        id: ModulationSourceId,
        sample_rate: u32,
        pattern: &[i16],
        frames_per_step: u32,
        direction: ModulationStepDirection,
    ) -> Result<Self, Error> {
        if !(8_000..=192_000).contains(&sample_rate) || frames_per_step > 60 * sample_rate {
            return Err(Error::InvalidParameterValue);
        }
        let duration =
            std::num::NonZeroU32::new(frames_per_step).ok_or(Error::InvalidParameterValue)?;
        Ok(Self {
            id,
            output: ModulationOutputId::MAIN,
            initial: SourceState::Steps(Steps::new(pattern, duration, direction)?),
            sample_rate: f64::from(sample_rate),
        })
    }

    /// Prepare a signed step pattern indexed by the host Arrangement beat.
    /// Duration is unsigned Q32.32 quarter-note beats. An unavailable
    /// Arrangement position holds the current output.
    pub fn transport_steps(
        id: ModulationSourceId,
        sample_rate: u32,
        pattern: &[i16],
        beats_per_step: u64,
        direction: ModulationStepDirection,
    ) -> Result<Self, Error> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(Error::InvalidParameterValue);
        }
        let duration =
            std::num::NonZeroU64::new(beats_per_step).ok_or(Error::InvalidParameterValue)?;
        Ok(Self {
            id,
            output: ModulationOutputId::MAIN,
            initial: SourceState::TransportSteps(TransportSteps::new(
                pattern, duration, direction,
            )?),
            sample_rate: f64::from(sample_rate),
        })
    }

    /// Prepare a free-running, seeded sample-and-hold source.
    pub fn random_hold(
        id: ModulationSourceId,
        sample_rate: u32,
        seed: u64,
        rate_hz: f64,
    ) -> Result<Self, Error> {
        let random = RandomHold::new(seed, sample_rate, rate_hz)
            .map_err(|_| Error::InvalidParameterValue)?;
        Ok(Self {
            id,
            output: ModulationOutputId::MAIN,
            initial: SourceState::Random(random),
            sample_rate: f64::from(sample_rate),
        })
    }
}

mod bank;
pub use bank::ModulationSourceBank;

#[cfg(test)]
mod tests;
