//! Original one-knob stereo drive with oversampled grit for the native host.
//!
//! Owns one automatable `drive` control (0–1, displayed as percent), a v1
//! little-endian float codec and a fixed-inline runtime. Per channel the input
//! is upsampled 2× through a linear-phase halfband FIR, shaped, and decimated
//! through the same FIR. The shaper runs, at the oversampled rate:
//! biased tanh drive with DC blocking, an algebraic soft clip, a 1 kHz
//! state-variable high-pass split whose high band is re-saturated, envelope-
//! scaled pink-noise grit, a slew limiter and a final tanh bound. The output is
//! divided by a compensation factor that grows with drive. The smoothed drive is
//! squared for the shaper, while a sigmoid curve sets the dry/wet balance.
//!
//! The dry path is delayed by exactly the declared [`LATENCY_FRAMES`], so a
//! settled drive of zero returns the delayed input bit-exactly. Drive moves
//! through a 20 ms one-pole smoother that snaps to its target. The native host
//! can supply a new drive target each frame, including sample-rate modulation. Each runtime owns
//! its own noise generator with a fixed seed, so renders are deterministic and
//! independent of block size. Non-finite input is silenced before it enters
//! recursive history, recursive states flush denormals, and long exact silence
//! clears every history, giving an explicit finite tail. No callback allocation,
//! locks, logging, resources or project-schema changes are needed.
//! The host keeps this same 11-frame latency during bypass, advancing ordinary
//! processing/automation and finite-tail history behind an aligned dry ramp.
//!
//! This is an independent implementation from a behavioural description of a
//! third-party device. It establishes no measured transfer function or parity
//! with that device.

use hex_lattice::{
    NativeDeviceDefinition, NativeProcessorRuntime, OutputTail, PreparedNativeProcessor,
    StereoFrame, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;

pub const RATTLE_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.rattle");
pub const RATTLE_STATE_VERSION: u32 = 1;
const STATE_BYTES: usize = 4;
const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;
/// Halfband taps at the 2× rate; odd so both resampling filters stay linear phase.
const TAPS: usize = 23;
/// Base-rate history needed by the polyphase upsampler.
const HISTORY: usize = TAPS.div_ceil(2);
/// Up- plus down-sampling group delay: `TAPS - 1` oversampled samples.
pub const LATENCY_FRAMES: u32 = ((TAPS - 1) / 2) as u32;
const SMOOTHING_MS: f64 = 20.0;
const SPLIT_HZ: f64 = 1_000.0;
const DC_BLOCK_HZ: f64 = 10.0;
const NOISE_SEEDS: [u32; 2] = [0x9e37_79b9, 0x85eb_ca6b];

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const DRIVE: ParameterId = ParameterId::from_static(1);
}

const SMOOTHED_ENVELOPE: hex_lattice_contract::ParameterSemantics =
    hex_lattice_contract::ParameterSemantics {
        smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
        modulation: hex_lattice_contract::ParameterModulation::SampleRate,
        ..crate::parameter_binding::SCALAR_ENVELOPE
    };
const BINDINGS: [crate::parameter_binding::Binding<RattleSettings>; 1] =
    [crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::DRIVE,
            label: "drive",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.4),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_ENVELOPE,
        |c| c.drive,
        |c, v| c.drive = v,
    )];
const DESCRIPTORS: &[ParameterDescriptor] =
    &crate::parameter_binding::descriptors(&BINDINGS, BINDINGS[0].descriptor);

#[derive(Clone, Copy, Debug, PartialEq)]
struct RattleSettings {
    drive: f32,
}

impl Default for RattleSettings {
    fn default() -> Self {
        Self { drive: 0.4 }
    }
}

fn validate(settings: RattleSettings) -> Result<(), DeviceContractError> {
    for binding in &BINDINGS {
        binding.descriptor.validate(binding.read(&settings))?;
    }
    Ok(())
}

fn decode(state: &NativeDeviceState) -> Result<RattleSettings, DeviceContractError> {
    if state.type_id() != RATTLE_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != RATTLE_STATE_VERSION {
        return Err(DeviceContractError::UnsupportedStateVersion);
    }
    let bytes: [u8; STATE_BYTES] = state
        .payload()
        .try_into()
        .map_err(|_| DeviceContractError::MalformedState)?;
    let settings = RattleSettings {
        drive: f32::from_le_bytes(bytes),
    };
    validate(settings)?;
    Ok(settings)
}

