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
//! through a 20 ms one-pole smoother that snaps to its target. Each runtime owns
//! its own noise generator with a fixed seed, so renders are deterministic and
//! independent of block size. Non-finite input is silenced before it enters
//! recursive history, recursive states flush denormals, and long exact silence
//! clears every history, giving an explicit finite tail. No callback allocation,
//! locks, logging, resources or project-schema changes are needed.
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
const FLOOR: f64 = 1.0e-20;
const NOISE_SEEDS: [u32; 2] = [0x9e37_79b9, 0x85eb_ca6b];

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const DRIVE: ParameterId = ParameterId::from_static(1);
}

const SMOOTHED_ENVELOPE: hex_lattice_contract::ParameterSemantics =
    hex_lattice_contract::ParameterSemantics {
        smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
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
        let base_rate = f64::from(sample_rate);
        let fast_rate = base_rate * 2.0;
        let g = (std::f64::consts::PI * SPLIT_HZ / fast_rate).tan();
        let damping = std::f64::consts::SQRT_2;
        Ok(PreparedRattle {
            settings,
            sample_rate,
            automated,
            coefficients: Coefficients {
                halfband: halfband(),
                smoothing: 1.0 - (-1_000.0 / (SMOOTHING_MS * base_rate)).exp(),
                dc_pole: (-std::f64::consts::TAU * DC_BLOCK_HZ / fast_rate).exp(),
                svf_g: g,
                svf_damping: damping,
                svf_norm: 1.0 / (1.0 + g * (g + damping)),
                attack: (-1.0 / (0.001 * fast_rate)).exp(),
                release: (-1.0 / (0.040 * fast_rate)).exp(),
                slew_per_hz: std::f64::consts::TAU / fast_rate,
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

/// Blackman-windowed sinc halfband at a quarter of the oversampled rate, unit DC gain.
fn halfband() -> [f64; TAPS] {
    let centre = (TAPS - 1) as f64 / 2.0;
    let mut taps = std::array::from_fn(|n| {
        let offset = n as f64 - centre;
        let sinc = if offset == 0.0 {
            0.5
        } else {
            (std::f64::consts::FRAC_PI_2 * offset).sin() / (std::f64::consts::PI * offset)
        };
        let phase = std::f64::consts::TAU * n as f64 / (TAPS - 1) as f64;
        sinc * (0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos())
    });
    let sum: f64 = taps.iter().sum();
    for tap in &mut taps {
        *tap /= sum;
    }
    taps
}

/// Control-prepared rate-dependent constants; fixed inline, no transcendental work per sample.
#[derive(Clone, Copy, Debug)]
struct Coefficients {
    halfband: [f64; TAPS],
    smoothing: f64,
    dc_pole: f64,
    svf_g: f64,
    svf_damping: f64,
    svf_norm: f64,
    attack: f64,
    release: f64,
    slew_per_hz: f64,
    silence_frames: u32,
}

/// Closed fixed-inline rattle factory. Envelope storage remains host-owned.
#[derive(Clone, Copy, Debug)]
pub struct PreparedRattle {
    settings: RattleSettings,
    sample_rate: u32,
    automated: bool,
    coefficients: Coefficients,
}

impl PreparedRattle {
    fn supports(&self, parameter: ParameterId) -> bool {
        self.automated && automation_supported(parameter)
    }
}

impl PreparedNativeProcessor for PreparedRattle {
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
            target: drive,
            drive,
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
    if value.abs() < FLOOR { 0.0 } else { value }
}

/// Per-channel oversampling, filter, envelope, noise and slew history.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Channel {
    /// Newest-first base-rate input; `input[HISTORY - 1]` is the latency-aligned dry sample.
    input: [f64; HISTORY],
    /// Newest-first shaped oversampled samples feeding the decimator.
    shaped: [f64; TAPS],
    dc_input: f64,
    dc_output: f64,
    out_dc: [f64; 2],
    svf: [f64; 2],
    envelope: f64,
    pink: [f64; 3],
    slew: f64,
    noise: u32,
    seed: u32,
}

impl Channel {
    const fn new(seed: u32) -> Self {
        Self {
            input: [0.0; HISTORY],
            shaped: [0.0; TAPS],
            dc_input: 0.0,
            dc_output: 0.0,
            out_dc: [0.0; 2],
            svf: [0.0; 2],
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

    /// Three-pole approximation of a -3 db/octave spectrum, roughly unit peak.
    fn pink(&mut self) -> f64 {
        let white = self.white();
        self.pink[0] = quiet(0.997_65 * self.pink[0] + 0.099_046 * white);
        self.pink[1] = quiet(0.963 * self.pink[1] + 0.296_516 * white);
        self.pink[2] = quiet(0.57 * self.pink[2] + 1.052_691 * white);
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
        let [s1, s2] = self.svf;
        let high = (soft - (c.svf_damping + c.svf_g) * s1 - s2) * c.svf_norm;
        let band = c.svf_g * high + s1;
        let low = c.svf_g * band + s2;
        self.svf = [quiet(c.svf_g * high + band), quiet(c.svf_g * band + low)];
        let split = (soft - high) + (shape.high_drive * high).tanh() * shape.high_gain;
        // Grit follows the program envelope, so silence stays silent.
        let level = split.abs();
        let pole = if level > self.envelope {
            c.attack
        } else {
            c.release
        };
        self.envelope = quiet(level + pole * (self.envelope - level));
        let gritty = split + shape.grit * self.envelope * self.pink();
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
        self.input.copy_within(0..HISTORY - 1, 1);
        self.input[0] = input;
        // Polyphase zero-stuffing interpolation: even taps then odd taps, gain 2.
        let mut even = 0.0;
        let mut odd = 0.0;
        for (k, sample) in self.input.iter().enumerate() {
            even += c.halfband[2 * k] * sample;
            if let Some(tap) = c.halfband.get(2 * k + 1) {
                odd += tap * sample;
            }
        }
        let first = self.shape(2.0 * even, shape, c);
        self.shaped.copy_within(0..TAPS - 1, 1);
        self.shaped[0] = first;
        // Decimate on the even phase so the total delay is a whole base frame count.
        let wet = c
            .halfband
            .iter()
            .zip(&self.shaped)
            .map(|(tap, sample)| tap * sample)
            .sum();
        let second = self.shape(2.0 * odd, shape, c);
        self.shaped.copy_within(0..TAPS - 1, 1);
        self.shaped[0] = second;
        (self.input[HISTORY - 1], wet)
    }
}

/// Closed fixed-inline DSP history and effective-control owner. It owns no
/// program, asset or external handle.
#[derive(Clone, Copy, Debug)]
pub struct RattleRuntime {
    base: PreparedRattle,
    controls: crate::parameter_binding::Controls<RattleSettings, 1>,
    target: f64,
    drive: f64,
    shape: Shape,
    channels: [Channel; 2],
    silent_frames: u32,
}

impl RattleRuntime {
    fn clear_history(&mut self) {
        self.channels = [Channel::new(NOISE_SEEDS[0]), Channel::new(NOISE_SEEDS[1])];
    }

    fn advance_drive(&mut self) {
        if self.drive == self.target {
            return;
        }
        let next = self.drive + (self.target - self.drive) * self.base.coefficients.smoothing;
        self.drive = if (next - self.target).abs() < 1.0e-6 {
            self.target
        } else {
            next
        };
        self.shape = Shape::new(self.drive, &self.base.coefficients);
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
                self.target = f64::from(next.effective().drive);
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
    fn process_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        _clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        values: &dyn hex_lattice::NativeParameterAutomation,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), DeviceContractError> {
        self.process_automated(frames, values, false)
    }
    fn process_bypassed_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        _clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        values: &dyn hex_lattice::NativeParameterAutomation,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), DeviceContractError> {
        self.process_automated(frames, values, true)
    }
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
        self.target = f64::from(self.base.settings.drive);
        self.drive = self.target;
        self.shape = Shape::new(self.drive, &self.base.coefficients);
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
        self.drive = previous.drive;
        self.shape = previous.shape;
        if self.base.automated && previous.base.automated {
            self.controls.inherit(&previous.controls);
            self.target = previous.target;
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
