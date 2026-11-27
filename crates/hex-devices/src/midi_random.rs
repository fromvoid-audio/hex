//! Original deterministic native MIDI Random pitch processor using the
//! bounded `hex-lattice` MIDI processor contract (`hex.midi.random`).
//!
//! For each incoming `NoteOn` in host event order, draws from a deterministic
//! integer SplitMix64 stream seeded from authored `seed`. With explicit finite
//! `chance` (`0..=100%`, stored at 0.01% resolution), `choices` (`1..=24`),
//! `scale` (`1..=24 st`), `sign` (`add`, `sub`, `bi`), and `overflow` policy
//! (`clamp`, `drop`, `fold`), computes a shifted pitch while tracking each
//! occurrence by `occurrence_id` so every held note releases cleanly across
//! seed, parameter, and bypass edits. `reset()` deterministically reseeds the
//! PRNG to its authored `seed` without owning any project clock.

use hex_lattice::{
    MidiProcessorOutput, MutableMidiProcessorRuntime, NativeInstrumentClock, NativeInstrumentEvent,
    NativeMidiProcessorDefinition, NativeMidiProcessorError, PreparedNativeMidiProcessor,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;

pub const RANDOM_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.midi.random");
pub const RANDOM_STATE_VERSION: u32 = 1;
pub const RANDOM_RUNTIME_VERSION: u32 = 1;

pub const MAX_CHANCE_BPS: u16 = 10_000;
pub const MIN_CHOICES: u8 = 1;
pub const MAX_CHOICES: u8 = 24;
pub const MIN_SCALE: u8 = 1;
pub const MAX_SCALE: u8 = 24;
pub const MAX_MIDI_NOTE: u8 = 127;

pub const RANDOM_MAX_INPUT_EVENTS: usize = 259;
pub const RANDOM_MAX_OUTPUT_EVENTS: usize = 259;
pub const RANDOM_MAX_ACTIVE_OCCURRENCES: usize = 64;
pub const RANDOM_MAX_WORK_UNITS: usize = RANDOM_MAX_INPUT_EVENTS * 2;

/// `[chance_bps_le(2), choices, scale, sign, seed_le(2), overflow]`
const STATE_BYTES: usize = 8;

const SEED_DOMAIN_TAG: u64 = 0x5241_4E44_4F4D_5F31;

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const CHANCE: ParameterId = ParameterId::from_static(1);
    pub const CHOICES: ParameterId = ParameterId::from_static(2);
    pub const SCALE: ParameterId = ParameterId::from_static(3);
    pub const SIGN: ParameterId = ParameterId::from_static(4);
    pub const SEED: ParameterId = ParameterId::from_static(5);
    pub const OVERFLOW: ParameterId = ParameterId::from_static(6);
}

const SIGN_LABELS: [&str; 3] = ["add", "sub", "bi"];
const OVERFLOW_LABELS: [&str; 3] = ["clamp", "drop", "fold"];

