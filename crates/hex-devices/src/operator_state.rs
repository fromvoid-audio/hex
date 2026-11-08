//! Original Operator control state and registered shared-context instrument.
//!
//! Versioned state preserves every earlier prefix and appends fixed controls.
//! The old FM4 device keeps its separate type, state layout and global ADSR.
use crate::operator_waveform::OperatorWaveform;
use crate::{
    SineTable,
    fm4::{
        OPERATORS, Oscillator, PreparedOperatorConfiguration, PreparedOperatorFm4,
        PreparedOperatorNote, Routing,
    },
    operator_envelope::OperatorEnvelope,
};
use hex_lattice::{
    NativeInstrumentDefinition, PreparedAnalyticNote, PreparedNativeInstrument,
    PreparedSharedAnalyticContext, PreparedSharedAnalyticNote, StereoFrame,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};

pub const OPERATOR_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.instrument.operator");
pub const OPERATOR_STATE_VERSION: u32 = KEYTRACK_STATE_VERSION;
const KEYTRACK_STATE_VERSION: u32 = 17;
const FINE_STATE_VERSION: u32 = 16;
const GLOBAL_STATE_VERSION: u32 = 15;
const COMPANION_STATE_VERSION: u32 = 14;
const CATALOG_STATE_VERSION: u32 = 13;
const HARMONIC_STATE_VERSION: u32 = 12;
const LFO_STATE_VERSION: u32 = 7;
const FILTER_PARAMETERS: usize = crate::operator_filter::COUNT - 1;
const ENABLE_STATE_VERSION: u32 = 8;
const FEEDBACK_STATE_VERSION: u32 = 5;
const TUNING_STATE_VERSION: u32 = 3;
const WAVEFORM_STATE_VERSION: u32 = 2;
pub mod algorithms;
mod companion;
mod enable;
mod fine;
mod global;
mod harmonic_runtime;
mod harmonics;
mod keytrack;
mod modulation;
pub use harmonics::{
    partial as harmonic_parameter, range as harmonic_range_parameter,
    repeat as harmonic_repeat_parameter,
};
mod enable_continuation;
mod feedback_runtime;
mod lfo;
mod lfo_runtime;
mod occurrence_gain;
mod tuning;
const LEGACY_STATE_VERSION: u32 = 1;
const CORE_FIELDS: usize = 32;
const ENVELOPE_FIELDS: usize = 6;
const FIELD_COUNT: usize = CORE_FIELDS + OPERATORS * ENVELOPE_FIELDS;
const FLOAT_PARAMETER_COUNT: usize = FIELD_COUNT - OPERATORS;
const WAVEFORM_COUNT: usize = OPERATORS;
const VELOCITY_SENSITIVITY_COUNT: usize = OPERATORS;
const STORED_PARAMETER_COUNT: usize = FLOAT_PARAMETER_COUNT
    + WAVEFORM_COUNT
    + 1
    + OPERATORS * 3
    + VELOCITY_SENSITIVITY_COUNT
    + FEEDBACK_COUNT
    + lfo::COUNT
    + enable::COUNT
    + FILTER_PARAMETERS
    + harmonics::COUNT
    + companion::COUNT;
const PARAMETER_COUNT: usize =
    STORED_PARAMETER_COUNT + global::COUNT + modulation::COUNT + 1 + fine::COUNT + keytrack::COUNT;
const LEGACY_PAYLOAD_BYTES: usize = FIELD_COUNT * size_of::<f32>();
const WAVEFORM_PAYLOAD_BYTES: usize = LEGACY_PAYLOAD_BYTES + WAVEFORM_COUNT * size_of::<u32>();
const TUNING_PAYLOAD_BYTES: usize = WAVEFORM_PAYLOAD_BYTES + tuning::BYTES;
const VELOCITY_PAYLOAD_BYTES: usize =
    TUNING_PAYLOAD_BYTES + VELOCITY_SENSITIVITY_COUNT * size_of::<f32>();
const FEEDBACK_COUNT: usize = OPERATORS;
const FEEDBACK_PAYLOAD_BYTES: usize = VELOCITY_PAYLOAD_BYTES + FEEDBACK_COUNT * size_of::<f32>();
const LFO_PAYLOAD_BYTES: usize = FEEDBACK_PAYLOAD_BYTES + lfo::BYTES;
const ENABLE_PAYLOAD_BYTES: usize = LFO_PAYLOAD_BYTES + enable::BYTES;
const FILTER_PAYLOAD_BYTES: usize = ENABLE_PAYLOAD_BYTES + crate::operator_filter::BYTES;
const HARMONIC_PAYLOAD_BYTES: usize = FILTER_PAYLOAD_BYTES + harmonics::BYTES;
const COMPANION_PAYLOAD_BYTES: usize = HARMONIC_PAYLOAD_BYTES + companion::BYTES;
const GLOBAL_PAYLOAD_BYTES: usize = COMPANION_PAYLOAD_BYTES + global::BYTES;
const MODULATION_PAYLOAD_BYTES: usize = GLOBAL_PAYLOAD_BYTES + modulation::BYTES;
const FINE_PAYLOAD_BYTES: usize = MODULATION_PAYLOAD_BYTES + fine::BYTES;
const PAYLOAD_BYTES: usize = FINE_PAYLOAD_BYTES + keytrack::BYTES;
const ENVELOPE_DEFAULTS: [f32; ENVELOPE_FIELDS] = [0.0, 1.0, 0.7, 0.005, 0.1, 0.1];

pub mod parameter {
    pub use crate::fm4::device::parameter::{
        carrier, modulation, oscillator_level, oscillator_phase, oscillator_ratio,
    };
    use hex_lattice_contract::ParameterId;

    /// Note-pitch tracking, 0–100%, from fixed to normal keyboard tuning. Operators are one-based.
    pub const fn pitch_keytrack(operator: u32) -> ParameterId {
        super::keytrack::parameter(operator)
    }

    /// Signed Hz added after ratio or fixed-frequency multiplication. Operators are one-based.
    pub const fn fine_offset(operator: u32) -> ParameterId {
        super::fine::parameter(operator)
    }

    /// Stable independently persisted enable switch; operators are one-based.
    pub const fn enabled(operator: u32) -> ParameterId {
        super::enable::parameter(operator)
    }

    /// Stable per-operator envelope field order in the opaque state.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[repr(u32)]
    pub enum EnvelopeField {
        Initial,
        Peak,
        Sustain,
        Attack,
        Decay,
        Release,
    }

