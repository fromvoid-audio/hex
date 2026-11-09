//! Original pedal-style stereo overdrive for the registered native host.
//!
//! Owns six validated controls, a v1 little-endian float codec, control-prepared
//! coefficients, inline runtime history and a static pre-filter response query.
//! The signal path is a two-pole high/low band-pass, an 8x polyphase-FIR
//! oversampled tanh drive with first-order antiderivative anti-aliasing (see
//! `crate::oversampling`), a tone low-pass and stereo-linked envelope compensation.
//! Dynamics blends compressed output toward the input envelope. The stage
//! declares 11 frames of latency; the dry path and the envelope's input peak
//! are delayed by the same 11 frames so mix and dynamics stay aligned.
//! Compatible revisions inherit history and ramp coefficients/gains over 5 ms;
//! reset clears history and rate changes refuse inheritance. Host bypass keeps
//! the same 11-frame latency and advances the ordinary wet filter/envelope/tail
//! history behind its bounded aligned dry ramp.
//! No callback allocation, resources, host-contract changes or project-schema
//! changes are needed. This is an independent implementation; the supplied image
//! establishes controls/defaults, not measured Ableton transfer functions or parity.

mod automation;

use hex_lattice::{
    NativeDeviceDefinition, NativeProcessorRuntime, OutputTail, PreparedNativeProcessor,
    StereoFrame, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;

use crate::oversampling::{Kernel, LATENCY_FRAMES, Oversampler, Shaper};

/// Overdrive's transfer: tanh, with the overflow-free ln(cosh) antiderivative.
struct Tanh;
impl Shaper for Tanh {
    fn tag(&self) -> u8 {
        1
    }
    fn value(&self, x: f64) -> f64 {
        x.tanh()
    }
    fn antiderivative(&self, x: f64) -> f64 {
        let a = x.abs();
        a + (-2.0 * a).exp().ln_1p() - std::f64::consts::LN_2
    }
}

pub const OVERDRIVE_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.overdrive");
pub const OVERDRIVE_STATE_VERSION: u32 = 1;
const STATE_BYTES: usize = 24;
const FLOOR: f64 = 1.0e-20;

pub mod parameter {
    use hex_lattice_contract::ParameterId;
    pub const FREQUENCY: ParameterId = ParameterId::from_static(1);
    pub const BANDWIDTH: ParameterId = ParameterId::from_static(2);
    pub const DRIVE: ParameterId = ParameterId::from_static(3);
    pub const TONE: ParameterId = ParameterId::from_static(4);
    pub const DYNAMICS: ParameterId = ParameterId::from_static(5);
    pub const DRY_WET: ParameterId = ParameterId::from_static(6);
}

const fn descriptor(
    id: ParameterId,
    label: &'static str,
    min: f32,
    max: f32,
    default: f32,
    unit: Option<&'static str>,
    presentation: ParameterPresentation,
) -> ParameterDescriptor {
    ParameterDescriptor {
        id,
        label,
        kind: ParameterKind::Float { min, max, unit },
        default: ParameterValue::Float(default),
        presentation,
    }
}
const DESCRIPTORS: &[ParameterDescriptor] = &[
    descriptor(
        parameter::FREQUENCY,
        "frequency",
        50.0,
        20_000.0,
        1_250.0,
        Some("hz"),
        ParameterPresentation::Logarithmic,
    ),
    descriptor(
        parameter::BANDWIDTH,
        "bandwidth",
        0.5,
        9.0,
        6.5,
        Some("oct"),
        ParameterPresentation::Linear,
    ),
    descriptor(
        parameter::DRIVE,
        "drive",
        0.0,
        1.0,
        0.5,
        None,
        ParameterPresentation::Linear,
    ),
    descriptor(
        parameter::TONE,
        "tone",
        0.0,
        1.0,
        0.5,
        None,
        ParameterPresentation::Linear,
    ),
    descriptor(
        parameter::DYNAMICS,
        "dynamics",
        0.0,
        1.0,
        0.5,
        None,
        ParameterPresentation::Linear,
    ),
    descriptor(
        parameter::DRY_WET,
        "dry/wet",
        0.0,
        1.0,
        0.5,
        None,
        ParameterPresentation::Linear,
    ),
];

#[derive(Clone, Copy, PartialEq)]
struct Settings([f32; 6]);
// Every admitted control already receives one native value per frame through
// the original coefficient ramp.
const MODULATED_SCALAR: hex_lattice_contract::ParameterSemantics =
    hex_lattice_contract::ParameterSemantics {
        smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
        modulation: hex_lattice_contract::ParameterModulation::SampleRate,
        ..crate::parameter_binding::SCALAR_ENVELOPE
    };
const BINDINGS: [crate::parameter_binding::Binding<Settings>; 6] = [
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[0],
        hex_lattice_contract::ParameterSemantics {
            normalization: hex_lattice_contract::ParameterNormalization::PositiveLogarithmic,
            ..MODULATED_SCALAR
        },
        |c| c.0[0],
        |c, v| c.0[0] = v,
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[1],
        MODULATED_SCALAR,
        |c| c.0[1],
        |c, v| c.0[1] = v,
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[2],
        MODULATED_SCALAR,
        |c| c.0[2],
        |c, v| c.0[2] = v,
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[3],
        MODULATED_SCALAR,
        |c| c.0[3],
        |c, v| c.0[3] = v,
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[4],
        MODULATED_SCALAR,
        |c| c.0[4],
        |c, v| c.0[4] = v,
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[5],
        MODULATED_SCALAR,
        |c| c.0[5],
        |c, v| c.0[5] = v,
    ),
];

