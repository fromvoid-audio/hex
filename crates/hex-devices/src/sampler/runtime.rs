//! Fixed-capacity mutable sampler host; PCM stays in the borrowed host scope.
use super::*;
use hex_lattice::{
    DeviceResources, MutableInstrumentRuntime, NativeInstrumentEvent,
    NativeInstrumentRuntimeError as Error, PreparedMutableInstrumentRuntime,
};
use std::any::Any;

pub const MAX_SAMPLER_RUNTIME_VOICES: usize = 32;
const MAX_EVENTS: usize = 1024;

#[derive(Clone, Copy, Default, PartialEq)]
struct PreparedKey {
    step: u64,
    start: u32,
    end: u32,
}

/// Control-prepared key table and original source binding, without PCM/markers.
/// Not registry admission: the actual owner graph must still qualify separately.
#[derive(Clone, PartialEq)]
pub struct PreparedSamplerRuntime {
    notes: [PreparedKey; 128],
    common: VoiceCoefficients,
    source: binding::SourceKey,
    voices: usize,
    tail: u64,
    velocity_amount: f64,
    max_events: usize,
}
impl PreparedSamplerRuntime {
    /// Exact requested sized Box body for the closed constructor below.
    /// This is not provenance for arbitrary factories or trait objects.
    pub fn runtime_box_layout_on_control() -> std::alloc::Layout {
        std::alloc::Layout::new::<SamplerRuntime>()
    }
    /// Narrow fixed-leaf declaration for this actual sampler factory. This is
    /// not a declaration for arbitrary trait objects, wrappers or source owners.
    /// The host separately retains and charges its owning allocation and PCM.
    pub fn control_storage_declaration(&self) -> hex_lattice::PreparedStorageDeclaration {
        let Self {
            notes,
            common,
            source,
            voices,
            tail,
            velocity_amount,
            max_events,
        } = self;
        let _: &[PreparedKey; 128] = notes;
        let _: (&usize, &u64, &f64, &usize) = (voices, tail, velocity_amount, max_events);
        source.audit_scalar_storage();
        common.audit_scalar_storage();
        for PreparedKey { step, start, end } in notes {
            let _: (&u64, &u32, &u32) = (step, start, end);
        }
        hex_lattice::PreparedStorageDeclaration::ExclusiveOnly {
            bytes: std::mem::size_of::<Self>(),
        }
    }
    fn note(&self, pitch: u8) -> Option<VoiceCoefficients> {
        let key = self.notes.get(usize::from(pitch))?;
        (key.step != 0).then(|| {
            let mut note = self.common;
            note.step = key.step;
            note.region = Region {
                start: key.start as usize,
                end: key.end as usize,
            };
            note
        })
    }
}
impl BoundSamplerPlan<'_> {
    pub fn prepare_runtime_on_control(
        self,
        voices: usize,
        max_events: usize,
        tuning: &hex_lattice::TuningTable,
    ) -> Result<PreparedSamplerRuntime, Invalid> {
        if !(1..=MAX_SAMPLER_RUNTIME_VOICES).contains(&voices)
            || !(1..=MAX_EVENTS).contains(&max_events)
        {
            return Err(Invalid("sampler voice count exceeds its fixed capacity"));
        }
        let mut notes = [PreparedKey::default(); 128];
        let mut common = None;
        let mut tail = 0;
        for (pitch, entry) in notes.iter_mut().enumerate() {
            // Velocity scaling is applied below using the original linear law.
            let voice = self
                .plan
                .prepare_tuned_voice_coefficients(pitch as u8, 127, tuning)?;
            if let Some(voice) = voice {
                common.get_or_insert(voice);
                *entry = PreparedKey {
                    step: voice.step,
                    start: voice.region.start as u32,
                    end: voice.region.end as u32,
                };
                tail = tail.max(match voice.gate {
                    GateMode::Trigger => voice
                        .natural_frames()
                        .ok_or(Invalid("trigger needs a finite end"))?,
                    GateMode::Gate => voice.release_frames(),
                });
            }
        }
        Ok(PreparedSamplerRuntime {
            notes,
            common: common.ok_or(Invalid("sampler needs at least one mapped key"))?,
            source: self.source,
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
    gate: Option<u64>,
}
type Active = [Option<ActiveVoice>; MAX_SAMPLER_RUNTIME_VOICES];
struct SamplerRuntime {
    prepared: PreparedSamplerRuntime,
    active: Active,
}
impl SamplerRuntime {
    fn audit_fixed_storage(&self) {
        let Self { prepared, active } = self;
        let _ = prepared.control_storage_declaration();
        let _: &Active = active;
        // The exhaustive leaf audit is compiled even for an empty voice bank.
        fn voice(voice: &ActiveVoice) {
            let ActiveVoice {
                occurrence,
                pitch,
                gain,
                age,
                gate,
            } = voice;
            let _: (&u64, &u8, &f64, &u64, &Option<u64>) = (occurrence, pitch, gain, age, gate);
        }
        for active in active.iter().flatten() {
            voice(active);
        }
    }
}
impl PreparedMutableInstrumentRuntime for PreparedSamplerRuntime {
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
        let runtime = SamplerRuntime {
            prepared: self.clone(),
            active: [None; MAX_SAMPLER_RUNTIME_VOICES],
        };
        runtime.audit_fixed_storage();
        Box::new(runtime)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl SamplerRuntime {
    fn advance(&self, active: &mut Active, frames: u64) -> Result<(), Error> {
        for entry in active.iter_mut().flatten() {
            entry.age = entry
                .age
                .checked_add(frames)
                .ok_or(Error::InvalidEventBatch)?;
        }
        for entry in active.iter_mut() {
            let Some(voice) = entry else { continue };
            let Some(note) = self.prepared.note(voice.pitch) else {
                *entry = None;
                continue;
            };
            let end = if let Some(gate) = voice.gate {
                Some(
                    note.end_age_frames(gate)
                        .map_err(|_| Error::InvalidEventBatch)?,
                )
            } else {
                note.natural_frames()
            };
            if end.is_some_and(|end| voice.age >= end) {
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
                if velocity == 0 || self.prepared.note(pitch).is_none() {
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
                    gate: None,
                });
            }
        }
        self.advance(active, 0)
    }
}
impl MutableInstrumentRuntime for SamplerRuntime {
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
                let Some(mut note) = self.prepared.note(voice.pitch) else {
                    continue;
                };
                for channel_gain in &mut note.channel_gain {
                    *channel_gain *= voice.gain;
                }
                let sample = note.sample(view.pcm, voice.age, voice.gate.unwrap_or(u64::MAX));
                for channel in 0..2 {
                    frame[channel] = (f64::from(frame[channel]) + f64::from(sample[channel]))
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
    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests;

// Loaded native adapter only. Registration/resource publication remain the
// host's separate admission boundary; this implementation retains no PCM.
impl hex_lattice::NativeInstrumentDefinition for super::metadata::SamplerMetadata {
    fn prepare_instrument(
        &self,
        state: &hex_lattice_contract::NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<
        Box<dyn hex_lattice::PreparedNativeInstrument>,
        hex_lattice_contract::DeviceContractError,
    > {
        use hex_lattice_contract::DeviceContractError as Error;
        if matches!(
            super::state::DeviceState::from_native(state)?,
            super::state::DeviceState::Empty
        ) {
            return Err(Error::AssetResourcesUnavailable);
        }
        context.validate_for(state)?;
        // Matches the existing native host's 256 authored events plus three
        // boundary events. This does not increase its query or voice ceilings.
        let prepared = super::state::prepare_runtime_on_control(state, context, 259)?;
        Ok(Box::new(prepared))
    }
}
impl hex_lattice::PreparedNativeInstrument for PreparedSamplerRuntime {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn prepare_note(
        &self,
        _pitch: u8,
        _velocity: u8,
    ) -> Result<hex_lattice::PreparedAnalyticNote, hex_lattice_contract::DeviceContractError> {
        // Mutable sample playback must never silently fall back to an analytic
        // oscillator or sample an unretained source through an invented token.
        Err(hex_lattice_contract::DeviceContractError::UnsupportedProcessingClass)
    }
    fn validate_event_note(
        &self,
        pitch: u8,
        velocity: u8,
    ) -> Result<(), hex_lattice_contract::DeviceContractError> {
        if pitch > 127 || velocity > 127 {
            return Err(hex_lattice_contract::DeviceContractError::InvalidParameterValue);
        }
        // Unmapped Slice keys and zero velocity are valid silent/no-onset
        // events under the actual runtime law, never fallback sample regions.
        Ok(())
    }
    fn prepare_mutable_runtime(
        &self,
    ) -> Result<
        Option<Box<dyn PreparedMutableInstrumentRuntime>>,
        hex_lattice_contract::DeviceContractError,
    > {
        Ok(Some(Box::new(self.clone())))
    }
}
