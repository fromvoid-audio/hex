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
//! this finite model does not prove every DAC's continuous-time behavior.
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
//! The classic factory's closed transactions at the same rate retain full
//! history for a compatible layout, including silent attenuation and parameter ramps. A layout change
//! (lookahead, routing or true-peak latency) is admitted only when the donor has
//! no queued output in either stage; it explicitly resets both candidate
//! histories to the new configuration. Physical storage layouts must also match
//! before copying a nonempty history. That transition discards only silent
//! future gain/program influence. In-flight transactions and rate changes
//! refuse; the host retains its existing PDC, publication and retirement rules.
use super::automation;
use super::state::{Settings, parameter::*};
use hex_lattice::{
    DynamicsMeter, OutputTail, OwnedProcessorRuntime, PreparedOwnedProcessorRuntime, StereoFrame,
};
use hex_lattice_contract::DeviceContractError;
use std::{any::Any, sync::OnceLock};

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
    // Doubled planar ring buffer so `[input_next..input_next + TAPS]` is a
    // contiguous 32-tap slice per lane with zero modulo indexing.
    input: [[f64; TAPS * 2]; 2],
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
            input: [[0.0; TAPS * 2]; 2],
            input_next: 0,
        }
    }
}
#[derive(Clone, Copy)]
struct StageDirty {
    full: bool,
    signal_start: usize,
    signal_len: usize,
    peaks_base_head: [usize; 2],
    peaks_head_off: [usize; 2],
    peaks_min: [usize; 2],
    peaks_max: [usize; 2],
}
impl Default for StageDirty {
    fn default() -> Self {
        Self {
            full: false,
            signal_start: 0,
            signal_len: 0,
            peaks_base_head: [0; 2],
            peaks_head_off: [0; 2],
            peaks_min: [usize::MAX; 2],
            peaks_max: [0; 2],
        }
    }
}
impl StageDirty {
    fn begin(scalar: &Scalar) -> Self {
        Self {
            full: false,
            signal_start: scalar.next,
            signal_len: 0,
            peaks_base_head: scalar.head,
            peaks_head_off: [0; 2],
            peaks_min: [usize::MAX; 2],
            peaks_max: [0; 2],
        }
    }
    fn has_writes(&self) -> bool {
        self.full
            || self.signal_len > 0
            || self.peaks_min[0] < self.peaks_max[0]
            || self.peaks_min[1] < self.peaks_max[1]
    }
}
fn copy_cyclic<T: Copy>(
    source: &[T],
    target: &mut [T],
    start: usize,
    length: usize,
    ring_len: usize,
) {
    if length == 0 {
        return;
    }
    if length >= ring_len {
        target[..ring_len].copy_from_slice(&source[..ring_len]);
        return;
    }
    let first = length.min(ring_len - start);
    target[start..start + first].copy_from_slice(&source[start..start + first]);
    let rest = length - first;
    if rest > 0 {
        target[..rest].copy_from_slice(&source[..rest]);
    }
}
#[inline(always)]
fn dot32(x: &[f64], h: &[f64; TAPS]) -> f64 {
    let mut a0 = 0.0;
    let mut a1 = 0.0;
    let mut a2 = 0.0;
    let mut a3 = 0.0;
    let mut i = 0;
    while i < TAPS {
        a0 += x[i] * h[i];
        a1 += x[i + 1] * h[i + 1];
        a2 += x[i + 2] * h[i + 2];
        a3 += x[i + 3] * h[i + 3];
        i += 4;
    }
    (a0 + a1) + (a2 + a3)
}
/// Coarse-to-fine peak search over the exact 32-phase, 32-tap Blackman-windowed
/// sinc reconstruction for one lane. Evaluates phase 0/32 directly from the
/// sample buffer, probes 4x phases (8, 16, 24), checks 8x midpoints only on
/// brackets that can beat the coarse peak, then refines at stride 2 and 1.
#[inline]
fn true_peak_lane(x: &[f64], phases: &[[f64; TAPS]; PHASES]) -> f64 {
    let v0 = x[15].abs();
    let v8 = dot32(x, &phases[8]).abs();
    let v16 = dot32(x, &phases[16]).abs();
    let v24 = dot32(x, &phases[24]).abs();
    let v32 = x[16].abs();
    let mut best = v0.max(v8).max(v16).max(v24).max(v32);
    if best == 0.0 {
        return 0.0;
    }
    let mut grid = [v0, 0.0, v8, 0.0, v16, 0.0, v24, 0.0, v32];
    // For any bandlimited signal (<= Nyquist), a 1/4-sample bracket's higher
    // endpoint is at least cos(pi/8) ~= 0.9239 of an interior peak; 0.85 adds
    // margin for sharp envelopes while rejecting non-peak brackets.
    let coarse_threshold = best * 0.85;
    for (idx, phase) in [(1usize, 4usize), (3, 12), (5, 20), (7, 28)] {
        if grid[idx - 1].max(grid[idx + 1]) >= coarse_threshold {
            let v = dot32(x, &phases[phase]).abs();
            grid[idx] = v;
            if v > best {
                best = v;
            }
        }
    }
    // At 8x spacing (1/8 sample), cos(pi/16) ~= 0.9808; 0.95 ensures every
    // candidate 8x neighborhood that could exceed `best` is refined to 32x.
    let fine_threshold = best * 0.95;
    for (idx, &val) in grid.iter().enumerate() {
        if val < fine_threshold {
            continue;
        }
        let mut center = idx * 4;
        for stride in [2usize, 1] {
            let mut next_center = center;
            if center >= stride {
                let p = center - stride;
                if p > 0 && p < PHASES {
                    let v = dot32(x, &phases[p]).abs();
                    if v > best {
                        best = v;
                        next_center = p;
                    }
                }
            }
            let p = center + stride;
            if p > 0 && p < PHASES {
                let v = dot32(x, &phases[p]).abs();
                if v > best {
                    best = v;
                    next_center = p;
                }
            }
            center = next_center;
        }
    }
    best
}
struct Stage {
    signal: Vec<Sample>,
    peaks: [Vec<Peak>; 2],
    scalar: Scalar,
    dirty: StageDirty,
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
            dirty: StageDirty::default(),
        })
    }
    fn begin_block(&mut self) {
        self.dirty = StageDirty::begin(&self.scalar);
    }
    fn copy_from(&mut self, old: &Self) {
        self.signal.copy_from_slice(&old.signal);
        for (target, source) in self.peaks.iter_mut().zip(&old.peaks) {
            target.copy_from_slice(source);
        }
        self.scalar = old.scalar;
        self.dirty = StageDirty::default();
    }
    fn sync_from(&mut self, source: &Self, dirty: &StageDirty, window: usize) {
        if dirty.full {
            self.signal.copy_from_slice(&source.signal);
            for (target, src) in self.peaks.iter_mut().zip(&source.peaks) {
                target.copy_from_slice(src);
            }
        } else {
            copy_cyclic(
                &source.signal,
                &mut self.signal,
                dirty.signal_start,
                dirty.signal_len,
                window,
            );
            for lane in 0..2 {
                if dirty.peaks_min[lane] < dirty.peaks_max[lane] {
                    let capacity = self.peaks[lane].len();
                    let span = dirty.peaks_max[lane] - dirty.peaks_min[lane];
                    let start = (dirty.peaks_base_head[lane] + dirty.peaks_min[lane]) % capacity;
                    copy_cyclic(
                        &source.peaks[lane],
                        &mut self.peaks[lane],
                        start,
                        span,
                        capacity,
                    );
                }
            }
        }
        self.scalar = source.scalar;
        self.dirty = StageDirty::default();
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
    fn is_idle_unity(&self, config: &Config) -> bool {
        let s = &self.scalar;
        if s.nonzero != 0
            || s.gain != [1.0; 2]
            || s.program != [0.0; 2]
            || s.attack_step != [0.0; 2]
            || (s.len[0] > 0 && self.peaks[0][s.head[0]].value != 0.0)
            || (s.len[1] > 0 && self.peaks[1][s.head[1]].value != 0.0)
            || s.input[0][..TAPS].iter().any(|&v| v != 0.0)
            || s.input[1][..TAPS].iter().any(|&v| v != 0.0)
        {
            return false;
        }
        if config.true_peak {
            let window = config.delay + 1;
            if self.signal[..window]
                .iter()
                .any(|sample| sample.detector != [0.0; 2])
            {
                return false;
            }
        }
        true
    }
    fn advance_silent(&mut self, count: usize, window: usize) {
        if self.dirty.has_writes() {
            self.dirty.full = true;
        }
        let s = &mut self.scalar;
        s.next = (s.next + count) % window;
        s.input_next = (s.input_next + count) % TAPS;
        s.stamp = s.stamp.wrapping_add(count as u64);
        s.len = [0, 0];
    }
    fn reset(&mut self) {
        self.signal.fill(Sample::default());
        self.scalar = Scalar::default();
        self.dirty.full = true;
    }
    fn same_storage_layout(&self, previous: &Self) -> bool {
        self.signal.len() == previous.signal.len()
            && self
                .peaks
                .iter()
                .zip(&previous.peaks)
                .all(|(a, b)| a.len() == b.len())
    }
    #[inline(always)]
    fn maximum(&mut self, lane: usize, value: f64, window: usize) -> f64 {
        let s = &mut self.scalar;
        let queue = &mut self.peaks[lane];
        let capacity = queue.len();
        while s.len[lane] > 0 && s.stamp.wrapping_sub(queue[s.head[lane]].stamp) >= window as u64 {
            let next_head = s.head[lane] + 1;
            s.head[lane] = if next_head == capacity { 0 } else { next_head };
            s.len[lane] -= 1;
            self.dirty.peaks_head_off[lane] = self.dirty.peaks_head_off[lane].saturating_add(1);
        }
        while s.len[lane] > 0 {
            let mut last = s.head[lane] + s.len[lane] - 1;
            if last >= capacity {
                last -= capacity;
            }
            if queue[last].value > value {
                break;
            }
            s.len[lane] -= 1;
        }
        let pos = self.dirty.peaks_head_off[lane].saturating_add(s.len[lane]);
        self.dirty.peaks_min[lane] = self.dirty.peaks_min[lane].min(pos);
        self.dirty.peaks_max[lane] = self.dirty.peaks_max[lane].max(pos.saturating_add(1));
        let mut tail = s.head[lane] + s.len[lane];
        if tail >= capacity {
            tail -= capacity;
        }
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
        parameters: &Parameters,
        link: f64,
        physical_guard: bool,
    ) -> (Sample, [f64; 2]) {
        let s = &mut self.scalar;
        let write_idx = s.input_next;
        s.input[0][write_idx] = sample.signal[0];
        s.input[0][write_idx + TAPS] = sample.signal[0];
        s.input[1][write_idx] = sample.signal[1];
        s.input[1][write_idx + TAPS] = sample.signal[1];
        s.input_next = if write_idx + 1 == TAPS {
            0
        } else {
            write_idx + 1
        };
        let mut detector = sample.signal.map(f64::abs);
        if config.true_peak {
            let start = s.input_next;
            let p0 = true_peak_lane(&s.input[0][start..start + TAPS], config.reconstruction);
            let p1 = true_peak_lane(&s.input[1][start..start + TAPS], config.reconstruction);
            detector[0] = detector[0].max(p0);
            detector[1] = detector[1].max(p1);
        }
        // Construction validates this logical window against both backing
        // stores. Physical capacity does not define latency or peak lifetime.
        let window = config.delay + 1;
        let maxima = [
            self.maximum(0, detector[0], window),
            self.maximum(1, detector[1], window),
        ];
        let worst = maxima[0].max(maxima[1]);
        sample.detector = detector;
        let s = &mut self.scalar;
        let old_next = s.next;
        if self.dirty.signal_len == 0 {
            self.dirty.signal_start = old_next;
            self.dirty.signal_len = 2.min(window);
        } else {
            self.dirty.signal_len = (self.dirty.signal_len + 1).min(window);
        }
        if self.signal[old_next].signal != [0.0; 2] {
            s.nonzero -= 1;
        }
        if sample.signal != [0.0; 2] {
            s.nonzero += 1;
        }
        self.signal[old_next] = sample;
        s.next = if old_next + 1 == window {
            0
        } else {
            old_next + 1
        };
        let mut delayed = self.signal[s.next];
        // Current-output constraints are separate from the long anticipation
        // maximum, allowing a smooth attack before the transient reaches output.
        let mut immediate = delayed.signal.map(f64::abs);
        if config.true_peak {
            let first = TAPS.min(window - s.next);
            for slot in &self.signal[s.next..s.next + first] {
                immediate[0] = immediate[0].max(slot.detector[0]);
                immediate[1] = immediate[1].max(slot.detector[1]);
            }
            for slot in &self.signal[..TAPS - first] {
                immediate[0] = immediate[0].max(slot.detector[0]);
                immediate[1] = immediate[1].max(slot.detector[1]);
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
            let target = if peak > parameters.ceiling {
                parameters.ceiling / peak
            } else {
                1.0
            };
            if target < 0.999999 {
                s.program[lane] += config.program_coefficient * ((1.0 - target) - s.program[lane]);
            } else if s.gain[lane] >= 0.999999 {
                s.program[lane] = 0.0;
            }
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
                if target == s.gain[lane] {
                    target
                } else {
                    let coefficient = if physical_guard {
                        config.guard_release_coefficient
                    } else if parameters.auto_release {
                        // x = 1 / (seconds * rate) <= 0.0025 (seconds >= 0.05, rate >= 8 kHz).
                        // 4-term Horner series for 1 - exp(-x) has max error < 1e-15.
                        let x = config.inv_rate / (0.05 + 0.22 * s.program[lane]);
                        x * (1.0 - x * (0.5 - x * ((1.0 / 6.0) - x * (1.0 / 24.0))))
                    } else {
                        parameters.release_coefficient
                    };
                    let coefficient = if config.true_peak {
                        coefficient.min(config.antimodulation_coefficient)
                    } else {
                        coefficient
                    };
                    target.min(s.gain[lane] + coefficient * (target - s.gain[lane]))
                }
            };
            let immediate_peak = immediate[lane] + link * (immediate_worst - immediate[lane]);
            if immediate_peak > parameters.ceiling {
                gain = gain.min(parameters.ceiling / immediate_peak);
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
/// Effective limiting targets. Derivation never rebuilds fixed reconstruction
/// coefficients or changes the prepared latency/layout. Cache and targets live
/// in each transaction history, so refusal/rollback restores both together.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Parameters {
    ceiling: f64,
    output_ceiling: f64,
    gain: f64,
    output: f64,
    release_coefficient: f64,
    auto_release: bool,
    link: f64,
}
impl Parameters {
    fn new(settings: Settings, rate: u32) -> Self {
        let maximize = settings.flag(MAXIMIZE);
        let output_ceiling = if maximize {
            1.0
        } else {
            amplitude(settings.value(CEILING))
        };
        Self {
            ceiling: output_ceiling,
            output_ceiling,
            gain: amplitude(
                settings.value(if maximize { THRESHOLD } else { GAIN })
                    * if maximize { -1.0 } else { 1.0 },
            ),
            output: if maximize {
                amplitude(settings.value(OUTPUT))
            } else {
                1.0
            },
            release_coefficient: 1.0
                - (-1000.0 / (f64::from(settings.value(RELEASE)) * f64::from(rate))).exp(),
            auto_release: settings.flag(AUTO_RELEASE),
            link: 0.0,
        }
        .for_layout(settings)
    }
    /// Project branch-local selectors onto rate/value coefficients already derived.
    fn for_layout(mut self, settings: Settings) -> Self {
        self.ceiling = self.output_ceiling
            * if settings.choice(MODE) == 1 {
                1.15
            } else {
                1.0
            };
        self.link = f64::from(settings.value(if settings.choice(ROUTING) == 1 {
            LINK_MID_SIDE
        } else {
            LINK
        }));
        self
    }
}
/// Fixed program data, independent of rate, state and layout. Config preparation
/// initializes this on control; callback processing reads only its stored static
/// reference, without touching OnceLock. No host resource custody is transferred.
fn reconstruction_coefficients() -> &'static [[f64; TAPS]; PHASES] {
    static COEFFICIENTS: OnceLock<[[f64; TAPS]; PHASES]> = OnceLock::new();
    COEFFICIENTS.get_or_init(|| {
        std::array::from_fn(|phase| {
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
        })
    })
}

#[derive(Clone, Copy)]
struct Config {
    rate: u32,
    inv_rate: f64,
    gain_ramp_inv: f64,
    settings: Settings,
    parameters: Parameters,
    guard_release_coefficient: f64,
    attack_coefficient: f64,
    attack_window_inverse: f64,
    antimodulation_coefficient: f64,
    program_coefficient: f64,
    true_peak: bool,
    soft_clip: bool,
    mid_side: bool,
    delay: usize,
    reconstruction: &'static [[f64; TAPS]; PHASES],
}
impl Config {
    fn new(settings: Settings, rate: u32) -> Self {
        let true_peak = settings.choice(MODE) == 2;
        let lookahead = [1.5, 3.0, 6.0][settings.choice(LOOKAHEAD) as usize];
        let delay = (lookahead * f64::from(rate) / 1000.0).ceil() as usize
            + if true_peak { TAPS - 1 } else { 0 };
        let reconstruction = reconstruction_coefficients();
        let inv_rate = 1.0 / f64::from(rate);
        Self {
            rate,
            inv_rate,
            gain_ramp_inv: inv_rate / 0.005,
            settings,
            parameters: Parameters::new(settings, rate),
            antimodulation_coefficient: 1.0 - (-1.0 / (0.001 * f64::from(rate))).exp(),
            attack_coefficient: 1.0
                - (-8.0 / (delay - if true_peak { TAPS - 1 } else { 0 }) as f64).exp(),
            attack_window_inverse: 1.0 / (delay + 1) as f64,
            guard_release_coefficient: 1.0 - (-1.0 / (0.22 * f64::from(rate))).exp(),
            program_coefficient: 1.0 - (-1.0 / (0.01 * f64::from(rate))).exp(),
            true_peak,
            soft_clip: settings.choice(MODE) == 1,
            mid_side: settings.choice(ROUTING) == 1,
            delay,
            reconstruction,
        }
    }
    fn latency(self) -> u32 {
        (self.delay * if self.mid_side { 2 } else { 1 }) as u32
    }
}
/// Fixed-layout factory for the existing owned Limiter transaction.
pub struct Factory {
    config: Config,
    admitted: [bool; automation::COUNT],
}
impl Factory {
    pub(super) fn new(settings: Settings, rate: u32) -> Self {
        Self {
            config: Config::new(settings, rate),
            admitted: [false; automation::COUNT],
        }
    }
}
impl Factory {
    pub(super) fn automated(
        settings: Settings,
        rate: u32,
        admitted: [bool; automation::COUNT],
    ) -> Self {
        Self {
            config: Config::new(settings, rate),
            admitted,
        }
    }
}
impl PreparedOwnedProcessorRuntime for Factory {
    fn supports_parameter_automation(&self, id: hex_lattice_contract::ParameterId) -> bool {
        automation::slot(id).is_some_and(|i| self.admitted[i])
    }
    fn parameter_automation_descriptor(
        &self,
        id: hex_lattice_contract::ParameterId,
    ) -> Option<hex_lattice_contract::ParameterDescriptor> {
        self.supports_parameter_automation(id)
            .then(|| automation::descriptor(id))
            .flatten()
    }

    fn reserved_bypass_latency_frames(&self) -> Option<u32> {
        Some(self.config.latency())
    }
    fn supports_power_off(&self) -> bool {
        true
    }
    fn parameter_modulation_semantics(
        &self,
        id: hex_lattice_contract::ParameterId,
    ) -> Option<hex_lattice_contract::ParameterSemantics> {
        self.supports_parameter_automation(id)
            .then(|| automation::modulation_semantics(id))
            .flatten()
    }

    fn runtime_version(&self) -> u32 {
        2
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
            primary: History::new(self.config, self.admitted)?,
            scratch: History::new(self.config, self.admitted)?,
            open: false,
            releasing: false,
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
    controls: crate::parameter_binding::Controls<Settings, { automation::COUNT }>,
    parameters: Parameters,
    first: Stage,
    final_stage: Option<Stage>,
    meter: DynamicsMeter,
    gain: f64,
    output: f64,
}
impl History {
    fn new(
        config: Config,
        admitted: [bool; automation::COUNT],
    ) -> Result<Self, DeviceContractError> {
        Self::with_capacity(config, admitted, config.delay + 1, config.mid_side)
    }
    /// Allocate bounded backing independently of the initial logical window.
    /// An unused final stage may be reserved; it stays silent until its owning
    /// layout is selected after a reset. No processing operation resizes storage.
    fn with_capacity(
        config: Config,
        admitted: [bool; automation::COUNT],
        capacity: usize,
        reserve_final_stage: bool,
    ) -> Result<Self, DeviceContractError> {
        if !(8000..=192000).contains(&config.rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let maximum = usize::try_from((u64::from(config.rate) * 6).div_ceil(1000))
            .map_err(|_| DeviceContractError::InvalidParameterValue)?
            .checked_add(TAPS)
            .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
        if capacity < config.delay + 1
            || capacity > maximum
            || (config.mid_side && !reserve_final_stage)
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(Self {
            controls: crate::parameter_binding::Controls::new(config.settings, admitted),
            parameters: config.parameters,
            first: Stage::new(capacity)?,
            final_stage: if reserve_final_stage {
                Some(Stage::new(capacity)?)
            } else {
                None
            },
            meter: DynamicsMeter::default(),
            gain: config.parameters.gain,
            output: config.parameters.output,
        })
    }
    fn same_storage_layout(&self, previous: &Self) -> bool {
        self.first.same_storage_layout(&previous.first)
            && match (&self.final_stage, &previous.final_stage) {
                (Some(a), Some(b)) => a.same_storage_layout(b),
                (None, None) => true,
                _ => false,
            }
    }
    fn begin_block(&mut self) {
        self.first.begin_block();
        if let Some(stage) = &mut self.final_stage {
            stage.begin_block();
        }
        self.meter = DynamicsMeter::default();
    }
    fn commit_from(&mut self, scratch: &mut Self, window: usize) {
        self.controls = scratch.controls;
        self.parameters = scratch.parameters;
        let first_dirty = scratch.first.dirty;
        self.first.sync_from(&scratch.first, &first_dirty, window);
        scratch.first.dirty = StageDirty::default();
        if let (Some(target), Some(source)) = (&mut self.final_stage, &mut scratch.final_stage) {
            let final_dirty = source.dirty;
            target.sync_from(source, &final_dirty, window);
            source.dirty = StageDirty::default();
        }
        self.meter = scratch.meter;
        self.gain = scratch.gain;
        self.output = scratch.output;
    }
    fn rollback_from(&mut self, committed: &Self, window: usize) {
        self.controls = committed.controls;
        self.parameters = committed.parameters;
        let first_dirty = self.first.dirty;
        self.first.sync_from(&committed.first, &first_dirty, window);
        if let (Some(target), Some(source)) = (&mut self.final_stage, &committed.final_stage) {
            let final_dirty = target.dirty;
            target.sync_from(source, &final_dirty, window);
        }
        self.meter = committed.meter;
        self.gain = committed.gain;
        self.output = committed.output;
    }
    fn copy_from(&mut self, old: &Self) {
        self.controls = old.controls;
        self.parameters = old.parameters;
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
        self.controls.reset();
        self.parameters = config.parameters;
        self.gain = config.parameters.gain;
        self.output = config.parameters.output;
    }
    fn bytes(&self) -> usize {
        self.first.bytes() + self.final_stage.as_ref().map_or(0, Stage::bytes)
    }
}
/// Original fixed-layout runtime; fields and history custody remain private.
pub struct Runtime {
    config: Config,
    primary: History,
    scratch: History,
    open: bool,
    releasing: bool,
}
impl Runtime {
    /// Borrow the exact inline owner and its committed/scratch heap storage.
    /// This control-side observation transfers no runtime or buffer ownership.
    pub fn control_storage(&self) -> Option<(usize, [usize; 2])> {
        Some((
            std::mem::size_of::<Self>(),
            [self.primary.bytes(), self.scratch.bytes()],
        ))
    }
    fn same_history_layout(&self, previous: &Self) -> bool {
        self.config.delay == previous.config.delay
            && self.config.mid_side == previous.config.mid_side
            && self.config.true_peak == previous.config.true_peak
            && self.primary.same_storage_layout(&previous.primary)
            && self.scratch.same_storage_layout(&previous.scratch)
    }
}
struct BlockStats {
    post_input_peak: f64,
    min_attenuation: [f64; 2],
}
impl BlockStats {
    fn new() -> Self {
        Self {
            post_input_peak: 0.0,
            min_attenuation: [1.0; 2],
        }
    }
    fn finish(self, history: &mut History) {
        self.finish_meter(&mut history.meter);
    }
    fn finish_meter(self, meter: &mut DynamicsMeter) {
        meter.post_input_peak = meter.post_input_peak.max(self.post_input_peak as f32);
        for (lane, attenuation) in self.min_attenuation.into_iter().enumerate() {
            meter.reduction_db[lane] = meter.reduction_db[lane].max(reduction(attenuation));
        }
    }
}
impl Runtime {
    fn restore_controls(&mut self) {
        let base = self.scratch.controls.base();
        let changed = base != *self.scratch.controls.effective_ref();
        self.scratch.controls.reset();
        if changed {
            self.scratch.parameters = Parameters::new(base, self.config.rate);
        }
    }
    fn process(&mut self, frames: &mut [StereoFrame], bypassed: bool) {
        if !self.open {
            return;
        }
        if !self.releasing && !frames.is_empty() {
            self.restore_controls();
        }
        if !frames.is_empty()
            && self.scratch.gain == self.scratch.parameters.gain
            && self.scratch.output == self.scratch.parameters.output
            && self.scratch.first.is_idle_unity(&self.config)
            && self
                .scratch
                .final_stage
                .as_ref()
                .is_none_or(|stage| stage.is_idle_unity(&self.config))
            && frames
                .iter()
                .all(|f| (f[0].to_bits() | f[1].to_bits()) == 0)
        {
            let window = self.config.delay + 1;
            self.scratch.first.advance_silent(frames.len(), window);
            if let Some(final_stage) = &mut self.scratch.final_stage {
                final_stage.advance_silent(frames.len(), window);
            }
            return;
        }
        let mut stats = BlockStats::new();
        for frame in frames {
            let dry = *frame;
            Self::process_frame(&self.config, &mut self.scratch, frame, &mut stats);
            if bypassed {
                *frame = dry;
            }
        }
        stats.finish(&mut self.scratch);
    }
    fn process_automation(
        &mut self,
        frames: &mut [StereoFrame],
        values: &dyn hex_lattice::NativeParameterAutomation,
        bypassed: bool,
    ) -> Result<(), DeviceContractError> {
        if !self.open {
            self.begin_block();
        }
        let mut map = self
            .scratch
            .controls
            .resolve(&automation::BINDINGS, values)?;
        let mut stats = BlockStats::new();
        for (offset, frame) in frames.iter_mut().enumerate() {
            if let Some(next) =
                self.scratch
                    .controls
                    .sample(&automation::BINDINGS, &mut map, values, offset)?
            {
                if next.effective_ref() != self.scratch.controls.effective_ref() {
                    self.scratch.parameters =
                        Parameters::new(*next.effective_ref(), self.config.rate);
                }
                self.scratch.controls.install(next);
            }
            let dry = *frame;
            Self::process_frame(&self.config, &mut self.scratch, frame, &mut stats);
            if bypassed {
                *frame = dry;
            }
        }
        stats.finish(&mut self.scratch);
        Ok(())
    }
    // Keep the canonical block loop optimized while sharing exact frame laws.
    #[inline(always)]
    fn process_frame(
        config: &Config,
        history: &mut History,
        frame: &mut StereoFrame,
        stats: &mut BlockStats,
    ) {
        let signal = Self::prepare_input(config, history, *frame, stats);
        Self::process_signal(config, history, signal, frame, stats);
    }
    /// Advance gain ramps once per device input frame and preserve the gained
    /// signal in f64. A buffered consumer may queue it before limiting while
    /// keeping the current ceiling/output controls at the output stage.
    #[inline(always)]
    fn prepare_input(
        config: &Config,
        history: &mut History,
        frame: StereoFrame,
        stats: &mut BlockStats,
    ) -> [f64; 2] {
        let parameters = &history.parameters;
        // Compatible edits ramp input/output gain over 5 ms. A falling ceiling
        // is immediate, preserving its safety invariant on the first sample.
        history.gain += (parameters.gain - history.gain) * config.gain_ramp_inv;
        history.output += (parameters.output - history.output) * config.gain_ramp_inv;
        // Output gain is never above the authored target after a downward edit.
        history.output = history.output.min(parameters.output);
        let signal = frame.map(|v| {
            if v.is_finite() {
                f64::from(v) * history.gain
            } else {
                0.0
            }
        });
        stats.post_input_peak = stats
            .post_input_peak
            .max(signal[0].abs())
            .max(signal[1].abs());
        signal
    }
    /// Limit already-gained input without advancing input/output gain ramps.
    /// The caller owns ramp timing and supplies the current output gain through
    /// history; this function preserves the existing limiting and meter laws.
    #[inline(always)]
    fn process_signal(
        config: &Config,
        history: &mut History,
        mut signal: [f64; 2],
        frame: &mut StereoFrame,
        stats: &mut BlockStats,
    ) {
        let parameters = &history.parameters;
        if config.mid_side {
            signal = [(signal[0] + signal[1]) * 0.5, (signal[0] - signal[1]) * 0.5];
        }
        let (mut sample, first_gain) = history.first.process(
            Sample {
                signal,
                ..Sample::default()
            },
            config,
            parameters,
            parameters.link,
            false,
        );
        let mut attenuation = first_gain.map(meter_factor);
        if config.mid_side
            && let Some(final_stage) = &mut history.final_stage
        {
            // The final stage delays this exact first-stage factor together
            // with its signal; its own factor is applied only on output.
            sample.attenuation = attenuation.map(|gain| gain as f32);
            sample.signal = [
                sample.signal[0] + sample.signal[1],
                sample.signal[0] - sample.signal[1],
            ];
            let (delayed, final_gain) = final_stage.process(sample, config, parameters, 1.0, true);
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
                    let normalized = v.abs() / parameters.output_ceiling;
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
                .any(|v| v.abs() > parameters.output_ceiling);
            for (lane, ratio) in ratios.into_iter().enumerate() {
                sample.signal[lane] *= ratio;
                attenuation[lane] *= meter_factor(ratio);
            }
        }
        *frame = sample.signal.map(|v| (v * history.output) as f32);
        for (lane, attenuation) in attenuation.into_iter().enumerate() {
            stats.min_attenuation[lane] = stats.min_attenuation[lane].min(attenuation);
        }
    }
}
impl OwnedProcessorRuntime for Runtime {
    fn fixed_storage_pair_bytes(&self) -> Option<usize> {
        let (inline, [committed, scratch]) = self.control_storage()?;
        inline.checked_add(committed)?.checked_add(scratch)
    }
    fn begin_block(&mut self) {
        if self.open {
            self.rollback_block();
        }
        self.scratch.begin_block();
        self.open = true;
    }
    fn supports_parameter_automation(&self, id: hex_lattice_contract::ParameterId) -> bool {
        self.primary.controls.supports(&automation::BINDINGS, id)
    }
    fn process_block(&mut self, frames: &mut [StereoFrame], _: &dyn hex_lattice::DeviceResources) {
        self.process(frames, false);
    }
    crate::parameter_binding::delegate_automated_block_processing!(process_automation);
    fn process_bypassed_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        _: &dyn hex_lattice::DeviceResources,
    ) {
        self.process(frames, true);
    }
    fn commit_block(&mut self) {
        if self.open {
            self.primary
                .commit_from(&mut self.scratch, self.config.delay + 1);
            self.open = false;
        }
    }
    fn rollback_block(&mut self) {
        if self.open {
            self.scratch
                .rollback_from(&self.primary, self.config.delay + 1);
            self.open = false;
        }
    }
    fn begin_output_tail(&mut self) {
        self.releasing = true;
    }
    fn reset(&mut self) {
        self.releasing = false;
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
            self.releasing = false;
            if self.same_history_layout(old) {
                let mut controls = self.primary.controls;
                controls.inherit(&old.primary.controls);
                self.primary.copy_from(&old.primary);
                self.primary.controls = controls;
                self.scratch.copy_from(&self.primary);
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

#[cfg(test)]
mod storage_tests;

#[cfg(test)]
mod detector_oracle_tests {
    use super::*;

    fn exhaustive_32_phase(x: &[f64; TAPS], phases: &[[f64; TAPS]; PHASES]) -> f64 {
        let mut max_val = 0.0_f64;
        for phase in phases {
            let mut acc = 0.0_f64;
            for (sample, coeff) in x.iter().zip(phase.iter()) {
                acc += sample * coeff;
            }
            max_val = max_val.max(acc.abs());
        }
        max_val
    }

    #[test]
    fn coarse_to_fine_true_peak_matches_exhaustive_32_phase_oracle() {
        let phases = reconstruction_coefficients();
        let mut x = [0.0_f64; TAPS];
        let mut max_under_read = 0.0_f64;
        let mut state = 0x1234_5678_9abc_def0_u64;
        for step in 0..8_192usize {
            let sample = match step / 2048 {
                0 => {
                    // Near-Nyquist gated bursts
                    let env = if step % 97 < 13 { 10.0 } else { 0.5 };
                    env * (step as f64 * 2.71 + 0.3).sin()
                }
                1 => {
                    // Full-band frequency sweep
                    let t = (step % 2048) as f64 / 2048.0;
                    (std::f64::consts::PI * t * (step % 2048) as f64).sin()
                }
                2 => {
                    // Sparse impulses & step edges
                    if step % 37 == 0 {
                        2.5
                    } else if step % 37 == 1 {
                        -2.5
                    } else {
                        0.0
                    }
                }
                _ => {
                    // Deterministic white noise
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    ((state >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
                }
            };
            x.rotate_left(1);
            x[TAPS - 1] = sample;
            let exact = exhaustive_32_phase(&x, phases);
            let fast = true_peak_lane(&x, phases);
            // `true_peak_lane` also inspects `x[16]` (phase 32 = next sample's phase 0),
            // so it is always >= exact (within floating-point rounding) and never under-reads.
            let under_read = (exact - fast).max(0.0);
            max_under_read = max_under_read.max(under_read);
            assert!(
                under_read <= 1e-6 * exact.max(1.0),
                "step={step}: fast={fast} exact={exact} under_read={under_read}"
            );
        }
        assert!(max_under_read <= 1e-6, "max_under_read={max_under_read}");
    }

    #[test]
    fn auto_release_taylor_matches_exp_across_rates_and_program_range() {
        for rate in [8_000u32, 44_100, 48_000, 96_000, 192_000] {
            let inv_rate = 1.0 / f64::from(rate);
            for step in 0..=100 {
                let program = step as f64 / 100.0;
                let seconds = 0.05 + 0.22 * program;
                let exact = 1.0 - (-1.0 / (seconds * f64::from(rate))).exp();
                let x = inv_rate / seconds;
                let approx = x * (1.0 - x * (0.5 - x * ((1.0 / 6.0) - x * (1.0 / 24.0))));
                assert!(
                    (approx - exact).abs() < 1e-14,
                    "rate={rate} program={program}: approx={approx} exact={exact}"
                );
            }
        }
    }
}

pub(super) mod structural;
