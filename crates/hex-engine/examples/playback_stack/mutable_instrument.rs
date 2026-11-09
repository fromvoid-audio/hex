use hex_lattice::{
    MutableInstrumentRuntime, NativeInstrumentDefinition, NativeInstrumentEvent,
    NativeInstrumentRuntimeError, PreparedAnalyticNote, PreparedMutableInstrumentRuntime,
    PreparedNativeInstrument,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterValue,
};
use std::any::Any;

use super::{MUTABLE_TYPE, RATE};

pub(super) static PROBE_INSTRUMENT: ProbeInstrumentDefinition = ProbeInstrumentDefinition;

pub(super) struct ProbeInstrumentDefinition;
struct ProbePreparedInstrument;
struct ProbeFactory;

impl NativeDeviceMetadata for ProbeInstrumentDefinition {
    fn type_id(&self) -> DeviceTypeId {
        MUTABLE_TYPE
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("stack probe mutable instrument")
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(MUTABLE_TYPE, 1, Vec::new())
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() == MUTABLE_TYPE
            && state.state_version() == 1
            && state.payload().is_empty()
        {
            Ok(state.clone())
        } else {
            Err(DeviceContractError::MalformedState)
        }
    }
    fn read_parameter(
        &self,
        _: &NativeDeviceState,
        _: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }
    fn patch_parameter(
        &self,
        _: &NativeDeviceState,
        _: ParameterId,
        _: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }
}

impl NativeInstrumentDefinition for ProbeInstrumentDefinition {
    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        let sample_rate = context.sample_rate;
        if !context.tuning.is_neutral() {
            return Err(hex_lattice_contract::DeviceContractError::UnsupportedProcessingClass);
        }
        if sample_rate != RATE || state.type_id() != MUTABLE_TYPE || state.state_version() != 1 {
            return Err(DeviceContractError::MalformedState);
        }
        Ok(Box::new(ProbePreparedInstrument))
    }
}

impl PreparedNativeInstrument for ProbePreparedInstrument {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn prepare_note(&self, _: u8, _: u8) -> Result<PreparedAnalyticNote, DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_mutable_runtime(
        &self,
    ) -> Result<Option<Box<dyn PreparedMutableInstrumentRuntime>>, DeviceContractError> {
        Ok(Some(Box::new(ProbeFactory)))
    }
}

impl PreparedMutableInstrumentRuntime for ProbeFactory {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<ProbeRuntime>()
    }
    fn max_events_per_block(&self) -> usize {
        256
    }
    fn max_active_occurrences(&self) -> usize {
        64
    }
    fn max_release_tail_frames(&self) -> u64 {
        64
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(ProbeRuntime::default())
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone, Copy, Default)]
struct ProbeVoice {
    occurrence: u64,
    pitch: u8,
    active: bool,
    gated: bool,
    release_left: u8,
}

struct ProbeRuntime {
    voices: [ProbeVoice; 64],
}

impl Default for ProbeRuntime {
    fn default() -> Self {
        Self {
            voices: [ProbeVoice::default(); 64],
        }
    }
}

