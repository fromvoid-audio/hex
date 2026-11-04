//! Original nonlinear stereo saturation for the native Lattice device chain.
//!
//! Drive, curve and output run inside the shared 8x oversampled ADAA stage
//! (`crate::oversampling`, Pulse #4146), so the soft and digital clips stay
//! below -60 db of aliasing at +36 db drive. The device declares the stage's
//! 11 frames of latency; the dry path is delayed by the same 11 frames, so the
//! zero-wet endpoint is the exact dry signal, delayed. Bypass advances the
//! histories and passes the input through undelayed (the host counts bypass as
//! zero latency). Non-finite input samples pass through to the host's audio
//! fault boundary without entering the histories.

use crate::oversampling::{Kernel, LATENCY_FRAMES, Oversampler, Shaper};
use hex_lattice::{
    NativeDeviceDefinition, NativeProcessorRuntime, OutputTail, PreparedNativeProcessor,
    StereoFrame, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;
mod automation;

pub const SATURATOR_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.saturator");
pub const SATURATOR_STATE_VERSION: u32 = 1;
const STATE_BYTES: usize = 13;

const CURVE_LABELS: &[&str] = &["soft clip", "digital clip"];

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const DRIVE: ParameterId = ParameterId::from_static(1);
    pub const OUTPUT: ParameterId = ParameterId::from_static(2);
    pub const DRY_WET: ParameterId = ParameterId::from_static(3);
    pub const CURVE: ParameterId = ParameterId::from_static(4);
}

const BINDINGS: [crate::parameter_binding::Binding<SaturatorSettings>; 4] = [
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::DRIVE,
            label: "drive",
            kind: ParameterKind::Float {
                min: -36.0,
                max: 36.0,
                unit: Some("db"),
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::SCALAR_ENVELOPE,
        |c| c.drive_db,
        |c, v| c.drive_db = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::OUTPUT,
            label: "output",
            kind: ParameterKind::Float {
                min: -36.0,
                max: 6.0,
                unit: Some("db"),
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::SCALAR_ENVELOPE,
        |c| c.output_db,
        |c, v| c.output_db = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::DRY_WET,
            label: "dry/wet",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(1.0),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::SCALAR_ENVELOPE,
        |c| c.dry_wet,
        |c, v| c.dry_wet = v,
    ),
    crate::parameter_binding::Binding::choice(
        ParameterDescriptor {
            id: parameter::CURVE,
            label: "curve",
            kind: ParameterKind::Choice {
                labels: CURVE_LABELS,
            },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::CHOICE_ENVELOPE,
        |c| match c.curve {
            SaturationCurve::SoftClip => 0,
            SaturationCurve::DigitalClip => 1,
        },
        |c, v| {
            c.curve = match v {
                0 => SaturationCurve::SoftClip,
                1 => SaturationCurve::DigitalClip,
                _ => return Err(DeviceContractError::InvalidParameterValue),
            };
            Ok(())
        },
    ),
];
const DESCRIPTORS: &[ParameterDescriptor] =
    &crate::parameter_binding::descriptors(&BINDINGS, BINDINGS[0].descriptor);

#[derive(Clone, Copy, Debug, PartialEq)]
struct SaturatorSettings {
    drive_db: f32,
    output_db: f32,
    dry_wet: f32,
    curve: SaturationCurve,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaturationCurve {
    SoftClip,
    DigitalClip,
}

impl Default for SaturatorSettings {
    fn default() -> Self {
        Self {
            drive_db: 0.0,
            output_db: 0.0,
            dry_wet: 1.0,
            curve: SaturationCurve::SoftClip,
        }
    }
}

fn decode(state: &NativeDeviceState) -> Result<SaturatorSettings, DeviceContractError> {
    if state.type_id() != SATURATOR_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != SATURATOR_STATE_VERSION {
        return Err(DeviceContractError::UnsupportedStateVersion);
    }
    let payload = state.payload();
    if payload.len() != STATE_BYTES || payload[12] > 1 {
        return Err(DeviceContractError::MalformedState);
    }
    let read = |offset| {
        f32::from_le_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ])
    };
    let settings = SaturatorSettings {
        drive_db: read(0),
        output_db: read(4),
        dry_wet: read(8),
        curve: match payload[12] {
            0 => SaturationCurve::SoftClip,
            1 => SaturationCurve::DigitalClip,
            _ => return Err(DeviceContractError::MalformedState),
        },
    };
    validate_settings(settings)?;
    Ok(settings)
}

fn validate_settings(settings: SaturatorSettings) -> Result<(), DeviceContractError> {
    for binding in &BINDINGS {
        binding.descriptor.validate(binding.read(&settings))?;
    }
    Ok(())
}

fn encode(settings: SaturatorSettings) -> Result<NativeDeviceState, DeviceContractError> {
    validate_settings(settings)?;
    let mut payload = Vec::with_capacity(STATE_BYTES);
    payload.extend_from_slice(&settings.drive_db.to_le_bytes());
    payload.extend_from_slice(&settings.output_db.to_le_bytes());
    payload.extend_from_slice(&settings.dry_wet.to_le_bytes());
    payload.push(match settings.curve {
        SaturationCurve::SoftClip => 0,
        SaturationCurve::DigitalClip => 1,
    });
    NativeDeviceState::new(SATURATOR_TYPE_ID, SATURATOR_STATE_VERSION, payload)
}

/// Built-in Saturator metadata, portable state codec and control-side processor.
pub struct SaturatorDefinition;

pub static SATURATOR_DEFINITION: SaturatorDefinition = SaturatorDefinition;

impl NativeDeviceMetadata for SaturatorDefinition {
    fn display_name(&self) -> Option<&'static str> {
        Some("saturator")
    }

    fn type_id(&self) -> DeviceTypeId {
        SATURATOR_TYPE_ID
    }

    fn state_version(&self) -> u32 {
        SATURATOR_STATE_VERSION
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        DESCRIPTORS
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        encode(SaturatorSettings::default())
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
        Ok(crate::parameter_binding::binding(&BINDINGS, id)?.read(&decode(state)?))
    }

    fn parameter_semantics_for_state(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<Option<hex_lattice_contract::ParameterSemantics>, DeviceContractError> {
        decode(state)?;
        Ok(Some(
            crate::parameter_binding::binding(&BINDINGS, id)?.semantics,
        ))
    }

    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut settings = decode(state)?;
        crate::parameter_binding::binding(&BINDINGS, id)?.write(&mut settings, value)?;
        encode(settings)
    }
}

