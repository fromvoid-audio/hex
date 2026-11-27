//! Original native MIDI Velocity processor using the bounded `hex-lattice`
//! MIDI processor contract (`hex.midi.velocity`).
//!
//! Applies an explicit, deterministic, finite mapping to incoming MIDI note-on
//! velocities (`1..=127`) with explicit output bounds (`out low`, `out high`),
//! input window (`lowest`, `range`), smooth monotonic `drive` curve (`-1.0..=1.0`),
//! and explicit mode (`clip`, `gate`, `fixed`).
//!
//! Preserves onset-to-off `occurrence_id` pairing and channel-wide controller
//! events without rewriting note-offs or introducing unbounded randomization.
//! In `gate` mode, notes outside `[lowest, lowest + range]` are suppressed and
//! tracked by `occurrence_id` so matching note-offs and per-note expression
//! events are also suppressed cleanly with zero stuck notes.

use hex_lattice::{
    MidiProcessorOutput, MutableMidiProcessorRuntime, NativeInstrumentClock, NativeInstrumentEvent,
    NativeMidiProcessorDefinition, NativeMidiProcessorError, PreparedNativeMidiProcessor,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;

pub const VELOCITY_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.midi.velocity");
pub const VELOCITY_STATE_VERSION: u32 = 1;
pub const VELOCITY_RUNTIME_VERSION: u32 = 1;

pub const MIN_MIDI_VELOCITY: u8 = 1;
pub const MAX_MIDI_VELOCITY: u8 = 127;

pub const VELOCITY_MAX_INPUT_EVENTS: usize = 259;
pub const VELOCITY_MAX_OUTPUT_EVENTS: usize = 259;
pub const VELOCITY_MAX_ACTIVE_OCCURRENCES: usize = 64;
pub const VELOCITY_MAX_WORK_UNITS: usize = VELOCITY_MAX_INPUT_EVENTS * 2;

/// `[mode, out_low, out_high, lowest, range, drive_i8 as u8]`
/// where `drive_i8` stores `-100..=+100` representing `-1.0..=+1.0`.
const STATE_BYTES: usize = 6;
const MAX_DRIVE_STEPS: i8 = 100;

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const MODE: ParameterId = ParameterId::from_static(1);
    pub const OUT_LOW: ParameterId = ParameterId::from_static(2);
    pub const OUT_HIGH: ParameterId = ParameterId::from_static(3);
    pub const LOWEST: ParameterId = ParameterId::from_static(4);
    pub const RANGE: ParameterId = ParameterId::from_static(5);
    pub const DRIVE: ParameterId = ParameterId::from_static(6);
}

const MODE_LABELS: [&str; 3] = ["clip", "gate", "fixed"];