    /// Operator numbers are one-based. Zero denotes an invalid endpoint.
    pub const fn envelope(operator: u32, field: EnvelopeField) -> ParameterId {
        if operator == 0 || operator > 4 {
            return ParameterId::from_static(0);
        }
        ParameterId::from_static(33 + (operator - 1) * 6 + field as u32)
    }

    /// Operator numbers are one-based. Zero denotes an invalid endpoint.
    pub const fn waveform(operator: u32) -> ParameterId {
        if operator == 0 || operator > 4 {
            return ParameterId::from_static(0);
        }
        ParameterId::from_static(56 + operator)
    }

    pub const fn fixed(operator: u32) -> ParameterId {
        tuning_parameter(operator, 0)
    }
    pub const fn fixed_frequency(operator: u32) -> ParameterId {
        tuning_parameter(operator, 1)
    }
    pub const fn fixed_multiplier(operator: u32) -> ParameterId {
        tuning_parameter(operator, 2)
    }

    /// Operator numbers are one-based. Zero denotes an invalid endpoint.
    pub const fn amplitude_velocity(operator: u32) -> ParameterId {
        if operator == 0 || operator > 4 {
            return ParameterId::from_static(0);
        }
        ParameterId::from_static(73 + operator)
    }
    /// Operator numbers are one-based. Zero denotes an invalid endpoint.
    pub const fn feedback(operator: u32) -> ParameterId {
        if operator == 0 || operator > 4 {
            return ParameterId::from_static(0);
        }
        ParameterId::from_static(77 + operator)
    }
    const fn tuning_parameter(operator: u32, field: u32) -> ParameterId {
        if operator == 0 || operator > 4 {
            return ParameterId::from_static(0);
        }
        ParameterId::from_static(62 + (operator - 1) * 3 + field)
    }

    pub const LFO_ENABLED: ParameterId = ParameterId::from_static(82);
    pub const LFO_WAVEFORM: ParameterId = ParameterId::from_static(83);
    pub const LFO_SYNC: ParameterId = ParameterId::from_static(84);
    pub const LFO_RATE: ParameterId = ParameterId::from_static(85);
    pub const LFO_DIVISION: ParameterId = ParameterId::from_static(86);
    pub const LFO_AMOUNT: ParameterId = ParameterId::from_static(87);
    pub const LFO_RETRIGGER: ParameterId = ParameterId::from_static(88);
    pub const fn lfo_destination(operator: u32) -> ParameterId {
        if operator == 0 || operator > 4 {
            return ParameterId::from_static(0);
        }
        ParameterId::from_static(88 + operator)
    }

    /// Sounding voice limit, independent of the64-occurrence admission ledger.
    pub const VOICES: ParameterId = ParameterId::from_static(601);
    pub const RETRIGGER: ParameterId = ParameterId::from_static(602);
    pub const INTERPOLATION: ParameterId = ParameterId::from_static(603);
    pub const ANTIALIAS: ParameterId = ParameterId::from_static(604);
    pub const PAN: ParameterId = ParameterId::from_static(605);
    pub const PAN_KEY: ParameterId = ParameterId::from_static(606);
    pub const PAN_RANDOM: ParameterId = ParameterId::from_static(607);
    pub const GLOBAL_MODE: ParameterId = ParameterId::from_static(608);

    /// Derived eleven-algorithm selector; zero preserves custom legacy routing.
    pub const ALGORITHM: ParameterId = ParameterId::from_static(600);

    /// Derived topology selector. Custom is inferred from the stored routing.
    pub const fn routing_preset() -> ParameterId {
        ParameterId::from_static(61)
    }
}

const ENVELOPE_LABELS: [&str; OPERATORS * ENVELOPE_FIELDS] = [
    "op1 initial",
    "op1 peak",
    "op1 sustain",
    "op1 attack",
    "op1 decay",
    "op1 release",
    "op2 initial",
    "op2 peak",
    "op2 sustain",
    "op2 attack",
    "op2 decay",
    "op2 release",
    "op3 initial",
    "op3 peak",
    "op3 sustain",
    "op3 attack",
    "op3 decay",
    "op3 release",
    "op4 initial",
    "op4 peak",
    "op4 sustain",
    "op4 attack",
    "op4 decay",
    "op4 release",
];

