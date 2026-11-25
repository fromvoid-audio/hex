//! Shared 8x Kaiser-sinc polyphase oversampler with first-order antiderivative
//! anti-aliasing (ADAA), structured for block-oriented SoA LLVM auto-vectorization
//! with `unsafe_code = "forbid"` and zero callback allocations.
//!
//! One linear-phase Kaiser-windowed sinc kernel (96 taps at the 8x oversampled
//! rate, cutoff `20 / 48 / 8` of the oversampled rate, `beta = 10`) serves both
//! polyphase interpolation and symmetric decimation:
//! - Interpolation uses a lag-major `[12][8]` `f32` table so each base sample
//!   computes all 8 oversampled phases via 12 contiguous 8-wide vector FMAs with
//!   zero horizontal reductions and zero per-sample history shifts.
//! - Decimation exploits linear-phase symmetry (`taps[k] == taps[95 - k]`) over
//!   a contiguous 8x-rate chunk buffer using 4-wide vector FMAs (48 multiplies
//!   instead of 96).
//! - Single-sample and block paths execute the exact same accumulation order,
//!   guaranteeing bit-identical output across arbitrary block partitions.
//!
//! Round-trip latency: `(96 - 1) / 2` oversampled samples per filter plus 0.5
//! from ADAA minus 7 phase advance = `88.5` oversampled samples = `11.0625`
//! base frames, declared as [`OVERSAMPLE_8X_LATENCY_FRAMES`] (`11`).
use crate::denormal::{flush_quiet, flush_quiet_f32};
use crate::window::{bessel_i0, kaiser_window, sinc};
use std::sync::OnceLock;

pub const OVERSAMPLE_8X_FACTOR: usize = 8;
pub const OVERSAMPLE_8X_TAPS: usize = 96;
pub const OVERSAMPLE_8X_PHASE_TAPS: usize = OVERSAMPLE_8X_TAPS / OVERSAMPLE_8X_FACTOR;
const PHASE_HIST: usize = OVERSAMPLE_8X_PHASE_TAPS - 1;
const SHAPED_HIST: usize = OVERSAMPLE_8X_TAPS - 1;
const HALF_TAPS: usize = OVERSAMPLE_8X_TAPS / 2;
/// Declared base-rate latency of the 8x oversampled stage (11.0625 rounded).
pub const OVERSAMPLE_8X_LATENCY_FRAMES: usize = 11;
/// Consecutive zero base-rate frames after which every history is exactly zero.
pub const OVERSAMPLE_8X_DRAIN_FRAMES: usize =
    OVERSAMPLE_8X_PHASE_TAPS + OVERSAMPLE_8X_TAPS / OVERSAMPLE_8X_FACTOR + 1;
/// Stack chunk size for zero-allocation SoA block processing.
pub const OVERSAMPLE_BLOCK_CHUNK: usize = 64;

const CUTOFF: f64 = 20.0 / 48.0 / OVERSAMPLE_8X_FACTOR as f64;
const BETA: f64 = 10.0;
/// Below this oversampled step, ADAA uses the midpoint value to avoid
/// cancellation in the antiderivative difference.
const ADAA_EPSILON: f64 = 1.0e-5;

/// A memoryless odd transfer with a closed-form antiderivative for first-order ADAA.
pub trait AdaaShaper {
    /// Distinct per transfer law, so a law change refreshes the cached
    /// antiderivative instead of mixing two laws in one ADAA difference.
    fn tag(&self) -> u8;
    fn value(&self, x: f64) -> f64;
    /// Must be continuous and satisfy `antiderivative(0) == 0`.
    fn antiderivative(&self, x: f64) -> f64;
}

/// First-order ADAA between previous and current arguments, given both antiderivatives.
#[inline(always)]
pub fn adaa_step(
    shaper: &impl AdaaShaper,
    current: f64,
    current_antiderivative: f64,
    previous: f64,
    previous_antiderivative: f64,
) -> f64 {
    let delta = current - previous;
    if delta.abs() < ADAA_EPSILON {
        shaper.value((current + previous) * 0.5)
    } else {
        (current_antiderivative - previous_antiderivative) / delta
    }
}

