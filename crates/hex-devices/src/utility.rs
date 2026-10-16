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
const DESCRIPTORS: &[ParameterDescriptor] = &[
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
    ParameterDescriptor {
        id: parameter::CHANNEL,
        label: "channel",
        kind: ParameterKind::Choice {
            labels: CHANNEL_LABELS,
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::INVERT_LEFT,
        label: "invert left",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::INVERT_RIGHT,
        label: "invert right",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::MONO,
        label: "mono",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::MUTE,
        label: "mute",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::MID_SIDE_ENABLED,
        label: "mid-side",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
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
    ParameterDescriptor {
        id: parameter::DC_REMOVE,
        label: "dc remove",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::BASS_MONO,
        label: "bass mono",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
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
    ParameterDescriptor {
        id: parameter::BASS_MONO_AUDITION,
        label: "bass mono audition",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
];

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
        let (settings, mid_side_enabled, mid_side_balance) = decode_state_fields(state)?;
        Ok(match id {
            parameter::GAIN => ParameterValue::Float(settings.gain),
            parameter::BALANCE => ParameterValue::Float(settings.balance),
            parameter::WIDTH => ParameterValue::Float(settings.width),
            parameter::CHANNEL => ParameterValue::Choice(match settings.channel_mode {
                ChannelMode::Stereo => 0,
                ChannelMode::Left => 1,
                ChannelMode::Right => 2,
                ChannelMode::Swap => 3,
            }),
            parameter::INVERT_LEFT => ParameterValue::Bool(settings.invert_left),
            parameter::INVERT_RIGHT => ParameterValue::Bool(settings.invert_right),
            parameter::MONO => ParameterValue::Bool(settings.mono),
            parameter::MUTE => ParameterValue::Bool(settings.mute),
            parameter::MID_SIDE_ENABLED => ParameterValue::Bool(mid_side_enabled),
            parameter::MID_SIDE_BALANCE => ParameterValue::Float(mid_side_balance),
            parameter::DC_REMOVE => ParameterValue::Bool(settings.dc_remove),
            parameter::BASS_MONO => ParameterValue::Bool(settings.bass_mono),
            parameter::BASS_MONO_CUTOFF => ParameterValue::Float(settings.bass_mono_cutoff_hz),
            parameter::BASS_MONO_AUDITION => ParameterValue::Bool(settings.bass_mono_audition),
            _ => return Err(DeviceContractError::UnknownParameter),
        })
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
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let settings = decode_state(state)?;
        let descriptor = DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.id == id)
            .ok_or(DeviceContractError::UnknownParameter)?;
        descriptor.validate(value)?;
        let (mut gain, mut balance, mut width, mut mode) = (
            settings.gain,
            settings.balance,
            settings.width,
            settings.channel_mode,
        );
        let (_, mut mid_side_enabled, mut mid_side_balance) = decode_state_fields(state)?;
        let mut dc_remove = settings.dc_remove;
        let mut bass_mono = settings.bass_mono;
        let mut bass_mono_cutoff_hz = settings.bass_mono_cutoff_hz;
        let mut bass_mono_audition = settings.bass_mono_audition;
        let (mut left, mut right, mut mono, mut mute) = (
            settings.invert_left,
            settings.invert_right,
            settings.mono,
            settings.mute,
        );
        match (id, value) {
            (parameter::GAIN, ParameterValue::Float(v)) => gain = v,
            (parameter::BALANCE, ParameterValue::Float(v)) => balance = v,
            (parameter::WIDTH, ParameterValue::Float(v)) => width = v,
            (parameter::CHANNEL, ParameterValue::Choice(v)) => {
                mode = match v {
                    0 => ChannelMode::Stereo,
                    1 => ChannelMode::Left,
                    2 => ChannelMode::Right,
                    3 => ChannelMode::Swap,
                    _ => return Err(DeviceContractError::InvalidParameterValue),
                }
            }
            (parameter::INVERT_LEFT, ParameterValue::Bool(v)) => left = v,
            (parameter::INVERT_RIGHT, ParameterValue::Bool(v)) => right = v,
            (parameter::MONO, ParameterValue::Bool(v)) => mono = v,
            (parameter::MUTE, ParameterValue::Bool(v)) => mute = v,
            (parameter::MID_SIDE_ENABLED, ParameterValue::Bool(v)) => mid_side_enabled = v,
            (parameter::MID_SIDE_BALANCE, ParameterValue::Float(v)) => mid_side_balance = v,
            (parameter::DC_REMOVE, ParameterValue::Bool(v)) => dc_remove = v,
            (parameter::BASS_MONO, ParameterValue::Bool(v)) => bass_mono = v,
            (parameter::BASS_MONO_CUTOFF, ParameterValue::Float(v)) => bass_mono_cutoff_hz = v,
            (parameter::BASS_MONO_AUDITION, ParameterValue::Bool(v)) => bass_mono_audition = v,
            _ => return Err(DeviceContractError::InvalidParameterValue),
        }
        let settings = UtilitySettings::new(
            gain,
            balance,
            width,
            mode,
            left,
            right,
            mono,
            mute,
            mid_side_enabled.then_some(mid_side_balance),
        )
        .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        let settings = UtilitySettings {
            dc_remove,
            bass_mono,
            bass_mono_cutoff_hz,
            bass_mono_audition,
            ..settings
        };
        encode_state_fields(settings, mid_side_enabled, mid_side_balance)
    }
}

impl NativeDeviceDefinition for UtilityDefinition {
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
        sample_rate: u32,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        Self::prepare_owned_filter(state, sample_rate)
            .map(|value| value.map(|value| Box::new(value) as Box<dyn PreparedNativeProcessor>))
    }
}

