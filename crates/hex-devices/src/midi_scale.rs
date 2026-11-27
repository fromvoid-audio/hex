//! Original native MIDI Scale processor using the bounded `hex-lattice`
//! MIDI processor contract (`hex.midi.scale`).
//!
//! Applies an explicit finite 12-degree pitch-class map relative to a
//! configurable `base` root (`c..b`), optional semitone `transpose` (`-36..=+36`),
//! input note window (`lowest`, `range`), and explicit overflow boundary policy
//! (`clamp`, `drop`, or octave `fold`). Any degree may also be set to `off`
//! (`12`) to suppress notes on that pitch class while preserving exact
//! `occurrence_id` onset-to-off pairing and zero stuck notes.

use hex_lattice::{
    MidiProcessorOutput, MutableMidiProcessorRuntime, NativeInstrumentClock, NativeInstrumentEvent,
    NativeMidiProcessorDefinition, NativeMidiProcessorError, PreparedNativeMidiProcessor,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;

pub const SCALE_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.midi.scale");
pub const SCALE_STATE_VERSION: u32 = 1;
pub const SCALE_RUNTIME_VERSION: u32 = 1;

pub const DEGREE_COUNT: usize = 12;
pub const DEGREE_OFF: u8 = 12;
pub const MIN_TRANSPOSE: i8 = -36;
pub const MAX_TRANSPOSE: i8 = 36;
pub const MAX_MIDI_NOTE: u8 = 127;

pub const SCALE_MAX_INPUT_EVENTS: usize = 259;
pub const SCALE_MAX_OUTPUT_EVENTS: usize = 259;
pub const SCALE_MAX_ACTIVE_OCCURRENCES: usize = 64;
pub const SCALE_MAX_WORK_UNITS: usize = SCALE_MAX_INPUT_EVENTS * 2;

/// `[base, transpose_i8 as u8, lowest, range, overflow, degree_map[0..12]]`
const STATE_BYTES: usize = 5 + DEGREE_COUNT;

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const BASE: ParameterId = ParameterId::from_static(1);
    pub const TRANSPOSE: ParameterId = ParameterId::from_static(2);
    pub const LOWEST: ParameterId = ParameterId::from_static(3);
    pub const RANGE: ParameterId = ParameterId::from_static(4);
    pub const OVERFLOW: ParameterId = ParameterId::from_static(5);

    pub const DEGREE_0: ParameterId = ParameterId::from_static(6);
    pub const DEGREE_1: ParameterId = ParameterId::from_static(7);
    pub const DEGREE_2: ParameterId = ParameterId::from_static(8);
    pub const DEGREE_3: ParameterId = ParameterId::from_static(9);
    pub const DEGREE_4: ParameterId = ParameterId::from_static(10);
    pub const DEGREE_5: ParameterId = ParameterId::from_static(11);
    pub const DEGREE_6: ParameterId = ParameterId::from_static(12);
    pub const DEGREE_7: ParameterId = ParameterId::from_static(13);
    pub const DEGREE_8: ParameterId = ParameterId::from_static(14);
    pub const DEGREE_9: ParameterId = ParameterId::from_static(15);
    pub const DEGREE_10: ParameterId = ParameterId::from_static(16);
    pub const DEGREE_11: ParameterId = ParameterId::from_static(17);
}

const NOTE_LABELS: [&str; 12] = [
    "c", "c#", "d", "d#", "e", "f", "f#", "g", "g#", "a", "a#", "b",
];
const OVERFLOW_LABELS: [&str; 3] = ["clamp", "drop", "fold"];
const DEGREE_TARGET_LABELS: [&str; 13] = [
    "0", "+1", "+2", "+3", "+4", "+5", "+6", "+7", "+8", "+9", "+10", "+11", "off",
];

const fn degree_descriptor(
    id: ParameterId,
    label: &'static str,
    default_degree: u32,
) -> ParameterDescriptor {
    ParameterDescriptor {
        id,
        label,
        kind: ParameterKind::Choice {
            labels: &DEGREE_TARGET_LABELS,
        },
        default: ParameterValue::Choice(default_degree),
        presentation: ParameterPresentation::Linear,
    }
}

