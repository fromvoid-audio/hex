//! Original native MIDI Control Change device using the bounded `hex-lattice`
//! MIDI processor contract (`hex.midi.cc_control`).
//!
//! Provides two finite configurable CC controller lanes (`a` and `b`) with
//! explicit `enable`, controller number (`0..=127`), target `value` (`0..=127`),
//! linear output range (`min..=max`, supporting inverted ranges), explicit
//! `channel` metadata (`1..=16`), per-block step `slew` rate limit (`0` =
//! immediate, `1..=127` = max value delta per block), and upstream controller
//! `conflict` policy (`override`, `pass`, `block`). Emits bounded
//! `NativeInstrumentEvent::ControlChange` events strictly on change at
//! `frame_offset: 0` ahead of same-offset notes while preserving exact
//! occurrence-owned `NoteOn`/`NoteOff` tracking and zero callback allocations.

use hex_lattice::{
    MidiProcessorOutput, MutableMidiProcessorRuntime, NativeInstrumentClock, NativeInstrumentEvent,
    NativeMidiProcessorDefinition, NativeMidiProcessorError, PreparedNativeMidiProcessor,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;

pub const CC_CONTROL_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.midi.cc_control");
pub const CC_CONTROL_STATE_VERSION: u32 = 1;
pub const CC_CONTROL_RUNTIME_VERSION: u32 = 1;

pub const CC_SLOT_COUNT: usize = 2;
pub const MAX_MIDI_VALUE: u8 = 127;
pub const MAX_MIDI_CHANNEL: u8 = 15;

pub const CC_CONTROL_MAX_INPUT_EVENTS: usize = 259;
pub const CC_CONTROL_MAX_OUTPUT_EVENTS: usize = 259;
pub const CC_CONTROL_MAX_ACTIVE_OCCURRENCES: usize = 64;
pub const CC_CONTROL_MAX_WORK_UNITS: usize = CC_CONTROL_MAX_INPUT_EVENTS * 2 + CC_SLOT_COUNT * 2;

/// `[channel, conflict, slew, (enabled, cc, value, min, max) * 2]`
const STATE_BYTES: usize = 3 + CC_SLOT_COUNT * 5;

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const CHANNEL: ParameterId = ParameterId::from_static(1);
    pub const CONFLICT: ParameterId = ParameterId::from_static(2);
    pub const SLEW: ParameterId = ParameterId::from_static(3);

    pub const A_ENABLE: ParameterId = ParameterId::from_static(4);
    pub const A_CC: ParameterId = ParameterId::from_static(5);
    pub const A_VALUE: ParameterId = ParameterId::from_static(6);
    pub const A_MIN: ParameterId = ParameterId::from_static(7);
    pub const A_MAX: ParameterId = ParameterId::from_static(8);

    pub const B_ENABLE: ParameterId = ParameterId::from_static(9);
    pub const B_CC: ParameterId = ParameterId::from_static(10);
    pub const B_VALUE: ParameterId = ParameterId::from_static(11);
    pub const B_MIN: ParameterId = ParameterId::from_static(12);
    pub const B_MAX: ParameterId = ParameterId::from_static(13);
}

const CONFLICT_LABELS: [&str; 3] = ["override", "pass", "block"];
const ENABLE_LABELS: [&str; 2] = ["off", "on"];