// The current concrete direct-control path implements only these fixed
// IDs, with value-only/no-smoothing updates. Other families may explicitly
// implement structural/smoothed delivery without changing Binding metadata.
fn runtime_parameter_automation_supported(parameter: ParameterId) -> bool {
    let implemented = parameter
        .value()
        .checked_sub(1)
        .is_some_and(|index| index < 4);
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

impl NativeDeviceDefinition for SaturatorDefinition {
    fn supports_parameter_automation(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<bool, DeviceContractError> {
        decode(state)?;
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
                std::sync::Arc::new(Self::prepare_owned(state, context.sample_rate, parameters)?),
            )),
        })
    }

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
        Ok(Some(Box::new(Self::prepare_owned(
            state,
            context.sample_rate,
            &[],
        )?)))
    }
}

fn amplitude_from_db(db: f32) -> Result<f64, DeviceContractError> {
    hex_dsp::Decibels::new(f64::from(db))
        .and_then(hex_dsp::Decibels::linear_amplitude)
        .map(hex_dsp::LinearAmplitude::value)
        .map_err(|_| DeviceContractError::InvalidParameterValue)
}

/// Closed control-prepared Saturator factory. All fields are fixed inline;
/// canonical envelope backing stays in the host's existing program owner.
#[derive(Clone, Copy, Debug)]
pub struct PreparedSaturator {
    automation_mask: u8,
    settings: SaturatorSettings,
    sample_rate: u32,
    drive_gain: f64,
    output_gain: f64,
}

impl PreparedNativeProcessor for PreparedSaturator {
    fn parameter_automation_descriptor(
        &self,
        parameter: ParameterId,
    ) -> Option<ParameterDescriptor> {
        self.supports(parameter)
            .then(|| DESCRIPTORS.iter().find(|d| d.id == parameter).copied())
            .flatten()
    }
    fn supports_parameter_automation(&self, parameter: ParameterId) -> bool {
        self.supports(parameter)
    }

