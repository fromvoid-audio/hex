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

impl SamplerRuntime {
    /// Frames this voice contributes before source, gate, or optional steal
    /// fade retirement. Preflight has validated the entire enclosing span.
    fn sounding_frames(
        &self,
        voice: &ActiveVoice,
        limit: usize,
        fade_start: Option<u64>,
    ) -> Result<usize, Error> {
        let Some(note) = self.prepared.notes[usize::from(voice.pitch)] else {
            return Ok(0);
        };
        let mut count = limit as u128;
        if note.loop_offsets.is_none() {
            let end = (note.region.end - note.region.start) as u128 * ONE;
            let remaining = end.saturating_sub(voice.position);
            let step = u128::from(note.step);
            let whole = remaining
                .checked_div(step)
                .ok_or(Error::InvalidEventBatch)?;
            let partial = remaining
                .checked_rem(step)
                .ok_or(Error::InvalidEventBatch)?
                != 0;
            count = count.min(
                whole
                    .checked_add(u128::from(partial))
                    .ok_or(Error::InvalidEventBatch)?,
            );
        }
        if note.gate == GateMode::Gate
            && let Some(gate) = voice.gate
        {
            count = count.min(u128::from(
                note.release_frames()
                    .saturating_sub(voice.age.saturating_sub(gate)),
            ));
        }
        if let Some(start) = fade_start {
            count = count.min(u128::from(
                self.prepared
                    .steal_fade_frames()
                    .saturating_sub(voice.age.saturating_sub(start)),
            ));
        }
        // count is bounded above by the original usize limit.
        Ok(count as usize)
    }

