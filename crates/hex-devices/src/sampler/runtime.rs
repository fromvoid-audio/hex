//! Fixed-capacity mutable sampler host; PCM stays in the borrowed host scope.
use super::catalog::{GAIN, GAIN_DESCRIPTORS, gain_semantics};
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
    max_events: usize,
}

impl PartialEq for PreparedSamplerRuntime {
    fn eq(&self, other: &Self) -> bool {
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
            max_events,
        } = self;
        *source == previous.source
            && *authored_pitch == previous.authored_pitch
            && *voices == previous.voices
            && *tail == previous.tail
            && *velocity_amount == previous.velocity_amount
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
            max_events,
        })
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
    last_gain: f32,
}
impl PreparedMutableInstrumentRuntime for PreparedSamplerRuntime {
    fn parameter_automation_descriptors(&self) -> &[ParameterDescriptor] {
        &GAIN_DESCRIPTORS
    }
    fn parameter_modulation_semantics(&self, parameter: ParameterId) -> Option<ParameterSemantics> {
        gain_semantics(parameter)
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
            last_gain: self.base_gain as f32,
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
        Some(self)
    }
    fn parameter_automation_readonly(&self) -> Option<&dyn InstrumentParameterRuntime> {
        Some(self)
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
        self.process_with_gains::<false>(events, frames, gains, resources, &[]);
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
        self.process_with_gains::<false>(events, frames, &[], resources, &[]);
    }
    fn reset(&mut self) {
        self.active.fill(None);
        self.fading.fill(None);
        self.last_gain = self.prepared.base_gain as f32;
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
            self.last_gain = previous.last_gain;
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
            self.last_gain = previous.last_gain;
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
        &GAIN_DESCRIPTORS
    }
    fn parameter_modulation_semantics(&self, parameter: ParameterId) -> Option<ParameterSemantics> {
        gain_semantics(parameter)
    }
    fn copy_targets(&self, output: &mut [Option<ParameterValue>]) -> Result<(), Error> {
        let [target] = output else {
            return Err(Error::InvalidParameterAutomation);
        };
        *target = Some(ParameterValue::Float(self.last_gain));
        Ok(())
    }
    fn preflight(
        &self,
        _: &[NativeInstrumentEvent],
        _: usize,
        _: Option<&dyn NativeInstrumentClock>,
        _: &[NativeInstrumentOccurrenceGain],
        _: &dyn NativeParameterAutomation,
        _: &dyn DeviceResources,
    ) -> Result<(), Error> {
        // The pair validates the complete single-gain view before rendering.
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
        if !values.parameter_ids().contains(&GAIN) {
            self.process_with_gains::<false>(events, frames, gains, resources, &[]);
            return;
        }
        // Read each control sample once, outside the per-voice loops. A
        // negative sentinel selects the exact authored f64 coefficient.
        let mut sampled_gain = [-1.0_f32; MAX_INSTRUMENT_AUTOMATION_FRAMES];
        let Some(sampled_gain) = sampled_gain.get_mut(..frames.len()) else {
            return;
        };
        for (offset, slot) in sampled_gain.iter_mut().enumerate() {
            if let Some(ParameterValue::Float(value)) = values.value_at(GAIN, offset) {
                *slot = value;
            }
        }
        self.process_with_gains::<true>(events, frames, gains, resources, sampled_gain);
    }
}

impl SamplerRuntime {
    fn process_with_gains<const MODULATED: bool>(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        gains: &[NativeInstrumentOccurrenceGain],
        resources: &dyn DeviceResources,
        sampled_gain: &[f32],
    ) {
        let pcm = (0..resources
            .len()
            .min(hex_lattice_contract::MAX_DEVICE_ASSET_REFERENCES))
            .filter_map(|index| resources.get(index))
            .find(|view| self.prepared.source.matches(*view));
        let Some(view) = pcm else { return };
        let pan = self.prepared.pan;
        let left = 1.0 - pan.max(0.0);
        let right = 1.0 + pan.min(0.0);
        let with_gain = |offset: usize, velocity: f64, base: [f64; 2]| {
            let value = sampled_gain[offset];
            if value >= 0.0 {
                let gain = f64::from(value);
                [(gain * left) * velocity, (gain * right) * velocity]
            } else {
                base
            }
        };
        let mut active = self.active;
        let mut fading = self.fading;
        let fade = self.prepared.steal_fade_frames() as f64;
        let mut next = 0;
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
                let Ok(count) = self.sounding_frames(&voice, span, None) else {
                    return;
                };
                let Some(mut note) = self.prepared.notes[usize::from(voice.pitch)] else {
                    *entry = None;
                    continue;
                };
                for channel_gain in &mut note.channel_gain {
                    *channel_gain *= voice.gain;
                }
                let base_gain = note.channel_gain;
                let mut final_voice = voice;
                let Ok(alive) = self.advance_voice(&mut final_voice, span as u64) else {
                    return;
                };
                let gain_index = gains
                    .binary_search_by_key(&voice.occurrence, |gain| gain.occurrence_id())
                    .ok();
                let mut cursor = voice;
                for (relative, frame) in frames[offset..offset + count].iter_mut().enumerate() {
                    let gain = gain_index.map_or(1.0, |index| {
                        gains[index].value_at_offset((offset + relative) as u64)
                    });
                    if MODULATED {
                        note.channel_gain = with_gain(offset + relative, voice.gain, base_gain);
                    }
                    Self::mix_prepared(frame, view.pcm, &cursor, gain, &note);
                    // Checked span advancement above proves these increments
                    // cannot overflow during the rendered prefix.
                    cursor.age = cursor.age.wrapping_add(1);
                    cursor.position = cursor.position.wrapping_add(u128::from(note.step));
                }
                *entry = alive.then_some(final_voice);
            }
            for entry in &mut fading {
                let Some(item) = *entry else { continue };
                let Ok(count) = self.sounding_frames(&item.voice, span, Some(item.start)) else {
                    return;
                };
                let Some(mut note) = self.prepared.notes[usize::from(item.voice.pitch)] else {
                    *entry = None;
                    continue;
                };
                for channel_gain in &mut note.channel_gain {
                    *channel_gain *= item.voice.gain;
                }
                let base_gain = note.channel_gain;
                let mut final_item = item;
                let Ok(alive) = self.advance_voice(&mut final_item.voice, span as u64) else {
                    return;
                };
                let mut cursor = item.voice;
                for (relative, frame) in frames[offset..offset + count].iter_mut().enumerate() {
                    if MODULATED {
                        note.channel_gain =
                            with_gain(offset + relative, item.voice.gain, base_gain);
                    }
                    let declick = 1.0 - cursor.age.saturating_sub(item.start) as f64 / fade;
                    Self::mix_prepared(frame, view.pcm, &cursor, item.gain * declick, &note);
                    cursor.age = cursor.age.wrapping_add(1);
                    cursor.position = cursor.position.wrapping_add(u128::from(note.step));
                }
                *entry = (alive
                    && final_item.voice.age.saturating_sub(item.start)
                        < self.prepared.steal_fade_frames())
                .then_some(final_item);
            }
            offset = end;
        }
        self.active = active;
        self.fading = fading;
        if !frames.is_empty() {
            self.last_gain = sampled_gain
                .last()
                .copied()
                .filter(|value| *value >= 0.0)
                .unwrap_or(self.prepared.base_gain as f32);
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
