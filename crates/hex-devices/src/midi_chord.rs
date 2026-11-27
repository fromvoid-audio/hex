//! Original native MIDI Chord processor using the bounded `hex-lattice`
//! MIDI processor contract (`hex.midi.chord`).
//!
//! Fans each incoming MIDI note-on into its root voice plus up to 6 configurable
//! transposition intervals (`shift 1..=6`, `-36..=+36 st`, `0` = off) with
//! per-voice velocity scaling (`velocity 1..=6`, `0.0..=2.0`).
//!
//! Every generated voice receives a distinct monotonic downstream `occurrence_id`
//! from a disjoint high namespace (`>= 1 << 48`) so generated voices never
//! collide with raw source occurrence IDs (including during host bypass
//! transitions), overlapping same-pitch inputs, or duplicate intervals.
//!
//! `preflight_block` validates all source slots, total active downstream voices,
//! output capacity, and work budget across the full event batch before mutating
//! state or emitting any event, guaranteeing zero partial fanout, zero stolen
//! terminal note-offs, and zero stuck notes.

use hex_lattice::{
    MidiProcessorOutput, MutableMidiProcessorRuntime, NativeInstrumentClock, NativeInstrumentEvent,
    NativeMidiProcessorDefinition, NativeMidiProcessorError, PreparedNativeMidiProcessor,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;
use std::num::NonZeroU64;

pub const CHORD_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.midi.chord");
pub const CHORD_STATE_VERSION: u32 = 1;
pub const CHORD_RUNTIME_VERSION: u32 = 1;

pub const CHORD_SHIFT_COUNT: usize = 6;
/// Root voice + 6 shift voices.
pub const MAX_CHORD_VOICES: usize = 1 + CHORD_SHIFT_COUNT;
pub const MIN_SHIFT_SEMITONES: i8 = -36;
pub const MAX_SHIFT_SEMITONES: i8 = 36;
pub const MAX_MIDI_NOTE: u8 = 127;

pub const CHORD_MAX_INPUT_EVENTS: usize = 259;
pub const CHORD_MAX_OUTPUT_EVENTS: usize = 259;
pub const CHORD_MAX_ACTIVE_OCCURRENCES: usize = 64;
pub const CHORD_MAX_WORK_UNITS: usize = CHORD_MAX_INPUT_EVENTS + CHORD_MAX_OUTPUT_EVENTS;

/// Disjoint generated occurrence-ID base so chord-emitted voices never collide
/// with canonical host source IDs (`1, 2, 3, ...`) even across bypass toggles.
const GENERATED_OCCURRENCE_BASE: u64 = 1_u64 << 48;

/// 6 shift bytes (`i8`) + 6 velocity-percent bytes (`u8`, 0..=200 -> 0.0..=2.0).
const STATE_BYTES: usize = CHORD_SHIFT_COUNT * 2;
const MAX_VELOCITY_SCALE_PERCENT: u8 = 200;

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const SHIFT_1: ParameterId = ParameterId::from_static(1);
    pub const SHIFT_2: ParameterId = ParameterId::from_static(2);
    pub const SHIFT_3: ParameterId = ParameterId::from_static(3);
    pub const SHIFT_4: ParameterId = ParameterId::from_static(4);
    pub const SHIFT_5: ParameterId = ParameterId::from_static(5);
    pub const SHIFT_6: ParameterId = ParameterId::from_static(6);

    pub const VELOCITY_1: ParameterId = ParameterId::from_static(7);
    pub const VELOCITY_2: ParameterId = ParameterId::from_static(8);
    pub const VELOCITY_3: ParameterId = ParameterId::from_static(9);
    pub const VELOCITY_4: ParameterId = ParameterId::from_static(10);
    pub const VELOCITY_5: ParameterId = ParameterId::from_static(11);
    pub const VELOCITY_6: ParameterId = ParameterId::from_static(12);
}