const DESCRIPTORS: [ParameterDescriptor; 6] = [
    ParameterDescriptor {
        id: parameter::CHANCE,
        label: "chance",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 100.0,
            unit: Some("%"),
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::CHOICES,
        label: "choices",
        kind: ParameterKind::Float {
            min: MIN_CHOICES as f32,
            max: MAX_CHOICES as f32,
            unit: None,
        },
        default: ParameterValue::Float(12.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::SCALE,
        label: "scale",
        kind: ParameterKind::Float {
            min: MIN_SCALE as f32,
            max: MAX_SCALE as f32,
            unit: Some("st"),
        },
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::SIGN,
        label: "sign",
        kind: ParameterKind::Choice {
            labels: &SIGN_LABELS,
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::SEED,
        label: "seed",
        kind: ParameterKind::Float {
            min: 0.0,
            max: u16::MAX as f32,
            unit: None,
        },
        default: ParameterValue::Float(1.0),
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

/// Direction policy for applied random pitch steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RandomSign {
    Add = 0,
    Sub = 1,
    Bi = 2,
}

impl RandomSign {
    pub const fn from_choice(choice: u32) -> Option<Self> {
        match choice {
            0 => Some(Self::Add),
            1 => Some(Self::Sub),
            2 => Some(Self::Bi),
            _ => None,
        }
    }

    pub const fn as_choice(self) -> u32 {
        self as u32
    }
}

/// Explicit boundary policy when a randomized pitch leaves MIDI `0..=127`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RandomOverflowPolicy {
    Clamp = 0,
    Drop = 1,
    Fold = 2,
}

impl RandomOverflowPolicy {
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

/// Pure deterministic SplitMix64 step (zero float, zero allocation).
const fn splitmix64_next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Deterministic initial PRNG state derived from an authored `u16` seed.
pub const fn initial_rng_state(seed: u16) -> u64 {
    let mut state = (seed as u64) ^ SEED_DOMAIN_TAG;
    let _ = splitmix64_next(&mut state);
    state
}

/// Validated authored settings for `hex.midi.random`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RandomSettings {
    /// Probability in basis points `0..=10_000` (`0.00%..=100.00%`).
    pub chance_bps: u16,
    /// Discrete number of step choices `1..=24`.
    pub choices: u8,
    /// Semitone step size `1..=24`.
    pub scale: u8,
    pub sign: RandomSign,
    /// Deterministic PRNG seed `0..=65535`.
    pub seed: u16,
    pub overflow: RandomOverflowPolicy,
}

impl Default for RandomSettings {
    fn default() -> Self {
        Self {
            chance_bps: 0,
            choices: 12,
            scale: 1,
            sign: RandomSign::Add,
            seed: 1,
            overflow: RandomOverflowPolicy::Clamp,
        }
    }
}

impl RandomSettings {
    pub fn encode(self) -> Result<NativeDeviceState, DeviceContractError> {
        self.validate()?;
        let mut payload = Vec::with_capacity(STATE_BYTES);
        payload.extend_from_slice(&self.chance_bps.to_le_bytes());
        payload.push(self.choices);
        payload.push(self.scale);
        payload.push(self.sign as u8);
        payload.extend_from_slice(&self.seed.to_le_bytes());
        payload.push(self.overflow as u8);
        NativeDeviceState::new(RANDOM_TYPE_ID, RANDOM_STATE_VERSION, payload)
    }

    pub fn decode(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != RANDOM_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != RANDOM_STATE_VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        if !state.asset_references().is_empty() {
            return Err(DeviceContractError::AssetResourcesUnavailable);
        }
        let payload = state.payload();
        if payload.len() != STATE_BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let chance_bps = u16::from_le_bytes([payload[0], payload[1]]);
        let sign = RandomSign::from_choice(u32::from(payload[4]))
            .ok_or(DeviceContractError::MalformedState)?;
        let seed = u16::from_le_bytes([payload[5], payload[6]]);
        let overflow = RandomOverflowPolicy::from_choice(u32::from(payload[7]))
            .ok_or(DeviceContractError::MalformedState)?;
        let settings = Self {
            chance_bps,
            choices: payload[2],
            scale: payload[3],
            sign,
            seed,
            overflow,
        };
        settings.validate()?;
        Ok(settings)
    }

    fn validate(self) -> Result<(), DeviceContractError> {
        if self.chance_bps > MAX_CHANCE_BPS
            || !(MIN_CHOICES..=MAX_CHOICES).contains(&self.choices)
            || !(MIN_SCALE..=MAX_SCALE).contains(&self.scale)
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(())
    }

    /// Deterministically advance `rng_state` by one `NoteOn` and return the
    /// mapped MIDI pitch (`None` when dropped by `RandomOverflowPolicy::Drop`).
    pub const fn next_pitch(self, pitch: u8, rng_state: &mut u64) -> Option<u8> {
        if pitch > MAX_MIDI_NOTE {
            return None;
        }
        let draw_chance = splitmix64_next(rng_state);
        let draw_step = splitmix64_next(rng_state);

        let triggered = self.chance_bps > 0
            && (self.chance_bps >= MAX_CHANCE_BPS
                || (draw_chance % (MAX_CHANCE_BPS as u64)) < (self.chance_bps as u64));

        let offset: i16 = if !triggered {
            0
        } else {
            let choices = self.choices as u64;
            let scale = self.scale as i16;
            match self.sign {
                RandomSign::Add => {
                    let step = ((draw_step % choices) + 1) as i16;
                    step * scale
                }
                RandomSign::Sub => {
                    let step = ((draw_step % choices) + 1) as i16;
                    -(step * scale)
                }
                RandomSign::Bi => {
                    let negative = (draw_step & 1) != 0;
                    let step = (((draw_step >> 1) % choices) + 1) as i16;
                    if negative {
                        -(step * scale)
                    } else {
                        step * scale
                    }
                }
            }
        };

        let target = (pitch as i16) + offset;
        if target >= 0 && target <= MAX_MIDI_NOTE as i16 {
            return Some(target as u8);
        }
        match self.overflow {
            RandomOverflowPolicy::Clamp => {
                if target < 0 {
                    Some(0)
                } else {
                    Some(MAX_MIDI_NOTE)
                }
            }
            RandomOverflowPolicy::Drop => None,
            RandomOverflowPolicy::Fold => {
                if target < 0 {
                    Some(target.rem_euclid(12) as u8)
                } else {
                    Some((127 - (127 - target).rem_euclid(12)) as u8)
                }
            }
        }
    }
}

pub static RANDOM_DEFINITION: RandomMidiProcessorDefinition = RandomMidiProcessorDefinition;

pub struct RandomMidiProcessorDefinition;

impl NativeDeviceMetadata for RandomMidiProcessorDefinition {
    fn type_id(&self) -> DeviceTypeId {
        RANDOM_TYPE_ID
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("random")
    }

    fn state_version(&self) -> u32 {
        RANDOM_STATE_VERSION
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &DESCRIPTORS
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        RandomSettings::default().encode()
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let settings = RandomSettings::decode(state)?;
        settings.encode()?.with_host_metadata_from(state)
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let settings = RandomSettings::decode(state)?;
        match parameter {
            parameter::CHANCE => Ok(ParameterValue::Float(
                f32::from(settings.chance_bps) / 100.0,
            )),
            parameter::CHOICES => Ok(ParameterValue::Float(f32::from(settings.choices))),
            parameter::SCALE => Ok(ParameterValue::Float(f32::from(settings.scale))),
            parameter::SIGN => Ok(ParameterValue::Choice(settings.sign.as_choice())),
            parameter::SEED => Ok(ParameterValue::Float(f32::from(settings.seed))),
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
        let mut settings = RandomSettings::decode(state)?;
        match (parameter, value) {
            (parameter::CHANCE, ParameterValue::Float(v)) if v.is_finite() => {
                if !(0.0..=100.0).contains(&v) {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.chance_bps = (v * 100.0).round() as u16;
            }
            (parameter::CHOICES, ParameterValue::Float(v)) if v.is_finite() => {
                let r = v.round();
                if r < f32::from(MIN_CHOICES) || r > f32::from(MAX_CHOICES) {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.choices = r as u8;
            }
            (parameter::SCALE, ParameterValue::Float(v)) if v.is_finite() => {
                let r = v.round();
                if r < f32::from(MIN_SCALE) || r > f32::from(MAX_SCALE) {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.scale = r as u8;
            }
            (parameter::SIGN, ParameterValue::Choice(choice)) => {
                settings.sign = RandomSign::from_choice(choice)
                    .ok_or(DeviceContractError::InvalidParameterValue)?;
            }
            (parameter::SEED, ParameterValue::Float(v)) if v.is_finite() => {
                let r = v.round();
                if r < 0.0 || r > f32::from(u16::MAX) {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                settings.seed = r as u16;
            }
            (parameter::OVERFLOW, ParameterValue::Choice(choice)) => {
                settings.overflow = RandomOverflowPolicy::from_choice(choice)
                    .ok_or(DeviceContractError::InvalidParameterValue)?;
            }
            (
                parameter::CHANCE
                | parameter::CHOICES
                | parameter::SCALE
                | parameter::SIGN
                | parameter::SEED
                | parameter::OVERFLOW,
                _,
            ) => return Err(DeviceContractError::InvalidParameterValue),
            _ => return Err(DeviceContractError::UnknownParameter),
        }
        settings.encode()?.with_host_metadata_from(state)
    }
}

impl NativeMidiProcessorDefinition for RandomMidiProcessorDefinition {
    fn prepare_midi_processor(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Box<dyn PreparedNativeMidiProcessor>, DeviceContractError> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let settings = RandomSettings::decode(state)?;
        Ok(Box::new(PreparedRandomMidiProcessor::new(settings)?))
    }
}

/// Control-prepared immutable factory for `hex.midi.random`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedRandomMidiProcessor {
    settings: RandomSettings,
    max_input_events: usize,
    max_output_events: usize,
    max_active_occurrences: usize,
    max_work_units: usize,
}

impl PreparedRandomMidiProcessor {
    pub fn new(settings: RandomSettings) -> Result<Self, DeviceContractError> {
        Self::with_caps(
            settings,
            RANDOM_MAX_INPUT_EVENTS,
            RANDOM_MAX_OUTPUT_EVENTS,
            RANDOM_MAX_ACTIVE_OCCURRENCES,
            RANDOM_MAX_WORK_UNITS,
        )
    }

    pub fn with_caps(
        settings: RandomSettings,
        max_input_events: usize,
        max_output_events: usize,
        max_active_occurrences: usize,
        max_work_units: usize,
    ) -> Result<Self, DeviceContractError> {
        settings.validate()?;
        if max_input_events == 0
            || max_output_events == 0
            || max_active_occurrences == 0
            || max_active_occurrences > RANDOM_MAX_ACTIVE_OCCURRENCES
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

    pub const fn settings(self) -> RandomSettings {
        self.settings
    }
}

impl PreparedNativeMidiProcessor for PreparedRandomMidiProcessor {
    fn runtime_version(&self) -> u32 {
        RANDOM_RUNTIME_VERSION
    }

    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<RandomMidiRuntime>()
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
        Box::new(RandomMidiRuntime {
            config: *self,
            rng_state: initial_rng_state(self.settings.seed),
            active: [None; RANDOM_MAX_ACTIVE_OCCURRENCES],
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TrackedRandomOccurrence {
    occurrence_id: u64,
    emitted: bool,
}

#[derive(Clone)]
struct RandomMidiRuntime {
    config: PreparedRandomMidiProcessor,
    rng_state: u64,
    active: [Option<TrackedRandomOccurrence>; RANDOM_MAX_ACTIVE_OCCURRENCES],
}

impl RandomMidiRuntime {
    fn find_slot(slots: &[Option<TrackedRandomOccurrence>], occurrence_id: u64) -> Option<usize> {
        slots
            .iter()
            .position(|slot| slot.is_some_and(|entry| entry.occurrence_id == occurrence_id))
    }

    fn find_free_slot(slots: &[Option<TrackedRandomOccurrence>]) -> Option<usize> {
        slots.iter().position(Option::is_none)
    }

    fn count_active(slots: &[Option<TrackedRandomOccurrence>]) -> usize {
        slots.iter().flatten().count()
    }
}

impl MutableMidiProcessorRuntime for RandomMidiRuntime {
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
        let mut sim_rng = self.rng_state;
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
                    let emitted = self
                        .config
                        .settings
                        .next_pitch(pitch, &mut sim_rng)
                        .is_some();
                    simulated[free] = Some(TrackedRandomOccurrence {
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
                    let mapped = self.config.settings.next_pitch(pitch, &mut self.rng_state);
                    if let Some(free) = Self::find_free_slot(&self.active[..active_cap]) {
                        self.active[free] = Some(TrackedRandomOccurrence {
                            occurrence_id,
                            emitted: mapped.is_some(),
                        });
                    }
                    if let Some(random_pitch) = mapped {
                        let _ = output.charge_work(1);
                        let _ = output.push(NativeInstrumentEvent::NoteOn {
                            frame_offset,
                            occurrence_id,
                            pitch: random_pitch,
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
        let mut ids = [0_u64; RANDOM_MAX_ACTIVE_OCCURRENCES];
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
        self.rng_state = initial_rng_state(self.config.settings.seed);
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
        // Keep the deterministic stream across non-seed parameter edits, and
        // explicitly reseed when the authored seed changes.
        if self.config.settings.seed == other.config.settings.seed {
            self.rng_state = other.rng_state;
        } else {
            self.rng_state = initial_rng_state(self.config.settings.seed);
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

    fn test_limits() -> NativeMidiProcessorLimits {
        NativeMidiProcessorLimits {
            max_pair_bytes: 256 * 1024,
            max_input_events_per_block: RANDOM_MAX_INPUT_EVENTS,
            max_output_events_per_block: RANDOM_MAX_OUTPUT_EVENTS,
            max_active_occurrences: RANDOM_MAX_ACTIVE_OCCURRENCES,
            max_work_units_per_block: RANDOM_MAX_WORK_UNITS,
        }
    }

    #[test]
    fn random_state_codec_validation_and_parameter_roundtrip() {
        let default = RANDOM_DEFINITION.default_state().unwrap();
        assert_eq!(
            RandomSettings::decode(&default).unwrap(),
            RandomSettings::default()
        );

        let patched = RANDOM_DEFINITION
            .patch_parameter(&default, parameter::CHANCE, ParameterValue::Float(75.25))
            .unwrap();
        let patched = RANDOM_DEFINITION
            .patch_parameter(&patched, parameter::CHOICES, ParameterValue::Float(8.0))
            .unwrap();
        let patched = RANDOM_DEFINITION
            .patch_parameter(&patched, parameter::SCALE, ParameterValue::Float(3.0))
            .unwrap();
        let patched = RANDOM_DEFINITION
            .patch_parameter(&patched, parameter::SIGN, ParameterValue::Choice(2))
            .unwrap();
        let patched = RANDOM_DEFINITION
            .patch_parameter(&patched, parameter::SEED, ParameterValue::Float(4_242.0))
            .unwrap();
        let patched = RANDOM_DEFINITION
            .patch_parameter(&patched, parameter::OVERFLOW, ParameterValue::Choice(1))
            .unwrap();

        let decoded = RandomSettings::decode(&patched).unwrap();
        assert_eq!(
            decoded,
            RandomSettings {
                chance_bps: 7_525,
                choices: 8,
                scale: 3,
                sign: RandomSign::Bi,
                seed: 4_242,
                overflow: RandomOverflowPolicy::Drop,
            }
        );
        assert_eq!(
            RANDOM_DEFINITION
                .read_parameter(&patched, parameter::CHANCE)
                .unwrap(),
            ParameterValue::Float(75.25)
        );
        assert_eq!(
            RANDOM_DEFINITION
                .read_parameter(&patched, parameter::SEED)
                .unwrap(),
            ParameterValue::Float(4_242.0)
        );
        assert_eq!(
            RANDOM_DEFINITION
                .read_parameter(&patched, parameter::SIGN)
                .unwrap(),
            ParameterValue::Choice(2)
        );

        // Reject out-of-range / NaN / wrong-type / malformed state payloads.
        assert!(
            RANDOM_DEFINITION
                .patch_parameter(&default, parameter::CHANCE, ParameterValue::Float(101.0))
                .is_err()
        );
        assert!(
            RANDOM_DEFINITION
                .patch_parameter(&default, parameter::CHANCE, ParameterValue::Float(f32::NAN))
                .is_err()
        );
        assert!(
            RANDOM_DEFINITION
                .patch_parameter(&default, parameter::CHOICES, ParameterValue::Float(0.0))
                .is_err()
        );
        assert!(
            RANDOM_DEFINITION
                .patch_parameter(&default, parameter::SCALE, ParameterValue::Float(25.0))
                .is_err()
        );
        assert!(
            RANDOM_DEFINITION
                .patch_parameter(&default, parameter::SIGN, ParameterValue::Choice(3))
                .is_err()
        );
        let malformed = NativeDeviceState::new(RANDOM_TYPE_ID, 1, vec![0, 1]).unwrap();
        assert_eq!(
            RandomSettings::decode(&malformed),
            Err(DeviceContractError::MalformedState)
        );
    }

    #[test]
    fn seeded_reproducibility_distinct_seeds_bounds_and_live_handoff_ledger() {
        let settings_a = RandomSettings {
            chance_bps: MAX_CHANCE_BPS,
            choices: 7,
            scale: 2,
            sign: RandomSign::Bi,
            seed: 17,
            overflow: RandomOverflowPolicy::Clamp,
        };
        let settings_b = RandomSettings {
            seed: 18,
            ..settings_a
        };

        // Reproducible independent oracle sequence & distinct-seed sensitivity.
        let mut rng_a = initial_rng_state(settings_a.seed);
        let mut rng_b = initial_rng_state(settings_b.seed);
        let seq_a: Vec<u8> = (0..16)
            .map(|_| settings_a.next_pitch(60, &mut rng_a).unwrap())
            .collect();
        let seq_b: Vec<u8> = (0..16)
            .map(|_| settings_b.next_pitch(60, &mut rng_b).unwrap())
            .collect();
        assert_ne!(
            seq_a, seq_b,
            "distinct seeds must produce distinct sequences"
        );
        for &p in &seq_a {
            assert!(
                p <= MAX_MIDI_NOTE && p != 60,
                "100% chance must shift pitch"
            );
            assert_eq!((p as i16 - 60).abs() % 2, 0, "step must respect scale=2");
        }

        // 0% chance is strict identity passthrough.
        let zero_chance = RandomSettings {
            chance_bps: 0,
            ..settings_a
        };
        let mut rng_z = initial_rng_state(zero_chance.seed);
        for _ in 0..8 {
            assert_eq!(zero_chance.next_pitch(60, &mut rng_z), Some(60));
        }

        // Overflow policies at MIDI 127 boundary.
        let high_add = RandomSettings {
            chance_bps: MAX_CHANCE_BPS,
            choices: 4,
            scale: 5,
            sign: RandomSign::Add,
            seed: 9,
            overflow: RandomOverflowPolicy::Drop,
        };
        let mut rng_drop = initial_rng_state(9);
        assert_eq!(high_add.next_pitch(125, &mut rng_drop), None);

        let high_fold = RandomSettings {
            overflow: RandomOverflowPolicy::Fold,
            ..high_add
        };
        let mut rng_fold = initial_rng_state(9);
        let folded = high_fold.next_pitch(125, &mut rng_fold).unwrap();
        assert!((116..=127).contains(&folded));

        // Full runtime pair test: overlapping same-pitch notes, reset reproducibility,
        // and queue-saturation atomic refusal without corrupting PRNG or active ledger.
        let prep_a = PreparedRandomMidiProcessor::new(settings_a).unwrap();
        let mut pair = NativeMidiProcessorRuntimePair::new(&prep_a, test_limits()).unwrap();
        let mut out_buf = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; 16];
        let batch = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 60,
                velocity: 100,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 10,
                occurrence_id: 2,
                pitch: 60,
                velocity: 90,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 20,
                occurrence_id: 1,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 30,
                occurrence_id: 2,
            },
        ];
        pair.begin_block();
        let written = pair.process_block(&batch, 64, &mut out_buf, None).unwrap();
        pair.commit_block();
        let out = out_buf[..written].to_vec();
        assert_eq!(out.len(), 4);
        assert!(matches!(
            out[0],
            NativeInstrumentEvent::NoteOn {
                occurrence_id: 1,
                pitch,
                ..
            } if pitch == seq_a[0]
        ));
        assert!(matches!(
            out[1],
            NativeInstrumentEvent::NoteOn {
                occurrence_id: 2,
                pitch,
                ..
            } if pitch == seq_a[1]
        ));
        assert!(matches!(
            out[2],
            NativeInstrumentEvent::NoteOff {
                occurrence_id: 1,
                ..
            }
        ));
        assert!(matches!(
            out[3],
            NativeInstrumentEvent::NoteOff {
                occurrence_id: 2,
                ..
            }
        ));
        assert_eq!(pair.committed_active_occurrences(), 0);

        // Reset reproduces the exact same first two pitches bit-for-bit.
        pair.reset();
        let mut replay_buf = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; 16];
        pair.begin_block();
        let replay_written = pair
            .process_block(&batch, 64, &mut replay_buf, None)
            .unwrap();
        pair.commit_block();
        assert_eq!(&replay_buf[..replay_written], out.as_slice());

        // Queue saturation / capacity refusal leaves committed PRNG & active state intact.
        let tiny_prep = PreparedRandomMidiProcessor::with_caps(settings_a, 4, 1, 2, 8).unwrap();
        let mut tiny_pair = NativeMidiProcessorRuntimePair::new(&tiny_prep, test_limits()).unwrap();
        let mut tiny_out = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; 1];
        tiny_pair.begin_block();
        assert_eq!(
            tiny_pair.process_block(&batch[..2], 64, &mut tiny_out, None),
            Err(NativeMidiProcessorError::OutputEventCapacityExceeded)
        );
        tiny_pair.rollback_block();
        assert_eq!(tiny_pair.committed_active_occurrences(), 0);
        // After refusal, the very first valid note still receives seq_a[0]!
        tiny_pair.begin_block();
        let tiny_written = tiny_pair
            .process_block(&batch[..1], 64, &mut tiny_out, None)
            .unwrap();
        tiny_pair.commit_block();
        assert_eq!(tiny_written, 1);
        assert!(matches!(
            tiny_out[0],
            NativeInstrumentEvent::NoteOn {
                occurrence_id: 1,
                pitch,
                ..
            } if pitch == seq_a[0]
        ));
    }
}
