//! Fixed-capacity mutable sampler host; PCM stays in the borrowed host scope.
use super::catalog::{
    ATTACK, DECAY, DESCRIPTORS, DETUNE, GAIN, LOOP_LENGTH, LOOP_START, PAN, PARAMETER_COUNT,
    RELEASE, SAMPLE_END, SAMPLE_START, SUSTAIN, TRANSPOSE, parameter_index, parameter_semantics,
};
use super::*;
use hex_lattice::{
    DeviceResources, InstrumentHistoryRetargetPreparation, InstrumentParameterRuntime,
    MAX_INSTRUMENT_AUTOMATION_FRAMES, MutableInstrumentRuntime, NativeInstrumentClock,
    NativeInstrumentEvent, NativeInstrumentOccurrenceGain, NativeInstrumentRuntimeError as Error,
    NativeParameterAutomation, PreparedInstrumentHistoryRetarget, PreparedMutableInstrumentRuntime,
};
use hex_lattice_contract::{
    DeviceContractError, ParameterDescriptor, ParameterId, ParameterSemantics, ParameterValue,
};
use std::{
    any::{Any, TypeId},
    sync::atomic::{AtomicU64, Ordering},
};

pub const MAX_SAMPLER_RUNTIME_VOICES: usize = 32;
pub(super) const MAX_EVENTS: usize = 1024;

static NEXT_CONFIGURATION_ID: AtomicU64 = AtomicU64::new(1);

#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct SamplerConfigurationId(u64);

impl SamplerConfigurationId {
    fn prepare_on_control() -> Result<Self, Invalid> {
        NEXT_CONFIGURATION_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map(Self)
            .map_err(|_| Invalid("sampler configuration identity exhausted"))
    }
}

#[derive(Clone, Copy, PartialEq)]
struct AuthoredPitch {
    root_note: u8,
    transpose_semitones: f64,
    detune_cents: f64,
    output_rate: u32,
}

/// Control-prepared key table and original source binding, without PCM/markers.
/// Not registry admission: the actual owner graph must still qualify separately.
/// Exact-copy equality covers the complete effective law, independently of
/// preparation identity. A retarget proof instead binds one exact prepared
/// candidate (and its clones) while permitting gain/pan, ADS and gated global
/// tuning changes. Authored pitch settings and output rate remain exact.
#[derive(Clone)]
pub struct PreparedSamplerRuntime {
    configuration: SamplerConfigurationId,
    notes: [Option<VoiceCoefficients>; 128],
    source: binding::SourceKey,
    authored_pitch: AuthoredPitch,
    voices: usize,
    tail: u64,
    velocity_amount: f64,
    base_gain: f64,
    pan: f64,
    base_envelope: Envelope,
    base_region: Region,
    base_loop_region: Option<Region>,
    expected_frames: usize,
    gain_modulation: bool,
    max_events: usize,
}

impl PartialEq for PreparedSamplerRuntime {
    fn eq(&self, other: &Self) -> bool {
        if self.configuration == other.configuration
            && self.gain_modulation == other.gain_modulation
        {
            return true;
        }
        let Self {
            configuration: _,
            notes,
            source,
            authored_pitch,
            voices,
            tail,
            velocity_amount,
            base_gain,
            pan,
            base_envelope,
            base_region,
            base_loop_region,
            expected_frames,
            gain_modulation,
            max_events,
        } = self;
        *notes == other.notes
            && *source == other.source
            && *authored_pitch == other.authored_pitch
            && *voices == other.voices
            && *tail == other.tail
            && *velocity_amount == other.velocity_amount
            && *base_gain == other.base_gain
            && *pan == other.pan
            && *base_envelope == other.base_envelope
            && *base_region == other.base_region
            && *base_loop_region == other.base_loop_region
            && *expected_frames == other.expected_frames
            && *gain_modulation == other.gain_modulation
            && *max_events == other.max_events
    }
}

/// An exact candidate authorization, with no PCM, configuration owner or heap child.
#[repr(transparent)]
struct SamplerHistoryRetarget(SamplerConfigurationId);