const DESCRIPTORS: [ParameterDescriptor; 17] = [
    ParameterDescriptor {
        id: parameter::BASE,
        label: "base",
        kind: ParameterKind::Choice {
            labels: &NOTE_LABELS,
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::TRANSPOSE,
        label: "transpose",
        kind: ParameterKind::Float {
            min: MIN_TRANSPOSE as f32,
            max: MAX_TRANSPOSE as f32,
            unit: Some("st"),
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::LOWEST,
        label: "lowest",
        kind: ParameterKind::Float {
            min: 0.0,
            max: MAX_MIDI_NOTE as f32,
            unit: None,
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::RANGE,
        label: "range",
        kind: ParameterKind::Float {
            min: 0.0,
            max: MAX_MIDI_NOTE as f32,
            unit: Some("st"),
        },
        default: ParameterValue::Float(MAX_MIDI_NOTE as f32),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::OVERFLOW,
        label: "overflow",
        kind: ParameterKind::Choice {
            labels: &OVERFLOW_LABELS,
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    degree_descriptor(parameter::DEGREE_0, "deg 1", 0),
    degree_descriptor(parameter::DEGREE_1, "deg 2", 1),
    degree_descriptor(parameter::DEGREE_2, "deg 3", 2),
    degree_descriptor(parameter::DEGREE_3, "deg 4", 3),
    degree_descriptor(parameter::DEGREE_4, "deg 5", 4),
    degree_descriptor(parameter::DEGREE_5, "deg 6", 5),
    degree_descriptor(parameter::DEGREE_6, "deg 7", 6),
    degree_descriptor(parameter::DEGREE_7, "deg 8", 7),
    degree_descriptor(parameter::DEGREE_8, "deg 9", 8),
    degree_descriptor(parameter::DEGREE_9, "deg 10", 9),
    degree_descriptor(parameter::DEGREE_10, "deg 11", 10),
    degree_descriptor(parameter::DEGREE_11, "deg 12", 11),
];

/// Explicit boundary policy when a mapped/transposed pitch leaves MIDI `0..=127`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ScaleOverflowPolicy {
    Clamp = 0,
    Drop = 1,
    Fold = 2,
}

impl ScaleOverflowPolicy {
    pub const fn from_choice(choice: u32) -> Option<Self> {
        match choice {
            0 => Some(Self::Clamp),
            1 => Some(Self::Drop),
            2 => Some(Self::Fold),
            _ => None,
        }
    }

    pub const fn as_choice(self) -> u32 {
        self as u32
    }
}

/// Validated authored settings for `hex.midi.scale`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScaleSettings {
    /// Root pitch class `0..=11` (`0` = C).
    pub base: u8,
    /// Post-map semitone transpose `-36..=+36`.
    pub transpose: i8,
    /// Input note window lower bound `0..=127`.
    pub lowest: u8,
    /// Input note window width `0..=127`.
    pub range: u8,
    pub overflow: ScaleOverflowPolicy,
    /// For each relative input degree `0..=11`, target relative degree `0..=11` or `DEGREE_OFF (12)`.
    pub degree_map: [u8; DEGREE_COUNT],
}

impl Default for ScaleSettings {
    fn default() -> Self {
        Self {
            base: 0,
            transpose: 0,
            lowest: 0,
            range: MAX_MIDI_NOTE,
            overflow: ScaleOverflowPolicy::Clamp,
            degree_map: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        }
    }
}

impl ScaleSettings {
    pub fn encode(self) -> Result<NativeDeviceState, DeviceContractError> {
        self.validate()?;
        let mut payload = Vec::with_capacity(STATE_BYTES);
        payload.push(self.base);
        payload.push(self.transpose as u8);
        payload.push(self.lowest);
        payload.push(self.range);
        payload.push(self.overflow as u8);
        payload.extend_from_slice(&self.degree_map);
        NativeDeviceState::new(SCALE_TYPE_ID, SCALE_STATE_VERSION, payload)
    }

    pub fn decode(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != SCALE_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != SCALE_STATE_VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        if !state.asset_references().is_empty() {
            return Err(DeviceContractError::AssetResourcesUnavailable);
        }
        let payload = state.payload();
        if payload.len() != STATE_BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let overflow = ScaleOverflowPolicy::from_choice(u32::from(payload[4]))
            .ok_or(DeviceContractError::MalformedState)?;
        let mut degree_map = [0_u8; DEGREE_COUNT];
        degree_map.copy_from_slice(&payload[5..]);
        let settings = Self {
            base: payload[0],
            transpose: payload[1] as i8,
            lowest: payload[2],
            range: payload[3],
            overflow,
            degree_map,
        };
        settings.validate()?;
        Ok(settings)
    }

    fn validate(self) -> Result<(), DeviceContractError> {
        if self.base >= DEGREE_COUNT as u8
            || !(MIN_TRANSPOSE..=MAX_TRANSPOSE).contains(&self.transpose)
            || self.lowest > MAX_MIDI_NOTE
            || self.range > MAX_MIDI_NOTE
            || self.degree_map.iter().any(|&d| d > DEGREE_OFF)
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(())
    }

    /// Map an incoming MIDI note-on pitch (`0..=127`) through the pitch-class
    /// table, transpose, and boundary policy. Returns `None` when gated outside
    /// `[lowest, lowest + range]`, mapped to `DEGREE_OFF`, or dropped at bounds.
    pub const fn map_pitch(self, pitch: u8) -> Option<u8> {
        if pitch > MAX_MIDI_NOTE {
            return None;
        }
        let upper = (self.lowest as u16) + (self.range as u16);
        if (pitch as u16) < (self.lowest as u16) || (pitch as u16) > upper {
            return None;
        }
        let rel = (pitch as i16) - (self.base as i16);
        let octave = rel.div_euclid(12);
        let degree = rel.rem_euclid(12) as usize;
        let mapped_degree = self.degree_map[degree];
        if mapped_degree == DEGREE_OFF {
            return None;
        }
        let target =
            octave * 12 + (self.base as i16) + (mapped_degree as i16) + (self.transpose as i16);
        if target >= 0 && target <= MAX_MIDI_NOTE as i16 {
            return Some(target as u8);
        }
        match self.overflow {
            ScaleOverflowPolicy::Clamp => {
                if target < 0 {
                    Some(0)
                } else {
                    Some(MAX_MIDI_NOTE)
                }
            }
            ScaleOverflowPolicy::Drop => None,
            ScaleOverflowPolicy::Fold => {
                if target < 0 {
                    Some(target.rem_euclid(12) as u8)
                } else {
                    Some((127 - (127 - target).rem_euclid(12)) as u8)
                }
            }
        }
    }
}

pub static SCALE_DEFINITION: ScaleMidiProcessorDefinition = ScaleMidiProcessorDefinition;

pub struct ScaleMidiProcessorDefinition;

fn degree_index(parameter: ParameterId) -> Option<usize> {
    match parameter {
        parameter::DEGREE_0 => Some(0),
        parameter::DEGREE_1 => Some(1),
        parameter::DEGREE_2 => Some(2),
        parameter::DEGREE_3 => Some(3),
        parameter::DEGREE_4 => Some(4),
        parameter::DEGREE_5 => Some(5),
        parameter::DEGREE_6 => Some(6),
        parameter::DEGREE_7 => Some(7),
        parameter::DEGREE_8 => Some(8),
        parameter::DEGREE_9 => Some(9),
        parameter::DEGREE_10 => Some(10),
        parameter::DEGREE_11 => Some(11),
        _ => None,
    }
}

impl NativeDeviceMetadata for ScaleMidiProcessorDefinition {
    fn type_id(&self) -> DeviceTypeId {
        SCALE_TYPE_ID
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("scale")
    }

    fn state_version(&self) -> u32 {
        SCALE_STATE_VERSION
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &DESCRIPTORS
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        ScaleSettings::default().encode()
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let settings = ScaleSettings::decode(state)?;
        settings.encode()?.with_host_metadata_from(state)
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let settings = ScaleSettings::decode(state)?;
        if let Some(i) = degree_index(parameter) {
            return Ok(ParameterValue::Choice(u32::from(settings.degree_map[i])));
        }
        match parameter {
            parameter::BASE => Ok(ParameterValue::Choice(u32::from(settings.base))),
            parameter::TRANSPOSE => Ok(ParameterValue::Float(f32::from(settings.transpose))),
            parameter::LOWEST => Ok(ParameterValue::Float(f32::from(settings.lowest))),
            parameter::RANGE => Ok(ParameterValue::Float(f32::from(settings.range))),
            parameter::OVERFLOW => Ok(ParameterValue::Choice(settings.overflow.as_choice())),
            _ => Err(DeviceContractError::UnknownParameter),
        }
    }

    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut settings = ScaleSettings::decode(state)?;
        if let Some(i) = degree_index(parameter) {
            let ParameterValue::Choice(choice) = value else {
                return Err(DeviceContractError::InvalidParameterValue);
            };
            let target =
                u8::try_from(choice).map_err(|_| DeviceContractError::InvalidParameterValue)?;
            if target > DEGREE_OFF {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            settings.degree_map[i] = target;
            return settings.encode()?.with_host_metadata_from(state);
        }
        match (parameter, value) {
            (parameter::BASE, ParameterValue::Choice(choice)) => {
                let base =
                    u8::try_from(choice).map_err(|_| DeviceContractError::InvalidParameterValue)?;
                if base >= DEGREE_COUNT as u8 {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.base = base;
            }
            (parameter::TRANSPOSE, ParameterValue::Float(v)) if v.is_finite() => {
                let r = v.round();
                if r < f32::from(MIN_TRANSPOSE) || r > f32::from(MAX_TRANSPOSE) {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.transpose = r as i8;
            }
            (parameter::LOWEST, ParameterValue::Float(v)) if v.is_finite() => {
                let r = v.round();
                if r < 0.0 || r > f32::from(MAX_MIDI_NOTE) {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.lowest = r as u8;
            }
            (parameter::RANGE, ParameterValue::Float(v)) if v.is_finite() => {
                let r = v.round();
                if r < 0.0 || r > f32::from(MAX_MIDI_NOTE) {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.range = r as u8;
            }
            (parameter::OVERFLOW, ParameterValue::Choice(choice)) => {
                settings.overflow = ScaleOverflowPolicy::from_choice(choice)
                    .ok_or(DeviceContractError::InvalidParameterValue)?;
            }
            (
                parameter::BASE
                | parameter::TRANSPOSE
                | parameter::LOWEST
                | parameter::RANGE
                | parameter::OVERFLOW,
                _,
            ) => return Err(DeviceContractError::InvalidParameterValue),
            _ => return Err(DeviceContractError::UnknownParameter),
        }
        settings.encode()?.with_host_metadata_from(state)
    }
}

impl NativeMidiProcessorDefinition for ScaleMidiProcessorDefinition {
    fn prepare_midi_processor(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Box<dyn PreparedNativeMidiProcessor>, DeviceContractError> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let settings = ScaleSettings::decode(state)?;
        Ok(Box::new(PreparedScaleMidiProcessor::new(settings)?))
    }
}

/// Control-prepared immutable factory for `hex.midi.scale`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedScaleMidiProcessor {
    settings: ScaleSettings,
    max_input_events: usize,
    max_output_events: usize,
    max_active_occurrences: usize,
    max_work_units: usize,
}

impl PreparedScaleMidiProcessor {
    pub fn new(settings: ScaleSettings) -> Result<Self, DeviceContractError> {
        Self::with_caps(
            settings,
            SCALE_MAX_INPUT_EVENTS,
            SCALE_MAX_OUTPUT_EVENTS,
            SCALE_MAX_ACTIVE_OCCURRENCES,
            SCALE_MAX_WORK_UNITS,
        )
    }

    pub fn with_caps(
        settings: ScaleSettings,
        max_input_events: usize,
        max_output_events: usize,
        max_active_occurrences: usize,
        max_work_units: usize,
    ) -> Result<Self, DeviceContractError> {
        settings.validate()?;
        if max_input_events == 0
            || max_output_events == 0
            || max_active_occurrences == 0
            || max_active_occurrences > SCALE_MAX_ACTIVE_OCCURRENCES
            || max_work_units == 0
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(Self {
            settings,
            max_input_events,
            max_output_events,
            max_active_occurrences,
            max_work_units,
        })
    }

    pub const fn settings(self) -> ScaleSettings {
        self.settings
    }
}

impl PreparedNativeMidiProcessor for PreparedScaleMidiProcessor {
    fn runtime_version(&self) -> u32 {
        SCALE_RUNTIME_VERSION
    }

    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<ScaleMidiRuntime>()
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
        Box::new(ScaleMidiRuntime {
            config: *self,
            active: [None; SCALE_MAX_ACTIVE_OCCURRENCES],
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TrackedScaleOccurrence {
    occurrence_id: u64,
    emitted: bool,
}

#[derive(Clone)]
struct ScaleMidiRuntime {
    config: PreparedScaleMidiProcessor,
    active: [Option<TrackedScaleOccurrence>; SCALE_MAX_ACTIVE_OCCURRENCES],
}

impl ScaleMidiRuntime {
    fn find_slot(slots: &[Option<TrackedScaleOccurrence>], occurrence_id: u64) -> Option<usize> {
        slots
            .iter()
            .position(|slot| slot.is_some_and(|entry| entry.occurrence_id == occurrence_id))
    }

    fn find_free_slot(slots: &[Option<TrackedScaleOccurrence>]) -> Option<usize> {
        slots.iter().position(Option::is_none)
    }

    fn count_active(slots: &[Option<TrackedScaleOccurrence>]) -> usize {
        slots.iter().flatten().count()
    }
}

impl MutableMidiProcessorRuntime for ScaleMidiRuntime {
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
                    let emitted = self.config.settings.map_pitch(pitch).is_some();
                    simulated[free] = Some(TrackedScaleOccurrence {
                        occurrence_id,
                        emitted,
                    });
                    if emitted {
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
                    if entry.emitted {
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
                    if entry.emitted {
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
                    let mapped = self.config.settings.map_pitch(pitch);
                    if let Some(free) = Self::find_free_slot(&self.active[..active_cap]) {
                        self.active[free] = Some(TrackedScaleOccurrence {
                            occurrence_id,
                            emitted: mapped.is_some(),
                        });
                    }
                    if let Some(scaled_pitch) = mapped {
                        let _ = output.charge_work(1);
                        let _ = output.push(NativeInstrumentEvent::NoteOn {
                            frame_offset,
                            occurrence_id,
                            pitch: scaled_pitch,
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
                        && entry.emitted
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
                        && self.active[slot].is_some_and(|entry| entry.emitted)
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
            .filter(|entry| entry.emitted)
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
        let mut ids = [0_u64; SCALE_MAX_ACTIVE_OCCURRENCES];
        let mut len = 0_usize;
        for slot in &mut self.active[..active_cap] {
            if let Some(entry) = slot.take()
                && entry.emitted
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
            .is_some_and(|other| {
                other.active_occurrence_count() <= self.config.max_active_occurrences
            })
    }

    fn copy_history_from(&mut self, previous: &dyn MutableMidiProcessorRuntime) {
        let Some(other) = previous.as_any().downcast_ref::<Self>() else {
            return;
        };
        self.active.fill(None);
        let active_cap = self.config.max_active_occurrences;
        for (write, entry) in other.active[..other.config.max_active_occurrences]
            .iter()
            .flatten()
            .enumerate()
        {
            if write >= active_cap {
                break;
            }
            self.active[write] = Some(*entry);
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
    use hex_lattice::{NativeMidiProcessorLimits, NativeMidiProcessorRuntimePair};
    use std::collections::BTreeMap;

    fn test_limits() -> NativeMidiProcessorLimits {
        NativeMidiProcessorLimits {
            max_pair_bytes: 256 * 1024,
            max_input_events_per_block: SCALE_MAX_INPUT_EVENTS,
            max_output_events_per_block: SCALE_MAX_OUTPUT_EVENTS,
            max_active_occurrences: SCALE_MAX_ACTIVE_OCCURRENCES,
            max_work_units_per_block: SCALE_MAX_WORK_UNITS * 2,
        }
    }

    fn make_pair(settings: ScaleSettings) -> NativeMidiProcessorRuntimePair {
        let prepared = PreparedScaleMidiProcessor::new(settings).unwrap();
        NativeMidiProcessorRuntimePair::new(&prepared, test_limits()).unwrap()
    }

    #[test]
    fn scale_pitch_class_mapping_boundaries_off_degrees_and_codec() {
        let registry = crate::native_midi_processor_registry().unwrap();
        let default = registry.default_state(SCALE_TYPE_ID).unwrap();
        assert_eq!(
            ScaleSettings::decode(&default).unwrap(),
            ScaleSettings::default()
        );
        for p in 0..=127 {
            assert_eq!(ScaleSettings::default().map_pitch(p), Some(p));
        }

        // C major quantization (sharps snap down; tritone F# suppressed via DEGREE_OFF).
        let c_major = ScaleSettings {
            base: 0,
            degree_map: [0, 0, 2, 2, 4, 5, DEGREE_OFF, 7, 7, 9, 9, 11],
            ..ScaleSettings::default()
        };
        assert_eq!(c_major.map_pitch(60), Some(60)); // C4 -> C4
        assert_eq!(c_major.map_pitch(61), Some(60)); // C#4 -> C4
        assert_eq!(c_major.map_pitch(66), None); // F#4 -> OFF
        assert_eq!(c_major.map_pitch(68), Some(67)); // G#4 -> G4

        // Non-zero base (D = 2) + transpose +12 + Drop overflow at upper boundary.
        let d_scale = ScaleSettings {
            base: 2,
            transpose: 12,
            overflow: ScaleOverflowPolicy::Drop,
            degree_map: [0, 0, 2, 3, 4, 5, 5, 7, 7, 9, 10, 11],
            ..ScaleSettings::default()
        };
        assert_eq!(d_scale.map_pitch(63), Some(74)); // D#4 (deg 1 above D) -> D4 + 12 = D5 (74)
        assert_eq!(d_scale.map_pitch(122), None); // D9 (122) + 12 = 134 -> Drop

        // Fold overflow at lower boundary.
        let fold_low = ScaleSettings {
            transpose: -12,
            overflow: ScaleOverflowPolicy::Fold,
            ..ScaleSettings::default()
        };
        assert_eq!(fold_low.map_pitch(3), Some(3));

        // Malformed states refuse deterministically.
        let bad_base = NativeDeviceState::new(SCALE_TYPE_ID, 1, vec![12; STATE_BYTES]).unwrap();
        assert!(ScaleSettings::decode(&bad_base).is_err());
        let bad_deg = NativeDeviceState::new(
            SCALE_TYPE_ID,
            1,
            vec![0, 0, 0, 127, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 99],
        )
        .unwrap();
        assert_eq!(
            ScaleSettings::decode(&bad_deg),
            Err(DeviceContractError::InvalidParameterValue)
        );
    }

    #[test]
    fn scale_unmapped_off_overlaps_live_edits_and_flush_preserve_ledger() {
        let mut pair = make_pair(ScaleSettings {
            base: 0,
            degree_map: [0, DEGREE_OFF, 2, 2, 4, 5, 5, 7, 7, 9, 9, 11],
            ..ScaleSettings::default()
        });
        let mut out = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; 16];
        let mut ledger = BTreeMap::new();

        // Overlapping C#4 (suppressed) + two C4s (mapped to 60).
        let b1 = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 61, // OFF
                velocity: 100,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 2,
                occurrence_id: 2,
                pitch: 60,
                velocity: 100,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 4,
                occurrence_id: 3,
                pitch: 60,
                velocity: 90,
            },
        ];
        pair.begin_block();
        let n1 = pair.process_block(&b1, 64, &mut out, None).unwrap();
        pair.commit_block();
        assert_eq!(n1, 2);
        for &e in &out[..n1] {
            if let NativeInstrumentEvent::NoteOn {
                occurrence_id,
                pitch,
                ..
            } = e
            {
                assert!(ledger.insert(occurrence_id, pitch).is_none());
            }
        }

        // Live edit scale while held so degree 1 now maps to +2 (D).
        let mut pair_d = make_pair(ScaleSettings {
            base: 0,
            degree_map: [0, 2, 2, 2, 4, 5, 5, 7, 7, 9, 9, 11],
            ..ScaleSettings::default()
        });
        assert!(pair_d.can_inherit_from(&pair));
        pair_d.inherit_from(&pair).unwrap();

        let b2 = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 4,
                pitch: 61, // now maps to D4 (62)
                velocity: 100,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 2,
                occurrence_id: 1, // stays suppressed cleanly
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 4,
                occurrence_id: 2,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 6,
                occurrence_id: 3,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 8,
                occurrence_id: 4,
            },
        ];
        pair_d.begin_block();
        let n2 = pair_d.process_block(&b2, 64, &mut out, None).unwrap();
        pair_d.commit_block();
        assert_eq!(n2, 4);
        for &e in &out[..n2] {
            match e {
                NativeInstrumentEvent::NoteOn {
                    occurrence_id,
                    pitch,
                    ..
                } => {
                    assert_eq!(pitch, 62);
                    assert!(ledger.insert(occurrence_id, pitch).is_none());
                }
                NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    assert!(ledger.remove(&occurrence_id).is_some());
                }
                _ => {}
            }
        }
        assert!(ledger.is_empty());
        assert!(pair_d.committed_is_quiescent());
    }
}
