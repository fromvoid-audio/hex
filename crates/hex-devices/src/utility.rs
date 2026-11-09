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
        Self("utility coefficient preparation overflowed")
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
                "utility gain must be finite and between 0 and +35 dB",
            ));
        }
        if !balance.is_finite() || !(-1.0..=1.0).contains(&balance) {
            return Err(UtilityError(
                "utility balance must be finite and between -1 and 1",
            ));
        }
        if !width.is_finite() || !(0.0..=4.0).contains(&width) {
            return Err(UtilityError(
                "utility width must be finite and between 0 and 4",
            ));
        }
        if mid_side_balance
            .is_some_and(|value| !value.is_finite() || !(-1.0..=1.0).contains(&value))
        {
            return Err(UtilityError(
                "utility mid-side balance must be finite and between -1 and 1",
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
        Some("utility")
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
        UtilityControls::decode(state)?;
        let mut semantics = crate::parameter_binding::binding(&BINDINGS, id)?.semantics;
        if runtime_parameter_automation_supported(id) {
            semantics.modulation = hex_lattice_contract::ParameterModulation::SampleRate;
        }
        Ok(Some(semantics))
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut controls = UtilityControls::decode(state)?;
        crate::parameter_binding::binding(&BINDINGS, id)?.write(&mut controls, value)?;
        controls.encode()
    }
}

// The current concrete direct-control path implements only these fixed
// IDs, with value-only/no-smoothing updates. Other families may explicitly
// implement structural/smoothed delivery without changing Binding metadata.
fn runtime_parameter_automation_supported(parameter: ParameterId) -> bool {
    let implemented = parameter
        .value()
        .checked_sub(1)
        .is_some_and(|index| index < 14);
    let Ok(binding) = crate::parameter_binding::binding(&BINDINGS, parameter) else {
        return false;
    };
    implemented
        && binding.declares_arrangement_automation()
        && binding.semantics.normalization
            != hex_lattice_contract::ParameterNormalization::Unspecified
        && binding.semantics.stepping != hex_lattice_contract::ParameterStepping::Unspecified
        && binding.semantics.change_impact == hex_lattice_contract::ParameterChangeImpact::ValueOnly
        && binding.semantics.smoothing == hex_lattice_contract::ParameterSmoothing::None
}

impl NativeDeviceDefinition for UtilityDefinition {
    fn source_effect_factory_bytes(&self) -> Option<usize> {
        // The prepared factory contains only inline settings and coefficients.
        Some(crate::source_effect_arc_bytes::<PreparedUtilityFilterRuntime>())
    }
    fn supports_parameter_automation(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<bool, DeviceContractError> {
        decode_state(state)?;
        let Ok(binding) = crate::parameter_binding::binding(&BINDINGS, parameter) else {
            return Ok(false);
        };
        binding.semantics.validate(binding.descriptor)?;
        Ok(runtime_parameter_automation_supported(parameter))
    }
    fn prepare_automated_runtime_instance(
        &self,
        _identity: hex_lattice::ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
        parameters: &[ParameterId],
    ) -> Result<hex_lattice::PreparedRuntimeDevice, DeviceContractError> {
        Ok(hex_lattice::PreparedRuntimeDevice {
            matrix: StereoMatrix::identity(),
            processor: Some(hex_lattice::PreparedProcessorFactory::Legacy(
                std::sync::Arc::new(Self::prepare_owned_automated_filter_for_parameters(
                    state,
                    context.sample_rate,
                    parameters,
                )?),
            )),
        })
    }

    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        let settings = decode_state(state)?;
        if settings.dc_remove || settings.bass_mono {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        PreparedUtility::new(settings)
            .map(|prepared| prepared.matrix())
            .map_err(|_| DeviceContractError::MalformedState)
    }

    fn prepare_effect_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        let mut settings = decode_state(state)?;
        // Stateful mute belongs to the processor, so its input matrix remains
        // compatible across mute handoffs and no audible old tail is detached.
        if settings.dc_remove || settings.bass_mono {
            settings.mute = false;
        }
        PreparedUtility::new(settings)
            .map(|prepared| prepared.matrix())
            .map_err(|_| DeviceContractError::MalformedState)
    }

    fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        let sample_rate = context.sample_rate;
        Self::prepare_owned_filter(state, sample_rate)
            .map(|value| value.map(|value| Box::new(value) as Box<dyn PreparedNativeProcessor>))
    }
}

/// The concrete fixed-inline Utility filter preparation. Its private fields
/// cannot contain caller-provided processor/storage implementations. Native
/// graph construction can retain this exact type's allocation declaration
/// before erasing the processor trait; this is not a generic provider receipt.
pub struct PreparedUtilityFilterRuntime(
    crate::utility_filters::PreparedUtilityFilters,
    Option<UtilityAutomation>,
);
impl PreparedUtilityFilterRuntime {
    fn check_inline_ownership(&self) {
        let Self(filters, automation) = self;
        if let Some(automation) = automation {
            automation.check_inline();
        }
        filters.check_inline();
    }
}

