//! Fixed-capacity mutable sampler host; PCM stays in the borrowed host scope.
use super::*;
use hex_lattice::{
    DeviceResources, InstrumentHistoryRetargetPreparation, MutableInstrumentRuntime,
    NativeInstrumentEvent, NativeInstrumentOccurrenceGain, NativeInstrumentRuntimeError as Error,
    PreparedInstrumentHistoryRetarget, PreparedMutableInstrumentRuntime,
};
use hex_lattice_contract::DeviceContractError;
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
            max_events,
        } = self;
        *notes == other.notes
            && *source == other.source
            && *authored_pitch == other.authored_pitch
            && *voices == other.voices
            && *tail == other.tail
            && *velocity_amount == other.velocity_amount
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
        reverse,
        step,
        channel_gain: _,
        gate,
        envelope,
    } = target;
    *expected_frames == source.expected_frames
        && *channels == source.channels
        && *region == source.region
        && *loop_offsets == source.loop_offsets
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
/// Opaque fixed-storage sampler processor created only by its prepared factory.
/// Hosts may identify this concrete audited type; no source owner is stored here.
pub struct SamplerRuntime {
    prepared: PreparedSamplerRuntime,
    active: Active,
}
impl PreparedMutableInstrumentRuntime for PreparedSamplerRuntime {
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
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(SamplerRuntime {
            prepared: self.clone(),
            active: [None; MAX_SAMPLER_RUNTIME_VOICES],
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
impl SamplerRuntime {
    fn advance(&self, active: &mut Active, frames: u64) -> Result<(), Error> {
        for entry in active.iter_mut() {
            let Some(voice) = entry else { continue };
            let Some(note) = self.prepared.notes[usize::from(voice.pitch)] else {
                *entry = None;
                continue;
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
                    if note.loop_offsets.is_some()
                        && gate.checked_add(note.release_frames()).is_none()
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
            if source_ended || release_ended {
                *entry = None;
            }
        }
        Ok(())
    }
    fn event(&self, active: &mut Active, event: NativeInstrumentEvent) -> Result<(), Error> {
        match event {
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
        self.advance(active, 0)
    }
}
impl MutableInstrumentRuntime for SamplerRuntime {
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
        self.process_with_gains(events, frames, gains, resources);
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
        let mut cursor = 0;
        for &event in events {
            let offset = event.frame_offset();
            if offset < cursor || offset >= frames {
                return Err(Error::InvalidEventBatch);
            }
            self.advance(&mut active, (offset - cursor) as u64)?;
            self.event(&mut active, event)?;
            cursor = offset;
        }
        self.advance(&mut active, (frames - cursor) as u64)
    }
    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        resources: &dyn DeviceResources,
    ) {
        self.process_with_gains(events, frames, &[], resources);
    }
    fn reset(&mut self) {
        self.active.fill(None);
    }
    fn is_quiescent(&self) -> bool {
        self.active.iter().all(Option::is_none)
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
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl SamplerRuntime {
    fn process_with_gains(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        gains: &[NativeInstrumentOccurrenceGain],
        resources: &dyn DeviceResources,
    ) {
        let pcm = (0..resources
            .len()
            .min(hex_lattice_contract::MAX_DEVICE_ASSET_REFERENCES))
            .filter_map(|index| resources.get(index))
            .find(|view| self.prepared.source.matches(*view));
        let Some(view) = pcm else { return };
        let mut active = self.active;
        let mut next = 0;
        for (offset, frame) in frames.iter_mut().enumerate() {
            while events
                .get(next)
                .is_some_and(|event| event.frame_offset() == offset)
            {
                if self.event(&mut active, events[next]).is_err() {
                    return;
                }
                next += 1;
            }
            for voice in active.iter().flatten() {
                let Some(mut note) = self.prepared.notes[usize::from(voice.pitch)] else {
                    continue;
                };
                for channel_gain in &mut note.channel_gain {
                    *channel_gain *= voice.gain;
                }
                let sample = note.sample_at_position(
                    view.pcm,
                    voice.age,
                    voice.gate.unwrap_or(u64::MAX),
                    voice.position,
                );
                let gain = gains
                    .binary_search_by_key(&voice.occurrence, |gain| gain.occurrence_id())
                    .ok()
                    .map_or(1.0, |index| gains[index].value_at_offset(offset as u64));
                for channel in 0..2 {
                    frame[channel] = (f64::from(frame[channel]) + f64::from(sample[channel]) * gain)
                        .clamp(-f64::from(f32::MAX), f64::from(f32::MAX))
                        as f32;
                }
            }
            if self.advance(&mut active, 1).is_err() {
                return;
            }
        }
        self.active = active;
    }
}

#[cfg(test)]
mod tests;