/// Shared 96-tap 8x Kaiser-sinc kernel normalized to unity DC gain per polyphase
/// branch, with transposed `f32` lag-major interpolation and symmetric decimation tables.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KaiserOversample8xKernel {
    taps: [f64; OVERSAMPLE_8X_TAPS],
    phases: [[f64; OVERSAMPLE_8X_PHASE_TAPS]; OVERSAMPLE_8X_FACTOR],
    /// Unscaled lag-major interpolation table: `interp_by_lag[lag][phase]` multiplies
    /// `in_buf[i + lag]` (oldest at lag 0, newest at lag 11). Keeping it unscaled
    /// (column sum `1/8`) prevents `f32` overflow even at `f32::MAX` inputs.
    interp_by_lag_f32: [[f32; OVERSAMPLE_8X_FACTOR]; OVERSAMPLE_8X_PHASE_TAPS],
    /// First half of symmetric decimation taps (`taps[0..48]`) normalized in `f32`
    /// so `2.0 * sum(half_taps_f32) == 1.0_f32` for exact DC unity gain.
    half_taps_f32: [f32; HALF_TAPS],
}

static SHARED_KERNEL: OnceLock<KaiserOversample8xKernel> = OnceLock::new();

impl Default for KaiserOversample8xKernel {
    fn default() -> Self {
        Self::new()
    }
}

impl KaiserOversample8xKernel {
    /// Warm and borrow the process-global kernel (computed once on control).
    pub fn shared() -> &'static Self {
        SHARED_KERNEL.get_or_init(Self::new)
    }

    /// Control-side construction using the unified [`bessel_i0`] and [`sinc`].
    pub fn new() -> Self {
        let middle = (OVERSAMPLE_8X_TAPS - 1) as f64 * 0.5;
        let inv_i0 = 1.0 / bessel_i0(BETA);
        let mut taps: [f64; OVERSAMPLE_8X_TAPS] = std::array::from_fn(|n| {
            let t = n as f64 - middle;
            let ideal = 2.0 * CUTOFF * sinc(2.0 * CUTOFF * t);
            ideal * kaiser_window(t / middle, BETA, inv_i0)
        });
        for phase in 0..OVERSAMPLE_8X_FACTOR {
            let gain: f64 = taps.iter().skip(phase).step_by(OVERSAMPLE_8X_FACTOR).sum();
            for tap in taps.iter_mut().skip(phase).step_by(OVERSAMPLE_8X_FACTOR) {
                *tap /= gain * OVERSAMPLE_8X_FACTOR as f64;
            }
        }
        let phases =
            std::array::from_fn(|p| std::array::from_fn(|j| taps[p + OVERSAMPLE_8X_FACTOR * j]));
        // lag = 0 corresponds to oldest sample x[i - 11] (tap j = 11);
        // lag = 11 corresponds to newest sample x[i] (tap j = 0).
        let mut interp_by_lag_f32 = std::array::from_fn(|lag| {
            let j = PHASE_HIST - lag;
            std::array::from_fn(|p| phases[p][j] as f32)
        });
        // Eliminate f32 rounding drift on each polyphase branch DC gain (`1/8`).
        let mut phase_sums = [0.0_f32; OVERSAMPLE_8X_FACTOR];
        for row in &interp_by_lag_f32 {
            for (sum, &tap) in phase_sums.iter_mut().zip(row) {
                *sum += tap;
            }
        }
        let target = 1.0_f32 / OVERSAMPLE_8X_FACTOR as f32;
        for (center_tap, sum) in interp_by_lag_f32[PHASE_HIST / 2].iter_mut().zip(phase_sums) {
            *center_tap += target - sum;
        }
        let mut half_taps_f32: [f32; HALF_TAPS] = std::array::from_fn(|k| taps[k] as f32);
        let half_sum: f32 = half_taps_f32.iter().copied().sum();
        half_taps_f32[HALF_TAPS - 1] += 0.5_f32 - half_sum;
        Self {
            taps,
            phases,
            interp_by_lag_f32,
            half_taps_f32,
        }
    }

    /// Borrow the full 96-tap `f64` kernel.
    #[inline]
    pub fn taps(&self) -> &[f64; OVERSAMPLE_8X_TAPS] {
        &self.taps
    }

    /// Borrow the `f64` polyphase branches.
    #[inline]
    pub fn phases(&self) -> &[[f64; OVERSAMPLE_8X_PHASE_TAPS]; OVERSAMPLE_8X_FACTOR] {
        &self.phases
    }
}