impl UtilityDefinition {
    /// Closed fixed-inline factory for the full Utility law. Automation always
    /// retains filters, including dormant histories, with an identity host matrix.
    pub fn prepare_owned_automated_filter(
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<PreparedUtilityFilterRuntime, DeviceContractError> {
        Self::prepare_automated_with_mask(state, sample_rate, 0x3fff)
    }
    /// Exact admitted target inventory allows dormant filters to skip work only
    /// when this revision cannot enable them. The concrete owner stays inline.
    pub fn prepare_owned_automated_filter_for_parameters(
        state: &NativeDeviceState,
        sample_rate: u32,
        parameters: &[ParameterId],
    ) -> Result<PreparedUtilityFilterRuntime, DeviceContractError> {
        let mut mask = 0u16;
        for parameter in parameters {
            let index = parameter
                .value()
                .checked_sub(1)
                .filter(|index| *index < 14)
                .ok_or(DeviceContractError::UnknownParameter)?;
            mask |= 1u16
                .checked_shl(index)
                .ok_or(DeviceContractError::UnknownParameter)?;
        }
        Self::prepare_automated_with_mask(state, sample_rate, mask)
    }
    fn prepare_automated_with_mask(
        state: &NativeDeviceState,
        sample_rate: u32,
        parameters: u16,
    ) -> Result<PreparedUtilityFilterRuntime, DeviceContractError> {
        state.require_asset_free_processing()?;
        for (index, binding) in BINDINGS.iter().enumerate() {
            if parameters & (1u16 << index) != 0 {
                binding.semantics.validate(binding.descriptor)?;
                if !runtime_parameter_automation_supported(binding.descriptor.id) {
                    return Err(DeviceContractError::UnsupportedProcessingClass);
                }
            }
        }
        let base = UtilityControls::decode(state)?;
        let mut automation = UtilityAutomation {
            controls: crate::parameter_binding::Controls::new(
                base,
                std::array::from_fn(|index| parameters & (1u16 << index) != 0),
            ),
            effective: base.settings(),
            matrix: StereoMatrix::identity(),
            pre_gain: StereoMatrix::identity(),
            post_gain: StereoMatrix::identity(),
            width_ramp: None,
        };
        let (matrix, filters) = automation.configuration(None, 0, sample_rate, None)?;
        let filters = filters.ok_or(DeviceContractError::InvalidDescriptor)?;
        let value = PreparedUtilityFilterRuntime(
            filters,
            Some(UtilityAutomation {
                matrix,
                ..automation
            }),
        );
        value.check_inline_ownership();
        Ok(value)
    }
    /// Prepare the same registered filter through its concrete constructor.
    /// Memoryless Utility settings return None, exactly as the device registry.
    pub fn prepare_owned_filter(
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Option<PreparedUtilityFilterRuntime>, DeviceContractError> {
        state.require_asset_free_processing()?;
        let settings = decode_state(state)?;
        if !settings.dc_remove && !settings.bass_mono {
            return Ok(None);
        }
        let prepared = crate::utility_filters::PreparedUtilityFilters::new(
            sample_rate,
            settings.dc_remove,
            settings.bass_mono,
            f64::from(settings.bass_mono_cutoff_hz),
            settings.bass_mono_audition,
        )
        .map_err(|_| DeviceContractError::MalformedState)?
        .with_output_mute(settings.mute);
        let value = PreparedUtilityFilterRuntime(prepared, None);
        value.check_inline_ownership();
        Ok(Some(value))
    }
}

impl PreparedNativeProcessor for PreparedUtilityFilterRuntime {
    fn supports_smoothed_bypass(&self) -> bool {
        // Both static and automated zero-latency filters advance private history
        // under bypass. The host can blend their complete output reversibly,
        // without retaining a separately audible old filter suffix.
        true
    }

    fn parameter_automation_descriptor(
        &self,
        parameter: ParameterId,
    ) -> Option<ParameterDescriptor> {
        self.1
            .filter(|automation| automation.supports(parameter))
            .and_then(|_| DESCRIPTORS.iter().find(|d| d.id == parameter).copied())
    }
    fn supports_parameter_automation(&self, parameter: ParameterId) -> bool {
        self.1
            .is_some_and(|automation| automation.supports(parameter))
    }
    fn parameter_modulation_semantics(
        &self,
        parameter: ParameterId,
    ) -> Option<hex_lattice_contract::ParameterSemantics> {
        if !self.supports_parameter_automation(parameter) {
            return None;
        }
        let binding = crate::parameter_binding::binding(&BINDINGS, parameter).ok()?;
        Some(hex_lattice_contract::ParameterSemantics {
            modulation: hex_lattice_contract::ParameterModulation::SampleRate,
            ..binding.semantics
        })
    }

    fn runtime_version(&self) -> u32 {
        1
    }

    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<crate::utility_filters::UtilityFilterProcessor>()
    }

    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(crate::utility_filters::UtilityFilterProcessor::with_automation(self.0, self.1))
    }

    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(self.0.algorithmic_latency_frames())
    }

    fn output_tail(&self) -> OutputTail {
        OutputTail::Unbounded
    }
}

/// Immutable authored controls plus the last effective matrix. No state codec,
/// allocation or metadata enumeration occurs during processing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct UtilityAutomation {
    controls: crate::parameter_binding::Controls<UtilityControls, 14>,
    pub(crate) effective: UtilitySettings,
    pub(crate) matrix: StereoMatrix,
    pre_gain: StereoMatrix,
    post_gain: StereoMatrix,
    width_ramp: Option<UtilityRamp>,
}

