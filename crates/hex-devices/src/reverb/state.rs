//! Strict original reverb state. V1 stores decay/wet; V2 adds room controls;
//! V3 adds resonant input filters. Migrations retain every authored older value
//! and supply explicitly versioned defaults for newly introduced controls. The project
//! opaque-device envelope is unchanged. Unknown versions/payloads are refused.
use super::{REVERB_STATE_VERSION, REVERB_TYPE_ID};
use hex_lattice_contract::{
    DeviceContractError, NativeDeviceState, ParameterDescriptor, ParameterId, ParameterKind,
    ParameterPresentation, ParameterValue,
};
pub const PARAMETER_COUNT: usize = 33;
pub mod parameter {
    use hex_lattice_contract::ParameterId;
    pub const DECAY: ParameterId = ParameterId::from_static(1);
    pub const DRY_WET: ParameterId = ParameterId::from_static(2);
    pub const LO_CUT: ParameterId = ParameterId::from_static(3);
    pub const LO_FREQ: ParameterId = ParameterId::from_static(4);
    pub const HI_CUT: ParameterId = ParameterId::from_static(5);
    pub const HI_FREQ: ParameterId = ParameterId::from_static(6);
    pub const PREDELAY: ParameterId = ParameterId::from_static(7);
    pub const SPIN: ParameterId = ParameterId::from_static(8);
    pub const SPIN_AMOUNT: ParameterId = ParameterId::from_static(9);
    pub const SPIN_RATE: ParameterId = ParameterId::from_static(10);
    pub const SHAPE: ParameterId = ParameterId::from_static(11);
    pub const HIGH: ParameterId = ParameterId::from_static(12);
    pub const HIGH_FREQ: ParameterId = ParameterId::from_static(13);
    pub const HIGH_RATIO: ParameterId = ParameterId::from_static(14);
    pub const LOW: ParameterId = ParameterId::from_static(15);
    pub const LOW_FREQ: ParameterId = ParameterId::from_static(16);
    pub const LOW_RATIO: ParameterId = ParameterId::from_static(17);
    pub const DIFFUSION: ParameterId = ParameterId::from_static(18);
    pub const SCALE: ParameterId = ParameterId::from_static(19);
    pub const SIZE: ParameterId = ParameterId::from_static(20);
    pub const CHORUS: ParameterId = ParameterId::from_static(21);
    pub const CHORUS_AMOUNT: ParameterId = ParameterId::from_static(22);
    pub const CHORUS_RATE: ParameterId = ParameterId::from_static(23);
    pub const SMOOTH: ParameterId = ParameterId::from_static(24);
    pub const DENSITY: ParameterId = ParameterId::from_static(25);
    pub const FREEZE: ParameterId = ParameterId::from_static(26);
    pub const FLAT: ParameterId = ParameterId::from_static(27);
    pub const CUT: ParameterId = ParameterId::from_static(28);
    pub const STEREO: ParameterId = ParameterId::from_static(29);
    pub const REFLECT: ParameterId = ParameterId::from_static(30);
    pub const LO_Q: ParameterId = ParameterId::from_static(32);
    pub const HI_Q: ParameterId = ParameterId::from_static(33);
    pub const DIFFUSE: ParameterId = ParameterId::from_static(31);
}
pub(super) const DESCRIPTORS: [ParameterDescriptor; PARAMETER_COUNT] = [
    ParameterDescriptor {
        id: parameter::DECAY,
        label: "decay",
        kind: ParameterKind::Float {
            min: 0.1,
            max: 60.0,
            unit: Some("s"),
        },
        default: ParameterValue::Float(1.2),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: parameter::DRY_WET,
        label: "dry/wet",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 100.0,
            unit: Some("%"),
        },
        default: ParameterValue::Float(55.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::LO_CUT,
        label: "lo cut",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::LO_FREQ,
        label: "lo cut hz",
        kind: ParameterKind::Float {
            min: 20.0,
            max: 20000.0,
            unit: Some("hz"),
        },
        default: ParameterValue::Float(830.0),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: parameter::HI_CUT,
        label: "hi cut",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(true),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::HI_FREQ,
        label: "hi cut hz",
        kind: ParameterKind::Float {
            min: 20.0,
            max: 20000.0,
            unit: Some("hz"),
        },
        default: ParameterValue::Float(830.0),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: parameter::PREDELAY,
        label: "predelay",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 250.0,
            unit: Some("ms"),
        },
        default: ParameterValue::Float(2.5),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::SPIN,
        label: "spin",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(true),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::SPIN_AMOUNT,
        label: "spin amount",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 100.0,
            unit: None,
        },
        default: ParameterValue::Float(17.5),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::SPIN_RATE,
        label: "spin rate",
        kind: ParameterKind::Float {
            min: 0.01,
            max: 10.0,
            unit: Some("hz"),
        },
        default: ParameterValue::Float(0.3),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: parameter::SHAPE,
        label: "shape",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(0.5),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::HIGH,
        label: "high",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(true),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::HIGH_FREQ,
        label: "high freq",
        kind: ParameterKind::Float {
            min: 20.0,
            max: 20000.0,
            unit: Some("hz"),
        },
        default: ParameterValue::Float(4500.0),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: parameter::HIGH_RATIO,
        label: "high ratio",
        kind: ParameterKind::Float {
            min: 0.1,
            max: 2.0,
            unit: None,
        },
        default: ParameterValue::Float(0.7),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: parameter::LOW,
        label: "low",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::LOW_FREQ,
        label: "low freq",
        kind: ParameterKind::Float {
            min: 20.0,
            max: 20000.0,
            unit: Some("hz"),
        },
        default: ParameterValue::Float(90.0),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: parameter::LOW_RATIO,
        label: "low ratio",
        kind: ParameterKind::Float {
            min: 0.1,
            max: 2.0,
            unit: None,
        },
        default: ParameterValue::Float(0.75),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: parameter::DIFFUSION,
        label: "diffusion",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 100.0,
            unit: Some("%"),
        },
        default: ParameterValue::Float(60.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::SCALE,
        label: "scale",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 100.0,
            unit: Some("%"),
        },
        default: ParameterValue::Float(40.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::SIZE,
        label: "size",
        kind: ParameterKind::Float {
            min: 1.0,
            max: 200.0,
            unit: Some("%"),
        },
        default: ParameterValue::Float(100.0),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: parameter::CHORUS,
        label: "chorus",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(true),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::CHORUS_AMOUNT,
        label: "chorus amount",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 4.0,
            unit: Some("ms"),
        },
        default: ParameterValue::Float(0.02),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::CHORUS_RATE,
        label: "chorus rate",
        kind: ParameterKind::Float {
            min: 0.01,
            max: 10.0,
            unit: Some("hz"),
        },
        default: ParameterValue::Float(0.02),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: parameter::SMOOTH,
        label: "smooth",
        kind: ParameterKind::Choice {
            labels: &["none", "fast", "slow"],
        },
        default: ParameterValue::Choice(1),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::DENSITY,
        label: "density",
        kind: ParameterKind::Choice {
            labels: &["low", "medium", "high"],
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::FREEZE,
        label: "freeze",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::FLAT,
        label: "flat",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(true),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::CUT,
        label: "cut",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(true),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::STEREO,
        label: "stereo",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 120.0,
            unit: Some("%"),
        },
        default: ParameterValue::Float(100.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::REFLECT,
        label: "reflect",
        kind: ParameterKind::Float {
            min: -60.0,
            max: 6.0,
            unit: Some("db"),
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::DIFFUSE,
        label: "diffuse",
        kind: ParameterKind::Float {
            min: -60.0,
            max: 6.0,
            unit: Some("db"),
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::LO_Q,
        label: "lo cut q",
        kind: ParameterKind::Float {
            min: 0.1,
            max: 10.0,
            unit: None,
        },
        default: ParameterValue::Float(std::f32::consts::FRAC_1_SQRT_2),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: parameter::HI_Q,
        label: "hi cut q",
        kind: ParameterKind::Float {
            min: 0.1,
            max: 10.0,
            unit: None,
        },
        default: ParameterValue::Float(5.85),
        presentation: ParameterPresentation::Logarithmic,
    },
];
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Settings(pub [f32; PARAMETER_COUNT]);
impl Settings {
    pub fn get(self, id: ParameterId) -> f32 {
        self.0[(id.value() - 1) as usize]
    }
    pub fn on(self, id: ParameterId) -> bool {
        self.get(id) == 1.0
    }
    pub fn default_values() -> Self {
        Self(DESCRIPTORS.map(|d| scalar(d.default)))
    }
}
pub(super) fn scalar(value: ParameterValue) -> f32 {
    match value {
        ParameterValue::Float(v) => v,
        ParameterValue::Bool(v) => f32::from(v),
        ParameterValue::Choice(v) => v as f32,
    }
}
pub(super) fn index(id: ParameterId) -> Result<usize, DeviceContractError> {
    DESCRIPTORS
        .iter()
        .position(|d| d.id == id)
        .ok_or(DeviceContractError::UnknownParameter)
}
pub(super) fn value(settings: Settings, i: usize) -> ParameterValue {
    match DESCRIPTORS[i].kind {
        ParameterKind::Float { .. } => ParameterValue::Float(settings.0[i]),
        ParameterKind::Bool => ParameterValue::Bool(settings.0[i] == 1.0),
        ParameterKind::Choice { .. } => ParameterValue::Choice(settings.0[i] as u32),
    }
}
pub(super) fn decode(state: &NativeDeviceState) -> Result<Settings, DeviceContractError> {
    if state.type_id() != REVERB_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != REVERB_STATE_VERSION {
        return Err(DeviceContractError::UnsupportedStateVersion);
    }
    decode_payload(state.payload(), PARAMETER_COUNT)
}
fn decode_payload(payload: &[u8], count: usize) -> Result<Settings, DeviceContractError> {
    if payload.len() != count * 4 {
        return Err(DeviceContractError::MalformedState);
    }
    let mut settings = Settings::default_values();
    for (i, bytes) in payload.chunks_exact(4).enumerate() {
        let bytes = bytes
            .try_into()
            .map_err(|_| DeviceContractError::MalformedState)?;
        settings.0[i] = match DESCRIPTORS[i].kind {
            ParameterKind::Float { .. } => f32::from_le_bytes(bytes),
            ParameterKind::Bool => {
                let v = u32::from_le_bytes(bytes);
                if v > 1 {
                    return Err(DeviceContractError::MalformedState);
                }
                v as f32
            }
            ParameterKind::Choice { .. } => u32::from_le_bytes(bytes) as f32,
        };
        DESCRIPTORS[i].validate(value(settings, i))?;
    }
    Ok(settings)
}
pub(super) fn encode(settings: Settings) -> Result<NativeDeviceState, DeviceContractError> {
    let mut payload = Vec::with_capacity(PARAMETER_COUNT * 4);
    for (i, descriptor) in DESCRIPTORS.iter().enumerate() {
        let v = value(settings, i);
        descriptor.validate(v)?;
        payload.extend_from_slice(&match v {
            ParameterValue::Float(v) => v.to_le_bytes(),
            ParameterValue::Bool(v) => u32::from(v).to_le_bytes(),
            ParameterValue::Choice(v) => v.to_le_bytes(),
        });
    }
    NativeDeviceState::new(REVERB_TYPE_ID, REVERB_STATE_VERSION, payload)
}
pub(super) fn migrate(state: &NativeDeviceState) -> Result<NativeDeviceState, DeviceContractError> {
    if state.type_id() == REVERB_TYPE_ID && state.state_version() == 1 {
        if state.payload().len() != 8 {
            return Err(DeviceContractError::MalformedState);
        }
        let mut settings = Settings::default_values();
        for (i, bytes) in state.payload().chunks_exact(4).enumerate() {
            let v = f32::from_le_bytes(
                bytes
                    .try_into()
                    .map_err(|_| DeviceContractError::MalformedState)?,
            );
            // V1's documented range was 0.1–2.5 s and 0–100 percent.
            if !v.is_finite() || !(if i == 0 { 0.1..=2.5 } else { 0.0..=100.0 }).contains(&v) {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            settings.0[i] = v;
        }
        return encode(settings);
    }
    if state.type_id() == REVERB_TYPE_ID && state.state_version() == 2 {
        return encode(decode_payload(state.payload(), 31)?);
    }
    encode(decode(state)?)
}
