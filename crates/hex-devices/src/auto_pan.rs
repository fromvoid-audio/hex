//! Original free-running stereo Auto Pan with bounded parameter transitions.

use hex_lattice::{
    NativeDeviceDefinition, NativeProcessorRuntime, OutputTail, PreparedNativeProcessor,
    StereoFrame, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;

pub const AUTO_PAN_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.auto-pan");
pub const AUTO_PAN_STATE_VERSION: u32 = 2;
const STATE_BYTES: usize = 16;
const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;
const DEFAULT_RAMP_MS: f64 = 5.0;

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const AMOUNT: ParameterId = ParameterId::from_static(1);
    pub const RATE: ParameterId = ParameterId::from_static(2);
    pub const RIGHT_PHASE: ParameterId = ParameterId::from_static(3);
    pub const WAVEFORM: ParameterId = ParameterId::from_static(4);
}

const DESCRIPTORS: &[ParameterDescriptor] = &[
    ParameterDescriptor {
        id: parameter::AMOUNT,
        label: "amount",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(0.5),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::RATE,
        label: "rate",
        kind: ParameterKind::Float {
            min: 0.05,
            max: 20.0,
            unit: Some("hz"),
        },
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: parameter::RIGHT_PHASE,
        label: "right phase",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 360.0,
            unit: Some("deg"),
        },
        default: ParameterValue::Float(180.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::WAVEFORM,
        label: "waveform",
        kind: ParameterKind::Choice {
            labels: &["sine", "triangle", "saw up", "saw down", "square"],
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
enum Waveform {
    Sine,
    Triangle,
    SawUp,
    SawDown,
    Square,
}
impl Waveform {
    const ALL: [Self; 5] = [
        Self::Sine,
        Self::Triangle,
        Self::SawUp,
        Self::SawDown,
        Self::Square,
    ];
    fn from_choice(value: u32) -> Result<Self, DeviceContractError> {
        Self::ALL
            .get(value as usize)
            .copied()
            .ok_or(DeviceContractError::InvalidParameterValue)
    }
    fn lfo(self, phase: f64) -> f64 {
        match self {
            Self::Sine => (1.0 + (std::f64::consts::TAU * phase).sin()) * 0.5,
            Self::Triangle => 1.0 - (2.0 * (phase + 0.25).rem_euclid(1.0) - 1.0).abs(),
            Self::SawUp => phase,
            Self::SawDown => 1.0 - phase,
            // Original symmetric square: high on [0,1/2), low on [1/2,1).
            Self::Square => {
                if phase < 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AutoPanSettings {
    amount: f32,
    rate_hz: f32,
    right_phase_degrees: f32,
    waveform: Waveform,
}

impl Default for AutoPanSettings {
    fn default() -> Self {
        Self {
            amount: 0.5,
            rate_hz: 1.0,
            right_phase_degrees: 180.0,
            waveform: Waveform::Sine,
        }
    }
}

fn decode(state: &NativeDeviceState) -> Result<AutoPanSettings, DeviceContractError> {
    if state.type_id() != AUTO_PAN_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != AUTO_PAN_STATE_VERSION {
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
    let settings = AutoPanSettings {
        amount: read(0),
        rate_hz: read(4),
        right_phase_degrees: read(8),
        waveform: Waveform::from_choice(u32::from_le_bytes(
            bytes[12..16]
                .try_into()
                .map_err(|_| DeviceContractError::MalformedState)?,
        ))?,
    };
    validate(settings)?;
    Ok(settings)
}

fn validate(settings: AutoPanSettings) -> Result<(), DeviceContractError> {
    for (id, value) in [
        (parameter::AMOUNT, settings.amount),
        (parameter::RATE, settings.rate_hz),
        (parameter::RIGHT_PHASE, settings.right_phase_degrees),
    ] {
        let descriptor = DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.id == id)
            .ok_or(DeviceContractError::UnknownParameter)?;
        descriptor.validate(ParameterValue::Float(value))?;
    }
    Ok(())
}

fn encode(settings: AutoPanSettings) -> Result<NativeDeviceState, DeviceContractError> {
    validate(settings)?;
    let mut payload = Vec::with_capacity(STATE_BYTES);
    for value in [
        settings.amount,
        settings.rate_hz,
        settings.right_phase_degrees,
    ] {
        payload.extend_from_slice(&value.to_le_bytes());
    }
    payload.extend_from_slice(&(settings.waveform as u32).to_le_bytes());
    NativeDeviceState::new(AUTO_PAN_TYPE_ID, AUTO_PAN_STATE_VERSION, payload)
}

/// Registered Lattice Auto Pan metadata, codec, and control-side preparation.
pub struct AutoPanDefinition;
pub static AUTO_PAN_DEFINITION: AutoPanDefinition = AutoPanDefinition;

impl NativeDeviceMetadata for AutoPanDefinition {
    fn type_id(&self) -> DeviceTypeId {
        AUTO_PAN_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("auto pan")
    }
    fn state_version(&self) -> u32 {
        AUTO_PAN_STATE_VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        DESCRIPTORS
    }
    fn parameter_semantics_for_state(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<Option<hex_lattice_contract::ParameterSemantics>, DeviceContractError> {
        use hex_lattice_contract::{
            ParameterAccess as A, ParameterAutomation as E, ParameterChangeImpact as I,
            ParameterModulation as M, ParameterNormalization as N, ParameterSemantics,
            ParameterSmoothing as S, ParameterStepping as T,
        };
        decode(state)?;
        let normalization = match id {
            parameter::WAVEFORM => N::Discrete,
            parameter::AMOUNT | parameter::RIGHT_PHASE => N::Linear,
            parameter::RATE => N::PositiveLogarithmic,
            _ => return Err(DeviceContractError::UnknownParameter),
        };
        Ok(Some(ParameterSemantics {
            access: A::ReadWrite,
            normalization,
            stepping: if id == parameter::WAVEFORM {
                T::Choice
            } else {
                T::Continuous
            },
            automation: E::Unsupported,
            modulation: M::Unsupported,
            smoothing: S::DeviceManaged,
            change_impact: I::ValueOnly,
        }))
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        encode(AutoPanSettings::default())
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
        if id == parameter::WAVEFORM {
            return Ok(ParameterValue::Choice(settings.waveform as u32));
        }
        let value = match id {
            parameter::AMOUNT => settings.amount,
            parameter::RATE => settings.rate_hz,
            parameter::RIGHT_PHASE => settings.right_phase_degrees,
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
        let descriptor = DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.id == id)
            .ok_or(DeviceContractError::UnknownParameter)?;
        descriptor.validate(value)?;
        if id == parameter::WAVEFORM {
            let ParameterValue::Choice(choice) = value else {
                return Err(DeviceContractError::InvalidParameterValue);
            };
            settings.waveform = Waveform::from_choice(choice)?;
            return encode(settings);
        }
        let ParameterValue::Float(value) = value else {
            return Err(DeviceContractError::InvalidParameterValue);
        };
        match id {
            parameter::AMOUNT => settings.amount = value,
            parameter::RATE => settings.rate_hz = value,
            parameter::RIGHT_PHASE => settings.right_phase_degrees = value,
            _ => return Err(DeviceContractError::UnknownParameter),
        }
        encode(settings)
    }
}

impl NativeDeviceDefinition for AutoPanDefinition {
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
        let ramp_frames =
            ((f64::from(sample_rate) * DEFAULT_RAMP_MS / 1_000.0).ceil() as u32).max(1);
        Ok(Some(Box::new(PreparedAutoPan {
            settings,
            sample_rate,
            ramp_frames,
        })))
    }
}

struct PreparedAutoPan {
    settings: AutoPanSettings,
    sample_rate: u32,
    ramp_frames: u32,
}

impl PreparedNativeProcessor for PreparedAutoPan {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<AutoPanRuntime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(AutoPanRuntime {
            sample_rate: self.sample_rate,
            ramp_frames: self.ramp_frames,
            phase_cycles: 0.0,
            amount: ScalarRamp::steady(f64::from(self.settings.amount)),
            rate_hz: ScalarRamp::steady(f64::from(self.settings.rate_hz)),
            right_phase_degrees: ScalarRamp::steady(normalize_degrees(f64::from(
                self.settings.right_phase_degrees,
            ))),
            waveform_weights: std::array::from_fn(|i| {
                ScalarRamp::steady(f64::from(i == self.settings.waveform as usize))
            }),
            targets: AutoPanTargets::from(self.settings),
        })
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite { max_tail_frames: 0 }
    }
}

#[derive(Clone, Copy)]
struct AutoPanTargets {
    amount: f64,
    rate_hz: f64,
    right_phase_degrees: f64,
    waveform: Waveform,
}

impl From<AutoPanSettings> for AutoPanTargets {
    fn from(settings: AutoPanSettings) -> Self {
        Self {
            amount: f64::from(settings.amount),
            rate_hz: f64::from(settings.rate_hz),
            right_phase_degrees: normalize_degrees(f64::from(settings.right_phase_degrees)),
            waveform: settings.waveform,
        }
    }
}

#[derive(Clone, Copy)]
struct ScalarRamp {
    current: f64,
    target: f64,
    step: f64,
    remaining: u32,
}

impl ScalarRamp {
    const fn steady(value: f64) -> Self {
        Self {
            current: value,
            target: value,
            step: 0.0,
            remaining: 0,
        }
    }

    fn retarget(&mut self, target: f64, frames: u32) {
        self.target = target;
        if self.current == target || frames == 0 {
            self.current = target;
            self.step = 0.0;
            self.remaining = 0;
        } else {
            self.step = (target - self.current) / f64::from(frames);
            self.remaining = frames;
        }
    }

    fn advance(&mut self) {
        if self.remaining == 0 {
            return;
        }
        if self.remaining == 1 {
            self.current = self.target;
            self.step = 0.0;
            self.remaining = 0;
        } else {
            self.current += self.step;
            self.remaining -= 1;
        }
    }
}

struct AutoPanRuntime {
    sample_rate: u32,
    ramp_frames: u32,
    phase_cycles: f64,
    amount: ScalarRamp,
    rate_hz: ScalarRamp,
    right_phase_degrees: ScalarRamp,
    waveform_weights: [ScalarRamp; 5],
    targets: AutoPanTargets,
}

impl AutoPanRuntime {
    fn reset_history(&mut self) {
        self.phase_cycles = 0.0;
        self.waveform_weights = std::array::from_fn(|i| {
            ScalarRamp::steady(f64::from(i == self.targets.waveform as usize))
        });
        self.amount = ScalarRamp::steady(self.targets.amount);
        self.rate_hz = ScalarRamp::steady(self.targets.rate_hz);
        self.right_phase_degrees = ScalarRamp::steady(self.targets.right_phase_degrees);
    }

    fn process_frame(&mut self, frame: &mut StereoFrame) {
        let left = f64::from(frame[0]);
        let right = f64::from(frame[1]);
        if !left.is_finite() || !right.is_finite() {
            return;
        }

        // Edits advance before this sample; phase is sampled before its clock advances.
        self.amount.advance();
        self.rate_hz.advance();
        self.right_phase_degrees.advance();
        for weight in &mut self.waveform_weights {
            weight.advance();
        }
        let amount = self.amount.current;
        if amount != 0.0 {
            let left_lfo = self.lfo(self.phase_cycles);
            let right_phase =
                (self.phase_cycles + self.right_phase_degrees.current / 360.0).rem_euclid(1.0);
            let right_lfo = self.lfo(right_phase);
            let base_gain = 1.0 - amount;
            frame[0] = (left * (base_gain + amount * left_lfo)) as f32;
            frame[1] = (right * (base_gain + amount * right_lfo)) as f32;
        }
        self.phase_cycles = (self.phase_cycles
            + self.rate_hz.current / f64::from(self.sample_rate))
        .rem_euclid(1.0);
    }

    fn lfo(&self, phase: f64) -> f64 {
        // Preserve the original Sine arithmetic and avoid blending when steady.
        if let Some(index) = self
            .waveform_weights
            .iter()
            .position(|weight| weight.current == 1.0)
        {
            return Waveform::ALL[index].lfo(phase);
        }
        self.waveform_weights
            .iter()
            .zip(Waveform::ALL)
            .filter(|(weight, _)| weight.current != 0.0)
            .map(|(weight, waveform)| weight.current * waveform.lfo(phase))
            .sum::<f64>()
            .clamp(0.0, 1.0)
    }

    fn copy_history_and_retarget(&mut self, previous: &Self) {
        self.waveform_weights = previous.waveform_weights;
        if self.targets.waveform != previous.targets.waveform {
            for (index, weight) in self.waveform_weights.iter_mut().enumerate() {
                weight.retarget(
                    f64::from(index == self.targets.waveform as usize),
                    self.ramp_frames,
                );
            }
        }
        self.phase_cycles = previous.phase_cycles;
        self.amount = previous.amount;
        self.rate_hz = previous.rate_hz;
        self.right_phase_degrees = previous.right_phase_degrees;

        if self.targets.amount != previous.targets.amount {
            self.amount.retarget(self.targets.amount, self.ramp_frames);
        }
        if self.targets.rate_hz != previous.targets.rate_hz {
            self.rate_hz
                .retarget(self.targets.rate_hz, self.ramp_frames);
        }
        if self.targets.right_phase_degrees != previous.targets.right_phase_degrees {
            let from = normalize_degrees(self.right_phase_degrees.current);
            let delta = shortest_degrees(from, self.targets.right_phase_degrees);
            self.right_phase_degrees
                .retarget(from + delta, self.ramp_frames);
        }
    }
}

fn normalize_degrees(degrees: f64) -> f64 {
    degrees.rem_euclid(360.0)
}

fn shortest_degrees(from: f64, to: f64) -> f64 {
    let mut delta = (to - from + 180.0).rem_euclid(360.0) - 180.0;
    if delta <= -180.0 {
        delta += 360.0;
    }
    delta
}

impl NativeProcessorRuntime for AutoPanRuntime {
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
            let original = *frame;
            self.process_frame(frame);
            *frame = original;
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
        let Some(previous) = previous
            .as_any()
            .downcast_ref::<Self>()
            .filter(|previous| self.sample_rate == previous.sample_rate)
        else {
            return;
        };
        self.copy_history_and_retarget(previous);
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

/// Bounded, sampled steady-target channel gains over one cycle. Not live phase.
#[derive(Clone, Debug, PartialEq)]
pub struct AutoPanWaveformPreview {
    pub left: [[f32; 2]; 129],
    pub right: [[f32; 2]; 129],
}

/// Uses the provider's exact waveform/amount/phase law, never prepares a runtime.
/// Rate changes the time represented by one cycle, not its normalized geometry.
pub fn waveform_preview(
    state: &NativeDeviceState,
) -> Result<AutoPanWaveformPreview, DeviceContractError> {
    let settings = decode(state)?;
    let channel = |offset: f64| {
        std::array::from_fn(|index| {
            let x = index as f64 / 128.0;
            let phase = (x + offset).rem_euclid(1.0);
            let gain = 1.0 - f64::from(settings.amount)
                + f64::from(settings.amount) * settings.waveform.lfo(phase);
            [x as f32, gain as f32]
        })
    };
    Ok(AutoPanWaveformPreview {
        left: channel(0.0),
        right: channel(f64::from(settings.right_phase_degrees) / 360.0),
    })
}

#[cfg(test)]
mod tests;
