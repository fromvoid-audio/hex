//! Shared windowed-sinc polyphase interpolators and true-peak reconstruction
//! banks with `unsafe_code = "forbid"` and zero callback allocations.
//!
//! Unifies:
//! 1. Multi-rate Kaiser-windowed sinc polyphase bank ([`KaiserSincBank`],
//!    [`KaiserSincKernel`], [`SincTaps`]) for pitched-up sampler reconstruction
//!    and SoA stereo block interpolation (`&[f32]` L/R).
//! 2. Fixed-rate Blackman-windowed sinc polyphase bank ([`BlackmanSincBank`])
//!    with coarse-to-fine intersample true-peak detection for the limiter and
//!    SoA stereo block scanning (`&[f32]` L/R).
use crate::window::{bessel_i0, blackman_window_centered, kaiser_window, sinc};
use std::sync::OnceLock;

/// Sinc lobes per side at full span for the multi-rate Kaiser interpolator.
pub const KAISER_SINC_ZERO_CROSSINGS: usize = 8;
/// Kaiser transition width relative to cutoff (stopband begins at output Nyquist).
const KAISER_TRANSITION: f64 = 0.3;
/// Kaiser shape for roughly 65 dB stopband attenuation.
const KAISER_BETA: f64 = 6.2;
/// Largest half-span in source frames; bounds per-frame work.
pub const KAISER_SINC_MAX_HALF_TAPS: usize = 32;
/// Rate classes per octave; a voice uses the next class at or above its rate.
const CLASSES_PER_OCTAVE: f64 = 12.0;
/// Highest class: rate `2^(60 / 12) = 32` (+60 semitones).
const CLASSES: usize = 60;
/// Phase rows per class (plus one), linearly interpolated.
const KAISER_PHASES: usize = 32;

/// Precomputed multi-rate Kaiser-windowed sinc polyphase bank (~0.5 MB, built once
/// on control and shared immutably across all audio voices without locks).
pub struct KaiserSincBank {
    /// Rows of `taps` weights, `KAISER_PHASES + 1` rows per class, classes in order.
    weights: Box<[f32]>,
    /// Sum of each row, for exact unity DC normalization.
    sums: Box<[f32]>,
    /// Per class: offset of its first row in `weights` and its tap count.
    classes: [(usize, usize); CLASSES + 1],
}

static KAISER_BANK: OnceLock<KaiserSincBank> = OnceLock::new();

impl KaiserSincBank {
    /// Warm and borrow the static multi-rate Kaiser-sinc bank on control.
    pub fn shared() -> &'static Self {
        KAISER_BANK.get_or_init(Self::build)
    }

    fn build() -> Self {
        let inv_i0 = 1.0 / bessel_i0(KAISER_BETA);
        let mut classes = [(0, 0); CLASSES + 1];
        let mut total = 0;
        for (class, entry) in classes.iter_mut().enumerate().skip(1) {
            let taps = 2 * half_taps(kaiser_shape(class_rate(class)).1);
            *entry = (total, taps);
            total += taps * (KAISER_PHASES + 1);
        }
        let mut weights = vec![0.0_f32; total].into_boxed_slice();
        let mut sums = vec![0.0_f32; CLASSES * (KAISER_PHASES + 1)].into_boxed_slice();
        for class in 1..=CLASSES {
            let (cutoff, half_span) = kaiser_shape(class_rate(class));
            let (offset, taps) = classes[class];
            let first = 1.0 - (taps / 2) as f64;
            for phase in 0..=KAISER_PHASES {
                let fraction = phase as f64 / KAISER_PHASES as f64;
                let row = &mut weights[offset + phase * taps..offset + (phase + 1) * taps];
                let mut sum = 0.0;
                for (tap, weight) in row.iter_mut().enumerate() {
                    let t = first + tap as f64 - fraction;
                    let w = sinc(cutoff * t) * kaiser_window(t / half_span, KAISER_BETA, inv_i0);
                    *weight = w as f32;
                    sum += w;
                }
                sums[(class - 1) * (KAISER_PHASES + 1) + phase] = sum as f32;
            }
        }
        Self {
            weights,
            sums,
            classes,
        }
    }
}

