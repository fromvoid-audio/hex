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

use crate::oversampling::{BLOCK_CHUNK, Kernel, LATENCY_FRAMES, Oversampler, Shaper};
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
        crate::parameter_binding::parameter_semantics_with_sample_rate_modulation(
            &BINDINGS,
            state,
            id,
            decode,
            runtime_parameter_automation_supported,
        )
    }

    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        crate::parameter_binding::patch_parameter(&BINDINGS, state, id, value, decode, encode)
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
        Ok(crate::inline_owned::prepared_owned_runtime(
            Self::prepare_owned(state, context.sample_rate, parameters)?,
        ))
    }
    fn prepare_runtime_factory(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<hex_lattice::PreparedProcessorFactory>, DeviceContractError> {
        Ok(Some(crate::inline_owned::owned_factory(
            Self::prepare_owned(state, context.sample_rate, &[])?,
        )))
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
    fn supports_latency_aligned_bypass(&self) -> bool {
        true
    }

    fn supports_power_off(&self) -> bool {
        true
    }

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
        std::mem::size_of::<SaturatorRuntime>()
    }

    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(self))
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

impl crate::inline_owned::InlinePreparedProcessor for PreparedSaturator {
    type Runtime = SaturatorRuntime;
    fn create_inline_runtime(&self) -> Self::Runtime {
        self.check_inline_ownership();
        let runtime = SaturatorRuntime {
            base: *self,
            controls: crate::parameter_binding::Controls::new(
                self.settings,
                std::array::from_fn(|index| self.automation_mask & (1u8 << index) != 0),
            ),
            settings: self.settings,
            sample_rate: self.sample_rate,
            gain_ramps: [
                hex_dsp::LinearRamp::settled(self.drive_gain),
                hex_dsp::LinearRamp::settled(self.output_gain),
            ],
            kernel: Kernel::shared(),
            channels: [Oversampler::default(); 2],
            delay: [[0.0; 2]; LATENCY_FRAMES],
            delay_index: 0,
        };
        runtime.check_inline_ownership();
        runtime
    }
}

/// Zero input frames after which every history is exactly zero and output is
/// silent: the oversampler drain, which outlasts the 11-frame dry delay.
const TAIL_FRAMES: u64 = crate::oversampling::DRAIN_FRAMES as u64;
const _: () = assert!(crate::oversampling::DRAIN_FRAMES >= LATENCY_FRAMES);

/// Closed fixed-inline DSP owner, including the last effective controls used by
/// transactional history copying. It owns no program, asset or external handle.
///
/// A direct drive or output edit arrives as a new runtime. Each fallback gain
/// ramps from the previous effective value to its new target over 5 ms, even if
/// another control is automated. Eligibility is independent of value presence;
/// authored gains keep their per-frame law. Each ramp is independent of sample
/// rate and block partition.
/// Curve changes switch immediately: the two clip laws are not crossfaded.
#[derive(Clone, Copy, Debug)]
pub struct SaturatorRuntime {
    base: PreparedSaturator,
    controls: crate::parameter_binding::Controls<SaturatorSettings, 4>,
    settings: SaturatorSettings,
    sample_rate: u32,
    gain_ramps: [hex_dsp::LinearRamp; 2],
    kernel: &'static Kernel,
    channels: [Oversampler; 2],
    /// Dry input, delayed by the oversampler latency.
    delay: [[f64; 2]; LATENCY_FRAMES],
    delay_index: usize,
}

/// Direct-edit gain ramp frames: 5 ms at every rate, like Compressor's dry/wet.
fn gain_ramp_frames(sample_rate: u32) -> u32 {
    hex_dsp::ramp_frames_for_ms(sample_rate, 5.0).unwrap_or(1)
}

impl SaturatorRuntime {
    #[inline]
    fn drive_gain(&self) -> f64 {
        self.gain_ramps[0].value()
    }

    #[inline]
    fn output_gain(&self) -> f64 {
        self.gain_ramps[1].value()
    }

    #[inline]
    fn gains_settled(&self) -> bool {
        self.gain_ramps[0].is_settled() && self.gain_ramps[1].is_settled()
    }

