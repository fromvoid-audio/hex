//! Original limiting laws, explicitly separate from reference parity.
//!
//! A bounded monotonic maximum queue anticipates each channel's next lookahead
//! interval. Standard attack ramps linearly toward the required gain over the
//! lookahead window, retaining the steepest active ramp so later peaks cannot
//! postpone earlier deadlines. Soft-clip attack is instantaneous; true-peak
//! attack anticipates with an exponential time constant of one eighth of the
//! selected lookahead. Output constraints guard each mode's ceiling.
//! Release is exponential. In true-peak mode an additional 1 ms envelope
//! recovery floor suppresses modulation beyond the finite reconstruction band;
//! slower authored release times remain unchanged. Auto release uses
//! a 10 ms program envelope to select 50–270 ms recovery, latched while
//! recovering from attenuation. Link interpolates each
//! lane's required attenuation toward the worse lane. Soft clip first limits at
//! 1.15 times ceiling, then applies the independently reconstructed transfer
//! sign(x) * ceiling * sqrt(tanh((x / ceiling)^2)). This is an empirical fit to
//! original reference observations, not a proprietary algorithm claim. Maximize applies inverse-threshold gain before a
//! 0 dB limiter and scales afterward to Output.
//!
//! True peak detects signed, normalized 32-tap Blackman-windowed sinc
//! reconstruction at thirty-two phases. Thirty-one extra declared frames make every
//! detector window covering an outgoing sample available. Anticipated attack
//! avoids an instantaneous gain discontinuity before the transient. Independent
//! dense reconstructed-output tests cover modulation and intersample peaks;
//! this finite model does not prove every DAC's continuous-time behavior or
//! acoustic identity with Live's unpublished reconstruction/release laws.
//! The detector reconstructs the pre-gain input, so gain that moves inside the
//! detector window can create untracked intersample peaks. On gated-burst
//! fixtures the standard BS.1770-4 4x meter reads up to about +0.25 dB above
//! the ceiling (tests bound it at +0.3 dB) and ideal reconstruction reads
//! higher; an output-aware gain path is required to remove this.
//!
//! M/s uses one independently linked encoded stage followed by a fully linked
//! l/r safety stage after decoding, so arbitrary stereo signals respect the
//! physical output ceiling. The final guard has an independent fixed 220 ms
//! recovery; authored manual/auto release belongs to the encoded stage. This
//! bounded original law fits the measured asymmetrical-to-mono reference
//! recovery and is not a claim about unpublished implementation identity.
//! Both stages' delays are declared. No final
//! hard clamp is used. Histories, queues and telemetry commit atomically; buffer
//! copies are bounded by the prepared rate/lookahead, not callback duration.
//! Metering carries the first-stage linear attenuation with its delayed sample,
//! combines the final-stage and shaper factors for that same sample, and converts
//! each process call's minimum to dB. Each factor retains the 1e-30 floor.
//!
//! Closed transactions at the same rate retain full history for a compatible
//! layout, including silent attenuation and parameter ramps. A layout change
//! (lookahead, routing or true-peak latency) is admitted only when the donor has
//! no queued output in either stage; it explicitly resets both candidate
//! histories to the new configuration. That transition discards only silent
//! future gain/program influence. In-flight transactions and rate changes
//! refuse; the host retains its existing PDC, publication and retirement rules.
use super::state::{Settings, parameter::*};
use hex_lattice::{
    DynamicsMeter, OutputTail, OwnedProcessorRuntime, PreparedOwnedProcessorRuntime, StereoFrame,
};
use hex_lattice_contract::DeviceContractError;
use std::any::Any;

