//! Bounded FDN history; preparation owns allocation and canonical coefficients.
//! Authored output/filter coefficients use bounded cached conversion in automation.
pub(super) mod automation;
use super::{PARAMETER_COUNT, Settings, parameter as p};
use automation::{AutomatedSettings, AutomatedValues};
use hex_lattice::{
    DeviceResources, NativeInstrumentClock, NativeParameterAutomation, OutputTail,
    OwnedProcessorRuntime, PreparedOwnedProcessorRuntime, StereoFrame,
};
use hex_lattice_contract::{DeviceContractError, ParameterDescriptor, ParameterId};
use std::any::Any;
const TIMES: [f32; 8] = [
    0.0233, 0.0297, 0.0311, 0.0371, 0.0411, 0.0437, 0.0479, 0.0539,
];
const TAPS: [f32; 8] = [
    0.0043, 0.0117, 0.0193, 0.0271, 0.0379, 0.0499, 0.0611, 0.0737,
];
// Precomputed (sin, cos) for early-reflection spin tap offsets `TAU * i * 0.137`.
const SPIN_TAP_SIN_COS: [(f32, f32); 8] = [
    (0.0, 1.0),
    (0.7583619, 0.6518337),
    (0.9886518, -0.1502256),
    (0.5305112, -0.8476779),
    (-0.2970416, -0.9548646),
    (-0.9177547, -0.3971479),
    (-0.8994052, 0.4371158),
    (-0.2547707, 0.9670015),
];
// Precomputed (sin, cos) for late-tank chorus tap offsets `TAU * i * 0.173`.
const CHORUS_TAP_SIN_COS: [(f32, f32); 8] = [
    (0.0, 1.0),
    (0.8852313, 0.4651511),
    (0.8235326, -0.567269),
    (-0.1190972, -0.9928826),
    (-0.9343289, -0.3564119),
    (-0.750111, 0.6613119),
    (0.236499, 0.9716317),
    (0.9701266, 0.2425992),
];
const COEFFICIENTS: usize = 32;
type FeedbackGains = (f32, f32, f32, f32);
type Rotations = [Option<(f32, f32)>; 3];

// Authored diffusion/density reuse the passive scatter law. A partial override
// keys on the other component's current canonical ramp, not its final target.
#[derive(Clone, Copy, Default)]
struct ScatterCache {
    parameters: Option<(f32, f32)>,
    rotations: Rotations,
}
impl ScatterCache {
    fn resolve(&mut self, diffusion: f32, density: f32) -> Rotations {
        let parameters = Some((diffusion, density));
        if self.parameters != parameters {
            self.rotations = scatter_rotations(diffusion * 0.01, density + 1.0);
            self.parameters = parameters;
        }
        self.rotations
    }
    fn check_inline(&self) {
        let Self {
            parameters,
            rotations,
        } = self;
        let _: &Option<(f32, f32)> = parameters;
        let _: &Rotations = rotations;
    }
}