/// Five-millisecond ramp for a direct Utility control retarget.
#[derive(Clone, Copy, Debug)]
pub(crate) struct UtilityRamp {
    from: f32,
    target: f32,
    elapsed: u32,
    total: u32,
}
impl UtilityRamp {
    pub(crate) fn new(from: f32, target: f32, sample_rate: u32) -> Option<Self> {
        (from != target).then_some(Self {
            from,
            target,
            elapsed: 0,
            total: sample_rate.div_ceil(200).max(1),
        })
    }
    pub(crate) fn check_inline(&self) {
        let Self {
            from,
            target,
            elapsed,
            total,
        } = self;
        let _: (&f32, &f32, &u32, &u32) = (from, target, elapsed, total);
    }
    pub(crate) fn advance(&mut self) -> f32 {
        self.elapsed += 1;
        self.value()
    }
    pub(crate) fn finished(self) -> bool {
        self.elapsed >= self.total
    }
    pub(crate) fn value(self) -> f32 {
        if self.finished() {
            self.target
        } else {
            let t = f64::from(self.elapsed) / f64::from(self.total);
            (f64::from(self.from) + (f64::from(self.target) - f64::from(self.from)) * t) as f32
        }
    }
}
impl UtilityAutomation {
    pub(crate) fn check_inline(&self) {
        let Self {
            controls,
            effective,
            matrix,
            pre_gain,
            post_gain,
            width_ramp,
        } = self;
        controls.check_inline(UtilityControls::check_inline);
        effective.check_inline();
        let _: (&StereoMatrix, &StereoMatrix, &StereoMatrix) = (matrix, pre_gain, post_gain);
        if let Some(ramp) = width_ramp {
            ramp.check_inline();
        }
    }
    pub(crate) fn inherit(&mut self, previous: &Self, sample_rate: u32) {
        let target_width = self.controls.base().settings().width;
        let previous_width = previous.effective.width;
        self.width_ramp = if self.controls.admitted(2) {
            None
        } else if !previous.controls.admitted(2)
            && target_width == previous.controls.base().settings().width
        {
            previous.width_ramp
        } else {
            UtilityRamp::new(previous_width, target_width, sample_rate)
        };
        self.controls.inherit(&previous.controls);
        self.matrix = previous.matrix;
        self.pre_gain = previous.pre_gain;
        self.post_gain = previous.post_gain;
        self.effective = previous.effective;
    }
    pub(crate) fn manual_mute_target(&self) -> Option<bool> {
        (!self.supports(parameter::MUTE)).then(|| self.controls.base().settings().mute)
    }
    pub(crate) fn supports(&self, parameter: ParameterId) -> bool {
        runtime_parameter_automation_supported(parameter)
            && self.controls.supports(&BINDINGS, parameter)
    }
    pub(crate) fn may_advance_dc(&self) -> bool {
        self.controls.base().settings.dc_remove || self.controls.admitted(10)
    }
    pub(crate) fn may_advance_bass(&self) -> bool {
        self.controls.base().settings.bass_mono || self.controls.admitted(11)
    }
    pub(crate) fn resolve(
        &self,
        values: &dyn hex_lattice::NativeParameterAutomation,
    ) -> Result<crate::parameter_binding::Resolved<14>, DeviceContractError> {
        self.controls.resolve(&BINDINGS, values)
    }
    pub(crate) fn configuration(
        &mut self,
        values: Option<(
            &dyn hex_lattice::NativeParameterAutomation,
            &mut crate::parameter_binding::Resolved<14>,
        )>,
        offset: usize,
        rate: u32,
        previous: Option<&crate::utility_filters::PreparedUtilityFilters>,
    ) -> Result<
        (
            StereoMatrix,
            Option<crate::utility_filters::PreparedUtilityFilters>,
        ),
        DeviceContractError,
    > {
        let reset = values.is_none();
        let pending = if let Some((values, resolved)) = values {
            self.controls.sample(&BINDINGS, resolved, values, offset)?
        } else {
            None
        };
        if !reset && pending.is_none() && previous.is_some() && self.width_ramp.is_none() {
            return Ok((self.matrix, None));
        }
        let mut settings = if reset {
            self.controls.base()
        } else {
            pending
                .as_ref()
                .map_or_else(|| self.controls.effective(), |next| next.effective())
        }
        .settings();
        let mut width_ramp = if reset { None } else { self.width_ramp };
        if let Some(ramp) = &mut width_ramp {
            settings.width = ramp.advance();
            if ramp.finished() {
                width_ramp = None;
            }
        }
        let filters = if previous.is_some()
            && settings.dc_remove == self.effective.dc_remove
            && settings.bass_mono == self.effective.bass_mono
            && settings.bass_mono_cutoff_hz == self.effective.bass_mono_cutoff_hz
            && settings.bass_mono_audition == self.effective.bass_mono_audition
            && settings.mute == self.effective.mute
        {
            None
        } else {
            Some(
                crate::utility_filters::PreparedUtilityFilters::new(
                    rate,
                    settings.dc_remove,
                    settings.bass_mono,
                    f64::from(settings.bass_mono_cutoff_hz),
                    settings.bass_mono_audition,
                )
                .map_err(|_| DeviceContractError::InvalidParameterValue)?
                .with_output_mute(settings.mute),
            )
        };
        // Reuse only stages whose controls are unchanged. The gain stage stays
        // between the exact original prefix and balance compositions, retaining
        // every f32 matrix rounding/validation operation of the static law.
        let prefix_changed = previous.is_none()
            || settings.channel_mode != self.effective.channel_mode
            || settings.invert_left != self.effective.invert_left
            || settings.invert_right != self.effective.invert_right
            || settings.width != self.effective.width
            || settings.mid_side_balance != self.effective.mid_side_balance
            || settings.mono != self.effective.mono;
        let balance_changed = previous.is_none() || settings.balance != self.effective.balance;
        let pre_gain = if prefix_changed {
            pre_gain_matrix(settings).map_err(|_| DeviceContractError::InvalidParameterValue)?
        } else {
            self.pre_gain
        };
        let post_gain = if balance_changed {
            balance_matrix(settings.balance)
                .map_err(|_| DeviceContractError::InvalidParameterValue)?
        } else {
            self.post_gain
        };
        let matrix = if prefix_changed || balance_changed || settings.gain != self.effective.gain {
            pre_gain
                .then(
                    scalar_matrix(settings.gain)
                        .map_err(|_| DeviceContractError::InvalidParameterValue)?,
                )
                .and_then(|m| m.then(post_gain))
                .map_err(|_| DeviceContractError::InvalidParameterValue)?
        } else {
            self.matrix
        };
        if reset {
            self.controls.reset();
        } else if let Some(pending) = pending {
            self.controls.install(pending);
        }
        self.effective = settings;
        self.matrix = matrix;
        self.pre_gain = pre_gain;
        self.post_gain = post_gain;
        self.width_ramp = width_ramp;
        Ok((matrix, filters))
    }
}

