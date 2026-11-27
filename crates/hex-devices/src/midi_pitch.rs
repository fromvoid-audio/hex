//! Original native MIDI Pitch processor using the bounded `hex-lattice`
//! MIDI processor contract (`hex.midi.pitch`).
//!
//! Transposes incoming MIDI note-on pitches by an authored semitone offset
//! within `-48..=+48`, optionally gating notes outside `[lowest, lowest + range]`,
//! and resolving transposed pitches outside `0..=127` according to an explicit
//! overflow policy (`clamp`, `drop`, or octave `fold`).
//!
//! Every held note is tracked by its canonical `occurrence_id`, never by pitch.
//! Same-pitch overlapping notes keep independent lifetimes, and changing pitch,
//! range, or overflow while notes are held transfers active occurrence tracking
//! seamlessly so already-sounding notes release cleanly while new notes adopt
//! the updated settings.

use hex_lattice::{
    MidiProcessorOutput, MutableMidiProcessorRuntime, NativeInstrumentClock, NativeInstrumentEvent,
    NativeMidiProcessorDefinition, NativeMidiProcessorError, PreparedNativeMidiProcessor,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;

pub const PITCH_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.midi.pitch");
pub const PITCH_STATE_VERSION: u32 = 1;
pub const PITCH_RUNTIME_VERSION: u32 = 1;

pub const MIN_SEMITONES: i8 = -48;
pub const MAX_SEMITONES: i8 = 48;
pub const MAX_MIDI_NOTE: u8 = 127;

pub const PITCH_MAX_INPUT_EVENTS: usize = 259;
pub const PITCH_MAX_OUTPUT_EVENTS: usize = 259;
pub const PITCH_MAX_ACTIVE_OCCURRENCES: usize = 64;
pub const PITCH_MAX_WORK_UNITS: usize = PITCH_MAX_INPUT_EVENTS * 2;

const STATE_BYTES: usize = 4;

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const PITCH: ParameterId = ParameterId::from_static(1);
    pub const LOWEST: ParameterId = ParameterId::from_static(2);
    pub const RANGE: ParameterId = ParameterId::from_static(3);
    pub const OVERFLOW: ParameterId = ParameterId::from_static(4);
}

const OVERFLOW_LABELS: [&str; 3] = ["clamp", "drop", "fold"];

const DESCRIPTORS: [ParameterDescriptor; 4] = [
    ParameterDescriptor {
        id: parameter::PITCH,
        label: "pitch",
        kind: ParameterKind::Float {
            min: MIN_SEMITONES as f32,
            max: MAX_SEMITONES as f32,
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
];

/// Explicit policy for transposed pitches falling outside MIDI `0..=127`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PitchOverflowPolicy {
    /// Clamp transposed pitches into `0..=127`.
    Clamp = 0,
    /// Suppress out-of-range notes cleanly while tracking their occurrence ID
    /// so matching note-off and per-note expression events are also suppressed.
    Drop = 1,
    /// Fold out-of-range pitches by octaves (12 semitones) back into `0..=127`.
    Fold = 2,
}

impl PitchOverflowPolicy {
    pub const fn from_enum(index: u32) -> Option<Self> {
        match index {
            0 => Some(Self::Clamp),
            1 => Some(Self::Drop),
            2 => Some(Self::Fold),
            _ => None,
        }
    }

    pub const fn as_enum(self) -> u32 {
        self as u32
    }
}

/// Validated authored settings for the native MIDI Pitch processor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PitchSettings {
    pub semitones: i8,
    pub lowest: u8,
    pub range: u8,
    pub overflow: PitchOverflowPolicy,
}

impl Default for PitchSettings {
    fn default() -> Self {
        Self {
            semitones: 0,
            lowest: 0,
            range: MAX_MIDI_NOTE,
            overflow: PitchOverflowPolicy::Clamp,
        }
    }
}

impl PitchSettings {
    pub fn encode(self) -> Result<NativeDeviceState, DeviceContractError> {
        self.validate()?;
        NativeDeviceState::new(
            PITCH_TYPE_ID,
            PITCH_STATE_VERSION,
            vec![
                self.semitones as u8,
                self.lowest,
                self.range,
                self.overflow as u8,
            ],
        )
    }

