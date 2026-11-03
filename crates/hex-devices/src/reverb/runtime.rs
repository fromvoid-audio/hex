//! Bounded FDN history; preparation is the only allocation and coefficient owner.
use super::{PARAMETER_COUNT, Settings, parameter as p};
use hex_lattice::{NativeProcessorRuntime, OutputTail, PreparedNativeProcessor, StereoFrame};
use hex_lattice_contract::DeviceContractError;
use std::any::Any;
const TIMES: [f32; 8] = [
    0.0233, 0.0297, 0.0311, 0.0371, 0.0411, 0.0437, 0.0479, 0.0539,
];
const TAPS: [f32; 8] = [
    0.0043, 0.0117, 0.0193, 0.0271, 0.0379, 0.0499, 0.0611, 0.0737,
];
const COEFFICIENTS: usize = 32;
#[derive(Clone, Copy)]
pub struct PreparedReverb {
    rate: u32,
    factor: u32,
    tank_rate: f32,
    settings: Settings,
    coefficients: [f32; COEFFICIENTS],
    input_capacity: usize,
    capacities: [usize; 8],
    allpass_capacities: [usize; 2],
    silence_limit: u64,
}
// Topology-preserving state-variable section. Positive g and damping remain
// stable throughout bounded parameter ramps; no arbitrary biquad interpolation.
fn input_filter(state: &mut [f32; 2], x: f32, g: f32, damping: f32, high: bool) -> f32 {
    let a1 = 1.0 / (1.0 + g * (g + damping));
    let a2 = g * a1;
    let a3 = g * a2;
    let v3 = x - state[1];
    let v1 = a1 * state[0] + a2 * v3;
    let v2 = state[1] + a2 * state[0] + a3 * v3;
    state[0] = clean(2.0 * v1 - state[0]);
    state[1] = clean(2.0 * v2 - state[1]);
    clean(if high { x - damping * v1 - v2 } else { v2 })
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
            input_capacity: (0.4 * tank_rate).ceil() as usize + 4,
            capacities: TIMES
                .map(|t| ((t * std::f32::consts::SQRT_2 + 0.004) * tank_rate).ceil() as usize + 4),
            allpass_capacities: [0.005_f32, 0.0017]
                .map(|t| (t * 2.0 * tank_rate).ceil() as usize + 4),
            silence_limit: ((f64::from(settings.get(p::DECAY)) * 3.0 + 1.0).max(8.0)
                * f64::from(rate))
            .ceil() as u64,
        })
    }
}
impl PreparedNativeProcessor for PreparedReverb {
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
        Box::new(ReverbRuntime {
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
        })
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn output_tail(&self) -> OutputTail {
        if self.settings.on(p::FREEZE) {
            OutputTail::Unbounded
        } else {
            OutputTail::Finite {
                // A copied freeze-off ramp may retain up to 100ms before its
                // silence clock starts. Include that transition in the promise.
                max_tail_frames: self.silence_limit + u64::from(self.rate).div_ceil(10),
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
        let a = (self.cursor + self.samples.len() - whole) % self.samples.len();
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
    rate: u32,
    factor: u32,
    tank_rate: f32,
    inputs: [Ring; 2],
    lines: [Ring; 8],
    allpasses: [[Ring; 2]; 2],
    parameters: [Ramp; PARAMETER_COUNT],
    coefficients: [Ramp; COEFFICIENTS],
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
            rate,
            factor,
            tank_rate,
            inputs,
            lines,
            allpasses,
            parameters,
            coefficients,
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
        inline(rate);
        inline(factor);
        inline(tank_rate);
        inline(parameters);
        inline(coefficients);
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
    fn tank(&mut self, input: StereoFrame) {
        let size = (self.v(p::SIZE) / 100.0).sqrt();
        let predelay = self.v(p::PREDELAY) * 0.001 * self.tank_rate;
        let excitation = 1.0 - self.v(p::FREEZE) * self.v(p::CUT);
        let mut late_input = [0.0; 2];
        let mut early = [0.0; 2];
        for c in 0..2 {
            let mut x = input[c] * excitation;
            let high = input_filter(
                &mut self.input_low[c],
                x,
                self.coefficients[24].current,
                self.coefficients[30].current,
                true,
            );
            x += (high - x) * self.v(p::LO_CUT);
            let low = input_filter(
                &mut self.input_high[c],
                x,
                self.coefficients[25].current,
                self.coefficients[31].current,
                false,
            );
            x += (low - x) * self.v(p::HI_CUT);
            self.inputs[c].push(x);
            late_input[c] = self.inputs[c].read(predelay + 1.0);
            for (i, seconds) in TAPS.into_iter().enumerate() {
                let phase = self.spin_phase + c as f32 * 0.25 + i as f32 * 0.137;
                let spin = (std::f32::consts::TAU * phase).sin()
                    * self.v(p::SPIN)
                    * self.v(p::SPIN_AMOUNT)
                    * 0.000025
                    * self.tank_rate;
                let delay = predelay
                    + (seconds * size * (0.75 + 0.5 * self.v(p::SHAPE)) * self.tank_rate + spin)
                        .max(0.0)
                    + 1.0;
                let weight = (1.0 - self.v(p::SHAPE) + self.v(p::SHAPE) * (i as f32 / 7.0)) * 0.125;
                early[c] +=
                    self.inputs[c].read(delay) * weight * if i % 3 == 2 { -1.0 } else { 1.0 };
            }
            late_input[c] += early[c] * 0.25;
        }
        let depth = self.v(p::CHORUS)
            * self.v(p::CHORUS_AMOUNT)
            * 0.001
            * self.tank_rate
            * (1.0 - self.v(p::FREEZE));
        let delayed = std::array::from_fn(|i| {
            let modulation =
                (std::f32::consts::TAU * (self.chorus_phase + i as f32 * 0.173)).sin() * depth;
            let base = TIMES[i] * size * self.tank_rate;
            let delay = base + self.v(p::FREEZE) * (base.round() - base);
            self.lines[i].read(delay + modulation)
        });
        let feedback = scatter(
            delayed,
            self.v(p::DIFFUSION) * 0.01,
            self.v(p::DENSITY) + 1.0,
        );
        for (i, sample) in feedback.into_iter().enumerate() {
            let lo_alpha = self.coefficients[26].current;
            let hi_alpha = self.coefficients[27].current;
            self.feedback_low[i] =
                clean(lo_alpha * self.feedback_low[i] + (1.0 - lo_alpha) * sample);
            // Cascaded shelves have a conservative product-gain bound. Keep
            // even crossed cutoffs and in-flight mode/ratio edits passive;
            // ordinary feedback stays below unity, flat freeze reaches unity.
            let (mid, low, high_ratio, normalizer) = shaping_gains(
                self.coefficients[i * 3].current,
                self.coefficients[i * 3 + 1].current,
                self.coefficients[i * 3 + 2].current,
            );
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
                let a = self.v(p::DIFFUSION) * 0.006;
                let old = self.allpasses[c][i]
                    .read(seconds * (0.5 + 1.5 * self.v(p::SCALE) * 0.01) * self.tank_rate);
                let output = old - a * diffuse;
                self.allpasses[c][i].push(diffuse + a * output);
                diffuse = output;
            }
            self.next[c] = reflection * self.coefficients[28].current
                + diffuse * self.coefficients[29].current;
        }
        let mid = (self.next[0] + self.next[1]) * 0.5;
        let side = (self.next[0] - self.next[1]) * 0.5 * self.v(p::STEREO) * 0.01;
        self.next = [mid + side, mid - side];
        self.spin_phase = (self.spin_phase + self.v(p::SPIN_RATE) / self.tank_rate).fract();
        self.chorus_phase = (self.chorus_phase + self.v(p::CHORUS_RATE) / self.tank_rate).fract();
    }
    fn frame(&mut self, frame: &mut StereoFrame) {
        for ramp in &mut self.parameters {
            ramp.advance();
        }
        for ramp in &mut self.coefficients {
            ramp.advance();
        }
        let input = frame.map(|v| {
            if v.is_finite() {
                v.clamp(-16.0, 16.0)
            } else {
                0.0
            }
        });
        if input != [0.0; 2] && self.v(p::FREEZE) * self.v(p::CUT) < 1.0 {
            self.active = true;
            self.silent_frames = 0;
        } else if self.active && self.v(p::FREEZE) == 0.0 {
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
                self.tank(self.sum.map(|v| v / self.factor as f32));
                self.sum = [0.0; 2];
            }
        }
        let fraction = (self.phase + 1) as f32 / self.factor as f32;
        let wet = self.v(p::DRY_WET) * 0.01;
        for c in 0..2 {
            let room = self.previous[c] + fraction * (self.next[c] - self.previous[c]);
            frame[c] = ((1.0 - wet) * input[c] + wet * room).clamp(-16.0, 16.0);
        }
    }
}
fn shaping_gains(low: f32, mid: f32, high: f32) -> (f32, f32, f32, f32) {
    let mid = mid.clamp(1e-8, 1.0);
    let envelope = mid.sqrt();
    let low = low.clamp(0.0, envelope);
    let high = high.clamp(0.0, envelope);
    let high_ratio = high / mid;
    let product = low.max(mid) * high_ratio.max(1.0);
    let normalizer = (mid.max(low).max(high) / product).min(1.0);
    (mid, low, high_ratio, normalizer)
}
fn scatter(input: [f32; 8], amount: f32, stages: f32) -> [f32; 8] {
    let mut output = input;
    for stage in 0..3 {
        let weight = (stages - stage as f32).clamp(0.0, 1.0);
        if weight == 0.0 {
            continue;
        }
        let (sin, cos) = (amount * weight * std::f32::consts::FRAC_PI_4).sin_cos();
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
        for frame in frames {
            self.frame(frame);
        }
    }
    fn reset(&mut self) {
        self.clear();
        for v in &mut self.parameters {
            *v = Ramp::steady(v.target);
        }
        for v in &mut self.coefficients {
            *v = Ramp::steady(v.target);
        }
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
        let frames = match self.settings.get(p::SMOOTH) as u32 {
            0 => 1,
            2 => (self.rate / 10).max(1),
            _ => (self.rate / 50).max(1),
        };
        for (v, p) in self.parameters.iter_mut().zip(old.parameters) {
            v.inherit(p, frames);
        }
        for (v, p) in self.coefficients.iter_mut().zip(old.coefficients) {
            v.inherit(p, frames);
        }
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
                        let y = input_filter(&mut state, x, g, 1.0 / q, high);
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
                    (energy(scatter(input, amount, stages as f32)) - energy(input)).abs() < 1e-6
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
