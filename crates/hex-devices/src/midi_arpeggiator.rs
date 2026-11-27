//! Original bounded native MIDI Arpeggiator using the `hex-lattice` MIDI
//! processor and bounded future-event horizon contracts (`hex.midi.arpeggiator`).
//!
//! Arpeggiates currently held input notes using only the host's borrowed
//! [`NativeInstrumentClock`] and an owned transactional
//! [`BoundedFutureMidiQueue`] — never a second project clock or callback
//! allocation. Supports finite `style` (`up`, `down`, `up down`, `down up`,
//! `order`), musical `rate` (`1/64..=1/2`), `gate` (`1..=100%`), octave/step
//! repeats `steps` (`1..=4`) with `distance` (`-24..=+24 st`), and `retrigger`
//! (`off`/`note`). Guarantees block-partition invariance across tempo changes,
//! clean live mode edits while held, and zero stuck notes on Stop, Seek, loop
//! wrap, bypass, or queue saturation.

use hex_lattice::{
    BoundedFutureMidiQueue, DueMidiEvent, FUTURE_MIDI_EVENT_QUEUE_CAPACITY,
    FUTURE_MIDI_RESERVED_NOTE_OFF_SLOTS, FutureMidiTime, GENERATED_OCCURRENCE_ID_BASE,
    MidiProcessorOutput, MutableMidiProcessorRuntime, NativeInstrumentClock, NativeInstrumentEvent,
    NativeMidiProcessorDefinition, NativeMidiProcessorError, PreparedNativeMidiProcessor,
    ScheduledMidiKind, resolve_future_time_in_block,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;

pub const ARPEGGIATOR_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.midi.arpeggiator");
pub const ARPEGGIATOR_STATE_VERSION: u32 = 1;
pub const ARPEGGIATOR_RUNTIME_VERSION: u32 = 1;

pub const MIN_GATE_PERCENT: u8 = 1;
pub const MAX_GATE_PERCENT: u8 = 100;
pub const MIN_STEPS: u8 = 1;
pub const MAX_STEPS: u8 = 4;
pub const MIN_DISTANCE: i8 = -24;
pub const MAX_DISTANCE: i8 = 24;
pub const MAX_MIDI_NOTE: u8 = 127;

pub const ARPEGGIATOR_MAX_INPUT_EVENTS: usize = 259;
pub const ARPEGGIATOR_MAX_OUTPUT_EVENTS: usize = 259;
pub const ARPEGGIATOR_MAX_ACTIVE_OCCURRENCES: usize = 64;
pub const ARPEGGIATOR_MAX_WORK_UNITS: usize = ARPEGGIATOR_MAX_INPUT_EVENTS * 4;
const MAX_GENERATED_STEPS_PER_BLOCK: usize = 64;
const MAX_PATTERN_CANDIDATES: usize = ARPEGGIATOR_MAX_ACTIVE_OCCURRENCES * (MAX_STEPS as usize);

/// `[style, rate, gate_percent, steps, distance_i8 as u8, retrigger]`
const STATE_BYTES: usize = 6;

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const STYLE: ParameterId = ParameterId::from_static(1);
    pub const RATE: ParameterId = ParameterId::from_static(2);
    pub const GATE: ParameterId = ParameterId::from_static(3);
    pub const STEPS: ParameterId = ParameterId::from_static(4);
    pub const DISTANCE: ParameterId = ParameterId::from_static(5);
    pub const RETRIGGER: ParameterId = ParameterId::from_static(6);
}

const STYLE_LABELS: [&str; 5] = ["up", "down", "up down", "down up", "order"];
const RATE_LABELS: [&str; 6] = ["1/64", "1/32", "1/16", "1/8", "1/4", "1/2"];
const RETRIGGER_LABELS: [&str; 2] = ["off", "note"];

