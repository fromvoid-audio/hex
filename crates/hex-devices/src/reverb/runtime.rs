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
    (db * (std::f32::consts::LOG2_10 / 20.0)).exp2()
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
fn allocate_ring(capacity: usize) -> Result<Box<[f32]>, DeviceContractError> {
    let mut samples = Vec::new();
    samples
        .try_reserve_exact(capacity)
        .map_err(|_| DeviceContractError::RuntimeAllocation)?;
    samples.resize(capacity, 0.0);
    Ok(samples.into_boxed_slice())
}
/// Paired committed/speculative delay ring with O(written frames) commit and
/// rollback. Between closed transactions, `committed` and `scratch` remain
/// identical at every slot.
struct RingPair {
    committed: Box<[f32]>,
    scratch: Box<[f32]>,
    committed_cursor: usize,
    committed_filled: bool,
    cursor: usize,
    filled: bool,
    dirty_start: usize,
    dirty_len: usize,
    cleared_in_block: bool,
    post_clear_writes: usize,
    post_clear_max: usize,
}
impl RingPair {
    fn try_new(capacity: usize) -> Result<Self, DeviceContractError> {
        Ok(Self {
            committed: allocate_ring(capacity)?,
            scratch: allocate_ring(capacity)?,
            committed_cursor: 0,
            committed_filled: false,
            cursor: 0,
            filled: false,
            dirty_start: 0,
            dirty_len: 0,
            cleared_in_block: false,
            post_clear_writes: 0,
            post_clear_max: 0,
        })
    }
    fn copy_interval(source: &[f32], destination: &mut [f32], start: usize, length: usize) {
        if length == 0 {
            return;
        }
        let first = length.min(source.len() - start);
        destination[start..start + first].copy_from_slice(&source[start..start + first]);
        let rest = length - first;
        if rest > 0 {
            destination[..rest].copy_from_slice(&source[..rest]);
        }
    }
    fn begin_block(&mut self) {
        self.dirty_start = self.cursor;
        self.dirty_len = 0;
        self.cleared_in_block = false;
        self.post_clear_writes = 0;
        self.post_clear_max = 0;
    }
    fn read<const MATURE: bool>(&self, delay: f32) -> f32 {
        let delay = delay.clamp(1.0, (self.scratch.len() - 2) as f32);
        let whole = delay.floor() as usize;
        let fraction = delay - whole as f32;
        let length = self.scratch.len();
        let unwrapped = self.cursor + length - whole;
        // The delay clamp and the ring cursor bound permit a single wrap.
        let a = if unwrapped >= length {
            unwrapped - length
        } else {
            unwrapped
        };
        let b = if a == 0 { length - 1 } else { a - 1 };
        let (newer, older) = if MATURE || self.filled || whole < self.cursor {
            (self.scratch[a], self.scratch[b])
        } else if whole == self.cursor {
            (self.scratch[a], 0.0)
        } else {
            (0.0, 0.0)
        };
        newer + fraction * (older - newer)
    }
    fn push(&mut self, value: f32) {
        let length = self.scratch.len();
        self.scratch[self.cursor] = clean(value);
        self.cursor += 1;
        if self.cursor == length {
            self.cursor = 0;
            self.filled = true;
        }
        if self.cleared_in_block {
            self.post_clear_writes = self.post_clear_writes.saturating_add(1).min(length);
            self.post_clear_max = self.post_clear_max.max(self.post_clear_writes);
        } else {
            self.dirty_len = self.dirty_len.saturating_add(1).min(length);
        }
    }
    fn clear(&mut self) {
        self.cursor = 0;
        self.filled = false;
        self.cleared_in_block = true;
        self.post_clear_writes = 0;
    }
    fn commit(&mut self) {
        Self::copy_interval(
            &self.scratch,
            &mut self.committed,
            self.dirty_start,
            self.dirty_len,
        );
        if self.post_clear_max > 0 {
            self.committed[..self.post_clear_max]
                .copy_from_slice(&self.scratch[..self.post_clear_max]);
        }
        self.committed_cursor = self.cursor;
        self.committed_filled = self.filled;
        self.dirty_start = self.cursor;
        self.dirty_len = 0;
        self.cleared_in_block = false;
        self.post_clear_writes = 0;
        self.post_clear_max = 0;
    }
    fn rollback(&mut self) {
        Self::copy_interval(
            &self.committed,
            &mut self.scratch,
            self.dirty_start,
            self.dirty_len,
        );
        if self.post_clear_max > 0 {
            self.scratch[..self.post_clear_max]
                .copy_from_slice(&self.committed[..self.post_clear_max]);
        }
        self.cursor = self.committed_cursor;
        self.filled = self.committed_filled;
        self.dirty_start = self.cursor;
        self.dirty_len = 0;
        self.cleared_in_block = false;
        self.post_clear_writes = 0;
        self.post_clear_max = 0;
    }
    fn reset(&mut self) {
        self.committed_cursor = 0;
        self.committed_filled = false;
        self.cursor = 0;
        self.filled = false;
        self.dirty_start = 0;
        self.dirty_len = 0;
        self.cleared_in_block = false;
        self.post_clear_writes = 0;
        self.post_clear_max = 0;
    }
    fn copy_committed_from(&mut self, source: &Self) {
        let valid = if source.committed_filled {
            source.committed.len()
        } else {
            source.committed_cursor
        };
        self.committed[..valid].copy_from_slice(&source.committed[..valid]);
        self.scratch[..valid].copy_from_slice(&source.committed[..valid]);
        self.committed_cursor = source.committed_cursor;
        self.committed_filled = source.committed_filled;
        self.cursor = source.committed_cursor;
        self.filled = source.committed_filled;
        self.dirty_start = self.cursor;
        self.dirty_len = 0;
        self.cleared_in_block = false;
        self.post_clear_writes = 0;
        self.post_clear_max = 0;
    }
    fn swap_committed_from(&mut self, source: &mut Self) {
        std::mem::swap(&mut self.committed, &mut source.committed);
        std::mem::swap(&mut self.scratch, &mut source.scratch);
        self.committed_cursor = source.committed_cursor;
        self.committed_filled = source.committed_filled;
        self.cursor = source.committed_cursor;
        self.filled = source.committed_filled;
        self.dirty_start = self.cursor;
        self.dirty_len = 0;
        self.cleared_in_block = false;
        self.post_clear_writes = 0;
        self.post_clear_max = 0;
        source.reset();
    }
}
fn clean(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        hex_dsp::flush_quiet_f32(value).clamp(-32.0, 32.0)
    }
}
type Ramp = hex_dsp::LinearRampF32;
/// Original exclusive requested allocations. This scalar observation is not a
/// custody receipt and never adopts arbitrary provider-owned storage.
#[derive(Clone, Copy)]
pub struct ReverbRuntimeStorage {
    pub inline_bytes: usize,
    pub committed_heap_bytes: [usize; RUNTIME_HEAP_ALLOCATIONS],
    pub scratch_heap_bytes: [usize; RUNTIME_HEAP_ALLOCATIONS],
}
pub const RUNTIME_HEAP_ALLOCATIONS: usize = 14;