    fn runtime_version(&self) -> u32 {
        1
    }

    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<SaturatorRuntime>()
    }

    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        self.check_inline_ownership();
        let runtime = SaturatorRuntime {
            base: *self,
            controls: crate::parameter_binding::Controls::new(
                self.settings,
                std::array::from_fn(|index| self.automation_mask & (1u8 << index) != 0),
            ),
            settings: self.settings,
            sample_rate: self.sample_rate,
            drive_gain: self.drive_gain,
            output_gain: self.output_gain,
            kernel: Kernel::new(),
            channels: [Oversampler::default(); 2],
            delay: [[0.0; 2]; LATENCY_FRAMES],
            delay_index: 0,
            gain_ramp_remaining: 0,
        };
        runtime.check_inline_ownership();
        Box::new(runtime)
    }

    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(LATENCY_FRAMES as u32)
    }

    fn output_tail(&self) -> OutputTail {
        // Oversampler histories and the dry delay drain within this bound.
        OutputTail::Finite {
            max_tail_frames: TAIL_FRAMES,
        }
    }
}

/// Zero input frames after which every history is exactly zero and output is
/// silent: the oversampler drain, which outlasts the 11-frame dry delay.
const TAIL_FRAMES: u64 = crate::oversampling::DRAIN_FRAMES as u64;
const _: () = assert!(crate::oversampling::DRAIN_FRAMES >= LATENCY_FRAMES);

/// Closed fixed-inline DSP owner, including the last effective controls used by
/// transactional history copying. It owns no program, asset or external handle.
///
/// A direct drive or output edit arrives as a new runtime. Each unautomated gain
/// ramps from the previous effective value to its new target over 5 ms, even if
/// another control is automated. Automated gains keep their per-frame law. The
/// ramp is independent of sample rate and block partition.
/// Curve changes switch immediately: the two clip laws are not crossfaded.
#[derive(Clone, Copy, Debug)]
pub struct SaturatorRuntime {
    base: PreparedSaturator,
    controls: crate::parameter_binding::Controls<SaturatorSettings, 4>,
    settings: SaturatorSettings,
    sample_rate: u32,
    drive_gain: f64,
    output_gain: f64,
    kernel: Kernel,
    channels: [Oversampler; 2],
    /// Dry input, delayed by the oversampler latency.
    delay: [[f64; 2]; LATENCY_FRAMES],
    delay_index: usize,
    gain_ramp_remaining: u32,
}

/// Direct-edit gain ramp frames: 5 ms at every rate, like Compressor's dry/wet.
fn gain_ramp_frames(sample_rate: u32) -> u32 {
    sample_rate.div_ceil(200)
}

impl SaturatorRuntime {
    /// Renders one frame and advances every history. Bypassed frames keep
    /// their input; an active direct-edit ramp advances only when audible.
    fn render_frame(&mut self, frame: &mut StereoFrame, bypassed: bool) {
        if !bypassed {
            self.advance_gain_ramp();
        }
        let settings = self.settings;
        let wet = f64::from(settings.dry_wet);
        let input = frame.map(f64::from);
        let finite = input.map(f64::is_finite);
        let mut processed = [0.0; 2];
        for (channel, (stage, value)) in self.channels.iter_mut().zip(input).enumerate() {
            let value = if finite[channel] { value } else { 0.0 };
            let shaped = match settings.curve {
                SaturationCurve::SoftClip => {
                    stage.process(value, self.drive_gain, &self.kernel, &SoftClip)
                }
                SaturationCurve::DigitalClip => {
                    stage.process(value, self.drive_gain, &self.kernel, &DigitalClip)
                }
            };
            processed[channel] = shaped * self.output_gain;
        }
        let stored = [0, 1].map(|channel| if finite[channel] { input[channel] } else { 0.0 });
        let dry = std::mem::replace(&mut self.delay[self.delay_index], stored);
        self.delay_index = (self.delay_index + 1) % LATENCY_FRAMES;
        if bypassed {
            return;
        }
        for channel in 0..2 {
            if !finite[channel] {
                // Leave the fault visible to the host's audio boundary.
                continue;
            }
            frame[channel] = if wet == 0.0 {
                dry[channel]
            } else if wet == 1.0 {
                processed[channel]
            } else {
                dry[channel] * (1.0 - wet) + processed[channel] * wet
            } as f32;
        }
    }

