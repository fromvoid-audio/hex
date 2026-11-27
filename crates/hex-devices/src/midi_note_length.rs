//! Original bounded native MIDI Note Length processor using the `hex-lattice`
//! MIDI processor and bounded future-event horizon contracts
//! (`hex.midi.note_length`).
//!
//! Supports explicit finite `trigger` (`note on` or `note off`), timing `mode`
//! (`time` in milliseconds via performance frames, or `sync` in musical beats
//! via the host's borrowed `NativeInstrumentClock`), `gate` percentage
//! (`1..=200%`), and `retrigger` (`overlap` or same-pitch `cut`). Owns a
//! transactional [`BoundedFutureMidiQueue`] inside its primary/scratch twins so
//! generated `NoteOff`s reserve queue capacity, follow host tempo changes,
//! survive live parameter edits cleanly, and leave zero stuck notes on Stop,
//! Seek, loop wrap, or bypass.

use hex_lattice::{
    BoundedFutureMidiQueue, DueMidiEvent, FUTURE_MIDI_EVENT_QUEUE_CAPACITY,
    FUTURE_MIDI_RESERVED_NOTE_OFF_SLOTS, FutureMidiTime, GENERATED_OCCURRENCE_ID_BASE,
    MidiProcessorOutput, MutableMidiProcessorRuntime, NativeInstrumentClock, NativeInstrumentEvent,
    NativeMidiProcessorDefinition, NativeMidiProcessorError, PreparedNativeMidiProcessor,
    ScheduledMidiKind,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;

pub const NOTE_LENGTH_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.midi.note_length");
pub const NOTE_LENGTH_STATE_VERSION: u32 = 1;
pub const NOTE_LENGTH_RUNTIME_VERSION: u32 = 1;

pub const MIN_TIME_MS: u16 = 1;
pub const MAX_TIME_MS: u16 = 10_000;
pub const MIN_GATE_PERCENT: u8 = 1;
pub const MAX_GATE_PERCENT: u8 = 200;

pub const NOTE_LENGTH_MAX_INPUT_EVENTS: usize = 259;
pub const NOTE_LENGTH_MAX_OUTPUT_EVENTS: usize = 259;
pub const NOTE_LENGTH_MAX_ACTIVE_OCCURRENCES: usize = 64;
pub const NOTE_LENGTH_MAX_WORK_UNITS: usize = NOTE_LENGTH_MAX_INPUT_EVENTS * 4;

/// `[trigger, mode, time_ms_le(2), sync_rate, gate_percent, retrigger]`
const STATE_BYTES: usize = 7;

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const TRIGGER: ParameterId = ParameterId::from_static(1);
    pub const MODE: ParameterId = ParameterId::from_static(2);
    pub const TIME_MS: ParameterId = ParameterId::from_static(3);
    pub const SYNC_RATE: ParameterId = ParameterId::from_static(4);
    pub const GATE: ParameterId = ParameterId::from_static(5);
    pub const RETRIGGER: ParameterId = ParameterId::from_static(6);
}

const TRIGGER_LABELS: [&str; 2] = ["note on", "note off"];
const MODE_LABELS: [&str; 2] = ["time", "sync"];
const SYNC_LABELS: [&str; 8] = [
    "1/64", "1/32", "1/16", "1/8", "1/4", "1/2", "1 bar", "2 bars",
];
const RETRIGGER_LABELS: [&str; 2] = ["overlap", "cut"];

/// Q32.32 quarter-note beat durations for each `SyncRate` choice.
const SYNC_BEATS_Q32: [u64; 8] = [
    1_u64 << 28, // 1/64 note = 1/16 quarter-note beat
    1_u64 << 29, // 1/32 note = 1/8 beat
    1_u64 << 30, // 1/16 note = 1/4 beat
    1_u64 << 31, // 1/8 note  = 1/2 beat
    1_u64 << 32, // 1/4 note  = 1 beat
    2_u64 << 32, // 1/2 note  = 2 beats
    4_u64 << 32, // 1 bar     = 4 beats
    8_u64 << 32, // 2 bars    = 8 beats
];