/// One channel's 8x polyphase oversampler history in compact contiguous layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Oversampler8x {
    /// Base-rate input history, oldest first (`in_hist[0]` is 11 frames ago).
    in_hist: [f32; PHASE_HIST],
    /// Oversampled shaped history, oldest first (`shaped_hist[94]` is newest).
    shaped_hist: [f32; SHAPED_HIST],
    /// Previous driven argument for ADAA, its antiderivative, and shaper tag.
    previous: f64,
    previous_antiderivative: f64,
    previous_tag: u8,
    /// Consecutive zero inputs, saturating at [`OVERSAMPLE_8X_DRAIN_FRAMES`].
    zero_run: usize,
}

impl Default for Oversampler8x {
    fn default() -> Self {
        Self::new()
    }
}

impl Oversampler8x {
    pub const fn new() -> Self {
        Self {
            in_hist: [0.0; PHASE_HIST],
            shaped_hist: [0.0; SHAPED_HIST],
            previous: 0.0,
            previous_antiderivative: 0.0,
            previous_tag: 0,
            zero_run: OVERSAMPLE_8X_DRAIN_FRAMES,
        }
    }

    #[inline]
    pub fn is_drained(&self) -> bool {
        self.zero_run >= OVERSAMPLE_8X_DRAIN_FRAMES
    }

    pub fn is_quiescent(&self) -> bool {
        self.previous == 0.0
            && self.in_hist.iter().all(|&v| v == 0.0)
            && self.shaped_hist.iter().all(|&v| v == 0.0)
    }

    /// Process one base-rate sample through 8x polyphase interpolation, `drive`,
    /// first-order ADAA shaping, and symmetric 8x decimation. Executes the exact
    /// same tap accumulation order as [`Self::process_block`], guaranteeing
    /// bit-identical output across arbitrary block partitions.
    #[inline(always)]
    pub fn process_sample(
        &mut self,
        input: f64,
        drive: f64,
        kernel: &KaiserOversample8xKernel,
        shaper: &impl AdaaShaper,
    ) -> f64 {
        let mut single = [input as f32];
        self.process_chunk(&mut single, drive, kernel, shaper);
        f64::from(single[0])
    }

    /// Process a contiguous single-channel `f32` slice in place using stack chunks
    /// (`<= 64` frames) with zero heap allocations.
    #[inline(always)]
    pub fn process_block(
        &mut self,
        samples: &mut [f32],
        drive: f64,
        kernel: &KaiserOversample8xKernel,
        shaper: &impl AdaaShaper,
    ) {
        if self.zero_run >= OVERSAMPLE_8X_DRAIN_FRAMES
            && samples.iter().all(|&s| flush_quiet_f32(s) == 0.0)
        {
            samples.fill(0.0);
            return;
        }
        for chunk in samples.chunks_mut(OVERSAMPLE_BLOCK_CHUNK) {
            self.process_chunk(chunk, drive, kernel, shaper);
        }
    }

    /// Process deinterleaved stereo SoA `&mut [f32]` L/R slices in place through
    /// two independent `Oversampler8x` channel states.
    #[inline(always)]
    pub fn process_stereo_block(
        left_stage: &mut Self,
        right_stage: &mut Self,
        left: &mut [f32],
        right: &mut [f32],
        drive: f64,
        kernel: &KaiserOversample8xKernel,
        shaper: &impl AdaaShaper,
    ) {
        let n = left.len().min(right.len());
        left_stage.process_block(&mut left[..n], drive, kernel, shaper);
        right_stage.process_block(&mut right[..n], drive, kernel, shaper);
    }

