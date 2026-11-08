//! Strict version-1 twelve-parameter opaque payload; no saved-project model change.
//! Inactive values (including independent l/r and m/s links) remain authored state.
//! Nine scalar/switch controls expose shared envelope delivery without changing
//! latency or reconstructing fixed detector coefficients on the callback.
use super::LimiterDefinition;
use hex_lattice_contract::*;
pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.limiter");
pub const STATE_VERSION: u32 = 1;
pub mod parameter {
    use hex_lattice_contract::ParameterId;
    pub const GAIN: ParameterId = ParameterId::from_static(1);
    pub const CEILING: ParameterId = ParameterId::from_static(2);
    pub const RELEASE: ParameterId = ParameterId::from_static(3);
    pub const AUTO_RELEASE: ParameterId = ParameterId::from_static(4);
    pub const LINK: ParameterId = ParameterId::from_static(5);
    pub const LOOKAHEAD: ParameterId = ParameterId::from_static(6);
    pub const ROUTING: ParameterId = ParameterId::from_static(7);
    pub const MODE: ParameterId = ParameterId::from_static(8);
    pub const MAXIMIZE: ParameterId = ParameterId::from_static(9);
    pub const THRESHOLD: ParameterId = ParameterId::from_static(10);
    pub const OUTPUT: ParameterId = ParameterId::from_static(11);
    pub const LINK_MID_SIDE: ParameterId = ParameterId::from_static(12);
}
use parameter::*;
pub(super) const COUNT: usize = 12;
pub(super) const DESCRIPTORS: &[ParameterDescriptor] = &[
    ParameterDescriptor {
        id: GAIN,
        label: "gain",
        kind: ParameterKind::Float {
            min: -24.0,
            max: 24.0,
            unit: Some("db"),
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: CEILING,
        label: "ceiling",
        kind: ParameterKind::Float {
            min: -24.0,
            max: 0.0,
            unit: Some("db"),
        },
        default: ParameterValue::Float(-0.3),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: RELEASE,
        label: "release",
        kind: ParameterKind::Float {
            min: 0.01,
            max: 3000.0,
            unit: Some("ms"),
        },
        default: ParameterValue::Float(100.0),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: AUTO_RELEASE,
        label: "auto",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: LINK,
        label: "link",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: LOOKAHEAD,
        label: "lookahead",
        kind: ParameterKind::Choice {
            labels: &["1.5 ms", "3 ms", "6 ms"],
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: ROUTING,
        label: "routing",
        kind: ParameterKind::Choice {
            labels: &["l/r", "m/s"],
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: MODE,
        label: "mode",
        kind: ParameterKind::Choice {
            labels: &["standard", "soft clip", "true peak"],
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: MAXIMIZE,
        label: "maximize",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: THRESHOLD,
        label: "threshold",
        kind: ParameterKind::Float {
            min: -24.0,
            max: 24.0,
            unit: Some("db"),
        },
        default: ParameterValue::Float(-0.3),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: OUTPUT,
        label: "output",
        kind: ParameterKind::Float {
            min: -24.0,
            max: 0.0,
            unit: Some("db"),
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: LINK_MID_SIDE,
        label: "link m/s",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
];
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Settings(pub [f32; COUNT]);
impl Settings {
    pub(super) fn value(self, id: ParameterId) -> f32 {
        self.0[id.value() as usize - 1]
    }
    pub(super) fn flag(self, id: ParameterId) -> bool {
        self.value(id) != 0.0
    }
    pub(super) fn choice(self, id: ParameterId) -> u32 {
        self.value(id) as u32
    }
}
fn descriptor(id: ParameterId) -> Result<&'static ParameterDescriptor, DeviceContractError> {
    DESCRIPTORS
        .iter()
        .find(|d| d.id == id)
        .ok_or(DeviceContractError::UnknownParameter)
}
fn from_scalar(d: &ParameterDescriptor, value: f32) -> Result<ParameterValue, DeviceContractError> {
    let value = match d.kind {
        ParameterKind::Float { .. } => ParameterValue::Float(value),
        ParameterKind::Bool if value == 0.0 || value == 1.0 => ParameterValue::Bool(value == 1.0),
        ParameterKind::Choice { .. }
            if value.is_finite() && value >= 0.0 && value.fract() == 0.0 =>
        {
            ParameterValue::Choice(value as u32)
        }
        _ => return Err(DeviceContractError::InvalidParameterValue),
    };
    d.validate(value)?;
    Ok(value)
}
fn scalar(value: ParameterValue) -> f32 {
    match value {
        ParameterValue::Float(v) => v,
        ParameterValue::Bool(v) => f32::from(v),
        ParameterValue::Choice(v) => v as f32,
    }
}
pub(super) fn decode(state: &NativeDeviceState) -> Result<Settings, DeviceContractError> {
    if state.type_id() != TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != STATE_VERSION {
        return Err(DeviceContractError::UnsupportedStateVersion);
    }
    if state.payload().len() != COUNT * 4 {
        return Err(DeviceContractError::MalformedState);
    }
    let mut values = [0.0; COUNT];
    for ((slot, bytes), d) in values
        .iter_mut()
        .zip(state.payload().chunks_exact(4))
        .zip(DESCRIPTORS)
    {
        *slot = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        from_scalar(d, *slot)?;
    }
    Ok(Settings(values))
}
fn encode(settings: Settings) -> Result<NativeDeviceState, DeviceContractError> {
    let mut payload = Vec::with_capacity(COUNT * 4);
    for (value, d) in settings.0.into_iter().zip(DESCRIPTORS) {
        from_scalar(d, value)?;
        payload.extend_from_slice(&value.to_le_bytes());
    }
    NativeDeviceState::new(TYPE_ID, STATE_VERSION, payload)
}
impl NativeDeviceMetadata for LimiterDefinition {
    fn type_id(&self) -> DeviceTypeId {
        TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("limiter")
    }
    fn state_version(&self) -> u32 {
        STATE_VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        encode(Settings(std::array::from_fn(|i| {
            scalar(DESCRIPTORS[i].default)
        })))
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
        let settings = decode(state)?;
        from_scalar(descriptor(id)?, settings.value(id))
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut settings = decode(state)?;
        descriptor(id)?.validate(value)?;
        settings.0[id.value() as usize - 1] = scalar(value);
        encode(settings)
    }
    fn parameter_semantics_for_state(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<Option<ParameterSemantics>, DeviceContractError> {
        decode(state)?;
        let d = descriptor(id)?;
        if let Some(s) = super::automation::semantics(id) {
            return Ok(Some(s));
        }
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
            change_impact: if matches!(id, LOOKAHEAD | ROUTING | MODE) {
                ParameterChangeImpact::Structural
            } else {
                ParameterChangeImpact::ValueOnly
            },
        }))
    }
    fn parameter_enabled(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<bool, DeviceContractError> {
        let settings = decode(state)?;
        descriptor(id)?;
        Ok(match id {
            RELEASE => !settings.flag(AUTO_RELEASE),
            GAIN | CEILING => !settings.flag(MAXIMIZE),
            THRESHOLD | OUTPUT => settings.flag(MAXIMIZE),
            LINK => settings.choice(ROUTING) == 0,
            LINK_MID_SIDE => settings.choice(ROUTING) == 1,
            _ => true,
        })
    }
}