const DESCRIPTORS: [ParameterDescriptor; 6] = [
    ParameterDescriptor {
        id: parameter::TRIGGER,
        label: "trigger",
        kind: ParameterKind::Choice {
            labels: &TRIGGER_LABELS,
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::MODE,
        label: "mode",
        kind: ParameterKind::Choice {
            labels: &MODE_LABELS,
        },
        default: ParameterValue::Choice(1),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::TIME_MS,
        label: "length",
        kind: ParameterKind::Float {
            min: MIN_TIME_MS as f32,
            max: MAX_TIME_MS as f32,
            unit: Some("ms"),
        },
        default: ParameterValue::Float(250.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::SYNC_RATE,
        label: "sync",
        kind: ParameterKind::Choice {
            labels: &SYNC_LABELS,
        },
        default: ParameterValue::Choice(2),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::GATE,
        label: "gate",
        kind: ParameterKind::Float {
            min: MIN_GATE_PERCENT as f32,
            max: MAX_GATE_PERCENT as f32,
            unit: Some("%"),
        },
        default: ParameterValue::Float(100.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::RETRIGGER,
        label: "retrigger",
        kind: ParameterKind::Choice {
            labels: &RETRIGGER_LABELS,
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
];

/// Which incoming note edge triggers the generated fixed-length note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum NoteLengthTrigger {
    NoteOn = 0,
    NoteOff = 1,
}

impl NoteLengthTrigger {
    pub const fn from_choice(choice: u32) -> Option<Self> {
        match choice {
            0 => Some(Self::NoteOn),
            1 => Some(Self::NoteOff),
            _ => None,
        }
    }

    pub const fn as_choice(self) -> u32 {
        self as u32
    }
}

/// Timebase domain for generated note duration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum NoteLengthMode {
    /// Milliseconds converted to monotonic host performance frames.
    Time = 0,
    /// Musical beat division converted via host `NativeInstrumentClock`.
    Sync = 1,
}

impl NoteLengthMode {
    pub const fn from_choice(choice: u32) -> Option<Self> {
        match choice {
            0 => Some(Self::Time),
            1 => Some(Self::Sync),
            _ => None,
        }
    }

    pub const fn as_choice(self) -> u32 {
        self as u32
    }
}

/// Policy when a new trigger arrives on a pitch that already has a sounding
/// generated note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum NoteLengthRetrigger {
    /// Allow overlapping independent generated occurrences on the same pitch.
    Overlap = 0,
    /// Release the prior sounding generated occurrence on that pitch immediately
    /// before emitting the new onset.
    Cut = 1,
}

impl NoteLengthRetrigger {
    pub const fn from_choice(choice: u32) -> Option<Self> {
        match choice {
            0 => Some(Self::Overlap),
            1 => Some(Self::Cut),
            _ => None,
        }
    }

    pub const fn as_choice(self) -> u32 {
        self as u32
    }
}

/// Validated authored settings for `hex.midi.note_length`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteLengthSettings {
    pub trigger: NoteLengthTrigger,
    pub mode: NoteLengthMode,
    /// Duration in milliseconds `1..=10_000` (used when `mode == Time`).
    pub time_ms: u16,
    /// Index into `SYNC_BEATS_Q32` `0..8` (used when `mode == Sync`).
    pub sync_rate: u8,
    /// Gate percentage `1..=200`.
    pub gate_percent: u8,
    pub retrigger: NoteLengthRetrigger,
}

impl Default for NoteLengthSettings {
    fn default() -> Self {
        Self {
            trigger: NoteLengthTrigger::NoteOn,
            mode: NoteLengthMode::Sync,
            time_ms: 250,
            sync_rate: 2,
            gate_percent: 100,
            retrigger: NoteLengthRetrigger::Overlap,
        }
    }
}

impl NoteLengthSettings {
    pub fn encode(self) -> Result<NativeDeviceState, DeviceContractError> {
        self.validate()?;
        let mut payload = Vec::with_capacity(STATE_BYTES);
        payload.push(self.trigger as u8);
        payload.push(self.mode as u8);
        payload.extend_from_slice(&self.time_ms.to_le_bytes());
        payload.push(self.sync_rate);
        payload.push(self.gate_percent);
        payload.push(self.retrigger as u8);
        NativeDeviceState::new(NOTE_LENGTH_TYPE_ID, NOTE_LENGTH_STATE_VERSION, payload)
    }

    pub fn decode(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != NOTE_LENGTH_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != NOTE_LENGTH_STATE_VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        if !state.asset_references().is_empty() {
            return Err(DeviceContractError::AssetResourcesUnavailable);
        }
        let payload = state.payload();
        if payload.len() != STATE_BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let trigger = NoteLengthTrigger::from_choice(u32::from(payload[0]))
            .ok_or(DeviceContractError::MalformedState)?;
        let mode = NoteLengthMode::from_choice(u32::from(payload[1]))
            .ok_or(DeviceContractError::MalformedState)?;
        let time_ms = u16::from_le_bytes([payload[2], payload[3]]);
        let retrigger = NoteLengthRetrigger::from_choice(u32::from(payload[6]))
            .ok_or(DeviceContractError::MalformedState)?;
        let settings = Self {
            trigger,
            mode,
            time_ms,
            sync_rate: payload[4],
            gate_percent: payload[5],
            retrigger,
        };
        settings.validate()?;
        Ok(settings)
    }

    fn validate(self) -> Result<(), DeviceContractError> {
        if !(MIN_TIME_MS..=MAX_TIME_MS).contains(&self.time_ms)
            || usize::from(self.sync_rate) >= SYNC_BEATS_Q32.len()
            || !(MIN_GATE_PERCENT..=MAX_GATE_PERCENT).contains(&self.gate_percent)
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(())
    }

    /// Exact Q32.32 beat duration for `Sync` mode (`>= 1`).
    pub const fn sync_duration_beats_q32(self) -> u64 {
        let base = SYNC_BEATS_Q32[self.sync_rate as usize] as u128;
        let scaled = (base * (self.gate_percent as u128)) / 100;
        if scaled == 0 { 1 } else { scaled as u64 }
    }

    /// Exact frame duration for `Time` mode at `sample_rate` (`>= 1`).
    pub const fn time_duration_frames(self, sample_rate: u32) -> u64 {
        let num = (sample_rate as u128) * (self.time_ms as u128) * (self.gate_percent as u128);
        let frames = (num + 50_000) / 100_000;
        if frames == 0 { 1 } else { frames as u64 }
    }
}

pub static NOTE_LENGTH_DEFINITION: NoteLengthMidiProcessorDefinition =
    NoteLengthMidiProcessorDefinition;

pub struct NoteLengthMidiProcessorDefinition;

impl NativeDeviceMetadata for NoteLengthMidiProcessorDefinition {
    fn type_id(&self) -> DeviceTypeId {
        NOTE_LENGTH_TYPE_ID
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("note length")
    }

    fn state_version(&self) -> u32 {
        NOTE_LENGTH_STATE_VERSION
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &DESCRIPTORS
    }

    fn ports(&self) -> &'static [hex_lattice_contract::DevicePortDescriptor] {
        hex_lattice_contract::MIDI_EFFECT_PORTS
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NoteLengthSettings::default().encode()
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let settings = NoteLengthSettings::decode(state)?;
        settings.encode()?.with_host_metadata_from(state)
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let settings = NoteLengthSettings::decode(state)?;
        match parameter {
            parameter::TRIGGER => Ok(ParameterValue::Choice(settings.trigger.as_choice())),
            parameter::MODE => Ok(ParameterValue::Choice(settings.mode.as_choice())),
            parameter::TIME_MS => Ok(ParameterValue::Float(f32::from(settings.time_ms))),
            parameter::SYNC_RATE => Ok(ParameterValue::Choice(u32::from(settings.sync_rate))),
            parameter::GATE => Ok(ParameterValue::Float(f32::from(settings.gate_percent))),
            parameter::RETRIGGER => Ok(ParameterValue::Choice(settings.retrigger.as_choice())),
            _ => Err(DeviceContractError::UnknownParameter),
        }
    }

    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut settings = NoteLengthSettings::decode(state)?;
        match (parameter, value) {
            (parameter::TRIGGER, ParameterValue::Choice(choice)) => {
                settings.trigger = NoteLengthTrigger::from_choice(choice)
                    .ok_or(DeviceContractError::InvalidParameterValue)?;
            }
            (parameter::MODE, ParameterValue::Choice(choice)) => {
                settings.mode = NoteLengthMode::from_choice(choice)
                    .ok_or(DeviceContractError::InvalidParameterValue)?;
            }
            (parameter::TIME_MS, ParameterValue::Float(v)) if v.is_finite() => {
                let r = v.round();
                if r < f32::from(MIN_TIME_MS) || r > f32::from(MAX_TIME_MS) {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.time_ms = r as u16;
            }
            (parameter::SYNC_RATE, ParameterValue::Choice(choice)) => {
                if choice as usize >= SYNC_BEATS_Q32.len() {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.sync_rate = choice as u8;
            }
            (parameter::GATE, ParameterValue::Float(v)) if v.is_finite() => {
                let r = v.round();
                if r < f32::from(MIN_GATE_PERCENT) || r > f32::from(MAX_GATE_PERCENT) {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.gate_percent = r as u8;
            }
            (parameter::RETRIGGER, ParameterValue::Choice(choice)) => {
                settings.retrigger = NoteLengthRetrigger::from_choice(choice)
                    .ok_or(DeviceContractError::InvalidParameterValue)?;
            }
            (
                parameter::TRIGGER
                | parameter::MODE
                | parameter::TIME_MS
                | parameter::SYNC_RATE
                | parameter::GATE
                | parameter::RETRIGGER,
                _,
            ) => return Err(DeviceContractError::InvalidParameterValue),
            _ => return Err(DeviceContractError::UnknownParameter),
        }
        settings.encode()?.with_host_metadata_from(state)
    }
}

impl NativeMidiProcessorDefinition for NoteLengthMidiProcessorDefinition {
    fn prepare_midi_processor(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Box<dyn PreparedNativeMidiProcessor>, DeviceContractError> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let settings = NoteLengthSettings::decode(state)?;
        Ok(Box::new(PreparedNoteLengthMidiProcessor::new(
            settings,
            sample_rate,
        )?))
    }
}

/// Control-prepared immutable factory for `hex.midi.note_length`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedNoteLengthMidiProcessor {
    settings: NoteLengthSettings,
    sample_rate: u32,
    max_input_events: usize,
    max_output_events: usize,
    max_active_occurrences: usize,
    max_work_units: usize,
}

impl PreparedNoteLengthMidiProcessor {
    pub fn new(
        settings: NoteLengthSettings,
        sample_rate: u32,
    ) -> Result<Self, DeviceContractError> {
        Self::with_caps(
            settings,
            sample_rate,
            NOTE_LENGTH_MAX_INPUT_EVENTS,
            NOTE_LENGTH_MAX_OUTPUT_EVENTS,
            NOTE_LENGTH_MAX_ACTIVE_OCCURRENCES,
            NOTE_LENGTH_MAX_WORK_UNITS,
        )
    }

    pub fn with_caps(
        settings: NoteLengthSettings,
        sample_rate: u32,
        max_input_events: usize,
        max_output_events: usize,
        max_active_occurrences: usize,
        max_work_units: usize,
    ) -> Result<Self, DeviceContractError> {
        settings.validate()?;
        if !(8_000..=192_000).contains(&sample_rate)
            || max_input_events == 0
            || max_output_events == 0
            || max_active_occurrences == 0
            || max_active_occurrences > NOTE_LENGTH_MAX_ACTIVE_OCCURRENCES
            || max_work_units == 0
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(Self {
            settings,
            sample_rate,
            max_input_events,
            max_output_events,
            max_active_occurrences,
            max_work_units,
        })
    }

    pub const fn settings(self) -> NoteLengthSettings {
        self.settings
    }
}

impl PreparedNativeMidiProcessor for PreparedNoteLengthMidiProcessor {
    fn runtime_version(&self) -> u32 {
        NOTE_LENGTH_RUNTIME_VERSION
    }

    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<NoteLengthMidiRuntime>()
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
        let queue = BoundedFutureMidiQueue::new(
            FUTURE_MIDI_EVENT_QUEUE_CAPACITY,
            FUTURE_MIDI_RESERVED_NOTE_OFF_SLOTS,
        )
        .unwrap_or_default();
        Box::new(NoteLengthMidiRuntime {
            config: *self,
            queue,
            next_generated_id: GENERATED_OCCURRENCE_ID_BASE,
            active_inputs: [None; NOTE_LENGTH_MAX_ACTIVE_OCCURRENCES],
            active_generated: [None; NOTE_LENGTH_MAX_ACTIVE_OCCURRENCES],
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TrackedInputNote {
    occurrence_id: u64,
    pitch: u8,
    velocity: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TrackedGeneratedNote {
    occurrence_id: u64,
    pitch: u8,
}

#[derive(Clone)]
struct NoteLengthMidiRuntime {
    config: PreparedNoteLengthMidiProcessor,
    queue: BoundedFutureMidiQueue,
    next_generated_id: u64,
    active_inputs: [Option<TrackedInputNote>; NOTE_LENGTH_MAX_ACTIVE_OCCURRENCES],
    active_generated: [Option<TrackedGeneratedNote>; NOTE_LENGTH_MAX_ACTIVE_OCCURRENCES],
}

struct EmitBatch<'a> {
    out: &'a mut [NativeInstrumentEvent; NOTE_LENGTH_MAX_OUTPUT_EVENTS],
    len: &'a mut usize,
    cap: usize,
    work_units: &'a mut usize,
}

impl NoteLengthMidiRuntime {
    fn find_input(slots: &[Option<TrackedInputNote>], id: u64) -> Option<usize> {
        slots
            .iter()
            .position(|slot| slot.is_some_and(|entry| entry.occurrence_id == id))
    }

    fn find_free_input(slots: &[Option<TrackedInputNote>]) -> Option<usize> {
        slots.iter().position(Option::is_none)
    }

    fn find_generated(slots: &[Option<TrackedGeneratedNote>], id: u64) -> Option<usize> {
        slots
            .iter()
            .position(|slot| slot.is_some_and(|entry| entry.occurrence_id == id))
    }

    fn find_free_generated(slots: &[Option<TrackedGeneratedNote>]) -> Option<usize> {
        slots.iter().position(Option::is_none)
    }

    fn count_inputs(slots: &[Option<TrackedInputNote>]) -> usize {
        slots.iter().flatten().count()
    }

    fn count_generated(slots: &[Option<TrackedGeneratedNote>]) -> usize {
        slots.iter().flatten().count()
    }

    fn compute_due(
        settings: NoteLengthSettings,
        sample_rate: u32,
        frame_offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<FutureMidiTime, NativeMidiProcessorError> {
        let Some(clock_ref) = clock else {
            return Err(NativeMidiProcessorError::ClockUnavailable);
        };
        match settings.mode {
            NoteLengthMode::Sync => {
                let now = clock_ref
                    .beat_at_offset(frame_offset)
                    .ok_or(NativeMidiProcessorError::ClockUnavailable)?;
                let due = now
                    .checked_add(settings.sync_duration_beats_q32())
                    .ok_or(NativeMidiProcessorError::FutureEventHorizonExceeded)?;
                Ok(FutureMidiTime::BeatQ32(due))
            }
            NoteLengthMode::Time => {
                let now = clock_ref
                    .performance_frame_at_offset(frame_offset)
                    .ok_or(NativeMidiProcessorError::ClockUnavailable)?;
                let due = now
                    .checked_add(settings.time_duration_frames(sample_rate))
                    .ok_or(NativeMidiProcessorError::FutureEventHorizonExceeded)?;
                Ok(FutureMidiTime::PerformanceFrame(due))
            }
        }
    }

    fn trigger_generated_note(
        &mut self,
        frame_offset: usize,
        pitch: u8,
        velocity: u8,
        clock: Option<&dyn NativeInstrumentClock>,
        batch: &mut EmitBatch<'_>,
    ) -> Result<(), NativeMidiProcessorError> {
        let active_cap = self.config.max_active_occurrences;
        if self.config.settings.retrigger == NoteLengthRetrigger::Cut {
            for slot in &mut self.active_generated[..active_cap] {
                if let Some(existing) = *slot
                    && existing.pitch == pitch
                {
                    *slot = None;
                    let _ = self.queue.cancel_occurrence(existing.occurrence_id);
                    if *batch.len >= batch.cap {
                        return Err(NativeMidiProcessorError::OutputEventCapacityExceeded);
                    }
                    batch.out[*batch.len] = NativeInstrumentEvent::NoteOff {
                        frame_offset,
                        occurrence_id: existing.occurrence_id,
                    };
                    *batch.len += 1;
                    *batch.work_units = batch
                        .work_units
                        .checked_add(1)
                        .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
                }
            }
        }
        let due = Self::compute_due(
            self.config.settings,
            self.config.sample_rate,
            frame_offset,
            clock,
        )?;
        let gen_id = self.next_generated_id;
        self.next_generated_id = gen_id
            .checked_add(1)
            .ok_or(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded)?;
        let free = Self::find_free_generated(&self.active_generated[..active_cap])
            .ok_or(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded)?;
        if *batch.len >= batch.cap {
            return Err(NativeMidiProcessorError::OutputEventCapacityExceeded);
        }
        // Schedule paired NoteOff BEFORE committing active_generated so capacity refusal is atomic.
        self.queue.schedule(
            due,
            ScheduledMidiKind::NoteOff {
                occurrence_id: gen_id,
            },
            clock,
        )?;
        self.active_generated[free] = Some(TrackedGeneratedNote {
            occurrence_id: gen_id,
            pitch,
        });
        batch.out[*batch.len] = NativeInstrumentEvent::NoteOn {
            frame_offset,
            occurrence_id: gen_id,
            pitch,
            velocity,
        };
        *batch.len += 1;
        *batch.work_units = batch
            .work_units
            .checked_add(2)
            .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
        Ok(())
    }

    fn simulate_block(
        &self,
        input: &[NativeInstrumentEvent],
        frame_count: usize,
        output_capacity: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        emit: Option<&mut MidiProcessorOutput<'_>>,
    ) -> Result<(Self, usize), NativeMidiProcessorError> {
        if frame_count == 0 {
            return Err(NativeMidiProcessorError::InvalidInputEventBatch);
        }
        if input.len() > self.config.max_input_events {
            return Err(NativeMidiProcessorError::InputEventCapacityExceeded);
        }
        let mut state = self.clone();
        let active_cap = state.config.max_active_occurrences;
        let out_cap = output_capacity.min(state.config.max_output_events);
        let mut work_units = 0_usize;

        let mut merged = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; NOTE_LENGTH_MAX_OUTPUT_EVENTS];
        let mut merged_len = 0_usize;

        for &event in input {
            work_units = work_units
                .checked_add(1)
                .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
            match event {
                NativeInstrumentEvent::NoteOn {
                    frame_offset,
                    occurrence_id,
                    pitch,
                    velocity,
                } => {
                    if Self::find_input(&state.active_inputs[..active_cap], occurrence_id).is_some()
                    {
                        return Err(NativeMidiProcessorError::DuplicateOccurrence);
                    }
                    let free = Self::find_free_input(&state.active_inputs[..active_cap])
                        .ok_or(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded)?;
                    state.active_inputs[free] = Some(TrackedInputNote {
                        occurrence_id,
                        pitch,
                        velocity,
                    });
                    if state.config.settings.trigger == NoteLengthTrigger::NoteOn {
                        let mut batch = EmitBatch {
                            out: &mut merged,
                            len: &mut merged_len,
                            cap: out_cap,
                            work_units: &mut work_units,
                        };
                        state.trigger_generated_note(
                            frame_offset,
                            pitch,
                            velocity,
                            clock,
                            &mut batch,
                        )?;
                    }
                }
                NativeInstrumentEvent::NoteOff {
                    frame_offset,
                    occurrence_id,
                } => {
                    let slot = Self::find_input(&state.active_inputs[..active_cap], occurrence_id)
                        .ok_or(NativeMidiProcessorError::OrphanedNoteOff)?;
                    let Some(tracked) = state.active_inputs[slot].take() else {
                        return Err(NativeMidiProcessorError::OrphanedNoteOff);
                    };
                    if state.config.settings.trigger == NoteLengthTrigger::NoteOff {
                        let mut batch = EmitBatch {
                            out: &mut merged,
                            len: &mut merged_len,
                            cap: out_cap,
                            work_units: &mut work_units,
                        };
                        state.trigger_generated_note(
                            frame_offset,
                            tracked.pitch,
                            tracked.velocity,
                            clock,
                            &mut batch,
                        )?;
                    }
                }
                NativeInstrumentEvent::NotePressure { occurrence_id, .. }
                | NativeInstrumentEvent::NotePitchBend { occurrence_id, .. }
                | NativeInstrumentEvent::NoteTimbre { occurrence_id, .. } => {
                    if Self::find_input(&state.active_inputs[..active_cap], occurrence_id.get())
                        .is_none()
                    {
                        return Err(NativeMidiProcessorError::InvalidInputEventBatch);
                    }
                }
                NativeInstrumentEvent::Expression { .. }
                | NativeInstrumentEvent::ControlChange { .. }
                | NativeInstrumentEvent::PitchBend { .. } => {
                    if merged_len >= out_cap {
                        return Err(NativeMidiProcessorError::OutputEventCapacityExceeded);
                    }
                    merged[merged_len] = event;
                    merged_len += 1;
                    work_units = work_units
                        .checked_add(1)
                        .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
                }
            }
            if work_units > state.config.max_work_units {
                return Err(NativeMidiProcessorError::WorkBudgetExceeded);
            }
        }

        // Drain any scheduled generated NoteOffs due inside [0, frame_count).
        let mut due_buf = [DueMidiEvent {
            frame_offset: 0,
            sequence: 0,
            kind: ScheduledMidiKind::NoteOff { occurrence_id: 1 },
        }; FUTURE_MIDI_EVENT_QUEUE_CAPACITY];
        let due_len = if !state.queue.is_empty() {
            state.queue.drain_due(frame_count, clock, &mut due_buf)?
        } else {
            0
        };
        for &due in &due_buf[..due_len] {
            if let ScheduledMidiKind::NoteOff { occurrence_id } = due.kind {
                let slot =
                    Self::find_generated(&state.active_generated[..active_cap], occurrence_id)
                        .ok_or(NativeMidiProcessorError::OrphanedNoteOff)?;
                state.active_generated[slot] = None;
            }
            if merged_len >= out_cap {
                return Err(NativeMidiProcessorError::OutputEventCapacityExceeded);
            }
            merged[merged_len] = due.into_native_event();
            merged_len += 1;
            work_units = work_units
                .checked_add(1)
                .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
            if work_units > state.config.max_work_units {
                return Err(NativeMidiProcessorError::WorkBudgetExceeded);
            }
        }

        // Allocation-free stable sort by `(frame_offset, rank)`:
        // CC/Expr (0) < NoteOff (1) < NoteOn (2).
        let sort_key = |ev: NativeInstrumentEvent| -> (usize, u8) {
            let rank = match ev {
                NativeInstrumentEvent::Expression { .. }
                | NativeInstrumentEvent::ControlChange { .. }
                | NativeInstrumentEvent::PitchBend { .. } => 0,
                NativeInstrumentEvent::NoteOff { .. } => 1,
                NativeInstrumentEvent::NoteOn { .. } => 2,
                _ => 3,
            };
            (ev.frame_offset(), rank)
        };
        for i in 1..merged_len {
            let cur = merged[i];
            let key = sort_key(cur);
            let mut j = i;
            while j > 0 && sort_key(merged[j - 1]) > key {
                merged[j] = merged[j - 1];
                j -= 1;
            }
            merged[j] = cur;
        }

        if let Some(sink) = emit {
            let _ = sink.charge_work(work_units);
            for &ev in &merged[..merged_len] {
                let _ = sink.push(ev);
            }
        }
        Ok((state, merged_len))
    }
}

impl MutableMidiProcessorRuntime for NoteLengthMidiRuntime {
    fn preflight_block(
        &self,
        input: &[NativeInstrumentEvent],
        frame_count: usize,
        output_capacity: usize,
        clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<usize, NativeMidiProcessorError> {
        let (_next, written) =
            self.simulate_block(input, frame_count, output_capacity, clock, None)?;
        Ok(written)
    }

    fn process_block(
        &mut self,
        input: &[NativeInstrumentEvent],
        frame_count: usize,
        output: &mut MidiProcessorOutput<'_>,
        clock: Option<&dyn NativeInstrumentClock>,
    ) {
        let cap = output.remaining_capacity();
        if let Ok((next, _)) = self.simulate_block(input, frame_count, cap, clock, Some(output)) {
            *self = next;
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
        let active =
            Self::count_generated(&self.active_generated[..self.config.max_active_occurrences]);
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
        let mut ids = [0_u64; NOTE_LENGTH_MAX_ACTIVE_OCCURRENCES];
        let mut len = 0_usize;
        for slot in &mut self.active_generated[..active_cap] {
            if let Some(entry) = slot.take() {
                ids[len] = entry.occurrence_id;
                len += 1;
            }
        }
        self.active_inputs.fill(None);
        self.queue.reset();
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
        let active_cap = self.config.max_active_occurrences;
        Self::count_inputs(&self.active_inputs[..active_cap])
            .max(Self::count_generated(&self.active_generated[..active_cap]))
    }

    fn is_quiescent(&self) -> bool {
        self.active_occurrence_count() == 0 && self.queue.is_empty()
    }

    fn reset(&mut self) {
        self.active_inputs.fill(None);
        self.active_generated.fill(None);
        self.queue.reset();
        self.next_generated_id = GENERATED_OCCURRENCE_ID_BASE;
    }

    fn can_copy_history_from(&self, previous: &dyn MutableMidiProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|other| {
                other.active_occurrence_count() <= self.config.max_active_occurrences
                    && other.queue.pending_count() <= self.queue.capacity()
            })
    }

    fn copy_history_from(&mut self, previous: &dyn MutableMidiProcessorRuntime) {
        let Some(other) = previous.as_any().downcast_ref::<Self>() else {
            return;
        };
        self.active_inputs = other.active_inputs;
        self.active_generated = other.active_generated;
        self.queue = other.queue;
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

    struct LinearClock {
        start_frame: u64,
        sample_rate: u32,
        micro_bpm: u32,
    }

    impl NativeInstrumentClock for LinearClock {
        fn beat_at_offset(&self, offset: usize) -> Option<u64> {
            let frame = self.start_frame + offset as u64;
            let num = u128::from(frame) * u128::from(self.micro_bpm) * (1_u128 << 32);
            let den = u128::from(self.sample_rate) * 60_000_000;
            Some((num / den) as u64)
        }

        fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
            Some(self.start_frame + offset as u64)
        }

        fn transport_is_running(&self) -> Option<bool> {
            Some(true)
        }
    }

    fn test_limits() -> NativeMidiProcessorLimits {
        NativeMidiProcessorLimits {
            max_pair_bytes: 256 * 1024,
            max_input_events_per_block: NOTE_LENGTH_MAX_INPUT_EVENTS,
            max_output_events_per_block: NOTE_LENGTH_MAX_OUTPUT_EVENTS,
            max_active_occurrences: NOTE_LENGTH_MAX_ACTIVE_OCCURRENCES,
            max_work_units_per_block: NOTE_LENGTH_MAX_WORK_UNITS,
        }
    }

    #[test]
    fn note_length_state_codec_and_validation() {
        let default = NOTE_LENGTH_DEFINITION.default_state().unwrap();
        assert_eq!(
            NoteLengthSettings::decode(&default).unwrap(),
            NoteLengthSettings::default()
        );

        let patched = NOTE_LENGTH_DEFINITION
            .patch_parameter(&default, parameter::TRIGGER, ParameterValue::Choice(1))
            .unwrap();
        let patched = NOTE_LENGTH_DEFINITION
            .patch_parameter(&patched, parameter::MODE, ParameterValue::Choice(0))
            .unwrap();
        let patched = NOTE_LENGTH_DEFINITION
            .patch_parameter(&patched, parameter::TIME_MS, ParameterValue::Float(125.0))
            .unwrap();
        let patched = NOTE_LENGTH_DEFINITION
            .patch_parameter(&patched, parameter::SYNC_RATE, ParameterValue::Choice(4))
            .unwrap();
        let patched = NOTE_LENGTH_DEFINITION
            .patch_parameter(&patched, parameter::GATE, ParameterValue::Float(50.0))
            .unwrap();
        let patched = NOTE_LENGTH_DEFINITION
            .patch_parameter(&patched, parameter::RETRIGGER, ParameterValue::Choice(1))
            .unwrap();

        let decoded = NoteLengthSettings::decode(&patched).unwrap();
        assert_eq!(
            decoded,
            NoteLengthSettings {
                trigger: NoteLengthTrigger::NoteOff,
                mode: NoteLengthMode::Time,
                time_ms: 125,
                sync_rate: 4,
                gate_percent: 50,
                retrigger: NoteLengthRetrigger::Cut,
            }
        );
        // 125 ms * 50% = 62.5 ms = 3000 frames @ 48 kHz.
        assert_eq!(decoded.time_duration_frames(48_000), 3_000);
        // 1/4 note (1 beat) * 50% = 1/2 beat.
        assert_eq!(decoded.sync_duration_beats_q32(), 1_u64 << 31);

        // Reject out-of-range / NaN / malformed payloads.
        assert!(
            NOTE_LENGTH_DEFINITION
                .patch_parameter(&default, parameter::TIME_MS, ParameterValue::Float(0.0))
                .is_err()
        );
        assert!(
            NOTE_LENGTH_DEFINITION
                .patch_parameter(&default, parameter::GATE, ParameterValue::Float(201.0))
                .is_err()
        );
        assert!(
            NOTE_LENGTH_DEFINITION
                .patch_parameter(&default, parameter::GATE, ParameterValue::Float(f32::NAN))
                .is_err()
        );
        assert!(
            NOTE_LENGTH_DEFINITION
                .patch_parameter(&default, parameter::SYNC_RATE, ParameterValue::Choice(8))
                .is_err()
        );
        let malformed = NativeDeviceState::new(NOTE_LENGTH_TYPE_ID, 1, vec![0, 1]).unwrap();
        assert_eq!(
            NoteLengthSettings::decode(&malformed),
            Err(DeviceContractError::MalformedState)
        );
    }

    #[test]
    fn retrigger_cut_and_note_off_trigger_preserve_occurrence_custody() {
        let cut_settings = NoteLengthSettings {
            trigger: NoteLengthTrigger::NoteOn,
            mode: NoteLengthMode::Time,
            time_ms: 10, // 480 frames @ 48 kHz
            gate_percent: 100,
            retrigger: NoteLengthRetrigger::Cut,
            ..Default::default()
        };
        let prep = PreparedNoteLengthMidiProcessor::new(cut_settings, 48_000).unwrap();
        let mut pair = NativeMidiProcessorRuntimePair::new(&prep, test_limits()).unwrap();
        let mut out_buf = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; 16];

        let clock0 = LinearClock {
            start_frame: 0,
            sample_rate: 48_000,
            micro_bpm: 120_000_000,
        };
        // Two same-pitch C4 notes at offset 0 and offset 100 -> second cuts the first!
        let batch = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 60,
                velocity: 100,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 100,
                occurrence_id: 2,
                pitch: 60,
                velocity: 90,
            },
        ];
        pair.begin_block();
        let n = pair
            .process_block(&batch, 256, &mut out_buf, Some(&clock0))
            .unwrap();
        pair.commit_block();
        assert_eq!(n, 3);
        assert!(matches!(
            out_buf[0],
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id,
                pitch: 60,
                velocity: 100
            } if occurrence_id == GENERATED_OCCURRENCE_ID_BASE
        ));
        // At offset 100, Cut releases first generated occurrence BEFORE emitting second!
        assert!(matches!(
            out_buf[1],
            NativeInstrumentEvent::NoteOff {
                frame_offset: 100,
                occurrence_id
            } if occurrence_id == GENERATED_OCCURRENCE_ID_BASE
        ));
        assert!(matches!(
            out_buf[2],
            NativeInstrumentEvent::NoteOn {
                frame_offset: 100,
                occurrence_id,
                pitch: 60,
                velocity: 90
            } if occurrence_id == GENERATED_OCCURRENCE_ID_BASE + 1
        ));

        // Second note finishes at frame 100 + 480 = 580 -> block [256..640) emits NoteOff at offset 324.
        let clock1 = LinearClock {
            start_frame: 256,
            sample_rate: 48_000,
            micro_bpm: 120_000_000,
        };
        pair.begin_block();
        let n2 = pair
            .process_block(
                &[
                    NativeInstrumentEvent::NoteOff {
                        frame_offset: 0,
                        occurrence_id: 1,
                    },
                    NativeInstrumentEvent::NoteOff {
                        frame_offset: 10,
                        occurrence_id: 2,
                    },
                ],
                384,
                &mut out_buf,
                Some(&clock1),
            )
            .unwrap();
        pair.commit_block();
        assert_eq!(n2, 1);
        assert!(matches!(
            out_buf[0],
            NativeInstrumentEvent::NoteOff {
                frame_offset: 324,
                occurrence_id
            } if occurrence_id == GENERATED_OCCURRENCE_ID_BASE + 1
        ));
        assert!(pair.committed_is_quiescent());
    }
}