impl MutableInstrumentRuntime for ProbeRuntime {
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if events.len() > 256 || frame_count > 256 {
            return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
        }
        let mut voices = self.voices;
        let mut previous_offset = 0;
        for event in events.iter().copied() {
            let (offset, occurrence, note_on, pitch) = match event {
                // This note-only stack fixture has no controller transfer law.
                NativeInstrumentEvent::NotePressure { .. }
                | NativeInstrumentEvent::Expression { .. } => {
                    return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                }
                NativeInstrumentEvent::NoteOn {
                    frame_offset,
                    occurrence_id,
                    pitch,
                    ..
                } => (frame_offset, occurrence_id, true, pitch),
                NativeInstrumentEvent::NoteOff {
                    frame_offset,
                    occurrence_id,
                } => (frame_offset, occurrence_id, false, 0),
            };
            if offset < previous_offset || offset >= frame_count {
                return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
            }
            for voice in &mut voices {
                if voice.active && !voice.gated {
                    voice.release_left = voice
                        .release_left
                        .saturating_sub(offset.saturating_sub(previous_offset) as u8);
                    if voice.release_left == 0 {
                        *voice = ProbeVoice::default();
                    }
                }
            }
            if note_on {
                if voices
                    .iter()
                    .any(|voice| voice.active && voice.occurrence == occurrence)
                {
                    return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                }
                let slot = voices
                    .iter_mut()
                    .find(|voice| !voice.active)
                    .ok_or(NativeInstrumentRuntimeError::EventCapacityExceeded)?;
                *slot = ProbeVoice {
                    occurrence,
                    pitch,
                    active: true,
                    gated: true,
                    release_left: 64,
                };
            } else {
                let voice = voices
                    .iter_mut()
                    .find(|voice| voice.active && voice.gated && voice.occurrence == occurrence)
                    .ok_or(NativeInstrumentRuntimeError::InvalidEventBatch)?;
                voice.gated = false;
                voice.release_left = 64;
            }
            previous_offset = offset;
        }
        Ok(())
    }

    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [[f32; 2]],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        let mut event_index = 0;
        for (frame_index, frame) in frames.iter_mut().enumerate() {
            while let Some(event) = events.get(event_index).copied() {
                match event {
                    NativeInstrumentEvent::NoteOn {
                        frame_offset,
                        occurrence_id,
                        pitch,
                        velocity,
                    } if frame_offset == frame_index => {
                        if let Some(voice) = self.voices.iter_mut().find(|voice| !voice.active) {
                            *voice = ProbeVoice {
                                occurrence: occurrence_id,
                                pitch,
                                active: true,
                                gated: true,
                                release_left: 64,
                            };
                            let amplitude = f32::from(velocity) / 127.0 * 0.000_1;
                            frame[0] += amplitude * (f32::from(pitch % 12) + 1.0);
                            frame[1] += amplitude;
                        }
                        event_index += 1;
                    }
                    NativeInstrumentEvent::NoteOff {
                        frame_offset,
                        occurrence_id,
                    } if frame_offset == frame_index => {
                        if let Some(voice) = self.voices.iter_mut().find(|voice| {
                            voice.active && voice.gated && voice.occurrence == occurrence_id
                        }) {
                            voice.gated = false;
                            voice.release_left = 64;
                        }
                        event_index += 1;
                    }
                    _ => break,
                }
            }
            for voice in &mut self.voices {
                if voice.active {
                    let release = if voice.gated {
                        1.0
                    } else {
                        f32::from(voice.release_left) / 64.0
                    };
                    let amplitude = (f32::from(voice.pitch % 12) + 1.0) * 0.000_001 * release;
                    frame[0] += amplitude;
                    frame[1] += amplitude * 0.75;
                    if !voice.gated {
                        voice.release_left = voice.release_left.saturating_sub(1);
                        if voice.release_left == 0 {
                            *voice = ProbeVoice::default();
                        }
                    }
                }
            }
        }
    }

    fn reset(&mut self) {
        self.voices = [ProbeVoice::default(); 64];
    }
    fn is_quiescent(&self) -> bool {
        self.voices.iter().all(|voice| !voice.active)
    }
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.voices = previous.voices;
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
mod tests {
    use super::*;
    #[test]
    fn note_only_stack_probe_refuses_expression_before_state_changes() {
        let runtime = ProbeRuntime::default();
        let events = [NativeInstrumentEvent::Expression {
            frame_offset: 0,
            expression: hex_lattice::NativeInstrumentExpression::ModWheel(127),
        }];
        assert_eq!(
            runtime.preflight_events(&events, 64, &hex_lattice::EMPTY_DEVICE_RESOURCES),
            Err(NativeInstrumentRuntimeError::InvalidEventBatch)
        );
        assert!(runtime.is_quiescent());
    }
}