    fn clear_history(&mut self) {
        self.channels = [Oversampler::default(); 2];
        self.delay = [[0.0; 2]; LATENCY_FRAMES];
        self.delay_index = 0;
    }

    /// Advance the direct-edit ramp by one frame; it ends on exact targets.
    #[inline]
    fn advance_gain_ramp(&mut self) {
        if self.gain_ramp_remaining == 0 {
            return;
        }
        let divisor = f64::from(self.gain_ramp_remaining);
        if !self.base.supports(parameter::DRIVE) {
            self.drive_gain += (self.base.drive_gain - self.drive_gain) / divisor;
        }
        if !self.base.supports(parameter::OUTPUT) {
            self.output_gain += (self.base.output_gain - self.output_gain) / divisor;
        }
        self.gain_ramp_remaining -= 1;
        if self.gain_ramp_remaining == 0 {
            if !self.base.supports(parameter::DRIVE) {
                self.drive_gain = self.base.drive_gain;
            }
            if !self.base.supports(parameter::OUTPUT) {
                self.output_gain = self.base.output_gain;
            }
        }
    }

    /// Bypass is already a discontinuity; resume from the exact targets.
    fn finish_gain_ramp(&mut self) {
        if self.gain_ramp_remaining != 0 {
            self.gain_ramp_remaining = 0;
            if !self.base.supports(parameter::DRIVE) {
                self.drive_gain = self.base.drive_gain;
            }
            if !self.base.supports(parameter::OUTPUT) {
                self.output_gain = self.base.output_gain;
            }
        }
    }
}