pub static UTILITY_DEFINITION: UtilityDefinition = UtilityDefinition;

/// Control-prepared Utility coefficients. Processing is allocation-free and has
/// no runtime state, lookahead, filtering, or buffering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreparedUtility {
    settings: UtilitySettings,
    matrix: StereoMatrix,
}

impl PreparedUtility {
    pub fn new(settings: UtilitySettings) -> Result<Self, UtilityError> {
        let mut matrix = pre_gain_matrix(settings)?;
        matrix = matrix.then(scalar_matrix(settings.gain)?)?;
        matrix = matrix.then(balance_matrix(settings.balance)?)?;
        if settings.mute {
            matrix = matrix.then(matrix_from_f64([[0.0, 0.0], [0.0, 0.0]])?)?;
        }
        Ok(Self { settings, matrix })
    }

    pub const fn settings(self) -> UtilitySettings {
        self.settings
    }

    pub const fn matrix(self) -> StereoMatrix {
        self.matrix
    }

    pub const fn latency_frames(self) -> u32 {
        0
    }

    pub fn process_frame(self, input: StereoFrame) -> StereoFrame {
        self.matrix.apply(input)
    }

    /// Process a caller-owned block without allocation or hidden limiting.
    pub fn process_block(self, frames: &mut [StereoFrame]) {
        self.matrix.apply_block(frames);
    }
}

fn pre_gain_matrix(settings: UtilitySettings) -> Result<StereoMatrix, UtilityError> {
    let mut matrix = StereoMatrix::identity();
    matrix = matrix.then(polarity_matrix(settings)?)?;
    matrix = matrix.then(channel_matrix(settings.channel_mode)?)?;

    // Left/Right monitoring duplicates one input channel; width and M/S
    // controls have no meaningful independent signal there and are bypassed.
    if !matches!(
        settings.channel_mode,
        ChannelMode::Left | ChannelMode::Right
    ) {
        if let Some(balance) = settings.mid_side_balance {
            matrix = matrix.then(mid_side_balance_matrix(balance)?)?;
        } else {
            matrix = matrix.then(width_matrix(settings.width)?)?;
        }
    }
    if settings.mono {
        matrix = matrix.then(matrix_from_f64([[0.5, 0.5], [0.5, 0.5]])?)?;
    }
    Ok(matrix)
}

fn polarity_matrix(settings: UtilitySettings) -> Result<StereoMatrix, UtilityError> {
    matrix_from_f64([
        [if settings.invert_left { -1.0 } else { 1.0 }, 0.0],
        [0.0, if settings.invert_right { -1.0 } else { 1.0 }],
    ])
}

fn channel_matrix(mode: ChannelMode) -> Result<StereoMatrix, UtilityError> {
    let values = match mode {
        ChannelMode::Stereo => [[1.0, 0.0], [0.0, 1.0]],
        ChannelMode::Left => [[1.0, 0.0], [1.0, 0.0]],
        ChannelMode::Right => [[0.0, 1.0], [0.0, 1.0]],
        ChannelMode::Swap => [[0.0, 1.0], [1.0, 0.0]],
    };
    matrix_from_f64(values)
}

fn width_matrix(width: f32) -> Result<StereoMatrix, UtilityError> {
    let width = f64::from(width);
    matrix_from_f64([
        [(1.0 + width) * 0.5, (1.0 - width) * 0.5],
        [(1.0 - width) * 0.5, (1.0 + width) * 0.5],
    ])
}

/// Original M/S control law: -1 keeps only mid, 0 is neutral, +1 keeps only side.
fn mid_side_balance_matrix(balance: f32) -> Result<StereoMatrix, UtilityError> {
    let balance = f64::from(balance);
    let (mid, side) = if balance < 0.0 {
        (1.0, 1.0 + balance)
    } else {
        (1.0 - balance, 1.0)
    };
    matrix_from_f64([
        [(mid + side) * 0.5, (mid - side) * 0.5],
        [(mid - side) * 0.5, (mid + side) * 0.5],
    ])
}

/// Center keeps both channels at unity; extremes attenuate one side to zero.
fn balance_matrix(balance: f32) -> Result<StereoMatrix, UtilityError> {
    let balance = f64::from(balance);
    let left = if balance > 0.0 { 1.0 - balance } else { 1.0 };
    let right = if balance < 0.0 { 1.0 + balance } else { 1.0 };
    matrix_from_f64([[left, 0.0], [0.0, right]])
}

fn scalar_matrix(gain: f32) -> Result<StereoMatrix, UtilityError> {
    let gain = f64::from(gain);
    matrix_from_f64([[gain, 0.0], [0.0, gain]])
}