fn class_rate(class: usize) -> f64 {
    2_f64.powf(class as f64 / CLASSES_PER_OCTAVE)
}

fn kaiser_shape(rate: f64) -> (f64, f64) {
    let full = 1.0 / (rate * (1.0 + KAISER_TRANSITION));
    if KAISER_SINC_ZERO_CROSSINGS as f64 / full <= KAISER_SINC_MAX_HALF_TAPS as f64 {
        (full, KAISER_SINC_ZERO_CROSSINGS as f64 / full)
    } else {
        let transition = KAISER_TRANSITION * KAISER_SINC_ZERO_CROSSINGS as f64
            / KAISER_SINC_MAX_HALF_TAPS as f64;
        (
            (rate.recip() - transition).max(0.25 / rate),
            KAISER_SINC_MAX_HALF_TAPS as f64,
        )
    }
}

fn half_taps(half_span: f64) -> usize {
    (half_span.ceil() as usize).clamp(1, KAISER_SINC_MAX_HALF_TAPS)
}

/// One output frame's interpolated polyphase taps: two adjacent phase rows and
/// their linear blend factor.
#[derive(Clone, Copy)]
pub struct SincTaps {
    pub first: i64,
    pub lower: &'static [f32],
    pub upper: &'static [f32],
    pub blend: f32,
    /// Sum of all blended weights; output is normalized by this value.
    pub total: f32,
}

impl SincTaps {
    /// Evaluate the normalized dot product over a contiguous source slice of
    /// length `>= self.lower.len()`, structured in 4-wide chunks for LLVM vectorization.
    #[inline]
    pub fn convolve(&self, source: &[f32]) -> f32 {
        let taps = self.lower.len();
        if source.len() < taps || self.total == 0.0 {
            return 0.0;
        }
        let mut lo0 = 0.0_f32;
        let mut lo1 = 0.0_f32;
        let mut lo2 = 0.0_f32;
        let mut lo3 = 0.0_f32;
        let mut hi0 = 0.0_f32;
        let mut hi1 = 0.0_f32;
        let mut hi2 = 0.0_f32;
        let mut hi3 = 0.0_f32;
        let mut i = 0;
        while i + 4 <= taps {
            let s0 = source[i];
            let s1 = source[i + 1];
            let s2 = source[i + 2];
            let s3 = source[i + 3];
            lo0 += s0 * self.lower[i];
            lo1 += s1 * self.lower[i + 1];
            lo2 += s2 * self.lower[i + 2];
            lo3 += s3 * self.lower[i + 3];
            hi0 += s0 * self.upper[i];
            hi1 += s1 * self.upper[i + 1];
            hi2 += s2 * self.upper[i + 2];
            hi3 += s3 * self.upper[i + 3];
            i += 4;
        }
        let mut lo = (lo0 + lo1) + (lo2 + lo3);
        let mut hi = (hi0 + hi1) + (hi2 + hi3);
        while i < taps {
            let s = source[i];
            lo += s * self.lower[i];
            hi += s * self.upper[i];
            i += 1;
        }
        (lo + (hi - lo) * self.blend) / self.total
    }
}

/// Per-voice Kaiser-sinc rate class handle paired with the pre-warmed static bank.
#[derive(Clone, Copy)]
pub struct KaiserSincKernel {
    class: usize,
    bank: &'static KaiserSincBank,
}

impl std::fmt::Debug for KaiserSincKernel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KaiserSincKernel")
            .field("class", &self.class)
            .finish()
    }
}

impl PartialEq for KaiserSincKernel {
    fn eq(&self, other: &Self) -> bool {
        self.class == other.class && std::ptr::eq(self.bank, other.bank)
    }
}

impl Eq for KaiserSincKernel {}

impl KaiserSincKernel {
    /// Returns `None` for unity or slower playback (linear interpolation suffices).
    /// Otherwise picks the next semitone class at or above `rate`, capped at +60 st.
    pub fn for_rate_with_bank(rate: f64, bank: &'static KaiserSincBank) -> Option<Self> {
        if !rate.is_finite() || rate <= 1.0 {
            return None;
        }
        let class = (rate.log2() * CLASSES_PER_OCTAVE - 1e-9).ceil();
        Some(Self {
            class: (class.max(1.0) as usize).min(CLASSES),
            bank,
        })
    }