#[derive(Clone, Copy)]
struct ReverbState {
    automation_controls: crate::parameter_binding::Controls<AutomatedSettings, 33>,
    automated_values: AutomatedValues,
    scatter_cache: ScatterCache,
    feedback_cache: FeedbackCache,
    parameters: [Ramp; PARAMETER_COUNT],
    coefficients: [Ramp; COEFFICIENTS],
    tank_ramp_remaining: u32,
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
impl ReverbState {
    fn check_inline(&self) {
        let Self {
            automation_controls,
            automated_values,
            scatter_cache,
            feedback_cache,
            parameters,
            coefficients,
            tank_ramp_remaining,
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
        scatter_cache.check_inline();
        feedback_cache.check_inline();
        inline(parameters);
        inline(coefficients);
        inline(tank_ramp_remaining);
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
    }
    fn v(&self, id: hex_lattice_contract::ParameterId) -> f32 {
        self.parameters[(id.value() - 1) as usize].value()
    }
    fn retime_canonical_ramps(&mut self, rate: u32, mode: u32) {
        if self.tank_ramp_remaining == 0 {
            return;
        }
        let frames = smoothing_frames(rate, mode);
        let mut remaining = 0;
        for ramp in self
            .parameters
            .iter_mut()
            .chain(self.coefficients.iter_mut())
        {
            if ramp.is_active() {
                ramp.retarget(ramp.target(), frames);
                remaining = frames;
            }
        }
        self.tank_ramp_remaining = remaining;
    }
    fn clear_scalars(&mut self) {
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
    fn reset(&mut self, rate: u32, settings: Settings) {
        self.automation_controls.reset();
        self.automated_values = AutomatedValues::default();
        self.scatter_cache = ScatterCache::default();
        self.feedback_cache = FeedbackCache::default();
        self.silence_limit = silence_deadline_frames(rate, settings.get(p::DECAY));
        self.clear_scalars();
        for v in &mut self.parameters {
            *v = Ramp::settled(v.target());
        }
        for v in &mut self.coefficients {
            *v = Ramp::settled(v.target());
        }
        self.tank_ramp_remaining = 0;
    }
}

pub struct ReverbRuntime {
    rate: u32,
    factor: u32,
    tank_rate: f32,
    settled_tank: TankDesign,
    settings: Settings,
    primary: ReverbState,
    speculative: ReverbState,
    tail_controls_frozen: bool,
    open: bool,
    inputs: [RingPair; 2],
    lines: [RingPair; 8],
    allpasses: [[RingPair; 2]; 2],
}
impl ReverbRuntime {
    /// Original exclusive runtime leaf, including every requested Box payload.
    /// A borrowed declaration never transfers native custody. The closed host
    /// constructor must establish origin before using this bounded observation.
    /// Exhaustive destructuring forces an owning-field audit when this changes.
    pub fn control_storage(&self) -> Option<ReverbRuntimeStorage> {
        let Self {
            rate,
            factor,
            tank_rate,
            settled_tank,
            settings,
            primary,
            speculative,
            tail_controls_frozen,
            open,
            inputs,
            lines,
            allpasses,
        } = self;
        fn inline<T: Copy>(_: &T) {}
        inline(rate);
        inline(factor);
        inline(tank_rate);
        inline(settled_tank);
        inline(settings);
        primary.check_inline();
        speculative.check_inline();
        inline(tail_controls_frozen);
        inline(open);
        let mut committed_heap_bytes = [0; RUNTIME_HEAP_ALLOCATIONS];
        let mut scratch_heap_bytes = [0; RUNTIME_HEAP_ALLOCATIONS];
        let mut observed = 0;
        for (index, ring) in inputs
            .iter()
            .chain(lines.iter())
            .chain(allpasses.iter().flatten())
            .enumerate()
        {
            let RingPair {
                committed,
                scratch,
                committed_cursor,
                committed_filled,
                cursor,
                filled,
                dirty_start,
                dirty_len,
                cleared_in_block,
                post_clear_writes,
                post_clear_max,
            } = ring;
            inline(committed_cursor);
            inline(committed_filled);
            inline(cursor);
            inline(filled);
            inline(dirty_start);
            inline(dirty_len);
            inline(cleared_in_block);
            inline(post_clear_writes);
            inline(post_clear_max);
            *committed_heap_bytes.get_mut(index)? =
                committed.len().checked_mul(std::mem::size_of::<f32>())?;
            *scratch_heap_bytes.get_mut(index)? =
                scratch.len().checked_mul(std::mem::size_of::<f32>())?;
            observed += 1;
        }
        if observed != RUNTIME_HEAP_ALLOCATIONS {
            return None;
        }
        Some(ReverbRuntimeStorage {
            inline_bytes: std::mem::size_of::<Self>(),
            committed_heap_bytes,
            scratch_heap_bytes,
        })
    }
    pub fn control_storage_bytes(&self) -> Option<usize> {
        let storage = self.control_storage()?;
        let committed = storage
            .committed_heap_bytes
            .into_iter()
            .try_fold(storage.inline_bytes, usize::checked_add)?;
        storage
            .scratch_heap_bytes
            .into_iter()
            .try_fold(committed, usize::checked_add)
    }
    fn rings_mut(&mut self) -> impl Iterator<Item = &mut RingPair> {
        self.inputs
            .iter_mut()
            .chain(self.lines.iter_mut())
            .chain(self.allpasses.iter_mut().flatten())
    }
    fn inherit_scalars(&mut self, old: &ReverbState, old_settings: Settings) {
        self.tail_controls_frozen = false;
        self.primary
            .automation_controls
            .inherit(&old.automation_controls);
        self.primary.automated_values = old.automated_values;
        self.primary.scatter_cache = old.scatter_cache;
        self.primary.feedback_cache = old.feedback_cache;
        if self.primary.automation_controls.admitted(0) {
            self.primary.silence_limit = silence_deadline_frames(
                self.rate,
                self.primary
                    .automated_values
                    .decay
                    .unwrap_or(self.settings.get(p::DECAY)),
            );
        }
        let canonical_mode = self.settings.get(p::SMOOTH) as u32;
        let mode = if self
            .primary
            .automation_controls
            .admitted(automation::SMOOTH_INDEX)
        {
            old.automated_values.smooth.unwrap_or(canonical_mode)
        } else {
            canonical_mode
        };
        let frames = smoothing_frames(self.rate, mode);
        let mut ramp_remaining = 0;
        for (v, p) in self.primary.parameters.iter_mut().zip(old.parameters) {
            v.inherit(p, frames);
            ramp_remaining = ramp_remaining.max(v.remaining());
        }
        for (v, p) in self.primary.coefficients.iter_mut().zip(old.coefficients) {
            v.inherit(p, frames);
            ramp_remaining = ramp_remaining.max(v.remaining());
        }
        self.primary.tank_ramp_remaining = ramp_remaining;
        self.primary.input_low = old.input_low;
        self.primary.input_high = old.input_high;
        self.primary.feedback_low = old.feedback_low;
        self.primary.feedback_high = old.feedback_high;
        self.primary.phase = old.phase;
        self.primary.sum = old.sum;
        self.primary.previous = old.previous;
        self.primary.next = old.next;
        self.primary.spin_phase = old.spin_phase;
        self.primary.chorus_phase = old.chorus_phase;
        self.primary.active = old.active;
        self.primary.silent_frames = if self.settings.get(p::DECAY) != old_settings.get(p::DECAY)
            || self.settings.on(p::FREEZE) != old_settings.on(p::FREEZE)
        {
            0
        } else {
            old.silent_frames
        };
        self.speculative = self.primary;
        self.open = false;
    }
    fn clear_speculative(&mut self) {
        for ring in self.rings_mut() {
            ring.clear();
        }
        self.speculative.clear_scalars();
    }
    fn tank<const MATURE: bool>(&mut self, input: StereoFrame, output: AutomatedValues) {
        let s = &mut self.speculative;
        let freeze = output.freeze.unwrap_or_else(|| s.v(p::FREEZE));
        let flat = output.flat.unwrap_or_else(|| s.v(p::FLAT));
        let cut = output.cut.unwrap_or_else(|| s.v(p::CUT));
        let predelay = output.predelay.unwrap_or_else(|| s.v(p::PREDELAY));
        let spin_amount = output.spin_amount.unwrap_or_else(|| s.v(p::SPIN_AMOUNT));
        let spin_rate = output.spin_rate.unwrap_or_else(|| s.v(p::SPIN_RATE));
        let shape = output.shape.unwrap_or_else(|| s.v(p::SHAPE));
        let scale = output.scale.unwrap_or_else(|| s.v(p::SCALE));
        let chorus_amount = output
            .chorus_amount
            .unwrap_or_else(|| s.v(p::CHORUS_AMOUNT));
        let chorus_rate = output.chorus_rate.unwrap_or_else(|| s.v(p::CHORUS_RATE));
        let diffusion = output.diffusion.unwrap_or_else(|| s.v(p::DIFFUSION));
        let density = output.density.unwrap_or_else(|| s.v(p::DENSITY));
        let size = (output.size.unwrap_or_else(|| s.v(p::SIZE)) / 100.0).sqrt();
        let predelay = predelay * 0.001 * self.tank_rate;
        let excitation = 1.0 - freeze * cut;
        let settled = s.tank_ramp_remaining == 0;
        let [high_filter, low_filter] = if settled {
            self.settled_tank.filters
        } else {
            [
                InputFilter::new(s.coefficients[24].value(), s.coefficients[30].value()),
                InputFilter::new(s.coefficients[25].value(), s.coefficients[31].value()),
            ]
        };
        let high_filter = if output.lo_g.is_some() || output.lo_damping.is_some() {
            InputFilter::new(
                output.lo_g.unwrap_or(s.coefficients[24].value()),
                output.lo_damping.unwrap_or(s.coefficients[30].value()),
            )
        } else {
            high_filter
        };
        let low_filter = if output.hi_g.is_some() || output.hi_damping.is_some() {
            InputFilter::new(
                output.hi_g.unwrap_or(s.coefficients[25].value()),
                output.hi_damping.unwrap_or(s.coefficients[31].value()),
            )
        } else {
            low_filter
        };
        let lo_alpha = output.low_alpha.unwrap_or(s.coefficients[26].value());
        let hi_alpha = output.high_alpha.unwrap_or(s.coefficients[27].value());
        let mut late_input = [0.0; 2];
        let mut early = [0.0; 2];
        let spin_scale =
            output.spin.unwrap_or_else(|| s.v(p::SPIN)) * spin_amount * 0.000025 * self.tank_rate;
        let spin_sc = (spin_scale != 0.0).then(|| (std::f32::consts::TAU * s.spin_phase).sin_cos());
        let tap_scale = size * (0.75 + 0.5 * shape) * self.tank_rate;
        let lo_cut = output.lo_cut.unwrap_or_else(|| s.v(p::LO_CUT));
        let hi_cut = output.hi_cut.unwrap_or_else(|| s.v(p::HI_CUT));
        for c in 0..2 {
            let mut x = input[c] * excitation;
            let high = high_filter.apply(&mut s.input_low[c], x, true);
            x += (high - x) * lo_cut;
            let low = low_filter.apply(&mut s.input_high[c], x, false);
            x += (low - x) * hi_cut;
            self.inputs[c].push(x);
            late_input[c] = self.inputs[c].read::<MATURE>(predelay + 1.0);
            // Channel 1 adds +0.25 turns (+pi/2): sin(a + pi/2 + b) = cos(a)cos(b) - sin(a)sin(b).
            let spin_channel = spin_sc.map(|(si, co)| if c == 0 { (si, co) } else { (co, -si) });
            for (i, seconds) in TAPS.into_iter().enumerate() {
                let spin = if let Some((si, co)) = spin_channel {
                    let (tap_sin, tap_cos) = SPIN_TAP_SIN_COS[i];
                    (si * tap_cos + co * tap_sin) * spin_scale
                } else {
                    0.0
                };
                let delay = predelay + (seconds * tap_scale + spin).max(0.0) + 1.0;
                let weight = (1.0 - shape + shape * (i as f32 / 7.0)) * 0.125;
                early[c] += self.inputs[c].read::<MATURE>(delay)
                    * weight
                    * if i % 3 == 2 { -1.0 } else { 1.0 };
            }
            late_input[c] += early[c] * 0.25;
        }
        let depth = output.chorus.unwrap_or_else(|| s.v(p::CHORUS))
            * chorus_amount
            * 0.001
            * self.tank_rate
            * (1.0 - freeze);
        let chorus_sc = (depth != 0.0).then(|| (std::f32::consts::TAU * s.chorus_phase).sin_cos());
        let delayed = std::array::from_fn(|i| {
            let modulation = if let Some((si, co)) = chorus_sc {
                let (tap_sin, tap_cos) = CHORUS_TAP_SIN_COS[i];
                (si * tap_cos + co * tap_sin) * depth
            } else {
                0.0
            };
            let base = TIMES[i] * size * self.tank_rate;
            let delay = base + freeze * (base.round() - base);
            self.lines[i].read::<MATURE>(delay + modulation)
        });
        let rotations = if output.diffusion.is_some() || output.density.is_some() {
            s.scatter_cache.resolve(diffusion, density)
        } else if settled {
            self.settled_tank.rotations
        } else {
            scatter_rotations(s.v(p::DIFFUSION) * 0.01, s.v(p::DENSITY) + 1.0)
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
                output.decay.unwrap_or_else(|| s.v(p::DECAY)),
                size,
                output.high_ratio.unwrap_or_else(|| s.v(p::HIGH_RATIO)),
                output.low_ratio.unwrap_or_else(|| s.v(p::LOW_RATIO)),
                freeze,
                flat,
                output.low.unwrap_or_else(|| s.v(p::LOW)),
                output.high.unwrap_or_else(|| s.v(p::HIGH)),
            ];
            Some(*s.feedback_cache.resolve(feedback_parameters))
        } else {
            None
        };
        for (i, sample) in feedback.into_iter().enumerate() {
            s.feedback_low[i] = clean(lo_alpha * s.feedback_low[i] + (1.0 - lo_alpha) * sample);
            // Cascaded shelves have a conservative product-gain bound. Keep
            // even crossed cutoffs and in-flight mode/ratio edits passive;
            // ordinary feedback stays below unity, flat freeze reaches unity.
            let (mid, low, high_ratio, normalizer) = if let Some(gains) = authored_feedback {
                gains[i]
            } else if settled {
                self.settled_tank.feedback[i]
            } else {
                shaping_gains(
                    s.coefficients[i * 3].value(),
                    s.coefficients[i * 3 + 1].value(),
                    s.coefficients[i * 3 + 2].value(),
                )
            };
            let low_shaped = sample * mid + s.feedback_low[i] * (low - mid);
            s.feedback_high[i] =
                clean(hi_alpha * s.feedback_high[i] + (1.0 - hi_alpha) * low_shaped);
            let shaped =
                (s.feedback_high[i] + (low_shaped - s.feedback_high[i]) * high_ratio) * normalizer;
            let excitation = (late_input[0] * if i & 1 == 0 { 1.0 } else { -1.0 }
                + late_input[1] * if i & 2 == 0 { 1.0 } else { -1.0 })
                * 0.25;
            self.lines[i].push(shaped + excitation);
        }
        s.previous = s.next;
        for (c, reflection) in early.into_iter().enumerate() {
            let mut diffuse = delayed
                .iter()
                .enumerate()
                .map(|(i, v)| v * if (i >> (c + 1)) & 1 == 0 { 0.25 } else { -0.25 })
                .sum::<f32>();
            for (i, seconds) in [0.005, 0.0017].into_iter().enumerate() {
                let a = diffusion * 0.006;
                let old = self.allpasses[c][i]
                    .read::<MATURE>(seconds * (0.5 + 1.5 * scale * 0.01) * self.tank_rate);
                let out = old - a * diffuse;
                self.allpasses[c][i].push(diffuse + a * out);
                diffuse = out;
            }
            s.next[c] = reflection * output.reflect.unwrap_or(s.coefficients[28].value())
                + diffuse * output.diffuse.unwrap_or(s.coefficients[29].value());
        }
        let mid = (s.next[0] + s.next[1]) * 0.5;
        let side =
            (s.next[0] - s.next[1]) * 0.5 * output.stereo.unwrap_or_else(|| s.v(p::STEREO)) * 0.01;
        s.next = [mid + side, mid - side];
        s.spin_phase = (s.spin_phase + spin_rate / self.tank_rate).fract();
        s.chorus_phase = (s.chorus_phase + chorus_rate / self.tank_rate).fract();
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
        if self.speculative.tank_ramp_remaining != 0 {
            for ramp in &mut self.speculative.parameters {
                ramp.advance();
            }
            for ramp in &mut self.speculative.coefficients {
                ramp.advance();
            }
            self.speculative.tank_ramp_remaining -= 1;
        }
        let input = frame.map(|v| {
            if v.is_finite() {
                v.clamp(-16.0, 16.0)
            } else {
                0.0
            }
        });
        let freeze = output
            .freeze
            .unwrap_or_else(|| self.speculative.v(p::FREEZE));
        let cut = output.cut.unwrap_or_else(|| self.speculative.v(p::CUT));
        if input != [0.0; 2] && freeze * cut < 1.0 {
            self.speculative.active = true;
            self.speculative.silent_frames = 0;
        } else if self.speculative.active && freeze == 0.0 {
            self.speculative.silent_frames += 1;
            if self.speculative.silent_frames >= self.speculative.silence_limit {
                self.clear_speculative();
            }
        }
        if self.speculative.active {
            for (sum, v) in self.speculative.sum.iter_mut().zip(input) {
                *sum += v;
            }
            self.speculative.phase += 1;
            if self.speculative.phase == self.factor {
                self.speculative.phase = 0;
                let averaged = self.speculative.sum.map(|v| v / self.factor as f32);
                if self.inputs[0].filled {
                    self.tank::<true>(averaged, output);
                } else {
                    self.tank::<false>(averaged, output);
                }
                self.speculative.sum = [0.0; 2];
            }
        }
        let fraction = (self.speculative.phase + 1) as f32 / self.factor as f32;
        let wet = output
            .dry_wet
            .unwrap_or_else(|| self.speculative.v(p::DRY_WET))
            * 0.01;
        for c in 0..2 {
            let room = self.speculative.previous[c]
                + fraction * (self.speculative.next[c] - self.speculative.previous[c]);
            frame[c] = ((1.0 - wet) * input[c] + wet * room).clamp(-16.0, 16.0);
        }
    }
    fn process(&mut self, frames: &mut [StereoFrame], bypassed: bool) {
        if !self.open {
            self.begin_block();
        }
        if !frames.is_empty() && !self.tail_controls_frozen {
            // This active render has no provider automation view. Discard
            // inherited delivery state without resetting the room; a later
            // tail must freeze canonical controls, and resumed identical
            // authored values must be sampled into a fresh cache.
            let canonical_mode = self.settings.get(p::SMOOTH) as u32;
            if self
                .speculative
                .automated_values
                .smooth
                .is_some_and(|mode| mode != canonical_mode)
            {
                self.speculative
                    .retime_canonical_ramps(self.rate, canonical_mode);
            }
            self.speculative.automation_controls.reset();
            self.speculative.automated_values = AutomatedValues::default();
            self.speculative.silence_limit =
                silence_deadline_frames(self.rate, self.settings.get(p::DECAY));
        }
        for frame in frames {
            let dry = *frame;
            if self.tail_controls_frozen {
                self.frame_outputs(frame, self.speculative.automated_values);
            } else {
                self.frame(frame, None);
            }
            if bypassed {
                *frame = dry;
            }
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
impl OwnedProcessorRuntime for ReverbRuntime {
    fn fixed_storage_pair_bytes(&self) -> Option<usize> {
        self.control_storage_bytes()
    }
    fn begin_block(&mut self) {
        if self.open {
            self.rollback_block();
        }
        self.speculative = self.primary;
        for ring in self.rings_mut() {
            ring.begin_block();
        }
        self.open = true;
    }
    fn process_block(&mut self, frames: &mut [StereoFrame], _: &dyn DeviceResources) {
        self.process(frames, false);
    }
    fn process_bypassed_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        _: Option<&dyn NativeInstrumentClock>,
        _: &dyn DeviceResources,
    ) {
        self.process(frames, true);
    }
    fn commit_block(&mut self) {
        if !self.open {
            return;
        }
        for ring in self.rings_mut() {
            ring.commit();
        }
        self.primary = self.speculative;
        self.open = false;
    }
    fn rollback_block(&mut self) {
        if !self.open {
            return;
        }
        for ring in self.rings_mut() {
            ring.rollback();
        }
        self.speculative = self.primary;
        self.open = false;
    }
    fn begin_output_tail(&mut self) {
        self.tail_controls_frozen = true;
    }
    fn supports_parameter_automation(&self, parameter: ParameterId) -> bool {
        automation::supports(parameter)
            && (parameter != p::DECAY || self.primary.automation_controls.admitted(0))
            && (parameter != p::FREEZE
                || self
                    .primary
                    .automation_controls
                    .admitted(automation::FREEZE_INDEX))
            && (parameter != p::SMOOTH
                || self
                    .primary
                    .automation_controls
                    .admitted(automation::SMOOTH_INDEX))
    }
    fn process_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        _: Option<&dyn NativeInstrumentClock>,
        automation: &dyn NativeParameterAutomation,
        _: &dyn DeviceResources,
    ) -> Result<(), DeviceContractError> {
        self.process_automation(frames, automation, false)
    }
    fn process_bypassed_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        _: Option<&dyn NativeInstrumentClock>,
        automation: &dyn NativeParameterAutomation,
        _: &dyn DeviceResources,
    ) -> Result<(), DeviceContractError> {
        self.process_automation(frames, automation, true)
    }
    fn reset(&mut self) {
        if self.open {
            self.rollback_block();
        }
        self.tail_controls_frozen = false;
        for ring in self.rings_mut() {
            ring.reset();
        }
        self.primary.reset(self.rate, self.settings);
        self.speculative = self.primary;
        self.open = false;
    }
    fn committed_is_quiescent(&self) -> bool {
        !self.primary.active
    }
    fn speculative_is_quiescent(&self) -> bool {
        if self.open {
            !self.speculative.active
        } else {
            !self.primary.active
        }
    }
    fn can_inherit_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|p| !self.open && !p.open && p.rate == self.rate)
    }
    fn can_retarget_parameters_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        self.can_inherit_from(previous)
    }
    fn inherit_from(&mut self, previous: &dyn OwnedProcessorRuntime) {
        if !self.can_inherit_from(previous) {
            return;
        }
        let Some(old) = previous.as_any().downcast_ref::<Self>() else {
            return;
        };
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
            to.copy_committed_from(from);
        }
        self.inherit_scalars(&old.primary, old.settings);
    }
    fn transfer_history_from(&mut self, previous: &mut dyn OwnedProcessorRuntime) {
        if !self.can_inherit_from(previous) {
            return;
        }
        let Some(old) = previous.as_any_mut().and_then(|v| v.downcast_mut::<Self>()) else {
            return;
        };
        for (to, from) in self
            .inputs
            .iter_mut()
            .chain(self.lines.iter_mut())
            .chain(self.allpasses.iter_mut().flatten())
            .zip(
                old.inputs
                    .iter_mut()
                    .chain(old.lines.iter_mut())
                    .chain(old.allpasses.iter_mut().flatten()),
            )
        {
            to.swap_committed_from(from);
        }
        let old_primary = old.primary;
        let old_settings = old.settings;
        self.inherit_scalars(&old_primary, old_settings);
        old.reset();
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_ring_wrap_matches_modulo_at_every_cursor() {
        let mut ring = RingPair::try_new(9).unwrap();
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
                let expected = ring.scratch[a]
                    + (clamped - whole as f32) * (ring.scratch[b] - ring.scratch[a]);
                assert_eq!(ring.read::<true>(delay).to_bits(), expected.to_bits());
                assert_eq!(ring.read::<false>(delay).to_bits(), expected.to_bits());
            }
        }
    }

    #[test]
    fn cleared_and_copied_ring_reads_match_fresh_history_through_wrap() {
        let mut source = RingPair::try_new(9).unwrap();
        let mut target = RingPair::try_new(9).unwrap();
        source.begin_block();
        target.begin_block();
        for i in 1..26 {
            source.push(i as f32);
            target.push(-(i as f32));
        }
        source.clear();
        source.commit();
        target.commit();

        let mut history = Vec::new();
        for i in 0..22 {
            target.copy_committed_from(&source);
            for delay in [1.0_f32, 1.5, 4.5, 7.0] {
                let whole = delay.floor() as usize;
                let read = |lag| {
                    history
                        .len()
                        .checked_sub(lag)
                        .and_then(|index| history.get(index))
                        .copied()
                        .unwrap_or(0.0)
                };
                let newer = read(whole);
                let older = read(whole + 1);
                let expected = newer + (delay - whole as f32) * (older - newer);
                assert_eq!(source.read::<false>(delay), expected);
                assert_eq!(target.read::<false>(delay), expected);
            }
            let new_sample = i as f32 * -0.25;
            source.begin_block();
            source.push(new_sample);
            source.commit();
            history.push(new_sample);
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
            let mut cached = prepare(original).runtime().unwrap();
            let mut recalculated = prepare(original).runtime().unwrap();
            let render = |cached: &mut ReverbRuntime, recalculated: &mut ReverbRuntime| {
                cached.begin_block();
                recalculated.begin_block();
                recalculated.speculative.tank_ramp_remaining = u32::MAX;
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
                assert_eq!(cached.speculative.tank_ramp_remaining, 0);
                cached.commit_block();
                recalculated.commit_block();
            };
            render(&mut cached, &mut recalculated);
            for settings in [frozen, original] {
                let mut next_cached = prepare(settings).runtime().unwrap();
                let mut next_recalculated = prepare(settings).runtime().unwrap();
                next_cached.inherit_from(&cached);
                next_recalculated.inherit_from(&recalculated);
                assert!(next_cached.primary.tank_ramp_remaining > 0);
                render(&mut next_cached, &mut next_recalculated);
                cached = next_cached;
                recalculated = next_recalculated;
            }
        }
    }

    #[test]
    fn dirty_range_rollback_and_mid_block_clear_preserve_exact_committed_history() {
        let mut settings = Settings::default_values();
        settings.0[super::super::index(p::DECAY).unwrap()] = 0.1;
        settings.0[super::super::index(p::DRY_WET).unwrap()] = 100.0;
        let prepared = PreparedReverb::new(8_000, settings).unwrap();
        let mut runtime = prepared.runtime().unwrap();
        let mut oracle = prepared.runtime().unwrap();

        let seed: Vec<StereoFrame> = (0..256)
            .map(|i| {
                let s = ((i % 29) as f32 - 14.0) / 32.0;
                [s, -s * 0.5]
            })
            .collect();
        let mut s1 = seed.clone();
        let mut s2 = seed;
        runtime.begin_block();
        runtime.process_block(&mut s1, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        runtime.commit_block();
        oracle.begin_block();
        oracle.process_block(&mut s2, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        oracle.commit_block();
        assert_eq!(s1, s2);

        // Speculatively advance past silence_limit (triggering mid-block clear)
        // and then re-excite in the same block, then roll back on fault.
        runtime.begin_block();
        let mut clear_block = vec![[0.0; 2]; runtime.primary.silence_limit as usize + 64];
        for frame in clear_block.iter_mut().rev().take(32) {
            *frame = [0.75, -0.5];
        }
        runtime.process_block(&mut clear_block, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        runtime.rollback_block();

        // Subsequent committed blocks must match the untouched oracle bit-for-bit.
        for chunk_len in [1, 17, 128, 257] {
            let mut actual: Vec<StereoFrame> = (0..chunk_len)
                .map(|i| {
                    [
                        (i as f32 * 0.13).sin() * 0.2,
                        -(i as f32 * 0.07).cos() * 0.2,
                    ]
                })
                .collect();
            let mut expected = actual.clone();
            runtime.begin_block();
            runtime.process_block(&mut actual, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            // Rollback and replay must produce identical output too.
            let first = actual.clone();
            runtime.rollback_block();
            actual.copy_from_slice(&expected);
            runtime.begin_block();
            runtime.process_block(&mut actual, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            assert_eq!(actual, first);
            runtime.commit_block();

            oracle.begin_block();
            oracle.process_block(&mut expected, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            oracle.commit_block();
            assert_eq!(actual, expected);
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
        let mut r = Ramp::settled(0.9998);
        r.retarget(1.0, 4800);
        for _ in 0..4800 {
            let previous = r.value();
            r.advance();
            assert!((previous..=1.0).contains(&r.value()));
        }
        assert_eq!(r.value(), 1.0);
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
            let runtime = prepared.create_runtime().unwrap();
            let runtime = runtime.as_any().downcast_ref::<ReverbRuntime>().unwrap();
            let heap = runtime
                .inputs
                .iter()
                .chain(runtime.lines.iter())
                .chain(runtime.allpasses.iter().flatten())
                .map(|ring| {
                    std::mem::size_of_val(ring.committed.as_ref())
                        + std::mem::size_of_val(ring.scratch.as_ref())
                })
                .sum::<usize>();
            assert_eq!(
                runtime.control_storage_bytes(),
                Some(prepared.declared_pair_bytes())
            );
            assert_eq!(
                prepared.declared_pair_bytes(),
                std::mem::size_of::<ReverbRuntime>() + heap
            );
            assert!(prepared.declared_pair_bytes() <= hex_lattice::MAX_OWNED_PROCESSOR_PAIR_BYTES);
        }
    }

    #[test]
    fn modulation_angle_addition_tables_match_direct_sin_for_all_taps_and_channels() {
        for i in 0..8 {
            let (expected_spin_sin, expected_spin_cos) =
                (std::f32::consts::TAU * i as f32 * 0.137).sin_cos();
            assert!((SPIN_TAP_SIN_COS[i].0 - expected_spin_sin).abs() < 1e-6);
            assert!((SPIN_TAP_SIN_COS[i].1 - expected_spin_cos).abs() < 1e-6);

            let (expected_chorus_sin, expected_chorus_cos) =
                (std::f32::consts::TAU * i as f32 * 0.173).sin_cos();
            assert!((CHORUS_TAP_SIN_COS[i].0 - expected_chorus_sin).abs() < 1e-6);
            assert!((CHORUS_TAP_SIN_COS[i].1 - expected_chorus_cos).abs() < 1e-6);
        }
        for phase in [0.0_f32, 0.07, 0.25, 0.43, 0.77, 0.99] {
            let (s, co) = (std::f32::consts::TAU * phase).sin_cos();
            for c in 0..2 {
                let (cs, cco) = if c == 0 { (s, co) } else { (co, -s) };
                for (i, &(tap_sin, tap_cos)) in SPIN_TAP_SIN_COS.iter().enumerate() {
                    let approx = cs * tap_cos + cco * tap_sin;
                    let exact = (std::f32::consts::TAU
                        * (phase + c as f32 * 0.25 + i as f32 * 0.137))
                        .sin();
                    assert!(
                        (approx - exact).abs() < 1e-5,
                        "spin phase={phase} c={c} i={i}: approx={approx} exact={exact}"
                    );
                }
            }
            for (i, &(tap_sin, tap_cos)) in CHORUS_TAP_SIN_COS.iter().enumerate() {
                let approx = s * tap_cos + co * tap_sin;
                let exact = (std::f32::consts::TAU * (phase + i as f32 * 0.173)).sin();
                assert!(
                    (approx - exact).abs() < 1e-5,
                    "chorus phase={phase} i={i}: approx={approx} exact={exact}"
                );
            }
        }
    }
}
