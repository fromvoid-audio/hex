//! Bounded FDN history; preparation owns allocation and canonical coefficients.
//! Authored output/filter coefficients use bounded cached conversion in automation.
pub(super) mod automation;
use super::{PARAMETER_COUNT, Settings, parameter as p};
use automation::{AutomatedSettings, AutomatedValues};
use hex_lattice::{
    DeviceResources, NativeInstrumentClock, NativeParameterAutomation, NativeProcessorRuntime,
    OutputTail, PreparedNativeProcessor, StereoFrame,
};
use hex_lattice_contract::{DeviceContractError, ParameterDescriptor, ParameterId};
use std::any::Any;
const TIMES: [f32; 8] = [
    0.0233, 0.0297, 0.0311, 0.0371, 0.0411, 0.0437, 0.0479, 0.0539,
];
const TAPS: [f32; 8] = [
    0.0043, 0.0117, 0.0193, 0.0271, 0.0379, 0.0499, 0.0611, 0.0737,
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
impl FeedbackCache {
    fn resolve(&mut self, parameters: [f32; 8]) -> &[FeedbackGains; 8] {
        if self.parameters != Some(parameters) {
            let [decay, size, high_ratio, low_ratio, freeze, flat, low, high] = parameters;
            self.gains = std::array::from_fn(|i| {
                let base = 10.0_f32.powf(-3.0 * TIMES[i] * size / decay);
                let mid = blend(base, 1.0, freeze);
                let shaped_low =
                    blend(mid, base.powf(1.0 / low_ratio), low * (1.0 - freeze * flat));
                let shaped_high = blend(
                    mid,
                    base.powf(1.0 / high_ratio),
                    high * (1.0 - freeze * flat),
                );
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
struct InputFilter {
    a1: f32,
    a2: f32,
    a3: f32,
    damping: f32,
}
impl InputFilter {
    fn new(g: f32, damping: f32) -> Self {
        let a1 = 1.0 / (1.0 + g * (g + damping));
        let a2 = g * a1;
        Self {
            a1,
            a2,
            a3: g * a2,
            damping,
        }
    }
    fn apply(self, state: &mut [f32; 2], x: f32, high: bool) -> f32 {
        let v3 = x - state[1];
        let v1 = self.a1 * state[0] + self.a2 * v3;
        let v2 = state[1] + self.a2 * state[0] + self.a3 * v3;
        state[0] = clean(2.0 * v1 - state[0]);
        state[1] = clean(2.0 * v2 - state[1]);
        clean(if high { x - self.damping * v1 - v2 } else { v2 })
    }
}
fn alpha(hz: f32, rate: f32) -> f32 {
    (-std::f32::consts::TAU * hz.min(rate * 0.45) / rate).exp()
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
        for (i, seconds) in TIMES.into_iter().enumerate() {
            let base = 10.0_f32.powf(-3.0 * seconds * size / settings.get(p::DECAY));
            let mid = if frozen { 1.0 } else { base };
            coefficients[i * 3] = if settings.on(p::LOW) && !flat {
                base.powf(1.0 / settings.get(p::LOW_RATIO))
            } else {
                mid
            };
            coefficients[i * 3 + 1] = mid;
            coefficients[i * 3 + 2] = if settings.on(p::HIGH) && !flat {
                base.powf(1.0 / settings.get(p::HIGH_RATIO))
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
        coefficients[28] = 10.0_f32.powf(settings.get(p::REFLECT) / 20.0);
        coefficients[29] = 10.0_f32.powf(settings.get(p::DIFFUSE) / 20.0);
        Ok(Self {
            rate,
            factor,
            tank_rate,
            settings,
            coefficients,
            settled_tank: TankDesign::new(settings, coefficients),
            input_capacity: (0.4 * tank_rate).ceil() as usize + 4,
            capacities: TIMES
                .map(|t| ((t * std::f32::consts::SQRT_2 + 0.004) * tank_rate).ceil() as usize + 4),
            allpass_capacities: [0.005_f32, 0.0017]
                .map(|t| (t * 2.0 * tank_rate).ceil() as usize + 4),
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
    fn runtime(&self) -> ReverbRuntime {
        ReverbRuntime {
            automation_controls: crate::parameter_binding::Controls::new(
                AutomatedSettings::new(self.settings),
                std::array::from_fn(|i| {
                    (i != 0 || self.decay_admitted)
                        && (i != automation::FREEZE_INDEX || self.freeze_admitted)
                        && (i != automation::SMOOTH_INDEX || self.smoothing_admitted)
                }),
            ),
            automated_values: AutomatedValues::default(),
            tail_controls_frozen: false,
            scatter_cache: ScatterCache::default(),
            feedback_cache: FeedbackCache::default(),
            rate: self.rate,
            factor: self.factor,
            tank_rate: self.tank_rate,
            inputs: std::array::from_fn(|_| Ring::new(self.input_capacity)),
            lines: std::array::from_fn(|i| Ring::new(self.capacities[i])),
            allpasses: std::array::from_fn(|_| {
                std::array::from_fn(|i| Ring::new(self.allpass_capacities[i]))
            }),
            parameters: self.settings.0.map(Ramp::steady),
            coefficients: self.coefficients.map(Ramp::steady),
            settled_tank: self.settled_tank,
            tank_ramp_remaining: 0,
            settings: self.settings,
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
        }
    }
}
impl PreparedNativeProcessor for PreparedReverb {
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

    fn runtime_version(&self) -> u32 {
        3
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<ReverbRuntime>()
            + 4 * (2 * self.input_capacity
                + self.capacities.iter().sum::<usize>()
                + 2 * self.allpass_capacities.iter().sum::<usize>())
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(self.runtime())
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
struct Ring {
    samples: Box<[f32]>,
    cursor: usize,
}
impl Ring {
    fn new(capacity: usize) -> Self {
        Self {
            samples: vec![0.0; capacity].into_boxed_slice(),
            cursor: 0,
        }
    }
    fn read(&self, delay: f32) -> f32 {
        let delay = delay.clamp(1.0, (self.samples.len() - 2) as f32);
        let whole = delay.floor() as usize;
        let fraction = delay - whole as f32;
        let length = self.samples.len();
        let unwrapped = self.cursor + length - whole;
        // The delay clamp and the ring cursor bound permit a single wrap.
        let a = if unwrapped >= length {
            unwrapped - length
        } else {
            unwrapped
        };
        let b = if a == 0 {
            self.samples.len() - 1
        } else {
            a - 1
        };
        self.samples[a] + fraction * (self.samples[b] - self.samples[a])
    }
    fn push(&mut self, value: f32) {
        self.samples[self.cursor] = clean(value);
        self.cursor += 1;
        if self.cursor == self.samples.len() {
            self.cursor = 0;
        }
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
fn clean(value: f32) -> f32 {
    if value.is_nan() || value.abs() < 1e-20 {
        0.0
    } else {
        value.clamp(-32.0, 32.0)
    }
}
#[derive(Clone, Copy)]
struct Ramp {
    current: f32,
    target: f32,
    step: f32,
    remaining: u32,
}
impl Ramp {
    fn steady(value: f32) -> Self {
        Self {
            current: value,
            target: value,
            step: 0.0,
            remaining: 0,
        }
    }
    fn retarget(&mut self, target: f32, frames: u32) {
        self.target = target;
        self.step = (target - self.current) / frames as f32;
        self.remaining = frames;
    }
    fn advance(&mut self) {
        if self.remaining > 0 {
            self.remaining -= 1;
            self.current = if self.remaining == 0 {
                self.target
            } else if self.step >= 0.0 {
                (self.current + self.step).min(self.target)
            } else {
                (self.current + self.step).max(self.target)
            };
        }
    }
    fn inherit(&mut self, previous: Self, frames: u32) {
        let target = self.target;
        *self = previous;
        if target != previous.target {
            self.retarget(target, frames);
        }
    }
}
/// Original exclusive requested allocations. This scalar observation is not a
/// custody receipt and never adopts arbitrary provider-owned storage.
#[derive(Clone, Copy)]
pub struct ReverbRuntimeStorage {
    pub inline_bytes: usize,
    pub heap_bytes: [usize; RUNTIME_HEAP_ALLOCATIONS],
}
pub const RUNTIME_HEAP_ALLOCATIONS: usize = 14;
pub struct ReverbRuntime {
    automation_controls: crate::parameter_binding::Controls<AutomatedSettings, 33>,
    automated_values: AutomatedValues,
    tail_controls_frozen: bool,
    scatter_cache: ScatterCache,
    feedback_cache: FeedbackCache,
    rate: u32,
    factor: u32,
    tank_rate: f32,
    inputs: [Ring; 2],
    lines: [Ring; 8],
    allpasses: [[Ring; 2]; 2],
    parameters: [Ramp; PARAMETER_COUNT],
    coefficients: [Ramp; COEFFICIENTS],
    settled_tank: TankDesign,
    tank_ramp_remaining: u32,
    settings: Settings,
    silence_limit: u64,
    silent_frames: u64,
    active: bool,
    input_low: [StereoFrame; 2],
    input_high: [StereoFrame; 2],
    feedback_low: [f32; 8],
    feedback_high: [f32; 8],
    phase: u32,
    sum: StereoFrame,
    previous: StereoFrame,
    next: StereoFrame,
    spin_phase: f32,
    chorus_phase: f32,
}
impl ReverbRuntime {
    /// Original exclusive runtime leaf, including every requested Box payload.
    /// A borrowed declaration never transfers native custody. The closed host
    /// constructor must establish origin before using this bounded observation.
    /// Exhaustive destructuring forces an owning-field audit when this changes.
    pub fn control_storage(&self) -> Option<ReverbRuntimeStorage> {
        let Self {
            automation_controls,
            automated_values,
            tail_controls_frozen,
            scatter_cache,
            feedback_cache,
            rate,
            factor,
            tank_rate,
            inputs,
            lines,
            allpasses,
            parameters,
            coefficients,
            settled_tank,
            tank_ramp_remaining,
            settings,
            silence_limit,
            silent_frames,
            active,
            input_low,
            input_high,
            feedback_low,
            feedback_high,
            phase,
            sum,
            previous,
            next,
            spin_phase,
            chorus_phase,
        } = self;
        fn inline<T: Copy>(_: &T) {}
        automation_controls.check_inline(AutomatedSettings::check_inline);
        automated_values.check_inline();
        inline(tail_controls_frozen);
        scatter_cache.check_inline();
        feedback_cache.check_inline();
        inline(rate);
        inline(factor);
        inline(tank_rate);
        inline(parameters);
        inline(coefficients);
        inline(settled_tank);
        inline(tank_ramp_remaining);
        inline(settings);
        inline(silence_limit);
        inline(silent_frames);
        inline(active);
        inline(input_low);
        inline(input_high);
        inline(feedback_low);
        inline(feedback_high);
        inline(phase);
        inline(sum);
        inline(previous);
        inline(next);
        inline(spin_phase);
        inline(chorus_phase);
        let mut heap_bytes = [0; RUNTIME_HEAP_ALLOCATIONS];
        let mut observed = 0;
        for (index, ring) in inputs
            .iter()
            .chain(lines.iter())
            .chain(allpasses.iter().flatten())
            .enumerate()
        {
            let Ring { samples, cursor } = ring;
            inline(cursor);
            *heap_bytes.get_mut(index)? = samples.len().checked_mul(std::mem::size_of::<f32>())?;
            observed += 1;
        }
        if observed != RUNTIME_HEAP_ALLOCATIONS {
            return None;
        }
        Some(ReverbRuntimeStorage {
            inline_bytes: std::mem::size_of::<Self>(),
            heap_bytes,
        })
    }
    pub fn control_storage_bytes(&self) -> Option<usize> {
        let storage = self.control_storage()?;
        storage
            .heap_bytes
            .into_iter()
            .try_fold(storage.inline_bytes, usize::checked_add)
    }
    fn retime_canonical_ramps(&mut self, mode: u32) {
        if self.tank_ramp_remaining == 0 {
            return;
        }
        let frames = smoothing_frames(self.rate, mode);
        let mut remaining = 0;
        for ramp in self
            .parameters
            .iter_mut()
            .chain(self.coefficients.iter_mut())
        {
            if ramp.remaining != 0 {
                ramp.retarget(ramp.target, frames);
                remaining = frames;
            }
        }
        self.tank_ramp_remaining = remaining;
    }
    fn v(&self, id: hex_lattice_contract::ParameterId) -> f32 {
        self.parameters[(id.value() - 1) as usize].current
    }
    fn clear(&mut self) {
        for ring in &mut self.inputs {
            ring.clear();
        }
        for ring in &mut self.lines {
            ring.clear();
        }
        for ring in self.allpasses.iter_mut().flatten() {
            ring.clear();
        }
        self.input_low = [[0.0; 2]; 2];
        self.input_high = [[0.0; 2]; 2];
        self.feedback_low = [0.0; 8];
        self.feedback_high = [0.0; 8];
        self.phase = 0;
        self.sum = [0.0; 2];
        self.previous = [0.0; 2];
        self.next = [0.0; 2];
        self.spin_phase = 0.0;
        self.chorus_phase = 0.0;
        self.silent_frames = 0;
        self.active = false;
    }
    fn tank(&mut self, input: StereoFrame, output: AutomatedValues) {
        let freeze = output.freeze.unwrap_or_else(|| self.v(p::FREEZE));
        let flat = output.flat.unwrap_or_else(|| self.v(p::FLAT));
        let cut = output.cut.unwrap_or_else(|| self.v(p::CUT));
        let predelay = output.predelay.unwrap_or_else(|| self.v(p::PREDELAY));
        let spin_amount = output.spin_amount.unwrap_or_else(|| self.v(p::SPIN_AMOUNT));
        let spin_rate = output.spin_rate.unwrap_or_else(|| self.v(p::SPIN_RATE));
        let shape = output.shape.unwrap_or_else(|| self.v(p::SHAPE));
        let scale = output.scale.unwrap_or_else(|| self.v(p::SCALE));
        let chorus_amount = output
            .chorus_amount
            .unwrap_or_else(|| self.v(p::CHORUS_AMOUNT));
        let chorus_rate = output.chorus_rate.unwrap_or_else(|| self.v(p::CHORUS_RATE));
        let diffusion = output.diffusion.unwrap_or_else(|| self.v(p::DIFFUSION));
        let density = output.density.unwrap_or_else(|| self.v(p::DENSITY));
        let size = (output.size.unwrap_or_else(|| self.v(p::SIZE)) / 100.0).sqrt();
        let predelay = predelay * 0.001 * self.tank_rate;
        let excitation = 1.0 - freeze * cut;
        let settled = self.tank_ramp_remaining == 0;
        let [high_filter, low_filter] = if settled {
            self.settled_tank.filters
        } else {
            [
                InputFilter::new(self.coefficients[24].current, self.coefficients[30].current),
                InputFilter::new(self.coefficients[25].current, self.coefficients[31].current),
            ]
        };
        let high_filter = if output.lo_g.is_some() || output.lo_damping.is_some() {
            InputFilter::new(
                output.lo_g.unwrap_or(self.coefficients[24].current),
                output.lo_damping.unwrap_or(self.coefficients[30].current),
            )
        } else {
            high_filter
        };
        let low_filter = if output.hi_g.is_some() || output.hi_damping.is_some() {
            InputFilter::new(
                output.hi_g.unwrap_or(self.coefficients[25].current),
                output.hi_damping.unwrap_or(self.coefficients[31].current),
            )
        } else {
            low_filter
        };
        let lo_alpha = output.low_alpha.unwrap_or(self.coefficients[26].current);
        let hi_alpha = output.high_alpha.unwrap_or(self.coefficients[27].current);
        let mut late_input = [0.0; 2];
        let mut early = [0.0; 2];
        for c in 0..2 {
            let mut x = input[c] * excitation;
            let high = high_filter.apply(&mut self.input_low[c], x, true);
            x += (high - x) * output.lo_cut.unwrap_or_else(|| self.v(p::LO_CUT));
            let low = low_filter.apply(&mut self.input_high[c], x, false);
            x += (low - x) * output.hi_cut.unwrap_or_else(|| self.v(p::HI_CUT));
            self.inputs[c].push(x);
            late_input[c] = self.inputs[c].read(predelay + 1.0);
            for (i, seconds) in TAPS.into_iter().enumerate() {
                let phase = self.spin_phase + c as f32 * 0.25 + i as f32 * 0.137;
                let spin = (std::f32::consts::TAU * phase).sin()
                    * output.spin.unwrap_or_else(|| self.v(p::SPIN))
                    * spin_amount
                    * 0.000025
                    * self.tank_rate;
                let delay = predelay
                    + (seconds * size * (0.75 + 0.5 * shape) * self.tank_rate + spin).max(0.0)
                    + 1.0;
                let weight = (1.0 - shape + shape * (i as f32 / 7.0)) * 0.125;
                early[c] +=
                    self.inputs[c].read(delay) * weight * if i % 3 == 2 { -1.0 } else { 1.0 };
            }
            late_input[c] += early[c] * 0.25;
        }
        let depth = output.chorus.unwrap_or_else(|| self.v(p::CHORUS))
            * chorus_amount
            * 0.001
            * self.tank_rate
            * (1.0 - freeze);
        let delayed = std::array::from_fn(|i| {
            let modulation =
                (std::f32::consts::TAU * (self.chorus_phase + i as f32 * 0.173)).sin() * depth;
            let base = TIMES[i] * size * self.tank_rate;
            let delay = base + freeze * (base.round() - base);
            self.lines[i].read(delay + modulation)
        });
        let rotations = if output.diffusion.is_some() || output.density.is_some() {
            self.scatter_cache.resolve(diffusion, density)
        } else if settled {
            self.settled_tank.rotations
        } else {
            scatter_rotations(self.v(p::DIFFUSION) * 0.01, self.v(p::DENSITY) + 1.0)
        };
        let feedback = scatter(delayed, rotations);
        let authored_feedback = if output.decay.is_some()
            || output.size.is_some()
            || output.high_ratio.is_some()
            || output.low_ratio.is_some()
            || output.high.is_some()
            || output.low.is_some()
            || output.freeze.is_some()
            || output.flat.is_some()
        {
            let feedback_parameters = [
                output.decay.unwrap_or_else(|| self.v(p::DECAY)),
                size,
                output.high_ratio.unwrap_or_else(|| self.v(p::HIGH_RATIO)),
                output.low_ratio.unwrap_or_else(|| self.v(p::LOW_RATIO)),
                freeze,
                flat,
                output.low.unwrap_or_else(|| self.v(p::LOW)),
                output.high.unwrap_or_else(|| self.v(p::HIGH)),
            ];
            Some(self.feedback_cache.resolve(feedback_parameters))
        } else {
            None
        };
        for (i, sample) in feedback.into_iter().enumerate() {
            self.feedback_low[i] =
                clean(lo_alpha * self.feedback_low[i] + (1.0 - lo_alpha) * sample);
            // Cascaded shelves have a conservative product-gain bound. Keep
            // even crossed cutoffs and in-flight mode/ratio edits passive;
            // ordinary feedback stays below unity, flat freeze reaches unity.
            let (mid, low, high_ratio, normalizer) = if let Some(gains) = authored_feedback {
                gains[i]
            } else if settled {
                self.settled_tank.feedback[i]
            } else {
                shaping_gains(
                    self.coefficients[i * 3].current,
                    self.coefficients[i * 3 + 1].current,
                    self.coefficients[i * 3 + 2].current,
                )
            };
            let low_shaped = sample * mid + self.feedback_low[i] * (low - mid);
            self.feedback_high[i] =
                clean(hi_alpha * self.feedback_high[i] + (1.0 - hi_alpha) * low_shaped);
            let shaped = (self.feedback_high[i]
                + (low_shaped - self.feedback_high[i]) * high_ratio)
                * normalizer;
            let excitation = (late_input[0] * if i & 1 == 0 { 1.0 } else { -1.0 }
                + late_input[1] * if i & 2 == 0 { 1.0 } else { -1.0 })
                * 0.25;
            self.lines[i].push(shaped + excitation);
        }
        self.previous = self.next;
        for (c, reflection) in early.into_iter().enumerate() {
            let mut diffuse = delayed
                .iter()
                .enumerate()
                .map(|(i, v)| v * if (i >> (c + 1)) & 1 == 0 { 0.25 } else { -0.25 })
                .sum::<f32>();
            for (i, seconds) in [0.005, 0.0017].into_iter().enumerate() {
                let a = diffusion * 0.006;
                let old = self.allpasses[c][i]
                    .read(seconds * (0.5 + 1.5 * scale * 0.01) * self.tank_rate);
                let output = old - a * diffuse;
                self.allpasses[c][i].push(diffuse + a * output);
                diffuse = output;
            }
            self.next[c] = reflection * output.reflect.unwrap_or(self.coefficients[28].current)
                + diffuse * output.diffuse.unwrap_or(self.coefficients[29].current);
        }
        let mid = (self.next[0] + self.next[1]) * 0.5;
        let side = (self.next[0] - self.next[1])
            * 0.5
            * output.stereo.unwrap_or_else(|| self.v(p::STEREO))
            * 0.01;
        self.next = [mid + side, mid - side];
        self.spin_phase = (self.spin_phase + spin_rate / self.tank_rate).fract();
        self.chorus_phase = (self.chorus_phase + chorus_rate / self.tank_rate).fract();
    }
    fn frame(&mut self, frame: &mut StereoFrame, dry_wet: Option<f32>) {
        self.frame_outputs(
            frame,
            AutomatedValues {
                dry_wet,
                ..AutomatedValues::default()
            },
        );
    }
    fn frame_outputs(&mut self, frame: &mut StereoFrame, output: AutomatedValues) {
        if self.tank_ramp_remaining != 0 {
            for ramp in &mut self.parameters {
                ramp.advance();
            }
            for ramp in &mut self.coefficients {
                ramp.advance();
            }
            self.tank_ramp_remaining -= 1;
        }
        let input = frame.map(|v| {
            if v.is_finite() {
                v.clamp(-16.0, 16.0)
            } else {
                0.0
            }
        });
        let freeze = output.freeze.unwrap_or_else(|| self.v(p::FREEZE));
        let cut = output.cut.unwrap_or_else(|| self.v(p::CUT));
        if input != [0.0; 2] && freeze * cut < 1.0 {
            self.active = true;
            self.silent_frames = 0;
        } else if self.active && freeze == 0.0 {
            self.silent_frames += 1;
            if self.silent_frames >= self.silence_limit {
                self.clear();
            }
        }
        if self.active {
            for (sum, v) in self.sum.iter_mut().zip(input) {
                *sum += v;
            }
            self.phase += 1;
            if self.phase == self.factor {
                self.phase = 0;
                self.tank(self.sum.map(|v| v / self.factor as f32), output);
                self.sum = [0.0; 2];
            }
        }
        let fraction = (self.phase + 1) as f32 / self.factor as f32;
        let wet = output.dry_wet.unwrap_or_else(|| self.v(p::DRY_WET)) * 0.01;
        for c in 0..2 {
            let room = self.previous[c] + fraction * (self.next[c] - self.previous[c]);
            frame[c] = ((1.0 - wet) * input[c] + wet * room).clamp(-16.0, 16.0);
        }
    }
}
fn shaping_gains(low: f32, mid: f32, high: f32) -> FeedbackGains {
    let mid = mid.clamp(1e-8, 1.0);
    let envelope = mid.sqrt();
    let low = low.clamp(0.0, envelope);
    let high = high.clamp(0.0, envelope);
    let high_ratio = high / mid;
    let product = low.max(mid) * high_ratio.max(1.0);
    let normalizer = (mid.max(low).max(high) / product).min(1.0);
    (mid, low, high_ratio, normalizer)
}
fn scatter_rotations(amount: f32, stages: f32) -> Rotations {
    std::array::from_fn(|stage| {
        let weight = (stages - stage as f32).clamp(0.0, 1.0);
        (weight != 0.0).then(|| (amount * weight * std::f32::consts::FRAC_PI_4).sin_cos())
    })
}
fn scatter(mut output: [f32; 8], rotations: Rotations) -> [f32; 8] {
    for (stage, rotation) in rotations.into_iter().enumerate() {
        let Some((sin, cos)) = rotation else {
            continue;
        };
        let stride = 1 << stage;
        for base in (0..8).step_by(stride * 2) {
            for offset in 0..stride {
                let a = output[base + offset];
                let b = output[base + offset + stride];
                output[base + offset] = a * cos + b * sin;
                output[base + offset + stride] = b * cos - a * sin;
            }
        }
    }
    output
}
impl NativeProcessorRuntime for ReverbRuntime {
    fn process_block(&mut self, frames: &mut [StereoFrame], _: &dyn hex_lattice::DeviceResources) {
        if !frames.is_empty() && !self.tail_controls_frozen {
            // This active render has no provider automation view. Discard
            // inherited delivery state without resetting the room; a later
            // tail must freeze canonical controls, and resumed identical
            // authored values must be sampled into a fresh cache.
            let canonical_mode = self.settings.get(p::SMOOTH) as u32;
            if self
                .automated_values
                .smooth
                .is_some_and(|mode| mode != canonical_mode)
            {
                self.retime_canonical_ramps(canonical_mode);
            }
            self.automation_controls.reset();
            self.automated_values = AutomatedValues::default();
            self.silence_limit = silence_deadline_frames(self.rate, self.settings.get(p::DECAY));
        }
        for frame in frames {
            if self.tail_controls_frozen {
                self.frame_outputs(frame, self.automated_values);
            } else {
                self.frame(frame, None);
            }
        }
    }
    fn begin_output_tail(&mut self) {
        self.tail_controls_frozen = true;
    }
    fn supports_parameter_automation(&self, parameter: ParameterId) -> bool {
        automation::supports(parameter)
            && (parameter != p::DECAY || self.automation_controls.admitted(0))
            && (parameter != p::FREEZE
                || self.automation_controls.admitted(automation::FREEZE_INDEX))
            && (parameter != p::SMOOTH
                || self.automation_controls.admitted(automation::SMOOTH_INDEX))
    }
    fn process_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        _: Option<&dyn NativeInstrumentClock>,
        automation: &dyn NativeParameterAutomation,
        _: &dyn DeviceResources,
    ) -> Result<(), DeviceContractError> {
        self.process_automation(frames, automation)
    }

    fn reset(&mut self) {
        self.automation_controls.reset();
        self.automated_values = AutomatedValues::default();
        self.tail_controls_frozen = false;
        self.scatter_cache = ScatterCache::default();
        self.feedback_cache = FeedbackCache::default();
        self.silence_limit = silence_deadline_frames(self.rate, self.settings.get(p::DECAY));
        self.clear();
        for v in &mut self.parameters {
            *v = Ramp::steady(v.target);
        }
        for v in &mut self.coefficients {
            *v = Ramp::steady(v.target);
        }
        self.tank_ramp_remaining = 0;
    }
    fn is_quiescent(&self) -> bool {
        !self.active
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|p| p.rate == self.rate)
    }
    fn can_retarget_parameters_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        self.can_copy_history_from(previous)
    }
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        let Some(old) = previous
            .as_any()
            .downcast_ref::<Self>()
            .filter(|p| p.rate == self.rate)
        else {
            return;
        };
        self.automation_controls.inherit(&old.automation_controls);
        self.automated_values = old.automated_values;
        self.scatter_cache = old.scatter_cache;
        self.feedback_cache = old.feedback_cache;
        if self.automation_controls.admitted(0) {
            self.silence_limit = silence_deadline_frames(
                self.rate,
                self.automated_values
                    .decay
                    .unwrap_or(self.settings.get(p::DECAY)),
            );
        }
        // Tail admission belongs to the host, not inherited DSP history. Fresh
        // canonical successors must ignore overrides from removed lanes.
        for (to, from) in self
            .inputs
            .iter_mut()
            .chain(self.lines.iter_mut())
            .chain(self.allpasses.iter_mut().flatten())
            .zip(
                old.inputs
                    .iter()
                    .chain(old.lines.iter())
                    .chain(old.allpasses.iter().flatten()),
            )
        {
            to.copy(from);
        }
        let canonical_mode = self.settings.get(p::SMOOTH) as u32;
        let mode = if self.automation_controls.admitted(automation::SMOOTH_INDEX) {
            old.automated_values.smooth.unwrap_or(canonical_mode)
        } else {
            canonical_mode
        };
        let frames = smoothing_frames(self.rate, mode);
        let mut ramp_remaining = 0;
        for (v, p) in self.parameters.iter_mut().zip(old.parameters) {
            v.inherit(p, frames);
            ramp_remaining = ramp_remaining.max(v.remaining);
        }
        for (v, p) in self.coefficients.iter_mut().zip(old.coefficients) {
            v.inherit(p, frames);
            ramp_remaining = ramp_remaining.max(v.remaining);
        }
        self.tank_ramp_remaining = ramp_remaining;
        self.input_low = old.input_low;
        self.input_high = old.input_high;
        self.feedback_low = old.feedback_low;
        self.feedback_high = old.feedback_high;
        self.phase = old.phase;
        self.sum = old.sum;
        self.previous = old.previous;
        self.next = old.next;
        self.spin_phase = old.spin_phase;
        self.chorus_phase = old.chorus_phase;
        self.active = old.active;
        self.silent_frames = if self.settings.get(p::DECAY) != old.settings.get(p::DECAY)
            || self.settings.on(p::FREEZE) != old.settings.on(p::FREEZE)
        {
            0
        } else {
            old.silent_frames
        };
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_ring_wrap_matches_modulo_at_every_cursor() {
        let mut ring = Ring::new(9);
        for index in 0..9 {
            ring.push(index as f32 * 0.25);
        }
        for cursor in 0..9 {
            ring.cursor = cursor;
            for delay in [0.0_f32, 1.0, 1.25, 4.5, 7.0, 9.0] {
                let clamped = delay.clamp(1.0, 7.0);
                let whole = clamped.floor() as usize;
                let a = (cursor + 9 - whole) % 9;
                let b = (a + 8) % 9;
                let expected = ring.samples[a]
                    + (clamped - whole as f32) * (ring.samples[b] - ring.samples[a]);
                assert_eq!(ring.read(delay).to_bits(), expected.to_bits());
            }
        }
    }

    #[test]
    fn settled_tank_matches_per_frame_recalculation_through_retarget_and_freeze() {
        let original = Settings::default_values();
        let mut frozen = original;
        for (parameter, value) in [
            (p::FREEZE, 1.0),
            (p::DIFFUSION, 21.0),
            (p::DENSITY, 0.0),
            (p::LO_Q, 2.0),
            (p::DECAY, 1.3),
        ] {
            frozen.0[super::super::index(parameter).unwrap()] = value;
        }
        for rate in [44_100, 96_000] {
            let prepare = |settings| PreparedReverb::new(rate, settings).unwrap();
            let mut cached = prepare(original).runtime();
            let mut recalculated = prepare(original).runtime();
            let render = |cached: &mut ReverbRuntime, recalculated: &mut ReverbRuntime| {
                recalculated.tank_ramp_remaining = u32::MAX;
                for index in 0..(rate / 20) {
                    let mut left = if index % 17 < 5 {
                        [0.125, -0.25]
                    } else {
                        [0.0; 2]
                    };
                    let mut right = left;
                    cached.frame(&mut left, None);
                    recalculated.frame(&mut right, None);
                    assert_eq!(left.map(f32::to_bits), right.map(f32::to_bits));
                }
                assert_eq!(cached.tank_ramp_remaining, 0);
            };
            render(&mut cached, &mut recalculated);
            for settings in [frozen, original] {
                let mut next_cached = prepare(settings).runtime();
                let mut next_recalculated = prepare(settings).runtime();
                next_cached.copy_history_from(&cached);
                next_recalculated.copy_history_from(&recalculated);
                assert!(next_cached.tank_ramp_remaining > 0);
                render(&mut next_cached, &mut next_recalculated);
                cached = next_cached;
                recalculated = next_recalculated;
            }
        }
    }

    #[test]
    fn clean_contains_nan_and_preserves_existing_limits() {
        assert_eq!(clean(f32::NAN), 0.0);
        assert_eq!(clean(f32::INFINITY), 32.0);
        assert_eq!(clean(-f32::INFINITY), -32.0);
        assert_eq!(clean(1e-25), 0.0);
        assert_eq!(clean(0.5), 0.5);
    }
    #[test]
    fn resonant_input_sections_match_independent_cutoff_gain_oracle() {
        // A second-order low/high-pass has magnitude Q at its cutoff. Exercise
        // original input sine waves after settling, independently of the preview.
        for rate in [8000.0_f32, 44100.0, 48000.0] {
            for q in [0.1_f32, std::f32::consts::FRAC_1_SQRT_2, 5.85, 10.0] {
                for high in [false, true] {
                    let mut state = [0.0; 2];
                    let g = (std::f32::consts::PI * 830.0 / rate).tan();
                    let mut energy = 0.0_f64;
                    for i in 0..96000 {
                        let x = 0.01 * (std::f32::consts::TAU * 830.0 * i as f32 / rate).sin();
                        let y = InputFilter::new(g, 1.0 / q).apply(&mut state, x, high);
                        if i >= 48000 {
                            energy += f64::from(y).powi(2);
                        }
                    }
                    let gain = (energy / 48000.0 * 2.0).sqrt() / 0.01;
                    assert!(
                        (gain - f64::from(q)).abs() < 0.002,
                        "{rate} {q} {high}: {gain}"
                    );
                }
            }
        }
    }
    #[test]
    fn scattering_is_energy_preserving_for_all_density_modes_and_diffusion_endpoints() {
        let input = [0.13, -0.37, 0.71, 0.25, -0.52, 0.87, 0.11, -0.09];
        let energy = |v: [f32; 8]| v.into_iter().map(|v| f64::from(v).powi(2)).sum::<f64>();
        for stages in 1..=3 {
            for amount in [0.0, 0.17, 0.6, 1.0] {
                assert!(
                    (energy(scatter(input, scatter_rotations(amount, stages as f32)))
                        - energy(input))
                    .abs()
                        < 1e-6
                );
            }
        }
    }
    #[test]
    fn near_unity_feedback_ramps_never_overshoot_or_restart_when_copied() {
        let mut r = Ramp::steady(0.9998);
        r.retarget(1.0, 4800);
        for _ in 0..4800 {
            let previous = r.current;
            r.advance();
            assert!((previous..=1.0).contains(&r.current));
        }
        assert_eq!(r.current, 1.0);
    }
    #[test]
    fn shelf_product_bound_never_exceeds_unity_even_for_asynchronous_edits() {
        for low in [0.001, 0.1, 0.7, 1.0] {
            for mid in [0.001, 0.1, 0.7, 1.0] {
                for high in [0.001, 0.1, 0.7, 1.0] {
                    let (mid, low, ratio, normalizer) = shaping_gains(low, mid, high);
                    let bound = mid.max(low) * ratio.max(1.0) * normalizer;
                    assert!(bound <= 1.000001);
                    if mid < 1.0 {
                        assert!(bound < 1.0);
                    }
                }
            }
        }
    }
    #[test]
    fn actual_heap_storage_matches_the_declaration_including_every_ring() {
        for rate in [8000, 192000] {
            let prepared = PreparedReverb::new(rate, Settings::default_values()).unwrap();
            let runtime = prepared.create_runtime();
            let runtime = runtime.as_any().downcast_ref::<ReverbRuntime>().unwrap();
            let heap = runtime
                .inputs
                .iter()
                .chain(runtime.lines.iter())
                .chain(runtime.allpasses.iter().flatten())
                .map(|ring| std::mem::size_of_val(ring.samples.as_ref()))
                .sum::<usize>();
            assert_eq!(
                runtime.control_storage_bytes(),
                Some(prepared.declared_runtime_bytes())
            );
            assert_eq!(
                prepared.declared_runtime_bytes(),
                std::mem::size_of::<ReverbRuntime>() + heap
            );
            assert!(prepared.declared_runtime_bytes() <= hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES);
        }
    }
}