    pub fn decode(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != PITCH_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != PITCH_STATE_VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        if !state.asset_references().is_empty() {
            return Err(DeviceContractError::AssetResourcesUnavailable);
        }
        let payload = state.payload();
        if payload.len() != STATE_BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let semitones = payload[0] as i8;
        let lowest = payload[1];
        let range = payload[2];
        let overflow = PitchOverflowPolicy::from_enum(u32::from(payload[3]))
            .ok_or(DeviceContractError::MalformedState)?;
        let settings = Self {
            semitones,
            lowest,
            range,
            overflow,
        };
        settings.validate()?;
        Ok(settings)
    }

    fn validate(self) -> Result<(), DeviceContractError> {
        if !(MIN_SEMITONES..=MAX_SEMITONES).contains(&self.semitones)
            || self.lowest > MAX_MIDI_NOTE
            || self.range > MAX_MIDI_NOTE
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(())
    }

    /// Map an incoming MIDI note-on pitch (`0..=127`) according to the input
    /// note window and overflow policy. Returns `None` when the note is gated
    /// out by `[lowest, lowest + range]` or suppressed by `PitchOverflowPolicy::Drop`.
    pub const fn map_pitch(self, pitch: u8) -> Option<u8> {
        if pitch > MAX_MIDI_NOTE {
            return None;
        }
        let upper = (self.lowest as u16) + (self.range as u16);
        if (pitch as u16) < (self.lowest as u16) || (pitch as u16) > upper {
            return None;
        }
        let shifted = (pitch as i16) + (self.semitones as i16);
        if shifted >= 0 && shifted <= MAX_MIDI_NOTE as i16 {
            return Some(shifted as u8);
        }
        match self.overflow {
            PitchOverflowPolicy::Clamp => {
                if shifted < 0 {
                    Some(0)
                } else {
                    Some(MAX_MIDI_NOTE)
                }
            }
            PitchOverflowPolicy::Drop => None,
            PitchOverflowPolicy::Fold => {
                if shifted < 0 {
                    Some(shifted.rem_euclid(12) as u8)
                } else {
                    // Fold high overflow by octaves into [116..=127], preserving pitch class.
                    Some((127 - (127 - shifted).rem_euclid(12)) as u8)
                }
            }
        }
    }
}

pub static PITCH_DEFINITION: PitchMidiProcessorDefinition = PitchMidiProcessorDefinition;

pub struct PitchMidiProcessorDefinition;

impl NativeDeviceMetadata for PitchMidiProcessorDefinition {
    fn type_id(&self) -> DeviceTypeId {
        PITCH_TYPE_ID
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("pitch")
    }