    /// Look up the interpolated phase taps for a fractional position in `[0, 1)`.
    #[inline]
    pub fn taps(self, fraction: f64) -> Option<SincTaps> {
        let bank = self.bank;
        let &(offset, taps) = bank.classes.get(self.class)?;
        let scaled = (fraction * KAISER_PHASES as f64).clamp(0.0, KAISER_PHASES as f64);
        let phase = (scaled as usize).min(KAISER_PHASES - 1);
        let blend = (scaled - phase as f64) as f32;
        let lower_start = offset + phase * taps;
        let lower = bank.weights.get(lower_start..lower_start + taps)?;
        let upper = bank
            .weights
            .get(lower_start + taps..lower_start + 2 * taps)?;
        let sums = bank
            .sums
            .get((self.class - 1) * (KAISER_PHASES + 1) + phase..)?;
        let (Some(&low_sum), Some(&high_sum)) = (sums.first(), sums.get(1)) else {
            return None;
        };
        Some(SincTaps {
            first: 1 - (taps / 2) as i64,
            lower,
            upper,
            blend,
            total: low_sum + (high_sum - low_sum) * blend,
        })
    }

    /// Interpolate a block of stereo SoA source slices (`src_left`, `src_right`)
    /// at constant playback step `step` starting from `start_pos`, writing into
    /// deinterleaved `out_left` / `out_right` slices.
    pub fn interpolate_stereo_block(
        self,
        src_left: &[f32],
        src_right: &[f32],
        start_pos: f64,
        step: f64,
        out_left: &mut [f32],
        out_right: &mut [f32],
    ) {
        let n = out_left.len().min(out_right.len());
        let src_len = src_left.len().min(src_right.len());
        if n == 0 || src_len == 0 || !start_pos.is_finite() || !step.is_finite() {
            return;
        }
        let mut pos = start_pos;
        for i in 0..n {
            let base = pos.floor();
            let frac = pos - base;
            let idx = base as i64;
            if let Some(taps) = self.taps(frac) {
                let first = idx + taps.first;
                let count = taps.lower.len();
                if first >= 0 && (first as usize) + count <= src_len {
                    let s = first as usize;
                    out_left[i] = taps.convolve(&src_left[s..s + count]);
                    out_right[i] = taps.convolve(&src_right[s..s + count]);
                } else {
                    let mut lo_l = 0.0_f32;
                    let mut hi_l = 0.0_f32;
                    let mut lo_r = 0.0_f32;
                    let mut hi_r = 0.0_f32;
                    for k in 0..count {
                        let p = first + k as i64;
                        if p >= 0 && (p as usize) < src_len {
                            let sl = src_left[p as usize];
                            let sr = src_right[p as usize];
                            let wl = taps.lower[k];
                            let wh = taps.upper[k];
                            lo_l += sl * wl;
                            hi_l += sl * wh;
                            lo_r += sr * wl;
                            hi_r += sr * wh;
                        }
                    }
                    let inv = if taps.total != 0.0 {
                        1.0 / taps.total
                    } else {
                        0.0
                    };
                    out_left[i] = (lo_l + (hi_l - lo_l) * taps.blend) * inv;
                    out_right[i] = (lo_r + (hi_r - lo_r) * taps.blend) * inv;
                }
            }
            pos += step;
        }
    }
}

/// Fixed-rate Blackman-windowed sinc polyphase bank for intersample true-peak
/// detection (`TAPS` even, `PHASES` power of two >= 8, e.g. `32, 32`).
pub struct BlackmanSincBank<const TAPS: usize, const PHASES: usize> {
    phases_f64: [[f64; TAPS]; PHASES],
    phases_f32: [[f32; TAPS]; PHASES],
}

