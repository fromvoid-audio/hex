//! Original, immutable, zero-latency stereo Utility processing.
use hex_lattice::{InvalidMatrix, OutputTail, StereoFrame, StereoMatrix};
use hex_lattice::{NativeDeviceDefinition, NativeProcessorRuntime, PreparedNativeProcessor};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::fmt;

/// Maximum admitted linear Utility gain, equivalent to +35 dB.
pub const MAX_UTILITY_GAIN: f32 = 56.234_13;
pub const UTILITY_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.utility");

pub mod parameter {
    use hex_lattice_contract::ParameterId;
    pub const GAIN: ParameterId = ParameterId::from_static(1);
    pub const BALANCE: ParameterId = ParameterId::from_static(2);
    pub const WIDTH: ParameterId = ParameterId::from_static(3);
    pub const CHANNEL: ParameterId = ParameterId::from_static(4);
    pub const INVERT_LEFT: ParameterId = ParameterId::from_static(5);
    pub const INVERT_RIGHT: ParameterId = ParameterId::from_static(6);
    pub const MONO: ParameterId = ParameterId::from_static(7);
    pub const MUTE: ParameterId = ParameterId::from_static(8);
    pub const MID_SIDE_ENABLED: ParameterId = ParameterId::from_static(9);
    pub const MID_SIDE_BALANCE: ParameterId = ParameterId::from_static(10);
    pub const DC_REMOVE: ParameterId = ParameterId::from_static(11);
    pub const BASS_MONO: ParameterId = ParameterId::from_static(12);
    pub const BASS_MONO_CUTOFF: ParameterId = ParameterId::from_static(13);
    pub const BASS_MONO_AUDITION: ParameterId = ParameterId::from_static(14);
}

/// Channel selection applied after input polarity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelMode {
    Stereo,
    Left,
    Right,
    Swap,
}

/// Invalid or unrepresentable Utility settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtilityError(pub &'static str);

impl fmt::Display for UtilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for UtilityError {}

impl From<InvalidMatrix> for UtilityError {
    fn from(_: InvalidMatrix) -> Self {
        Self("mix coefficient preparation overflowed")
    }
}

/// Validated control-side parameters for a memoryless stereo Utility processor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UtilitySettings {
    gain: f32,
    balance: f32,
    width: f32,
    channel_mode: ChannelMode,
    invert_left: bool,
    invert_right: bool,
    mono: bool,
    mute: bool,
    mid_side_balance: Option<f32>,
    dc_remove: bool,
    bass_mono: bool,
    bass_mono_cutoff_hz: f32,
    bass_mono_audition: bool,
}

impl UtilitySettings {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        gain: f32,
        balance: f32,
        width: f32,
        channel_mode: ChannelMode,
        invert_left: bool,
        invert_right: bool,
        mono: bool,
        mute: bool,
        mid_side_balance: Option<f32>,
    ) -> Result<Self, UtilityError> {
        if !gain.is_finite() || !(0.0..=MAX_UTILITY_GAIN).contains(&gain) {
            return Err(UtilityError(
                "mix gain must be finite and between 0 and +35 dB",
            ));
        }
        if !balance.is_finite() || !(-1.0..=1.0).contains(&balance) {
            return Err(UtilityError(
                "mix balance must be finite and between -1 and 1",
            ));
        }
        if !width.is_finite() || !(0.0..=4.0).contains(&width) {
            return Err(UtilityError("mix width must be finite and between 0 and 4"));
        }
        if mid_side_balance
            .is_some_and(|value| !value.is_finite() || !(-1.0..=1.0).contains(&value))
        {
            return Err(UtilityError(
                "mix mid-side balance must be finite and between -1 and 1",
            ));
        }
        Ok(Self {
            gain,
            balance,
            width,
            channel_mode,
            invert_left,
            invert_right,
            mono,
            mute,
            mid_side_balance,
            dc_remove: false,
            bass_mono: false,
            bass_mono_cutoff_hz: 200.0,
            bass_mono_audition: false,
        })
    }

    fn check_inline(&self) {
        let Self {
            gain,
            balance,
            width,
            channel_mode,
            invert_left,
            invert_right,
            mono,
            mute,
            mid_side_balance,
            dc_remove,
            bass_mono,
            bass_mono_cutoff_hz,
            bass_mono_audition,
        } = self;
        let _: [&f32; 4] = [gain, balance, width, bass_mono_cutoff_hz];
        let _: [&bool; 7] = [
            invert_left,
            invert_right,
            mono,
            mute,
            dc_remove,
            bass_mono,
            bass_mono_audition,
        ];
        let _: &ChannelMode = channel_mode;
        let _: &Option<f32> = mid_side_balance;
    }
    pub const fn gain(self) -> f32 {
        self.gain
    }

    pub const fn balance(self) -> f32 {
        self.balance
    }

    pub const fn width(self) -> f32 {
        self.width
    }

    pub const fn channel_mode(self) -> ChannelMode {
        self.channel_mode
    }

    pub const fn mid_side_balance(self) -> Option<f32> {
        self.mid_side_balance
    }

    pub const fn invert_left(self) -> bool {
        self.invert_left
    }

    pub const fn invert_right(self) -> bool {
        self.invert_right
    }

    pub const fn mono(self) -> bool {
        self.mono
    }

    pub const fn mute(self) -> bool {
        self.mute
    }
}