    #[inline(always)]
    fn process_chunk(
        &mut self,
        samples: &mut [f32],
        drive: f64,
        kernel: &KaiserOversample8xKernel,
        shaper: &impl AdaaShaper,
    ) {
        let n = samples.len();
        debug_assert!(n <= OVERSAMPLE_BLOCK_CHUNK);
        if n == 0 {
            return;
        }

        let mut any_nonzero = false;
        let mut zero_run = self.zero_run;
        for sample in samples.iter_mut() {
            let cleaned = flush_quiet_f32(*sample);
            *sample = cleaned;
            if cleaned == 0.0 {
                zero_run = zero_run.saturating_add(1).min(OVERSAMPLE_8X_DRAIN_FRAMES);
            } else {
                zero_run = 0;
                any_nonzero = true;
            }
        }
        if !any_nonzero && self.zero_run >= OVERSAMPLE_8X_DRAIN_FRAMES {
            return;
        }
        self.zero_run = zero_run;

        if self.previous_tag != shaper.tag() {
            self.previous_tag = shaper.tag();
            self.previous_antiderivative = shaper.antiderivative(self.previous);
        }

        // 1. Contiguous base-rate input buffer: [11 history | n new samples].
        let mut in_buf = [0.0_f32; PHASE_HIST + OVERSAMPLE_BLOCK_CHUNK];
        in_buf[..PHASE_HIST].copy_from_slice(&self.in_hist);
        in_buf[PHASE_HIST..PHASE_HIST + n].copy_from_slice(samples);
        self.in_hist.copy_from_slice(&in_buf[n..n + PHASE_HIST]);

        // 2. Contiguous 8x-rate shaped buffer: [95 history | 8*n new samples].
        let mut shaped_buf = [0.0_f32; SHAPED_HIST + OVERSAMPLE_8X_FACTOR * OVERSAMPLE_BLOCK_CHUNK];
        shaped_buf[..SHAPED_HIST].copy_from_slice(&self.shaped_hist);

        let scale = OVERSAMPLE_8X_FACTOR as f64 * drive;
        let interp_table = &kernel.interp_by_lag_f32;
        let mut prev = self.previous;
        let mut prev_ad = self.previous_antiderivative;

        for i in 0..n {
            // Vectorized 8-wide lag-major interpolation: 12 broadcast-FMAs across [f32; 8].
            let src = &in_buf[i..i + OVERSAMPLE_8X_PHASE_TAPS];
            let s0 = src[0];
            let r0 = &interp_table[0];
            let mut up = [
                s0 * r0[0],
                s0 * r0[1],
                s0 * r0[2],
                s0 * r0[3],
                s0 * r0[4],
                s0 * r0[5],
                s0 * r0[6],
                s0 * r0[7],
            ];
            for lag in 1..OVERSAMPLE_8X_PHASE_TAPS {
                let s = src[lag];
                let row = &interp_table[lag];
                for p in 0..OVERSAMPLE_8X_FACTOR {
                    up[p] += s * row[p];
                }
            }
            // Chronological ADAA shaping into contiguous 8-sample output slot.
            let dst = &mut shaped_buf[SHAPED_HIST + i * OVERSAMPLE_8X_FACTOR
                ..SHAPED_HIST + (i + 1) * OVERSAMPLE_8X_FACTOR];
            for p in 0..OVERSAMPLE_8X_FACTOR {
                let driven = flush_quiet(f64::from(up[p]) * scale);
                let ad = shaper.antiderivative(driven);
                let out = flush_quiet(adaa_step(shaper, driven, ad, prev, prev_ad));
                prev = driven;
                prev_ad = ad;
                dst[p] = out as f32;
            }
        }
        self.previous = prev;
        self.previous_antiderivative = prev_ad;
        self.shaped_hist.copy_from_slice(
            &shaped_buf[n * OVERSAMPLE_8X_FACTOR..n * OVERSAMPLE_8X_FACTOR + SHAPED_HIST],
        );

        // 3. Symmetric 4-wide vector decimation (48 taps instead of 96).
        //    For frame `i`, the newest sample is at `shaped_buf[8 * i + 95]`, and
        //    the 96-sample window is `shaped_buf[8 * i + 7 .. 8 * i + 103]`?
        //    Check index: when i=0, newest written is at index SHAPED_HIST + 7 = 95 + 7 = 102!
        //    The 96 samples ending at 102 inclusive are indices 7..103!
        //    When i=0, oldest of those 96 is 7 = 8*0 + 7, newest is 102 = 8*0 + 102.
        //    Check for n=1: shaped_hist was 0..95, new 8 samples at 95..103,
        //    window is 7..103 (length 96), and saved history is 8..103 (length 95)! Exact!
        let half_taps = &kernel.half_taps_f32;
        for (i, out_sample) in samples.iter_mut().enumerate() {
            let base = i * OVERSAMPLE_8X_FACTOR + (OVERSAMPLE_8X_FACTOR - 1);
            let w = &shaped_buf[base..base + OVERSAMPLE_8X_TAPS];
            let mut acc = [0.0_f32; 4];
            let mut k = 0;
            while k < HALF_TAPS {
                let r = OVERSAMPLE_8X_TAPS - 4 - k;
                let f = &w[k..k + 4];
                let b = &w[r..r + 4];
                let t = &half_taps[k..k + 4];
                acc[0] += (f[0] + b[3]) * t[0];
                acc[1] += (f[1] + b[2]) * t[1];
                acc[2] += (f[2] + b[1]) * t[2];
                acc[3] += (f[3] + b[0]) * t[3];
                k += 4;
            }
            *out_sample = (acc[0] + acc[1]) + (acc[2] + acc[3]);
        }

        if self.zero_run >= OVERSAMPLE_8X_DRAIN_FRAMES {
            self.in_hist = [0.0; PHASE_HIST];
            self.shaped_hist = [0.0; SHAPED_HIST];
            self.previous = 0.0;
            self.previous_antiderivative = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::fast_ln_cosh_f64;

    struct TanhShaper;
    impl AdaaShaper for TanhShaper {
        fn tag(&self) -> u8 {
            1
        }
        fn value(&self, x: f64) -> f64 {
            x.tanh()
        }
        fn antiderivative(&self, x: f64) -> f64 {
            fast_ln_cosh_f64(x)
        }
    }

    #[test]
    fn kernel_is_symmetric_with_unity_dc_and_phase_gains() {
        let kernel = KaiserOversample8xKernel::new();
        for n in 0..OVERSAMPLE_8X_TAPS {
            assert!((kernel.taps[n] - kernel.taps[OVERSAMPLE_8X_TAPS - 1 - n]).abs() < 1e-15);
        }
        assert!((kernel.taps.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        for phase in 0..OVERSAMPLE_8X_FACTOR {
            let gain: f64 = (0..OVERSAMPLE_8X_PHASE_TAPS)
                .map(|j| kernel.taps[phase + OVERSAMPLE_8X_FACTOR * j])
                .sum();
            assert!((gain * OVERSAMPLE_8X_FACTOR as f64 - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn block_and_sample_paths_are_bit_identical_across_partitions() {
        let kernel = KaiserOversample8xKernel::shared();
        let input: [f32; 137] = std::array::from_fn(|i| 0.7 * ((i as f32) * 0.31).sin());

        let mut single = Oversampler8x::new();
        let mut expected = [0.0_f32; 137];
        for (dst, &src) in expected.iter_mut().zip(&input) {
            *dst = single.process_sample(f64::from(src), 12.0, kernel, &TanhShaper) as f32;
        }

        let mut blocked = Oversampler8x::new();
        let mut actual = input;
        for (start, len) in [(0, 13), (13, 64), (77, 1), (78, 59)] {
            blocked.process_block(&mut actual[start..start + len], 12.0, kernel, &TanhShaper);
        }

        assert_eq!(actual, expected);
        assert_eq!(blocked, single);
    }

    #[test]
    fn small_signal_passes_with_unity_gain_and_declared_delay() {
        let kernel = KaiserOversample8xKernel::shared();
        let mut stage = Oversampler8x::new();
        let drive = 1.0e-3;
        let response: Vec<f64> = (0..64)
            .map(|n| {
                stage.process_sample(if n == 0 { 1.0 } else { 0.0 }, drive, kernel, &TanhShaper)
                    / drive
            })
            .collect();
        let sum: f64 = response.iter().sum();
        let centroid: f64 = response
            .iter()
            .enumerate()
            .map(|(n, y)| n as f64 * y)
            .sum::<f64>()
            / sum;
        assert!((sum - 1.0).abs() < 1e-5, "dc gain {sum}");
        assert!(
            (centroid - 88.5 / OVERSAMPLE_8X_FACTOR as f64).abs() < 1e-3,
            "delay {centroid}"
        );
        assert_eq!(centroid.round() as usize, OVERSAMPLE_8X_LATENCY_FRAMES);
    }

    #[test]
    fn extreme_f32_max_input_remains_finite() {
        let kernel = KaiserOversample8xKernel::shared();
        let mut stage = Oversampler8x::new();
        let mut buf = [f32::MAX; 32];
        stage.process_block(&mut buf, 1.0, kernel, &TanhShaper);
        assert!(buf[31].is_finite() && (buf[31] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn silence_drains_every_history_before_the_fast_path() {
        let kernel = KaiserOversample8xKernel::shared();
        let mut stage = Oversampler8x::new();
        let mut active: [f32; 128] = std::array::from_fn(|n| ((n as f32) * 0.7).sin());
        stage.process_block(&mut active, 30.0, kernel, &TanhShaper);
        let mut silence = [0.0_f32; OVERSAMPLE_8X_DRAIN_FRAMES];
        stage.process_block(&mut silence, 30.0, kernel, &TanhShaper);
        assert!(stage.is_drained());
        assert!(stage.is_quiescent());
        assert_eq!(stage.process_sample(0.0, 30.0, kernel, &TanhShaper), 0.0);
    }
}