/// The concrete fixed-inline Utility filter preparation. Its private fields
/// cannot contain caller-provided processor/storage implementations. Native
/// graph construction can retain this exact type's allocation declaration
/// before erasing the processor trait; this is not a generic provider receipt.
pub struct PreparedUtilityFilterRuntime(crate::utility_filters::PreparedUtilityFilters);

impl UtilityDefinition {
    /// Prepare the same registered filter through its concrete constructor.
    /// Memoryless Utility settings return None, exactly as the device registry.
    pub fn prepare_owned_filter(
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Option<PreparedUtilityFilterRuntime>, DeviceContractError> {
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
        Ok(Some(PreparedUtilityFilterRuntime(prepared)))
    }
}

impl PreparedNativeProcessor for PreparedUtilityFilterRuntime {
    fn runtime_version(&self) -> u32 {
        1
    }

    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<crate::utility_filters::UtilityFilterProcessor>()
    }

    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(crate::utility_filters::UtilityFilterProcessor::new(self.0))
    }

    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(self.0.algorithmic_latency_frames())
    }

    fn output_tail(&self) -> OutputTail {
        OutputTail::Unbounded
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
                .prepare_processor(&state, 48_000)
                .unwrap()
                .unwrap();
            assert_eq!(prepared.algorithmic_latency_frames(), Some(0));
            assert_eq!(prepared.output_tail(), OutputTail::Unbounded);
            assert!(prepared.create_runtime().is_quiescent());
        }
    }

    #[test]
    fn registered_utility_mute_suppresses_filter_residual_after_history_copy() {
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
                .prepare_processor(&state, 48_000)
                .unwrap()
                .unwrap()
                .create_runtime();
            let matrix = UTILITY_DEFINITION.prepare_effect_matrix(&state).unwrap();
            for _ in 0..512 {
                let mut frame = [[0.75, -0.25]];
                matrix.apply_block(&mut frame);
                source.process_block(&mut frame);
            }

            let muted_state = UTILITY_DEFINITION
                .patch_parameter(&state, parameter::MUTE, ParameterValue::Bool(true))
                .unwrap();
            let muted_matrix = UTILITY_DEFINITION
                .prepare_effect_matrix(&muted_state)
                .unwrap();
            let mut candidate = UTILITY_DEFINITION
                .prepare_processor(&muted_state, 48_000)
                .unwrap()
                .unwrap()
                .create_runtime();
            assert!(candidate.can_copy_history_from(source.as_ref()));
            candidate.copy_history_from(source.as_ref());

            let mut silence = [[0.5, -0.125]; 16];
            muted_matrix.apply_block(&mut silence);
            candidate.process_block(&mut silence);
            assert!(
                silence.iter().all(|frame| *frame == [0.0, 0.0]),
                "mute leaked filter residual for dc={dc_remove}, bass={bass_mono}, audition={audition}"
            );

            let mut oracle_silence = [[0.0, 0.0]; 16];
            source.process_block(&mut oracle_silence);
            let mut unmuted = UTILITY_DEFINITION
                .prepare_processor(&state, 48_000)
                .unwrap()
                .unwrap()
                .create_runtime();
            assert!(unmuted.can_copy_history_from(candidate.as_ref()));
            unmuted.copy_history_from(candidate.as_ref());
            for frame in 0..32 {
                let input = [[0.2 + frame as f32 * 0.001, -0.4 + frame as f32 * 0.002]];
                let mut actual = input;
                let mut expected = input;
                unmuted.process_block(&mut actual);
                source.process_block(&mut expected);
                assert_eq!(actual, expected);
            }
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