impl Default for UtilitySettings {
    fn default() -> Self {
        Self {
            gain: 1.0,
            balance: 0.0,
            width: 1.0,
            channel_mode: ChannelMode::Stereo,
            invert_left: false,
            invert_right: false,
            mono: false,
            mute: false,
            mid_side_balance: None,
            dc_remove: false,
            bass_mono: false,
            bass_mono_cutoff_hz: 200.0,
            bass_mono_audition: false,
        }
    }
}

const CHANNEL_LABELS: &[&str] = &["stereo", "left", "right", "swap"];
#[derive(Clone, Copy, Debug, PartialEq)]
struct UtilityControls {
    settings: UtilitySettings,
    mid_side_enabled: bool,
    mid_side_balance: f32,
}
impl UtilityControls {
    fn decode(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        let (settings, mid_side_enabled, mid_side_balance) = decode_state_fields(state)?;
        Ok(Self {
            settings,
            mid_side_enabled,
            mid_side_balance,
        })
    }
    fn settings(self) -> UtilitySettings {
        UtilitySettings {
            mid_side_balance: self.mid_side_enabled.then_some(self.mid_side_balance),
            ..self.settings
        }
    }
    fn encode(self) -> Result<NativeDeviceState, DeviceContractError> {
        encode_state_fields(
            self.settings(),
            self.mid_side_enabled,
            self.mid_side_balance,
        )
    }
    fn check_inline(&self) {
        let Self {
            settings,
            mid_side_enabled,
            mid_side_balance,
        } = self;
        settings.check_inline();
        let _: &bool = mid_side_enabled;
        let _: &f32 = mid_side_balance;
    }
}
const BINDINGS: [crate::parameter_binding::Binding<UtilityControls>; 14] = [
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::GAIN,
            label: "gain",
            kind: ParameterKind::Float {
                min: 0.0,
                max: MAX_UTILITY_GAIN,
                unit: Some("linear"),
            },
            default: ParameterValue::Float(1.0),
            presentation: ParameterPresentation::LinearAmplitudeDecibels,
        },
        crate::parameter_binding::SCALAR_ENVELOPE,
        |c| c.settings.gain,
        |c, v| c.settings.gain = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::BALANCE,
            label: "balance",
            kind: ParameterKind::Float {
                min: -1.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::SCALAR_ENVELOPE,
        |c| c.settings.balance,
        |c, v| c.settings.balance = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::WIDTH,
            label: "width",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 4.0,
                unit: None,
            },
            default: ParameterValue::Float(1.0),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::SCALAR_ENVELOPE,
        |c| c.settings.width,
        |c, v| c.settings.width = v,
    ),
    crate::parameter_binding::Binding::choice(
        ParameterDescriptor {
            id: parameter::CHANNEL,
            label: "channel",
            kind: ParameterKind::Choice {
                labels: CHANNEL_LABELS,
            },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::CHOICE_ENVELOPE,
        |c| match c.settings.channel_mode {
            ChannelMode::Stereo => 0,
            ChannelMode::Left => 1,
            ChannelMode::Right => 2,
            ChannelMode::Swap => 3,
        },
        |c, v| {
            c.settings.channel_mode = match v {
                0 => ChannelMode::Stereo,
                1 => ChannelMode::Left,
                2 => ChannelMode::Right,
                3 => ChannelMode::Swap,
                _ => return Err(DeviceContractError::InvalidParameterValue),
            };
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::boolean(
        ParameterDescriptor {
            id: parameter::INVERT_LEFT,
            label: "invert left",
            kind: ParameterKind::Bool,
            default: ParameterValue::Bool(false),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::BOOLEAN_ENVELOPE,
        |c| c.settings.invert_left,
        |c, v| c.settings.invert_left = v,
    ),
    crate::parameter_binding::Binding::boolean(
        ParameterDescriptor {
            id: parameter::INVERT_RIGHT,
            label: "invert right",
            kind: ParameterKind::Bool,
            default: ParameterValue::Bool(false),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::BOOLEAN_ENVELOPE,
        |c| c.settings.invert_right,
        |c, v| c.settings.invert_right = v,
    ),
    crate::parameter_binding::Binding::boolean(
        ParameterDescriptor {
            id: parameter::MONO,
            label: "mono",
            kind: ParameterKind::Bool,
            default: ParameterValue::Bool(false),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::BOOLEAN_ENVELOPE,
        |c| c.settings.mono,
        |c, v| c.settings.mono = v,
    ),
    crate::parameter_binding::Binding::boolean(
        ParameterDescriptor {
            id: parameter::MUTE,
            label: "mute",
            kind: ParameterKind::Bool,
            default: ParameterValue::Bool(false),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::BOOLEAN_ENVELOPE,
        |c| c.settings.mute,
        |c, v| c.settings.mute = v,
    ),
    crate::parameter_binding::Binding::boolean(
        ParameterDescriptor {
            id: parameter::MID_SIDE_ENABLED,
            label: "mid-side",
            kind: ParameterKind::Bool,
            default: ParameterValue::Bool(false),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::BOOLEAN_ENVELOPE,
        |c| c.mid_side_enabled,
        |c, v| c.mid_side_enabled = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::MID_SIDE_BALANCE,
            label: "mid-side balance",
            kind: ParameterKind::Float {
                min: -1.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::SCALAR_ENVELOPE,
        |c| c.mid_side_balance,
        |c, v| c.mid_side_balance = v,
    ),
    crate::parameter_binding::Binding::boolean(
        ParameterDescriptor {
            id: parameter::DC_REMOVE,
            label: "dc remove",
            kind: ParameterKind::Bool,
            default: ParameterValue::Bool(false),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::BOOLEAN_ENVELOPE,
        |c| c.settings.dc_remove,
        |c, v| c.settings.dc_remove = v,
    ),
    crate::parameter_binding::Binding::boolean(
        ParameterDescriptor {
            id: parameter::BASS_MONO,
            label: "bass mono",
            kind: ParameterKind::Bool,
            default: ParameterValue::Bool(false),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::BOOLEAN_ENVELOPE,
        |c| c.settings.bass_mono,
        |c, v| c.settings.bass_mono = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::BASS_MONO_CUTOFF,
            label: "bass mono cutoff",
            kind: ParameterKind::Float {
                min: 50.0,
                max: 500.0,
                unit: Some("hz"),
            },
            default: ParameterValue::Float(200.0),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::SCALAR_ENVELOPE,
        |c| c.settings.bass_mono_cutoff_hz,
        |c, v| c.settings.bass_mono_cutoff_hz = v,
    ),
    crate::parameter_binding::Binding::boolean(
        ParameterDescriptor {
            id: parameter::BASS_MONO_AUDITION,
            label: "bass mono audition",
            kind: ParameterKind::Bool,
            default: ParameterValue::Bool(false),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::BOOLEAN_ENVELOPE,
        |c| c.settings.bass_mono_audition,
        |c, v| c.settings.bass_mono_audition = v,
    ),
];
const DESCRIPTORS: &[ParameterDescriptor] =
    &crate::parameter_binding::descriptors(&BINDINGS, BINDINGS[0].descriptor);

/// Built-in Utility metadata, portable state codec, and control-side factory.
pub struct UtilityDefinition;

fn decode_state_fields(
    state: &NativeDeviceState,
) -> Result<(UtilitySettings, bool, f32), DeviceContractError> {
    if state.type_id() != UTILITY_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if !matches!(state.state_version(), 1 | 2) {
        return Err(DeviceContractError::UnsupportedStateVersion);
    }
    let bytes = state.payload();
    let valid_length = if state.state_version() == 1 {
        bytes.len() == 19
    } else {
        bytes.len() == 26
    };
    if !valid_length || bytes[12] > 3 || bytes[13] & !0x0f != 0 || bytes[14] > 1 {
        return Err(DeviceContractError::MalformedState);
    }
    let read = |offset| {
        f32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    };
    let channel_mode = match bytes[12] {
        0 => ChannelMode::Stereo,
        1 => ChannelMode::Left,
        2 => ChannelMode::Right,
        3 => ChannelMode::Swap,
        _ => return Err(DeviceContractError::MalformedState),
    };
    let mid_side_enabled = bytes[14] == 1;
    let mid_side_balance = read(15);
    let settings = UtilitySettings::new(
        read(0),
        read(4),
        read(8),
        channel_mode,
        bytes[13] & 1 != 0,
        bytes[13] & 2 != 0,
        bytes[13] & 4 != 0,
        bytes[13] & 8 != 0,
        Some(mid_side_balance),
    )
    .map_err(|_| DeviceContractError::MalformedState)?;
    let settings = UtilitySettings {
        mid_side_balance: mid_side_enabled.then_some(mid_side_balance),
        ..settings
    };
    let (dc_remove, bass_mono, audition, cutoff) = if state.state_version() == 1 {
        (false, false, false, 200.0)
    } else {
        if bytes[19] > 1 || bytes[20] > 1 || bytes[21] > 1 {
            return Err(DeviceContractError::MalformedState);
        }
        (bytes[19] == 1, bytes[20] == 1, bytes[21] == 1, read(22))
    };
    if !cutoff.is_finite() || !(50.0..=500.0).contains(&cutoff) {
        return Err(DeviceContractError::MalformedState);
    }
    Ok((
        UtilitySettings {
            dc_remove,
            bass_mono,
            bass_mono_cutoff_hz: cutoff,
            bass_mono_audition: audition,
            ..settings
        },
        mid_side_enabled,
        mid_side_balance,
    ))
}

fn decode_state(state: &NativeDeviceState) -> Result<UtilitySettings, DeviceContractError> {
    decode_state_fields(state).map(|(settings, _, _)| settings)
}

fn encode_state(settings: UtilitySettings) -> Result<NativeDeviceState, DeviceContractError> {
    let mid_side_balance = settings.mid_side_balance.unwrap_or(0.0);
    encode_state_fields(
        settings,
        settings.mid_side_balance.is_some(),
        mid_side_balance,
    )
}

fn encode_state_fields(
    settings: UtilitySettings,
    mid_side_enabled: bool,
    mid_side_balance: f32,
) -> Result<NativeDeviceState, DeviceContractError> {
    let mut bytes = Vec::with_capacity(26);
    for value in [settings.gain, settings.balance, settings.width] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.push(match settings.channel_mode {
        ChannelMode::Stereo => 0,
        ChannelMode::Left => 1,
        ChannelMode::Right => 2,
        ChannelMode::Swap => 3,
    });
    bytes.push(
        u8::from(settings.invert_left)
            | (u8::from(settings.invert_right) << 1)
            | (u8::from(settings.mono) << 2)
            | (u8::from(settings.mute) << 3),
    );
    bytes.push(u8::from(mid_side_enabled));
    bytes.extend_from_slice(&mid_side_balance.to_le_bytes());
    bytes.push(u8::from(settings.dc_remove));
    bytes.push(u8::from(settings.bass_mono));
    bytes.push(u8::from(settings.bass_mono_audition));
    bytes.extend_from_slice(&settings.bass_mono_cutoff_hz.to_le_bytes());
    NativeDeviceState::new(UTILITY_TYPE_ID, 2, bytes)
}

impl NativeDeviceMetadata for UtilityDefinition {
    fn display_name(&self) -> Option<&'static str> {
        Some("mix")
    }
    fn type_id(&self) -> DeviceTypeId {
        UTILITY_TYPE_ID
    }
    fn state_version(&self) -> u32 {
        2
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        encode_state(UtilitySettings::default())
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let (settings, mid_side_enabled, mid_side_balance) = decode_state_fields(state)?;
        encode_state_fields(settings, mid_side_enabled, mid_side_balance)
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let controls = UtilityControls::decode(state)?;
        Ok(crate::parameter_binding::binding(&BINDINGS, id)?.read(&controls))
    }
    fn parameter_enabled(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<bool, DeviceContractError> {
        let settings = decode_state(state)?;
        let descriptor = DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.id == id)
            .ok_or(DeviceContractError::UnknownParameter)?;
        descriptor.validate(self.read_parameter(state, id)?)?;
        let stereo_channels = !matches!(
            settings.channel_mode,
            ChannelMode::Left | ChannelMode::Right
        );
        Ok(match id {
            parameter::WIDTH | parameter::MID_SIDE_ENABLED | parameter::MID_SIDE_BALANCE => {
                stereo_channels
            }
            _ => true,
        })
    }
    fn parameter_semantics_for_state(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<Option<hex_lattice_contract::ParameterSemantics>, DeviceContractError> {
        crate::parameter_binding::parameter_semantics_with_sample_rate_modulation(
            &BINDINGS,
            state,
            id,
            UtilityControls::decode,
            runtime_parameter_automation_supported,
        )
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        crate::parameter_binding::patch_parameter(
            &BINDINGS,
            state,
            id,
            value,
            UtilityControls::decode,
            UtilityControls::encode,
        )
    }
}

mod matrix;
mod runtime;
pub use matrix::PreparedUtility;
use matrix::{balance_matrix, pre_gain_matrix, scalar_matrix};
pub use runtime::PreparedUtilityFilterRuntime;
use runtime::runtime_parameter_automation_supported;
pub(crate) use runtime::{UtilityAutomation, UtilityRamp};

pub static UTILITY_DEFINITION: UtilityDefinition = UtilityDefinition;

#[cfg(test)]
#[path = "utility/automation_tests.rs"]
mod automation_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod width_ramp_tests;

#[cfg(test)]
mod bypass_tests;