    fn state_version(&self) -> u32 {
        PITCH_STATE_VERSION
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &DESCRIPTORS
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        PitchSettings::default().encode()
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let settings = PitchSettings::decode(state)?;
        settings.encode()?.with_host_metadata_from(state)
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let settings = PitchSettings::decode(state)?;
        match parameter {
            parameter::PITCH => Ok(ParameterValue::Float(f32::from(settings.semitones))),
            parameter::LOWEST => Ok(ParameterValue::Float(f32::from(settings.lowest))),
            parameter::RANGE => Ok(ParameterValue::Float(f32::from(settings.range))),
            parameter::OVERFLOW => Ok(ParameterValue::Choice(settings.overflow.as_enum())),
            _ => Err(DeviceContractError::UnknownParameter),
        }
    }

    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut settings = PitchSettings::decode(state)?;
        match (parameter, value) {
            (parameter::PITCH, ParameterValue::Float(v)) if v.is_finite() => {
                let rounded = v.round();
                if rounded < f32::from(MIN_SEMITONES) || rounded > f32::from(MAX_SEMITONES) {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.semitones = rounded as i8;
            }
            (parameter::LOWEST, ParameterValue::Float(v)) if v.is_finite() => {
                let rounded = v.round();
                if rounded < 0.0 || rounded > f32::from(MAX_MIDI_NOTE) {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.lowest = rounded as u8;
            }
            (parameter::RANGE, ParameterValue::Float(v)) if v.is_finite() => {
                let rounded = v.round();
                if rounded < 0.0 || rounded > f32::from(MAX_MIDI_NOTE) {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.range = rounded as u8;
            }
            (parameter::PITCH | parameter::LOWEST | parameter::RANGE, ParameterValue::Float(_)) => {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            (parameter::OVERFLOW, ParameterValue::Choice(v)) => {
                settings.overflow = PitchOverflowPolicy::from_enum(v)
                    .ok_or(DeviceContractError::InvalidParameterValue)?;
            }
            (parameter::PITCH | parameter::LOWEST | parameter::RANGE | parameter::OVERFLOW, _) => {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            _ => return Err(DeviceContractError::UnknownParameter),
        }
        settings.encode()?.with_host_metadata_from(state)
    }
}

impl NativeMidiProcessorDefinition for PitchMidiProcessorDefinition {
    fn prepare_midi_processor(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Box<dyn PreparedNativeMidiProcessor>, DeviceContractError> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let settings = PitchSettings::decode(state)?;
        Ok(Box::new(PreparedPitchMidiProcessor::new(settings)?))
    }
}

/// Control-prepared immutable factory for `hex.midi.pitch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedPitchMidiProcessor {
    settings: PitchSettings,
    max_input_events: usize,
    max_output_events: usize,
    max_active_occurrences: usize,
    max_work_units: usize,
}

impl PreparedPitchMidiProcessor {
    pub fn new(settings: PitchSettings) -> Result<Self, DeviceContractError> {
        Self::with_caps(
            settings,
            PITCH_MAX_INPUT_EVENTS,
            PITCH_MAX_OUTPUT_EVENTS,
            PITCH_MAX_ACTIVE_OCCURRENCES,
            PITCH_MAX_WORK_UNITS,
        )
    }

    pub fn with_caps(
        settings: PitchSettings,
        max_input_events: usize,
        max_output_events: usize,
        max_active_occurrences: usize,
        max_work_units: usize,
    ) -> Result<Self, DeviceContractError> {
        settings.validate()?;
        if max_input_events == 0
            || max_output_events == 0
            || max_active_occurrences == 0
            || max_active_occurrences > PITCH_MAX_ACTIVE_OCCURRENCES
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

    pub const fn settings(self) -> PitchSettings {
        self.settings
    }
}

impl PreparedNativeMidiProcessor for PreparedPitchMidiProcessor {
    fn runtime_version(&self) -> u32 {
        PITCH_RUNTIME_VERSION
    }

    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<PitchMidiRuntime>()
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
        Box::new(PitchMidiRuntime {
            config: *self,
            active: [None; PITCH_MAX_ACTIVE_OCCURRENCES],
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TrackedPitchOccurrence {
    occurrence_id: u64,
    /// `true` if a `NoteOn` was emitted downstream; `false` if gated/dropped.
    emitted: bool,
}

#[derive(Clone)]
struct PitchMidiRuntime {
    config: PreparedPitchMidiProcessor,
    active: [Option<TrackedPitchOccurrence>; PITCH_MAX_ACTIVE_OCCURRENCES],
}

impl PitchMidiRuntime {
    fn find_slot(slots: &[Option<TrackedPitchOccurrence>], occurrence_id: u64) -> Option<usize> {
        slots
            .iter()
            .position(|slot| slot.is_some_and(|entry| entry.occurrence_id == occurrence_id))
    }

    fn find_free_slot(slots: &[Option<TrackedPitchOccurrence>]) -> Option<usize> {
        slots.iter().position(Option::is_none)
    }

    fn count_active(slots: &[Option<TrackedPitchOccurrence>]) -> usize {
        slots.iter().flatten().count()
    }
}

impl MutableMidiProcessorRuntime for PitchMidiRuntime {
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
                    simulated[free] = Some(TrackedPitchOccurrence {
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
                        self.active[free] = Some(TrackedPitchOccurrence {
                            occurrence_id,
                            emitted: mapped.is_some(),
                        });
                    }
                    if let Some(transposed) = mapped {
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
        let mut ids = [0_u64; PITCH_MAX_ACTIVE_OCCURRENCES];
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

    /// Because downstream note releases are keyed by `occurrence_id` rather
    /// than pitch, a newly prepared `PitchMidiRuntime` with updated semitone,
    /// range, or overflow settings can inherit all active held occurrences
    /// seamlessly as long as they fit within its active capacity.
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
    use std::num::NonZeroU64;

    fn test_limits() -> NativeMidiProcessorLimits {
        NativeMidiProcessorLimits {
            max_pair_bytes: 256 * 1024,
            max_input_events_per_block: PITCH_MAX_INPUT_EVENTS,
            max_output_events_per_block: PITCH_MAX_OUTPUT_EVENTS,
            max_active_occurrences: PITCH_MAX_ACTIVE_OCCURRENCES,
            max_work_units_per_block: PITCH_MAX_WORK_UNITS * 2,
        }
    }

    fn make_pair(settings: PitchSettings) -> NativeMidiProcessorRuntimePair {
        let prepared = PreparedPitchMidiProcessor::new(settings).unwrap();
        NativeMidiProcessorRuntimePair::new(&prepared, test_limits()).unwrap()
    }

    /// Independent downstream note-on / note-off ledger that fails immediately
    /// on duplicate note-on, orphaned note-off, or unreleased stuck notes.
    #[derive(Default)]
    struct DownstreamLedger {
        active: BTreeMap<u64, u8>,
    }

    impl DownstreamLedger {
        fn observe(&mut self, events: &[NativeInstrumentEvent]) {
            for &event in events {
                match event {
                    NativeInstrumentEvent::NoteOn {
                        occurrence_id,
                        pitch,
                        ..
                    } => {
                        assert!(
                            self.active.insert(occurrence_id, pitch).is_none(),
                            "duplicate downstream note-on for occurrence {occurrence_id}"
                        );
                    }
                    NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                        assert!(
                            self.active.remove(&occurrence_id).is_some(),
                            "orphaned downstream note-off for occurrence {occurrence_id}"
                        );
                    }
                    NativeInstrumentEvent::NotePressure { occurrence_id, .. }
                    | NativeInstrumentEvent::NotePitchBend { occurrence_id, .. }
                    | NativeInstrumentEvent::NoteTimbre { occurrence_id, .. } => {
                        assert!(
                            self.active.contains_key(&occurrence_id.get()),
                            "per-note expression for inactive occurrence {}",
                            occurrence_id.get()
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn registry_metadata_state_codec_and_malformed_state_rejection() {
        let registry = crate::native_midi_processor_registry().unwrap();
        let default = registry.default_state(PITCH_TYPE_ID).unwrap();
        assert_eq!(
            PitchSettings::decode(&default).unwrap(),
            PitchSettings::default()
        );
        let definition = registry.definition(PITCH_TYPE_ID).unwrap();
        assert_eq!(
            definition
                .read_parameter(&default, parameter::PITCH)
                .unwrap(),
            ParameterValue::Float(0.0)
        );
        assert_eq!(
            definition
                .read_parameter(&default, parameter::OVERFLOW)
                .unwrap(),
            ParameterValue::Choice(0)
        );

        let patched = registry
            .patch_parameter(&default, parameter::PITCH, ParameterValue::Float(-24.0))
            .unwrap();
        let patched = registry
            .patch_parameter(&patched, parameter::LOWEST, ParameterValue::Float(36.0))
            .unwrap();
        let patched = registry
            .patch_parameter(&patched, parameter::RANGE, ParameterValue::Float(48.0))
            .unwrap();
        let patched = registry
            .patch_parameter(&patched, parameter::OVERFLOW, ParameterValue::Choice(2))
            .unwrap();
        assert_eq!(
            PitchSettings::decode(&patched).unwrap(),
            PitchSettings {
                semitones: -24,
                lowest: 36,
                range: 48,
                overflow: PitchOverflowPolicy::Fold,
            }
        );

        // Out-of-range parameter writes are refused.
        assert!(
            registry
                .patch_parameter(&default, parameter::PITCH, ParameterValue::Float(49.0))
                .is_err()
        );
        assert!(
            registry
                .patch_parameter(&default, parameter::PITCH, ParameterValue::Float(-49.0))
                .is_err()
        );
        assert!(
            registry
                .patch_parameter(&default, parameter::LOWEST, ParameterValue::Float(128.0))
                .is_err()
        );
        assert!(
            registry
                .patch_parameter(&default, parameter::OVERFLOW, ParameterValue::Choice(3))
                .is_err()
        );

        // Malformed states are rejected deterministically.
        let bad_len = NativeDeviceState::new(PITCH_TYPE_ID, 1, vec![0, 0, 127]).unwrap();
        assert_eq!(
            PitchSettings::decode(&bad_len),
            Err(DeviceContractError::MalformedState)
        );
        let bad_ver = NativeDeviceState::new(PITCH_TYPE_ID, 2, vec![0, 0, 127, 0]).unwrap();
        assert_eq!(
            PitchSettings::decode(&bad_ver),
            Err(DeviceContractError::UnsupportedStateVersion)
        );
        let bad_semi =
            NativeDeviceState::new(PITCH_TYPE_ID, 1, vec![60_i8 as u8, 0, 127, 0]).unwrap();
        assert_eq!(
            PitchSettings::decode(&bad_semi),
            Err(DeviceContractError::InvalidParameterValue)
        );
        let bad_policy = NativeDeviceState::new(PITCH_TYPE_ID, 1, vec![0, 0, 127, 9]).unwrap();
        assert_eq!(
            PitchSettings::decode(&bad_policy),
            Err(DeviceContractError::MalformedState)
        );
    }

    #[test]
    fn pitch_boundaries_overflow_policies_and_note_window_match_ledger() {
        // Clamp policy at 0 and 127 boundaries.
        let clamp_high = PitchSettings {
            semitones: 48,
            overflow: PitchOverflowPolicy::Clamp,
            ..PitchSettings::default()
        };
        assert_eq!(clamp_high.map_pitch(100), Some(127));
        assert_eq!(clamp_high.map_pitch(127), Some(127));

        let clamp_low = PitchSettings {
            semitones: -48,
            overflow: PitchOverflowPolicy::Clamp,
            ..PitchSettings::default()
        };
        assert_eq!(clamp_low.map_pitch(12), Some(0));
        assert_eq!(clamp_low.map_pitch(0), Some(0));

        // Fold policy folds by octaves preserving pitch class into 0..=127.
        let fold_high = PitchSettings {
            semitones: 12,
            overflow: PitchOverflowPolicy::Fold,
            ..PitchSettings::default()
        };
        assert_eq!(fold_high.map_pitch(120), Some(120)); // 132 -> 120 (C)
        assert_eq!(fold_high.map_pitch(127), Some(127)); // 139 -> 127 (G)
        assert_eq!(fold_high.map_pitch(116), Some(116)); // 128 -> 116 (G#)

        let fold_low = PitchSettings {
            semitones: -12,
            overflow: PitchOverflowPolicy::Fold,
            ..PitchSettings::default()
        };
        assert_eq!(fold_low.map_pitch(0), Some(0)); // -12 -> 0
        assert_eq!(fold_low.map_pitch(11), Some(11)); // -1 -> 11
        assert_eq!(fold_low.map_pitch(5), Some(5)); // -7 -> 5

        // Drop policy + window gating suppress out-of-range notes and their MPE/NoteOffs.
        let mut pair = make_pair(PitchSettings {
            semitones: 24,
            lowest: 48,
            range: 60, // accepts [48..=108]
            overflow: PitchOverflowPolicy::Drop,
        });
        let mut ledger = DownstreamLedger::default();
        let mut out = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; 16];

        let input = [
            // Gated below lowest (47 < 48) -> dropped
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 47,
                velocity: 100,
            },
            // In window and in range (60 + 24 = 84) -> emitted
            NativeInstrumentEvent::NoteOn {
                frame_offset: 1,
                occurrence_id: 2,
                pitch: 60,
                velocity: 100,
            },
            // In window (108 <= 108) but 108 + 24 = 132 > 127 -> dropped
            NativeInstrumentEvent::NoteOn {
                frame_offset: 2,
                occurrence_id: 3,
                pitch: 108,
                velocity: 100,
            },
            // Per-note pressure on dropped occurrence 3 -> suppressed cleanly
            NativeInstrumentEvent::NotePressure {
                frame_offset: 3,
                occurrence_id: NonZeroU64::new(3).unwrap(),
                value: 90,
            },
            // Per-note pressure on emitted occurrence 2 -> passed through
            NativeInstrumentEvent::NotePressure {
                frame_offset: 4,
                occurrence_id: NonZeroU64::new(2).unwrap(),
                value: 80,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 5,
                occurrence_id: 1,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 6,
                occurrence_id: 3,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 7,
                occurrence_id: 2,
            },
        ];

        pair.begin_block();
        let written = pair.process_block(&input, 64, &mut out, None).unwrap();
        pair.commit_block();
        assert_eq!(written, 3);
        ledger.observe(&out[..written]);
        assert!(ledger.active.is_empty());
        assert!(pair.committed_is_quiescent());
    }

    #[test]
    fn same_pitch_overlaps_and_transposition_changes_while_held_leave_no_stuck_notes() {
        let mut pair_0 = make_pair(PitchSettings {
            semitones: 0,
            ..PitchSettings::default()
        });
        let mut ledger = DownstreamLedger::default();
        let mut out = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; 16];

        // Start two overlapping C4 notes (occ 1 and occ 2) at +0 st.
        let block1 = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 60,
                velocity: 100,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 8,
                occurrence_id: 2,
                pitch: 60,
                velocity: 90,
            },
        ];
        pair_0.begin_block();
        let n1 = pair_0.process_block(&block1, 64, &mut out, None).unwrap();
        pair_0.commit_block();
        ledger.observe(&out[..n1]);
        assert_eq!(ledger.active.get(&1), Some(&60));
        assert_eq!(ledger.active.get(&2), Some(&60));

        // Change transposition to +7 st WHILE both C4 notes are held!
        let mut pair_7 = make_pair(PitchSettings {
            semitones: 7,
            ..PitchSettings::default()
        });
        assert!(pair_7.can_inherit_from(&pair_0));
        pair_7.inherit_from(&pair_0).unwrap();

        // New C4 (occ 3) arrives at +7 -> G4 (67), while occ 1 releases cleanly.
        let block2 = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 3,
                pitch: 60,
                velocity: 110,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 16,
                occurrence_id: 1,
            },
        ];
        pair_7.begin_block();
        let n2 = pair_7.process_block(&block2, 64, &mut out, None).unwrap();
        pair_7.commit_block();
        ledger.observe(&out[..n2]);
        assert_eq!(ledger.active.len(), 2);
        assert_eq!(ledger.active.get(&2), Some(&60));
        assert_eq!(ledger.active.get(&3), Some(&67));

        // Change again to +48 Drop while occ 2 and occ 3 are held, then trigger dropped note (occ 4) and release all.
        let mut pair_drop = make_pair(PitchSettings {
            semitones: 48,
            overflow: PitchOverflowPolicy::Drop,
            ..PitchSettings::default()
        });
        assert!(pair_drop.can_inherit_from(&pair_7));
        pair_drop.inherit_from(&pair_7).unwrap();

        let block3 = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 4,
                pitch: 100, // 148 -> dropped
                velocity: 100,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 4,
                occurrence_id: 2,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 8,
                occurrence_id: 4,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 12,
                occurrence_id: 3,
            },
        ];
        pair_drop.begin_block();
        let n3 = pair_drop
            .process_block(&block3, 64, &mut out, None)
            .unwrap();
        pair_drop.commit_block();
        assert_eq!(n3, 2); // Only occ 2 and occ 3 release downstream; occ 4 stayed suppressed.
        ledger.observe(&out[..n3]);
        assert!(ledger.active.is_empty());
        assert!(pair_drop.committed_is_quiescent());
    }

    #[test]
    fn saturated_queue_and_invalid_batches_refuse_deterministically_with_no_stuck_notes() {
        let prepared =
            PreparedPitchMidiProcessor::with_caps(PitchSettings::default(), 4, 2, 2, 16).unwrap();
        let mut pair = NativeMidiProcessorRuntimePair::new(&prepared, test_limits()).unwrap();
        let mut ledger = DownstreamLedger::default();
        let mut out = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; 8];

        // Fill active occurrence capacity to 2/2.
        let ok_block = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 10,
                pitch: 60,
                velocity: 100,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 1,
                occurrence_id: 11,
                pitch: 64,
                velocity: 100,
            },
        ];
        pair.begin_block();
        let n = pair.process_block(&ok_block, 64, &mut out, None).unwrap();
        pair.commit_block();
        ledger.observe(&out[..n]);

        // 3rd simultaneous note exceeds active capacity -> deterministic refusal, committed intact.
        let overflow_active = [NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 12,
            pitch: 67,
            velocity: 100,
        }];
        pair.begin_block();
        assert_eq!(
            pair.process_block(&overflow_active, 64, &mut out, None),
            Err(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded)
        );
        pair.rollback_block();
        assert_eq!(pair.committed_active_occurrences(), 2);

        // Saturated output queue (3 events > max_output_events=2) -> deterministic refusal.
        let overflow_output = [
            NativeInstrumentEvent::ControlChange {
                frame_offset: 0,
                cc: 1,
                value: 64,
            },
            NativeInstrumentEvent::ControlChange {
                frame_offset: 1,
                cc: 1,
                value: 65,
            },
            NativeInstrumentEvent::ControlChange {
                frame_offset: 2,
                cc: 1,
                value: 66,
            },
        ];
        pair.begin_block();
        assert_eq!(
            pair.process_block(&overflow_output, 64, &mut out, None),
            Err(NativeMidiProcessorError::OutputEventCapacityExceeded)
        );
        pair.rollback_block();
        assert_eq!(pair.committed_active_occurrences(), 2);

        // Flush active note-offs drains both held notes deterministically -> zero stuck notes.
        pair.begin_block();
        let flushed = pair.flush_active_note_offs(0, 64, &mut out).unwrap();
        pair.commit_block();
        assert_eq!(flushed, 2);
        ledger.observe(&out[..flushed]);
        assert!(ledger.active.is_empty());
        assert!(pair.committed_is_quiescent());
    }
}