const fn shift_descriptor(id: ParameterId, label: &'static str) -> ParameterDescriptor {
    ParameterDescriptor {
        id,
        label,
        kind: ParameterKind::Float {
            min: MIN_SHIFT_SEMITONES as f32,
            max: MAX_SHIFT_SEMITONES as f32,
            unit: Some("st"),
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    }
}

const fn velocity_descriptor(id: ParameterId, label: &'static str) -> ParameterDescriptor {
    ParameterDescriptor {
        id,
        label,
        kind: ParameterKind::Float {
            min: 0.0,
            max: 2.0,
            unit: None,
        },
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::Linear,
    }
}

const DESCRIPTORS: [ParameterDescriptor; 12] = [
    shift_descriptor(parameter::SHIFT_1, "shift 1"),
    shift_descriptor(parameter::SHIFT_2, "shift 2"),
    shift_descriptor(parameter::SHIFT_3, "shift 3"),
    shift_descriptor(parameter::SHIFT_4, "shift 4"),
    shift_descriptor(parameter::SHIFT_5, "shift 5"),
    shift_descriptor(parameter::SHIFT_6, "shift 6"),
    velocity_descriptor(parameter::VELOCITY_1, "velocity 1"),
    velocity_descriptor(parameter::VELOCITY_2, "velocity 2"),
    velocity_descriptor(parameter::VELOCITY_3, "velocity 3"),
    velocity_descriptor(parameter::VELOCITY_4, "velocity 4"),
    velocity_descriptor(parameter::VELOCITY_5, "velocity 5"),
    velocity_descriptor(parameter::VELOCITY_6, "velocity 6"),
];

/// Validated authored settings for `hex.midi.chord`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChordSettings {
    /// Semitone offsets in `-36..=+36`; `0` disables the voice.
    pub shifts: [i8; CHORD_SHIFT_COUNT],
    /// Per-voice velocity scaling in integer percent `0..=200` (`100` = 1.0x).
    pub velocity_percent: [u8; CHORD_SHIFT_COUNT],
}

impl Default for ChordSettings {
    fn default() -> Self {
        Self {
            shifts: [0; CHORD_SHIFT_COUNT],
            velocity_percent: [100; CHORD_SHIFT_COUNT],
        }
    }
}

impl ChordSettings {
    pub fn encode(self) -> Result<NativeDeviceState, DeviceContractError> {
        self.validate()?;
        let mut payload = Vec::with_capacity(STATE_BYTES);
        for shift in self.shifts {
            payload.push(shift as u8);
        }
        for vel in self.velocity_percent {
            payload.push(vel);
        }
        NativeDeviceState::new(CHORD_TYPE_ID, CHORD_STATE_VERSION, payload)
    }

    pub fn decode(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != CHORD_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != CHORD_STATE_VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        if !state.asset_references().is_empty() {
            return Err(DeviceContractError::AssetResourcesUnavailable);
        }
        let payload = state.payload();
        if payload.len() != STATE_BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let mut shifts = [0_i8; CHORD_SHIFT_COUNT];
        let mut velocity_percent = [100_u8; CHORD_SHIFT_COUNT];
        for i in 0..CHORD_SHIFT_COUNT {
            shifts[i] = payload[i] as i8;
            velocity_percent[i] = payload[CHORD_SHIFT_COUNT + i];
        }
        let settings = Self {
            shifts,
            velocity_percent,
        };
        settings.validate()?;
        Ok(settings)
    }

    fn validate(self) -> Result<(), DeviceContractError> {
        for shift in self.shifts {
            if !(MIN_SHIFT_SEMITONES..=MAX_SHIFT_SEMITONES).contains(&shift) {
                return Err(DeviceContractError::InvalidParameterValue);
            }
        }
        for vel in self.velocity_percent {
            if vel > MAX_VELOCITY_SCALE_PERCENT {
                return Err(DeviceContractError::InvalidParameterValue);
            }
        }
        Ok(())
    }

    /// Compute the ordered generated `(pitch, velocity)` pairs for one incoming
    /// note-on. Root always comes first, followed by enabled non-zero shifts
    /// whose transposed pitch stays inside `0..=127`.
    pub fn expand_note(self, pitch: u8, velocity: u8) -> ([(u8, u8); MAX_CHORD_VOICES], usize) {
        let mut out = [(0_u8, 1_u8); MAX_CHORD_VOICES];
        if pitch > MAX_MIDI_NOTE || velocity == 0 {
            return (out, 0);
        }
        let mut len = 0_usize;
        out[len] = (pitch, velocity.min(127));
        len += 1;
        for i in 0..CHORD_SHIFT_COUNT {
            let shift = self.shifts[i];
            if shift == 0 {
                continue;
            }
            let shifted = i16::from(pitch) + i16::from(shift);
            if !(0..=i16::from(MAX_MIDI_NOTE)).contains(&shifted) {
                continue;
            }
            let scaled = (u16::from(velocity) * u16::from(self.velocity_percent[i]) + 50) / 100;
            let voice_vel = scaled.clamp(1, 127) as u8;
            out[len] = (shifted as u8, voice_vel);
            len += 1;
        }
        (out, len)
    }
}