// Authored feedback derives from effective parameter ramps at tank rate.
// Canonical-only rendering keeps the established prepared coefficient ramps.
#[derive(Clone, Copy, Default)]
struct FeedbackCache {
    parameters: Option<[f32; 8]>,
    gains: [FeedbackGains; 8],
}
fn blend(a: f32, b: f32, weight: f32) -> f32 {
    if weight <= 0.0 {
        a
    } else if weight >= 1.0 {
        b
    } else {
        a + (b - a) * weight
    }
}
#[inline]
fn decay_gain(log2_rate: f32, time: f32) -> f32 {
    (log2_rate * time).exp2()
}
#[inline]
pub(super) fn db_to_linear(db: f32) -> f32 {
    hex_dsp::Decibels::new(f64::from(db))
        .and_then(hex_dsp::Decibels::linear_amplitude)
        .map_or(0.0, |amplitude| amplitude.value() as f32)
}
impl FeedbackCache {
    fn resolve(&mut self, parameters: [f32; 8]) -> &[FeedbackGains; 8] {
        if self.parameters != Some(parameters) {
            let [decay, size, high_ratio, low_ratio, freeze, flat, low, high] = parameters;
            let base_rate = -3.0 * std::f32::consts::LOG2_10 * size / decay;
            let unflat = 1.0 - freeze * flat;
            let low_weight = low * unflat;
            let high_weight = high * unflat;
            let low_rate = if low_weight > 0.0 {
                base_rate / low_ratio
            } else {
                0.0
            };
            let high_rate = if high_weight > 0.0 {
                base_rate / high_ratio
            } else {
                0.0
            };
            self.gains = std::array::from_fn(|i| {
                let time = TIMES[i];
                let base = decay_gain(base_rate, time);
                let mid = blend(base, 1.0, freeze);
                let shaped_low = if low_weight <= 0.0 {
                    mid
                } else {
                    blend(mid, decay_gain(low_rate, time), low_weight)
                };
                let shaped_high = if high_weight <= 0.0 {
                    mid
                } else {
                    blend(mid, decay_gain(high_rate, time), high_weight)
                };
                shaping_gains(shaped_low, mid, shaped_high)
            });
            self.parameters = Some(parameters);
        }
        &self.gains
    }
    fn check_inline(&self) {
        let Self { parameters, gains } = self;
        let _: &Option<[f32; 8]> = parameters;
        let _: &[(f32, f32, f32, f32); 8] = gains;
    }
}
// Validated none/fast/slow choice; each supported rate gives a nonzero duration.
fn smoothing_frames(rate: u32, mode: u32) -> u32 {
    match mode {
        0 => 1,
        2 => (rate / 10).max(1),
        _ => (rate / 50).max(1),
    }
}
// Inputs are validated: decay0.1..60s and rate8..192kHz. The largest
// product is34,752,000frames, safely representable before this cast.
fn silence_deadline_frames(rate: u32, decay: f32) -> u64 {
    ((f64::from(decay) * 3.0 + 1.0).max(8.0) * f64::from(rate)).ceil() as u64
}

#[derive(Clone, Copy)]
struct TankDesign {
    filters: [InputFilter; 2],
    feedback: [FeedbackGains; 8],
    rotations: Rotations,
}

impl TankDesign {
    fn new(settings: Settings, coefficients: [f32; COEFFICIENTS]) -> Self {
        Self {
            filters: [
                InputFilter::new(coefficients[24], coefficients[30]),
                InputFilter::new(coefficients[25], coefficients[31]),
            ],
            feedback: std::array::from_fn(|index| {
                let start = index * 3;
                shaping_gains(
                    coefficients[start],
                    coefficients[start + 1],
                    coefficients[start + 2],
                )
            }),
            rotations: scatter_rotations(
                settings.get(p::DIFFUSION) * 0.01,
                settings.get(p::DENSITY) + 1.0,
            ),
        }
    }
}