fn encode(settings: RattleSettings) -> Result<NativeDeviceState, DeviceContractError> {
    validate(settings)?;
    NativeDeviceState::new(
        RATTLE_TYPE_ID,
        RATTLE_STATE_VERSION,
        settings.drive.to_le_bytes().to_vec(),
    )
}

fn automation_supported(parameter: ParameterId) -> bool {
    crate::parameter_binding::binding(&BINDINGS, parameter).is_ok_and(|binding| {
        binding.declares_arrangement_automation()
            && binding.semantics.smoothing
                == hex_lattice_contract::ParameterSmoothing::DeviceManaged
            && binding.semantics.change_impact
                == hex_lattice_contract::ParameterChangeImpact::ValueOnly
    })
}

/// Registered rattle metadata, strict portable codec and control-side preparation.
pub struct RattleDefinition;
pub static RATTLE_DEFINITION: RattleDefinition = RattleDefinition;

impl NativeDeviceMetadata for RattleDefinition {
    fn type_id(&self) -> DeviceTypeId {
        RATTLE_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("rattle")
    }
    fn state_version(&self) -> u32 {
        RATTLE_STATE_VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        encode(RattleSettings::default())
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
        crate::parameter_binding::parameter_semantics_for_state(&BINDINGS, state, id, decode)
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

impl RattleDefinition {
    /// Asset-free, fixed-inline constructor used by both preparation paths.
    /// All transcendental coefficient work happens here, on control.
    pub fn prepare_owned(
        state: &NativeDeviceState,
        sample_rate: u32,
        parameters: &[ParameterId],
    ) -> Result<PreparedRattle, DeviceContractError> {
        state.require_asset_free_processing()?;
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate)
            || parameters.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let settings = decode(state)?;
        let mut automated = false;
        for parameter in parameters {
            let binding = crate::parameter_binding::binding(&BINDINGS, *parameter)?;
            binding.semantics.validate(binding.descriptor)?;
            if !automation_supported(*parameter) {
                return Err(DeviceContractError::UnsupportedProcessingClass);
            }
            automated = true;
        }
        let fast_rate = f64::from(sample_rate) * 2.0;
        let g = (std::f64::consts::PI * SPLIT_HZ / fast_rate).tan();
        let damping = std::f64::consts::SQRT_2;
        let smoothing = hex_dsp::one_pole_coefficient(sample_rate, SMOOTHING_MS * 0.001)
            .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        let dc_pole = hex_dsp::frequency_pole(DC_BLOCK_HZ, fast_rate)
            .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        let fast_rate_u32 = sample_rate
            .checked_mul(2)
            .ok_or(DeviceContractError::InvalidParameterValue)?;
        let attack = hex_dsp::one_pole_pole(fast_rate_u32, 0.001)
            .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        let release = hex_dsp::one_pole_pole(fast_rate_u32, 0.040)
            .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        let drive_smoother =
            hex_dsp::OnePoleSmoother::from_coefficient(f64::from(settings.drive), smoothing)
                .and_then(|s| s.with_snap_epsilon(1.0e-6))
                .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        // Paul Kellet 3-pole pink-noise approximation scaled from its 44.1 kHz
        // reference to the actual 2x oversampled rate, preserving corner
        // frequencies and per-branch DC gain across 16–384 kHz.
        const PINK_REF_RATE: f64 = 44_100.0;
        const PINK_REF_POLES: [f64; 3] = [0.997_65, 0.963, 0.57];
        const PINK_REF_GAINS: [f64; 3] = [0.099_046, 0.296_516, 1.052_691];
        let pink_ratio = PINK_REF_RATE / fast_rate;
        let pink_poles = PINK_REF_POLES.map(|p| p.powf(pink_ratio));
        let pink_gains = std::array::from_fn(|i| {
            PINK_REF_GAINS[i] * (1.0 - pink_poles[i]) / (1.0 - PINK_REF_POLES[i])
        });
        Ok(PreparedRattle {
            settings,
            sample_rate,
            automated,
            drive_smoother,
            coefficients: Coefficients {
                halfband: hex_dsp::Halfband2xKernel::blackman(),
                dc_pole,
                svf_g: g,
                svf_damping: damping,
                svf_norm: 1.0 / (1.0 + g * (g + damping)),
                attack,
                release,
                slew_per_hz: std::f64::consts::TAU / fast_rate,
                pink_poles,
                pink_gains,
                silence_frames: LATENCY_FRAMES + sample_rate / 4,
            },
        })
    }
}

impl NativeDeviceDefinition for RattleDefinition {
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
        Ok(automation_supported(parameter))
    }
    fn prepare_automated_runtime_instance(
        &self,
        _identity: hex_lattice::ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
        parameters: &[ParameterId],
    ) -> Result<hex_lattice::PreparedRuntimeDevice, DeviceContractError> {
        Ok(crate::parameter_binding::prepared_legacy_runtime(
            Self::prepare_owned(state, context.sample_rate, parameters)?,
        ))
    }
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
        Ok(Some(Box::new(Self::prepare_owned(
            state,
            context.sample_rate,
            &[],
        )?)))
    }
}

