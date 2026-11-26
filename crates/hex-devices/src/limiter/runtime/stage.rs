use super::*;

pub(super) const TAPS: usize = 32;
pub(super) const PHASES: usize = 32;
// Zero is the untouched-slot sentinel for unity. Every stored first-stage
// factor is floored at 1e-30, so it remains strictly positive in f32.
#[derive(Clone, Copy, Default)]
pub(super) struct Sample {
    pub(super) signal: [f64; 2],
    pub(super) attenuation: [f32; 2],
    pub(super) detector: [f64; 2],
}
#[derive(Clone, Copy, Default)]
pub(super) struct Peak {
    pub(super) value: f64,
    pub(super) stamp: u64,
}
#[derive(Clone, Copy)]
pub(super) struct Scalar {
    pub(super) next: usize,
    pub(super) stamp: u64,
    pub(super) nonzero: usize,
    pub(super) head: [usize; 2],
    pub(super) len: [usize; 2],
    pub(super) gain: [f64; 2],
    pub(super) attack_step: [f64; 2],
    pub(super) program: [f64; 2],
    // Doubled planar ring buffer so `[input_next..input_next + TAPS]` is a
    // contiguous 32-tap slice per lane with zero modulo indexing.
    pub(super) input: [[f64; TAPS * 2]; 2],
    pub(super) input_next: usize,
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
pub(super) struct StageDirty {
    pub(super) full: bool,
    pub(super) signal_start: usize,
    pub(super) signal_len: usize,
    pub(super) peaks_base_head: [usize; 2],
    pub(super) peaks_head_off: [usize; 2],
    pub(super) peaks_min: [usize; 2],
    pub(super) peaks_max: [usize; 2],
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
    pub(super) fn begin(scalar: &Scalar) -> Self {
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
    pub(super) fn has_writes(&self) -> bool {
        self.full
            || self.signal_len > 0
            || self.peaks_min[0] < self.peaks_max[0]
            || self.peaks_min[1] < self.peaks_max[1]
    }
}
pub(super) fn copy_cyclic<T: Copy>(
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
pub(super) struct Stage {
    pub(super) signal: Vec<Sample>,
    pub(super) peaks: [Vec<Peak>; 2],
    pub(super) scalar: Scalar,
    pub(super) dirty: StageDirty,
}
pub(super) fn allocate<T: Clone + Default>(length: usize) -> Result<Vec<T>, DeviceContractError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(length)
        .map_err(|_| DeviceContractError::RuntimeAllocation)?;
    values.resize(length, T::default());
    Ok(values)
}
impl Stage {
    pub(super) fn new(length: usize) -> Result<Self, DeviceContractError> {
        Ok(Self {
            signal: allocate(length)?,
            peaks: [allocate(length)?, allocate(length)?],
            scalar: Scalar::default(),
            dirty: StageDirty::default(),
        })
    }
    pub(super) fn begin_block(&mut self) {
        self.dirty = StageDirty::begin(&self.scalar);
    }
    pub(super) fn copy_from(&mut self, old: &Self) {
        self.signal.copy_from_slice(&old.signal);
        for (target, source) in self.peaks.iter_mut().zip(&old.peaks) {
            target.copy_from_slice(source);
        }
        self.scalar = old.scalar;
        self.dirty = StageDirty::default();
    }
    pub(super) fn sync_from(&mut self, source: &Self, dirty: &StageDirty, window: usize) {
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
    pub(super) fn bytes(&self) -> usize {
        self.signal.capacity() * std::mem::size_of::<Sample>()
            + self
                .peaks
                .iter()
                .map(|p| p.capacity() * std::mem::size_of::<Peak>())
                .sum::<usize>()
    }
    pub(super) fn quiet(&self) -> bool {
        self.scalar.nonzero == 0
    }
    pub(super) fn is_idle_unity(&self, config: &Config) -> bool {
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
    pub(super) fn advance_silent(&mut self, count: usize, window: usize) {
        if self.dirty.has_writes() {
            self.dirty.full = true;
        }
        let s = &mut self.scalar;
        s.next = (s.next + count) % window;
        s.input_next = (s.input_next + count) % TAPS;
        s.stamp = s.stamp.wrapping_add(count as u64);
        s.len = [0, 0];
    }
    pub(super) fn reset(&mut self) {
        self.signal.fill(Sample::default());
        self.scalar = Scalar::default();
        self.dirty.full = true;
    }
    pub(super) fn same_storage_layout(&self, previous: &Self) -> bool {
        self.signal.len() == previous.signal.len()
            && self
                .peaks
                .iter()
                .zip(&previous.peaks)
                .all(|(a, b)| a.len() == b.len())
    }
    #[inline(always)]
    pub(super) fn maximum(&mut self, lane: usize, value: f64, window: usize) -> f64 {
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
    pub(super) fn process(
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
            let p0 = config
                .reconstruction
                .true_peak_lane_f64(&s.input[0][start..start + TAPS]);
            let p1 = config
                .reconstruction
                .true_peak_lane_f64(&s.input[1][start..start + TAPS]);
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