#[derive(Clone, Copy)]
pub struct PreparedReverb {
    rate: u32,
    factor: u32,
    tank_rate: f32,
    settings: Settings,
    coefficients: [f32; COEFFICIENTS],
    settled_tank: TankDesign,
    input_capacity: usize,
    capacities: [usize; 8],
    allpass_capacities: [usize; 2],
    silence_limit: u64,
    decay_admitted: bool,
    freeze_admitted: bool,
    smoothing_admitted: bool,
}
// Topology-preserving state-variable section. Positive g and damping remain
// stable throughout bounded parameter ramps; no arbitrary biquad interpolation.
#[derive(Clone, Copy)]
struct InputFilter(hex_dsp::SvfSectionF32);
impl InputFilter {
    fn new(g: f32, damping: f32) -> Self {
        Self(hex_dsp::SvfSectionF32::from_validated(g, damping))
    }
    fn apply(self, state: &mut [f32; 2], x: f32, high: bool) -> f32 {
        self.0.apply_bounded(state, x, high, 32.0)
    }
}
fn alpha(hz: f32, rate: f32) -> f32 {
    hex_dsp::frequency_pole(f64::from(hz.min(rate * 0.45)), f64::from(rate)).unwrap_or(0.0) as f32
}
impl PreparedReverb {
    pub(super) fn new(rate: u32, settings: Settings) -> Result<Self, DeviceContractError> {
        if !(8_000..=192_000).contains(&rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let factor = rate.div_ceil(48_000);
        let tank_rate = rate as f32 / factor as f32;
        let size = (settings.get(p::SIZE) / 100.0).sqrt();
        let frozen = settings.on(p::FREEZE);
        let flat = frozen && settings.on(p::FLAT);
        let mut coefficients = [0.0; COEFFICIENTS];
        let base_rate = -3.0 * std::f32::consts::LOG2_10 * size / settings.get(p::DECAY);
        let low_active = settings.on(p::LOW) && !flat;
        let high_active = settings.on(p::HIGH) && !flat;
        let low_rate = if low_active {
            base_rate / settings.get(p::LOW_RATIO)
        } else {
            0.0
        };
        let high_rate = if high_active {
            base_rate / settings.get(p::HIGH_RATIO)
        } else {
            0.0
        };
        for (i, seconds) in TIMES.into_iter().enumerate() {
            let base = decay_gain(base_rate, seconds);
            let mid = if frozen { 1.0 } else { base };
            coefficients[i * 3] = if low_active {
                decay_gain(low_rate, seconds)
            } else {
                mid
            };
            coefficients[i * 3 + 1] = mid;
            coefficients[i * 3 + 2] = if high_active {
                decay_gain(high_rate, seconds)
            } else {
                mid
            };
        }
        for (slot, id) in [(26, p::LOW_FREQ), (27, p::HIGH_FREQ)] {
            coefficients[slot] = alpha(settings.get(id), tank_rate);
        }
        coefficients[24] = (std::f32::consts::PI * settings.get(p::LO_FREQ).min(tank_rate * 0.45)
            / tank_rate)
            .tan();
        coefficients[25] = (std::f32::consts::PI * settings.get(p::HI_FREQ).min(tank_rate * 0.45)
            / tank_rate)
            .tan();
        coefficients[30] = 1.0 / settings.get(p::LO_Q);
        coefficients[31] = 1.0 / settings.get(p::HI_Q);
        coefficients[28] = db_to_linear(settings.get(p::REFLECT));
        coefficients[29] = db_to_linear(settings.get(p::DIFFUSE));
        let capacities =
            TIMES.map(|t| ((t * std::f32::consts::SQRT_2 + 0.004) * tank_rate).ceil() as usize + 4);
        let allpass_capacities =
            [0.005_f32, 0.0017].map(|t| (t * 2.0 * tank_rate).ceil() as usize + 4);
        // Each ring advances once per tank tick. The input rings must finish
        // their first wrap last so they can prove the whole bank is warm.
        let input_capacity = capacities
            .iter()
            .chain(&allpass_capacities)
            .copied()
            .fold((0.4 * tank_rate).ceil() as usize + 4, usize::max);
        Ok(Self {
            rate,
            factor,
            tank_rate,
            settings,
            coefficients,
            settled_tank: TankDesign::new(settings, coefficients),
            input_capacity,
            capacities,
            allpass_capacities,
            silence_limit: silence_deadline_frames(rate, settings.get(p::DECAY)),
            decay_admitted: false,
            freeze_admitted: false,
            smoothing_admitted: false,
        })
    }
    pub(super) fn with_automation(mut self, parameters: &[ParameterId]) -> Self {
        self.decay_admitted = parameters.contains(&p::DECAY);
        self.freeze_admitted = parameters.contains(&p::FREEZE);
        self.smoothing_admitted = parameters.contains(&p::SMOOTH);
        self
    }
    fn runtime(&self) -> Result<ReverbRuntime, DeviceContractError> {
        let initial = ReverbState {
            automation_controls: crate::parameter_binding::Controls::new(
                AutomatedSettings::new(self.settings),
                std::array::from_fn(|i| {
                    (i != 0 || self.decay_admitted)
                        && (i != automation::FREEZE_INDEX || self.freeze_admitted)
                        && (i != automation::SMOOTH_INDEX || self.smoothing_admitted)
                }),
            ),
            automated_values: AutomatedValues::default(),
            scatter_cache: ScatterCache::default(),
            feedback_cache: FeedbackCache::default(),
            parameters: self.settings.0.map(Ramp::settled),
            coefficients: self.coefficients.map(Ramp::settled),
            tank_ramp_remaining: 0,
            silence_limit: self.silence_limit,
            silent_frames: 0,
            active: false,
            input_low: [[0.0; 2]; 2],
            input_high: [[0.0; 2]; 2],
            feedback_low: [0.0; 8],
            feedback_high: [0.0; 8],
            phase: 0,
            sum: [0.0; 2],
            previous: [0.0; 2],
            next: [0.0; 2],
            spin_phase: 0.0,
            chorus_phase: 0.0,
        };
        let inputs = [
            RingPair::try_new(self.input_capacity)?,
            RingPair::try_new(self.input_capacity)?,
        ];
        let lines = [
            RingPair::try_new(self.capacities[0])?,
            RingPair::try_new(self.capacities[1])?,
            RingPair::try_new(self.capacities[2])?,
            RingPair::try_new(self.capacities[3])?,
            RingPair::try_new(self.capacities[4])?,
            RingPair::try_new(self.capacities[5])?,
            RingPair::try_new(self.capacities[6])?,
            RingPair::try_new(self.capacities[7])?,
        ];
        let allpasses = [
            [
                RingPair::try_new(self.allpass_capacities[0])?,
                RingPair::try_new(self.allpass_capacities[1])?,
            ],
            [
                RingPair::try_new(self.allpass_capacities[0])?,
                RingPair::try_new(self.allpass_capacities[1])?,
            ],
        ];
        Ok(ReverbRuntime {
            rate: self.rate,
            factor: self.factor,
            tank_rate: self.tank_rate,
            settled_tank: self.settled_tank,
            settings: self.settings,
            primary: initial,
            speculative: initial,
            tail_controls_frozen: false,
            open: false,
            inputs,
            lines,
            allpasses,
        })
    }
}
impl PreparedOwnedProcessorRuntime for PreparedReverb {
    fn parameter_automation_descriptor(
        &self,
        parameter: ParameterId,
    ) -> Option<ParameterDescriptor> {
        if !self.supports_parameter_automation(parameter) {
            return None;
        }
        super::DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.id == parameter)
            .copied()
    }
    fn supports_parameter_automation(&self, parameter: ParameterId) -> bool {
        automation::supports(parameter)
            && (parameter != p::DECAY || self.decay_admitted)
            && (parameter != p::FREEZE || self.freeze_admitted)
            && (parameter != p::SMOOTH || self.smoothing_admitted)
    }