const ROUTING_PRESET_LABELS: [&str; 4] = ["custom", "serial", "parallel", "two pairs"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OperatorRoutingPreset {
    Custom,
    Serial,
    Parallel,
    TwoPairs,
}

impl OperatorRoutingPreset {
    const fn from_choice(choice: u32) -> Option<Self> {
        match choice {
            0 => Some(Self::Custom),
            1 => Some(Self::Serial),
            2 => Some(Self::Parallel),
            3 => Some(Self::TwoPairs),
            _ => None,
        }
    }

    const fn choice(self) -> u32 {
        match self {
            Self::Custom => 0,
            Self::Serial => 1,
            Self::Parallel => 2,
            Self::TwoPairs => 3,
        }
    }

    const fn routing_fields(self) -> Option<[f32; 20]> {
        let mut fields = [0.0; 20];
        match self {
            Self::Custom => None,
            Self::Serial => {
                fields[4] = 0.25; // op2 -> op1
                fields[9] = 0.25; // op3 -> op2
                fields[14] = 0.25; // op4 -> op3
                fields[16] = 1.0; // op1 carrier
                Some(fields)
            }
            Self::Parallel => {
                fields[16] = 0.25;
                fields[17] = 0.25;
                fields[18] = 0.25;
                fields[19] = 0.25;
                Some(fields)
            }
            Self::TwoPairs => {
                fields[4] = 0.25; // op2 -> op1
                fields[14] = 0.25; // op4 -> op3
                fields[16] = 0.5; // op1 carrier
                fields[18] = 0.5; // op3 carrier
                Some(fields)
            }
        }
    }
}

const fn parameters() -> [ParameterDescriptor; PARAMETER_COUNT] {
    let mut result = [crate::fm4::device::PARAMETERS[0]; PARAMETER_COUNT];
    let mut index = 0;
    while index < CORE_FIELDS - OPERATORS {
        result[index] = crate::fm4::device::PARAMETERS[index];
        if index < 12 && index % 3 == 1 && index > 1 {
            result[index].default = ParameterValue::Float(0.0);
        }
        index += 1;
    }
    let mut envelope = 0;
    while envelope < ENVELOPE_LABELS.len() {
        let field = envelope % ENVELOPE_FIELDS;
        let time = field >= 3;
        result[index] = ParameterDescriptor {
            id: ParameterId::from_static((CORE_FIELDS + envelope + 1) as u32),
            label: ENVELOPE_LABELS[envelope],
            kind: ParameterKind::Float {
                min: if field >= 4 { 0.001 } else { 0.0 },
                max: match field {
                    3 => crate::operator_envelope::MAX_ATTACK_SECONDS as f32,
                    4 => crate::operator_envelope::MAX_DECAY_SECONDS as f32,
                    5 => crate::operator_envelope::MAX_RELEASE_SECONDS as f32,
                    _ => 1.0,
                },
                unit: if time { Some("s") } else { None },
            },
            default: ParameterValue::Float(ENVELOPE_DEFAULTS[field]),
            presentation: ParameterPresentation::Linear,
        };
        index += 1;
        envelope += 1;
    }
    let waveforms = [
        "op1 waveform",
        "op2 waveform",
        "op3 waveform",
        "op4 waveform",
    ];
    let mut operator = 0;
    while operator < OPERATORS {
        result[index] = ParameterDescriptor {
            id: ParameterId::from_static((57 + operator) as u32),
            label: waveforms[operator],
            kind: ParameterKind::Choice {
                labels: &harmonics::WAVE_LABELS,
            },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        };
        index += 1;
        operator += 1;
    }
    result[index] = ParameterDescriptor {
        id: ParameterId::from_static(61),
        label: "routing preset",
        kind: ParameterKind::Choice {
            labels: &ROUTING_PRESET_LABELS,
        },
        default: ParameterValue::Choice(1),
        presentation: ParameterPresentation::Linear,
    };
    index += 1;
    let tuning = tuning::descriptors();
    let mut offset = 0;
    while offset < tuning.len() {
        result[index + offset] = tuning[offset];
        offset += 1;
    }
    index += tuning.len();
    let velocity_labels = [
        "op1 amplitude velocity",
        "op2 amplitude velocity",
        "op3 amplitude velocity",
        "op4 amplitude velocity",
    ];
    let mut operator = 0;
    while operator < OPERATORS {
        result[index + operator] = ParameterDescriptor {
            id: ParameterId::from_static(74 + operator as u32),
            label: velocity_labels[operator],
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        };
        operator += 1;
    }
    let feedback_labels = [
        "op1 feedback",
        "op2 feedback",
        "op3 feedback",
        "op4 feedback",
    ];
    operator = 0;
    while operator < OPERATORS {
        result[index + OPERATORS + operator] = ParameterDescriptor {
            id: ParameterId::from_static(78 + operator as u32),
            label: feedback_labels[operator],
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        };
        operator += 1;
    }
    let lfo = lfo::descriptors();
    let mut offset = 0;
    while offset < lfo::COUNT {
        result[STORED_PARAMETER_COUNT
            - companion::COUNT
            - harmonics::COUNT
            - FILTER_PARAMETERS
            - lfo::COUNT
            - enable::COUNT
            + offset] = lfo[offset];
        offset += 1;
    }
    let enabled = enable::descriptors();
    let mut offset = 0;
    while offset < enable::COUNT {
        result[STORED_PARAMETER_COUNT
            - companion::COUNT
            - harmonics::COUNT
            - FILTER_PARAMETERS
            - enable::COUNT
            + offset] = enabled[offset];
        offset += 1;
    }
    let filter = crate::operator_filter::descriptors();
    let mut offset = 0;
    while offset < FILTER_PARAMETERS {
        result[STORED_PARAMETER_COUNT - companion::COUNT - harmonics::COUNT - FILTER_PARAMETERS
            + offset] = filter[offset];
        offset += 1;
    }
    index = STORED_PARAMETER_COUNT - companion::COUNT - harmonics::COUNT;
    let harmonic = harmonics::descriptors();
    let mut extra = 0;
    while extra < harmonics::COUNT {
        result[index] = harmonic[extra];
        index += 1;
        extra += 1;
    }
    let companion = companion::descriptors();
    let mut offset = 0;
    while offset < companion::COUNT {
        result[STORED_PARAMETER_COUNT - companion::COUNT + offset] = companion[offset];
        offset += 1;
    }

    let globals = global::descriptors();
    let mut offset = 0;
    while offset < global::COUNT {
        result[STORED_PARAMETER_COUNT + offset] = globals[offset];
        offset += 1;
    }
    let assignments = modulation::descriptors();
    let mut offset = 0;
    while offset < modulation::COUNT {
        result[STORED_PARAMETER_COUNT + global::COUNT + offset] = assignments[offset];
        offset += 1;
    }
    result[STORED_PARAMETER_COUNT + global::COUNT + modulation::COUNT] = algorithms::DESCRIPTOR;
    let fine = fine::descriptors();
    let mut offset = 0;
    while offset < fine::COUNT {
        result[PARAMETER_COUNT - keytrack::COUNT - fine::COUNT + offset] = fine[offset];
        offset += 1;
    }
    let keytrack = keytrack::descriptors();
    let mut offset = 0;
    while offset < keytrack::COUNT {
        result[PARAMETER_COUNT - keytrack::COUNT + offset] = keytrack[offset];
        offset += 1;
    }
    result
}

static PARAMETERS: [ParameterDescriptor; PARAMETER_COUNT] = parameters();

/// Validated control-side configuration. It owns no runtime voices or assets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OperatorSettings {
    fields: [f32; FIELD_COUNT],
    waveforms: [OperatorWaveform; OPERATORS],
    tuning: tuning::TuningState,
    amplitude_velocity: [f32; OPERATORS],
    feedback: [f32; OPERATORS],
    lfo: lfo::LfoSettings,
    enabled: enable::EnableSettings,
    harmonics: harmonics::Harmonics,
    filter: crate::operator_filter::Settings,
    companion: companion::Settings,
    global: global::Settings,
    modulation: modulation::Settings,
    fine: fine::Settings,
    pitch_keytrack: keytrack::Settings,
}

impl OperatorSettings {
    /// Original defaults; no measured Live transfer/default compatibility claim.
    pub fn defaults() -> Self {
        let mut fields = [0.0; FIELD_COUNT];
        for descriptor in PARAMETERS.iter().take(FLOAT_PARAMETER_COUNT) {
            if let ParameterValue::Float(value) = descriptor.default {
                fields[descriptor.id.value() as usize - 1] = value;
            }
        }
        fields[12..32].copy_from_slice(&algorithms::CATALOG[0].routing_fields());
        Self {
            fields,
            waveforms: [OperatorWaveform::Sine; OPERATORS],
            tuning: tuning::TuningState::default(),
            amplitude_velocity: [0.0; OPERATORS],
            feedback: [0.0; OPERATORS],
            lfo: lfo::LfoSettings::default(),
            enabled: enable::EnableSettings::default(),
            harmonics: harmonics::Harmonics::default(),
            filter: crate::operator_filter::Settings::default(),
            companion: companion::Settings::default(),
            global: global::Settings::default(),
            modulation: modulation::Settings::default(),
            fine: fine::Settings::default(),
            pitch_keytrack: keytrack::Settings::default(),
        }
    }