/// Control-prepared rate-dependent constants; fixed inline, no transcendental work per sample.
#[derive(Clone, Copy, Debug)]
struct Coefficients {
    halfband: hex_dsp::Halfband2xKernel<TAPS, HISTORY>,
    dc_pole: f64,
    svf_g: f64,
    svf_damping: f64,
    svf_norm: f64,
    attack: f64,
    release: f64,
    slew_per_hz: f64,
    pink_poles: [f64; 3],
    pink_gains: [f64; 3],
    silence_frames: u32,
}

/// Closed fixed-inline rattle factory. Envelope storage remains host-owned.
#[derive(Clone, Copy, Debug)]
pub struct PreparedRattle {
    settings: RattleSettings,
    sample_rate: u32,
    automated: bool,
    drive_smoother: hex_dsp::OnePoleSmoother,
    coefficients: Coefficients,
}

impl PreparedRattle {
    fn supports(&self, parameter: ParameterId) -> bool {
        self.automated && automation_supported(parameter)
    }
}

impl PreparedNativeProcessor for PreparedRattle {
    // The host keeps the exact 11-frame dry delay through settled Off.
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
            .then(|| {
                crate::parameter_binding::binding(&BINDINGS, parameter)
                    .ok()
                    .map(|b| b.descriptor)
            })
            .flatten()
    }
    fn supports_parameter_automation(&self, parameter: ParameterId) -> bool {
        self.supports(parameter)
    }
    fn parameter_modulation_semantics(
        &self,
        parameter: ParameterId,
    ) -> Option<hex_lattice_contract::ParameterSemantics> {
        self.supports(parameter).then_some(SMOOTHED_ENVELOPE)
    }
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<RattleRuntime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        let drive = f64::from(self.settings.drive);
        Box::new(RattleRuntime {
            base: *self,
            controls: crate::parameter_binding::Controls::new(self.settings, [self.automated]),
            drive: self.drive_smoother,
            shape: Shape::new(drive, &self.coefficients),
            channels: [Channel::new(NOISE_SEEDS[0]), Channel::new(NOISE_SEEDS[1])],
            silent_frames: 0,
        })
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(LATENCY_FRAMES)
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite {
            max_tail_frames: u64::from(self.coefficients.silence_frames),
        }
    }
}

/// Drive-derived shaper constants, recomputed only while the smoothed drive moves.
#[derive(Clone, Copy, Debug)]
struct Shape {
    pre_gain: f64,
    bias: f64,
    bias_offset: f64,
    high_drive: f64,
    high_gain: f64,
    grit: f64,
    slew_step: f64,
    output_gain: f64,
    wet: f64,
}

/// Sigmoid dry/wet balance: exactly 0 at drive 0 and 1 at full drive.
fn wet_curve(drive: f64) -> f64 {
    let sigmoid = |x: f64| 1.0 / (1.0 + (-10.0 * (x - 0.3)).exp());
    let low = sigmoid(0.0);
    ((sigmoid(drive) - low) / (sigmoid(1.0) - low)).clamp(0.0, 1.0)
}

impl Shape {
    fn new(drive: f64, coefficients: &Coefficients) -> Self {
        let amount = drive * drive;
        let bias = 0.2 * amount;
        let high_drive = 1.0 + 8.0 * amount;
        // Slew ceiling falls from 20 khz to 2.5 khz at full scale as drive rises.
        let slew_hz = 20_000.0 - 17_500.0 * amount;
        Self {
            pre_gain: 1.0 + 30.0 * amount,
            bias,
            bias_offset: bias.tanh(),
            high_drive,
            high_gain: high_drive.sqrt().recip(),
            grit: 0.35 * amount * drive,
            slew_step: slew_hz * coefficients.slew_per_hz,
            output_gain: (1.0 + COMPENSATION * (6.0 * amount).tanh()).recip(),
            wet: wet_curve(drive),
        }
    }
}