impl PreparedInstrumentHistoryRetarget for SamplerHistoryRetarget {
    fn declared_fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl PreparedSamplerRuntime {
    /// Admit the borrowed gain parameter on both twins. Closed factory owners
    /// must first reserve and admit the parameter storage for their graph.
    pub fn with_sample_rate_gain(mut self) -> Self {
        self.gain_modulation = true;
        self
    }

    /// Control preparation's exact scalar proof payload allowance. The proof
    /// has no child allocation or source owner; its Box is owned by the graph.
    pub const fn history_retarget_inline_bound() -> usize {
        std::mem::size_of::<SamplerHistoryRetarget>()
    }

    /// Recognize only this provider's actual scalar proof for host storage accounting.
    /// An `as_any` projection or a declared byte count alone grants no authority.
    pub fn history_retarget_inline_bytes(
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> Option<usize> {
        let bytes = Self::history_retarget_inline_bound();
        (proof.type_id() == TypeId::of::<SamplerHistoryRetarget>()
            && proof.as_any().is::<SamplerHistoryRetarget>()
            && std::mem::size_of_val(proof) == bytes
            && proof.declared_fixed_storage_bytes() == bytes
            && bytes == 8)
            .then_some(bytes)
    }

    fn can_retarget_from(&self, previous: &Self) -> bool {
        let Self {
            configuration: _,
            notes,
            source,
            authored_pitch,
            voices,
            tail,
            velocity_amount,
            base_gain: _,
            pan: _,
            base_envelope: _,
            base_region: _,
            base_loop_region: _,
            expected_frames,
            gain_modulation: _,
            max_events,
        } = self;
        *source == previous.source
            && *authored_pitch == previous.authored_pitch
            && *voices == previous.voices
            && *tail == previous.tail
            && *velocity_amount == previous.velocity_amount
            && *expected_frames == previous.expected_frames
            && *max_events == previous.max_events
            && notes
                .iter()
                .zip(&previous.notes)
                .all(|(target, source)| match (target, source) {
                    (None, None) => true,
                    (Some(target), Some(source)) => retarget_coordinates_match(target, source),
                    _ => false,
                })
    }
}

fn retarget_coordinates_match(target: &VoiceCoefficients, source: &VoiceCoefficients) -> bool {
    let VoiceCoefficients {
        expected_frames,
        channels,
        region,
        loop_offsets,
        loop_crossfade,
        reverse,
        step,
        kernel: _,
        channel_gain: _,
        gate,
        envelope,
    } = target;
    *expected_frames == source.expected_frames
        && *channels == source.channels
        && *region == source.region
        && *loop_offsets == source.loop_offsets
        && *loop_crossfade == source.loop_crossfade
        && *reverse == source.reverse
        && (*step == source.step || *gate == GateMode::Gate)
        && *gate == source.gate
        && envelope.release_frames() == source.envelope.release_frames()
}
impl BoundSamplerPlan<'_> {
    /// Prepare all note coefficients and fixed runtime declarations on control.
    /// The caller must still admit the exact resource owners and pair budgets.
    pub fn prepare_runtime_on_control(
        self,
        voices: usize,
        max_events: usize,
        tuning: &hex_lattice::TuningTable,
    ) -> Result<PreparedSamplerRuntime, Invalid> {
        if !(1..=MAX_SAMPLER_RUNTIME_VOICES).contains(&voices)
            || !(1..=MAX_EVENTS).contains(&max_events)
        {
            return Err(Invalid(
                "sampler runtime exceeds fixed voice/event capacity",
            ));
        }
        let mut notes = [None; 128];
        let mut tail = 0;
        for (pitch, entry) in notes.iter_mut().enumerate() {
            // Velocity scaling is applied below using the original linear law.
            *entry = self
                .plan
                .prepare_tuned_voice_coefficients(pitch as u8, 127, tuning)?;
            if let Some(voice) = entry {
                tail = tail.max(match voice.gate {
                    GateMode::Trigger => voice
                        .natural_frames()
                        .ok_or(Invalid("trigger needs a finite end"))?,
                    GateMode::Gate => voice.release_frames(),
                });
            }
        }
        Ok(PreparedSamplerRuntime {
            configuration: SamplerConfigurationId::prepare_on_control()?,
            notes,
            source: self.source,
            authored_pitch: AuthoredPitch {
                root_note: self.plan.settings.root_note,
                transpose_semitones: self.plan.settings.transpose_semitones,
                detune_cents: self.plan.settings.detune_cents,
                output_rate: self.plan.output_rate(),
            },
            voices,
            tail,
            velocity_amount: self.plan.settings.velocity_amount,
            base_gain: self.plan.settings.gain,
            pan: self.plan.settings.pan,
            base_envelope: self.plan.settings.envelope,
            base_region: self.plan.settings.region,
            base_loop_region: match self.plan.settings.mode {
                Mode::Classic { loop_region } => loop_region,
                _ => None,
            },
            expected_frames: self.plan.sample.frames(),
            gain_modulation: false,
            max_events,
        })
    }
}

impl PreparedSamplerRuntime {
    fn base_targets(&self) -> [f32; PARAMETER_COUNT] {
        let extent = self.expected_frames.max(1) as f64;
        let region_len = self
            .base_region
            .end
            .saturating_sub(self.base_region.start)
            .max(1) as f64;
        let (loop_start, loop_length) = match self.base_loop_region {
            Some(loop_region) => (
                (loop_region.start.saturating_sub(self.base_region.start) as f64 / region_len)
                    .clamp(0.0, 1.0) as f32,
                (loop_region.end.saturating_sub(loop_region.start) as f64 / region_len)
                    .clamp(0.0, 1.0) as f32,
            ),
            None => (0.0, 1.0),
        };
        [
            self.base_gain as f32,
            self.pan as f32,
            self.authored_pitch.transpose_semitones as f32,
            self.authored_pitch.detune_cents as f32,
            (self.base_region.start as f64 / extent).clamp(0.0, 1.0) as f32,
            (self.base_region.end as f64 / extent).clamp(0.0, 1.0) as f32,
            loop_start,
            loop_length,
            self.base_envelope.attack_seconds as f32,
            self.base_envelope.decay_seconds as f32,
            self.base_envelope.sustain as f32,
            self.base_envelope.release_seconds as f32,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct MidiState {
    pub(super) channel_pitch_bend: i16,
    pub(super) cc_volume: u8,
    pub(super) cc_expression: u8,
    pub(super) cc_mod_wheel: u8,
    pub(super) cc_timbre: u8,
    pub(super) channel_aftertouch: u8,
    pub(super) sustain_pedal: bool,
}

impl Default for MidiState {
    fn default() -> Self {
        Self {
            channel_pitch_bend: 0,
            cc_volume: 127,
            cc_expression: 127,
            cc_mod_wheel: 0,
            cc_timbre: 0,
            channel_aftertouch: 0,
            sustain_pedal: false,
        }
    }
}

impl MidiState {
    #[inline]
    pub(super) fn effective_step(&self, base_step: u64, voice_pitch_offset_cents: i16) -> u64 {
        if voice_pitch_offset_cents == 0 && self.channel_pitch_bend == 0 {
            return base_step;
        }
        let channel_cents = if self.channel_pitch_bend == 0 {
            0.0
        } else {
            f64::from(self.channel_pitch_bend)
                / if self.channel_pitch_bend < 0 {
                    8192.0
                } else {
                    8191.0
                }
                * 200.0
        };
        let ratio = 2.0_f64.powf((f64::from(voice_pitch_offset_cents) + channel_cents) / 1200.0);
        ((base_step as f64) * ratio).round().max(1.0) as u64
    }

    #[inline]
    pub(super) fn gain_multiplier(&self, voice_timbre: u8) -> f64 {
        let timbre = voice_timbre.max(self.cc_timbre);
        if self.cc_volume == 127
            && self.cc_expression == 127
            && timbre == 0
            && self.channel_aftertouch == 0
        {
            return 1.0;
        }
        let vol = f64::from(self.cc_volume) / 127.0;
        let expr = f64::from(self.cc_expression) / 127.0;
        let timbre_scale = 1.0 + 0.25 * (f64::from(timbre) / 127.0);
        let at_scale = 1.0 + 0.25 * (f64::from(self.channel_aftertouch) / 127.0);
        vol * expr * timbre_scale * at_scale
    }
}

#[derive(Clone, Copy)]
struct ActiveVoice {
    occurrence: u64,
    pitch: u8,
    gain: f64,
    age: u64,
    // Monotonic Q32 source distance, independent of the envelope age. Retuning
    // changes future increments without reinterpreting the rendered prefix.
    position: u128,
    gate: Option<u64>,
    pitch_offset_cents: i16,
    timbre: u8,
    pedal_held: bool,
}
type Active = [Option<ActiveVoice>; MAX_SAMPLER_RUNTIME_VOICES];

/// A stolen releasing voice finishing its forced declick. It keeps its source
/// cursor, envelope age and gate, and adds a linear fade from unity at `start`
/// to zero after `PreparedSamplerRuntime::steal_fade_frames`. Its occurrence
/// gain is frozen at the steal frame because the host no longer addresses it.
#[derive(Clone, Copy)]
struct FadingVoice {
    voice: ActiveVoice,
    start: u64,
    gain: f64,
}
type Fading = [Option<FadingVoice>; MAX_SAMPLER_RUNTIME_VOICES];

/// Outcome of a note-on that found every voice slot occupied.
enum Steal {
    /// The oldest releasing voice moved into the fade pool.
    Stolen,
    /// No voice is releasing: the note-on is refused.
    AllHeld,
    /// Every fade slot is still declicking: the note-on is dropped.
    FadePoolFull,
}

/// Opaque fixed-storage sampler processor created only by its prepared factory.
/// Hosts may identify this concrete audited type; no source owner is stored here.
///
/// Voice stealing (`steals_releasing_voices`): a note-on that finds every one
/// of the prepared `voices` slots occupied steals the oldest releasing voice
/// (greatest age among note-off'd voices, lowest slot on ties) instead of
/// refusing. If every slot is still held, it refuses with
/// `EventCapacityExceeded` as before. The stolen voice moves into the fixed
/// `fading` pool beside the slots, where it continues its release multiplied
/// by a linear declick of about 1 ms, so its output has no step. A fading voice
/// is not an active occurrence: `active_occurrence_ids` omits it, it does not
/// count toward `max_active_occurrences`, it ignores later events, and it takes
/// no occurrence gain beyond the value frozen at the steal frame. Its audio is
/// still mixed, and it retires no later than its own release would have, so
/// `max_release_tail_frames` still bounds quiescence. Steals never cut a
/// sounding voice: when all `MAX_SAMPLER_RUNTIME_VOICES` fade slots are still
/// declicking (more steals than that within one declick), the incoming note-on
/// is dropped as a voice-limit outcome instead. The block still renders, every
/// other event (including every NoteOff) is applied, the dropped occurrence
/// never becomes active, and its later NoteOff is ignored like any unknown
/// occurrence. Admission and processing make the same decision, so a
/// preflighted block never refuses during processing.
pub struct SamplerRuntime {
    prepared: PreparedSamplerRuntime,
    active: Active,
    fading: Fading,
    midi: MidiState,
    last_targets: [f32; PARAMETER_COUNT],
}
impl PreparedMutableInstrumentRuntime for PreparedSamplerRuntime {
    fn parameter_automation_descriptors(&self) -> &[ParameterDescriptor] {
        if self.gain_modulation {
            &DESCRIPTORS
        } else {
            &[]
        }
    }
    fn parameter_modulation_semantics(&self, parameter: ParameterId) -> Option<ParameterSemantics> {
        self.gain_modulation
            .then(|| parameter_semantics(parameter))
            .flatten()
    }
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }
    fn supports_occurrence_gain(&self) -> bool {
        true
    }
    fn resource_factory_inline_bytes(&self) -> Option<usize> {
        Some(std::mem::size_of::<Self>())
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<SamplerRuntime>()
    }
    fn max_events_per_block(&self) -> usize {
        self.max_events
    }
    fn max_active_occurrences(&self) -> usize {
        self.voices
    }
    fn max_release_tail_frames(&self) -> u64 {
        self.tail
    }
    fn steals_releasing_voices(&self) -> bool {
        true
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(SamplerRuntime {
            prepared: self.clone(),
            active: [None; MAX_SAMPLER_RUNTIME_VOICES],
            fading: [None; MAX_SAMPLER_RUNTIME_VOICES],
            midi: MidiState::default(),
            last_targets: self.base_targets(),
        })
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn prepare_history_retarget_target(
        &self,
    ) -> Result<InstrumentHistoryRetargetPreparation, DeviceContractError> {
        Ok(InstrumentHistoryRetargetPreparation::Prepared(Box::new(
            SamplerHistoryRetarget(self.configuration),
        )))
    }
}
impl PreparedSamplerRuntime {
    /// Forced declick length for a stolen voice: 1 ms at the output rate,
    /// rounded, and at least one frame.
    const fn steal_fade_frames(&self) -> u64 {
        let frames = (self.authored_pitch.output_rate as u64 + 500) / 1000;
        if frames == 0 { 1 } else { frames }
    }
}

mod modulation;
mod traits;
use modulation::{DYNAMIC_CURSOR_MASK, FrameModulation};

#[cfg(test)]
mod tests;