    pub fn from_state(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != OPERATOR_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        let expected_len = match state.state_version() {
            LEGACY_STATE_VERSION => LEGACY_PAYLOAD_BYTES,
            WAVEFORM_STATE_VERSION => WAVEFORM_PAYLOAD_BYTES,
            TUNING_STATE_VERSION => TUNING_PAYLOAD_BYTES,
            4 => VELOCITY_PAYLOAD_BYTES,
            FEEDBACK_STATE_VERSION | 6 => FEEDBACK_PAYLOAD_BYTES,
            LFO_STATE_VERSION => LFO_PAYLOAD_BYTES,
            ENABLE_STATE_VERSION => ENABLE_PAYLOAD_BYTES,
            11 => FILTER_PAYLOAD_BYTES,
            HARMONIC_STATE_VERSION | CATALOG_STATE_VERSION => HARMONIC_PAYLOAD_BYTES,
            COMPANION_STATE_VERSION => COMPANION_PAYLOAD_BYTES,
            GLOBAL_STATE_VERSION => MODULATION_PAYLOAD_BYTES,
            FINE_STATE_VERSION => FINE_PAYLOAD_BYTES,
            OPERATOR_STATE_VERSION => PAYLOAD_BYTES,
            _ => return Err(DeviceContractError::UnsupportedStateVersion),
        };
        if state.payload().len() != expected_len {
            return Err(DeviceContractError::MalformedState);
        }
        let mut fields = [0.0; FIELD_COUNT];
        for (index, bytes) in state.payload()[..LEGACY_PAYLOAD_BYTES]
            .chunks_exact(size_of::<f32>())
            .enumerate()
        {
            let bytes: [u8; 4] = bytes
                .try_into()
                .map_err(|_| DeviceContractError::MalformedState)?;
            let field = fields
                .get_mut(index)
                .ok_or(DeviceContractError::MalformedState)?;
            *field = f32::from_le_bytes(bytes);
        }
        // Versions 1–5 permitted every stage up to 120s. Validate the old
        // domain before clamping; malformed values must remain errors.
        if state.state_version() < 6 {
            for operator in 0..OPERATORS {
                for (field, max) in [
                    (3, crate::operator_envelope::MAX_ATTACK_SECONDS),
                    (4, crate::operator_envelope::MAX_DECAY_SECONDS),
                    (5, crate::operator_envelope::MAX_RELEASE_SECONDS),
                ] {
                    let value = &mut fields[CORE_FIELDS + operator * ENVELOPE_FIELDS + field];
                    if !value.is_finite() || !(0.0..=120.0).contains(value) {
                        return Err(DeviceContractError::InvalidParameterValue);
                    }
                    *value = value.clamp(if field >= 4 { 0.001 } else { 0.0 }, max as f32);
                }
            }
        }
        let mut waveforms = [OperatorWaveform::Sine; OPERATORS];
        if state.state_version() >= WAVEFORM_STATE_VERSION {
            for (index, bytes) in state.payload()[LEGACY_PAYLOAD_BYTES..WAVEFORM_PAYLOAD_BYTES]
                .chunks_exact(size_of::<u32>())
                .enumerate()
            {
                let bytes: [u8; 4] = bytes
                    .try_into()
                    .map_err(|_| DeviceContractError::MalformedState)?;
                let choice = u32::from_le_bytes(bytes);
                if state.state_version() < CATALOG_STATE_VERSION && choice > 3 {
                    return Err(DeviceContractError::MalformedState);
                }
                waveforms[index] = OperatorWaveform::from_choice(choice)
                    .ok_or(DeviceContractError::MalformedState)?;
            }
        }
        let tuning = if state.state_version() >= TUNING_STATE_VERSION {
            tuning::TuningState::decode(
                &state.payload()[WAVEFORM_PAYLOAD_BYTES..TUNING_PAYLOAD_BYTES],
            )?
        } else {
            tuning::TuningState::default()
        };
        let mut amplitude_velocity = [0.0; OPERATORS];
        if state.state_version() >= 4 {
            for (index, bytes) in state.payload()[TUNING_PAYLOAD_BYTES..VELOCITY_PAYLOAD_BYTES]
                .chunks_exact(size_of::<f32>())
                .enumerate()
            {
                let bytes: [u8; 4] = bytes
                    .try_into()
                    .map_err(|_| DeviceContractError::MalformedState)?;
                amplitude_velocity[index] = f32::from_le_bytes(bytes);
            }
        }
        let mut feedback = [0.0; OPERATORS];
        if state.state_version() >= FEEDBACK_STATE_VERSION {
            for (index, bytes) in state.payload()[VELOCITY_PAYLOAD_BYTES..FEEDBACK_PAYLOAD_BYTES]
                .chunks_exact(size_of::<f32>())
                .enumerate()
            {
                let bytes: [u8; 4] = bytes
                    .try_into()
                    .map_err(|_| DeviceContractError::MalformedState)?;
                feedback[index] = f32::from_le_bytes(bytes);
            }
        }
        let lfo = if state.state_version() >= LFO_STATE_VERSION {
            lfo::LfoSettings::decode(&state.payload()[FEEDBACK_PAYLOAD_BYTES..LFO_PAYLOAD_BYTES])?
        } else {
            lfo::LfoSettings::default()
        };
        let enabled = if state.state_version() >= ENABLE_STATE_VERSION {
            enable::EnableSettings::decode(
                &state.payload()[LFO_PAYLOAD_BYTES..ENABLE_PAYLOAD_BYTES],
            )?
        } else {
            enable::EnableSettings::default()
        };
        let harmonics = if state.state_version() >= HARMONIC_STATE_VERSION {
            harmonics::Harmonics::decode(
                &state.payload()[FILTER_PAYLOAD_BYTES..HARMONIC_PAYLOAD_BYTES],
            )?
        } else {
            harmonics::Harmonics::default()
        };
        let filter = if state.state_version() >= 11 {
            crate::operator_filter::Settings::decode(
                &state.payload()[ENABLE_PAYLOAD_BYTES..FILTER_PAYLOAD_BYTES],
            )?
        } else {
            crate::operator_filter::Settings::default()
        };
        let companion = if state.state_version() >= COMPANION_STATE_VERSION {
            companion::Settings::decode(
                &state.payload()[HARMONIC_PAYLOAD_BYTES..COMPANION_PAYLOAD_BYTES],
            )?
        } else {
            companion::Settings::default()
        };
        let global = if state.state_version() >= GLOBAL_STATE_VERSION {
            global::Settings::decode(
                &state.payload()[COMPANION_PAYLOAD_BYTES..GLOBAL_PAYLOAD_BYTES],
            )?
        } else {
            global::Settings::legacy()
        };
        let modulation = if state.state_version() >= GLOBAL_STATE_VERSION {
            modulation::Settings::decode(
                &state.payload()[GLOBAL_PAYLOAD_BYTES..MODULATION_PAYLOAD_BYTES],
            )?
        } else {
            modulation::Settings::default()
        };
        let fine = if state.state_version() >= FINE_STATE_VERSION {
            fine::Settings::decode(&state.payload()[MODULATION_PAYLOAD_BYTES..FINE_PAYLOAD_BYTES])?
        } else {
            fine::Settings::default()
        };
        let pitch_keytrack = if state.state_version() >= KEYTRACK_STATE_VERSION {
            keytrack::Settings::decode(&state.payload()[FINE_PAYLOAD_BYTES..])?
        } else {
            keytrack::Settings::from_fixed(tuning.fixed_flags())
        };
        if tuning.fixed_flags() != pitch_keytrack.fixed_flags() {
            return Err(DeviceContractError::MalformedState);
        }
        let result = Self {
            pitch_keytrack,
            fine,
            modulation,
            global,
            companion,
            filter,
            fields,
            waveforms,
            tuning,
            amplitude_velocity,
            feedback,
            lfo,
            enabled,
            harmonics,
        };
        result.kernel_parts()?;
        Ok(result)
    }