/// Fit so a -12 dbfs 220 hz sine stays within about 0.5 db rms across drive.
const COMPENSATION: f64 = 2.2;

fn quiet(value: f64) -> f64 {
    hex_dsp::flush_quiet(value)
}

/// Per-channel oversampling, filter, envelope, noise and slew history.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Channel {
    halfband: hex_dsp::Halfband2x<TAPS, HISTORY>,
    dc_input: f64,
    dc_output: f64,
    out_dc: [f64; 2],
    svf: hex_dsp::SvfSection,
    envelope: f64,
    pink: [f64; 3],
    slew: f64,
    noise: u32,
    seed: u32,
}

impl Channel {
    const fn new(seed: u32) -> Self {
        Self {
            halfband: hex_dsp::Halfband2x::new(),
            dc_input: 0.0,
            dc_output: 0.0,
            out_dc: [0.0; 2],
            svf: hex_dsp::SvfSection::new(),
            envelope: 0.0,
            pink: [0.0; 3],
            slew: 0.0,
            noise: seed,
            seed,
        }
    }

    fn is_clear(&self) -> bool {
        *self == Self::new(self.seed)
    }

    /// Uniform white noise in [-1, 1) from a xorshift generator.
    fn white(&mut self) -> f64 {
        let mut x = self.noise;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.noise = x;
        f64::from(x) / 2_147_483_648.0 - 1.0
    }

    /// Three-pole approximation of a -3 db/octave spectrum, roughly unit peak,
    /// using poles scaled to the 2x oversampled rate in `Coefficients`.
    fn pink(&mut self, c: &Coefficients) -> f64 {
        let white = self.white();
        for i in 0..3 {
            self.pink[i] = quiet(c.pink_poles[i] * self.pink[i] + c.pink_gains[i] * white);
        }
        (self.pink[0] + self.pink[1] + self.pink[2] + 0.184_8 * white) * 0.2
    }

    fn shape(&mut self, input: f64, shape: &Shape, c: &Coefficients) -> f64 {
        // Biased tanh drive; subtracting the bias keeps silence at zero.
        let driven = (shape.pre_gain * input + shape.bias).tanh() - shape.bias_offset;
        // One-pole DC blocker removes the asymmetric stage's offset.
        let blocked = quiet(driven - self.dc_input + c.dc_pole * self.dc_output);
        self.dc_input = driven;
        self.dc_output = blocked;
        let soft = blocked / (1.0 + blocked * blocked).sqrt();
        // Topology-preserving state-variable high-pass split.
        let high = self
            .svf
            .process_outputs_precomputed(soft, c.svf_g, c.svf_damping, c.svf_norm)
            .high;
        let split = (soft - high) + (shape.high_drive * high).tanh() * shape.high_gain;
        // Grit follows the program envelope, so silence stays silent.
        let level = split.abs();
        let pole = if level > self.envelope {
            c.attack
        } else {
            c.release
        };
        self.envelope = quiet(level + pole * (self.envelope - level));
        let gritty = split + shape.grit * self.envelope * self.pink(c);
        let step = (gritty - self.slew).clamp(-shape.slew_step, shape.slew_step);
        self.slew = quiet(self.slew + step);
        let bounded = self.slew.tanh();
        // The asymmetric shaping above can leave a small offset; block it again.
        let [previous_in, previous_out] = self.out_dc;
        let centred = quiet(bounded - previous_in + c.dc_pole * previous_out);
        self.out_dc = [bounded, centred];
        centred * shape.output_gain
    }

    /// Process one base-rate sample, returning `(delayed dry, wet)`.
    fn frame(&mut self, input: f64, shape: &Shape, c: &Coefficients) -> (f64, f64) {
        let (dry, first_in, second_in) = self.halfband.upsample_sample(input, &c.halfband);
        let first = self.shape(first_in, shape, c);
        let second = self.shape(second_in, shape, c);
        let wet = self.halfband.downsample_sample(first, second, &c.halfband);
        (dry, wet)
    }
}

/// Closed fixed-inline DSP history and effective-control owner. It owns no
/// program, asset or external handle.
#[derive(Clone, Copy, Debug)]
pub struct RattleRuntime {
    base: PreparedRattle,
    controls: crate::parameter_binding::Controls<RattleSettings, 1>,
    drive: hex_dsp::OnePoleSmoother,
    shape: Shape,
    channels: [Channel; 2],
    silent_frames: u32,
}