const TAPS: usize = 32;
const PHASES: usize = 32;
// Zero is the untouched-slot sentinel for unity. Every stored first-stage
// factor is floored at 1e-30, so it remains strictly positive in f32.
#[derive(Clone, Copy, Default)]
struct Sample {
    signal: [f64; 2],
    attenuation: [f32; 2],
    detector: [f64; 2],
}
#[derive(Clone, Copy, Default)]
struct Peak {
    value: f64,
    stamp: u64,
}
#[derive(Clone, Copy)]
struct Scalar {
    next: usize,
    stamp: u64,
    nonzero: usize,
    head: [usize; 2],
    len: [usize; 2],
    gain: [f64; 2],
    attack_step: [f64; 2],
    program: [f64; 2],
    input: [[f64; 2]; TAPS],
    input_next: usize,
}
impl Default for Scalar {
    fn default() -> Self {
        Self {
            next: 0,
            stamp: 0,
            nonzero: 0,
            head: [0; 2],
            len: [0; 2],
            gain: [1.0; 2],
            attack_step: [0.0; 2],
            program: [0.0; 2],
            input: [[0.0; 2]; TAPS],
            input_next: 0,
        }
    }
}
struct Stage {
    signal: Vec<Sample>,
    peaks: [Vec<Peak>; 2],
    scalar: Scalar,
}
fn allocate<T: Clone + Default>(length: usize) -> Result<Vec<T>, DeviceContractError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(length)
        .map_err(|_| DeviceContractError::RuntimeAllocation)?;
    values.resize(length, T::default());
    Ok(values)
}
impl Stage {
    fn new(length: usize) -> Result<Self, DeviceContractError> {
        Ok(Self {
            signal: allocate(length)?,
            peaks: [allocate(length)?, allocate(length)?],
            scalar: Scalar::default(),
        })
    }
    fn copy_from(&mut self, old: &Self) {
        self.signal.copy_from_slice(&old.signal);
        for (target, source) in self.peaks.iter_mut().zip(&old.peaks) {
            target.copy_from_slice(source);
        }
        self.scalar = old.scalar;
    }
    fn bytes(&self) -> usize {
        self.signal.capacity() * std::mem::size_of::<Sample>()
            + self
                .peaks
                .iter()
                .map(|p| p.capacity() * std::mem::size_of::<Peak>())
                .sum::<usize>()
    }
    fn quiet(&self) -> bool {
        self.scalar.nonzero == 0
    }
    fn reset(&mut self) {
        self.signal.fill(Sample::default());
        self.scalar = Scalar::default();
    }
    fn maximum(&mut self, lane: usize, value: f64) -> f64 {
        let s = &mut self.scalar;
        let queue = &mut self.peaks[lane];
        let capacity = queue.len();
        while s.len[lane] > 0 && s.stamp.wrapping_sub(queue[s.head[lane]].stamp) >= capacity as u64
        {
            s.head[lane] = (s.head[lane] + 1) % capacity;
            s.len[lane] -= 1;
        }
        while s.len[lane] > 0 {
            let last = (s.head[lane] + s.len[lane] - 1) % capacity;
            if queue[last].value > value {
                break;
            }
            s.len[lane] -= 1;
        }
        let tail = (s.head[lane] + s.len[lane]) % capacity;
        queue[tail] = Peak {
            value,
            stamp: s.stamp,
        };
        s.len[lane] += 1;
        queue[s.head[lane]].value
    }
    fn process(
        &mut self,
        mut sample: Sample,
        config: &Config,
        link: f64,
        physical_guard: bool,
    ) -> (Sample, [f64; 2]) {
        let s = &mut self.scalar;
        s.input[s.input_next] = sample.signal;
        s.input_next = (s.input_next + 1) % TAPS;
        let mut detector = sample.signal.map(f64::abs);
        if config.true_peak {
            for phase in &config.reconstruction {
                let mut bound = [0.0; 2];
                for (tap, coefficient) in phase.iter().enumerate() {
                    let input = s.input[(s.input_next + tap) % TAPS];
                    bound[0] += input[0] * coefficient;
                    bound[1] += input[1] * coefficient;
                }
                detector[0] = detector[0].max(bound[0].abs());
                detector[1] = detector[1].max(bound[1].abs());
            }
        }
        let maxima = [self.maximum(0, detector[0]), self.maximum(1, detector[1])];
        let worst = maxima[0].max(maxima[1]);
        sample.detector = detector;
        let s = &mut self.scalar;
        if self.signal[s.next].signal != [0.0; 2] {
            s.nonzero -= 1;
        }
        if sample.signal != [0.0; 2] {
            s.nonzero += 1;
        }
        self.signal[s.next] = sample;
        s.next = (s.next + 1) % self.signal.len();
        let mut delayed = self.signal[s.next];
        // Current-output constraints are separate from the long anticipation
        // maximum, allowing a smooth attack before the transient reaches output.
        let mut immediate = delayed.signal.map(f64::abs);
        if config.true_peak {
            for offset in 0..TAPS {
                let detector = self.signal[(s.next + offset) % self.signal.len()].detector;
                for lane in 0..2 {
                    immediate[lane] = immediate[lane].max(detector[lane]);
                }
            }
        }
        self.signal[s.next] = Sample::default();
        if delayed.signal != [0.0; 2] {
            s.nonzero -= 1;
        }
        let immediate_worst = immediate[0].max(immediate[1]);
        let mut applied = [1.0; 2];
        for lane in 0..2 {
            let peak = maxima[lane] + link * (worst - maxima[lane]);
            let target = if peak > config.ceiling {
                config.ceiling / peak
            } else {
                1.0
            };
            if target < 0.999999 {
                s.program[lane] += config.program_coefficient * ((1.0 - target) - s.program[lane]);
            } else if s.gain[lane] >= 0.999999 {
                s.program[lane] = 0.0;
            }
            let coefficient = if physical_guard {
                config.guard_release_coefficient
            } else if config.auto_release {
                let seconds = 0.05 + 0.22 * s.program[lane];
                1.0 - (-1.0 / (seconds * f64::from(config.rate))).exp()
            } else {
                config.release_coefficient
            };
            let coefficient = if config.true_peak {
                coefficient.min(config.antimodulation_coefficient)
            } else {
                coefficient
            };
            let mut gain = if target < s.gain[lane] && !config.soft_clip {
                if config.true_peak {
                    s.attack_step[lane] = 0.0;
                    s.gain[lane] + config.attack_coefficient * (target - s.gain[lane])
                } else {
                    // Never relax an in-flight ramp: a less urgent later peak
                    // cannot postpone the deadline of one already detected.
                    s.attack_step[lane] = s.attack_step[lane]
                        .max((s.gain[lane] - target) * config.attack_window_inverse);
                    (s.gain[lane] - s.attack_step[lane]).max(target)
                }
            } else {
                s.attack_step[lane] = 0.0;
                target.min(s.gain[lane] + coefficient * (target - s.gain[lane]))
            };
            let immediate_peak = immediate[lane] + link * (immediate_worst - immediate[lane]);
            if immediate_peak > config.ceiling {
                gain = gain.min(config.ceiling / immediate_peak);
            }
            if gain <= target {
                s.attack_step[lane] = 0.0;
            }
            s.gain[lane] = gain;
            applied[lane] = gain;
        }
        for (lane, gain) in applied.into_iter().enumerate() {
            delayed.signal[lane] *= gain;
        }
        s.stamp = s.stamp.wrapping_add(1);
        (delayed, applied)
    }
}
fn reduction(attenuation: f64) -> f32 {
    (-20.0 * attenuation.log10()).max(0.0) as f32
}
fn meter_factor(gain: f64) -> f64 {
    if gain > 1.0 { 1.0 } else { gain.max(1e-30) }
}
fn amplitude(db: f32) -> f64 {
    10.0_f64.powf(f64::from(db) / 20.0)
}
#[derive(Clone, Copy)]
struct Config {
    rate: u32,
    ceiling: f64,
    output_ceiling: f64,
    gain: f64,
    output: f64,
    release_coefficient: f64,
    guard_release_coefficient: f64,
    attack_coefficient: f64,
    attack_window_inverse: f64,
    antimodulation_coefficient: f64,
    program_coefficient: f64,
    auto_release: bool,
    true_peak: bool,
    soft_clip: bool,
    mid_side: bool,
    link: f64,
    delay: usize,
    reconstruction: [[f64; TAPS]; PHASES],
}
impl Config {
    fn new(settings: Settings, rate: u32) -> Self {
        let maximize = settings.flag(MAXIMIZE);
        let true_peak = settings.choice(MODE) == 2;
        let lookahead = [1.5, 3.0, 6.0][settings.choice(LOOKAHEAD) as usize];
        let delay = (lookahead * f64::from(rate) / 1000.0).ceil() as usize
            + if true_peak { TAPS - 1 } else { 0 };
        let reconstruction = std::array::from_fn(|phase| {
            let position = (TAPS / 2 - 1) as f64 + phase as f64 / PHASES as f64;
            let mut coefficients = std::array::from_fn(|tap| {
                let distance = position - tap as f64;
                let sinc = if distance.abs() < 1e-12 {
                    1.0
                } else {
                    (std::f64::consts::PI * distance).sin() / (std::f64::consts::PI * distance)
                };
                let window = 0.42
                    + 0.5 * (std::f64::consts::PI * distance / (TAPS / 2) as f64).cos()
                    + 0.08 * (2.0 * std::f64::consts::PI * distance / (TAPS / 2) as f64).cos();
                sinc * window
            });
            let sum: f64 = coefficients.iter().sum();
            for c in &mut coefficients {
                *c /= sum;
            }
            coefficients
        });
        Self {
            rate,
            ceiling: (if maximize {
                1.0
            } else {
                amplitude(settings.value(CEILING))
            }) * if settings.choice(MODE) == 1 {
                1.15
            } else {
                1.0
            },
            output_ceiling: if maximize {
                1.0
            } else {
                amplitude(settings.value(CEILING))
            },
            gain: if maximize {
                amplitude(-settings.value(THRESHOLD))
            } else {
                amplitude(settings.value(GAIN))
            },
            output: if maximize {
                amplitude(settings.value(OUTPUT))
            } else {
                1.0
            },
            antimodulation_coefficient: 1.0 - (-1.0 / (0.001 * f64::from(rate))).exp(),
            attack_coefficient: 1.0
                - (-8.0 / (delay - if true_peak { TAPS - 1 } else { 0 }) as f64).exp(),
            attack_window_inverse: 1.0 / (delay + 1) as f64,
            guard_release_coefficient: 1.0 - (-1.0 / (0.22 * f64::from(rate))).exp(),
            release_coefficient: 1.0
                - (-1000.0 / (f64::from(settings.value(RELEASE)) * f64::from(rate))).exp(),
            program_coefficient: 1.0 - (-1.0 / (0.01 * f64::from(rate))).exp(),
            auto_release: settings.flag(AUTO_RELEASE),
            true_peak,
            soft_clip: settings.choice(MODE) == 1,
            mid_side: settings.choice(ROUTING) == 1,
            link: f64::from(settings.value(if settings.choice(ROUTING) == 1 {
                LINK_MID_SIDE
            } else {
                LINK
            })),
            delay,
            reconstruction,
        }
    }
    fn latency(self) -> u32 {
        (self.delay * if self.mid_side { 2 } else { 1 }) as u32
    }
}
pub(super) struct Factory {
    config: Config,
}
impl Factory {
    pub(super) fn new(settings: Settings, rate: u32) -> Self {
        Self {
            config: Config::new(settings, rate),
        }
    }
}
impl PreparedOwnedProcessorRuntime for Factory {
    fn reserved_bypass_latency_frames(&self) -> Option<u32> {
        Some(self.config.latency())
    }
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_pair_bytes(&self) -> usize {
        std::mem::size_of::<Runtime>()
            + 2 * if self.config.mid_side { 2 } else { 1 }
                * (self.config.delay + 1)
                * (std::mem::size_of::<Sample>() + 2 * std::mem::size_of::<Peak>())
    }
    fn create_runtime(&self) -> Result<Box<dyn OwnedProcessorRuntime>, DeviceContractError> {
        let runtime = Runtime {
            config: self.config,
            primary: History::new(self.config)?,
            scratch: History::new(self.config)?,
            open: false,
        };
        if runtime.fixed_storage_pair_bytes() != Some(self.declared_pair_bytes()) {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }
        Ok(Box::new(runtime))
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite { max_tail_frames: 0 }
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(self.config.latency())
    }
}
struct History {
    first: Stage,
    final_stage: Option<Stage>,
    meter: DynamicsMeter,
    gain: f64,
    output: f64,
}
impl History {
    fn new(config: Config) -> Result<Self, DeviceContractError> {
        Ok(Self {
            first: Stage::new(config.delay + 1)?,
            final_stage: if config.mid_side {
                Some(Stage::new(config.delay + 1)?)
            } else {
                None
            },
            meter: DynamicsMeter::default(),
            gain: config.gain,
            output: config.output,
        })
    }
    fn copy_from(&mut self, old: &Self) {
        self.first.copy_from(&old.first);
        if let (Some(target), Some(source)) = (&mut self.final_stage, &old.final_stage) {
            target.copy_from(source);
        }
        self.meter = old.meter;
        self.gain = old.gain;
        self.output = old.output;
    }
    fn quiet(&self) -> bool {
        self.first.quiet() && self.final_stage.as_ref().is_none_or(Stage::quiet)
    }
    fn clear_attack(&mut self) {
        self.first.scalar.attack_step = [0.0; 2];
        if let Some(stage) = &mut self.final_stage {
            stage.scalar.attack_step = [0.0; 2];
        }
    }
    fn reset(&mut self, config: Config) {
        self.first.reset();
        if let Some(stage) = &mut self.final_stage {
            stage.reset();
        }
        self.meter = DynamicsMeter::default();
        self.gain = config.gain;
        self.output = config.output;
    }
    fn bytes(&self) -> usize {
        self.first.bytes() + self.final_stage.as_ref().map_or(0, Stage::bytes)
    }
}
struct Runtime {
    config: Config,
    primary: History,
    scratch: History,
    open: bool,
}
impl Runtime {
    fn same_history_layout(&self, previous: &Self) -> bool {
        self.config.delay == previous.config.delay
            && self.config.mid_side == previous.config.mid_side
            && self.config.true_peak == previous.config.true_peak
    }
}
impl OwnedProcessorRuntime for Runtime {
    fn fixed_storage_pair_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>()
            .checked_add(self.primary.bytes())?
            .checked_add(self.scratch.bytes())
    }
    fn begin_block(&mut self) {
        self.scratch.copy_from(&self.primary);
        self.scratch.meter = DynamicsMeter::default();
        self.open = true;
    }
    fn process_block(&mut self, frames: &mut [StereoFrame], _: &dyn hex_lattice::DeviceResources) {
        if !self.open {
            return;
        }
        let config = &self.config;
        let history = &mut self.scratch;
        let mut post_input_peak = 0.0_f64;
        let mut min_attenuation = [1.0_f64; 2];
        for frame in frames {
            // Compatible edits ramp input/output gain over 5 ms. A falling ceiling
            // is immediate, preserving its safety invariant on the first sample.
            history.gain += (config.gain - history.gain) / (0.005 * f64::from(config.rate));
            history.output += (config.output - history.output) / (0.005 * f64::from(config.rate));
            // Output gain is never above the authored target after a downward edit.
            history.output = history.output.min(config.output);
            let mut signal = frame.map(|v| {
                if v.is_finite() {
                    f64::from(v) * history.gain
                } else {
                    0.0
                }
            });
            post_input_peak = post_input_peak.max(signal[0].abs()).max(signal[1].abs());
            if config.mid_side {
                signal = [(signal[0] + signal[1]) * 0.5, (signal[0] - signal[1]) * 0.5];
            }
            let (mut sample, first_gain) = history.first.process(
                Sample {
                    signal,
                    ..Sample::default()
                },
                config,
                config.link,
                false,
            );
            let mut attenuation = first_gain.map(meter_factor);
            if let Some(final_stage) = &mut history.final_stage {
                // The final stage delays this exact first-stage factor together
                // with its signal; its own factor is applied only on output.
                sample.attenuation = attenuation.map(|gain| gain as f32);
                sample.signal = [
                    sample.signal[0] + sample.signal[1],
                    sample.signal[0] - sample.signal[1],
                ];
                let (delayed, final_gain) = final_stage.process(sample, config, 1.0, true);
                sample = delayed;
                attenuation = std::array::from_fn(|lane| {
                    (if sample.attenuation[lane] == 0.0 {
                        1.0
                    } else {
                        f64::from(sample.attenuation[lane])
                    }) * meter_factor(final_gain[lane])
                });
            }
            if config.soft_clip {
                let ratios = sample.signal.map(|v| {
                    if v == 0.0 {
                        1.0
                    } else {
                        let normalized = v.abs() / config.output_ceiling;
                        (normalized * normalized).tanh().sqrt() / normalized
                    }
                });
                // A shared output shaper in m/s preserves encoded lane balance;
                // its common attenuation can be reported truthfully in both lanes.
                let ratios = if config.mid_side {
                    [ratios[0].min(ratios[1]); 2]
                } else {
                    ratios
                };
                history.meter.clipped |= sample
                    .signal
                    .iter()
                    .any(|v| v.abs() > config.output_ceiling);
                for (lane, ratio) in ratios.into_iter().enumerate() {
                    sample.signal[lane] *= ratio;
                    attenuation[lane] *= meter_factor(ratio);
                }
            }
            *frame = sample.signal.map(|v| (v * history.output) as f32);
            for lane in 0..2 {
                min_attenuation[lane] = min_attenuation[lane].min(attenuation[lane]);
            }
        }
        history.meter.post_input_peak = history.meter.post_input_peak.max(post_input_peak as f32);
        for (lane, attenuation) in min_attenuation.into_iter().enumerate() {
            history.meter.reduction_db[lane] =
                history.meter.reduction_db[lane].max(reduction(attenuation));
        }
    }
    fn process_bypassed_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        resources: &dyn hex_lattice::DeviceResources,
    ) {
        // Bounded stack scratch preserves dry input without allocating, even
        // when the host supplies an arbitrarily large segment.
        let mut scratch = [[0.0; 2]; 256];
        for chunk in frames.chunks(256) {
            let work = &mut scratch[..chunk.len()];
            work.copy_from_slice(chunk);
            self.process_block(work, resources);
        }
    }
    fn commit_block(&mut self) {
        if self.open {
            std::mem::swap(&mut self.primary, &mut self.scratch);
            self.open = false;
        }
    }
    fn rollback_block(&mut self) {
        self.open = false;
    }
    fn reset(&mut self) {
        self.primary.reset(self.config);
        self.scratch.reset(self.config);
        self.open = false;
    }
    fn committed_is_quiescent(&self) -> bool {
        self.primary.quiet()
    }
    fn speculative_is_quiescent(&self) -> bool {
        if self.open {
            self.scratch.quiet()
        } else {
            self.primary.quiet()
        }
    }
    fn can_inherit_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        previous.as_any().downcast_ref::<Self>().is_some_and(|old| {
            !self.open
                && !old.open
                && self.config.rate == old.config.rate
                && (self.same_history_layout(old) || old.primary.quiet())
        })
    }
    fn inherit_from(&mut self, previous: &dyn OwnedProcessorRuntime) {
        if !self.can_inherit_from(previous) {
            return;
        }
        if let Some(old) = previous.as_any().downcast_ref::<Self>() {
            if self.same_history_layout(old) {
                self.primary.copy_from(&old.primary);
                self.scratch.copy_from(&old.primary);
                if self.config.soft_clip != old.config.soft_clip {
                    self.primary.clear_attack();
                    self.scratch.clear_attack();
                }
            } else {
                // Admission proved that no donor audio remains queued. A new
                // layout starts with its own configuration, including both
                // histories even when this candidate was previously rendered.
                self.reset();
            }
        }
    }
    fn can_retarget_parameters_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        self.can_inherit_from(previous)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn take_dynamics_meter(&mut self) -> Option<DynamicsMeter> {
        let result = self.primary.meter;
        self.primary.meter = DynamicsMeter::default();
        Some(result)
    }
}