impl<const TAPS: usize, const PHASES: usize> BlackmanSincBank<TAPS, PHASES> {
    /// Build the normalized Blackman-windowed sinc reconstruction bank on control.
    pub fn build() -> Self {
        assert!(
            TAPS >= 4 && TAPS.is_multiple_of(4),
            "BlackmanSincBank TAPS must be a multiple of 4"
        );
        assert!(PHASES >= 8, "BlackmanSincBank PHASES must be >= 8");
        let half = (TAPS / 2) as f64;
        let phases_f64: [[f64; TAPS]; PHASES] = std::array::from_fn(|phase| {
            let position = (TAPS / 2 - 1) as f64 + phase as f64 / PHASES as f64;
            let mut coefficients = std::array::from_fn(|tap| {
                let distance = position - tap as f64;
                sinc(distance) * blackman_window_centered(distance / half)
            });
            let sum: f64 = coefficients.iter().sum();
            for c in &mut coefficients {
                *c /= sum;
            }
            coefficients
        });
        let phases_f32 = std::array::from_fn(|p| std::array::from_fn(|t| phases_f64[p][t] as f32));
        Self {
            phases_f64,
            phases_f32,
        }
    }

    /// Borrow the raw `f64` polyphase table.
    #[inline]
    pub fn phases_f64(&self) -> &[[f64; TAPS]; PHASES] {
        &self.phases_f64
    }

    /// Borrow the raw `f32` polyphase table.
    #[inline]
    pub fn phases_f32(&self) -> &[[f32; TAPS]; PHASES] {
        &self.phases_f32
    }