    pub fn to_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        self.kernel_parts()?;
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(PAYLOAD_BYTES)
            .map_err(|_| DeviceContractError::RuntimeAllocation)?;
        for field in self.fields {
            payload.extend_from_slice(&field.to_le_bytes());
        }
        for waveform in self.waveforms {
            payload.extend_from_slice(&waveform.choice().to_le_bytes());
        }
        self.tuning.append(&mut payload);
        for amount in self.amplitude_velocity {
            payload.extend_from_slice(&amount.to_le_bytes());
        }
        for amount in self.feedback {
            payload.extend_from_slice(&amount.to_le_bytes());
        }
        self.lfo.append(&mut payload);
        self.enabled.append(&mut payload);
        self.filter.append(&mut payload);
        self.harmonics.append(&mut payload);
        self.companion.append(&mut payload);
        self.global.append(&mut payload);
        self.modulation.append(&mut payload);
        self.fine.append(&mut payload);
        self.pitch_keytrack.append(&mut payload);
        NativeDeviceState::new(OPERATOR_TYPE_ID, OPERATOR_STATE_VERSION, payload)
    }

    // Control preparation still runs the canonical encoder and preserves its
    // error ordering. Reuse only the exact validated type/version/byte owner;
    // migration or normalization must retain the newly encoded allocation.
    fn prepared_authored_state(
        &self,
        input: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let canonical = self.to_state()?;
        Ok(if input == &canonical {
            input.clone()
        } else {
            canonical
        })
    }

    /// Decode validated original oscillator, graph and envelope laws on control.
    pub fn kernel_parts(
        &self,
    ) -> Result<
        (
            [Oscillator; OPERATORS],
            Routing,
            [OperatorEnvelope; OPERATORS],
        ),
        DeviceContractError,
    > {
        self.tuning.modes()?;
        if self.tuning.fixed_flags() != self.pitch_keytrack.fixed_flags() {
            return Err(DeviceContractError::MalformedState);
        }
        self.lfo.validate()?;
        self.filter.validate()?;
        self.companion.validate()?;
        self.global.validate()?;
        self.modulation.validate()?;
        for descriptor in PARAMETERS.iter().take(FLOAT_PARAMETER_COUNT) {
            descriptor.validate(ParameterValue::Float(
                self.fields[descriptor.id.value() as usize - 1],
            ))?;
        }
        for (index, amount) in self.amplitude_velocity.iter().enumerate() {
            PARAMETERS
                .iter()
                .find(|descriptor| descriptor.id == parameter::amplitude_velocity(index as u32 + 1))
                .ok_or(DeviceContractError::UnknownParameter)?
                .validate(ParameterValue::Float(*amount))?;
        }
        for (index, amount) in self.feedback.iter().enumerate() {
            PARAMETERS
                .iter()
                .find(|descriptor| descriptor.id == parameter::feedback(index as u32 + 1))
                .ok_or(DeviceContractError::UnknownParameter)?
                .validate(ParameterValue::Float(*amount))?;
        }
        if (0..OPERATORS).any(|operator| self.fields[12 + operator * OPERATORS + operator] != 0.0) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let oscillators = std::array::from_fn(|operator| {
            let offset = operator * 3;
            Oscillator::new(
                f64::from(self.fields[offset]),
                if self.enabled.flags()[operator] {
                    f64::from(self.fields[offset + 1])
                } else {
                    0.0
                },
                f64::from(self.fields[offset + 2]),
            )
            .map_err(|_| DeviceContractError::InvalidParameterValue)
        });
        let [a, b, c, d] = oscillators;
        let routing = Routing::new(
            std::array::from_fn(|source| {
                std::array::from_fn(|target| {
                    f64::from(self.fields[12 + source * OPERATORS + target])
                })
            }),
            std::array::from_fn(|operator| f64::from(self.fields[28 + operator])),
        )
        .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        let envelopes = std::array::from_fn(|operator| {
            let offset = CORE_FIELDS + operator * ENVELOPE_FIELDS;
            OperatorEnvelope::new(
                f64::from(self.fields[offset]),
                f64::from(self.fields[offset + 1]),
                f64::from(self.fields[offset + 2]),
                f64::from(self.fields[offset + 3]),
                f64::from(self.fields[offset + 4]),
                f64::from(self.fields[offset + 5]),
            )
            .map_err(|_| DeviceContractError::InvalidParameterValue)
        });
        let [ea, eb, ec, ed] = envelopes;
        Ok(([a?, b?, c?, d?], routing, [ea?, eb?, ec?, ed?]))
    }

    fn routing_preset(&self) -> OperatorRoutingPreset {
        for preset in [
            OperatorRoutingPreset::Serial,
            OperatorRoutingPreset::Parallel,
            OperatorRoutingPreset::TwoPairs,
        ] {
            if let Some(expected) = preset.routing_fields()
                && self.fields[12..32] == expected
            {
                return preset;
            }
        }
        OperatorRoutingPreset::Custom
    }

    fn apply_routing_preset(&mut self, preset: OperatorRoutingPreset) {
        if let Some(fields) = preset.routing_fields() {
            self.fields[12..32].copy_from_slice(&fields);
        }
    }

    fn amplitude_velocity_index(parameter: ParameterId) -> Option<usize> {
        let index = parameter.value().checked_sub(74)? as usize;
        (index < OPERATORS).then_some(index)
    }

    fn feedback_index(parameter: ParameterId) -> Option<usize> {
        let index = parameter.value().checked_sub(78)? as usize;
        (index < FEEDBACK_COUNT).then_some(index)
    }

    fn feedback_enabled(&self, operator: usize) -> bool {
        operator < OPERATORS
            && (0..OPERATORS).all(|source| {
                source == operator || self.fields[12 + source * OPERATORS + operator] == 0.0
            })
    }

    fn effective_feedback(&self) -> [f32; OPERATORS] {
        std::array::from_fn(|operator| {
            if self.feedback_enabled(operator) {
                self.feedback[operator]
            } else {
                0.0
            }
        })
    }

    /// Standalone control preparation only; no native sampler/registry admission.
    pub fn prepare_configuration(
        &self,
        sample_rate: u32,
    ) -> Result<PreparedOperatorConfiguration, DeviceContractError> {
        let (oscillators, routing, envelopes) = self.kernel_parts()?;
        let fixed_anchors = self.tuning.fixed_anchors()?;
        PreparedOperatorConfiguration::new_with_tuning_and_amplitude_velocity(
            sample_rate,
            oscillators,
            routing,
            envelopes,
            self.waveforms,
            self.tuning.modes()?,
            self.amplitude_velocity,
        )
        .and_then(|configuration| {
            configuration.with_note_modifiers(
                std::array::from_fn(|i| self.companion.float(i, 4) as f32),
                std::array::from_fn(|i| self.companion.float(i, 1) as f32),
                std::array::from_fn(|i| self.companion.words[i][2] != 0),
            )
        })
        .and_then(|configuration| configuration.with_fine_offsets(self.fine.values()))
        .and_then(|configuration| {
            configuration.with_pitch_keytrack(self.pitch_keytrack.values(), fixed_anchors)
        })
        .map_err(|_| DeviceContractError::InvalidParameterValue)
    }

    /// Standalone control preparation only. Free-run and loops require native runtime clocks.
    pub fn prepare_note(
        &self,
        sample_rate: u32,
        pitch: u8,
        velocity: u8,
    ) -> Result<PreparedOperatorFm4, DeviceContractError> {
        if !self.companion.standalone_supported() {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        let configuration = self.prepare_configuration(sample_rate)?;
        let envelopes = self.companion.envelopes(&self.fields, velocity)?;
        configuration
            .standalone_note(pitch, velocity, envelopes)
            .map_err(|_| DeviceContractError::InvalidParameterValue)
    }
}