impl Default for Settings {
    fn default() -> Self {
        Self([1_250.0, 6.5, 0.5, 0.5, 0.5, 0.5])
    }
}
fn index(id: ParameterId) -> Result<usize, DeviceContractError> {
    DESCRIPTORS
        .iter()
        .position(|d| d.id == id)
        .ok_or(DeviceContractError::UnknownParameter)
}
fn validate(settings: Settings) -> Result<(), DeviceContractError> {
    for (descriptor, value) in DESCRIPTORS.iter().zip(settings.0) {
        descriptor.validate(ParameterValue::Float(value))?;
    }
    Ok(())
}
fn decode(state: &NativeDeviceState) -> Result<Settings, DeviceContractError> {
    if state.type_id() != OVERDRIVE_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != OVERDRIVE_STATE_VERSION {
        return Err(DeviceContractError::UnsupportedStateVersion);
    }
    if state.payload().len() != STATE_BYTES {
        return Err(DeviceContractError::MalformedState);
    }
    let mut settings = Settings::default();
    for (value, bytes) in settings.0.iter_mut().zip(state.payload().chunks_exact(4)) {
        *value = f32::from_le_bytes(
            bytes
                .try_into()
                .map_err(|_| DeviceContractError::MalformedState)?,
        );
    }
    validate(settings)?;
    Ok(settings)
}
fn encode(settings: Settings) -> Result<NativeDeviceState, DeviceContractError> {
    validate(settings)?;
    let mut bytes = Vec::with_capacity(STATE_BYTES);
    for value in settings.0 {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    NativeDeviceState::new(OVERDRIVE_TYPE_ID, OVERDRIVE_STATE_VERSION, bytes)
}

/// Registered metadata, strict portable codec and control-side preparation.
pub struct OverdriveDefinition;
pub static OVERDRIVE_DEFINITION: OverdriveDefinition = OverdriveDefinition;
impl NativeDeviceMetadata for OverdriveDefinition {
    fn type_id(&self) -> DeviceTypeId {
        OVERDRIVE_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("overdrive")
    }
    fn state_version(&self) -> u32 {
        OVERDRIVE_STATE_VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        encode(Settings::default())
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
}

fn validate_rate(rate: u32) -> Result<(), DeviceContractError> {
    if !(8_000..=192_000).contains(&rate) {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    Ok(())
}
fn pole(hz: f64, rate: f64) -> f64 {
    (-std::f64::consts::TAU * hz / rate).exp()
}
fn band_poles(settings: Settings, rate: u32) -> [f64; 2] {
    let rate = f64::from(rate);
    let spread = 2.0_f64.powf(f64::from(settings.0[1]) * 0.5);
    let center = f64::from(settings.0[0]);
    [
        pole((center / spread).clamp(20.0, rate * 0.45), rate),
        pole((center * spread).clamp(20.0, rate * 0.45), rate),
    ]
}
// Static targets are prepared on control. Authored changes convert only their
// changed coefficient groups at delivery; held values reuse these targets.
fn targets(settings: Settings, rate: u32) -> [f64; 6] {
    let [hp, lp] = band_poles(settings, rate);
    [
        hp,
        lp,
        10.0_f64.powf(f64::from(settings.0[2]) * 1.5),
        pole(
            (500.0 * 40.0_f64.powf(f64::from(settings.0[3]))).min(f64::from(rate) * 0.45),
            f64::from(rate),
        ),
        f64::from(settings.0[4]),
        f64::from(settings.0[5]),
    ]
}

impl NativeDeviceDefinition for OverdriveDefinition {
    fn supports_parameter_automation(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<bool, DeviceContractError> {
        decode(state)?;
        Ok(crate::parameter_binding::binding(&BINDINGS, id)
            .is_ok_and(|b| b.declares_arrangement_automation()))
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
                std::sync::Arc::new(PreparedOverdrive::new(
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
        Ok(Some(Box::new(PreparedOverdrive::new(
            state,
            context.sample_rate,
            &[],
        )?)))
    }
}
struct PreparedOverdrive {
    settings: Settings,
    admitted: [bool; 6],
    targets: [f64; 6],
    rate: u32,
    kernel: Kernel,
}
impl PreparedNativeProcessor for PreparedOverdrive {
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        self.supports(id)
    }
    fn parameter_automation_descriptor(&self, id: ParameterId) -> Option<ParameterDescriptor> {
        crate::parameter_binding::binding(&BINDINGS, id)
            .ok()
            .filter(|_| self.supports(id))
            .map(|b| b.descriptor)
    }
    fn parameter_modulation_semantics(
        &self,
        id: ParameterId,
    ) -> Option<hex_lattice_contract::ParameterSemantics> {
        crate::parameter_binding::binding(&BINDINGS, id)
            .ok()
            .filter(|_| self.supports(id))
            .map(|binding| binding.semantics)
    }

    // Keep the exact 11-frame dry delay and ordinary wet processing (including
    // automation, noise/filter/envelope state and finite-tail guards) warm.
    // The host owns the bounded dry history and transactional bypass ramp.
    fn supports_latency_aligned_bypass(&self) -> bool {
        true
    }

    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<OverdriveRuntime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(OverdriveRuntime {
            base_targets: self.targets,
            controls: crate::parameter_binding::Controls::new(self.settings, self.admitted),
            targets: self.targets,
            current: self.targets,
            step: [0.0; 6],
            remaining: 0,
            rate: self.rate,
            channels: [Channel::default(); 2],
            envelope: [0.0; 2],
            attack: (-1.0 / (0.001 * f64::from(self.rate))).exp(),
            release: (-1.0 / (0.080 * f64::from(self.rate))).exp(),
            kernel: self.kernel,
            delay: [[0.0; 3]; LATENCY_FRAMES],
            delay_index: 0,
            silent_frames: 0,
        })
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(LATENCY_FRAMES as u32)
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite {
            max_tail_frames: u64::from(self.rate) * 4,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Channel {
    high: [f64; 2],
    low: [f64; 2],
    oversampler: Oversampler,
    tone: f64,
}
fn quiet(value: f64) -> f64 {
    if value.abs() < FLOOR { 0.0 } else { value }
}
fn lowpass(input: f64, state: &mut f64, pole: f64) -> f64 {
    *state = quiet(input + pole * (*state - input));
    *state
}
impl Channel {
    fn band(&mut self, input: f64, hp: f64, lp: f64) -> f64 {
        let mut sample = input;
        for state in &mut self.high {
            sample -= lowpass(sample, state, hp);
        }
        for state in &mut self.low {
            sample = lowpass(sample, state, lp);
        }
        sample
    }
    fn distort(&mut self, input: f64, drive: f64, kernel: &Kernel, tone: f64) -> f64 {
        let sample = self.oversampler.process(input, drive, kernel, &Tanh);
        lowpass(sample, &mut self.tone, tone)
    }
}

struct OverdriveRuntime {
    base_targets: [f64; 6],
    controls: crate::parameter_binding::Controls<Settings, 6>,
    targets: [f64; 6],
    current: [f64; 6],
    step: [f64; 6],
    remaining: u32,
    rate: u32,
    channels: [Channel; 2],
    envelope: [f64; 2],
    attack: f64,
    release: f64,
    kernel: Kernel,
    /// Dry left/right and input band peak, delayed by the oversampler latency.
    delay: [[f64; 3]; LATENCY_FRAMES],
    delay_index: usize,
    silent_frames: u32,
}
impl OverdriveRuntime {
    fn clear_history(&mut self) {
        self.channels = [Channel::default(); 2];
        self.envelope = [0.0; 2];
        self.delay = [[0.0; 3]; LATENCY_FRAMES];
        self.delay_index = 0;
    }
    fn frame(&mut self, frame: &mut StereoFrame) {
        if self.remaining > 0 {
            self.remaining -= 1;
            for i in 0..6 {
                self.current[i] = if self.remaining == 0 {
                    self.targets[i]
                } else {
                    self.current[i] + self.step[i]
                };
            }
        }
        // Reject poisoned input before it can enter recursive histories.
        if !frame.iter().all(|s| s.is_finite()) {
            *frame = [0.0; 2];
            return;
        }
        if *frame == [0.0; 2] {
            self.silent_frames = self.silent_frames.saturating_add(1).min(self.rate * 4);
            // Explicit finite-tail guarantee, including all envelope/interpolator history.
            if self.silent_frames == self.rate * 4 {
                self.clear_history();
                return;
            }
        } else {
            self.silent_frames = 0;
        }
        let [hp, lp, drive, tone, dynamics, mix] = self.current;
        let input = frame.map(f64::from);
        let mut wet = [0.0; 2];
        let mut input_peak = 0.0_f64;
        for (i, channel) in self.channels.iter_mut().enumerate() {
            let band = channel.band(input[i], hp, lp);
            input_peak = input_peak.max(band.abs());
            wet[i] = channel.distort(band, drive, &self.kernel, tone);
        }
        // Align the dry path and the envelope's input peak with the wet path.
        let [dry_left, dry_right, band_peak] = std::mem::replace(
            &mut self.delay[self.delay_index],
            [input[0], input[1], input_peak],
        );
        self.delay_index = (self.delay_index + 1) % LATENCY_FRAMES;
        let dry = [dry_left, dry_right];
        let wet_peak = wet[0].abs().max(wet[1].abs());
        for (envelope, peak) in self.envelope.iter_mut().zip([band_peak, wet_peak]) {
            let p = if peak > *envelope {
                self.attack
            } else {
                self.release
            };
            *envelope = quiet(peak + p * (*envelope - peak));
        }
        let restore = if self.envelope[1] > FLOOR {
            (self.envelope[0] / self.envelope[1]).clamp(0.0, 4.0)
        } else {
            1.0
        };
        let gain = 1.0 + dynamics * (restore - 1.0);
        // Keep the zero-mix endpoint bit-exact (delayed by the declared
        // latency) while still advancing histories.
        for i in 0..2 {
            frame[i] = if mix == 0.0 {
                dry[i] as f32
            } else {
                (dry[i] * (1.0 - mix) + wet[i] * gain * mix) as f32
            };
        }
    }
}
impl NativeProcessorRuntime for OverdriveRuntime {
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        self.controls.supports(&BINDINGS, id)
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
            self.frame(frame);
        }
    }
    fn reset(&mut self) {
        self.controls.reset();
        self.targets = self.base_targets;
        self.clear_history();
        self.current = self.targets;
        self.step = [0.0; 6];
        self.remaining = 0;
        self.silent_frames = 0;
    }
    fn is_quiescent(&self) -> bool {
        self.channels.iter().all(|c| {
            c.high == [0.0; 2] && c.low == [0.0; 2] && c.oversampler.is_quiescent() && c.tone == 0.0
        }) && self.envelope == [0.0; 2]
            && self.delay.iter().flatten().all(|v| *v == 0.0)
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|p| p.rate == self.rate)
    }
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        let Some(previous) = previous
            .as_any()
            .downcast_ref::<Self>()
            .filter(|p| p.rate == self.rate)
        else {
            return;
        };
        self.channels = previous.channels;
        self.envelope = previous.envelope;
        self.delay = previous.delay;
        self.delay_index = previous.delay_index;
        self.silent_frames = previous.silent_frames;
        if BINDINGS
            .iter()
            .any(|b| self.controls.supports(&BINDINGS, b.descriptor.id))
            && BINDINGS
                .iter()
                .any(|b| previous.controls.supports(&BINDINGS, b.descriptor.id))
        {
            self.controls.inherit(&previous.controls);
            self.targets = previous.targets;
        }
        self.current = previous.current;
        self.step = previous.step;
        self.remaining = previous.remaining;
        if self.targets != previous.targets {
            self.remaining = (self.rate / 200).max(1);
            for i in 0..6 {
                self.step[i] = (self.targets[i] - self.current[i]) / f64::from(self.remaining);
            }
        }
    }
    fn can_retarget_parameters_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        self.can_copy_history_from(previous)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Static pre-distortion band response. X is logarithmic 20–20,000 hz; y is
/// -48–0 db. This excludes drive, tone, mix and running envelope history.
#[derive(Clone, Debug, PartialEq)]
pub struct OverdriveFilterResponse {
    pub points: [[f32; 2]; 129],
}
/// Bounded control-side query using the same rate-clamped poles as the runtime.
pub fn filter_response(
    state: &NativeDeviceState,
    sample_rate: u32,
) -> Result<OverdriveFilterResponse, DeviceContractError> {
    validate_rate(sample_rate)?;
    let [hp, lp] = band_poles(decode(state)?, sample_rate);
    let points = std::array::from_fn(|i| {
        let x = i as f64 / 128.0;
        let hz = 20.0 * 1000.0_f64.powf(x);
        if hz >= f64::from(sample_rate) * 0.5 {
            return [x as f32, 0.0];
        }
        let cosine = (std::f64::consts::TAU * hz / f64::from(sample_rate)).cos();
        let high_squared = 2.0 * hp * hp * (1.0 - cosine) / (1.0 + hp * hp - 2.0 * hp * cosine);
        let low_squared = (1.0 - lp).powi(2) / (1.0 + lp * lp - 2.0 * lp * cosine);
        // Two cascaded poles per side: magnitude = squared single-pole response.
        let db = 20.0 * (high_squared * low_squared).max(1.0e-24).log10();
        [x as f32, ((db + 48.0) / 48.0).clamp(0.0, 1.0) as f32]
    });
    Ok(OverdriveFilterResponse { points })
}

#[cfg(test)]
mod tests;