    fn parameter_modulation_semantics(
        &self,
        parameter: ParameterId,
    ) -> Option<hex_lattice_contract::ParameterSemantics> {
        automation::semantics(parameter).filter(|semantics| {
            self.supports_parameter_automation(parameter)
                && semantics.modulation == hex_lattice_contract::ParameterModulation::SampleRate
        })
    }

    fn supports_smoothed_bypass(&self) -> bool {
        // Zero latency and history advancement under bypass permit the host's
        // reversible 5 ms blend without a second room or retained-tail owner.
        true
    }

    fn supports_power_off(&self) -> bool {
        true
    }

    fn runtime_version(&self) -> u32 {
        4
    }
    fn declared_pair_bytes(&self) -> usize {
        std::mem::size_of::<ReverbRuntime>()
            + 2 * 4
                * (2 * self.input_capacity
                    + self.capacities.iter().sum::<usize>()
                    + 2 * self.allpass_capacities.iter().sum::<usize>())
    }
    fn create_runtime(&self) -> Result<Box<dyn OwnedProcessorRuntime>, DeviceContractError> {
        let runtime = self.runtime()?;
        if runtime.fixed_storage_pair_bytes() != Some(self.declared_pair_bytes()) {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }
        Ok(Box::new(runtime))
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn output_tail(&self) -> OutputTail {
        if self.settings.on(p::FREEZE) || self.freeze_admitted {
            OutputTail::Unbounded
        } else {
            OutputTail::Finite {
                // A copied freeze-off ramp may retain up to 100ms before its
                // silence clock starts. Include that transition in the promise.
                max_tail_frames: if self.decay_admitted {
                    silence_deadline_frames(self.rate, 60.0)
                } else {
                    self.silence_limit
                } + u64::from(self.rate).div_ceil(10),
            }
        }
    }
}

mod processor;
mod ring;
#[cfg(test)]
use processor::scatter;
pub use processor::{RUNTIME_HEAP_ALLOCATIONS, ReverbRuntime, ReverbRuntimeStorage};
use processor::{Ramp, ReverbState, scatter_rotations, shaping_gains};
use ring::{RingPair, clean};

#[cfg(test)]
mod tests;