#[derive(Clone, Copy)]
pub(super) struct FilterVoiceSeed {
    note: PreparedOperatorNote,
    age: u64,
    gate: Option<u64>,
    previous: [f64; 4],
    id: u64,
}

/// Generic Operator control metadata and shared analytic instrument definition.
#[derive(Clone, Copy, Debug, Default)]
pub struct OperatorMetadata;
pub static OPERATOR_METADATA: OperatorMetadata = OperatorMetadata;

impl NativeDeviceMetadata for OperatorMetadata {
    fn type_id(&self) -> DeviceTypeId {
        OPERATOR_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("vector")
    }
    fn state_version(&self) -> u32 {
        OPERATOR_STATE_VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &PARAMETERS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        OperatorSettings::defaults().to_state()
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let settings = OperatorSettings::from_state(state)?;
        settings.to_state()
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let settings = OperatorSettings::from_state(state)?;
        if let Some(value) = settings.pitch_keytrack.read(parameter) {
            return Ok(value);
        }
        if let Some(value) = settings.fine.read(parameter) {
            return Ok(value);
        }
        if let Some(value) = settings.modulation.read(parameter) {
            return Ok(value);
        }
        if let Some(value) = settings.global.read(parameter) {
            return Ok(value);
        }
        if let Some(value) = settings.companion.read(parameter) {
            return Ok(value);
        }
        if let Some(value) = settings.filter.read(parameter) {
            return Ok(value);
        }
        if let Some(value) = settings.harmonics.read(parameter, settings.waveforms)? {
            return Ok(value);
        }
        if let Some(value) = settings.enabled.read(parameter) {
            return Ok(value);
        }
        if let Some(value) = settings.lfo.read(parameter) {
            return Ok(value);
        }
        if let Some(value) = settings.tuning.read(parameter) {
            return Ok(value);
        }
        if parameter == parameter::ALGORITHM {
            return Ok(ParameterValue::Choice(algorithms::choice(
                &settings.fields[12..32],
            )));
        }
        if parameter == parameter::routing_preset() {
            return Ok(ParameterValue::Choice(settings.routing_preset().choice()));
        }
        if let Some(index) = OperatorSettings::amplitude_velocity_index(parameter) {
            return settings
                .amplitude_velocity
                .get(index)
                .copied()
                .map(ParameterValue::Float)
                .ok_or(DeviceContractError::UnknownParameter);
        }
        if let Some(index) = OperatorSettings::feedback_index(parameter) {
            return settings
                .feedback
                .get(index)
                .copied()
                .map(ParameterValue::Float)
                .ok_or(DeviceContractError::UnknownParameter);
        }
        if let Some(index) = waveform_index(parameter) {
            return settings
                .waveforms
                .get(index)
                .copied()
                .map(|waveform| {
                    ParameterValue::Choice(if settings.harmonics.custom[index] {
                        4
                    } else {
                        waveform.choice()
                    })
                })
                .ok_or(DeviceContractError::UnknownParameter);
        }
        settings
            .fields
            .get(parameter_index(parameter)?)
            .copied()
            .map(ParameterValue::Float)
            .ok_or(DeviceContractError::UnknownParameter)
    }
    fn parameter_enabled(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<bool, DeviceContractError> {
        let settings = OperatorSettings::from_state(state)?;
        if !settings.global.active
            && ((601..=607).contains(&parameter.value())
                || (620..=639).contains(&parameter.value()))
        {
            return Ok(false);
        }
        if let Some(index) = OperatorSettings::feedback_index(parameter) {
            return Ok(settings.feedback_enabled(index));
        }
        let descriptor = PARAMETERS
            .iter()
            .find(|descriptor| descriptor.id == parameter)
            .ok_or(DeviceContractError::UnknownParameter)?;
        descriptor.validate(self.read_parameter(state, parameter)?)?;
        Ok(true)
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let descriptor = PARAMETERS
            .iter()
            .find(|descriptor| descriptor.id == parameter)
            .ok_or(DeviceContractError::UnknownParameter)?;
        descriptor.validate(value)?;
        let mut settings = OperatorSettings::from_state(state)?;
        if settings.pitch_keytrack.patch(parameter, value)? {
            settings
                .tuning
                .set_fixed_flags(settings.pitch_keytrack.fixed_flags());
            return settings.to_state();
        }
        if settings.fine.patch(parameter, value)? {
            return settings.to_state();
        }
        if settings.modulation.patch(parameter, value)? {
            settings.global.active = true;
            return settings.to_state();
        }
        if settings.global.patch(parameter, value)? {
            return settings.to_state();
        }
        if settings.companion.patch(parameter, value) {
            return settings.to_state();
        }
        if settings.filter.patch(parameter, value)? {
            return settings.to_state();
        }
        if settings
            .harmonics
            .patch(parameter, value, settings.waveforms)?
        {
            return settings.to_state();
        }
        if settings.enabled.patch(parameter, value)? {
            return settings.to_state();
        }
        if settings.lfo.patch(parameter, value) {
            return settings.to_state();
        }
        if let Some(index) = OperatorSettings::feedback_index(parameter) {
            let ParameterValue::Float(amount) = value else {
                return Err(DeviceContractError::InvalidParameterValue);
            };
            let field = settings
                .feedback
                .get_mut(index)
                .ok_or(DeviceContractError::UnknownParameter)?;
            *field = amount;
            return settings.to_state();
        }
        if settings.tuning.patch(parameter, value)? {
            for operator in 1..=OPERATORS as u32 {
                if parameter == parameter::fixed(operator) {
                    let fixed = settings.tuning.fixed_flags()[operator as usize - 1];
                    settings.pitch_keytrack.patch(
                        parameter::pitch_keytrack(operator),
                        ParameterValue::Float(if fixed { 0.0 } else { 100.0 }),
                    )?;
                }
            }
            return settings.to_state();
        }
        if parameter == parameter::ALGORITHM {
            let ParameterValue::Choice(choice) = value else {
                return Err(DeviceContractError::InvalidParameterValue);
            };
            if let Some(algorithm) = algorithms::from_choice(choice)? {
                settings.global.active = true;
                settings.fields[12..32].copy_from_slice(&algorithm.routing_fields());
            }
            return settings.to_state();
        }
        if parameter == parameter::routing_preset() {
            let ParameterValue::Choice(choice) = value else {
                return Err(DeviceContractError::InvalidParameterValue);
            };
            let preset = OperatorRoutingPreset::from_choice(choice)
                .ok_or(DeviceContractError::InvalidParameterValue)?;
            settings.apply_routing_preset(preset);
            return settings.to_state();
        }
        if let Some(index) = OperatorSettings::amplitude_velocity_index(parameter) {
            let ParameterValue::Float(amount) = value else {
                return Err(DeviceContractError::InvalidParameterValue);
            };
            let field = settings
                .amplitude_velocity
                .get_mut(index)
                .ok_or(DeviceContractError::UnknownParameter)?;
            *field = amount;
            return settings.to_state();
        }
        if let Some(index) = waveform_index(parameter) {
            let ParameterValue::Choice(value) = value else {
                return Err(DeviceContractError::InvalidParameterValue);
            };
            if value == 4 {
                if !settings.harmonics.authored[index] {
                    settings.harmonics.seed(index, settings.waveforms[index])?;
                }
                settings.harmonics.custom[index] = true;
                settings.harmonics.authored[index] = true;
            } else {
                settings.waveforms[index] = OperatorWaveform::from_choice(value)
                    .ok_or(DeviceContractError::InvalidParameterValue)?;
                settings.harmonics.custom[index] = false;
            }
            return settings.to_state();
        }
        let ParameterValue::Float(value) = value else {
            return Err(DeviceContractError::InvalidParameterValue);
        };
        let field = settings
            .fields
            .get_mut(parameter_index(parameter)?)
            .ok_or(DeviceContractError::UnknownParameter)?;
        *field = value;
        settings.to_state()
    }
}

impl NativeInstrumentDefinition for OperatorMetadata {
    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        let sample_rate = context.sample_rate;
        let tuning = context.tuning;
        let settings = OperatorSettings::from_state(state)?;
        let configuration = settings.prepare_configuration(sample_rate)?;
        let authored_state = settings.prepared_authored_state(state)?;
        let effective_levels = *configuration
            .feedback_parameters([0.0; OPERATORS])
            .map_err(|_| DeviceContractError::InvalidParameterValue)?
            .oscillator_levels();
        let continuation_format =
            hex_lattice::AnalyticContinuationFormat::new(OPERATOR_TYPE_ID, 2)?;
        // The shared table is initialized on the control side before any
        // callback can borrow it for note sampling.
        let sine_table = crate::shared_sine_table();
        let lfo_runtime = lfo_runtime::prepare_factory(
            &settings,
            configuration.clone(),
            sample_rate,
            sine_table,
        )?;
        let feedback_runtime = feedback_runtime::prepare_factory(
            &settings,
            configuration.clone(),
            sample_rate,
            sine_table,
        )?;
        Ok(Box::new(PreparedOperatorInstrument {
            configuration: configuration.clone(),
            authored_state,
            effective_levels,
            continuation_format,
            sine_table,
            feedback_runtime,
            lfo_runtime,
            harmonic_runtime: harmonic_runtime::Factory::new_with_tuning(
                settings,
                configuration.clone(),
                sine_table,
                sample_rate,
                (!tuning.is_neutral()).then_some(*tuning),
            )?,
        }))
    }
}

