//! Bounded neutral native MIDI processor contract, registry, and transactional pair.
//!
//! MIDI processors transform caller-owned bounded `NativeInstrumentEvent` slices
//! before a native instrument consumes them. They never own a second transport
//! scheduler, never allocate or drop on the audio callback, and track active
//! occurrences by stable `occurrence_id` so same-pitch overlaps, paired
//! note-offs, Stop, Seek, loop wraps, route transitions, and revision handoffs
//! leave no stuck notes.

mod pair;
#[cfg(test)]
mod tests;

pub use pair::NativeMidiProcessorRuntimePair;

use super::{MAX_NATIVE_DEVICE_TYPES, NativeInstrumentClock, NativeInstrumentEvent};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
};
use std::{any::Any, fmt};

/// Caller-supplied finite ceilings validated on control before constructing a
/// MIDI processor runtime pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeMidiProcessorLimits {
    /// Ceiling for the checked combined declaration of committed and scratch
    /// runtime storage.
    pub max_pair_bytes: usize,
    /// Maximum caller-supplied input events in one processed block or segment.
    pub max_input_events_per_block: usize,
    /// Maximum output events emitted in one processed block or segment.
    pub max_output_events_per_block: usize,
    /// Maximum simultaneous tracked occurrence identities.
    pub max_active_occurrences: usize,
    /// Maximum declared finite work units per processed block.
    pub max_work_units_per_block: usize,
}

/// Errors reported before mutable MIDI processor history or downstream
/// instrument state is modified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeMidiProcessorError {
    RuntimeBudgetExceeded,
    IncompatibleTwins,
    IncompatibleHistory,
    BlockNotStarted,
    InvalidInputEventBatch,
    InputEventCapacityExceeded,
    OutputEventCapacityExceeded,
    ActiveOccurrenceCapacityExceeded,
    WorkBudgetExceeded,
    InvalidOutputEventBatch,
    OrphanedNoteOff,
    DuplicateOccurrence,
    InvalidEventPayload,
}

impl NativeMidiProcessorError {
    /// Allocation-free lowercase diagnostic message for callback reporting.
    pub const fn diagnostic_message(self) -> &'static str {
        match self {
            Self::RuntimeBudgetExceeded => "native midi processor runtime exceeds caller limits",
            Self::IncompatibleTwins => "native midi processor runtime twins are incompatible",
            Self::IncompatibleHistory => "native midi processor runtime histories are incompatible",
            Self::BlockNotStarted => "native midi processor block was not started",
            Self::InvalidInputEventBatch => "native midi processor input event batch is invalid",
            Self::InputEventCapacityExceeded => {
                "native midi processor input event capacity is exceeded"
            }
            Self::OutputEventCapacityExceeded => {
                "native midi processor output event capacity is exceeded"
            }
            Self::ActiveOccurrenceCapacityExceeded => {
                "native midi processor active occurrence capacity is exceeded"
            }
            Self::WorkBudgetExceeded => "native midi processor work budget is exceeded",
            Self::InvalidOutputEventBatch => "native midi processor output event batch is invalid",
            Self::OrphanedNoteOff => "native midi processor note-off has no active occurrence",
            Self::DuplicateOccurrence => "native midi processor occurrence is already active",
            Self::InvalidEventPayload => "native midi processor event payload is out of range",
        }
    }
}

impl fmt::Display for NativeMidiProcessorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.diagnostic_message())
    }
}

impl std::error::Error for NativeMidiProcessorError {}

/// Preallocated caller-owned output event sink with bounded capacity and work
/// accounting. Pushing or charging work never allocates.
pub struct MidiProcessorOutput<'a> {
    buffer: &'a mut [NativeInstrumentEvent],
    len: usize,
    max_output_events: usize,
    work_units: usize,
    max_work_units: usize,
}

impl<'a> MidiProcessorOutput<'a> {
    pub fn new(
        buffer: &'a mut [NativeInstrumentEvent],
        max_output_events: usize,
        max_work_units: usize,
    ) -> Self {
        let cap = buffer.len().min(max_output_events);
        Self {
            buffer,
            len: 0,
            max_output_events: cap,
            work_units: 0,
            max_work_units,
        }
    }

    pub fn push(&mut self, event: NativeInstrumentEvent) -> Result<(), NativeMidiProcessorError> {
        if self.len >= self.max_output_events {
            return Err(NativeMidiProcessorError::OutputEventCapacityExceeded);
        }
        let slot = self
            .buffer
            .get_mut(self.len)
            .ok_or(NativeMidiProcessorError::OutputEventCapacityExceeded)?;
        *slot = event;
        self.len += 1;
        Ok(())
    }

