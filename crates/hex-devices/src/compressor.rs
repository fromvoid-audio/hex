//! Original zero-lookahead, stereo-linked feed-forward dynamics processor.

use hex_lattice::{
    NativeDeviceDefinition, NativeProcessorRuntime, OutputTail, PreparedNativeProcessor,
    StereoFrame, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;

pub const COMPRESSOR_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.compressor");
pub const COMPRESSOR_STATE_VERSION: u32 = 1;
const STATE_BYTES: usize = 24;
const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;
const GAIN_SMOOTH_MS: f64 = 5.0;
const SILENCE_FLOOR_DB: f64 = -120.0;

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const THRESHOLD: ParameterId = ParameterId::from_static(1);
    pub const RATIO: ParameterId = ParameterId::from_static(2);
    pub const ATTACK: ParameterId = ParameterId::from_static(3);
    pub const RELEASE: ParameterId = ParameterId::from_static(4);
    pub const KNEE: ParameterId = ParameterId::from_static(5);
    pub const MAKEUP: ParameterId = ParameterId::from_static(6);
}

const DESCRIPTORS: &[ParameterDescriptor] = &[
    ParameterDescriptor {
        id: parameter::THRESHOLD,
        label: "threshold",
        kind: ParameterKind::Float {
            min: -60.0,
            max: 0.0,
            unit: Some("db"),
        },
        default: ParameterValue::Float(-18.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::RATIO,
        label: "ratio",
        kind: ParameterKind::Float {
            min: 1.0,
            max: 20.0,
            unit: None,
        },
        default: ParameterValue::Float(4.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::ATTACK,
        label: "attack",
        kind: ParameterKind::Float {
            min: 0.1,
            max: 100.0,
            unit: Some("ms"),
        },
        default: ParameterValue::Float(10.0),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: parameter::RELEASE,
        label: "release",
        kind: ParameterKind::Float {
            min: 5.0,
            max: 1_000.0,
            unit: Some("ms"),
        },
        default: ParameterValue::Float(100.0),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: parameter::KNEE,
        label: "knee",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 12.0,
            unit: Some("db"),
        },
        default: ParameterValue::Float(6.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::MAKEUP,
        label: "makeup",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 24.0,
            unit: Some("db"),
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
];

#[derive(Clone, Copy, Debug, PartialEq)]
struct CompressorSettings {
    threshold_db: f32,
    ratio: f32,
    attack_ms: f32,
    release_ms: f32,
    knee_db: f32,
    makeup_db: f32,
}

impl Default for CompressorSettings {
    fn default() -> Self {
        Self {
            threshold_db: -18.0,
            ratio: 4.0,
            attack_ms: 10.0,
            release_ms: 100.0,
            knee_db: 6.0,
            makeup_db: 0.0,
        }
    }
}

fn decode(state: &NativeDeviceState) -> Result<CompressorSettings, DeviceContractError> {
    if state.type_id() != COMPRESSOR_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != COMPRESSOR_STATE_VERSION {
        return Err(DeviceContractError::UnsupportedStateVersion);
    }
    let bytes = state.payload();
    if bytes.len() != STATE_BYTES {
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
    let settings = CompressorSettings {
        threshold_db: read(0),
        ratio: read(4),
        attack_ms: read(8),
        release_ms: read(12),
        knee_db: read(16),
        makeup_db: read(20),
    };
    validate(settings)?;
    Ok(settings)
}

fn validate(settings: CompressorSettings) -> Result<(), DeviceContractError> {
    for (id, value) in [
        (parameter::THRESHOLD, settings.threshold_db),
        (parameter::RATIO, settings.ratio),
        (parameter::ATTACK, settings.attack_ms),
        (parameter::RELEASE, settings.release_ms),
        (parameter::KNEE, settings.knee_db),
        (parameter::MAKEUP, settings.makeup_db),
    ] {
        let descriptor = DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.id == id)
            .ok_or(DeviceContractError::UnknownParameter)?;
        descriptor.validate(ParameterValue::Float(value))?;
    }
    Ok(())
}

fn encode(settings: CompressorSettings) -> Result<NativeDeviceState, DeviceContractError> {
    validate(settings)?;
    let mut payload = Vec::with_capacity(STATE_BYTES);
    for value in [
        settings.threshold_db,
        settings.ratio,
        settings.attack_ms,
        settings.release_ms,
        settings.knee_db,
        settings.makeup_db,
    ] {
        payload.extend_from_slice(&value.to_le_bytes());
    }
    NativeDeviceState::new(COMPRESSOR_TYPE_ID, COMPRESSOR_STATE_VERSION, payload)
}

/// Built-in Compressor metadata, portable state codec, and control-side processor.
pub struct CompressorDefinition;

pub static COMPRESSOR_DEFINITION: CompressorDefinition = CompressorDefinition;

impl NativeDeviceMetadata for CompressorDefinition {
    fn type_id(&self) -> DeviceTypeId {
        COMPRESSOR_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("compressor")
    }
    fn state_version(&self) -> u32 {
        COMPRESSOR_STATE_VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        encode(CompressorSettings::default())
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
        let value = match id {
            parameter::THRESHOLD => settings.threshold_db,
            parameter::RATIO => settings.ratio,
            parameter::ATTACK => settings.attack_ms,
            parameter::RELEASE => settings.release_ms,
            parameter::KNEE => settings.knee_db,
            parameter::MAKEUP => settings.makeup_db,
            _ => return Err(DeviceContractError::UnknownParameter),
        };
        Ok(ParameterValue::Float(value))
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut settings = decode(state)?;
        patch_settings(&mut settings, id, value)?;
        encode(settings)
    }
}

fn patch_settings(
    settings: &mut CompressorSettings,
    id: ParameterId,
    value: ParameterValue,
) -> Result<(), DeviceContractError> {
    let descriptor = DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.id == id)
        .ok_or(DeviceContractError::UnknownParameter)?;
    descriptor.validate(value)?;
    let ParameterValue::Float(value) = value else {
        return Err(DeviceContractError::InvalidParameterValue);
    };
    match id {
        parameter::THRESHOLD => settings.threshold_db = value,
        parameter::RATIO => settings.ratio = value,
        parameter::ATTACK => settings.attack_ms = value,
        parameter::RELEASE => settings.release_ms = value,
        parameter::KNEE => settings.knee_db = value,
        parameter::MAKEUP => settings.makeup_db = value,
        _ => return Err(DeviceContractError::UnknownParameter),
    }
    Ok(())
}

impl NativeDeviceDefinition for CompressorDefinition {
    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        let _ = decode(state)?;
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_effect_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        let _ = decode(state)?;
        Ok(StereoMatrix::identity())
    }
    fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        let sample_rate = context.sample_rate;
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let settings = decode(state)?;
        let samples_per_ms = f64::from(sample_rate) / 1_000.0;
        let attack_alpha = (-1.0 / (f64::from(settings.attack_ms) * samples_per_ms)).exp();
        let release_alpha = (-1.0 / (f64::from(settings.release_ms) * samples_per_ms)).exp();
        let gain_alpha = (-1.0 / (GAIN_SMOOTH_MS * samples_per_ms)).exp();
        Ok(Some(Box::new(PreparedCompressor {
            settings,
            sample_rate,
            attack_alpha,
            release_alpha,
            gain_alpha,
        })))
    }
}