    /// Unrolled 4-accumulator `f64` dot product over `TAPS` contiguous samples.
    #[inline(always)]
    pub fn dot_f64(x: &[f64], h: &[f64; TAPS]) -> f64 {
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

    /// Unrolled 4-accumulator `f32` dot product over `TAPS` contiguous samples.
    #[inline(always)]
    pub fn dot_f32(x: &[f32], h: &[f32; TAPS]) -> f32 {
        let mut a0 = 0.0_f32;
        let mut a1 = 0.0_f32;
        let mut a2 = 0.0_f32;
        let mut a3 = 0.0_f32;
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
}

impl BlackmanSincBank<32, 32> {
    /// Warm and borrow the static 32-tap, 32-phase Blackman-sinc bank used by
    /// the true-peak limiter detector.
    pub fn shared_32x32() -> &'static Self {
        static BANK_32X32: OnceLock<BlackmanSincBank<32, 32>> = OnceLock::new();
        BANK_32X32.get_or_init(Self::build)
    }

    /// Coarse-to-fine peak search over the exact 32-phase, 32-tap Blackman-windowed
    /// sinc reconstruction for one `f64` lane (`x.len() >= 32`).
    #[inline(always)]
    pub fn true_peak_lane_f64(&self, x: &[f64]) -> f64 {
        let phases = &self.phases_f64;
        let v0 = x[15].abs();
        let v8 = Self::dot_f64(x, &phases[8]).abs();
        let v16 = Self::dot_f64(x, &phases[16]).abs();
        let v24 = Self::dot_f64(x, &phases[24]).abs();
        let v32 = x[16].abs();
        let mut best = v0.max(v8).max(v16).max(v24).max(v32);
        if best == 0.0 {
            return 0.0;
        }
        let mut grid = [v0, 0.0, v8, 0.0, v16, 0.0, v24, 0.0, v32];
        let coarse_threshold = best * 0.85;
        for (idx, phase) in [(1usize, 4usize), (3, 12), (5, 20), (7, 28)] {
            if grid[idx - 1].max(grid[idx + 1]) >= coarse_threshold {
                let v = Self::dot_f64(x, &phases[phase]).abs();
                grid[idx] = v;
                if v > best {
                    best = v;
                }
            }
        }
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
                    if p > 0 && p < 32 {
                        let v = Self::dot_f64(x, &phases[p]).abs();
                        if v > best {
                            best = v;
                            next_center = p;
                        }
                    }
                }
                let p = center + stride;
                if p > 0 && p < 32 {
                    let v = Self::dot_f64(x, &phases[p]).abs();
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

    /// Coarse-to-fine peak search for one `f32` lane (`x.len() >= 32`), returning
    /// the reconstructed intersample peak magnitude.
    #[inline(always)]
    pub fn true_peak_lane_f32(&self, x: &[f32]) -> f32 {
        let phases = &self.phases_f32;
        let v0 = x[15].abs();
        let v8 = Self::dot_f32(x, &phases[8]).abs();
        let v16 = Self::dot_f32(x, &phases[16]).abs();
        let v24 = Self::dot_f32(x, &phases[24]).abs();
        let v32 = x[16].abs();
        let mut best = v0.max(v8).max(v16).max(v24).max(v32);
        if best == 0.0 {
            return 0.0;
        }
        let mut grid = [v0, 0.0, v8, 0.0, v16, 0.0, v24, 0.0, v32];
        let coarse_threshold = best * 0.85;
        for (idx, phase) in [(1usize, 4usize), (3, 12), (5, 20), (7, 28)] {
            if grid[idx - 1].max(grid[idx + 1]) >= coarse_threshold {
                let v = Self::dot_f32(x, &phases[phase]).abs();
                grid[idx] = v;
                if v > best {
                    best = v;
                }
            }
        }
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
                    if p > 0 && p < 32 {
                        let v = Self::dot_f32(x, &phases[p]).abs();
                        if v > best {
                            best = v;
                            next_center = p;
                        }
                    }
                }
                let p = center + stride;
                if p > 0 && p < 32 {
                    let v = Self::dot_f32(x, &phases[p]).abs();
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

    /// Compute true peaks across deinterleaved SoA stereo input slices (`left`, `right`,
    /// each of length `>= n + 31`), writing `n` intersample peaks into `out_left` / `out_right`.
    pub fn true_peak_stereo_block(
        &self,
        left: &[f32],
        right: &[f32],
        out_left: &mut [f32],
        out_right: &mut [f32],
    ) {
        let n = out_left
            .len()
            .min(out_right.len())
            .min(left.len().saturating_sub(31))
            .min(right.len().saturating_sub(31));
        for i in 0..n {
            out_left[i] = self.true_peak_lane_f32(&left[i..i + 32]);
            out_right[i] = self.true_peak_lane_f32(&right[i..i + 32]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kaiser_sinc_bank_dc_unity_and_stereo_block_interpolation() {
        let bank = KaiserSincBank::shared();
        assert!(KaiserSincKernel::for_rate_with_bank(1.0, bank).is_none());
        let kernel = KaiserSincKernel::for_rate_with_bank(2.0, bank).unwrap();
        let src_l = [1.0_f32; 128];
        let src_r = [-0.5_f32; 128];
        let mut out_l = [0.0_f32; 16];
        let mut out_r = [0.0_f32; 16];
        kernel.interpolate_stereo_block(&src_l, &src_r, 32.25, 2.0, &mut out_l, &mut out_r);
        for i in 0..16 {
            assert!((out_l[i] - 1.0).abs() < 1e-6, "out_l[{i}]={}", out_l[i]);
            assert!((out_r[i] + 0.5).abs() < 1e-6, "out_r[{i}]={}", out_r[i]);
        }
    }

    #[test]
    fn blackman_sinc_32x32_detects_intersample_peak_above_sample_peak() {
        let bank = BlackmanSincBank::shared_32x32();
        // Quarter-rate sine with equal-magnitude samples (+sqrt(2)/2) at taps 15 and 16
        // has its true intersample peak (+1.0) halfway between taps 15 and 16.
        let sig_f64: [f64; 32] = std::array::from_fn(|i| {
            (std::f64::consts::FRAC_PI_2 * (i as f64 - 15.0) + std::f64::consts::FRAC_PI_4).sin()
        });
        let sig_f32: [f32; 32] = std::array::from_fn(|i| sig_f64[i] as f32);
        assert!(sig_f64[15].abs() < 0.72 && sig_f64[16].abs() < 0.72);
        let peak_f64 = bank.true_peak_lane_f64(&sig_f64);
        let peak_f32 = bank.true_peak_lane_f32(&sig_f32);
        assert!(peak_f64 > 0.99 && peak_f64 < 1.01, "peak_f64={peak_f64}");
        assert!((peak_f64 as f32 - peak_f32).abs() < 1e-5);
    }
}
