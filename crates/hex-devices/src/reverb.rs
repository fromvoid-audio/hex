//! Original stereo Schroeder room: four parallel combs and two serial allpasses.
//!
//! Decay is nominal comb RT60 (0.1–2.5 seconds); fixed diffusion adds a short
//! onset/decay spread. Dry/wet is a linear amplitude crossfade. No freeze, modes,
//! modulation or reference-device parity is implied. Version 1 stores exactly
//! two little-endian f32 values: decay seconds, then wet percent.
//!
//! The tank runs at sample_rate / ceil(sample_rate / 24000). Box averaging on
//! input and linear interpolation on output deliberately limit wet bandwidth;
//! dry audio stays at host rate. Separate L/R delay lengths decorrelate mono
//! input without discarding stereo. No fixed host buffering latency is added;
//! acoustic delay is part of the effect. All allocations are control-side.
//!
//! History is copied only at the same rate. Authored edits ramp feedback and mix
//! for 20 ms, preserving in-flight ramps when targets are unchanged. Bypass uses
//! the host's dry passthrough/private advancement contract. Reset clears history
//! and installs targets. Host Stop/seek reset the tank; loop wrap continues it.
//! Removed track/return effects drain under zero input through the host tail
//! groups; final destruction remains control-side.
//! Zero input for eight seconds clears the bounded tank, even at maximum decay
//! (over three nominal RT60s). This deadline includes interpolation and partial
//! decimation input. Host finite-tail and aggregate memory budgets still apply.
use hex_lattice::{
    NativeDeviceDefinition, NativeProcessorRuntime, OutputTail, PreparedNativeProcessor,
    StereoFrame, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;

pub const REVERB_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.reverb");
pub const REVERB_STATE_VERSION: u32 = 1;
pub mod parameter {
    use hex_lattice_contract::ParameterId;
    pub const DECAY: ParameterId = ParameterId::from_static(1);
    pub const DRY_WET: ParameterId = ParameterId::from_static(2);
}
const DESCRIPTORS: &[ParameterDescriptor] = &[
    ParameterDescriptor {
        id: parameter::DECAY,
        label: "decay",
        kind: ParameterKind::Float {
            min: 0.1,
            max: 2.5,
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
        default: ParameterValue::Float(25.0),
        presentation: ParameterPresentation::Linear,
    },
];

fn decode(state: &NativeDeviceState) -> Result<[f32; 2], DeviceContractError> {
    if state.type_id() != REVERB_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != REVERB_STATE_VERSION {
        return Err(DeviceContractError::UnsupportedStateVersion);
    }
    if state.payload().len() != 8 {
        return Err(DeviceContractError::MalformedState);
    }
    let mut values = [0.0; 2];
    for (i, value) in values.iter_mut().enumerate() {
        *value = f32::from_le_bytes(
            state.payload()[4 * i..4 * i + 4]
                .try_into()
                .map_err(|_| DeviceContractError::MalformedState)?,
        );
        DESCRIPTORS[i].validate(ParameterValue::Float(*value))?;
    }
    Ok(values)
}
fn encode(values: [f32; 2]) -> Result<NativeDeviceState, DeviceContractError> {
    let mut bytes = Vec::with_capacity(8);
    for (descriptor, value) in DESCRIPTORS.iter().zip(values) {
        descriptor.validate(ParameterValue::Float(value))?;
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    NativeDeviceState::new(REVERB_TYPE_ID, REVERB_STATE_VERSION, bytes)
}
fn index(id: ParameterId) -> Result<usize, DeviceContractError> {
    DESCRIPTORS
        .iter()
        .position(|d| d.id == id)
        .ok_or(DeviceContractError::UnknownParameter)
}

pub struct ReverbDefinition;
pub static REVERB_DEFINITION: ReverbDefinition = ReverbDefinition;
impl NativeDeviceMetadata for ReverbDefinition {
    fn type_id(&self) -> DeviceTypeId {
        REVERB_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("reverb")
    }
    fn state_version(&self) -> u32 {
        REVERB_STATE_VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        encode([1.2, 25.0])
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
        Ok(ParameterValue::Float(decode(state)?[index(id)?]))
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut values = decode(state)?;
        let i = index(id)?;
        DESCRIPTORS[i].validate(value)?;
        let ParameterValue::Float(value) = value else {
            return Err(DeviceContractError::InvalidParameterValue);
        };
        values[i] = value;
        encode(values)
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
        index(id)?;
        Ok(Some(ParameterSemantics {
            access: A::ReadWrite,
            normalization: if id == parameter::DECAY {
                N::PositiveLogarithmic
            } else {
                N::Linear
            },
            stepping: T::Continuous,
            automation: E::Unsupported,
            modulation: M::Unsupported,
            smoothing: S::DeviceManaged,
            change_impact: I::ValueOnly,
        }))
    }
}
impl NativeDeviceDefinition for ReverbDefinition {
    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        decode(state)?;
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_effect_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        decode(state)?;
        Ok(StereoMatrix::identity())
    }
    fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        if !(8_000..=192_000).contains(&context.sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let values = decode(state)?;
        let factor = context.sample_rate.div_ceil(24_000);
        let rate = f64::from(context.sample_rate) / f64::from(factor);
        let lengths = std::array::from_fn(|channel| {
            std::array::from_fn(|i| {
                let seconds =
                    [0.0297, 0.0371, 0.0411, 0.0437, 0.005, 0.0017][i] + channel as f64 * 0.00073;
                (seconds * rate).round().max(1.0) as usize
            })
        });
        let feedback = std::array::from_fn(|channel| {
            std::array::from_fn(|i| {
                10.0_f64.powf(-3.0 * lengths[channel][i] as f64 / (rate * f64::from(values[0])))
                    as f32
            })
        });
        Ok(Some(Box::new(PreparedReverb {
            sample_rate: context.sample_rate,
            factor,
            lengths,
            feedback,
            wet: values[1] / 100.0,
        })))
    }
}
struct PreparedReverb {
    sample_rate: u32,
    factor: u32,
    lengths: [[usize; 6]; 2],
    feedback: [[f32; 4]; 2],
    wet: f32,
}
impl PreparedNativeProcessor for PreparedReverb {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<ReverbRuntime>()
            + self.lengths.iter().flatten().sum::<usize>() * std::mem::size_of::<f32>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(ReverbRuntime {
            sample_rate: self.sample_rate,
            factor: self.factor,
            lines: std::array::from_fn(|c| {
                std::array::from_fn(|i| Line {
                    samples: vec![0.0; self.lengths[c][i]].into_boxed_slice(),
                    cursor: 0,
                })
            }),
            feedback: self.feedback.map(|row| row.map(Ramp::steady)),
            wet: Ramp::steady(self.wet),
            phase: 0,
            sum: [0.0; 2],
            previous: [0.0; 2],
            next: [0.0; 2],
            silent_frames: 0,
            active: false,
        })
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite {
            max_tail_frames: u64::from(self.sample_rate) * 8,
        }
    }
}
struct Line {
    samples: Box<[f32]>,
    cursor: usize,
}
impl Line {
    fn tick(&mut self, input: f32, feedback: f32, allpass: bool) -> f32 {
        let delayed = self.samples[self.cursor];
        let output = if allpass {
            delayed - input * feedback
        } else {
            delayed
        };
        self.samples[self.cursor] = if allpass {
            input + output * feedback
        } else {
            input + delayed * feedback
        };
        self.cursor += 1;
        if self.cursor == self.samples.len() {
            self.cursor = 0;
        }
        output
    }
    fn clear(&mut self) {
        self.samples.fill(0.0);
        self.cursor = 0;
    }
    fn copy(&mut self, source: &Self) {
        self.samples.copy_from_slice(&source.samples);
        self.cursor = source.cursor;
    }
}
#[derive(Clone, Copy)]
struct Ramp {
    value: f32,
    target: f32,
    step: f32,
    remaining: u32,
}
impl Ramp {
    fn steady(value: f32) -> Self {
        Self {
            value,
            target: value,
            step: 0.0,
            remaining: 0,
        }
    }
    fn retarget(&mut self, target: f32, frames: u32) {
        self.target = target;
        self.step = (target - self.value) / frames as f32;
        self.remaining = frames;
    }
    fn advance(&mut self) -> f32 {
        if self.remaining > 0 {
            self.remaining -= 1;
            self.value = if self.remaining == 0 {
                self.target
            } else {
                self.value + self.step
            };
        }
        self.value
    }
}
struct ReverbRuntime {
    sample_rate: u32,
    factor: u32,
    lines: [[Line; 6]; 2],
    feedback: [[Ramp; 4]; 2],
    wet: Ramp,
    phase: u32,
    sum: StereoFrame,
    previous: StereoFrame,
    next: StereoFrame,
    silent_frames: u64,
    active: bool,
}
impl ReverbRuntime {
    fn clear(&mut self) {
        for line in self.lines.iter_mut().flatten() {
            line.clear();
        }
        self.phase = 0;
        self.sum = [0.0; 2];
        self.previous = [0.0; 2];
        self.next = [0.0; 2];
        self.silent_frames = 0;
        self.active = false;
    }
    fn frame(&mut self, frame: &mut StereoFrame) {
        // Nonfinite audio is silence; extreme finite input is bounded before feedback.
        let input = frame.map(|v| {
            if v.is_finite() {
                v.clamp(-16.0, 16.0)
            } else {
                0.0
            }
        });
        let mix = self.wet.advance();
        if input != [0.0; 2] {
            self.silent_frames = 0;
            self.active = true;
        } else if self.active {
            self.silent_frames += 1;
            if self.silent_frames >= u64::from(self.sample_rate) * 8 {
                self.clear();
            }
        }
        for gain in self.feedback.iter_mut().flatten() {
            gain.advance();
        }
        if self.active {
            for (sum, value) in self.sum.iter_mut().zip(input) {
                *sum += value;
            }
            self.phase += 1;
            if self.phase == self.factor {
                self.phase = 0;
                self.previous = self.next;
                for c in 0..2 {
                    let excitation = self.sum[c] / self.factor as f32;
                    let mut wet = 0.0;
                    for i in 0..4 {
                        wet += self.lines[c][i].tick(excitation, self.feedback[c][i].value, false)
                            * 0.25;
                    }
                    for i in 4..6 {
                        wet = self.lines[c][i].tick(wet, 0.5, true);
                    }
                    self.next[c] = wet;
                }
                self.sum = [0.0; 2];
            }
        }
        let fraction = (self.phase + 1) as f32 / self.factor as f32;
        for c in 0..2 {
            let wet = self.previous[c] + fraction * (self.next[c] - self.previous[c]);
            frame[c] = ((1.0 - mix) * input[c] + mix * wet).clamp(-16.0, 16.0);
        }
    }
}
impl NativeProcessorRuntime for ReverbRuntime {
    fn process_block(&mut self, frames: &mut [StereoFrame], _: &dyn hex_lattice::DeviceResources) {
        for frame in frames {
            self.frame(frame);
        }
    }
    fn reset(&mut self) {
        self.clear();
        self.wet = Ramp::steady(self.wet.target);
        for gain in self.feedback.iter_mut().flatten() {
            *gain = Ramp::steady(gain.target);
        }
    }
    fn is_quiescent(&self) -> bool {
        !self.active
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|p| p.sample_rate == self.sample_rate)
    }
    fn can_retarget_parameters_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        self.can_copy_history_from(previous)
    }
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        let Some(p) = previous
            .as_any()
            .downcast_ref::<Self>()
            .filter(|p| p.sample_rate == self.sample_rate)
        else {
            return;
        };
        for (line, source) in self
            .lines
            .iter_mut()
            .flatten()
            .zip(p.lines.iter().flatten())
        {
            line.copy(source);
        }
        let frames = (self.sample_rate / 50).max(1);
        for (gain, source) in self
            .feedback
            .iter_mut()
            .flatten()
            .zip(p.feedback.iter().flatten())
        {
            let target = gain.target;
            *gain = *source;
            if target != source.target {
                gain.retarget(target, frames);
            }
        }
        let target = self.wet.target;
        self.wet = p.wet;
        if target != p.wet.target {
            self.wet.retarget(target, frames);
        }
        self.phase = p.phase;
        self.sum = p.sum;
        self.previous = p.previous;
        self.next = p.next;
        self.silent_frames = p.silent_frames;
        self.active = p.active;
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
#[cfg(test)]
mod tests;