struct PreparedCompressor {
    settings: CompressorSettings,
    sample_rate: u32,
    attack_alpha: f64,
    release_alpha: f64,
    gain_alpha: f64,
}

impl PreparedNativeProcessor for PreparedCompressor {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<CompressorRuntime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(CompressorRuntime {
            settings: self.settings,
            sample_rate: self.sample_rate,
            attack_alpha: self.attack_alpha,
            release_alpha: self.release_alpha,
            gain_alpha: self.gain_alpha,
            envelope: 0.0,
            gain_db: f64::from(self.settings.makeup_db),
        })
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite { max_tail_frames: 0 }
    }
}

struct CompressorRuntime {
    settings: CompressorSettings,
    sample_rate: u32,
    attack_alpha: f64,
    release_alpha: f64,
    gain_alpha: f64,
    envelope: f64,
    gain_db: f64,
}

impl CompressorRuntime {
    fn reset_history(&mut self) {
        self.envelope = 0.0;
        self.gain_db = f64::from(self.settings.makeup_db);
    }

    fn process_frame(&mut self, frame: &mut StereoFrame) {
        let left = f64::from(frame[0]);
        let right = f64::from(frame[1]);
        if !left.is_finite() || !right.is_finite() {
            return;
        }
        let peak = left.abs().max(right.abs());
        let coefficient = if peak > self.envelope {
            self.attack_alpha
        } else {
            self.release_alpha
        };
        self.envelope = coefficient * self.envelope + (1.0 - coefficient) * peak;
        let level_db = if self.envelope > 0.0 {
            20.0 * self.envelope.log10()
        } else {
            SILENCE_FLOOR_DB
        };
        let reduction_db = gain_reduction_db(
            level_db,
            f64::from(self.settings.threshold_db),
            f64::from(self.settings.ratio),
            f64::from(self.settings.knee_db),
        );
        let target_gain_db = reduction_db + f64::from(self.settings.makeup_db);
        self.gain_db = self.gain_alpha * self.gain_db + (1.0 - self.gain_alpha) * target_gain_db;
        let gain = 10.0_f64.powf(self.gain_db / 20.0);
        frame[0] = (left * gain) as f32;
        frame[1] = (right * gain) as f32;
    }
}