pub static CHORD_DEFINITION: ChordMidiProcessorDefinition = ChordMidiProcessorDefinition;

pub struct ChordMidiProcessorDefinition;

fn shift_index(parameter: ParameterId) -> Option<usize> {
    match parameter {
        parameter::SHIFT_1 => Some(0),
        parameter::SHIFT_2 => Some(1),
        parameter::SHIFT_3 => Some(2),
        parameter::SHIFT_4 => Some(3),
        parameter::SHIFT_5 => Some(4),
        parameter::SHIFT_6 => Some(5),
        _ => None,
    }
}

fn velocity_index(parameter: ParameterId) -> Option<usize> {
    match parameter {
        parameter::VELOCITY_1 => Some(0),
        parameter::VELOCITY_2 => Some(1),
        parameter::VELOCITY_3 => Some(2),
        parameter::VELOCITY_4 => Some(3),
        parameter::VELOCITY_5 => Some(4),
        parameter::VELOCITY_6 => Some(5),
        _ => None,
    }
}

impl NativeDeviceMetadata for ChordMidiProcessorDefinition {
    fn type_id(&self) -> DeviceTypeId {
        CHORD_TYPE_ID
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("chord")
    }

    fn state_version(&self) -> u32 {
        CHORD_STATE_VERSION
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &DESCRIPTORS
    }

    fn ports(&self) -> &'static [hex_lattice_contract::DevicePortDescriptor] {
        hex_lattice_contract::MIDI_EFFECT_PORTS
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        ChordSettings::default().encode()
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let settings = ChordSettings::decode(state)?;
        settings.encode()?.with_host_metadata_from(state)
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let settings = ChordSettings::decode(state)?;
        if let Some(i) = shift_index(parameter) {
            return Ok(ParameterValue::Float(f32::from(settings.shifts[i])));
        }
        if let Some(i) = velocity_index(parameter) {
            return Ok(ParameterValue::Float(
                f32::from(settings.velocity_percent[i]) / 100.0,
            ));
        }
        Err(DeviceContractError::UnknownParameter)
    }

    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut settings = ChordSettings::decode(state)?;
        if let Some(i) = shift_index(parameter) {
            let ParameterValue::Float(v) = value else {
                return Err(DeviceContractError::InvalidParameterValue);
            };
            if !v.is_finite() {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            let rounded = v.round();
            if rounded < f32::from(MIN_SHIFT_SEMITONES) || rounded > f32::from(MAX_SHIFT_SEMITONES)
            {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            settings.shifts[i] = rounded as i8;
        } else if let Some(i) = velocity_index(parameter) {
            let ParameterValue::Float(v) = value else {
                return Err(DeviceContractError::InvalidParameterValue);
            };
            if !v.is_finite() || !(0.0..=2.0).contains(&v) {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            settings.velocity_percent[i] = (v * 100.0).round().clamp(0.0, 200.0) as u8;
        } else {
            return Err(DeviceContractError::UnknownParameter);
        }
        settings.encode()?.with_host_metadata_from(state)
    }
}

impl NativeMidiProcessorDefinition for ChordMidiProcessorDefinition {
    fn prepare_midi_processor(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Box<dyn PreparedNativeMidiProcessor>, DeviceContractError> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let settings = ChordSettings::decode(state)?;
        Ok(Box::new(PreparedChordMidiProcessor::new(settings)?))
    }
}

/// Control-prepared immutable factory for `hex.midi.chord`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedChordMidiProcessor {
    settings: ChordSettings,
    max_input_events: usize,
    max_output_events: usize,
    max_active_occurrences: usize,
    max_work_units: usize,
}

impl PreparedChordMidiProcessor {
    pub fn new(settings: ChordSettings) -> Result<Self, DeviceContractError> {
        Self::with_caps(
            settings,
            CHORD_MAX_INPUT_EVENTS,
            CHORD_MAX_OUTPUT_EVENTS,
            CHORD_MAX_ACTIVE_OCCURRENCES,
            CHORD_MAX_WORK_UNITS,
        )
    }