struct PreparedOperatorInstrument {
    configuration: PreparedOperatorConfiguration,
    authored_state: NativeDeviceState,
    effective_levels: [f64; OPERATORS],
    continuation_format: hex_lattice::AnalyticContinuationFormat,
    sine_table: &'static SineTable,
    feedback_runtime: Option<feedback_runtime::OperatorFeedbackFactory>,
    lfo_runtime: Option<lfo_runtime::OperatorLfoFactory>,
    harmonic_runtime: harmonic_runtime::Factory,
}

impl PreparedNativeInstrument for PreparedOperatorInstrument {
    fn prefers_mutable_continuation(&self) -> bool {
        self.harmonic_runtime.tuning.is_some()
            || self.harmonic_runtime.settings.fine.values() != [0.0; OPERATORS]
            || self
                .harmonic_runtime
                .settings
                .pitch_keytrack
                .values()
                .iter()
                .any(|v| *v != 0.0 && *v != 100.0)
            || self
                .harmonic_runtime
                .settings
                .harmonics
                .custom
                .iter()
                .any(|v| *v)
            || self.harmonic_runtime.settings.companion.requires_runtime()
            || self.harmonic_runtime.settings.global.active
    }
    fn prepare_mutable_continuation_runtime(
        &self,
    ) -> Result<Option<Box<dyn hex_lattice::PreparedMutableInstrumentRuntime>>, DeviceContractError>
    {
        Ok(Some(Box::new(self.harmonic_runtime.clone())))
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn validate_event_note(&self, pitch: u8, velocity: u8) -> Result<(), DeviceContractError> {
        if self.harmonic_runtime.tuning.is_some() {
            // Custom tuning selects the harmonic runtime even for plain/LFO
            // voices. Admit exactly its tuned oscillator/excursion domain.
            let tuning = self.harmonic_runtime.tuning.as_ref();
            if self.harmonic_runtime.settings.lfo.active() {
                self.configuration
                    .prepare_lfo_note_with_tuning(pitch, velocity, tuning)
            } else {
                self.configuration
                    .prepare_note_with_tuning(pitch, velocity, tuning)
            }
            .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        } else if let Some(factory) = self.lfo_runtime.as_ref() {
            factory
                .prepare_note_for_runtime(pitch, velocity)
                .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        } else if self.harmonic_runtime.settings.lfo.active() {
            self.configuration
                .prepare_lfo_note(pitch, velocity)
                .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        } else {
            self.configuration
                .prepare_note(pitch, velocity)
                .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        }
        Ok(())
    }

    fn prepare_note(&self, _: u8, _: u8) -> Result<PreparedAnalyticNote, DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }

