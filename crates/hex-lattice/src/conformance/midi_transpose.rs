//! Original deterministic transpose MIDI processor fixture for host/contract verification.
//!
//! This fixture lives solely under `conformance` / `test` and is never exposed
//! in the production native device catalog. It proves bounded event transform,
//! stable occurrence identity across same-pitch overlaps, explicit out-of-range
//! policies (`Refuse`, `Clamp`, `Drop`), work-unit accounting, history
//! compatibility, and deterministic active-note flushing.

use crate::{
    MidiProcessorOutput, MutableMidiProcessorRuntime, NativeInstrumentClock, NativeInstrumentEvent,
    NativeMidiProcessorDefinition, NativeMidiProcessorError, PreparedNativeMidiProcessor,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;

pub const TRANSPOSE_FIXTURE_TYPE_ID: DeviceTypeId =
    DeviceTypeId::from_static("test.midi.transpose");
pub const TRANSPOSE_FIXTURE_STATE_VERSION: u32 = 1;
pub const TRANSPOSE_FIXTURE_RUNTIME_VERSION: u32 = 1;
pub const TRANSPOSE_MIN_SEMITONES: i8 = -48;
pub const TRANSPOSE_MAX_SEMITONES: i8 = 48;
pub const TRANSPOSE_MAX_ACTIVE_OCCURRENCES: usize = 64;
pub const TRANSPOSE_MAX_EVENTS_PER_BLOCK: usize = 259;
pub const TRANSPOSE_MAX_WORK_UNITS_PER_BLOCK: usize = TRANSPOSE_MAX_EVENTS_PER_BLOCK * 2;

pub const PARAM_SEMITONES: ParameterId = ParameterId::from_static(1);
pub const PARAM_OUT_OF_RANGE: ParameterId = ParameterId::from_static(2);

const OUT_OF_RANGE_LABELS: &[&str] = &["refuse", "clamp", "drop"];

const DESCRIPTORS: [ParameterDescriptor; 2] = [
    ParameterDescriptor {
        id: PARAM_SEMITONES,
        label: "semitones",
        kind: ParameterKind::Float {
            min: -48.0,
            max: 48.0,
            unit: Some("st"),
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: PARAM_OUT_OF_RANGE,
        label: "out of range",
        kind: ParameterKind::Choice {
            labels: OUT_OF_RANGE_LABELS,
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
];

/// Explicit boundary law when `pitch + semitones` leaves MIDI `0..=127`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransposeOutOfRangePolicy {
    /// Refuse the event batch in preflight before changing any state.
    Refuse = 0,
    /// Clamp transposed pitch into `0..=127`.
    Clamp = 1,
    /// Suppress the out-of-range onset and track its occurrence ID so its
    /// paired `NoteOff` and per-note expression events are also suppressed.
    Drop = 2,
}

impl TransposeOutOfRangePolicy {
    fn from_u8(raw: u8) -> Result<Self, DeviceContractError> {
        match raw {
            0 => Ok(Self::Refuse),
            1 => Ok(Self::Clamp),
            2 => Ok(Self::Drop),
            _ => Err(DeviceContractError::MalformedState),
        }
    }
}

/// Deterministic transpose fixture definition.
#[derive(Clone, Copy, Debug, Default)]
pub struct TransposeMidiProcessorDefinition;

impl TransposeMidiProcessorDefinition {
    pub fn encode_state(
        semitones: i8,
        policy: TransposeOutOfRangePolicy,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if !(TRANSPOSE_MIN_SEMITONES..=TRANSPOSE_MAX_SEMITONES).contains(&semitones) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        NativeDeviceState::new(
            TRANSPOSE_FIXTURE_TYPE_ID,
            TRANSPOSE_FIXTURE_STATE_VERSION,
            vec![semitones as u8, policy as u8],
        )
    }

    fn decode_state(
        state: &NativeDeviceState,
    ) -> Result<(i8, TransposeOutOfRangePolicy), DeviceContractError> {
        if state.type_id() != TRANSPOSE_FIXTURE_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != TRANSPOSE_FIXTURE_STATE_VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        let payload = state.payload();
        if payload.len() != 2 {
            return Err(DeviceContractError::MalformedState);
        }
        let semitones = payload[0] as i8;
        if !(TRANSPOSE_MIN_SEMITONES..=TRANSPOSE_MAX_SEMITONES).contains(&semitones) {
            return Err(DeviceContractError::MalformedState);
        }
        let policy = TransposeOutOfRangePolicy::from_u8(payload[1])?;
        Ok((semitones, policy))
    }
}

impl NativeDeviceMetadata for TransposeMidiProcessorDefinition {
    fn type_id(&self) -> DeviceTypeId {
        TRANSPOSE_FIXTURE_TYPE_ID
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("transpose fixture")
    }

    fn state_version(&self) -> u32 {
        TRANSPOSE_FIXTURE_STATE_VERSION
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &DESCRIPTORS
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        Self::encode_state(0, TransposeOutOfRangePolicy::Refuse)
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let (semitones, policy) = Self::decode_state(state)?;
        Self::encode_state(semitones, policy)
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let (semitones, policy) = Self::decode_state(state)?;
        match parameter {
            PARAM_SEMITONES => Ok(ParameterValue::Float(f32::from(semitones))),
            PARAM_OUT_OF_RANGE => Ok(ParameterValue::Choice(policy as u32)),
            _ => Err(DeviceContractError::UnknownParameter),
        }
    }

    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let (mut semitones, mut policy) = Self::decode_state(state)?;
        match (parameter, value) {
            (PARAM_SEMITONES, ParameterValue::Float(v)) if v.is_finite() => {
                let rounded = v.round();
                if rounded < f32::from(TRANSPOSE_MIN_SEMITONES)
                    || rounded > f32::from(TRANSPOSE_MAX_SEMITONES)
                {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                semitones = rounded as i8;
            }
            (PARAM_OUT_OF_RANGE, ParameterValue::Choice(choice)) => {
                let raw =
                    u8::try_from(choice).map_err(|_| DeviceContractError::InvalidParameterValue)?;
                policy = TransposeOutOfRangePolicy::from_u8(raw)
                    .map_err(|_| DeviceContractError::InvalidParameterValue)?;
            }
            (PARAM_SEMITONES | PARAM_OUT_OF_RANGE, _) => {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            _ => return Err(DeviceContractError::UnknownParameter),
        }
        Self::encode_state(semitones, policy)
    }
}

impl NativeMidiProcessorDefinition for TransposeMidiProcessorDefinition {
    fn prepare_midi_processor(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Box<dyn PreparedNativeMidiProcessor>, DeviceContractError> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let (semitones, policy) = Self::decode_state(state)?;
        Ok(Box::new(PreparedTransposeMidiProcessor::new(
            semitones, policy,
        )?))
    }
}

/// Control-prepared immutable transpose fixture factory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedTransposeMidiProcessor {
    semitones: i8,
    policy: TransposeOutOfRangePolicy,
    max_input_events: usize,
    max_output_events: usize,
    max_active_occurrences: usize,
    max_work_units: usize,
}

impl PreparedTransposeMidiProcessor {
    pub fn new(
        semitones: i8,
        policy: TransposeOutOfRangePolicy,
    ) -> Result<Self, DeviceContractError> {
        Self::with_caps(
            semitones,
            policy,
            TRANSPOSE_MAX_EVENTS_PER_BLOCK,
            TRANSPOSE_MAX_EVENTS_PER_BLOCK,
            TRANSPOSE_MAX_ACTIVE_OCCURRENCES,
            TRANSPOSE_MAX_WORK_UNITS_PER_BLOCK,
        )
    }

    pub fn with_caps(
        semitones: i8,
        policy: TransposeOutOfRangePolicy,
        max_input_events: usize,
        max_output_events: usize,
        max_active_occurrences: usize,
        max_work_units: usize,
    ) -> Result<Self, DeviceContractError> {
        if !(TRANSPOSE_MIN_SEMITONES..=TRANSPOSE_MAX_SEMITONES).contains(&semitones) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        if max_active_occurrences == 0
            || max_active_occurrences > TRANSPOSE_MAX_ACTIVE_OCCURRENCES
            || max_input_events == 0
            || max_output_events == 0
            || max_work_units == 0
        {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }
        Ok(Self {
            semitones,
            policy,
            max_input_events,
            max_output_events,
            max_active_occurrences,
            max_work_units,
        })
    }

    pub const fn semitones(self) -> i8 {
        self.semitones
    }

    pub const fn policy(self) -> TransposeOutOfRangePolicy {
        self.policy
    }
}

impl PreparedNativeMidiProcessor for PreparedTransposeMidiProcessor {
    fn runtime_version(&self) -> u32 {
        TRANSPOSE_FIXTURE_RUNTIME_VERSION
    }

    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<TransposeMidiRuntime>()
    }

    fn max_input_events_per_block(&self) -> usize {
        self.max_input_events
    }

    fn max_output_events_per_block(&self) -> usize {
        self.max_output_events
    }

    fn max_active_occurrences(&self) -> usize {
        self.max_active_occurrences
    }

    fn max_work_units_per_block(&self) -> usize {
        self.max_work_units
    }

    fn create_runtime(&self) -> Box<dyn MutableMidiProcessorRuntime> {
        Box::new(TransposeMidiRuntime {
            config: *self,
            active: [None; TRANSPOSE_MAX_ACTIVE_OCCURRENCES],
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TrackedTransposeOccurrence {
    occurrence_id: u64,
    /// `Some(pitch)` if emitted downstream; `None` if suppressed by `Drop`.
    emitted_pitch: Option<u8>,
}

#[derive(Clone)]
struct TransposeMidiRuntime {
    config: PreparedTransposeMidiProcessor,
    active: [Option<TrackedTransposeOccurrence>; TRANSPOSE_MAX_ACTIVE_OCCURRENCES],
}

impl TransposeMidiRuntime {
    fn map_pitch(&self, pitch: u8) -> Result<Option<u8>, NativeMidiProcessorError> {
        let shifted = i16::from(pitch) + i16::from(self.config.semitones);
        if (0..=127).contains(&shifted) {
            return Ok(Some(shifted as u8));
        }
        match self.config.policy {
            TransposeOutOfRangePolicy::Refuse => Err(NativeMidiProcessorError::InvalidEventPayload),
            TransposeOutOfRangePolicy::Clamp => Ok(Some(shifted.clamp(0, 127) as u8)),
            TransposeOutOfRangePolicy::Drop => Ok(None),
        }
    }

    fn find_slot(
        slots: &[Option<TrackedTransposeOccurrence>],
        occurrence_id: u64,
    ) -> Option<usize> {
        slots
            .iter()
            .position(|slot| slot.is_some_and(|entry| entry.occurrence_id == occurrence_id))
    }

    fn find_free_slot(slots: &[Option<TrackedTransposeOccurrence>]) -> Option<usize> {
        slots.iter().position(Option::is_none)
    }

    fn count_active(slots: &[Option<TrackedTransposeOccurrence>]) -> usize {
        slots.iter().flatten().count()
    }
}

impl MutableMidiProcessorRuntime for TransposeMidiRuntime {
    fn preflight_block(
        &self,
        input: &[NativeInstrumentEvent],
        _frame_count: usize,
        output_capacity: usize,
        _clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<usize, NativeMidiProcessorError> {
        if input.len() > self.config.max_input_events {
            return Err(NativeMidiProcessorError::InputEventCapacityExceeded);
        }
        let mut simulated = self.active;
        let active_cap = self.config.max_active_occurrences;
        let mut output_count = 0_usize;
        let mut work_units = 0_usize;

        for &event in input {
            work_units = work_units
                .checked_add(1)
                .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
            match event {
                NativeInstrumentEvent::NoteOn {
                    occurrence_id,
                    pitch,
                    ..
                } => {
                    if Self::find_slot(&simulated[..active_cap], occurrence_id).is_some() {
                        return Err(NativeMidiProcessorError::DuplicateOccurrence);
                    }
                    let free = Self::find_free_slot(&simulated[..active_cap])
                        .ok_or(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded)?;
                    let emitted_pitch = self.map_pitch(pitch)?;
                    simulated[free] = Some(TrackedTransposeOccurrence {
                        occurrence_id,
                        emitted_pitch,
                    });
                    if emitted_pitch.is_some() {
                        output_count = output_count
                            .checked_add(1)
                            .ok_or(NativeMidiProcessorError::OutputEventCapacityExceeded)?;
                        work_units = work_units
                            .checked_add(1)
                            .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
                    }
                }
                NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    let slot = Self::find_slot(&simulated[..active_cap], occurrence_id)
                        .ok_or(NativeMidiProcessorError::OrphanedNoteOff)?;
                    let Some(entry) = simulated[slot].take() else {
                        return Err(NativeMidiProcessorError::OrphanedNoteOff);
                    };
                    if entry.emitted_pitch.is_some() {
                        output_count = output_count
                            .checked_add(1)
                            .ok_or(NativeMidiProcessorError::OutputEventCapacityExceeded)?;
                        work_units = work_units
                            .checked_add(1)
                            .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
                    }
                }
                NativeInstrumentEvent::NotePressure { occurrence_id, .. }
                | NativeInstrumentEvent::NotePitchBend { occurrence_id, .. }
                | NativeInstrumentEvent::NoteTimbre { occurrence_id, .. } => {
                    let slot = Self::find_slot(&simulated[..active_cap], occurrence_id.get())
                        .ok_or(NativeMidiProcessorError::InvalidInputEventBatch)?;
                    let Some(entry) = simulated[slot] else {
                        return Err(NativeMidiProcessorError::InvalidInputEventBatch);
                    };
                    if entry.emitted_pitch.is_some() {
                        output_count = output_count
                            .checked_add(1)
                            .ok_or(NativeMidiProcessorError::OutputEventCapacityExceeded)?;
                        work_units = work_units
                            .checked_add(1)
                            .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
                    }
                }
                NativeInstrumentEvent::Expression { .. }
                | NativeInstrumentEvent::ControlChange { .. }
                | NativeInstrumentEvent::PitchBend { .. } => {
                    output_count = output_count
                        .checked_add(1)
                        .ok_or(NativeMidiProcessorError::OutputEventCapacityExceeded)?;
                    work_units = work_units
                        .checked_add(1)
                        .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
                }
            }
            if output_count > output_capacity || output_count > self.config.max_output_events {
                return Err(NativeMidiProcessorError::OutputEventCapacityExceeded);
            }
            if work_units > self.config.max_work_units {
                return Err(NativeMidiProcessorError::WorkBudgetExceeded);
            }
        }
        Ok(output_count)
    }

    fn process_block(
        &mut self,
        input: &[NativeInstrumentEvent],
        _frame_count: usize,
        output: &mut MidiProcessorOutput<'_>,
        _clock: Option<&dyn NativeInstrumentClock>,
    ) {
        let active_cap = self.config.max_active_occurrences;
        for &event in input {
            let _ = output.charge_work(1);
            match event {
                NativeInstrumentEvent::NoteOn {
                    frame_offset,
                    occurrence_id,
                    pitch,
                    velocity,
                } => {
                    let Ok(emitted_pitch) = self.map_pitch(pitch) else {
                        continue;
                    };
                    if let Some(free) = Self::find_free_slot(&self.active[..active_cap]) {
                        self.active[free] = Some(TrackedTransposeOccurrence {
                            occurrence_id,
                            emitted_pitch,
                        });
                    }
                    if let Some(transposed) = emitted_pitch {
                        let _ = output.charge_work(1);
                        let _ = output.push(NativeInstrumentEvent::NoteOn {
                            frame_offset,
                            occurrence_id,
                            pitch: transposed,
                            velocity,
                        });
                    }
                }
                NativeInstrumentEvent::NoteOff {
                    frame_offset,
                    occurrence_id,
                } => {
                    if let Some(slot) = Self::find_slot(&self.active[..active_cap], occurrence_id)
                        && let Some(entry) = self.active[slot].take()
                        && entry.emitted_pitch.is_some()
                    {
                        let _ = output.charge_work(1);
                        let _ = output.push(NativeInstrumentEvent::NoteOff {
                            frame_offset,
                            occurrence_id,
                        });
                    }
                }
                NativeInstrumentEvent::NotePressure { occurrence_id, .. }
                | NativeInstrumentEvent::NotePitchBend { occurrence_id, .. }
                | NativeInstrumentEvent::NoteTimbre { occurrence_id, .. } => {
                    if let Some(slot) =
                        Self::find_slot(&self.active[..active_cap], occurrence_id.get())
                        && self.active[slot].is_some_and(|entry| entry.emitted_pitch.is_some())
                    {
                        let _ = output.charge_work(1);
                        let _ = output.push(event);
                    }
                }
                NativeInstrumentEvent::Expression { .. }
                | NativeInstrumentEvent::ControlChange { .. }
                | NativeInstrumentEvent::PitchBend { .. } => {
                    let _ = output.charge_work(1);
                    let _ = output.push(event);
                }
            }
        }
    }

    fn preflight_flush_active_note_offs(
        &self,
        frame_offset: usize,
        frame_count: usize,
        output_capacity: usize,
    ) -> Result<usize, NativeMidiProcessorError> {
        if frame_count == 0 || frame_offset >= frame_count {
            return Err(NativeMidiProcessorError::InvalidInputEventBatch);
        }
        let emitted = self.active[..self.config.max_active_occurrences]
            .iter()
            .flatten()
            .filter(|entry| entry.emitted_pitch.is_some())
            .count();
        if emitted > output_capacity || emitted > self.config.max_output_events {
            return Err(NativeMidiProcessorError::OutputEventCapacityExceeded);
        }
        if emitted > self.config.max_work_units {
            return Err(NativeMidiProcessorError::WorkBudgetExceeded);
        }
        Ok(emitted)
    }

    fn flush_active_note_offs(
        &mut self,
        frame_offset: usize,
        output: &mut MidiProcessorOutput<'_>,
    ) {
        let active_cap = self.config.max_active_occurrences;
        let mut ids = [0_u64; TRANSPOSE_MAX_ACTIVE_OCCURRENCES];
        let mut len = 0_usize;
        for slot in &mut self.active[..active_cap] {
            if let Some(entry) = slot.take()
                && entry.emitted_pitch.is_some()
            {
                ids[len] = entry.occurrence_id;
                len += 1;
            }
        }
        ids[..len].sort_unstable();
        for &occurrence_id in &ids[..len] {
            let _ = output.charge_work(1);
            let _ = output.push(NativeInstrumentEvent::NoteOff {
                frame_offset,
                occurrence_id,
            });
        }
    }

    fn active_occurrence_count(&self) -> usize {
        Self::count_active(&self.active[..self.config.max_active_occurrences])
    }

    fn reset(&mut self) {
        self.active.fill(None);
    }

    fn can_copy_history_from(&self, previous: &dyn MutableMidiProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|other| self.config == other.config)
    }

    fn copy_history_from(&mut self, previous: &dyn MutableMidiProcessorRuntime) {
        if let Some(other) = previous.as_any().downcast_ref::<Self>()
            && self.config == other.config
        {
            self.active = other.active;
        }
    }

    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