/// Q32.32 quarter-note beat durations per arpeggio step.
const RATE_BEATS_Q32: [u64; 6] = [
    1_u64 << 28, // 1/64 note = 1/16 quarter-note beat
    1_u64 << 29, // 1/32 note = 1/8 beat
    1_u64 << 30, // 1/16 note = 1/4 beat
    1_u64 << 31, // 1/8 note  = 1/2 beat
    1_u64 << 32, // 1/4 note  = 1 beat
    2_u64 << 32, // 1/2 note  = 2 beats
];

const DESCRIPTORS: [ParameterDescriptor; 6] = [
    ParameterDescriptor {
        id: parameter::STYLE,
        label: "style",
        kind: ParameterKind::Choice {
            labels: &STYLE_LABELS,
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::RATE,
        label: "rate",
        kind: ParameterKind::Choice {
            labels: &RATE_LABELS,
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
        default: ParameterValue::Float(50.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::STEPS,
        label: "steps",
        kind: ParameterKind::Float {
            min: MIN_STEPS as f32,
            max: MAX_STEPS as f32,
            unit: None,
        },
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::DISTANCE,
        label: "distance",
        kind: ParameterKind::Float {
            min: MIN_DISTANCE as f32,
            max: MAX_DISTANCE as f32,
            unit: Some("st"),
        },
        default: ParameterValue::Float(12.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::RETRIGGER,
        label: "retrigger",
        kind: ParameterKind::Choice {
            labels: &RETRIGGER_LABELS,
        },
        default: ParameterValue::Choice(1),
        presentation: ParameterPresentation::Linear,
    },
];

/// Traversal order over the currently held note set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ArpeggiatorStyle {
    Up = 0,
    Down = 1,
    UpDown = 2,
    DownUp = 3,
    Order = 4,
}

impl ArpeggiatorStyle {
    pub const fn from_choice(choice: u32) -> Option<Self> {
        match choice {
            0 => Some(Self::Up),
            1 => Some(Self::Down),
            2 => Some(Self::UpDown),
            3 => Some(Self::DownUp),
            4 => Some(Self::Order),
            _ => None,
        }
    }

    pub const fn as_choice(self) -> u32 {
        self as u32
    }
}

/// Reset behavior when the held note set transitions from empty to non-empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ArpeggiatorRetrigger {
    Off = 0,
    Note = 1,
}

impl ArpeggiatorRetrigger {
    pub const fn from_choice(choice: u32) -> Option<Self> {
        match choice {
            0 => Some(Self::Off),
            1 => Some(Self::Note),
            _ => None,
        }
    }

    pub const fn as_choice(self) -> u32 {
        self as u32
    }
}

/// Validated authored settings for `hex.midi.arpeggiator`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArpeggiatorSettings {
    pub style: ArpeggiatorStyle,
    /// Index into `RATE_BEATS_Q32` (`0..6`).
    pub rate: u8,
    /// Step gate percentage (`1..=100`).
    pub gate_percent: u8,
    /// Number of transposition passes (`1..=4`).
    pub steps: u8,
    /// Semitone shift per repeat pass (`-24..=+24`).
    pub distance: i8,
    pub retrigger: ArpeggiatorRetrigger,
}

impl Default for ArpeggiatorSettings {
    fn default() -> Self {
        Self {
            style: ArpeggiatorStyle::Up,
            rate: 2,
            gate_percent: 50,
            steps: 1,
            distance: 12,
            retrigger: ArpeggiatorRetrigger::Note,
        }
    }
}

impl ArpeggiatorSettings {
    pub fn encode(self) -> Result<NativeDeviceState, DeviceContractError> {
        self.validate()?;
        let payload = vec![
            self.style as u8,
            self.rate,
            self.gate_percent,
            self.steps,
            self.distance as u8,
            self.retrigger as u8,
        ];
        NativeDeviceState::new(ARPEGGIATOR_TYPE_ID, ARPEGGIATOR_STATE_VERSION, payload)
    }

    pub fn decode(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != ARPEGGIATOR_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != ARPEGGIATOR_STATE_VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        if !state.asset_references().is_empty() {
            return Err(DeviceContractError::AssetResourcesUnavailable);
        }
        let payload = state.payload();
        if payload.len() != STATE_BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let style = ArpeggiatorStyle::from_choice(u32::from(payload[0]))
            .ok_or(DeviceContractError::MalformedState)?;
        let retrigger = ArpeggiatorRetrigger::from_choice(u32::from(payload[5]))
            .ok_or(DeviceContractError::MalformedState)?;
        let settings = Self {
            style,
            rate: payload[1],
            gate_percent: payload[2],
            steps: payload[3],
            distance: payload[4] as i8,
            retrigger,
        };
        settings.validate()?;
        Ok(settings)
    }

    fn validate(self) -> Result<(), DeviceContractError> {
        if usize::from(self.rate) >= RATE_BEATS_Q32.len()
            || !(MIN_GATE_PERCENT..=MAX_GATE_PERCENT).contains(&self.gate_percent)
            || !(MIN_STEPS..=MAX_STEPS).contains(&self.steps)
            || !(MIN_DISTANCE..=MAX_DISTANCE).contains(&self.distance)
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(())
    }

    pub const fn step_beats_q32(self) -> u64 {
        RATE_BEATS_Q32[self.rate as usize]
    }

    pub const fn gate_beats_q32(self) -> u64 {
        let step = self.step_beats_q32() as u128;
        let scaled = (step * (self.gate_percent as u128)) / 100;
        if scaled == 0 { 1 } else { scaled as u64 }
    }
}

pub static ARPEGGIATOR_DEFINITION: ArpeggiatorMidiProcessorDefinition =
    ArpeggiatorMidiProcessorDefinition;

pub struct ArpeggiatorMidiProcessorDefinition;

impl NativeDeviceMetadata for ArpeggiatorMidiProcessorDefinition {
    fn type_id(&self) -> DeviceTypeId {
        ARPEGGIATOR_TYPE_ID
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("arpeggiator")
    }

    fn state_version(&self) -> u32 {
        ARPEGGIATOR_STATE_VERSION
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &DESCRIPTORS
    }

    fn ports(&self) -> &'static [hex_lattice_contract::DevicePortDescriptor] {
        hex_lattice_contract::MIDI_EFFECT_PORTS
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        ArpeggiatorSettings::default().encode()
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let settings = ArpeggiatorSettings::decode(state)?;
        settings.encode()?.with_host_metadata_from(state)
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let settings = ArpeggiatorSettings::decode(state)?;
        match parameter {
            parameter::STYLE => Ok(ParameterValue::Choice(settings.style.as_choice())),
            parameter::RATE => Ok(ParameterValue::Choice(u32::from(settings.rate))),
            parameter::GATE => Ok(ParameterValue::Float(f32::from(settings.gate_percent))),
            parameter::STEPS => Ok(ParameterValue::Float(f32::from(settings.steps))),
            parameter::DISTANCE => Ok(ParameterValue::Float(f32::from(settings.distance))),
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
        let mut settings = ArpeggiatorSettings::decode(state)?;
        match (parameter, value) {
            (parameter::STYLE, ParameterValue::Choice(choice)) => {
                settings.style = ArpeggiatorStyle::from_choice(choice)
                    .ok_or(DeviceContractError::InvalidParameterValue)?;
            }
            (parameter::RATE, ParameterValue::Choice(choice)) => {
                if choice as usize >= RATE_BEATS_Q32.len() {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.rate = choice as u8;
            }
            (parameter::GATE, ParameterValue::Float(v)) if v.is_finite() => {
                let r = v.round();
                if r < f32::from(MIN_GATE_PERCENT) || r > f32::from(MAX_GATE_PERCENT) {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.gate_percent = r as u8;
            }
            (parameter::STEPS, ParameterValue::Float(v)) if v.is_finite() => {
                let r = v.round();
                if r < f32::from(MIN_STEPS) || r > f32::from(MAX_STEPS) {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.steps = r as u8;
            }
            (parameter::DISTANCE, ParameterValue::Float(v)) if v.is_finite() => {
                let r = v.round();
                if r < f32::from(MIN_DISTANCE) || r > f32::from(MAX_DISTANCE) {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.distance = r as i8;
            }
            (parameter::RETRIGGER, ParameterValue::Choice(choice)) => {
                settings.retrigger = ArpeggiatorRetrigger::from_choice(choice)
                    .ok_or(DeviceContractError::InvalidParameterValue)?;
            }
            (
                parameter::STYLE
                | parameter::RATE
                | parameter::GATE
                | parameter::STEPS
                | parameter::DISTANCE
                | parameter::RETRIGGER,
                _,
            ) => return Err(DeviceContractError::InvalidParameterValue),
            _ => return Err(DeviceContractError::UnknownParameter),
        }
        settings.encode()?.with_host_metadata_from(state)
    }
}

impl NativeMidiProcessorDefinition for ArpeggiatorMidiProcessorDefinition {
    fn prepare_midi_processor(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Box<dyn PreparedNativeMidiProcessor>, DeviceContractError> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let settings = ArpeggiatorSettings::decode(state)?;
        Ok(Box::new(PreparedArpeggiatorMidiProcessor::new(settings)?))
    }
}

/// Control-prepared immutable factory for `hex.midi.arpeggiator`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedArpeggiatorMidiProcessor {
    settings: ArpeggiatorSettings,
    max_input_events: usize,
    max_output_events: usize,
    max_active_occurrences: usize,
    max_work_units: usize,
}

impl PreparedArpeggiatorMidiProcessor {
    pub fn new(settings: ArpeggiatorSettings) -> Result<Self, DeviceContractError> {
        Self::with_caps(
            settings,
            ARPEGGIATOR_MAX_INPUT_EVENTS,
            ARPEGGIATOR_MAX_OUTPUT_EVENTS,
            ARPEGGIATOR_MAX_ACTIVE_OCCURRENCES,
            ARPEGGIATOR_MAX_WORK_UNITS,
        )
    }

    pub fn with_caps(
        settings: ArpeggiatorSettings,
        max_input_events: usize,
        max_output_events: usize,
        max_active_occurrences: usize,
        max_work_units: usize,
    ) -> Result<Self, DeviceContractError> {
        settings.validate()?;
        if max_input_events == 0
            || max_output_events == 0
            || max_active_occurrences == 0
            || max_active_occurrences > ARPEGGIATOR_MAX_ACTIVE_OCCURRENCES
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

    pub const fn settings(self) -> ArpeggiatorSettings {
        self.settings
    }
}

impl PreparedNativeMidiProcessor for PreparedArpeggiatorMidiProcessor {
    fn runtime_version(&self) -> u32 {
        ARPEGGIATOR_RUNTIME_VERSION
    }

    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<ArpeggiatorMidiRuntime>()
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
        Box::new(ArpeggiatorMidiRuntime {
            config: *self,
            queue,
            next_generated_id: GENERATED_OCCURRENCE_ID_BASE,
            next_order_seq: 1,
            next_step_beat: None,
            step_index: 0,
            held: [None; ARPEGGIATOR_MAX_ACTIVE_OCCURRENCES],
            active_generated: [None; ARPEGGIATOR_MAX_ACTIVE_OCCURRENCES],
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HeldArpNote {
    occurrence_id: u64,
    pitch: u8,
    velocity: u8,
    order_seq: u64,
}

#[derive(Clone)]
struct ArpeggiatorMidiRuntime {
    config: PreparedArpeggiatorMidiProcessor,
    queue: BoundedFutureMidiQueue,
    next_generated_id: u64,
    next_order_seq: u64,
    next_step_beat: Option<u64>,
    step_index: u32,
    held: [Option<HeldArpNote>; ARPEGGIATOR_MAX_ACTIVE_OCCURRENCES],
    active_generated: [Option<u64>; ARPEGGIATOR_MAX_ACTIVE_OCCURRENCES],
}

impl ArpeggiatorMidiRuntime {
    fn find_held(slots: &[Option<HeldArpNote>], id: u64) -> Option<usize> {
        slots
            .iter()
            .position(|slot| slot.is_some_and(|entry| entry.occurrence_id == id))
    }

    fn find_free_held(slots: &[Option<HeldArpNote>]) -> Option<usize> {
        slots.iter().position(Option::is_none)
    }

    fn count_held(slots: &[Option<HeldArpNote>]) -> usize {
        slots.iter().flatten().count()
    }

    fn find_generated(slots: &[Option<u64>], id: u64) -> Option<usize> {
        slots.iter().position(|slot| *slot == Some(id))
    }

    fn find_free_generated(slots: &[Option<u64>]) -> Option<usize> {
        slots.iter().position(Option::is_none)
    }

    fn count_generated(slots: &[Option<u64>]) -> usize {
        slots.iter().flatten().count()
    }

    /// Select the `(pitch, velocity)` for `self.step_index` from currently held
    /// notes under `self.config.settings`, without heap allocation.
    fn pick_step_note(&self) -> Option<(u8, u8)> {
        let active_cap = self.config.max_active_occurrences;
        let mut base = [HeldArpNote {
            occurrence_id: 0,
            pitch: 0,
            velocity: 1,
            order_seq: 0,
        }; ARPEGGIATOR_MAX_ACTIVE_OCCURRENCES];
        let mut base_len = 0_usize;
        for &entry in self.held[..active_cap].iter().flatten() {
            base[base_len] = entry;
            base_len += 1;
        }
        if base_len == 0 {
            return None;
        }
        let settings = self.config.settings;
        match settings.style {
            ArpeggiatorStyle::Up | ArpeggiatorStyle::UpDown => {
                base[..base_len].sort_unstable_by_key(|n| (n.pitch, n.order_seq));
            }
            ArpeggiatorStyle::Down | ArpeggiatorStyle::DownUp => {
                base[..base_len].sort_unstable_by_key(|n| (u8::MAX - n.pitch, n.order_seq));
            }
            ArpeggiatorStyle::Order => {
                base[..base_len].sort_unstable_by_key(|n| n.order_seq);
            }
        }

        // Expand across `steps` repeats with `distance` semitones per repeat.
        let mut expanded = [(0_u8, 1_u8); MAX_PATTERN_CANDIDATES];
        let mut exp_len = 0_usize;
        for repeat in 0..usize::from(settings.steps) {
            let shift = (repeat as i16) * i16::from(settings.distance);
            for note in &base[..base_len] {
                let p = i16::from(note.pitch) + shift;
                if (0..=i16::from(MAX_MIDI_NOTE)).contains(&p) && exp_len < MAX_PATTERN_CANDIDATES {
                    expanded[exp_len] = (p as u8, note.velocity);
                    exp_len += 1;
                }
            }
        }
        if exp_len == 0 {
            return None;
        }

        let idx = match settings.style {
            ArpeggiatorStyle::UpDown | ArpeggiatorStyle::DownUp if exp_len >= 3 => {
                let period = 2 * (exp_len - 1);
                let pos = (self.step_index as usize) % period;
                if pos < exp_len { pos } else { period - pos }
            }
            _ => (self.step_index as usize) % exp_len,
        };
        Some(expanded[idx])
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
        }; ARPEGGIATOR_MAX_OUTPUT_EVENTS];
        let mut merged_len = 0_usize;

        let mut in_idx = 0_usize;
        let mut steps_emitted = 0_usize;

        loop {
            let next_in_off = input.get(in_idx).map(|e| e.frame_offset());
            let next_step_off = if Self::count_held(&state.held[..active_cap]) > 0
                && let Some(step_beat) = state.next_step_beat
            {
                resolve_future_time_in_block(
                    FutureMidiTime::BeatQ32(step_beat),
                    frame_count,
                    clock,
                )?
            } else {
                None
            };

            let consume_input = match (next_in_off, next_step_off) {
                (Some(io), Some(so)) => io <= so,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => break,
            };

            if consume_input {
                let event = input[in_idx];
                in_idx += 1;
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
                        if Self::find_held(&state.held[..active_cap], occurrence_id).is_some() {
                            return Err(NativeMidiProcessorError::DuplicateOccurrence);
                        }
                        let was_empty = Self::count_held(&state.held[..active_cap]) == 0;
                        let free = Self::find_free_held(&state.held[..active_cap])
                            .ok_or(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded)?;
                        let order_seq = state.next_order_seq;
                        state.next_order_seq = order_seq.wrapping_add(1);
                        state.held[free] = Some(HeldArpNote {
                            occurrence_id,
                            pitch,
                            velocity,
                            order_seq,
                        });
                        if was_empty {
                            if state.config.settings.retrigger == ArpeggiatorRetrigger::Note {
                                state.step_index = 0;
                            }
                            let Some(clock_ref) = clock else {
                                return Err(NativeMidiProcessorError::ClockUnavailable);
                            };
                            let now_beat = clock_ref
                                .beat_at_offset(frame_offset)
                                .ok_or(NativeMidiProcessorError::ClockUnavailable)?;
                            state.next_step_beat = Some(now_beat);
                        }
                    }
                    NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                        let slot = Self::find_held(&state.held[..active_cap], occurrence_id)
                            .ok_or(NativeMidiProcessorError::OrphanedNoteOff)?;
                        state.held[slot] = None;
                        if Self::count_held(&state.held[..active_cap]) == 0 {
                            state.next_step_beat = None;
                        }
                    }
                    NativeInstrumentEvent::NotePressure { occurrence_id, .. }
                    | NativeInstrumentEvent::NotePitchBend { occurrence_id, .. }
                    | NativeInstrumentEvent::NoteTimbre { occurrence_id, .. } => {
                        if Self::find_held(&state.held[..active_cap], occurrence_id.get()).is_none()
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
            } else if let (Some(step_off), Some(step_beat)) = (next_step_off, state.next_step_beat)
            {
                if steps_emitted >= MAX_GENERATED_STEPS_PER_BLOCK {
                    return Err(NativeMidiProcessorError::WorkBudgetExceeded);
                }
                steps_emitted += 1;
                work_units = work_units
                    .checked_add(2)
                    .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
                if let Some((pitch, velocity)) = state.pick_step_note() {
                    let gate_due = step_beat
                        .checked_add(state.config.settings.gate_beats_q32())
                        .ok_or(NativeMidiProcessorError::FutureEventHorizonExceeded)?;
                    let gen_id = state.next_generated_id;
                    state.next_generated_id = gen_id
                        .checked_add(1)
                        .ok_or(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded)?;
                    let free = Self::find_free_generated(&state.active_generated[..active_cap])
                        .ok_or(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded)?;
                    if merged_len >= out_cap {
                        return Err(NativeMidiProcessorError::OutputEventCapacityExceeded);
                    }
                    state.queue.schedule(
                        FutureMidiTime::BeatQ32(gate_due),
                        ScheduledMidiKind::NoteOff {
                            occurrence_id: gen_id,
                        },
                        clock,
                    )?;
                    state.active_generated[free] = Some(gen_id);
                    merged[merged_len] = NativeInstrumentEvent::NoteOn {
                        frame_offset: step_off,
                        occurrence_id: gen_id,
                        pitch,
                        velocity,
                    };
                    merged_len += 1;
                }
                state.step_index = state.step_index.wrapping_add(1);
                let next_beat = step_beat
                    .checked_add(state.config.settings.step_beats_q32())
                    .ok_or(NativeMidiProcessorError::FutureEventHorizonExceeded)?;
                state.next_step_beat = Some(next_beat);
            }

            if work_units > state.config.max_work_units {
                return Err(NativeMidiProcessorError::WorkBudgetExceeded);
            }
        }

        // Drain all scheduled generated NoteOffs due inside `[0, frame_count)`.
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

impl MutableMidiProcessorRuntime for ArpeggiatorMidiRuntime {
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
        let mut ids = [0_u64; ARPEGGIATOR_MAX_ACTIVE_OCCURRENCES];
        let mut len = 0_usize;
        for slot in &mut self.active_generated[..active_cap] {
            if let Some(id) = slot.take() {
                ids[len] = id;
                len += 1;
            }
        }
        self.held.fill(None);
        self.next_step_beat = None;
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
        Self::count_held(&self.held[..active_cap])
            .max(Self::count_generated(&self.active_generated[..active_cap]))
    }

    fn is_quiescent(&self) -> bool {
        self.active_occurrence_count() == 0 && self.queue.is_empty()
    }

    fn reset(&mut self) {
        self.held.fill(None);
        self.active_generated.fill(None);
        self.queue.reset();
        self.next_step_beat = None;
        self.step_index = 0;
        self.next_order_seq = 1;
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
        self.held = other.held;
        self.active_generated = other.active_generated;
        self.queue = other.queue;
        self.next_step_beat = other.next_step_beat;
        self.step_index = other.step_index;
        self.next_order_seq = other.next_order_seq;
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
            max_input_events_per_block: ARPEGGIATOR_MAX_INPUT_EVENTS,
            max_output_events_per_block: ARPEGGIATOR_MAX_OUTPUT_EVENTS,
            max_active_occurrences: ARPEGGIATOR_MAX_ACTIVE_OCCURRENCES,
            max_work_units_per_block: ARPEGGIATOR_MAX_WORK_UNITS,
        }
    }

    #[test]
    fn arpeggiator_state_codec_and_validation() {
        let default = ARPEGGIATOR_DEFINITION.default_state().unwrap();
        assert_eq!(
            ArpeggiatorSettings::decode(&default).unwrap(),
            ArpeggiatorSettings::default()
        );

        let patched = ARPEGGIATOR_DEFINITION
            .patch_parameter(&default, parameter::STYLE, ParameterValue::Choice(2))
            .unwrap();
        let patched = ARPEGGIATOR_DEFINITION
            .patch_parameter(&patched, parameter::RATE, ParameterValue::Choice(0))
            .unwrap();
        let patched = ARPEGGIATOR_DEFINITION
            .patch_parameter(&patched, parameter::GATE, ParameterValue::Float(75.0))
            .unwrap();
        let patched = ARPEGGIATOR_DEFINITION
            .patch_parameter(&patched, parameter::STEPS, ParameterValue::Float(2.0))
            .unwrap();
        let patched = ARPEGGIATOR_DEFINITION
            .patch_parameter(&patched, parameter::DISTANCE, ParameterValue::Float(-12.0))
            .unwrap();
        let patched = ARPEGGIATOR_DEFINITION
            .patch_parameter(&patched, parameter::RETRIGGER, ParameterValue::Choice(0))
            .unwrap();

        let decoded = ArpeggiatorSettings::decode(&patched).unwrap();
        assert_eq!(
            decoded,
            ArpeggiatorSettings {
                style: ArpeggiatorStyle::UpDown,
                rate: 0,
                gate_percent: 75,
                steps: 2,
                distance: -12,
                retrigger: ArpeggiatorRetrigger::Off,
            }
        );

        assert!(
            ARPEGGIATOR_DEFINITION
                .patch_parameter(&default, parameter::GATE, ParameterValue::Float(101.0))
                .is_err()
        );
        assert!(
            ARPEGGIATOR_DEFINITION
                .patch_parameter(&default, parameter::STEPS, ParameterValue::Float(5.0))
                .is_err()
        );
        assert!(
            ARPEGGIATOR_DEFINITION
                .patch_parameter(&default, parameter::DISTANCE, ParameterValue::Float(25.0))
                .is_err()
        );
        assert!(
            ARPEGGIATOR_DEFINITION
                .patch_parameter(&default, parameter::STYLE, ParameterValue::Choice(5))
                .is_err()
        );
        let malformed = NativeDeviceState::new(ARPEGGIATOR_TYPE_ID, 1, vec![0, 1]).unwrap();
        assert_eq!(
            ArpeggiatorSettings::decode(&malformed),
            Err(DeviceContractError::MalformedState)
        );
    }

    #[test]
    fn arpeggiator_styles_octave_steps_and_100_percent_gate_note_off_priority() {
        // 1/64 note = 1/16 beat = 1500 frames @ 120 BPM / 48 kHz.
        // Gate = 100% -> NoteOff for step 0 lands at exact same frame (1500) as step 1 NoteOn,
        // and NoteOff MUST precede NoteOn!
        let settings = ArpeggiatorSettings {
            style: ArpeggiatorStyle::UpDown,
            rate: 0,
            gate_percent: 100,
            steps: 1,
            distance: 12,
            retrigger: ArpeggiatorRetrigger::Note,
        };
        let prep = PreparedArpeggiatorMidiProcessor::new(settings).unwrap();
        let mut pair = NativeMidiProcessorRuntimePair::new(&prep, test_limits()).unwrap();
        let mut out_buf = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; 64];

        // Hold C4, E4, G4 for 4 steps (6000 frames) -> UpDown produces 60, 64, 67, 64.
        let chord_on = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 67,
                velocity: 100,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 2,
                pitch: 60,
                velocity: 100,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 3,
                pitch: 64,
                velocity: 100,
            },
        ];
        let clock0 = LinearClock {
            start_frame: 0,
            sample_rate: 48_000,
            micro_bpm: 120_000_000,
        };
        pair.begin_block();
        let n = pair
            .process_block(&chord_on, 5_900, &mut out_buf, Some(&clock0))
            .unwrap();
        pair.commit_block();

        let pitches: Vec<u8> = out_buf[..n]
            .iter()
            .filter_map(|e| match *e {
                NativeInstrumentEvent::NoteOn { pitch, .. } => Some(pitch),
                _ => None,
            })
            .collect();
        assert_eq!(pitches, vec![60, 64, 67, 64]);

        // Verify NoteOff strictly precedes NoteOn at offset 1500!
        let at_1500: Vec<_> = out_buf[..n]
            .iter()
            .copied()
            .filter(|e| e.frame_offset() == 1_500)
            .collect();
        assert_eq!(at_1500.len(), 2);
        assert!(matches!(at_1500[0], NativeInstrumentEvent::NoteOff { .. }));
        assert!(matches!(
            at_1500[1],
            NativeInstrumentEvent::NoteOn { pitch: 64, .. }
        ));

        // Release chord at frame 6000 -> step 3 NoteOff drains cleanly -> quiescent!
        let chord_off = [
            NativeInstrumentEvent::NoteOff {
                frame_offset: 100,
                occurrence_id: 1,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 100,
                occurrence_id: 2,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 100,
                occurrence_id: 3,
            },
        ];
        let clock1 = LinearClock {
            start_frame: 5_900,
            sample_rate: 48_000,
            micro_bpm: 120_000_000,
        };
        pair.begin_block();
        let n_off = pair
            .process_block(&chord_off, 256, &mut out_buf, Some(&clock1))
            .unwrap();
        pair.commit_block();
        assert_eq!(n_off, 1);
        assert!(matches!(
            out_buf[0],
            NativeInstrumentEvent::NoteOff {
                frame_offset: 100,
                ..
            }
        ));
        assert!(pair.committed_is_quiescent());
    }
}