    fn prepare_shared_analytic_context(
        &self,
    ) -> Result<Option<Box<dyn PreparedSharedAnalyticContext>>, DeviceContractError> {
        if self.prefers_mutable_continuation()
            || self.feedback_runtime.is_some()
            || self.lfo_runtime.is_some()
        {
            // Mutable feedback must never be silently rendered through the
            // legacy analytic sampler. Event-source compilation owns this
            // instrument through `prepare_mutable_runtime` instead.
            return Ok(None);
        }
        Ok(Some(Box::new(PreparedOperatorContext {
            configuration: self.configuration.clone(),
            authored_state: self.authored_state.clone(),
            effective_levels: self.effective_levels,
            continuation_format: self.continuation_format,
            sine_table: self.sine_table,
            companion: self.harmonic_runtime.settings.companion,
        })))
    }

    fn prepare_mutable_runtime(
        &self,
    ) -> Result<Option<Box<dyn hex_lattice::PreparedMutableInstrumentRuntime>>, DeviceContractError>
    {
        if self.prefers_mutable_continuation() {
            return self.prepare_mutable_continuation_runtime();
        }
        if let Some(factory) = self.lfo_runtime.as_ref() {
            return Ok(Some(
                Box::new(factory.clone()) as Box<dyn hex_lattice::PreparedMutableInstrumentRuntime>
            ));
        }
        Ok(self.feedback_runtime.as_ref().map(|factory| {
            Box::new(factory.clone()) as Box<dyn hex_lattice::PreparedMutableInstrumentRuntime>
        }))
    }
}

pub(super) struct PreparedOperatorContext {
    companion: companion::Settings,
    configuration: PreparedOperatorConfiguration,
    authored_state: NativeDeviceState,
    effective_levels: [f64; OPERATORS],
    continuation_format: hex_lattice::AnalyticContinuationFormat,
    sine_table: &'static SineTable,
}

impl PreparedSharedAnalyticContext for PreparedOperatorContext {
    fn analytic_continuation(&self) -> Option<&dyn hex_lattice::PreparedAnalyticContinuation> {
        Some(self)
    }

    fn import_identity(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
    fn declared_context_bytes(&self) -> usize {
        // Charge the canonical payload even while the control factory shares
        // its Arc. The allocation includes the two reference-count words.
        size_of::<Self>() + self.authored_state.payload().len() + 2 * size_of::<usize>()
    }

    fn prepare_note(
        &self,
        pitch: u8,
        velocity: u8,
    ) -> Result<PreparedSharedAnalyticNote, DeviceContractError> {
        let note = self
            .configuration
            .prepare_note(pitch, velocity)
            .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        Ok(PreparedSharedAnalyticNote::new(
            note.into_shared_payload(),
            self.configuration.release_frames(),
        ))
    }

    fn sample(
        &self,
        note: &PreparedSharedAnalyticNote,
        age_frames: u64,
        gate_frames: u64,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> StereoFrame {
        if note.release_frames() != self.configuration.release_frames() {
            return [0.0; 2];
        }
        let Some(note) = PreparedOperatorNote::from_shared_payload(note.payload()) else {
            return [0.0; 2];
        };
        if !self.configuration.note_matches_configuration(&note) {
            return [0.0; 2];
        }
        let sample = self
            .configuration
            .sample(self.sine_table, &note, age_frames, gate_frames);
        [sample, sample]
    }
}

fn parameter_index(parameter: ParameterId) -> Result<usize, DeviceContractError> {
    if !PARAMETERS
        .iter()
        .take(FLOAT_PARAMETER_COUNT)
        .any(|descriptor| descriptor.id == parameter)
    {
        return Err(DeviceContractError::UnknownParameter);
    }
    usize::try_from(parameter.value())
        .ok()
        .and_then(|id| id.checked_sub(1))
        .ok_or(DeviceContractError::UnknownParameter)
}

fn waveform_index(parameter: ParameterId) -> Option<usize> {
    let index = parameter.value().checked_sub(57)? as usize;
    (index < OPERATORS).then_some(index)
}

#[cfg(test)]
mod canonical_reuse_tests;
#[cfg(test)]
mod capture_proof;
#[cfg(test)]
mod enable_continuation_tests;
#[cfg(test)]
mod enable_tests;
#[cfg(test)]
mod fine_tests;
#[cfg(test)]
mod fixed_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod filter_runtime_tests;