fn matrix_from_f64(values: [[f64; 2]; 2]) -> Result<StereoMatrix, UtilityError> {
    let mut coefficients = [[0.0_f32; 2]; 2];
    for row in 0..2 {
        for column in 0..2 {
            let value = values[row][column];
            if !value.is_finite() || value.abs() > f64::from(f32::MAX) {
                return Err(UtilityError("utility coefficient is not representable"));
            }
            coefficients[row][column] = value as f32;
        }
    }
    StereoMatrix::new(coefficients).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concrete_memoryless_preparation_refuses_unresolved_asset_bindings() {
        use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
        let source = UTILITY_DEFINITION.default_state().unwrap();
        assert!(
            UtilityDefinition::prepare_owned_filter(&source, 48000)
                .unwrap()
                .is_none()
        );
        let bound = source
            .with_asset_references(vec![DeviceAssetReference {
                slot: DeviceAssetSlot::new(1).unwrap(),
                asset: DeviceAssetKey::new(42).unwrap(),
            }])
            .unwrap();
        assert!(matches!(
            UtilityDefinition::prepare_owned_filter(&bound, 48000),
            Err(DeviceContractError::AssetResourcesUnavailable)
        ));
    }

    #[allow(clippy::too_many_arguments)]
    fn settings(
        gain: f32,
        balance: f32,
        width: f32,
        mode: ChannelMode,
        invert_left: bool,
        invert_right: bool,
        mono: bool,
        mute: bool,
        mid_side_balance: Option<f32>,
    ) -> UtilitySettings {
        UtilitySettings::new(
            gain,
            balance,
            width,
            mode,
            invert_left,
            invert_right,
            mono,
            mute,
            mid_side_balance,
        )
        .unwrap()
    }

    fn close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.000_01,
            "{actual} != {expected}"
        );
    }

    #[test]
    fn default_is_exact_identity_and_declares_zero_latency() {
        let utility = PreparedUtility::new(UtilitySettings::default()).unwrap();
        let input = [f32::from_bits(0x8000_0000), 0.75];
        let output = utility.process_frame(input);
        assert_eq!(output[0].to_bits(), input[0].to_bits());
        assert_eq!(output[1].to_bits(), input[1].to_bits());
        assert_eq!(utility.latency_frames(), 0);
    }

    #[test]
    fn prepared_utility_filter_variants_declare_zero_algorithmic_latency() {
        let default_state = UTILITY_DEFINITION.default_state().unwrap();
        for (dc_remove, bass_mono) in [(true, false), (false, true), (true, true)] {
            let state = UTILITY_DEFINITION
                .patch_parameter(
                    &default_state,
                    parameter::DC_REMOVE,
                    ParameterValue::Bool(dc_remove),
                )
                .and_then(|state| {
                    UTILITY_DEFINITION.patch_parameter(
                        &state,
                        parameter::BASS_MONO,
                        ParameterValue::Bool(bass_mono),
                    )
                })
                .unwrap();
            let prepared = UTILITY_DEFINITION
                .prepare_processor(
                    &state,
                    &hex_lattice::DevicePreparationContext {
                        sample_rate: 48_000,
                        tuning: &hex_lattice::TuningTable::default(),
                        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                    },
                )
                .unwrap()
                .unwrap();
            assert_eq!(prepared.algorithmic_latency_frames(), Some(0));
            assert_eq!(prepared.output_tail(), OutputTail::Unbounded);
            assert!(prepared.create_runtime().is_quiescent());
        }
    }

    #[test]
    fn registered_utility_mute_ramps_and_suppresses_the_settled_filter_residual() {
        use hex_lattice_contract::ParameterValue;

        let default_state = UTILITY_DEFINITION.default_state().unwrap();
        for (dc_remove, bass_mono, audition) in [
            (true, false, false),
            (false, true, false),
            (false, true, true),
        ] {
            let mut state = default_state.clone();
            for (id, value) in [
                (parameter::DC_REMOVE, ParameterValue::Bool(dc_remove)),
                (parameter::BASS_MONO, ParameterValue::Bool(bass_mono)),
                (
                    parameter::BASS_MONO_AUDITION,
                    ParameterValue::Bool(audition),
                ),
            ] {
                state = UTILITY_DEFINITION
                    .patch_parameter(&state, id, value)
                    .unwrap();
            }
            let mut source = UTILITY_DEFINITION
                .prepare_processor(
                    &state,
                    &hex_lattice::DevicePreparationContext {
                        sample_rate: 48_000,
                        tuning: &hex_lattice::TuningTable::default(),
                        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                    },
                )
                .unwrap()
                .unwrap()
                .create_runtime();
            let matrix = UTILITY_DEFINITION.prepare_effect_matrix(&state).unwrap();
            for _ in 0..512 {
                let mut frame = [[0.75, -0.25]];
                matrix.apply_block(&mut frame);
                source.process_block(&mut frame, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            }

            let muted_state = UTILITY_DEFINITION
                .patch_parameter(&state, parameter::MUTE, ParameterValue::Bool(true))
                .unwrap();
            let muted_matrix = UTILITY_DEFINITION
                .prepare_effect_matrix(&muted_state)
                .unwrap();
            let mut candidate = UTILITY_DEFINITION
                .prepare_processor(
                    &muted_state,
                    &hex_lattice::DevicePreparationContext {
                        sample_rate: 48_000,
                        tuning: &hex_lattice::TuningTable::default(),
                        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                    },
                )
                .unwrap()
                .unwrap()
                .create_runtime();
            assert!(candidate.can_copy_history_from(source.as_ref()));
            candidate.copy_history_from(source.as_ref());

            const RAMP_FRAMES: usize = 240;
            let mut muted = [[0.5, -0.125]; RAMP_FRAMES];
            muted_matrix.apply_block(&mut muted);
            let mut oracle = muted;
            oracle[RAMP_FRAMES - 1] = [0.0; 2];
            source.process_block(&mut oracle, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            candidate.process_block(&mut muted, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            for (frame, (actual, expected)) in muted.into_iter().zip(oracle).enumerate() {
                let gain = 1.0 - (frame + 1) as f32 / RAMP_FRAMES as f32;
                for channel in 0..2 {
                    assert!((actual[channel] - expected[channel] * gain).abs() < 1.0e-7);
                }
            }

            let mut silence = [[0.5, -0.125]; 16];
            muted_matrix.apply_block(&mut silence);
            candidate.process_block(&mut silence, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            assert!(
                silence.iter().all(|frame| *frame == [0.0, 0.0]),
                "mute leaked filter residual for dc={dc_remove}, bass={bass_mono}, audition={audition}"
            );

            let mut oracle_silence = [[0.0, 0.0]; 16];
            source.process_block(&mut oracle_silence, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            let mut unmuted = UTILITY_DEFINITION
                .prepare_processor(
                    &state,
                    &hex_lattice::DevicePreparationContext {
                        sample_rate: 48_000,
                        tuning: &hex_lattice::TuningTable::default(),
                        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                    },
                )
                .unwrap()
                .unwrap()
                .create_runtime();
            assert!(unmuted.can_copy_history_from(candidate.as_ref()));
            unmuted.copy_history_from(candidate.as_ref());
            for frame in 0..RAMP_FRAMES + 16 {
                let input = [[0.2 + frame as f32 * 0.001, -0.4 + frame as f32 * 0.002]];
                let mut actual = input;
                let mut expected = input;
                unmuted.process_block(&mut actual, &hex_lattice::EMPTY_DEVICE_RESOURCES);
                source.process_block(&mut expected, &hex_lattice::EMPTY_DEVICE_RESOURCES);
                let gain = ((frame + 1) as f32 / RAMP_FRAMES as f32).min(1.0);
                for channel in 0..2 {
                    assert!((actual[0][channel] - expected[0][channel] * gain).abs() < 1.0e-7);
                }
            }
            candidate.copy_history_from(unmuted.as_ref());
            let mut signal = [[0.5, -0.125]];
            candidate.process_block(&mut signal, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            assert_ne!(signal, [[0.0; 2]], "mute starts from the audible history");
            candidate.reset();
            let mut signal = [[0.5, -0.125]];
            candidate.process_block(&mut signal, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            assert_eq!(signal, [[0.0; 2]], "reset applies the authored mute");
        }
    }

    #[test]
    fn channel_modes_polarity_and_stereo_width_have_defined_matrices() {
        let frame = [0.4, -0.2];
        let left = PreparedUtility::new(settings(
            1.0,
            0.0,
            3.0,
            ChannelMode::Left,
            false,
            false,
            false,
            false,
            Some(1.0),
        ))
        .unwrap()
        .process_frame(frame);
        assert_eq!(left, [0.4, 0.4]);
        let right = PreparedUtility::new(settings(
            1.0,
            0.0,
            1.0,
            ChannelMode::Right,
            false,
            true,
            false,
            false,
            None,
        ))
        .unwrap()
        .process_frame(frame);
        assert_eq!(right, [0.2, 0.2]);
        let swapped_and_inverted = PreparedUtility::new(settings(
            1.0,
            0.0,
            1.0,
            ChannelMode::Swap,
            true,
            false,
            false,
            false,
            None,
        ))
        .unwrap()
        .process_frame(frame);
        assert_eq!(swapped_and_inverted, [-0.2, -0.4]);
        let mono_width = PreparedUtility::new(settings(
            1.0,
            0.0,
            0.0,
            ChannelMode::Stereo,
            false,
            false,
            false,
            false,
            None,
        ))
        .unwrap()
        .process_frame(frame);
        assert_eq!(mono_width, [0.1, 0.1]);
        let wide = PreparedUtility::new(settings(
            1.0,
            0.0,
            2.0,
            ChannelMode::Stereo,
            false,
            false,
            false,
            false,
            None,
        ))
        .unwrap()
        .process_frame(frame);
        close(wide[0], 0.7);
        close(wide[1], -0.5);
    }

    #[test]
    fn mid_side_balance_endpoints_and_mono_are_explicit() {
        let input = [0.8, 0.2];
        let mid_only = PreparedUtility::new(settings(
            1.0,
            0.0,
            3.0,
            ChannelMode::Stereo,
            false,
            false,
            false,
            false,
            Some(-1.0),
        ))
        .unwrap()
        .process_frame(input);
        assert_eq!(mid_only, [0.5, 0.5]);
        let side_only = PreparedUtility::new(settings(
            1.0,
            0.0,
            3.0,
            ChannelMode::Stereo,
            false,
            false,
            false,
            false,
            Some(1.0),
        ))
        .unwrap()
        .process_frame(input);
        assert_eq!(side_only, [0.3, -0.3]);
        let mono = PreparedUtility::new(settings(
            1.0,
            0.0,
            2.0,
            ChannelMode::Stereo,
            false,
            false,
            true,
            false,
            None,
        ))
        .unwrap()
        .process_frame(input);
        assert_eq!(mono, [0.5, 0.5]);
    }

    #[test]
    fn output_balance_gain_zero_mute_and_block_processing_are_defined() {
        let right_only = PreparedUtility::new(settings(
            2.0,
            1.0,
            1.0,
            ChannelMode::Stereo,
            false,
            false,
            false,
            false,
            None,
        ))
        .unwrap();
        assert_eq!(right_only.process_frame([0.4, 0.3]), [0.0, 0.6]);
        let silent = PreparedUtility::new(settings(
            0.0,
            0.0,
            1.0,
            ChannelMode::Stereo,
            false,
            false,
            false,
            false,
            None,
        ))
        .unwrap();
        assert_eq!(silent.process_frame([1.0, -1.0]), [0.0, 0.0]);
        let muted = PreparedUtility::new(settings(
            1.0,
            0.0,
            1.0,
            ChannelMode::Stereo,
            false,
            false,
            false,
            true,
            None,
        ))
        .unwrap();
        assert_eq!(muted.process_frame([1.0, -1.0]), [0.0, 0.0]);

        let mut block = [[0.2, 0.4], [-0.6, 0.8], [0.0, 0.1]];
        let expected = block.map(|frame| right_only.process_frame(frame));
        right_only.process_block(&mut block);
        assert_eq!(block, expected);
        assert!(
            PreparedUtility::new(settings(
                MAX_UTILITY_GAIN,
                -1.0,
                4.0,
                ChannelMode::Stereo,
                true,
                true,
                false,
                false,
                None,
            ))
            .is_ok()
        );
    }

    #[test]
    fn invalid_utility_settings_are_rejected() {
        let invalid = [
            (f32::NAN, 0.0, 1.0, None),
            (f32::INFINITY, 0.0, 1.0, None),
            (-0.1, 0.0, 1.0, None),
            (MAX_UTILITY_GAIN + 1.0, 0.0, 1.0, None),
            (1.0, f32::NAN, 1.0, None),
            (1.0, 1.1, 1.0, None),
            (1.0, 0.0, f32::INFINITY, None),
            (1.0, 0.0, -0.1, None),
            (1.0, 0.0, 4.1, None),
            (1.0, 0.0, 1.0, Some(f32::NAN)),
            (1.0, 0.0, 1.0, Some(-1.1)),
            (1.0, 0.0, 1.0, Some(1.1)),
        ];
        for (gain, balance, width, ms) in invalid {
            assert!(
                UtilitySettings::new(
                    gain,
                    balance,
                    width,
                    ChannelMode::Stereo,
                    false,
                    false,
                    false,
                    false,
                    ms,
                )
                .is_err()
            );
        }
    }

    #[test]
    fn utility_definition_owns_validated_portable_state_and_generic_metadata() {
        use hex_lattice::NativeDeviceRegistry;
        use hex_lattice_contract::{NativeDeviceMetadata, ParameterPresentation, ParameterValue};

        let definitions: [&dyn NativeDeviceDefinition; 1] = [&UTILITY_DEFINITION];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        let state = registry.default_state(UTILITY_TYPE_ID).unwrap();
        assert_eq!(registry.migrate_state(&state).unwrap(), state);
        assert_eq!(
            registry
                .definition(UTILITY_TYPE_ID)
                .unwrap()
                .descriptors()
                .len(),
            14
        );
        for descriptor in UTILITY_DEFINITION.descriptors() {
            descriptor.validate_descriptor().unwrap();
        }
        assert_eq!(
            UTILITY_DEFINITION.descriptors()[0].presentation,
            ParameterPresentation::LinearAmplitudeDecibels
        );
        assert!(matches!(
            UTILITY_DEFINITION.descriptors()[0].kind,
            ParameterKind::Float {
                unit: Some("linear"),
                ..
            }
        ));
        assert!(
            UTILITY_DEFINITION.descriptors()[1..]
                .iter()
                .all(|descriptor| descriptor.presentation == ParameterPresentation::Linear)
        );

        let patched = registry
            .patch_parameter(&state, parameter::GAIN, ParameterValue::Float(2.0))
            .unwrap();
        assert_eq!(
            UTILITY_DEFINITION
                .read_parameter(&patched, parameter::GAIN)
                .unwrap(),
            ParameterValue::Float(2.0)
        );
        assert_eq!(
            UTILITY_DEFINITION
                .read_parameter(&patched, parameter::WIDTH)
                .unwrap(),
            ParameterValue::Float(1.0)
        );
        assert!(
            registry
                .patch_parameter(&patched, parameter::GAIN, ParameterValue::Float(f32::NAN))
                .is_err()
        );
        assert!(
            registry
                .patch_parameter(
                    &patched,
                    parameter::CHANNEL,
                    ParameterValue::Choice(u32::MAX)
                )
                .is_err()
        );

        let malformed = NativeDeviceState::new(UTILITY_TYPE_ID, 1, vec![0; 18]).unwrap();
        assert!(registry.migrate_state(&malformed).is_err());
        let future =
            NativeDeviceState::new(UTILITY_TYPE_ID, 99, patched.payload().to_vec()).unwrap();
        assert!(registry.migrate_state(&future).is_err());
    }

    #[test]
    fn utility_width_and_mid_side_availability_tracks_channel_mode_without_resetting_values() {
        use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

        let mut state = UTILITY_DEFINITION.default_state().unwrap();
        for (parameter, value) in [
            (parameter::WIDTH, ParameterValue::Float(2.5)),
            (parameter::MID_SIDE_ENABLED, ParameterValue::Bool(true)),
            (parameter::MID_SIDE_BALANCE, ParameterValue::Float(-0.4)),
        ] {
            state = UTILITY_DEFINITION
                .patch_parameter(&state, parameter, value)
                .unwrap();
        }
        let conditional = [
            parameter::WIDTH,
            parameter::MID_SIDE_ENABLED,
            parameter::MID_SIDE_BALANCE,
        ];

        for mode in [1, 2] {
            let left_or_right = UTILITY_DEFINITION
                .patch_parameter(&state, parameter::CHANNEL, ParameterValue::Choice(mode))
                .unwrap();
            for parameter in conditional {
                assert!(
                    !UTILITY_DEFINITION
                        .parameter_enabled(&left_or_right, parameter)
                        .unwrap()
                );
            }
            assert_eq!(
                UTILITY_DEFINITION
                    .read_parameter(&left_or_right, parameter::WIDTH)
                    .unwrap(),
                ParameterValue::Float(2.5)
            );
            assert_eq!(
                UTILITY_DEFINITION
                    .read_parameter(&left_or_right, parameter::MID_SIDE_ENABLED)
                    .unwrap(),
                ParameterValue::Bool(true)
            );
            assert_eq!(
                UTILITY_DEFINITION
                    .read_parameter(&left_or_right, parameter::MID_SIDE_BALANCE)
                    .unwrap(),
                ParameterValue::Float(-0.4)
            );
        }

        for mode in [0, 3] {
            let stereo_or_swap = UTILITY_DEFINITION
                .patch_parameter(&state, parameter::CHANNEL, ParameterValue::Choice(mode))
                .unwrap();
            for parameter in conditional {
                assert!(
                    UTILITY_DEFINITION
                        .parameter_enabled(&stereo_or_swap, parameter)
                        .unwrap()
                );
            }
        }

        assert!(
            UTILITY_DEFINITION
                .parameter_enabled(&state, ParameterId::from_static(99))
                .is_err()
        );
        let malformed = NativeDeviceState::new(UTILITY_TYPE_ID, 2, vec![0; 19]).unwrap();
        assert!(
            UTILITY_DEFINITION
                .parameter_enabled(&malformed, parameter::WIDTH)
                .is_err()
        );
    }

    #[test]
    fn schema_one_utility_state_migrates_to_neutral_stateful_defaults() {
        use hex_lattice::NativeDeviceRegistry;
        use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

        let definitions: [&dyn NativeDeviceDefinition; 1] = [&UTILITY_DEFINITION];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        let schema_two = registry.default_state(UTILITY_TYPE_ID).unwrap();
        let schema_one =
            NativeDeviceState::new(UTILITY_TYPE_ID, 1, schema_two.payload()[..19].to_vec())
                .unwrap();
        let migrated = registry.migrate_state(&schema_one).unwrap();
        assert_eq!(migrated.state_version(), 2);
        assert_eq!(migrated.payload().len(), 26);
        assert_eq!(
            UTILITY_DEFINITION
                .read_parameter(&migrated, parameter::DC_REMOVE)
                .unwrap(),
            ParameterValue::Bool(false)
        );
        assert_eq!(
            UTILITY_DEFINITION
                .read_parameter(&migrated, parameter::BASS_MONO)
                .unwrap(),
            ParameterValue::Bool(false)
        );
        assert_eq!(
            UTILITY_DEFINITION
                .read_parameter(&migrated, parameter::BASS_MONO_CUTOFF)
                .unwrap(),
            ParameterValue::Float(200.0)
        );
        assert_eq!(
            UTILITY_DEFINITION
                .read_parameter(&migrated, parameter::BASS_MONO_AUDITION)
                .unwrap(),
            ParameterValue::Bool(false)
        );
    }

    #[test]
    fn inactive_mid_side_balance_remains_validated_and_round_trips_independently() {
        use hex_lattice::NativeDeviceRegistry;
        use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

        let definitions: [&dyn NativeDeviceDefinition; 1] = [&UTILITY_DEFINITION];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        let mut state = registry.default_state(UTILITY_TYPE_ID).unwrap();

        state = registry
            .patch_parameter(
                &state,
                parameter::MID_SIDE_BALANCE,
                ParameterValue::Float(-0.75),
            )
            .unwrap();
        assert_eq!(
            UTILITY_DEFINITION
                .read_parameter(&state, parameter::MID_SIDE_ENABLED)
                .unwrap(),
            ParameterValue::Bool(false)
        );
        assert_eq!(
            UTILITY_DEFINITION
                .read_parameter(&state, parameter::MID_SIDE_BALANCE)
                .unwrap(),
            ParameterValue::Float(-0.75)
        );

        state = registry
            .patch_parameter(
                &state,
                parameter::MID_SIDE_ENABLED,
                ParameterValue::Bool(true),
            )
            .unwrap();
        state = registry
            .patch_parameter(
                &state,
                parameter::MID_SIDE_BALANCE,
                ParameterValue::Float(0.75),
            )
            .unwrap();
        state = registry
            .patch_parameter(
                &state,
                parameter::MID_SIDE_ENABLED,
                ParameterValue::Bool(false),
            )
            .unwrap();
        state = registry
            .patch_parameter(&state, parameter::GAIN, ParameterValue::Float(2.0))
            .unwrap();
        assert_eq!(
            UTILITY_DEFINITION
                .read_parameter(&state, parameter::MID_SIDE_BALANCE)
                .unwrap(),
            ParameterValue::Float(0.75)
        );
        state = registry
            .patch_parameter(
                &state,
                parameter::MID_SIDE_ENABLED,
                ParameterValue::Bool(true),
            )
            .unwrap();
        assert_eq!(
            UTILITY_DEFINITION
                .read_parameter(&state, parameter::MID_SIDE_BALANCE)
                .unwrap(),
            ParameterValue::Float(0.75)
        );

        let mut malformed_payload = registry
            .default_state(UTILITY_TYPE_ID)
            .unwrap()
            .payload()
            .to_vec();
        malformed_payload[15..19].copy_from_slice(&f32::NAN.to_le_bytes());
        let malformed = NativeDeviceState::new(UTILITY_TYPE_ID, 2, malformed_payload).unwrap();
        assert!(registry.migrate_state(&malformed).is_err());
    }

    #[test]
    fn legacy_nineteen_byte_state_migrates_to_filter_defaults() {
        use hex_lattice::NativeDeviceRegistry;
        use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

        let definitions: [&dyn NativeDeviceDefinition; 1] = [&UTILITY_DEFINITION];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        let legacy = NativeDeviceState::new(
            UTILITY_TYPE_ID,
            1,
            UTILITY_DEFINITION.default_state().unwrap().payload()[..19].to_vec(),
        )
        .unwrap();
        let migrated = registry.migrate_state(&legacy).unwrap();
        assert_eq!(migrated.state_version(), 2);
        assert_eq!(migrated.payload().len(), 26);
        for (id, expected) in [
            (parameter::DC_REMOVE, ParameterValue::Bool(false)),
            (parameter::BASS_MONO, ParameterValue::Bool(false)),
            (parameter::BASS_MONO_CUTOFF, ParameterValue::Float(200.0)),
            (parameter::BASS_MONO_AUDITION, ParameterValue::Bool(false)),
        ] {
            assert_eq!(
                UTILITY_DEFINITION.read_parameter(&migrated, id).unwrap(),
                expected
            );
        }
    }
}

#[cfg(test)]
#[path = "utility/automation_tests.rs"]
mod automation_tests;
#[cfg(test)]
mod width_ramp_tests;

#[cfg(test)]
mod bypass_tests;
