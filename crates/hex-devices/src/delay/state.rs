//! Delay's versioned opaque state and scalar laws. No project/host ownership.
//! Version 1 is the exact three-f32 minimal delay on this branch: milliseconds,
//! feedback and mix. Migration preserves its free stereo timing and dry/wet,
//! disables the new filter and modulation, and retains its repitch transition.
//! Version 2 adds the independently authored Live-style control inventory.
//! Feedback, dry/wet, filter frequency/width all eight LFO controls and linked timing expose shared owned-transaction automation; other
//! parameters retain their explicit unsupported declaration.
use super::DelayDefinition;
use hex_lattice_contract::*;
pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.delay");
pub const STATE_VERSION: u32 = 2;
pub const TIME: ParameterId = ParameterId::from_static(1);
pub const FEEDBACK: ParameterId = ParameterId::from_static(2);
pub const DRY_WET: ParameterId = ParameterId::from_static(3);
pub const RIGHT_TIME: ParameterId = ParameterId::from_static(4);
pub const LEFT_SYNC: ParameterId = ParameterId::from_static(5);
pub const RIGHT_SYNC: ParameterId = ParameterId::from_static(6);
pub const LEFT_DIVISION: ParameterId = ParameterId::from_static(7);
pub const RIGHT_DIVISION: ParameterId = ParameterId::from_static(8);
pub const LEFT_OFFSET: ParameterId = ParameterId::from_static(9);
pub const RIGHT_OFFSET: ParameterId = ParameterId::from_static(10);
pub const LINK: ParameterId = ParameterId::from_static(11);
pub const SMOOTHING: ParameterId = ParameterId::from_static(12);
pub const PING_PONG: ParameterId = ParameterId::from_static(13);
pub const FREEZE: ParameterId = ParameterId::from_static(14);
pub const FILTER: ParameterId = ParameterId::from_static(15);
pub const FREQUENCY: ParameterId = ParameterId::from_static(16);
pub const WIDTH: ParameterId = ParameterId::from_static(17);
pub const LFO_DELAY: ParameterId = ParameterId::from_static(18);
pub const LFO_FILTER: ParameterId = ParameterId::from_static(19);
pub const LFO_MODE: ParameterId = ParameterId::from_static(20);
pub const LFO_RATE: ParameterId = ParameterId::from_static(21);
pub const LFO_TIME: ParameterId = ParameterId::from_static(22);
pub const LFO_DIVISION: ParameterId = ParameterId::from_static(23);
pub const LFO_WAVE: ParameterId = ParameterId::from_static(24);
pub const LFO_MORPH: ParameterId = ParameterId::from_static(25);
pub const HIGH_QUALITY: ParameterId = ParameterId::from_static(26);
pub const EQUAL_LOUDNESS: ParameterId = ParameterId::from_static(27);
pub(super) const COUNT: usize = 27;
pub(super) const DESCRIPTORS: &[ParameterDescriptor] = &[
    ParameterDescriptor {
        id: TIME,
        label: "left time",
        kind: ParameterKind::Float {
            min: 1.0,
            max: 5000.0,
            unit: Some("ms"),
        },
        default: ParameterValue::Float(375.0),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: FEEDBACK,
        label: "feedback",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 0.95,
            unit: None,
        },
        default: ParameterValue::Float(0.5),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: DRY_WET,
        label: "dry/wet",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(0.5),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: RIGHT_TIME,
        label: "right time",
        kind: ParameterKind::Float {
            min: 1.0,
            max: 5000.0,
            unit: Some("ms"),
        },
        default: ParameterValue::Float(375.0),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: LEFT_SYNC,
        label: "left sync",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(true),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: RIGHT_SYNC,
        label: "right sync",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(true),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: LEFT_DIVISION,
        label: "left division",
        kind: ParameterKind::Choice {
            labels: &["1", "2", "3", "4", "5", "6", "8", "16"],
        },
        default: ParameterValue::Choice(2),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: RIGHT_DIVISION,
        label: "right division",
        kind: ParameterKind::Choice {
            labels: &["1", "2", "3", "4", "5", "6", "8", "16"],
        },
        default: ParameterValue::Choice(2),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: LEFT_OFFSET,
        label: "left offset",
        kind: ParameterKind::Float {
            min: -33.3,
            max: 33.3,
            unit: Some("%"),
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: RIGHT_OFFSET,
        label: "right offset",
        kind: ParameterKind::Float {
            min: -33.3,
            max: 33.3,
            unit: Some("%"),
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: LINK,
        label: "link",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(true),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: SMOOTHING,
        label: "smoothing",
        kind: ParameterKind::Choice {
            labels: &["repitch", "fade", "jump"],
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: PING_PONG,
        label: "ping pong",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: FREEZE,
        label: "freeze",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: FILTER,
        label: "filter",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(true),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: FREQUENCY,
        label: "freq",
        kind: ParameterKind::Float {
            min: 20.0,
            max: 20000.0,
            unit: Some("hz"),
        },
        default: ParameterValue::Float(1000.0),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: WIDTH,
        label: "width",
        kind: ParameterKind::Float {
            min: 0.5,
            max: 9.0,
            unit: Some("oct"),
        },
        default: ParameterValue::Float(8.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: LFO_DELAY,
        label: "delay mod",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: LFO_FILTER,
        label: "filter mod",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: LFO_MODE,
        label: "lfo mode",
        kind: ParameterKind::Choice {
            labels: &["rate", "time", "synced", "triplet", "dotted", "sixteenth"],
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: LFO_RATE,
        label: "lfo rate",
        kind: ParameterKind::Float {
            min: 0.01,
            max: 40.0,
            unit: Some("hz"),
        },
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: LFO_TIME,
        label: "lfo time",
        kind: ParameterKind::Float {
            min: 10.0,
            max: 60000.0,
            unit: Some("ms"),
        },
        default: ParameterValue::Float(1000.0),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: LFO_DIVISION,
        label: "lfo division",
        kind: ParameterKind::Choice {
            labels: &[
                "1/64", "1/32", "1/16", "1/8", "1/4", "1/2", "1", "2", "4", "8", "16",
            ],
        },
        default: ParameterValue::Choice(4),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: LFO_WAVE,
        label: "wave",
        kind: ParameterKind::Choice {
            labels: &[
                "sine",
                "triangle",
                "ramp up",
                "ramp down",
                "square",
                "s&h",
                "wander",
            ],
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: LFO_MORPH,
        label: "morph",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(0.5),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: HIGH_QUALITY,
        label: "hi quality",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(true),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: EQUAL_LOUDNESS,
        label: "equal loudness",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
];
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Settings(pub [f32; COUNT]);
impl Default for Settings {
    fn default() -> Self {
        Self(std::array::from_fn(|i| match DESCRIPTORS[i].default {
            ParameterValue::Float(v) => v,
            ParameterValue::Bool(v) => f32::from(v),
            ParameterValue::Choice(v) => v as f32,
        }))
    }
}
impl Settings {
    pub(super) fn value(self, id: ParameterId) -> f32 {
        self.0[id.value() as usize - 1]
    }
    pub(super) fn flag(self, id: ParameterId) -> bool {
        self.value(id) == 1.0
    }
    pub(super) fn choice(self, id: ParameterId) -> u32 {
        self.value(id) as u32
    }
    pub(super) fn set(&mut self, id: ParameterId, value: f32) {
        self.0[id.value() as usize - 1] = value;
    }
    pub(super) fn from_legacy(values: [f32; 3]) -> Self {
        let mut s = Self::default();
        s.0[..3].copy_from_slice(&values);
        s.set(RIGHT_TIME, values[0]);
        s.set(LEFT_SYNC, 0.0);
        s.set(RIGHT_SYNC, 0.0);
        s.set(FILTER, 0.0);
        s.set(HIGH_QUALITY, 0.0);
        s
    }
}
fn parameter(settings: Settings, index: usize) -> Result<ParameterValue, DeviceContractError> {
    let v = settings.0[index];
    if !v.is_finite() {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    let value = match DESCRIPTORS[index].kind {
        ParameterKind::Float { .. } => ParameterValue::Float(v),
        ParameterKind::Bool if v == 0.0 || v == 1.0 => ParameterValue::Bool(v == 1.0),
        ParameterKind::Choice { .. } if v >= 0.0 && v.fract() == 0.0 => {
            ParameterValue::Choice(v as u32)
        }
        _ => return Err(DeviceContractError::InvalidParameterValue),
    };
    DESCRIPTORS[index].validate(value)?;
    Ok(value)
}
pub(super) fn decode(state: &NativeDeviceState) -> Result<Settings, DeviceContractError> {
    if state.type_id() != TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    let count = match state.state_version() {
        1 => 3,
        STATE_VERSION => COUNT,
        _ => return Err(DeviceContractError::UnsupportedStateVersion),
    };
    if state.payload().len() != count * 4 {
        return Err(DeviceContractError::MalformedState);
    }
    let mut s = Settings::default();
    for (value, bytes) in s.0.iter_mut().zip(state.payload().chunks_exact(4)) {
        *value = f32::from_le_bytes(
            bytes
                .try_into()
                .map_err(|_| DeviceContractError::MalformedState)?,
        );
    }
    if count == 3 {
        if !(1.0..=1000.0).contains(&s.0[0]) || !(0.0..=0.85).contains(&s.0[1]) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        s = Settings::from_legacy([s.0[0], s.0[1], s.0[2]]);
    }
    for index in 0..COUNT {
        parameter(s, index)?;
    }
    if s.flag(LINK)
        && [
            (TIME, RIGHT_TIME),
            (LEFT_SYNC, RIGHT_SYNC),
            (LEFT_DIVISION, RIGHT_DIVISION),
        ]
        .iter()
        .any(|(a, b)| s.value(*a) != s.value(*b))
    {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    Ok(s)
}
pub(super) fn encode(s: Settings) -> Result<NativeDeviceState, DeviceContractError> {
    let mut bytes = Vec::with_capacity(COUNT * 4);
    for index in 0..COUNT {
        parameter(s, index)?;
        bytes.extend_from_slice(&s.0[index].to_le_bytes());
    }
    NativeDeviceState::new(TYPE_ID, STATE_VERSION, bytes)
}
impl NativeDeviceMetadata for DelayDefinition {
    fn type_id(&self) -> DeviceTypeId {
        TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("delay")
    }
    fn state_version(&self) -> u32 {
        STATE_VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        encode(Settings::default())
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        encode(decode(state)?)
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let s = decode(state)?;
        let index = DESCRIPTORS
            .iter()
            .position(|d| d.id == id)
            .ok_or(DeviceContractError::UnknownParameter)?;
        parameter(s, index)
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut s = decode(state)?;
        let index = DESCRIPTORS
            .iter()
            .position(|d| d.id == id)
            .ok_or(DeviceContractError::UnknownParameter)?;
        DESCRIPTORS[index].validate(value)?;
        let v = match value {
            ParameterValue::Float(v) => v,
            ParameterValue::Bool(v) => f32::from(v),
            ParameterValue::Choice(v) => v as f32,
        };
        s.set(id, v);
        if id == LINK && s.flag(LINK) {
            for (left, right) in [
                (TIME, RIGHT_TIME),
                (LEFT_SYNC, RIGHT_SYNC),
                (LEFT_DIVISION, RIGHT_DIVISION),
            ] {
                s.set(right, s.value(left));
            }
        } else if s.flag(LINK) {
            for (left, right) in [
                (TIME, RIGHT_TIME),
                (LEFT_SYNC, RIGHT_SYNC),
                (LEFT_DIVISION, RIGHT_DIVISION),
            ] {
                if id == left {
                    s.set(right, v);
                } else if id == right {
                    s.set(left, v);
                }
            }
        }
        encode(s)
    }
    fn parameter_semantics_for_state(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<Option<ParameterSemantics>, DeviceContractError> {
        decode(state)?;
        if let Some(semantics) = super::automation::semantics(id) {
            return Ok(Some(semantics));
        }
        let d = DESCRIPTORS
            .iter()
            .find(|d| d.id == id)
            .ok_or(DeviceContractError::UnknownParameter)?;
        let (normalization, stepping) = match d.kind {
            ParameterKind::Bool => (ParameterNormalization::Discrete, ParameterStepping::Boolean),
            ParameterKind::Choice { .. } => {
                (ParameterNormalization::Discrete, ParameterStepping::Choice)
            }
            _ => (
                if d.presentation == ParameterPresentation::Logarithmic {
                    ParameterNormalization::PositiveLogarithmic
                } else {
                    ParameterNormalization::Linear
                },
                ParameterStepping::Continuous,
            ),
        };
        Ok(Some(ParameterSemantics {
            access: ParameterAccess::ReadWrite,
            normalization,
            stepping,
            automation: ParameterAutomation::Unsupported,
            modulation: ParameterModulation::Unsupported,
            smoothing: ParameterSmoothing::DeviceManaged,
            change_impact: ParameterChangeImpact::ValueOnly,
        }))
    }
}