    /// Renders one frame and advances every history. Bypassed frames keep
    /// their input; an active direct-edit ramp advances only when audible.
    fn render_frame(&mut self, frame: &mut StereoFrame, bypassed: bool) {
        if !bypassed {
            self.advance_gain_ramp();
        }
        let settings = self.settings;
        let wet = f64::from(settings.dry_wet);
        let input = frame.map(|sample| {
            if sample.is_finite() {
                f64::from(sample)
            } else {
                0.0
            }
        });
        let drive_gain = self.drive_gain();
        let output_gain = self.output_gain();
        let mut processed = [0.0; 2];
        for (channel, (stage, value)) in self.channels.iter_mut().zip(input).enumerate() {
            let shaped = match settings.curve {
                SaturationCurve::SoftClip => {
                    stage.process_sample(value, drive_gain, self.kernel, &SoftClip)
                }
                SaturationCurve::DigitalClip => {
                    stage.process_sample(value, drive_gain, self.kernel, &DigitalClip)
                }
            };
            processed[channel] = shaped * output_gain;
        }
        let dry = std::mem::replace(&mut self.delay[self.delay_index], input);
        self.delay_index = (self.delay_index + 1) % LATENCY_FRAMES;
        if bypassed {
            return;
        }
        for channel in 0..2 {
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

    /// Advance each fallback gain independently; eligibility alone never
    /// suppresses a direct edit. Authored values retain their per-frame law.
    #[inline]
    fn advance_gain_ramp(&mut self) {
        if self.gains_settled() {
            return;
        }
        for (index, ramp) in self.gain_ramps.iter_mut().enumerate() {
            if ramp.is_settled() || self.controls.has_value(index) {
                continue;
            }
            ramp.advance_by_remaining();
        }
    }

    /// Bypass is already a discontinuity; resume from exact fallback targets.
    fn finish_gain_ramp(&mut self) {
        if self.gains_settled() {
            return;
        }
        for (index, (ramp, target)) in self
            .gain_ramps
            .iter_mut()
            .zip([self.base.drive_gain, self.base.output_gain])
            .enumerate()
        {
            if ramp.is_active() {
                if !self.controls.has_value(index) {
                    ramp.reset(target);
                } else {
                    ramp.reset(ramp.value());
                }
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
            self.finish_gain_ramp();
            self.render_frame(frame, true);
        }
        Ok(())
    }

    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        if self.gains_settled()
            && self.channels[0].is_drained()
            && self.channels[1].is_drained()
            && frames
                .iter()
                .all(|f| (f[0].to_bits() | f[1].to_bits()) == 0)
            && self.delay.iter().flatten().all(|&v| v == 0.0)
        {
            self.delay_index = (self.delay_index + frames.len()) % LATENCY_FRAMES;
            return;
        }
        if !self.gains_settled() {
            for frame in frames {
                self.render_frame(frame, false);
            }
            return;
        }
        let settings = self.settings;
        let wet = f64::from(settings.dry_wet);
        let dry_mix = 1.0 - wet;
        let drive = self.drive_gain();
        let output_gain = self.output_gain();
        let kernel = self.kernel;
        for chunk in frames.chunks_mut(BLOCK_CHUNK) {
            let n = chunk.len();
            if !chunk.iter().all(|f| f[0].is_finite() && f[1].is_finite()) {
                for frame in chunk {
                    self.render_frame(frame, false);
                }
                continue;
            }
            let mut left = [0.0_f32; BLOCK_CHUNK];
            let mut right = [0.0_f32; BLOCK_CHUNK];
            let mut dry_l = [0.0_f64; BLOCK_CHUNK];
            let mut dry_r = [0.0_f64; BLOCK_CHUNK];
            for i in 0..n {
                let in_l = chunk[i][0];
                let in_r = chunk[i][1];
                left[i] = in_l;
                right[i] = in_r;
                let dry = std::mem::replace(
                    &mut self.delay[self.delay_index],
                    [f64::from(in_l), f64::from(in_r)],
                );
                self.delay_index = (self.delay_index + 1) % LATENCY_FRAMES;
                dry_l[i] = dry[0];
                dry_r[i] = dry[1];
            }
            let [left_stage, right_stage] = &mut self.channels;
            match settings.curve {
                SaturationCurve::SoftClip => Oversampler::process_stereo_block(
                    left_stage,
                    right_stage,
                    &mut left[..n],
                    &mut right[..n],
                    drive,
                    kernel,
                    &SoftClip,
                ),
                SaturationCurve::DigitalClip => Oversampler::process_stereo_block(
                    left_stage,
                    right_stage,
                    &mut left[..n],
                    &mut right[..n],
                    drive,
                    kernel,
                    &DigitalClip,
                ),
            }
            if wet == 0.0 {
                for i in 0..n {
                    chunk[i][0] = dry_l[i] as f32;
                    chunk[i][1] = dry_r[i] as f32;
                }
            } else if wet == 1.0 {
                for i in 0..n {
                    chunk[i][0] = (f64::from(left[i]) * output_gain) as f32;
                    chunk[i][1] = (f64::from(right[i]) * output_gain) as f32;
                }
            } else {
                for i in 0..n {
                    chunk[i][0] =
                        (dry_l[i] * dry_mix + f64::from(left[i]) * output_gain * wet) as f32;
                    chunk[i][1] =
                        (dry_r[i] * dry_mix + f64::from(right[i]) * output_gain * wet) as f32;
                }
            }
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
        self.gain_ramps = [
            hex_dsp::LinearRamp::settled(self.base.drive_gain),
            hex_dsp::LinearRamp::settled(self.base.output_gain),
        ];
        self.clear_history();
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
        }
        let frames = gain_ramp_frames(self.sample_rate);
        for (index, (target, old_target, parameter)) in [
            (
                self.base.drive_gain,
                previous.base.drive_gain,
                parameter::DRIVE,
            ),
            (
                self.base.output_gain,
                previous.base.output_gain,
                parameter::OUTPUT,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let mut ramp = previous.gain_ramps[index];
            if ramp.value() == target {
                ramp.reset(target);
            } else if target == old_target
                && (self.base.supports(parameter) || !previous.controls.has_value(index))
            {
                // Transactional twins retain each in-flight trajectory.
            } else {
                ramp.retarget(target, frames);
            }
            self.gain_ramps[index] = ramp;
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
            magnitude
                - 0.25 * hex_dsp::fast_ln_f64(magnitude)
                - 0.375
                - 0.25 * std::f64::consts::LN_2
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