impl NativeProcessorRuntime for SaturatorRuntime {
    fn supports_parameter_automation(&self, parameter: ParameterId) -> bool {
        self.base.supports(parameter)
    }
    fn process_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        _clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        values: &dyn hex_lattice::NativeParameterAutomation,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), DeviceContractError> {
        let mut resolved = self.controls.resolve(&BINDINGS, values)?;
        for (offset, frame) in frames.iter_mut().enumerate() {
            self.apply_automation(values, &mut resolved, offset)?;
            self.render_frame(frame, false);
        }
        Ok(())
    }
    fn process_bypassed_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        _clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        values: &dyn hex_lattice::NativeParameterAutomation,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), DeviceContractError> {
        self.finish_gain_ramp();
        let mut resolved = self.controls.resolve(&BINDINGS, values)?;
        for (offset, frame) in frames.iter_mut().enumerate() {
            self.apply_automation(values, &mut resolved, offset)?;
            self.render_frame(frame, true);
        }
        Ok(())
    }

    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        for frame in frames {
            self.render_frame(frame, false);
        }
    }

    fn process_bypassed_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        self.finish_gain_ramp();
        for frame in frames {
            self.render_frame(frame, true);
        }
    }

    fn reset(&mut self) {
        self.controls.reset();
        self.settings = self.base.settings;
        self.drive_gain = self.base.drive_gain;
        self.output_gain = self.base.output_gain;
        self.clear_history();
        self.gain_ramp_remaining = 0;
    }

    fn is_quiescent(&self) -> bool {
        self.channels.iter().all(Oversampler::is_quiescent)
            && self.delay.iter().flatten().all(|v| *v == 0.0)
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
        self.channels = previous.channels;
        self.delay = previous.delay;
        self.delay_index = previous.delay_index;
        if self.base.automation_mask != 0 && previous.base.automation_mask != 0 {
            self.controls.inherit(&previous.controls);
            self.settings = previous.settings;
            self.drive_gain = previous.drive_gain;
            self.output_gain = previous.output_gain;
        }
        let direct_drive = !self.base.supports(parameter::DRIVE);
        let direct_output = !self.base.supports(parameter::OUTPUT);
        if direct_drive {
            self.drive_gain = previous.drive_gain;
        }
        if direct_output {
            self.output_gain = previous.output_gain;
        }
        let needs_ramp = (direct_drive && self.drive_gain != self.base.drive_gain)
            || (direct_output && self.output_gain != self.base.output_gain);
        self.gain_ramp_remaining = if !needs_ramp {
            0
        } else if (!direct_drive
            || (!previous.base.supports(parameter::DRIVE)
                && previous.base.drive_gain == self.base.drive_gain))
            && (!direct_output
                || (!previous.base.supports(parameter::OUTPUT)
                    && previous.base.output_gain == self.base.output_gain))
            && previous.gain_ramp_remaining != 0
        {
            // Scratch/rollback twins copy the trajectory without restarting it.
            previous.gain_ramp_remaining
        } else {
            gain_ramp_frames(self.sample_rate)
        };
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

/// The soft-clip law and its continuous antiderivative: x^2 / 2 inside
/// |x| <= 0.5, |x| - ln|x| / 4 + C outside, with C joining both at 0.5.
struct SoftClip;
impl Shaper for SoftClip {
    fn tag(&self) -> u8 {
        1
    }
    fn value(&self, x: f64) -> f64 {
        soft_clip(x)
    }
    fn antiderivative(&self, x: f64) -> f64 {
        let magnitude = x.abs();
        if magnitude <= 0.5 {
            0.5 * x * x
        } else {
            magnitude - 0.25 * magnitude.ln() - 0.375 - 0.25 * std::f64::consts::LN_2
        }
    }
}
/// The digital clip and its antiderivative: x^2 / 2 inside |x| <= 1, |x| - 1/2
/// outside.
struct DigitalClip;
impl Shaper for DigitalClip {
    fn tag(&self) -> u8 {
        2
    }
    fn value(&self, x: f64) -> f64 {
        x.clamp(-1.0, 1.0)
    }
    fn antiderivative(&self, x: f64) -> f64 {
        if x.abs() <= 1.0 {
            0.5 * x * x
        } else {
            x.abs() - 0.5
        }
    }
}

fn soft_clip(input: f64) -> f64 {
    let magnitude = input.abs();
    if magnitude <= 0.5 {
        input
    } else {
        input.signum() * (1.0 - 0.25 / magnitude)
    }
}

#[inline]
/// The static transfer: the value a held finite input settles to once the
/// oversampled stage and dry delay are full (`transfer_curve` previews it).
fn process_sample(
    sample: f64,
    settings: SaturatorSettings,
    drive: f64,
    output: f64,
    wet: f64,
) -> f64 {
    if !sample.is_finite() {
        return sample;
    }
    let driven = sample * drive;
    let clipped = match settings.curve {
        SaturationCurve::SoftClip => soft_clip(driven),
        SaturationCurve::DigitalClip => driven.clamp(-1.0, 1.0),
    };
    let processed = clipped * output;
    if wet == 1.0 {
        processed
    } else {
        sample * (1.0 - wet) + processed * wet
    }
}

/// Bounded control-side view of the actual sample-processing law.
/// Coordinates are normalized; the horizontal input range is ±1 and the
/// vertical output range is ±`output_peak`. No audio history is inspected.
#[derive(Clone, Debug, PartialEq)]
pub struct SaturatorTransferCurve {
    pub points: [[f32; 2]; 129],
    pub output_peak: f32,
}

/// Sample the current drive, curve, output and mix through the same law as DSP.
/// The output axis expands for positive output gain rather than hiding peaks.
pub fn transfer_curve(
    state: &NativeDeviceState,
) -> Result<SaturatorTransferCurve, DeviceContractError> {
    let settings = decode(state)?;
    let drive = amplitude_from_db(settings.drive_db)?;
    let output = amplitude_from_db(settings.output_db)?;
    let peak = output.max(1.0);
    let wet = f64::from(settings.dry_wet);
    let points = std::array::from_fn(|index| {
        let x = index as f64 / 128.0;
        let input = x * 2.0 - 1.0;
        let value = process_sample(input, settings, drive, output, wet);
        [x as f32, ((value / peak + 1.0) * 0.5) as f32]
    });
    Ok(SaturatorTransferCurve {
        points,
        output_peak: peak as f32,
    })
}

#[cfg(test)]
mod tests;