impl RattleRuntime {
    fn clear_history(&mut self) {
        self.channels = [Channel::new(NOISE_SEEDS[0]), Channel::new(NOISE_SEEDS[1])];
    }

    fn advance_drive(&mut self) {
        if self.drive.is_settled() {
            return;
        }
        let next = self.drive.advance();
        self.shape = Shape::new(next, &self.base.coefficients);
    }

    fn process_frame(&mut self, frame: &mut StereoFrame) {
        self.advance_drive();
        // Reject poisoned input before it can enter recursive histories.
        if !frame.iter().all(|sample| sample.is_finite()) {
            *frame = [0.0; 2];
        }
        if *frame == [0.0; 2] {
            let limit = self.base.coefficients.silence_frames;
            self.silent_frames = self.silent_frames.saturating_add(1).min(limit);
            if self.silent_frames == limit {
                // Explicit finite tail: every history returns to its initial state.
                if !self.channels.iter().all(Channel::is_clear) {
                    self.clear_history();
                }
                return;
            }
        } else {
            self.silent_frames = 0;
        }
        let wet_mix = self.shape.wet;
        for (sample, channel) in frame.iter_mut().zip(&mut self.channels) {
            let (dry, wet) =
                channel.frame(f64::from(*sample), &self.shape, &self.base.coefficients);
            *sample = if wet_mix == 0.0 {
                // Bit-exact latency-aligned input at a settled drive of zero.
                dry as f32
            } else {
                (dry + (wet - dry) * wet_mix) as f32
            };
        }
    }

    fn process_automated(
        &mut self,
        frames: &mut [StereoFrame],
        values: &dyn hex_lattice::NativeParameterAutomation,
        bypassed: bool,
    ) -> Result<(), DeviceContractError> {
        let mut resolved = self.controls.resolve(&BINDINGS, values)?;
        for (offset, frame) in frames.iter_mut().enumerate() {
            if let Some(next) = self
                .controls
                .sample(&BINDINGS, &mut resolved, values, offset)?
            {
                // Validated typed value; smoothing is infallible DSP-only work.
                self.drive.set_target(f64::from(next.effective().drive));
                self.controls.install(next);
            }
            let original = *frame;
            self.process_frame(frame);
            if bypassed {
                *frame = original;
            }
        }
        Ok(())
    }
}

impl NativeProcessorRuntime for RattleRuntime {
    fn supports_parameter_automation(&self, parameter: ParameterId) -> bool {
        self.base.supports(parameter)
    }
    crate::parameter_binding::delegate_automated_block_processing!(process_automated);
    fn process_block(&mut self, frames: &mut [StereoFrame], _: &dyn hex_lattice::DeviceResources) {
        for frame in frames {
            self.process_frame(frame);
        }
    }
    /// Bypass passes input through untouched while history keeps advancing,
    /// so re-enabling resumes without a stale oversampler or envelope.
    fn process_bypassed_block(
        &mut self,
        frames: &mut [StereoFrame],
        _: &dyn hex_lattice::DeviceResources,
    ) {
        for frame in frames {
            let mut scratch = *frame;
            self.process_frame(&mut scratch);
        }
    }
    fn reset(&mut self) {
        self.controls.reset();
        let drive = f64::from(self.base.settings.drive);
        self.drive.reset(drive);
        self.shape = Shape::new(drive, &self.base.coefficients);
        self.clear_history();
        self.silent_frames = 0;
    }
    fn is_quiescent(&self) -> bool {
        self.channels.iter().all(Channel::is_clear)
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| previous.base.sample_rate == self.base.sample_rate)
    }
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        let Some(previous) = previous
            .as_any()
            .downcast_ref::<Self>()
            .filter(|previous| previous.base.sample_rate == self.base.sample_rate)
        else {
            return;
        };
        self.channels = previous.channels;
        self.silent_frames = previous.silent_frames;
        // Continue from the audible drive; the new target is reached by smoothing.
        self.drive.inherit_value(&previous.drive);
        self.shape = previous.shape;
        if self.base.automated && previous.base.automated {
            self.controls.inherit(&previous.controls);
            self.drive.set_target(previous.drive.target());
        }
    }
    fn can_retarget_parameters_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        self.can_copy_history_from(previous)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests;