    pub fn with_caps(
        settings: ChordSettings,
        max_input_events: usize,
        max_output_events: usize,
        max_active_occurrences: usize,
        max_work_units: usize,
    ) -> Result<Self, DeviceContractError> {
        settings.validate()?;
        if max_input_events == 0
            || max_output_events == 0
            || max_active_occurrences == 0
            || max_active_occurrences > CHORD_MAX_ACTIVE_OCCURRENCES
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

    pub const fn settings(self) -> ChordSettings {
        self.settings
    }
}

impl PreparedNativeMidiProcessor for PreparedChordMidiProcessor {
    fn runtime_version(&self) -> u32 {
        CHORD_RUNTIME_VERSION
    }

    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<ChordMidiRuntime>()
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
        Box::new(ChordMidiRuntime {
            config: *self,
            active: [None; CHORD_MAX_ACTIVE_OCCURRENCES],
            next_generated_id: GENERATED_OCCURRENCE_BASE,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TrackedChordOccurrence {
    source_id: u64,
    generated: [u64; MAX_CHORD_VOICES],
    generated_len: u8,
}

#[derive(Clone)]
struct ChordMidiRuntime {
    config: PreparedChordMidiProcessor,
    active: [Option<TrackedChordOccurrence>; CHORD_MAX_ACTIVE_OCCURRENCES],
    next_generated_id: u64,
}

impl ChordMidiRuntime {
    fn find_slot(slots: &[Option<TrackedChordOccurrence>], source_id: u64) -> Option<usize> {
        slots
            .iter()
            .position(|slot| slot.is_some_and(|entry| entry.source_id == source_id))
    }

    fn find_free_slot(slots: &[Option<TrackedChordOccurrence>]) -> Option<usize> {
        slots.iter().position(Option::is_none)
    }

    fn total_generated_voices(slots: &[Option<TrackedChordOccurrence>]) -> usize {
        slots
            .iter()
            .flatten()
            .map(|entry| usize::from(entry.generated_len))
            .sum()
    }
}

impl MutableMidiProcessorRuntime for ChordMidiRuntime {
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
        let mut active_voices = Self::total_generated_voices(&simulated);
        let mut next_gen = self.next_generated_id;
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
                    velocity,
                    ..
                } => {
                    if Self::find_slot(&simulated, occurrence_id).is_some() {
                        return Err(NativeMidiProcessorError::DuplicateOccurrence);
                    }
                    let free = Self::find_free_slot(&simulated)
                        .ok_or(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded)?;
                    let (_, count) = self.config.settings.expand_note(pitch, velocity);
                    let new_total = active_voices
                        .checked_add(count)
                        .ok_or(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded)?;
                    if new_total > self.config.max_active_occurrences {
                        return Err(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded);
                    }
                    next_gen = next_gen
                        .checked_add(count as u64)
                        .ok_or(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded)?;
                    simulated[free] = Some(TrackedChordOccurrence {
                        source_id: occurrence_id,
                        generated: [0; MAX_CHORD_VOICES],
                        generated_len: count as u8,
                    });
                    active_voices = new_total;
                    output_count = output_count
                        .checked_add(count)
                        .ok_or(NativeMidiProcessorError::OutputEventCapacityExceeded)?;
                    work_units = work_units
                        .checked_add(count)
                        .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
                }
                NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    let slot = Self::find_slot(&simulated, occurrence_id)
                        .ok_or(NativeMidiProcessorError::OrphanedNoteOff)?;
                    let Some(entry) = simulated[slot].take() else {
                        return Err(NativeMidiProcessorError::OrphanedNoteOff);
                    };
                    let count = usize::from(entry.generated_len);
                    active_voices = active_voices.saturating_sub(count);
                    output_count = output_count
                        .checked_add(count)
                        .ok_or(NativeMidiProcessorError::OutputEventCapacityExceeded)?;
                    work_units = work_units
                        .checked_add(count)
                        .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
                }
                NativeInstrumentEvent::NotePressure { occurrence_id, .. }
                | NativeInstrumentEvent::NotePitchBend { occurrence_id, .. }
                | NativeInstrumentEvent::NoteTimbre { occurrence_id, .. } => {
                    let slot = Self::find_slot(&simulated, occurrence_id.get())
                        .ok_or(NativeMidiProcessorError::InvalidInputEventBatch)?;
                    let Some(entry) = simulated[slot] else {
                        return Err(NativeMidiProcessorError::InvalidInputEventBatch);
                    };
                    let count = usize::from(entry.generated_len);
                    output_count = output_count
                        .checked_add(count)
                        .ok_or(NativeMidiProcessorError::OutputEventCapacityExceeded)?;
                    work_units = work_units
                        .checked_add(count)
                        .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
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
        for &event in input {
            let _ = output.charge_work(1);
            match event {
                NativeInstrumentEvent::NoteOn {
                    frame_offset,
                    occurrence_id,
                    pitch,
                    velocity,
                } => {
                    let (voices, count) = self.config.settings.expand_note(pitch, velocity);
                    let mut generated = [0_u64; MAX_CHORD_VOICES];
                    for (slot, &(voice_pitch, voice_vel)) in voices[..count].iter().enumerate() {
                        let gen_id = self.next_generated_id;
                        self.next_generated_id = self.next_generated_id.saturating_add(1);
                        generated[slot] = gen_id;
                        let _ = output.charge_work(1);
                        let _ = output.push(NativeInstrumentEvent::NoteOn {
                            frame_offset,
                            occurrence_id: gen_id,
                            pitch: voice_pitch,
                            velocity: voice_vel,
                        });
                    }
                    if let Some(free) = Self::find_free_slot(&self.active) {
                        self.active[free] = Some(TrackedChordOccurrence {
                            source_id: occurrence_id,
                            generated,
                            generated_len: count as u8,
                        });
                    }
                }
                NativeInstrumentEvent::NoteOff {
                    frame_offset,
                    occurrence_id,
                } => {
                    if let Some(slot) = Self::find_slot(&self.active, occurrence_id)
                        && let Some(entry) = self.active[slot].take()
                    {
                        for &gen_id in &entry.generated[..usize::from(entry.generated_len)] {
                            let _ = output.charge_work(1);
                            let _ = output.push(NativeInstrumentEvent::NoteOff {
                                frame_offset,
                                occurrence_id: gen_id,
                            });
                        }
                    }
                }
                NativeInstrumentEvent::NotePressure {
                    frame_offset,
                    occurrence_id,
                    value,
                } => {
                    if let Some(slot) = Self::find_slot(&self.active, occurrence_id.get())
                        && let Some(entry) = self.active[slot]
                    {
                        for &gen_id in &entry.generated[..usize::from(entry.generated_len)] {
                            if let Some(id) = NonZeroU64::new(gen_id) {
                                let _ = output.charge_work(1);
                                let _ = output.push(NativeInstrumentEvent::NotePressure {
                                    frame_offset,
                                    occurrence_id: id,
                                    value,
                                });
                            }
                        }
                    }
                }
                NativeInstrumentEvent::NotePitchBend {
                    frame_offset,
                    occurrence_id,
                    cents,
                } => {
                    if let Some(slot) = Self::find_slot(&self.active, occurrence_id.get())
                        && let Some(entry) = self.active[slot]
                    {
                        for &gen_id in &entry.generated[..usize::from(entry.generated_len)] {
                            if let Some(id) = NonZeroU64::new(gen_id) {
                                let _ = output.charge_work(1);
                                let _ = output.push(NativeInstrumentEvent::NotePitchBend {
                                    frame_offset,
                                    occurrence_id: id,
                                    cents,
                                });
                            }
                        }
                    }
                }
                NativeInstrumentEvent::NoteTimbre {
                    frame_offset,
                    occurrence_id,
                    value,
                } => {
                    if let Some(slot) = Self::find_slot(&self.active, occurrence_id.get())
                        && let Some(entry) = self.active[slot]
                    {
                        for &gen_id in &entry.generated[..usize::from(entry.generated_len)] {
                            if let Some(id) = NonZeroU64::new(gen_id) {
                                let _ = output.charge_work(1);
                                let _ = output.push(NativeInstrumentEvent::NoteTimbre {
                                    frame_offset,
                                    occurrence_id: id,
                                    value,
                                });
                            }
                        }
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
        let emitted = Self::total_generated_voices(&self.active);
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
        let mut ids = [0_u64; CHORD_MAX_ACTIVE_OCCURRENCES];
        let mut len = 0_usize;
        for slot in &mut self.active {
            if let Some(entry) = slot.take() {
                for &gen_id in &entry.generated[..usize::from(entry.generated_len)] {
                    if len < ids.len() {
                        ids[len] = gen_id;
                        len += 1;
                    }
                }
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
        Self::total_generated_voices(&self.active)
    }

    fn reset(&mut self) {
        self.active.fill(None);
        self.next_generated_id = GENERATED_OCCURRENCE_BASE;
    }

    /// Already-held chords retain their exact generated downstream occurrence IDs
    /// so editing intervals or velocity scales mid-note releases every sounding
    /// voice cleanly while new notes adopt the updated chord voicing.
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
        self.active = other.active;
        self.next_generated_id = other.next_generated_id;
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
            max_input_events_per_block: CHORD_MAX_INPUT_EVENTS,
            max_output_events_per_block: CHORD_MAX_OUTPUT_EVENTS,
            max_active_occurrences: CHORD_MAX_ACTIVE_OCCURRENCES,
            max_work_units_per_block: CHORD_MAX_WORK_UNITS * 2,
        }
    }

    fn make_pair(settings: ChordSettings) -> NativeMidiProcessorRuntimePair {
        let prepared = PreparedChordMidiProcessor::new(settings).unwrap();
        NativeMidiProcessorRuntimePair::new(&prepared, test_limits()).unwrap()
    }

    #[derive(Default)]
    struct DownstreamLedger {
        active: BTreeMap<u64, (u8, u8)>,
    }

    impl DownstreamLedger {
        fn observe(&mut self, events: &[NativeInstrumentEvent]) {
            for &event in events {
                match event {
                    NativeInstrumentEvent::NoteOn {
                        occurrence_id,
                        pitch,
                        velocity,
                        ..
                    } => {
                        assert!(
                            self.active
                                .insert(occurrence_id, (pitch, velocity))
                                .is_none(),
                            "duplicate downstream note-on for generated occurrence {occurrence_id}"
                        );
                    }
                    NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                        assert!(
                            self.active.remove(&occurrence_id).is_some(),
                            "orphaned downstream note-off for generated occurrence {occurrence_id}"
                        );
                    }
                    NativeInstrumentEvent::NotePressure { occurrence_id, .. }
                    | NativeInstrumentEvent::NotePitchBend { occurrence_id, .. }
                    | NativeInstrumentEvent::NoteTimbre { occurrence_id, .. } => {
                        assert!(
                            self.active.contains_key(&occurrence_id.get()),
                            "per-note expression for inactive generated occurrence {}",
                            occurrence_id.get()
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn registry_state_codec_and_malformed_state_rejection() {
        let registry = crate::native_midi_processor_registry().unwrap();
        let default = registry.default_state(CHORD_TYPE_ID).unwrap();
        assert_eq!(
            ChordSettings::decode(&default).unwrap(),
            ChordSettings::default()
        );

        let patched = registry
            .patch_parameter(&default, parameter::SHIFT_1, ParameterValue::Float(4.0))
            .unwrap();
        let patched = registry
            .patch_parameter(&patched, parameter::SHIFT_2, ParameterValue::Float(7.0))
            .unwrap();
        let patched = registry
            .patch_parameter(&patched, parameter::VELOCITY_2, ParameterValue::Float(0.5))
            .unwrap();
        let decoded = ChordSettings::decode(&patched).unwrap();
        assert_eq!(decoded.shifts, [4, 7, 0, 0, 0, 0]);
        assert_eq!(decoded.velocity_percent, [100, 50, 100, 100, 100, 100]);

        // Out-of-range writes and malformed payloads refuse cleanly.
        assert!(
            registry
                .patch_parameter(&default, parameter::SHIFT_1, ParameterValue::Float(37.0))
                .is_err()
        );
        assert!(
            registry
                .patch_parameter(&default, parameter::VELOCITY_1, ParameterValue::Float(2.5))
                .is_err()
        );
        let bad_len = NativeDeviceState::new(CHORD_TYPE_ID, 1, vec![0; 4]).unwrap();
        assert_eq!(
            ChordSettings::decode(&bad_len),
            Err(DeviceContractError::MalformedState)
        );
    }

    #[test]
    fn overlapping_same_pitch_duplicate_intervals_boundaries_and_live_edits_leave_no_stuck_notes() {
        // Major triad with duplicate +7 shift and distinct velocity scaling.
        let triad = ChordSettings {
            shifts: [4, 7, 7, 12, 0, 0],
            velocity_percent: [100, 80, 50, 100, 100, 100],
        };
        let mut pair = make_pair(triad);
        let mut ledger = DownstreamLedger::default();
        let mut out = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; 64];

        // Overlapping C4 notes (occ 1 & 2) + high boundary note B8 (119: +12 = 131 omitted!).
        let block1 = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 60,
                velocity: 100,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 4,
                occurrence_id: 2,
                pitch: 60,
                velocity: 90,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 8,
                occurrence_id: 3,
                pitch: 119, // 119, 123, 126, 126 emitted; 131 omitted!
                velocity: 100,
            },
        ];
        pair.begin_block();
        let n1 = pair.process_block(&block1, 64, &mut out, None).unwrap();
        pair.commit_block();
        // occ 1 -> 5 voices, occ 2 -> 5 voices, occ 3 -> 4 voices = 14 voices total.
        assert_eq!(n1, 14);
        // Verify stable equal-offset ordering on occ 1: [60, 64, 67, 67, 72] with ascending IDs.
        let pitches: Vec<u8> = out[..5]
            .iter()
            .map(|e| match *e {
                NativeInstrumentEvent::NoteOn { pitch, .. } => pitch,
                _ => panic!("expected note on"),
            })
            .collect();
        assert_eq!(pitches, vec![60, 64, 67, 67, 72]);
        ledger.observe(&out[..n1]);
        assert_eq!(ledger.active.len(), 14);
        assert_eq!(pair.committed_active_occurrences(), 14);

        // Live edit chord voicing while all 3 source notes are held!
        let minor = ChordSettings {
            shifts: [3, 7, 0, 0, 0, 0],
            ..ChordSettings::default()
        };
        let mut pair_minor = make_pair(minor);
        assert!(pair_minor.can_inherit_from(&pair));
        pair_minor.inherit_from(&pair).unwrap();

        // New note (occ 4) gets minor voicing (3 voices), while occ 1 & 2 & 3 release all 14 voices!
        let block2 = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 4,
                pitch: 62,
                velocity: 100,
            },
            NativeInstrumentEvent::NotePressure {
                frame_offset: 2,
                occurrence_id: NonZeroU64::new(4).unwrap(),
                value: 75,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 4,
                occurrence_id: 1,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 6,
                occurrence_id: 2,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 8,
                occurrence_id: 3,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 10,
                occurrence_id: 4,
            },
        ];
        pair_minor.begin_block();
        let n2 = pair_minor
            .process_block(&block2, 64, &mut out, None)
            .unwrap();
        pair_minor.commit_block();
        // 3 NoteOn + 3 NotePressure + 5 + 5 + 4 + 3 NoteOff = 23 events.
        assert_eq!(n2, 23);
        ledger.observe(&out[..n2]);
        assert!(ledger.active.is_empty());
        assert!(pair_minor.committed_is_quiescent());
    }

    #[test]
    fn voice_and_output_saturation_refuse_atomically_without_stranding_voices_or_stealing_note_offs()
     {
        // 4-voice chord with active capacity 6 -> 1st note (4 voices) admitted, 2nd note (4+4=8 > 6) refused atomically!
        let four_voice = ChordSettings {
            shifts: [4, 7, 11, 0, 0, 0],
            ..ChordSettings::default()
        };
        let prepared = PreparedChordMidiProcessor::with_caps(four_voice, 16, 6, 6, 32).unwrap();
        let mut pair = NativeMidiProcessorRuntimePair::new(&prepared, test_limits()).unwrap();
        let mut ledger = DownstreamLedger::default();
        let mut out = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; 16];

        let first = [NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 60,
            velocity: 100,
        }];
        pair.begin_block();
        let n1 = pair.process_block(&first, 64, &mut out, None).unwrap();
        pair.commit_block();
        assert_eq!(n1, 4);
        ledger.observe(&out[..n1]);

        // Second note would exceed 6 active generated voices -> atomic refusal, zero partial emission!
        let second = [NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 2,
            pitch: 64,
            velocity: 100,
        }];
        pair.begin_block();
        assert_eq!(
            pair.process_block(&second, 64, &mut out, None),
            Err(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded)
        );
        pair.rollback_block();
        assert_eq!(pair.committed_active_occurrences(), 4);

        // Terminal NoteOff for occ 1 still succeeds cleanly with zero stolen note-offs!
        let release = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }];
        pair.begin_block();
        let n2 = pair.process_block(&release, 64, &mut out, None).unwrap();
        pair.commit_block();
        assert_eq!(n2, 4);
        ledger.observe(&out[..n2]);
        assert!(ledger.active.is_empty());
        assert!(pair.committed_is_quiescent());
    }
}