    pub fn charge_work(&mut self, units: usize) -> Result<(), NativeMidiProcessorError> {
        let next = self
            .work_units
            .checked_add(units)
            .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
        if next > self.max_work_units {
            return Err(NativeMidiProcessorError::WorkBudgetExceeded);
        }
        self.work_units = next;
        Ok(())
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn remaining_capacity(&self) -> usize {
        self.max_output_events.saturating_sub(self.len)
    }

    pub const fn work_units(&self) -> usize {
        self.work_units
    }

    pub const fn remaining_work(&self) -> usize {
        self.max_work_units.saturating_sub(self.work_units)
    }

    pub fn as_slice(&self) -> &[NativeInstrumentEvent] {
        &self.buffer[..self.len]
    }
}

/// Control-side definition of one native MIDI processor type.
pub trait NativeMidiProcessorDefinition: NativeDeviceMetadata + Send + Sync {
    fn prepare_midi_processor(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Box<dyn PreparedNativeMidiProcessor>, DeviceContractError>;
}

/// Bounded immutable registry for native MIDI processors.
pub struct NativeMidiProcessorRegistry<'a> {
    definitions: &'a [&'a dyn NativeMidiProcessorDefinition],
}

impl<'a> NativeMidiProcessorRegistry<'a> {
    pub fn new(
        definitions: &'a [&'a dyn NativeMidiProcessorDefinition],
    ) -> Result<Self, DeviceContractError> {
        if definitions.len() > MAX_NATIVE_DEVICE_TYPES {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        for (index, definition) in definitions.iter().enumerate() {
            let type_id = NativeDeviceMetadata::type_id(*definition);
            type_id.validate()?;
            if definition.display_name().is_some_and(|name| {
                name.is_empty()
                    || name.len() > 128
                    || name.trim() != name
                    || name
                        .chars()
                        .any(|character| character.is_control() || character.is_uppercase())
            }) {
                return Err(DeviceContractError::InvalidDescriptor);
            }
            if definition.state_version() == 0 {
                return Err(DeviceContractError::InvalidStateVersion);
            }
            if definitions[..index]
                .iter()
                .any(|prior| NativeDeviceMetadata::type_id(*prior) == type_id)
            {
                return Err(DeviceContractError::DuplicateDeviceType);
            }
            hex_lattice_contract::validate_parameter_descriptors(definition.descriptors())?;
        }
        Ok(Self { definitions })
    }

    pub fn definition(
        &self,
        type_id: DeviceTypeId,
    ) -> Option<&'a dyn NativeMidiProcessorDefinition> {
        self.definitions
            .iter()
            .copied()
            .find(|definition| NativeDeviceMetadata::type_id(*definition) == type_id)
    }

    pub fn definitions(&self) -> &'a [&'a dyn NativeMidiProcessorDefinition] {
        self.definitions
    }

    pub fn default_state(
        &self,
        type_id: DeviceTypeId,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let definition = self
            .definition(type_id)
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let state = definition.default_state()?;
        self.validate_current(
            definition,
            definition
                .migrate_state(&state)?
                .with_host_metadata_from(&state)?,
        )
    }

    pub fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        self.validate_current(
            definition,
            definition
                .migrate_state(state)?
                .with_host_metadata_from(state)?,
        )
    }

    pub fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: hex_lattice_contract::ParameterId,
        value: hex_lattice_contract::ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let current = self.validate_current(
            definition,
            definition
                .migrate_state(state)?
                .with_host_metadata_from(state)?,
        )?;
        hex_lattice_contract::validate_parameter_write(definition, &current, parameter, value)?;
        let patched = definition.patch_parameter(&current, parameter, value)?;
        self.validate_current(
            definition,
            definition
                .migrate_state(&patched)?
                .with_host_metadata_from(state)?,
        )
    }

    pub fn prepare_midi_processor(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Box<dyn PreparedNativeMidiProcessor>, DeviceContractError> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        if !state.asset_references().is_empty() {
            return Err(DeviceContractError::AssetResourcesUnavailable);
        }
        let current = self.validate_current(
            definition,
            definition
                .migrate_state(state)?
                .with_host_metadata_from(state)?,
        )?;
        definition.prepare_midi_processor(&current, sample_rate)
    }

    fn validate_current(
        &self,
        definition: &dyn NativeMidiProcessorDefinition,
        state: NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() != definition.type_id() {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != definition.state_version() {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        let _ = hex_lattice_contract::validated_state_parameters(definition, &state)?;
        Ok(state)
    }
}

/// Control-prepared immutable factory for a bounded MIDI processor runtime pair.
pub trait PreparedNativeMidiProcessor: Send + Sync {
    fn runtime_version(&self) -> u32;
    fn declared_twin_bytes(&self) -> usize;
    fn max_input_events_per_block(&self) -> usize;
    fn max_output_events_per_block(&self) -> usize;
    fn max_active_occurrences(&self) -> usize;
    fn max_work_units_per_block(&self) -> usize;
    fn create_runtime(&self) -> Box<dyn MutableMidiProcessorRuntime>;
    fn as_any(&self) -> &dyn Any;
}

/// Mutable per-route MIDI processor state owned inside a transactional pair.
///
/// `preflight_block` and `preflight_flush_active_note_offs` must be read-only
/// and validate all occurrence identities, capacities, pitch/velocity bounds,
/// and work limits before any state or caller output buffer changes. Once
/// preflight succeeds, `process_block`, `flush_active_note_offs`, history
/// copies, and `reset` must be infallible, allocation-free, lock-free,
/// I/O-free, and free of final drops.
pub trait MutableMidiProcessorRuntime: Any + Send {
    fn preflight_block(
        &self,
        input: &[NativeInstrumentEvent],
        frame_count: usize,
        output_capacity: usize,
        clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<usize, NativeMidiProcessorError>;

    fn process_block(
        &mut self,
        input: &[NativeInstrumentEvent],
        frame_count: usize,
        output: &mut MidiProcessorOutput<'_>,
        clock: Option<&dyn NativeInstrumentClock>,
    );

    fn preflight_flush_active_note_offs(
        &self,
        frame_offset: usize,
        frame_count: usize,
        output_capacity: usize,
    ) -> Result<usize, NativeMidiProcessorError>;

    fn flush_active_note_offs(&mut self, frame_offset: usize, output: &mut MidiProcessorOutput<'_>);

    fn active_occurrence_count(&self) -> usize;

    fn is_quiescent(&self) -> bool {
        self.active_occurrence_count() == 0
    }

    fn reset(&mut self);

    /// Whether all committed history in `previous` can be copied with exact
    /// future event equivalence under this runtime's immutable configuration.
    fn can_copy_history_from(&self, previous: &dyn MutableMidiProcessorRuntime) -> bool;

    /// Copy complete committed state from a compatible runtime. Bounded and
    /// allocation-free.
    fn copy_history_from(&mut self, previous: &dyn MutableMidiProcessorRuntime);

    fn fixed_storage_bytes(&self) -> usize;

    fn as_any(&self) -> &dyn Any;
}

pub(crate) fn validate_midi_event_batch(
    events: &[NativeInstrumentEvent],
    frame_count: usize,
    max_events: usize,
    capacity_error: NativeMidiProcessorError,
    invalid_error: NativeMidiProcessorError,
) -> Result<(), NativeMidiProcessorError> {
    if events.len() > max_events {
        return Err(capacity_error);
    }
    let mut previous_offset = 0_usize;
    for event in events {
        let offset = event.frame_offset();
        if frame_count == 0 || offset >= frame_count || offset < previous_offset {
            return Err(invalid_error);
        }
        previous_offset = offset;
        let payload_valid = match *event {
            NativeInstrumentEvent::NoteOn {
                occurrence_id,
                pitch,
                velocity,
                ..
            } => occurrence_id != 0 && pitch <= 127 && (1..=127).contains(&velocity),
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => occurrence_id != 0,
            NativeInstrumentEvent::NotePressure { value, .. }
            | NativeInstrumentEvent::NoteTimbre { value, .. } => value <= 127,
            NativeInstrumentEvent::NotePitchBend { cents, .. } => (-4_800..=4_800).contains(&cents),
            NativeInstrumentEvent::ControlChange { cc, value, .. } => cc <= 127 && value <= 127,
            NativeInstrumentEvent::PitchBend { value, .. } => (-8_192..=8_191).contains(&value),
            NativeInstrumentEvent::Expression { expression, .. } => expression.is_valid(),
        };
        if !payload_valid {
            return Err(invalid_error);
        }
    }
    Ok(())
}