    /// Advance one voice by `frames`; `Ok(false)` means it has retired.
    fn advance_voice(&self, voice: &mut ActiveVoice, frames: u64) -> Result<bool, Error> {
        let Some(note) = self.prepared.notes[usize::from(voice.pitch)] else {
            return Ok(false);
        };
        voice.age = voice
            .age
            .checked_add(frames)
            .ok_or(Error::InvalidEventBatch)?;
        let distance = u128::from(note.step)
            .checked_mul(u128::from(frames))
            .ok_or(Error::InvalidEventBatch)?;
        voice.position = voice
            .position
            .checked_add(distance)
            .ok_or(Error::InvalidEventBatch)?;
        let source_ended = note.loop_offsets.is_none()
            && voice.position >= (note.region.end - note.region.start) as u128 * ONE;
        let release_ended = match (note.gate, voice.gate) {
            (GateMode::Gate, Some(gate)) => {
                // A looping release still needs a representable deadline,
                // as before cursor-based source exhaustion was introduced.
                if note.loop_offsets.is_some() && gate.checked_add(note.release_frames()).is_none()
                {
                    return Err(Error::InvalidEventBatch);
                }
                voice
                    .age
                    .checked_sub(gate)
                    .ok_or(Error::InvalidEventBatch)?
                    >= note.release_frames()
            }
            _ => false,
        };
        Ok(!(source_ended || release_ended))
    }
    fn advance(&self, active: &mut Active, fading: &mut Fading, frames: u64) -> Result<(), Error> {
        for entry in active.iter_mut() {
            if let Some(voice) = entry
                && !self.advance_voice(voice, frames)?
            {
                *entry = None;
            }
        }
        let fade = self.prepared.steal_fade_frames();
        for entry in fading.iter_mut() {
            if let Some(fading) = entry
                && (!self.advance_voice(&mut fading.voice, frames)?
                    || fading.voice.age.saturating_sub(fading.start) >= fade)
            {
                *entry = None;
            }
        }
        Ok(())
    }
    /// Move the oldest releasing voice into a free fade slot, freeing its
    /// voice slot. Changes nothing unless it returns `Steal::Stolen`.
    fn steal(
        &self,
        active: &mut Active,
        fading: &mut Fading,
        gain_at: &dyn Fn(u64) -> f64,
    ) -> Steal {
        let mut oldest: Option<(usize, u64)> = None;
        for (index, entry) in active[..self.prepared.voices].iter().enumerate() {
            if let Some(voice) = entry
                && voice.gate.is_some()
                && oldest.is_none_or(|(_, age)| voice.age > age)
            {
                oldest = Some((index, voice.age));
            }
        }
        let Some((index, _)) = oldest else {
            return Steal::AllHeld;
        };
        let Some(target) = fading.iter().position(Option::is_none) else {
            return Steal::FadePoolFull;
        };
        let Some(voice) = active[index].take() else {
            return Steal::AllHeld;
        };
        fading[target] = Some(FadingVoice {
            voice,
            start: voice.age,
            gain: gain_at(voice.occurrence),
        });
        Steal::Stolen
    }
    fn event(
        &self,
        active: &mut Active,
        fading: &mut Fading,
        event: NativeInstrumentEvent,
        gain_at: &dyn Fn(u64) -> f64,
    ) -> Result<(), Error> {
        match event {
            NativeInstrumentEvent::NotePressure { .. } => {
                return Err(Error::UnsupportedNotePressure);
            }
            NativeInstrumentEvent::ControlChange { .. }
            | NativeInstrumentEvent::PitchBend { .. }
            | NativeInstrumentEvent::NotePitchBend { .. }
            | NativeInstrumentEvent::NoteTimbre { .. } => {}
            NativeInstrumentEvent::Expression { expression, .. } => {
                if !expression.is_valid() {
                    return Err(Error::InvalidEventBatch);
                }
            }
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                for voice in active
                    .iter_mut()
                    .flatten()
                    .filter(|voice| voice.occurrence == occurrence_id)
                {
                    if voice.gate.is_none() {
                        voice.gate = Some(voice.age);
                    }
                }
            }
            NativeInstrumentEvent::NoteOn {
                occurrence_id,
                pitch,
                velocity,
                ..
            } => {
                if pitch > 127
                    || velocity > 127
                    || occurrence_id == 0
                    || active
                        .iter()
                        .flatten()
                        .any(|voice| voice.occurrence == occurrence_id)
                {
                    return Err(Error::InvalidEventBatch);
                }
                if velocity == 0 || self.prepared.notes[usize::from(pitch)].is_none() {
                    return Ok(());
                }
                if !active[..self.prepared.voices].iter().any(Option::is_none) {
                    match self.steal(active, fading, gain_at) {
                        Steal::Stolen => {}
                        Steal::FadePoolFull => return self.advance(active, fading, 0),
                        Steal::AllHeld => return Err(Error::EventCapacityExceeded),
                    }
                }
                let entry = active[..self.prepared.voices]
                    .iter_mut()
                    .find(|entry| entry.is_none())
                    .ok_or(Error::EventCapacityExceeded)?;
                *entry = Some(ActiveVoice {
                    occurrence: occurrence_id,
                    pitch,
                    gain: 1.0 - self.prepared.velocity_amount
                        + self.prepared.velocity_amount * f64::from(velocity) / 127.0,
                    age: 0,
                    position: 0,
                    gate: None,
                });
            }
        }
        self.advance(active, fading, 0)
    }
}
impl MutableInstrumentRuntime for SamplerRuntime {
    fn parameter_automation(&mut self) -> Option<&mut dyn InstrumentParameterRuntime> {
        self.prepared.gain_modulation.then_some(self)
    }
    fn parameter_automation_readonly(&self) -> Option<&dyn InstrumentParameterRuntime> {
        self.prepared.gain_modulation.then_some(self)
    }
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }
    fn active_occurrence_ids(&self, output: &mut [u64]) -> Result<usize, Error> {
        let count = self.active.iter().flatten().count();
        if output.len() < count {
            return Err(Error::ActiveOccurrenceCapacityExceeded);
        }
        for (slot, voice) in output[..count].iter_mut().zip(self.active.iter().flatten()) {
            *slot = voice.occurrence;
        }
        output[..count].sort_unstable();
        Ok(count)
    }
    fn supports_occurrence_gain(&self) -> bool {
        true
    }
    fn preflight_occurrence_gains(
        &self,
        events: &[NativeInstrumentEvent],
        gains: &[NativeInstrumentOccurrenceGain],
    ) -> Result<(), Error> {
        if events.len() > self.prepared.max_events
            || gains.len() > MAX_EVENTS + MAX_SAMPLER_RUNTIME_VOICES
        {
            return Err(Error::EventCapacityExceeded);
        }
        // Mark the bounded union once instead of scanning the entire event
        // batch for every gain (up to 1,024 incoming identities). The caller
        // already proved sorted unique gain IDs. This is local validation
        // scratch, not retained voice/gain state.
        let mut known = [false; MAX_EVENTS + MAX_SAMPLER_RUNTIME_VOICES];
        let mut mark = |id| {
            if let Ok(index) = gains.binary_search_by_key(&id, |gain| gain.occurrence_id()) {
                known[index] = true;
            }
        };
        for voice in self.active.iter().flatten() {
            mark(voice.occurrence);
        }
        for event in events {
            if let NativeInstrumentEvent::NoteOn { occurrence_id, .. } = event {
                mark(*occurrence_id);
            }
        }
        if !known[..gains.len()].iter().all(|known| *known) {
            return Err(Error::InvalidOccurrenceGain);
        }
        Ok(())
    }
    fn process_block_with_clock_and_occurrence_gains(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        gains: &[NativeInstrumentOccurrenceGain],
        resources: &dyn DeviceResources,
    ) {
        self.process_with_gains::<false>(events, frames, gains, resources, None, 0);
    }
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frames: usize,
        resources: &dyn DeviceResources,
    ) -> Result<(), Error> {
        if events.len() > self.prepared.max_events {
            return Err(Error::EventCapacityExceeded);
        }
        // Resolve the original slot from actual inventory, not a same-index substitute.
        if !(0..resources
            .len()
            .min(hex_lattice_contract::MAX_DEVICE_ASSET_REFERENCES))
            .filter_map(|index| resources.get(index))
            .any(|view| self.prepared.source.matches(view))
        {
            return Err(Error::ResourceUnavailable);
        }
        if events.is_empty() && self.is_quiescent() {
            return Ok(());
        }
        let mut active = self.active;
        let mut fading = self.fading;
        let mut cursor = 0;
        // Gain values never affect admission; only processing freezes them.
        let unity = |_| 1.0;
        for &event in events {
            let offset = event.frame_offset();
            if offset < cursor || offset >= frames {
                return Err(Error::InvalidEventBatch);
            }
            self.advance(&mut active, &mut fading, (offset - cursor) as u64)?;
            self.event(&mut active, &mut fading, event, &unity)?;
            cursor = offset;
        }
        self.advance(&mut active, &mut fading, (frames - cursor) as u64)
    }
    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        resources: &dyn DeviceResources,
    ) {
        self.process_with_gains::<false>(events, frames, &[], resources, None, 0);
    }
    fn reset(&mut self) {
        self.active.fill(None);
        self.fading.fill(None);
        self.last_targets = self.prepared.base_targets();
    }
    fn is_quiescent(&self) -> bool {
        self.active.iter().all(Option::is_none) && self.fading.iter().all(Option::is_none)
    }
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| self.prepared == previous.prepared)
    }
    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(previous) = previous
            .as_any()
            .downcast_ref::<Self>()
            .filter(|previous| self.prepared == previous.prepared)
        {
            self.active = previous.active;
            self.fading = previous.fading;
            self.last_targets = previous.last_targets;
        }
    }
    fn can_retarget_history_from(
        &self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> bool {
        if previous.type_id() != TypeId::of::<Self>()
            || PreparedSamplerRuntime::history_retarget_inline_bytes(proof).is_none()
        {
            return false;
        }
        let (Some(previous), Some(proof)) = (
            previous.as_any().downcast_ref::<Self>(),
            proof.as_any().downcast_ref::<SamplerHistoryRetarget>(),
        ) else {
            return false;
        };
        self.prepared.configuration == proof.0
            && self.prepared.can_retarget_from(&previous.prepared)
    }
    fn retarget_history_from(
        &mut self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) {
        // Gain/pan and ADS use the carried envelope age/gate. Gated tuning
        // uses the carried source cursor and the candidate step from now on.
        // Authored pitch, effective coordinates, velocity and release stay exact.
        if self.can_retarget_history_from(previous, proof)
            && let Some(previous) = previous.as_any().downcast_ref::<Self>()
        {
            self.active = previous.active;
            self.fading = previous.fading;
            self.last_targets = previous.last_targets;
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl InstrumentParameterRuntime for SamplerRuntime {
    fn descriptors(&self) -> &[ParameterDescriptor] {
        &DESCRIPTORS
    }
    fn parameter_modulation_semantics(&self, parameter: ParameterId) -> Option<ParameterSemantics> {
        parameter_semantics(parameter)
    }
    fn copy_targets(&self, output: &mut [Option<ParameterValue>]) -> Result<(), Error> {
        if output.len() != PARAMETER_COUNT {
            return Err(Error::InvalidParameterAutomation);
        }
        for (slot, value) in output.iter_mut().zip(self.last_targets) {
            *slot = Some(ParameterValue::Float(value));
        }
        Ok(())
    }
    fn preflight(
        &self,
        events: &[NativeInstrumentEvent],
        frames: usize,
        _: Option<&dyn NativeInstrumentClock>,
        _: &[NativeInstrumentOccurrenceGain],
        values: &dyn NativeParameterAutomation,
        _: &dyn DeviceResources,
    ) -> Result<(), Error> {
        let mask = Self::present_mask(values);
        if (mask & DYNAMIC_CURSOR_MASK) == 0 || frames == 0 {
            return Ok(());
        }
        if frames > MAX_INSTRUMENT_AUTOMATION_FRAMES {
            return Err(Error::InvalidParameterAutomation);
        }
        if events.is_empty() && self.is_quiescent() {
            return Ok(());
        }
        let mut active = self.active;
        let mut fading = self.fading;
        let fade = self.prepared.steal_fade_frames();
        let unity = |_| 1.0;
        let mut next = 0;
        for offset in 0..frames {
            let fm = FrameModulation::sample(&self.prepared, Some(values), mask, offset);
            while events
                .get(next)
                .is_some_and(|event| event.frame_offset() == offset)
            {
                self.event_with_modulation(
                    &mut active,
                    &mut fading,
                    events[next],
                    &unity,
                    Some(&fm),
                )?;
                next += 1;
            }
            self.advance_dynamic_frame(&mut active, &mut fading, &fm, fade);
        }
        Ok(())
    }
    fn process(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _: Option<&dyn NativeInstrumentClock>,
        gains: &[NativeInstrumentOccurrenceGain],
        values: &dyn NativeParameterAutomation,
        resources: &dyn DeviceResources,
    ) {
        let present_mask = Self::present_mask(values);
        if present_mask == 0 {
            self.process_with_gains::<false>(events, frames, gains, resources, None, 0);
            return;
        }
        if frames.len() > MAX_INSTRUMENT_AUTOMATION_FRAMES {
            return;
        }
        self.process_with_gains::<true>(
            events,
            frames,
            gains,
            resources,
            Some(values),
            present_mask,
        );
    }
}

const GAIN_PAN_MASK: u16 = (1 << 0) | (1 << 1);
const PITCH_MASK: u16 = (1 << 2) | (1 << 3);
const REGION_MASK: u16 = (1 << 4) | (1 << 5) | (1 << 6) | (1 << 7);
const ENV_MASK: u16 = (1 << 8) | (1 << 9) | (1 << 10) | (1 << 11);
const DYNAMIC_CURSOR_MASK: u16 =
    (1 << 2) | (1 << 3) | (1 << 4) | (1 << 5) | (1 << 6) | (1 << 7) | (1 << 11);

#[derive(Clone, Copy)]
struct RegionModulation {
    sample_start: Option<f32>,
    sample_end: Option<f32>,
    loop_start: Option<f32>,
    loop_length: Option<f32>,
}

/// Global modulated sampler parameters evaluated once per output frame rather
/// than once per voice per frame.
#[derive(Clone, Copy)]
struct FrameModulation {
    gain_pan: Option<[f64; 2]>,
    pitch_factor: Option<f64>,
    region: Option<RegionModulation>,
    envelope: Option<envelope::PreparedEnvelope>,
}

impl FrameModulation {
    fn sample(
        prepared: &PreparedSamplerRuntime,
        values: Option<&dyn NativeParameterAutomation>,
        mask: u16,
        offset: usize,
    ) -> Self {
        let gain_pan = if (mask & GAIN_PAN_MASK) != 0 {
            let gain = SamplerRuntime::read_param(values, mask, GAIN, offset)
                .map_or(prepared.base_gain, |v| f64::from(v).clamp(0.0, 4.0));
            let pan = SamplerRuntime::read_param(values, mask, PAN, offset)
                .map_or(prepared.pan, |v| f64::from(v).clamp(-1.0, 1.0));
            let left = 1.0 - pan.max(0.0);
            let right = 1.0 + pan.min(0.0);
            Some([gain * left, gain * right])
        } else {
            None
        };

        let pitch_factor = if (mask & PITCH_MASK) != 0 {
            let transpose = SamplerRuntime::read_param(values, mask, TRANSPOSE, offset)
                .map_or(prepared.authored_pitch.transpose_semitones, |v| {
                    f64::from(v).clamp(-48.0, 48.0)
                });
            let detune = SamplerRuntime::read_param(values, mask, DETUNE, offset)
                .map_or(prepared.authored_pitch.detune_cents, |v| {
                    f64::from(v).clamp(-100.0, 100.0)
                });
            let delta_semitones = (transpose - prepared.authored_pitch.transpose_semitones)
                + (detune - prepared.authored_pitch.detune_cents) / 100.0;
            Some((delta_semitones / 12.0).exp2())
        } else {
            None
        };

        let region = if (mask & REGION_MASK) != 0 {
            Some(RegionModulation {
                sample_start: SamplerRuntime::read_param(values, mask, SAMPLE_START, offset),
                sample_end: SamplerRuntime::read_param(values, mask, SAMPLE_END, offset),
                loop_start: SamplerRuntime::read_param(values, mask, LOOP_START, offset),
                loop_length: SamplerRuntime::read_param(values, mask, LOOP_LENGTH, offset),
            })
        } else {
            None
        };

        let envelope = if (mask & ENV_MASK) != 0 {
            let env = Envelope {
                attack_seconds: SamplerRuntime::read_param(values, mask, ATTACK, offset)
                    .map_or(prepared.base_envelope.attack_seconds, |v| {
                        f64::from(v).clamp(0.0, 60.0)
                    }),
                decay_seconds: SamplerRuntime::read_param(values, mask, DECAY, offset)
                    .map_or(prepared.base_envelope.decay_seconds, |v| {
                        f64::from(v).clamp(0.0, 60.0)
                    }),
                sustain: SamplerRuntime::read_param(values, mask, SUSTAIN, offset)
                    .map_or(prepared.base_envelope.sustain, |v| {
                        f64::from(v).clamp(0.0, 1.0)
                    }),
                release_seconds: SamplerRuntime::read_param(values, mask, RELEASE, offset)
                    .map_or(prepared.base_envelope.release_seconds, |v| {
                        f64::from(v).clamp(0.0, 60.0)
                    }),
            };
            env.prepare(prepared.authored_pitch.output_rate)
                .ok()
                .map(|prepared_env| prepared_env.clamp_release_frames(prepared.tail))
        } else {
            None
        };

        Self {
            gain_pan,
            pitch_factor,
            region,
            envelope,
        }
    }

    fn apply_to(
        &self,
        base: &VoiceCoefficients,
        velocity_gain: f64,
        base_channel_gain: [f64; 2],
    ) -> VoiceCoefficients {
        let mut note = *base;
        if let Some([left, right]) = self.gain_pan {
            note.channel_gain = [left * velocity_gain, right * velocity_gain];
        } else {
            note.channel_gain = base_channel_gain;
        }
        if let Some(factor) = self.pitch_factor {
            let scaled = (base.step as f64 * factor).round();
            note.step = (scaled as u64).max(1);
        }
        if let Some(RegionModulation {
            sample_start,
            sample_end,
            loop_start,
            loop_length,
        }) = self.region
        {
            let extent = base.expected_frames.max(1);
            let mut region = base.region;
            if let Some(s) = sample_start {
                region.start = ((f64::from(s).clamp(0.0, 1.0) * extent as f64).round() as usize)
                    .min(extent.saturating_sub(1));
            }
            if let Some(e) = sample_end {
                region.end = ((f64::from(e).clamp(0.0, 1.0) * extent as f64).round() as usize)
                    .clamp(1, extent);
            }
            if region.start >= region.end {
                region.start = region.end.saturating_sub(1);
            }
            note.region = region;
            let region_len = region.end.saturating_sub(region.start).max(1);
            if let Some((base_ls, base_le)) = base.loop_offsets {
                let mut ls = base_ls.min(region_len.saturating_sub(1));
                let mut loop_len = base_le.saturating_sub(base_ls).max(1);
                if let Some(s) = loop_start {
                    ls = ((f64::from(s).clamp(0.0, 1.0) * region_len as f64).round() as usize)
                        .min(region_len.saturating_sub(1));
                }
                if let Some(l) = loop_length {
                    loop_len = ((f64::from(l).clamp(0.0, 1.0) * region_len as f64).round()
                        as usize)
                        .max(1);
                }
                let le = ls.saturating_add(loop_len).min(region_len);
                let ls = ls.min(le.saturating_sub(1));
                let le = le.max(ls.saturating_add(1));
                note.loop_offsets = Some((ls, le));
                note.loop_crossfade = base.loop_crossfade.min((le - ls).min(ls));
            }
        }
        if let Some(prepared_env) = self.envelope {
            note.envelope = prepared_env;
        }
        note
    }
}

impl SamplerRuntime {
    fn present_mask(values: &dyn NativeParameterAutomation) -> u16 {
        let mut mask: u16 = 0;
        for &id in values.parameter_ids() {
            if let Some(index) = parameter_index(id) {
                mask |= 1_u16 << index;
            }
        }
        mask
    }

    fn read_param(
        values: Option<&dyn NativeParameterAutomation>,
        mask: u16,
        id: ParameterId,
        offset: usize,
    ) -> Option<f32> {
        let index = parameter_index(id)?;
        if (mask & (1_u16 << index)) == 0 {
            return None;
        }
        match values?.value_at(id, offset) {
            Some(ParameterValue::Float(v)) if v.is_finite() => Some(v),
            _ => None,
        }
    }

    fn voice_retired(cursor: &ActiveVoice, note: &VoiceCoefficients) -> bool {
        let source_ended = note.loop_offsets.is_none()
            && cursor.position >= (note.region.end - note.region.start) as u128 * ONE;
        let release_ended = note.gate == GateMode::Gate
            && cursor
                .gate
                .is_some_and(|gate| cursor.age.saturating_sub(gate) >= note.release_frames());
        source_ended || release_ended
    }

    fn retire_zero_with_modulation(
        &self,
        active: &mut Active,
        fading: &mut Fading,
        fm: &FrameModulation,
    ) {
        let fade = self.prepared.steal_fade_frames();
        for entry in active.iter_mut() {
            let Some(voice) = *entry else { continue };
            let Some(base_note) = self.prepared.notes[usize::from(voice.pitch)] else {
                *entry = None;
                continue;
            };
            let note = fm.apply_to(&base_note, voice.gain, base_note.channel_gain);
            if Self::voice_retired(&voice, &note) {
                *entry = None;
            }
        }
        for entry in fading.iter_mut() {
            let Some(item) = *entry else { continue };
            let Some(base_note) = self.prepared.notes[usize::from(item.voice.pitch)] else {
                *entry = None;
                continue;
            };
            let note = fm.apply_to(&base_note, item.voice.gain, base_note.channel_gain);
            if Self::voice_retired(&item.voice, &note)
                || item.voice.age.saturating_sub(item.start) >= fade
            {
                *entry = None;
            }
        }
    }

    fn event_with_modulation(
        &self,
        active: &mut Active,
        fading: &mut Fading,
        event: NativeInstrumentEvent,
        gain_at: &dyn Fn(u64) -> f64,
        fm: Option<&FrameModulation>,
    ) -> Result<(), Error> {
        match event {
            NativeInstrumentEvent::NotePressure { .. } => {
                return Err(Error::UnsupportedNotePressure);
            }
            NativeInstrumentEvent::ControlChange { .. }
            | NativeInstrumentEvent::PitchBend { .. }
            | NativeInstrumentEvent::NotePitchBend { .. }
            | NativeInstrumentEvent::NoteTimbre { .. } => {}
            NativeInstrumentEvent::Expression { expression, .. } => {
                if !expression.is_valid() {
                    return Err(Error::InvalidEventBatch);
                }
            }
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                for voice in active
                    .iter_mut()
                    .flatten()
                    .filter(|voice| voice.occurrence == occurrence_id)
                {
                    if voice.gate.is_none() {
                        voice.gate = Some(voice.age);
                    }
                }
            }
            NativeInstrumentEvent::NoteOn {
                occurrence_id,
                pitch,
                velocity,
                ..
            } => {
                if pitch > 127
                    || velocity > 127
                    || occurrence_id == 0
                    || active
                        .iter()
                        .flatten()
                        .any(|voice| voice.occurrence == occurrence_id)
                {
                    return Err(Error::InvalidEventBatch);
                }
                if velocity == 0 || self.prepared.notes[usize::from(pitch)].is_none() {
                    return Ok(());
                }
                if !active[..self.prepared.voices].iter().any(Option::is_none) {
                    match self.steal(active, fading, gain_at) {
                        Steal::Stolen => {}
                        Steal::FadePoolFull => {
                            if let Some(fm) = fm {
                                self.retire_zero_with_modulation(active, fading, fm);
                                return Ok(());
                            }
                            return self.advance(active, fading, 0);
                        }
                        Steal::AllHeld => return Err(Error::EventCapacityExceeded),
                    }
                }
                let entry = active[..self.prepared.voices]
                    .iter_mut()
                    .find(|entry| entry.is_none())
                    .ok_or(Error::EventCapacityExceeded)?;
                *entry = Some(ActiveVoice {
                    occurrence: occurrence_id,
                    pitch,
                    gain: 1.0 - self.prepared.velocity_amount
                        + self.prepared.velocity_amount * f64::from(velocity) / 127.0,
                    age: 0,
                    position: 0,
                    gate: None,
                });
            }
        }
        if let Some(fm) = fm {
            self.retire_zero_with_modulation(active, fading, fm);
            Ok(())
        } else {
            self.advance(active, fading, 0)
        }
    }

    fn advance_dynamic_frame(
        &self,
        active: &mut Active,
        fading: &mut Fading,
        fm: &FrameModulation,
        fade: u64,
    ) {
        for entry in active.iter_mut() {
            let Some(mut cursor) = *entry else { continue };
            let Some(base_note) = self.prepared.notes[usize::from(cursor.pitch)] else {
                *entry = None;
                continue;
            };
            let note = fm.apply_to(&base_note, cursor.gain, base_note.channel_gain);
            if Self::voice_retired(&cursor, &note) {
                *entry = None;
                continue;
            }
            cursor.age = cursor.age.saturating_add(1);
            cursor.position = cursor.position.saturating_add(u128::from(note.step));
            *entry = (!Self::voice_retired(&cursor, &note)).then_some(cursor);
        }
        for entry in fading.iter_mut() {
            let Some(mut item) = *entry else { continue };
            let Some(base_note) = self.prepared.notes[usize::from(item.voice.pitch)] else {
                *entry = None;
                continue;
            };
            let note = fm.apply_to(&base_note, item.voice.gain, base_note.channel_gain);
            if Self::voice_retired(&item.voice, &note)
                || item.voice.age.saturating_sub(item.start) >= fade
            {
                *entry = None;
                continue;
            }
            item.voice.age = item.voice.age.saturating_add(1);
            item.voice.position = item.voice.position.saturating_add(u128::from(note.step));
            *entry = (!Self::voice_retired(&item.voice, &note)
                && item.voice.age.saturating_sub(item.start) < fade)
                .then_some(item);
        }
    }

    fn process_with_gains<const MODULATED: bool>(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        gains: &[NativeInstrumentOccurrenceGain],
        resources: &dyn DeviceResources,
        values: Option<&dyn NativeParameterAutomation>,
        mask: u16,
    ) {
        let pcm = (0..resources
            .len()
            .min(hex_lattice_contract::MAX_DEVICE_ASSET_REFERENCES))
            .filter_map(|index| resources.get(index))
            .find(|view| self.prepared.source.matches(*view));
        let Some(view) = pcm else { return };
        if events.is_empty() && self.is_quiescent() {
            if MODULATED && !frames.is_empty() {
                self.record_last_targets(values, mask, frames.len() - 1);
            }
            return;
        }
        let mut active = self.active;
        let mut fading = self.fading;
        let fade_u64 = self.prepared.steal_fade_frames();
        let fade = fade_u64 as f64;
        let mut next = 0;

        if MODULATED {
            // Frame-major modulated pass: evaluates global parameters once per
            // sample across all active and steal-fading voices, bounds RELEASE
            // by `max_release_tail_frames()`, advances fading cursors with the
            // exact modulated `note.step`, and never aborts mid-block on voice
            // limit so subsequent NoteOffs and `self.active`/`self.fading`
            // always commit cleanly.
            for (offset, frame) in frames.iter_mut().enumerate() {
                let fm = FrameModulation::sample(&self.prepared, values, mask, offset);
                let gain_at = |occurrence: u64| {
                    gains
                        .binary_search_by_key(&occurrence, |gain| gain.occurrence_id())
                        .ok()
                        .map_or(1.0, |index| gains[index].value_at_offset(offset as u64))
                };
                while events
                    .get(next)
                    .is_some_and(|event| event.frame_offset() == offset)
                {
                    let _ = self.event_with_modulation(
                        &mut active,
                        &mut fading,
                        events[next],
                        &gain_at,
                        Some(&fm),
                    );
                    next += 1;
                }
                for entry in &mut active {
                    let Some(mut cursor) = *entry else { continue };
                    let Some(base_note) = self.prepared.notes[usize::from(cursor.pitch)] else {
                        *entry = None;
                        continue;
                    };
                    let base_gain = [
                        base_note.channel_gain[0] * cursor.gain,
                        base_note.channel_gain[1] * cursor.gain,
                    ];
                    let note = fm.apply_to(&base_note, cursor.gain, base_gain);
                    if Self::voice_retired(&cursor, &note) {
                        *entry = None;
                        continue;
                    }
                    let gain = gain_at(cursor.occurrence);
                    Self::mix_prepared(frame, view.pcm, &cursor, gain, &note);
                    cursor.age = cursor.age.saturating_add(1);
                    cursor.position = cursor.position.saturating_add(u128::from(note.step));
                    *entry = (!Self::voice_retired(&cursor, &note)).then_some(cursor);
                }
                for entry in &mut fading {
                    let Some(mut item) = *entry else { continue };
                    let Some(base_note) = self.prepared.notes[usize::from(item.voice.pitch)] else {
                        *entry = None;
                        continue;
                    };
                    let base_gain = [
                        base_note.channel_gain[0] * item.voice.gain,
                        base_note.channel_gain[1] * item.voice.gain,
                    ];
                    let note = fm.apply_to(&base_note, item.voice.gain, base_gain);
                    if Self::voice_retired(&item.voice, &note)
                        || item.voice.age.saturating_sub(item.start) >= fade_u64
                    {
                        *entry = None;
                        continue;
                    }
                    let declick = 1.0 - item.voice.age.saturating_sub(item.start) as f64 / fade;
                    Self::mix_prepared(frame, view.pcm, &item.voice, item.gain * declick, &note);
                    item.voice.age = item.voice.age.saturating_add(1);
                    item.voice.position = item.voice.position.saturating_add(u128::from(note.step));
                    *entry = (!Self::voice_retired(&item.voice, &note)
                        && item.voice.age.saturating_sub(item.start) < fade_u64)
                        .then_some(item);
                }
            }
        } else {
            let mut offset = 0;
            while offset < frames.len() {
                let gain_at = |occurrence: u64| {
                    gains
                        .binary_search_by_key(&occurrence, |gain| gain.occurrence_id())
                        .ok()
                        .map_or(1.0, |index| gains[index].value_at_offset(offset as u64))
                };
                while events
                    .get(next)
                    .is_some_and(|event| event.frame_offset() == offset)
                {
                    if self
                        .event(&mut active, &mut fading, events[next], &gain_at)
                        .is_err()
                    {
                        return;
                    }
                    next += 1;
                }
                let end = events
                    .get(next)
                    .map_or(frames.len(), |event| event.frame_offset());
                if end <= offset || end > frames.len() {
                    return;
                }
                let span = end - offset;
                for entry in &mut active {
                    let Some(voice) = *entry else { continue };
                    let Some(base_note) = self.prepared.notes[usize::from(voice.pitch)] else {
                        *entry = None;
                        continue;
                    };
                    let mut note = base_note;
                    for channel_gain in &mut note.channel_gain {
                        *channel_gain *= voice.gain;
                    }
                    let gain_index = gains
                        .binary_search_by_key(&voice.occurrence, |gain| gain.occurrence_id())
                        .ok();
                    let Ok(count) = self.sounding_frames(&voice, span, None) else {
                        return;
                    };
                    let mut final_voice = voice;
                    let Ok(alive) = self.advance_voice(&mut final_voice, span as u64) else {
                        return;
                    };
                    let mut cursor = voice;
                    for (relative, frame) in frames[offset..offset + count].iter_mut().enumerate() {
                        let gain = gain_index.map_or(1.0, |index| {
                            gains[index].value_at_offset((offset + relative) as u64)
                        });
                        Self::mix_prepared(frame, view.pcm, &cursor, gain, &note);
                        cursor.age = cursor.age.wrapping_add(1);
                        cursor.position = cursor.position.wrapping_add(u128::from(note.step));
                    }
                    *entry = alive.then_some(final_voice);
                }
                for entry in &mut fading {
                    let Some(item) = *entry else { continue };
                    let Some(base_note) = self.prepared.notes[usize::from(item.voice.pitch)] else {
                        *entry = None;
                        continue;
                    };
                    let mut note = base_note;
                    for channel_gain in &mut note.channel_gain {
                        *channel_gain *= item.voice.gain;
                    }
                    let Ok(count) = self.sounding_frames(&item.voice, span, Some(item.start))
                    else {
                        return;
                    };
                    let mut final_item = item;
                    let Ok(alive) = self.advance_voice(&mut final_item.voice, span as u64) else {
                        return;
                    };
                    let mut cursor = item.voice;
                    for frame in &mut frames[offset..offset + count] {
                        let declick = 1.0 - cursor.age.saturating_sub(item.start) as f64 / fade;
                        Self::mix_prepared(frame, view.pcm, &cursor, item.gain * declick, &note);
                        cursor.age = cursor.age.wrapping_add(1);
                        cursor.position = cursor.position.wrapping_add(u128::from(note.step));
                    }
                    *entry = (alive && final_item.voice.age.saturating_sub(item.start) < fade_u64)
                        .then_some(final_item);
                }
                offset = end;
            }
        }
        self.active = active;
        self.fading = fading;
        if !frames.is_empty() {
            if MODULATED {
                self.record_last_targets(values, mask, frames.len() - 1);
            } else {
                self.last_targets = self.prepared.base_targets();
            }
        }
    }

    fn record_last_targets(
        &mut self,
        values: Option<&dyn NativeParameterAutomation>,
        mask: u16,
        last_offset: usize,
    ) {
        let defaults = self.prepared.base_targets();
        for (index, descriptor) in DESCRIPTORS.iter().enumerate() {
            self.last_targets[index] = Self::read_param(values, mask, descriptor.id, last_offset)
                .unwrap_or(defaults[index]);
        }
    }

    #[cfg(test)]
    fn mix(
        &self,
        frame: &mut StereoFrame,
        pcm: hex_lattice::DeviceResourcePcm<'_>,
        voice: &ActiveVoice,
        gain: f64,
    ) {
        let Some(mut note) = self.prepared.notes[usize::from(voice.pitch)] else {
            return;
        };
        for channel_gain in &mut note.channel_gain {
            *channel_gain *= voice.gain;
        }
        Self::mix_prepared(frame, pcm, voice, gain, &note);
    }

    fn mix_prepared(
        frame: &mut StereoFrame,
        pcm: hex_lattice::DeviceResourcePcm<'_>,
        voice: &ActiveVoice,
        gain: f64,
        note: &VoiceCoefficients,
    ) {
        let sample = note.sample_at_position(
            pcm,
            voice.age,
            voice.gate.unwrap_or(u64::MAX),
            voice.position,
        );
        for channel in 0..2 {
            frame[channel] = (f64::from(frame[channel]) + f64::from(sample[channel]) * gain)
                .clamp(-f64::from(f32::MAX), f64::from(f32::MAX))
                as f32;
        }
    }
}

#[cfg(test)]
mod tests;