const DESCRIPTORS: [ParameterDescriptor; 6] = [
    ParameterDescriptor {
        id: parameter::MODE,
        label: "mode",
        kind: ParameterKind::Choice {
            labels: &MODE_LABELS,
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::OUT_LOW,
        label: "out low",
        kind: ParameterKind::Float {
            min: MIN_MIDI_VELOCITY as f32,
            max: MAX_MIDI_VELOCITY as f32,
            unit: None,
        },
        default: ParameterValue::Float(MIN_MIDI_VELOCITY as f32),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::OUT_HIGH,
        label: "out high",
        kind: ParameterKind::Float {
            min: MIN_MIDI_VELOCITY as f32,
            max: MAX_MIDI_VELOCITY as f32,
            unit: None,
        },
        default: ParameterValue::Float(MAX_MIDI_VELOCITY as f32),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::LOWEST,
        label: "lowest",
        kind: ParameterKind::Float {
            min: MIN_MIDI_VELOCITY as f32,
            max: MAX_MIDI_VELOCITY as f32,
            unit: None,
        },
        default: ParameterValue::Float(MIN_MIDI_VELOCITY as f32),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::RANGE,
        label: "range",
        kind: ParameterKind::Float {
            min: 0.0,
            max: (MAX_MIDI_VELOCITY - MIN_MIDI_VELOCITY) as f32,
            unit: None,
        },
        default: ParameterValue::Float((MAX_MIDI_VELOCITY - MIN_MIDI_VELOCITY) as f32),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::DRIVE,
        label: "drive",
        kind: ParameterKind::Float {
            min: -1.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
];

/// Explicit velocity boundary handling mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum VelocityMode {
    /// Clamp out-of-window inputs to the input window edges before mapping.
    Clip = 0,
    /// Suppress notes whose input velocity falls outside `[lowest, lowest + range]`.
    Gate = 1,
    /// Emit all accepted notes at `out_high`.
    Fixed = 2,
}

impl VelocityMode {
    pub const fn from_choice(choice: u32) -> Option<Self> {
        match choice {
            0 => Some(Self::Clip),
            1 => Some(Self::Gate),
            2 => Some(Self::Fixed),
            _ => None,
        }
    }

    pub const fn as_choice(self) -> u32 {
        self as u32
    }
}

/// Validated authored settings for `hex.midi.velocity`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VelocitySettings {
    pub mode: VelocityMode,
    pub out_low: u8,
    pub out_high: u8,
    pub lowest: u8,
    pub range: u8,
    /// Integer drive in `-100..=+100` representing `-1.0..=+1.0`.
    pub drive_percent: i8,
}

impl Default for VelocitySettings {
    fn default() -> Self {
        Self {
            mode: VelocityMode::Clip,
            out_low: MIN_MIDI_VELOCITY,
            out_high: MAX_MIDI_VELOCITY,
            lowest: MIN_MIDI_VELOCITY,
            range: MAX_MIDI_VELOCITY - MIN_MIDI_VELOCITY,
            drive_percent: 0,
        }
    }
}

impl VelocitySettings {
    pub fn encode(self) -> Result<NativeDeviceState, DeviceContractError> {
        self.validate()?;
        NativeDeviceState::new(
            VELOCITY_TYPE_ID,
            VELOCITY_STATE_VERSION,
            vec![
                self.mode as u8,
                self.out_low,
                self.out_high,
                self.lowest,
                self.range,
                self.drive_percent as u8,
            ],
        )
    }

    pub fn decode(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != VELOCITY_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != VELOCITY_STATE_VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        if !state.asset_references().is_empty() {
            return Err(DeviceContractError::AssetResourcesUnavailable);
        }
        let payload = state.payload();
        if payload.len() != STATE_BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let mode = VelocityMode::from_choice(u32::from(payload[0]))
            .ok_or(DeviceContractError::MalformedState)?;
        let settings = Self {
            mode,
            out_low: payload[1],
            out_high: payload[2],
            lowest: payload[3],
            range: payload[4],
            drive_percent: payload[5] as i8,
        };
        settings.validate()?;
        Ok(settings)
    }

    fn validate(self) -> Result<(), DeviceContractError> {
        if !(MIN_MIDI_VELOCITY..=MAX_MIDI_VELOCITY).contains(&self.out_low)
            || !(MIN_MIDI_VELOCITY..=MAX_MIDI_VELOCITY).contains(&self.out_high)
            || !(MIN_MIDI_VELOCITY..=MAX_MIDI_VELOCITY).contains(&self.lowest)
            || self.range > (MAX_MIDI_VELOCITY - MIN_MIDI_VELOCITY)
            || !(-MAX_DRIVE_STEPS..=MAX_DRIVE_STEPS).contains(&self.drive_percent)
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(())
    }

    /// Map an incoming MIDI note-on velocity (`1..=127`). Returns `None` when
    /// `mode == VelocityMode::Gate` suppresses a note outside `[lowest, upper]`.
    pub fn map_velocity(self, velocity: u8) -> Option<u8> {
        if !(MIN_MIDI_VELOCITY..=MAX_MIDI_VELOCITY).contains(&velocity) {
            return None;
        }
        let upper = self
            .lowest
            .saturating_add(self.range)
            .min(MAX_MIDI_VELOCITY);
        if self.mode == VelocityMode::Gate && (velocity < self.lowest || velocity > upper) {
            return None;
        }
        if self.mode == VelocityMode::Fixed {
            return Some(self.out_high.clamp(MIN_MIDI_VELOCITY, MAX_MIDI_VELOCITY));
        }
        let clamped_in = velocity.clamp(self.lowest, upper);
        let span = upper.saturating_sub(self.lowest);
        let t = if span == 0 {
            1.0_f64
        } else {
            f64::from(clamped_in - self.lowest) / f64::from(span)
        };
        // Exact rational monotonic quadratic curve: t' = t + drive * t * (1 - t).
        // Identity when drive == 0; exact endpoints 0 -> 0 and 1 -> 1.
        let drive = f64::from(self.drive_percent) / 100.0;
        let shaped = (t + drive * t * (1.0 - t)).clamp(0.0, 1.0);
        let mapped =
            f64::from(self.out_low) + shaped * (f64::from(self.out_high) - f64::from(self.out_low));
        Some(
            mapped
                .round()
                .clamp(f64::from(MIN_MIDI_VELOCITY), f64::from(MAX_MIDI_VELOCITY))
                as u8,
        )
    }
}

pub static VELOCITY_DEFINITION: VelocityMidiProcessorDefinition = VelocityMidiProcessorDefinition;

pub struct VelocityMidiProcessorDefinition;

impl NativeDeviceMetadata for VelocityMidiProcessorDefinition {
    fn type_id(&self) -> DeviceTypeId {
        VELOCITY_TYPE_ID
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("velocity")
    }

    fn state_version(&self) -> u32 {
        VELOCITY_STATE_VERSION
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &DESCRIPTORS
    }

    fn ports(&self) -> &'static [hex_lattice_contract::DevicePortDescriptor] {
        hex_lattice_contract::MIDI_EFFECT_PORTS
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        VelocitySettings::default().encode()
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let settings = VelocitySettings::decode(state)?;
        settings.encode()?.with_host_metadata_from(state)
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let settings = VelocitySettings::decode(state)?;
        match parameter {
            parameter::MODE => Ok(ParameterValue::Choice(settings.mode.as_choice())),
            parameter::OUT_LOW => Ok(ParameterValue::Float(f32::from(settings.out_low))),
            parameter::OUT_HIGH => Ok(ParameterValue::Float(f32::from(settings.out_high))),
            parameter::LOWEST => Ok(ParameterValue::Float(f32::from(settings.lowest))),
            parameter::RANGE => Ok(ParameterValue::Float(f32::from(settings.range))),
            parameter::DRIVE => Ok(ParameterValue::Float(
                f32::from(settings.drive_percent) / 100.0,
            )),
            _ => Err(DeviceContractError::UnknownParameter),
        }
    }

    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut settings = VelocitySettings::decode(state)?;
        match (parameter, value) {
            (parameter::MODE, ParameterValue::Choice(choice)) => {
                settings.mode = VelocityMode::from_choice(choice)
                    .ok_or(DeviceContractError::InvalidParameterValue)?;
            }
            (parameter::OUT_LOW, ParameterValue::Float(v)) if v.is_finite() => {
                let r = v.round();
                if r < f32::from(MIN_MIDI_VELOCITY) || r > f32::from(MAX_MIDI_VELOCITY) {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.out_low = r as u8;
            }
            (parameter::OUT_HIGH, ParameterValue::Float(v)) if v.is_finite() => {
                let r = v.round();
                if r < f32::from(MIN_MIDI_VELOCITY) || r > f32::from(MAX_MIDI_VELOCITY) {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.out_high = r as u8;
            }
            (parameter::LOWEST, ParameterValue::Float(v)) if v.is_finite() => {
                let r = v.round();
                if r < f32::from(MIN_MIDI_VELOCITY) || r > f32::from(MAX_MIDI_VELOCITY) {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.lowest = r as u8;
            }
            (parameter::RANGE, ParameterValue::Float(v)) if v.is_finite() => {
                let r = v.round();
                if r < 0.0 || r > f32::from(MAX_MIDI_VELOCITY - MIN_MIDI_VELOCITY) {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.range = r as u8;
            }
            (parameter::DRIVE, ParameterValue::Float(v)) if v.is_finite() => {
                if !(-1.0..=1.0).contains(&v) {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.drive_percent = (v * 100.0).round().clamp(-100.0, 100.0) as i8;
            }
            (
                parameter::MODE
                | parameter::OUT_LOW
                | parameter::OUT_HIGH
                | parameter::LOWEST
                | parameter::RANGE
                | parameter::DRIVE,
                _,
            ) => return Err(DeviceContractError::InvalidParameterValue),
            _ => return Err(DeviceContractError::UnknownParameter),
        }
        settings.encode()?.with_host_metadata_from(state)
    }
}

impl NativeMidiProcessorDefinition for VelocityMidiProcessorDefinition {
    fn prepare_midi_processor(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Box<dyn PreparedNativeMidiProcessor>, DeviceContractError> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let settings = VelocitySettings::decode(state)?;
        Ok(Box::new(PreparedVelocityMidiProcessor::new(settings)?))
    }
}

/// Control-prepared immutable factory for `hex.midi.velocity`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedVelocityMidiProcessor {
    settings: VelocitySettings,
    max_input_events: usize,
    max_output_events: usize,
    max_active_occurrences: usize,
    max_work_units: usize,
}

impl PreparedVelocityMidiProcessor {
    pub fn new(settings: VelocitySettings) -> Result<Self, DeviceContractError> {
        Self::with_caps(
            settings,
            VELOCITY_MAX_INPUT_EVENTS,
            VELOCITY_MAX_OUTPUT_EVENTS,
            VELOCITY_MAX_ACTIVE_OCCURRENCES,
            VELOCITY_MAX_WORK_UNITS,
        )
    }

    pub fn with_caps(
        settings: VelocitySettings,
        max_input_events: usize,
        max_output_events: usize,
        max_active_occurrences: usize,
        max_work_units: usize,
    ) -> Result<Self, DeviceContractError> {
        settings.validate()?;
        if max_input_events == 0
            || max_output_events == 0
            || max_active_occurrences == 0
            || max_active_occurrences > VELOCITY_MAX_ACTIVE_OCCURRENCES
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

    pub const fn settings(self) -> VelocitySettings {
        self.settings
    }
}

impl PreparedNativeMidiProcessor for PreparedVelocityMidiProcessor {
    fn runtime_version(&self) -> u32 {
        VELOCITY_RUNTIME_VERSION
    }

    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<VelocityMidiRuntime>()
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
        Box::new(VelocityMidiRuntime {
            config: *self,
            active: [None; VELOCITY_MAX_ACTIVE_OCCURRENCES],
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TrackedVelocityOccurrence {
    occurrence_id: u64,
    emitted: bool,
}

#[derive(Clone)]
struct VelocityMidiRuntime {
    config: PreparedVelocityMidiProcessor,
    active: [Option<TrackedVelocityOccurrence>; VELOCITY_MAX_ACTIVE_OCCURRENCES],
}

impl VelocityMidiRuntime {
    fn find_slot(slots: &[Option<TrackedVelocityOccurrence>], occurrence_id: u64) -> Option<usize> {
        slots
            .iter()
            .position(|slot| slot.is_some_and(|entry| entry.occurrence_id == occurrence_id))
    }

    fn find_free_slot(slots: &[Option<TrackedVelocityOccurrence>]) -> Option<usize> {
        slots.iter().position(Option::is_none)
    }

    fn count_active(slots: &[Option<TrackedVelocityOccurrence>]) -> usize {
        slots.iter().flatten().count()
    }
}

impl MutableMidiProcessorRuntime for VelocityMidiRuntime {
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
                    velocity,
                    ..
                } => {
                    if Self::find_slot(&simulated[..active_cap], occurrence_id).is_some() {
                        return Err(NativeMidiProcessorError::DuplicateOccurrence);
                    }
                    let free = Self::find_free_slot(&simulated[..active_cap])
                        .ok_or(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded)?;
                    let emitted = self.config.settings.map_velocity(velocity).is_some();
                    simulated[free] = Some(TrackedVelocityOccurrence {
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
                    let mapped = self.config.settings.map_velocity(velocity);
                    if let Some(free) = Self::find_free_slot(&self.active[..active_cap]) {
                        self.active[free] = Some(TrackedVelocityOccurrence {
                            occurrence_id,
                            emitted: mapped.is_some(),
                        });
                    }
                    if let Some(new_velocity) = mapped {
                        let _ = output.charge_work(1);
                        let _ = output.push(NativeInstrumentEvent::NoteOn {
                            frame_offset,
                            occurrence_id,
                            pitch,
                            velocity: new_velocity,
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
        let mut ids = [0_u64; VELOCITY_MAX_ACTIVE_OCCURRENCES];
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
            max_input_events_per_block: VELOCITY_MAX_INPUT_EVENTS,
            max_output_events_per_block: VELOCITY_MAX_OUTPUT_EVENTS,
            max_active_occurrences: VELOCITY_MAX_ACTIVE_OCCURRENCES,
            max_work_units_per_block: VELOCITY_MAX_WORK_UNITS * 2,
        }
    }

    fn make_pair(settings: VelocitySettings) -> NativeMidiProcessorRuntimePair {
        let prepared = PreparedVelocityMidiProcessor::new(settings).unwrap();
        NativeMidiProcessorRuntimePair::new(&prepared, test_limits()).unwrap()
    }

    #[test]
    fn velocity_mapping_boundaries_modes_and_codec_validation() {
        let registry = crate::native_midi_processor_registry().unwrap();
        let default = registry.default_state(VELOCITY_TYPE_ID).unwrap();
        assert_eq!(
            VelocitySettings::decode(&default).unwrap(),
            VelocitySettings::default()
        );
        // Default mapping is exact identity across 1..=127.
        for v in 1..=127 {
            assert_eq!(VelocitySettings::default().map_velocity(v), Some(v));
        }

        // Clip mode compresses to [40..=100] and boosts midpoint with positive drive.
        let compressed = VelocitySettings {
            mode: VelocityMode::Clip,
            out_low: 40,
            out_high: 100,
            lowest: 20,
            range: 80, // [20..=100]
            drive_percent: 50,
        };
        assert_eq!(compressed.map_velocity(1), Some(40));
        assert_eq!(compressed.map_velocity(20), Some(40));
        assert_eq!(compressed.map_velocity(100), Some(100));
        assert_eq!(compressed.map_velocity(127), Some(100));
        // Midpoint v=60 -> t=0.5 -> shaped = 0.5 + 0.5*0.25 = 0.625 -> 40 + 37.5 = 78.
        assert_eq!(compressed.map_velocity(60), Some(78));

        // Inverted range (out_low > out_high) works monotonically without panics.
        let inverted = VelocitySettings {
            out_low: 127,
            out_high: 1,
            ..VelocitySettings::default()
        };
        assert_eq!(inverted.map_velocity(1), Some(127));
        assert_eq!(inverted.map_velocity(127), Some(1));

        // Gate mode suppresses out-of-window notes.
        let gated = VelocitySettings {
            mode: VelocityMode::Gate,
            lowest: 50,
            range: 30, // [50..=80]
            ..VelocitySettings::default()
        };
        assert_eq!(gated.map_velocity(49), None);
        assert_eq!(gated.map_velocity(50), Some(1));
        assert_eq!(gated.map_velocity(80), Some(127));
        assert_eq!(gated.map_velocity(81), None);

        // Fixed mode emits out_high for every valid velocity.
        let fixed = VelocitySettings {
            mode: VelocityMode::Fixed,
            out_high: 96,
            ..VelocitySettings::default()
        };
        assert_eq!(fixed.map_velocity(1), Some(96));
        assert_eq!(fixed.map_velocity(127), Some(96));

        // Malformed states refuse deterministically.
        let bad_out_low =
            NativeDeviceState::new(VELOCITY_TYPE_ID, 1, vec![0, 0, 127, 1, 126, 0]).unwrap();
        assert_eq!(
            VelocitySettings::decode(&bad_out_low),
            Err(DeviceContractError::InvalidParameterValue)
        );
        let bad_mode =
            NativeDeviceState::new(VELOCITY_TYPE_ID, 1, vec![9, 1, 127, 1, 126, 0]).unwrap();
        assert_eq!(
            VelocitySettings::decode(&bad_mode),
            Err(DeviceContractError::MalformedState)
        );
    }

    #[test]
    fn gate_suppression_same_note_overlaps_live_edits_and_saturation_preserve_ledger() {
        let mut pair = make_pair(VelocitySettings {
            mode: VelocityMode::Gate,
            lowest: 60,
            range: 67,
            ..VelocitySettings::default()
        });
        let mut out = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; 16];
        let mut ledger: BTreeMap<u64, u8> = BTreeMap::new();

        // Two overlapping C4 notes: occ 1 velocity 40 (gated), occ 2 velocity 100 (emitted), plus CC passthrough.
        let b1 = [
            NativeInstrumentEvent::ControlChange {
                frame_offset: 0,
                cc: 1,
                value: 64,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 60,
                velocity: 40,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 4,
                occurrence_id: 2,
                pitch: 60,
                velocity: 100,
            },
        ];
        pair.begin_block();
        let n1 = pair.process_block(&b1, 64, &mut out, None).unwrap();
        pair.commit_block();
        assert_eq!(n1, 2); // CC + occ 2 NoteOn (occ 1 gated).
        for &e in &out[..n1] {
            if let NativeInstrumentEvent::NoteOn {
                occurrence_id,
                velocity,
                ..
            } = e
            {
                assert!(ledger.insert(occurrence_id, velocity).is_none());
            }
        }
        assert_eq!(ledger.len(), 1);
        assert!(ledger.contains_key(&2));

        // Live edit to Fixed(110) while occ 1 (gated) and occ 2 (emitted) are held!
        let mut pair_fixed = make_pair(VelocitySettings {
            mode: VelocityMode::Fixed,
            out_high: 110,
            ..VelocitySettings::default()
        });
        assert!(pair_fixed.can_inherit_from(&pair));
        pair_fixed.inherit_from(&pair).unwrap();

        let b2 = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 3,
                pitch: 60,
                velocity: 20, // now emitted at Fixed 110
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 2,
                occurrence_id: 1, // stays suppressed cleanly!
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 4,
                occurrence_id: 2,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 6,
                occurrence_id: 3,
            },
        ];
        pair_fixed.begin_block();
        let n2 = pair_fixed.process_block(&b2, 64, &mut out, None).unwrap();
        pair_fixed.commit_block();
        assert_eq!(n2, 3); // occ 3 NoteOn(110) + occ 2 NoteOff + occ 3 NoteOff.
        for &e in &out[..n2] {
            match e {
                NativeInstrumentEvent::NoteOn {
                    occurrence_id,
                    velocity,
                    ..
                } => {
                    assert_eq!(velocity, 110);
                    assert!(ledger.insert(occurrence_id, velocity).is_none());
                }
                NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    assert!(ledger.remove(&occurrence_id).is_some());
                }
                _ => {}
            }
        }
        assert!(ledger.is_empty());
        assert!(pair_fixed.committed_is_quiescent());
    }
}