const DESCRIPTORS: [ParameterDescriptor; 13] = [
    ParameterDescriptor {
        id: parameter::CHANNEL,
        label: "channel",
        kind: ParameterKind::Float {
            min: 1.0,
            max: 16.0,
            unit: None,
        },
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::CONFLICT,
        label: "conflict",
        kind: ParameterKind::Choice {
            labels: &CONFLICT_LABELS,
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::SLEW,
        label: "slew",
        kind: ParameterKind::Float {
            min: 0.0,
            max: MAX_MIDI_VALUE as f32,
            unit: None,
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::A_ENABLE,
        label: "cc a",
        kind: ParameterKind::Choice {
            labels: &ENABLE_LABELS,
        },
        default: ParameterValue::Choice(1),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::A_CC,
        label: "cc a num",
        kind: ParameterKind::Float {
            min: 0.0,
            max: MAX_MIDI_VALUE as f32,
            unit: None,
        },
        default: ParameterValue::Float(74.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::A_VALUE,
        label: "cc a val",
        kind: ParameterKind::Float {
            min: 0.0,
            max: MAX_MIDI_VALUE as f32,
            unit: None,
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::A_MIN,
        label: "cc a min",
        kind: ParameterKind::Float {
            min: 0.0,
            max: MAX_MIDI_VALUE as f32,
            unit: None,
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::A_MAX,
        label: "cc a max",
        kind: ParameterKind::Float {
            min: 0.0,
            max: MAX_MIDI_VALUE as f32,
            unit: None,
        },
        default: ParameterValue::Float(MAX_MIDI_VALUE as f32),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::B_ENABLE,
        label: "cc b",
        kind: ParameterKind::Choice {
            labels: &ENABLE_LABELS,
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::B_CC,
        label: "cc b num",
        kind: ParameterKind::Float {
            min: 0.0,
            max: MAX_MIDI_VALUE as f32,
            unit: None,
        },
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::B_VALUE,
        label: "cc b val",
        kind: ParameterKind::Float {
            min: 0.0,
            max: MAX_MIDI_VALUE as f32,
            unit: None,
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::B_MIN,
        label: "cc b min",
        kind: ParameterKind::Float {
            min: 0.0,
            max: MAX_MIDI_VALUE as f32,
            unit: None,
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::B_MAX,
        label: "cc b max",
        kind: ParameterKind::Float {
            min: 0.0,
            max: MAX_MIDI_VALUE as f32,
            unit: None,
        },
        default: ParameterValue::Float(MAX_MIDI_VALUE as f32),
        presentation: ParameterPresentation::Linear,
    },
];

/// Policy for resolving conflicts between upstream incoming CCs and enabled lanes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CcConflictPolicy {
    /// Suppress matching incoming CCs and emit this lane's mapped value on change.
    Override = 0,
    /// Pass incoming CCs through in arrival order alongside lane emissions on change.
    Pass = 1,
    /// Suppress matching incoming CCs without emitting lane values.
    Block = 2,
}

impl CcConflictPolicy {
    pub const fn from_choice(choice: u32) -> Option<Self> {
        match choice {
            0 => Some(Self::Override),
            1 => Some(Self::Pass),
            2 => Some(Self::Block),
            _ => None,
        }
    }

    pub const fn as_choice(self) -> u32 {
        self as u32
    }
}

/// One bounded CC controller lane (`0..=127` controller, value, min, max).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CcLaneSettings {
    pub enabled: bool,
    pub cc: u8,
    pub value: u8,
    pub min: u8,
    pub max: u8,
}

impl CcLaneSettings {
    /// Linearly map `self.value` (`0..=127`) onto `[self.min, self.max]`,
    /// supporting inverted ranges (`min > max`) with exact integer rounding.
    pub const fn target_value(self) -> u8 {
        let min_i = self.min as i32;
        let max_i = self.max as i32;
        let diff = max_i - min_i;
        let num = diff * (self.value as i32);
        let scaled = if num >= 0 {
            min_i + (num + 63) / 127
        } else {
            min_i + (num - 63) / 127
        };
        if scaled < 0 {
            0
        } else if scaled > MAX_MIDI_VALUE as i32 {
            MAX_MIDI_VALUE
        } else {
            scaled as u8
        }
    }

    fn validate(self) -> Result<(), DeviceContractError> {
        if self.cc > MAX_MIDI_VALUE
            || self.value > MAX_MIDI_VALUE
            || self.min > MAX_MIDI_VALUE
            || self.max > MAX_MIDI_VALUE
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(())
    }
}

/// Validated authored settings for `hex.midi.cc_control`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CcControlSettings {
    /// Zero-based MIDI channel metadata (`0..=15`, displayed as `1..=16`).
    pub channel: u8,
    pub conflict: CcConflictPolicy,
    /// Per-block slew step ceiling (`0` = immediate, `1..=127` = max delta/block).
    pub slew: u8,
    pub lanes: [CcLaneSettings; CC_SLOT_COUNT],
}

impl Default for CcControlSettings {
    fn default() -> Self {
        Self {
            channel: 0,
            conflict: CcConflictPolicy::Override,
            slew: 0,
            lanes: [
                CcLaneSettings {
                    enabled: true,
                    cc: 74,
                    value: 0,
                    min: 0,
                    max: MAX_MIDI_VALUE,
                },
                CcLaneSettings {
                    enabled: false,
                    cc: 1,
                    value: 0,
                    min: 0,
                    max: MAX_MIDI_VALUE,
                },
            ],
        }
    }
}

impl CcControlSettings {
    pub fn encode(self) -> Result<NativeDeviceState, DeviceContractError> {
        self.validate()?;
        let mut payload = Vec::with_capacity(STATE_BYTES);
        payload.push(self.channel);
        payload.push(self.conflict as u8);
        payload.push(self.slew);
        for lane in self.lanes {
            payload.push(u8::from(lane.enabled));
            payload.push(lane.cc);
            payload.push(lane.value);
            payload.push(lane.min);
            payload.push(lane.max);
        }
        NativeDeviceState::new(CC_CONTROL_TYPE_ID, CC_CONTROL_STATE_VERSION, payload)
    }

    pub fn decode(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != CC_CONTROL_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != CC_CONTROL_STATE_VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        if !state.asset_references().is_empty() {
            return Err(DeviceContractError::AssetResourcesUnavailable);
        }
        let payload = state.payload();
        if payload.len() != STATE_BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let conflict = CcConflictPolicy::from_choice(u32::from(payload[1]))
            .ok_or(DeviceContractError::MalformedState)?;
        let mut lanes = [CcLaneSettings {
            enabled: false,
            cc: 0,
            value: 0,
            min: 0,
            max: MAX_MIDI_VALUE,
        }; CC_SLOT_COUNT];
        for (i, lane) in lanes.iter_mut().enumerate() {
            let base = 3 + i * 5;
            let enabled = match payload[base] {
                0 => false,
                1 => true,
                _ => return Err(DeviceContractError::MalformedState),
            };
            *lane = CcLaneSettings {
                enabled,
                cc: payload[base + 1],
                value: payload[base + 2],
                min: payload[base + 3],
                max: payload[base + 4],
            };
        }
        let settings = Self {
            channel: payload[0],
            conflict,
            slew: payload[2],
            lanes,
        };
        settings.validate()?;
        Ok(settings)
    }

    fn validate(self) -> Result<(), DeviceContractError> {
        if self.channel > MAX_MIDI_CHANNEL || self.slew > MAX_MIDI_VALUE {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        for lane in self.lanes {
            lane.validate()?;
        }
        Ok(())
    }

    fn suppresses_incoming_cc(self, cc: u8) -> bool {
        if self.conflict == CcConflictPolicy::Pass {
            return false;
        }
        self.lanes.iter().any(|lane| lane.enabled && lane.cc == cc)
    }
}

pub static CC_CONTROL_DEFINITION: CcControlMidiProcessorDefinition =
    CcControlMidiProcessorDefinition;

pub struct CcControlMidiProcessorDefinition;

fn parse_u8_param(value: ParameterValue, min: u8, max: u8) -> Result<u8, DeviceContractError> {
    let ParameterValue::Float(v) = value else {
        return Err(DeviceContractError::InvalidParameterValue);
    };
    if !v.is_finite() {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    let rounded = v.round();
    if rounded < f32::from(min) || rounded > f32::from(max) {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    Ok(rounded as u8)
}

fn parse_enable_param(value: ParameterValue) -> Result<bool, DeviceContractError> {
    match value {
        ParameterValue::Choice(0) => Ok(false),
        ParameterValue::Choice(1) => Ok(true),
        _ => Err(DeviceContractError::InvalidParameterValue),
    }
}

impl NativeDeviceMetadata for CcControlMidiProcessorDefinition {
    fn type_id(&self) -> DeviceTypeId {
        CC_CONTROL_TYPE_ID
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("cc control")
    }

    fn state_version(&self) -> u32 {
        CC_CONTROL_STATE_VERSION
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &DESCRIPTORS
    }

    fn ports(&self) -> &'static [hex_lattice_contract::DevicePortDescriptor] {
        hex_lattice_contract::MIDI_EFFECT_PORTS
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        CcControlSettings::default().encode()
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let settings = CcControlSettings::decode(state)?;
        settings.encode()?.with_host_metadata_from(state)
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let settings = CcControlSettings::decode(state)?;
        match parameter {
            parameter::CHANNEL => Ok(ParameterValue::Float(f32::from(settings.channel + 1))),
            parameter::CONFLICT => Ok(ParameterValue::Choice(settings.conflict.as_choice())),
            parameter::SLEW => Ok(ParameterValue::Float(f32::from(settings.slew))),
            parameter::A_ENABLE => Ok(ParameterValue::Choice(u32::from(settings.lanes[0].enabled))),
            parameter::A_CC => Ok(ParameterValue::Float(f32::from(settings.lanes[0].cc))),
            parameter::A_VALUE => Ok(ParameterValue::Float(f32::from(settings.lanes[0].value))),
            parameter::A_MIN => Ok(ParameterValue::Float(f32::from(settings.lanes[0].min))),
            parameter::A_MAX => Ok(ParameterValue::Float(f32::from(settings.lanes[0].max))),
            parameter::B_ENABLE => Ok(ParameterValue::Choice(u32::from(settings.lanes[1].enabled))),
            parameter::B_CC => Ok(ParameterValue::Float(f32::from(settings.lanes[1].cc))),
            parameter::B_VALUE => Ok(ParameterValue::Float(f32::from(settings.lanes[1].value))),
            parameter::B_MIN => Ok(ParameterValue::Float(f32::from(settings.lanes[1].min))),
            parameter::B_MAX => Ok(ParameterValue::Float(f32::from(settings.lanes[1].max))),
            _ => Err(DeviceContractError::UnknownParameter),
        }
    }

    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut settings = CcControlSettings::decode(state)?;
        match parameter {
            parameter::CHANNEL => {
                settings.channel = parse_u8_param(value, 1, 16)? - 1;
            }
            parameter::CONFLICT => {
                let ParameterValue::Choice(choice) = value else {
                    return Err(DeviceContractError::InvalidParameterValue);
                };
                settings.conflict = CcConflictPolicy::from_choice(choice)
                    .ok_or(DeviceContractError::InvalidParameterValue)?;
            }
            parameter::SLEW => settings.slew = parse_u8_param(value, 0, MAX_MIDI_VALUE)?,
            parameter::A_ENABLE => settings.lanes[0].enabled = parse_enable_param(value)?,
            parameter::A_CC => settings.lanes[0].cc = parse_u8_param(value, 0, MAX_MIDI_VALUE)?,
            parameter::A_VALUE => {
                settings.lanes[0].value = parse_u8_param(value, 0, MAX_MIDI_VALUE)?;
            }
            parameter::A_MIN => settings.lanes[0].min = parse_u8_param(value, 0, MAX_MIDI_VALUE)?,
            parameter::A_MAX => settings.lanes[0].max = parse_u8_param(value, 0, MAX_MIDI_VALUE)?,
            parameter::B_ENABLE => settings.lanes[1].enabled = parse_enable_param(value)?,
            parameter::B_CC => settings.lanes[1].cc = parse_u8_param(value, 0, MAX_MIDI_VALUE)?,
            parameter::B_VALUE => {
                settings.lanes[1].value = parse_u8_param(value, 0, MAX_MIDI_VALUE)?;
            }
            parameter::B_MIN => settings.lanes[1].min = parse_u8_param(value, 0, MAX_MIDI_VALUE)?,
            parameter::B_MAX => settings.lanes[1].max = parse_u8_param(value, 0, MAX_MIDI_VALUE)?,
            _ => return Err(DeviceContractError::UnknownParameter),
        }
        settings.encode()?.with_host_metadata_from(state)
    }
}

impl NativeMidiProcessorDefinition for CcControlMidiProcessorDefinition {
    fn prepare_midi_processor(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Box<dyn PreparedNativeMidiProcessor>, DeviceContractError> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let settings = CcControlSettings::decode(state)?;
        Ok(Box::new(PreparedCcControlMidiProcessor::new(settings)?))
    }
}

/// Control-prepared immutable factory for `hex.midi.cc_control`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedCcControlMidiProcessor {
    settings: CcControlSettings,
    max_input_events: usize,
    max_output_events: usize,
    max_active_occurrences: usize,
    max_work_units: usize,
}

impl PreparedCcControlMidiProcessor {
    pub fn new(settings: CcControlSettings) -> Result<Self, DeviceContractError> {
        Self::with_caps(
            settings,
            CC_CONTROL_MAX_INPUT_EVENTS,
            CC_CONTROL_MAX_OUTPUT_EVENTS,
            CC_CONTROL_MAX_ACTIVE_OCCURRENCES,
            CC_CONTROL_MAX_WORK_UNITS,
        )
    }

    pub fn with_caps(
        settings: CcControlSettings,
        max_input_events: usize,
        max_output_events: usize,
        max_active_occurrences: usize,
        max_work_units: usize,
    ) -> Result<Self, DeviceContractError> {
        settings.validate()?;
        if max_input_events == 0
            || max_output_events == 0
            || max_active_occurrences == 0
            || max_active_occurrences > CC_CONTROL_MAX_ACTIVE_OCCURRENCES
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

    pub const fn settings(self) -> CcControlSettings {
        self.settings
    }
}

impl PreparedNativeMidiProcessor for PreparedCcControlMidiProcessor {
    fn runtime_version(&self) -> u32 {
        CC_CONTROL_RUNTIME_VERSION
    }

    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<CcControlMidiRuntime>()
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
        Box::new(CcControlMidiRuntime {
            config: *self,
            last_emitted: [None; CC_SLOT_COUNT],
            active: [None; CC_CONTROL_MAX_ACTIVE_OCCURRENCES],
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EmittedLaneState {
    cc: u8,
    value: u8,
}

#[derive(Clone)]
struct CcControlMidiRuntime {
    config: PreparedCcControlMidiProcessor,
    last_emitted: [Option<EmittedLaneState>; CC_SLOT_COUNT],
    active: [Option<u64>; CC_CONTROL_MAX_ACTIVE_OCCURRENCES],
}

impl CcControlMidiRuntime {
    fn find_slot(slots: &[Option<u64>], occurrence_id: u64) -> Option<usize> {
        slots.iter().position(|slot| *slot == Some(occurrence_id))
    }

    fn find_free_slot(slots: &[Option<u64>]) -> Option<usize> {
        slots.iter().position(Option::is_none)
    }

    fn count_active(slots: &[Option<u64>]) -> usize {
        slots.iter().flatten().count()
    }

    /// Compute the next CC value to emit for `lane_index`, if any, applying the
    /// explicit per-block `slew` step ceiling.
    fn next_lane_emission(&self, lane_index: usize) -> Option<(u8, u8)> {
        if self.config.settings.conflict == CcConflictPolicy::Block {
            return None;
        }
        let lane = self.config.settings.lanes[lane_index];
        if !lane.enabled {
            return None;
        }
        let target = lane.target_value();
        let slew = self.config.settings.slew;
        match self.last_emitted[lane_index] {
            Some(prev) if prev.cc == lane.cc && prev.value == target => None,
            Some(prev) if prev.cc == lane.cc && slew > 0 => {
                let next = if target > prev.value {
                    prev.value.saturating_add(slew).min(target)
                } else {
                    prev.value.saturating_sub(slew).max(target)
                };
                Some((lane.cc, next))
            }
            _ => Some((lane.cc, target)),
        }
    }
}

impl MutableMidiProcessorRuntime for CcControlMidiRuntime {
    fn preflight_block(
        &self,
        input: &[NativeInstrumentEvent],
        frame_count: usize,
        output_capacity: usize,
        _clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<usize, NativeMidiProcessorError> {
        if frame_count == 0 {
            return Err(NativeMidiProcessorError::InvalidInputEventBatch);
        }
        if input.len() > self.config.max_input_events {
            return Err(NativeMidiProcessorError::InputEventCapacityExceeded);
        }
        let mut simulated = self.active;
        let active_cap = self.config.max_active_occurrences;
        let mut output_count = 0_usize;
        let mut work_units = 0_usize;

        for lane_index in 0..CC_SLOT_COUNT {
            work_units = work_units
                .checked_add(1)
                .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
            if self.next_lane_emission(lane_index).is_some() {
                output_count = output_count
                    .checked_add(1)
                    .ok_or(NativeMidiProcessorError::OutputEventCapacityExceeded)?;
                work_units = work_units
                    .checked_add(1)
                    .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
            }
        }

        for &event in input {
            work_units = work_units
                .checked_add(1)
                .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
            let emit = match event {
                NativeInstrumentEvent::NoteOn { occurrence_id, .. } => {
                    if Self::find_slot(&simulated[..active_cap], occurrence_id).is_some() {
                        return Err(NativeMidiProcessorError::DuplicateOccurrence);
                    }
                    let free = Self::find_free_slot(&simulated[..active_cap])
                        .ok_or(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded)?;
                    simulated[free] = Some(occurrence_id);
                    true
                }
                NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    let slot = Self::find_slot(&simulated[..active_cap], occurrence_id)
                        .ok_or(NativeMidiProcessorError::OrphanedNoteOff)?;
                    simulated[slot] = None;
                    true
                }
                NativeInstrumentEvent::NotePressure { occurrence_id, .. }
                | NativeInstrumentEvent::NotePitchBend { occurrence_id, .. }
                | NativeInstrumentEvent::NoteTimbre { occurrence_id, .. } => {
                    if Self::find_slot(&simulated[..active_cap], occurrence_id.get()).is_none() {
                        return Err(NativeMidiProcessorError::InvalidInputEventBatch);
                    }
                    true
                }
                NativeInstrumentEvent::ControlChange { cc, value, .. } => {
                    if cc > MAX_MIDI_VALUE || value > MAX_MIDI_VALUE {
                        return Err(NativeMidiProcessorError::InvalidEventPayload);
                    }
                    !self.config.settings.suppresses_incoming_cc(cc)
                }
                NativeInstrumentEvent::Expression { .. }
                | NativeInstrumentEvent::PitchBend { .. } => true,
            };
            if emit {
                output_count = output_count
                    .checked_add(1)
                    .ok_or(NativeMidiProcessorError::OutputEventCapacityExceeded)?;
                work_units = work_units
                    .checked_add(1)
                    .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
            }
            if output_count > output_capacity || output_count > self.config.max_output_events {
                return Err(NativeMidiProcessorError::OutputEventCapacityExceeded);
            }
            if work_units > self.config.max_work_units {
                return Err(NativeMidiProcessorError::WorkBudgetExceeded);
            }
        }
        if output_count > output_capacity || output_count > self.config.max_output_events {
            return Err(NativeMidiProcessorError::OutputEventCapacityExceeded);
        }
        if work_units > self.config.max_work_units {
            return Err(NativeMidiProcessorError::WorkBudgetExceeded);
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
        // Emit any lane CC updates at frame_offset 0 ahead of same-offset NoteOn events.
        for lane_index in 0..CC_SLOT_COUNT {
            let _ = output.charge_work(1);
            if let Some((cc, value)) = self.next_lane_emission(lane_index) {
                self.last_emitted[lane_index] = Some(EmittedLaneState { cc, value });
                let _ = output.charge_work(1);
                let _ = output.push(NativeInstrumentEvent::ControlChange {
                    frame_offset: 0,
                    cc,
                    value,
                });
            }
        }

        let active_cap = self.config.max_active_occurrences;
        for &event in input {
            let _ = output.charge_work(1);
            let emit = match event {
                NativeInstrumentEvent::NoteOn { occurrence_id, .. } => {
                    if let Some(free) = Self::find_free_slot(&self.active[..active_cap]) {
                        self.active[free] = Some(occurrence_id);
                    }
                    true
                }
                NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    if let Some(slot) = Self::find_slot(&self.active[..active_cap], occurrence_id) {
                        self.active[slot] = None;
                    }
                    true
                }
                NativeInstrumentEvent::ControlChange { cc, .. } => {
                    !self.config.settings.suppresses_incoming_cc(cc)
                }
                _ => true,
            };
            if emit {
                let _ = output.charge_work(1);
                let _ = output.push(event);
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
        let active = Self::count_active(&self.active[..self.config.max_active_occurrences]);
        if active > output_capacity || active > self.config.max_output_events {
            return Err(NativeMidiProcessorError::OutputEventCapacityExceeded);
        }
        if active > self.config.max_work_units {
            return Err(NativeMidiProcessorError::WorkBudgetExceeded);
        }
        Ok(active)
    }

    fn flush_active_note_offs(
        &mut self,
        frame_offset: usize,
        output: &mut MidiProcessorOutput<'_>,
    ) {
        let active_cap = self.config.max_active_occurrences;
        let mut ids = [0_u64; CC_CONTROL_MAX_ACTIVE_OCCURRENCES];
        let mut len = 0_usize;
        for slot in &mut self.active[..active_cap] {
            if let Some(occurrence_id) = slot.take() {
                ids[len] = occurrence_id;
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
        self.last_emitted = [None; CC_SLOT_COUNT];
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
        for (write, &occurrence_id) in other.active[..other.config.max_active_occurrences]
            .iter()
            .flatten()
            .enumerate()
        {
            if write >= active_cap {
                break;
            }
            self.active[write] = Some(occurrence_id);
        }
        self.last_emitted = other.last_emitted;
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

    fn test_limits() -> NativeMidiProcessorLimits {
        NativeMidiProcessorLimits {
            max_pair_bytes: 256 * 1024,
            max_input_events_per_block: CC_CONTROL_MAX_INPUT_EVENTS,
            max_output_events_per_block: CC_CONTROL_MAX_OUTPUT_EVENTS,
            max_active_occurrences: CC_CONTROL_MAX_ACTIVE_OCCURRENCES,
            max_work_units_per_block: CC_CONTROL_MAX_WORK_UNITS,
        }
    }

    #[test]
    fn cc_control_state_codec_range_mapping_and_validation() {
        let default = CC_CONTROL_DEFINITION.default_state().unwrap();
        assert_eq!(
            CcControlSettings::decode(&default).unwrap(),
            CcControlSettings::default()
        );

        let patched = CC_CONTROL_DEFINITION
            .patch_parameter(&default, parameter::CHANNEL, ParameterValue::Float(4.0))
            .unwrap();
        let patched = CC_CONTROL_DEFINITION
            .patch_parameter(&patched, parameter::CONFLICT, ParameterValue::Choice(1))
            .unwrap();
        let patched = CC_CONTROL_DEFINITION
            .patch_parameter(&patched, parameter::SLEW, ParameterValue::Float(16.0))
            .unwrap();
        let patched = CC_CONTROL_DEFINITION
            .patch_parameter(&patched, parameter::A_CC, ParameterValue::Float(74.0))
            .unwrap();
        let patched = CC_CONTROL_DEFINITION
            .patch_parameter(&patched, parameter::A_VALUE, ParameterValue::Float(127.0))
            .unwrap();
        let patched = CC_CONTROL_DEFINITION
            .patch_parameter(&patched, parameter::A_MIN, ParameterValue::Float(100.0))
            .unwrap();
        let patched = CC_CONTROL_DEFINITION
            .patch_parameter(&patched, parameter::A_MAX, ParameterValue::Float(20.0))
            .unwrap();

        let decoded = CcControlSettings::decode(&patched).unwrap();
        assert_eq!(decoded.channel, 3);
        assert_eq!(decoded.conflict, CcConflictPolicy::Pass);
        assert_eq!(decoded.slew, 16);
        // Inverted range [100 -> 20]: value 127 maps to 20, value 0 maps to 100.
        assert_eq!(decoded.lanes[0].target_value(), 20);
        assert_eq!(
            CcLaneSettings {
                value: 0,
                ..decoded.lanes[0]
            }
            .target_value(),
            100
        );
        assert_eq!(
            CC_CONTROL_DEFINITION
                .read_parameter(&patched, parameter::CHANNEL)
                .unwrap(),
            ParameterValue::Float(4.0)
        );

        // Out-of-range / NaN / wrong-type / malformed state payloads refuse explicitly.
        assert!(
            CC_CONTROL_DEFINITION
                .patch_parameter(&default, parameter::CHANNEL, ParameterValue::Float(0.0))
                .is_err()
        );
        assert!(
            CC_CONTROL_DEFINITION
                .patch_parameter(&default, parameter::CHANNEL, ParameterValue::Float(17.0))
                .is_err()
        );
        assert!(
            CC_CONTROL_DEFINITION
                .patch_parameter(&default, parameter::A_CC, ParameterValue::Float(128.0))
                .is_err()
        );
        assert!(
            CC_CONTROL_DEFINITION
                .patch_parameter(&default, parameter::SLEW, ParameterValue::Float(f32::NAN))
                .is_err()
        );
        let malformed = NativeDeviceState::new(CC_CONTROL_TYPE_ID, 1, vec![0, 1]).unwrap();
        assert_eq!(
            CcControlSettings::decode(&malformed),
            Err(DeviceContractError::MalformedState)
        );
    }

    #[test]
    fn cc_control_slew_conflict_ordering_and_queue_saturation_ledger() {
        let mut settings = CcControlSettings {
            slew: 30,
            conflict: CcConflictPolicy::Override,
            lanes: [
                CcLaneSettings {
                    enabled: true,
                    cc: 74,
                    value: 0,
                    min: 0,
                    max: 127,
                },
                CcControlSettings::default().lanes[1],
            ],
            ..CcControlSettings::default()
        };
        let prep_0 = PreparedCcControlMidiProcessor::new(settings).unwrap();
        let mut pair_0 = NativeMidiProcessorRuntimePair::new(&prep_0, test_limits()).unwrap();

        let mut out_buf = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; 16];

        // Block 1: emits initial CC74=0 ahead of offset-0 NoteOn, and suppresses conflicting upstream CC74=99
        // while passing unrelated CC1=50 cleanly.
        let batch = [
            NativeInstrumentEvent::ControlChange {
                frame_offset: 0,
                cc: 74,
                value: 99,
            },
            NativeInstrumentEvent::ControlChange {
                frame_offset: 0,
                cc: 1,
                value: 50,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 60,
                velocity: 100,
            },
        ];
        pair_0.begin_block();
        let n1 = pair_0
            .process_block(&batch, 64, &mut out_buf, None)
            .unwrap();
        pair_0.commit_block();
        assert_eq!(
            &out_buf[..n1],
            &[
                NativeInstrumentEvent::ControlChange {
                    frame_offset: 0,
                    cc: 74,
                    value: 0,
                },
                NativeInstrumentEvent::ControlChange {
                    frame_offset: 0,
                    cc: 1,
                    value: 50,
                },
                NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: 1,
                    pitch: 60,
                    velocity: 100,
                },
            ]
        );

        // Block 2 (no state change): zero redundant CC emission!
        pair_0.begin_block();
        let n2 = pair_0.process_block(&[], 64, &mut out_buf, None).unwrap();
        pair_0.commit_block();
        assert_eq!(n2, 0);

        // Live edit target to 75 with slew=30 -> inherits held note & ramps 30 -> 60 -> 75 over 3 blocks!
        settings.lanes[0].value = 75;
        let prep_75 = PreparedCcControlMidiProcessor::new(settings).unwrap();
        let mut pair_75 = NativeMidiProcessorRuntimePair::new(&prep_75, test_limits()).unwrap();
        pair_75.inherit_from(&pair_0).unwrap();
        let mut emitted_vals = Vec::new();
        for _ in 0..4 {
            pair_75.begin_block();
            let n = pair_75.process_block(&[], 64, &mut out_buf, None).unwrap();
            pair_75.commit_block();
            for &ev in &out_buf[..n] {
                if let NativeInstrumentEvent::ControlChange { cc: 74, value, .. } = ev {
                    emitted_vals.push(value);
                }
            }
        }
        assert_eq!(emitted_vals, vec![30, 60, 75]);

        // Release held note cleanly -> zero stuck notes!
        pair_75.begin_block();
        let n_off = pair_75
            .process_block(
                &[NativeInstrumentEvent::NoteOff {
                    frame_offset: 0,
                    occurrence_id: 1,
                }],
                64,
                &mut out_buf,
                None,
            )
            .unwrap();
        pair_75.commit_block();
        assert_eq!(n_off, 1);
        assert_eq!(pair_75.committed_active_occurrences(), 0);

        // Queue saturation refuses atomically without corrupting last_emitted or active ledger.
        let tiny_prep = PreparedCcControlMidiProcessor::with_caps(settings, 4, 1, 2, 16).unwrap();
        let mut tiny_pair = NativeMidiProcessorRuntimePair::new(&tiny_prep, test_limits()).unwrap();
        let mut tiny_out = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; 1];
        tiny_pair.begin_block();
        assert_eq!(
            tiny_pair.process_block(
                &[NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: 1,
                    pitch: 60,
                    velocity: 100,
                }],
                64,
                &mut tiny_out,
                None,
            ),
            Err(NativeMidiProcessorError::OutputEventCapacityExceeded)
        );
        tiny_pair.rollback_block();
        assert_eq!(tiny_pair.committed_active_occurrences(), 0);
    }
}