impl NativeProcessorRuntime for CompressorRuntime {
    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        for frame in frames {
            self.process_frame(frame);
        }
    }
    fn process_bypassed_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        for frame in frames {
            let input = *frame;
            self.process_frame(frame);
            *frame = input;
        }
    }
    fn reset(&mut self) {
        self.reset_history();
    }
    fn is_quiescent(&self) -> bool {
        true
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| self.sample_rate == previous.sample_rate)
    }
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        if let Some(previous) = previous
            .as_any()
            .downcast_ref::<Self>()
            .filter(|previous| self.sample_rate == previous.sample_rate)
        {
            self.envelope = previous.envelope;
            self.gain_db = previous.gain_db;
        }
    }
    fn can_retarget_parameters_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| self.sample_rate == previous.sample_rate)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn gain_reduction_db(level_db: f64, threshold_db: f64, ratio: f64, knee_db: f64) -> f64 {
    let over_threshold = level_db - threshold_db;
    let ratio_slope = 1.0 / ratio - 1.0;
    if knee_db <= 0.0 {
        return if over_threshold > 0.0 {
            ratio_slope * over_threshold
        } else {
            0.0
        };
    }
    let knee_start = -knee_db * 0.5;
    let knee_end = knee_db * 0.5;
    if over_threshold <= knee_start {
        0.0
    } else if over_threshold >= knee_end {
        ratio_slope * over_threshold
    } else {
        ratio_slope * (over_threshold - knee_start).powi(2) / (2.0 * knee_db)
    }
}

/// Bounded control-side static knee curve, independent of detector timing.
#[derive(Clone, Debug, PartialEq)]
pub struct CompressorTransferCurve {
    pub points: [[f32; 2]; 129],
}

/// Steady detector input is -60..0 dB; output is normalized to -66..24 dB.
/// Uses the same gain-reduction law as the runtime, including manual makeup.
pub fn transfer_curve(
    state: &NativeDeviceState,
) -> Result<CompressorTransferCurve, DeviceContractError> {
    transfer_curve_with_overrides(state, &[])
}

/// Borrowed current state plus at most four effective pending UI values.
/// The shared canonical patch law updates only Copy settings; no state owner is born.
pub fn transfer_curve_with_overrides(
    state: &NativeDeviceState,
    overrides: &[(ParameterId, ParameterValue)],
) -> Result<CompressorTransferCurve, DeviceContractError> {
    if overrides.len() > 4 {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    let mut settings = decode(state)?;
    for (index, &(id, value)) in overrides.iter().enumerate() {
        if overrides[..index].iter().any(|(prior, _)| *prior == id) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        patch_settings(&mut settings, id, value)?;
    }
    let points = std::array::from_fn(|index| {
        let x = index as f64 / 128.0;
        let input_db = -60.0 + x * 60.0;
        let output_db = input_db
            + gain_reduction_db(
                input_db,
                f64::from(settings.threshold_db),
                f64::from(settings.ratio),
                f64::from(settings.knee_db),
            )
            + f64::from(settings.makeup_db);
        [x as f32, ((output_db + 66.0) / 90.0) as f32]
    });
    Ok(CompressorTransferCurve { points })
}

#[cfg(test)]
mod tests;
