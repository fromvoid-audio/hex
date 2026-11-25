//! Shared 2x linear-phase Blackman-windowed sinc halfband FIR kernels.
//!
//! Provides both per-sample `f64` processing (for exact legacy parity when
//! controls vary per sample) and block-oriented SoA stereo (`&mut [f32]` L/R)
//! upsampling and decimation structured for LLVM auto-vectorization with
//! `unsafe_code = "forbid"` and zero callback allocations.
use crate::window::{blackman_window, sinc};

/// Stack chunk size for zero-allocation block-oriented halfband resampling.
pub const HALFBAND_BLOCK_CHUNK: usize = 64;

/// Odd-length Blackman-windowed sinc halfband FIR kernel at a quarter of the
/// 2x oversampled rate, normalized to unity DC gain.
///
/// `TAPS` must be odd (e.g., 23 for 11-frame round-trip latency).
/// `HISTORY` must equal `TAPS.div_ceil(2)` (e.g., 12 for 23 taps).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Halfband2xKernel<const TAPS: usize, const HISTORY: usize> {
    taps: [f64; TAPS],
    taps_f32: [f32; TAPS],
    /// Even-indexed taps (`taps[0], taps[2], ...`), scaled by 2 for 2x zero-stuffing.
    even_up_f32: [f32; HISTORY],
    /// Odd-indexed taps (`taps[1], taps[3], ...`, trailing 0), scaled by 2.
    odd_up_f32: [f32; HISTORY],
}

impl<const TAPS: usize, const HISTORY: usize> Halfband2xKernel<TAPS, HISTORY> {
    /// Design the odd-length Blackman-windowed sinc halfband kernel on control.
    ///
    /// Panics at compile/debug time if `TAPS` is even or `HISTORY != TAPS.div_ceil(2)`.
    pub fn blackman() -> Self {
        assert!(TAPS >= 3 && TAPS % 2 == 1, "halfband TAPS must be odd >= 3");
        assert!(
            HISTORY == TAPS.div_ceil(2),
            "halfband HISTORY must equal TAPS.div_ceil(2)"
        );
        let centre = (TAPS - 1) as f64 * 0.5;
        let mut taps = std::array::from_fn(|n| {
            let offset = n as f64 - centre;
            let ideal = 0.5 * sinc(0.5 * offset);
            let phase = std::f64::consts::TAU * n as f64 / (TAPS - 1) as f64;
            ideal * blackman_window(phase)
        });
        let sum: f64 = taps.iter().sum();
        for tap in &mut taps {
            *tap /= sum;
        }
        let taps_f32 = std::array::from_fn(|n| taps[n] as f32);
        let even_up_f32 = std::array::from_fn(|k| (2.0 * taps[2 * k]) as f32);
        let odd_up_f32 =
            std::array::from_fn(|k| taps.get(2 * k + 1).map_or(0.0, |&t| (2.0 * t) as f32));
        Self {
            taps,
            taps_f32,
            even_up_f32,
            odd_up_f32,
        }
    }

    /// Declared base-rate round-trip latency `(TAPS - 1) / 2` frames.
    pub const fn latency_frames() -> usize {
        (TAPS - 1) / 2
    }

    /// Borrow the full `f64` taps.
    #[inline]
    pub fn taps(&self) -> &[f64; TAPS] {
        &self.taps
    }

    /// Borrow the full `f32` taps.
    #[inline]
    pub fn taps_f32(&self) -> &[f32; TAPS] {
        &self.taps_f32
    }
}

/// One channel's 2x halfband upsampler/decimator FIR state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Halfband2x<const TAPS: usize, const HISTORY: usize> {
    /// Base-rate input history, newest first (`input[0]` = newest, `input[HISTORY-1]` = delayed dry).
    input: [f64; HISTORY],
    /// Oversampled shaped history, newest first (`shaped[0]` = newest).
    shaped: [f64; TAPS],
}

impl<const TAPS: usize, const HISTORY: usize> Default for Halfband2x<TAPS, HISTORY> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const TAPS: usize, const HISTORY: usize> Halfband2x<TAPS, HISTORY> {
    pub const fn new() -> Self {
        Self {
            input: [0.0; HISTORY],
            shaped: [0.0; TAPS],
        }
    }

    pub fn reset(&mut self) {
        self.input = [0.0; HISTORY];
        self.shaped = [0.0; TAPS];
    }

    pub fn is_clear(&self) -> bool {
        self.input == [0.0; HISTORY] && self.shaped == [0.0; TAPS]
    }

    /// Push one base-rate sample and return `(delayed_dry, first_2x, second_2x)`
    /// via symmetric polyphase zero-stuffing interpolation (exploiting halfband
    /// zero odd taps and linear-phase symmetry).
    #[inline(always)]
    pub fn upsample_sample(
        &mut self,
        sample: f64,
        kernel: &Halfband2xKernel<TAPS, HISTORY>,
    ) -> (f64, f64, f64) {
        self.input.copy_within(0..HISTORY - 1, 1);
        self.input[0] = sample;
        let taps = &kernel.taps;
        let mut even = 0.0;
        let pairs = HISTORY / 2;
        for k in 0..pairs {
            even += taps[2 * k] * (self.input[k] + self.input[HISTORY - 1 - k]);
        }
        if HISTORY % 2 == 1 {
            even += taps[2 * pairs] * self.input[pairs];
        }
        // For odd halfband kernels (e.g. TAPS = 23, center = 11), only the center odd tap
        // is nonzero when center is odd; otherwise fall back to summing all odd taps.
        let centre = (TAPS - 1) / 2;
        let odd = if centre % 2 == 1 {
            taps[centre] * self.input[centre / 2]
        } else {
            let mut sum = 0.0;
            for k in 0..HISTORY {
                if let Some(&tap) = taps.get(2 * k + 1) {
                    sum += tap * self.input[k];
                }
            }
            sum
        };
        (self.input[HISTORY - 1], 2.0 * even, 2.0 * odd)
    }

    /// Push the two shaped 2x samples (`first` on the even phase, `second` on the
    /// odd phase) and decimate on the even phase so round-trip delay is an exact
    /// whole number of base frames (`(TAPS - 1) / 2`). Exploits halfband zero
    /// taps and linear-phase symmetry.
    #[inline(always)]
    pub fn downsample_sample(
        &mut self,
        first_shaped: f64,
        second_shaped: f64,
        kernel: &Halfband2xKernel<TAPS, HISTORY>,
    ) -> f64 {
        self.shaped.copy_within(0..TAPS - 1, 1);
        self.shaped[0] = first_shaped;
        let taps = &kernel.taps;
        let centre = (TAPS - 1) / 2;
        let wet = if centre % 2 == 1 {
            let mut sum = taps[centre] * self.shaped[centre];
            let pairs = HISTORY / 2;
            for k in 0..pairs {
                let idx = 2 * k;
                sum += taps[idx] * (self.shaped[idx] + self.shaped[TAPS - 1 - idx]);
            }
            sum
        } else {
            dot_f64(&self.shaped, taps)
        };
        self.shaped.copy_within(0..TAPS - 1, 1);
        self.shaped[0] = second_shaped;
        wet
    }

    /// Process one base-rate frame with an arbitrary 2x-rate closure, returning
    /// `(delayed_dry, decimated_wet)`.
    #[inline]
    pub fn process_sample(
        &mut self,
        sample: f64,
        kernel: &Halfband2xKernel<TAPS, HISTORY>,
        mut shape: impl FnMut(f64) -> f64,
    ) -> (f64, f64) {
        let (dry, first_in, second_in) = self.upsample_sample(sample, kernel);
        let first = shape(first_in);
        let second = shape(second_in);
        let wet = self.downsample_sample(first, second, kernel);
        (dry, wet)
    }
}

/// Unrolled four-accumulator dot product for auto-vectorization on fixed FIR arrays.
#[inline(always)]
fn dot_f64<const N: usize>(x: &[f64; N], h: &[f64; N]) -> f64 {
    let mut a0 = 0.0;
    let mut a1 = 0.0;
    let mut a2 = 0.0;
    let mut a3 = 0.0;
    let mut i = 0;
    while i + 4 <= N {
        a0 += x[i] * h[i];
        a1 += x[i + 1] * h[i + 1];
        a2 += x[i + 2] * h[i + 2];
        a3 += x[i + 3] * h[i + 3];
        i += 4;
    }
    let mut tail = 0.0;
    while i < N {
        tail += x[i] * h[i];
        i += 1;
    }
    (a0 + a1) + (a2 + a3) + tail
}

/// Block-oriented SoA stereo 2x halfband upsampler/decimator operating on
/// deinterleaved `&mut [f32]` L/R slices with zero allocations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Halfband2xStereo<const TAPS: usize, const HISTORY: usize> {
    /// Oldest-to-newest base-rate history for L and R (`HISTORY - 1` samples stored
    /// in indices `0..HISTORY - 1`; slot `HISTORY - 1` unused/zero).
    in_hist: [[f32; HISTORY]; 2],
    /// Oldest-to-newest 2x-rate history for L and R (`TAPS - 1` samples stored
    /// in indices `0..TAPS - 1`).
    out_hist: [[f32; TAPS]; 2],
}

impl<const TAPS: usize, const HISTORY: usize> Default for Halfband2xStereo<TAPS, HISTORY> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const TAPS: usize, const HISTORY: usize> Halfband2xStereo<TAPS, HISTORY> {
    pub const fn new() -> Self {
        Self {
            in_hist: [[0.0; HISTORY]; 2],
            out_hist: [[0.0; TAPS]; 2],
        }
    }

    pub fn reset(&mut self) {
        self.in_hist = [[0.0; HISTORY]; 2];
        self.out_hist = [[0.0; TAPS]; 2];
    }

    /// Upsample a stereo SoA block (`input = (left, right)`, `len <= HALFBAND_BLOCK_CHUNK`)
    /// into 2x-rate SoA buffers `up = (up_left, up_right)` (`len >= 2 * n`) and write the
    /// `(TAPS - 1) / 2`-delayed dry signal into `dry = (dry_left, dry_right)`.
    pub fn upsample_stereo_chunk(
        &mut self,
        input: (&[f32], &[f32]),
        up: (&mut [f32], &mut [f32]),
        dry: (&mut [f32], &mut [f32]),
        kernel: &Halfband2xKernel<TAPS, HISTORY>,
    ) {
        let (left, right) = input;
        let (up_left, up_right) = up;
        let (dry_left, dry_right) = dry;
        let n = left
            .len()
            .min(right.len())
            .min(dry_left.len())
            .min(dry_right.len())
            .min(up_left.len() / 2)
            .min(up_right.len() / 2)
            .min(HALFBAND_BLOCK_CHUNK);
        if n == 0 {
            return;
        }
        Self::upsample_channel(
            &mut self.in_hist[0],
            &left[..n],
            &mut up_left[..2 * n],
            &mut dry_left[..n],
            kernel,
        );
        Self::upsample_channel(
            &mut self.in_hist[1],
            &right[..n],
            &mut up_right[..2 * n],
            &mut dry_right[..n],
            kernel,
        );
    }

    fn upsample_channel(
        hist: &mut [f32; HISTORY],
        input: &[f32],
        up: &mut [f32],
        dry: &mut [f32],
        kernel: &Halfband2xKernel<TAPS, HISTORY>,
    ) {
        let n = input.len();
        let pad = HISTORY - 1;
        let mut src = [0.0_f32; HALFBAND_BLOCK_CHUNK + 64];
        debug_assert!(pad + n <= src.len());
        src[..pad].copy_from_slice(&hist[..pad]);
        src[pad..pad + n].copy_from_slice(input);
        // Delayed dry is `HISTORY - 1` samples older = `src[0..n]`.
        dry.copy_from_slice(&src[..n]);

        let mut even = [0.0_f32; HALFBAND_BLOCK_CHUNK];
        let mut odd = [0.0_f32; HALFBAND_BLOCK_CHUNK];
        for k in 0..HISTORY {
            let offset = pad - k;
            let tap_e = kernel.even_up_f32[k];
            let tap_o = kernel.odd_up_f32[k];
            let slice = &src[offset..offset + n];
            if tap_e != 0.0 {
                for (d, &s) in even[..n].iter_mut().zip(slice) {
                    *d += s * tap_e;
                }
            }
            if tap_o != 0.0 {
                for (d, &s) in odd[..n].iter_mut().zip(slice) {
                    *d += s * tap_o;
                }
            }
        }
        for i in 0..n {
            up[2 * i] = even[i];
            up[2 * i + 1] = odd[i];
        }
        hist[..pad].copy_from_slice(&src[n..n + pad]);
    }

    /// Decimate a 2x-rate stereo SoA block (`2 * n` samples per channel) back to
    /// base-rate SoA slices `left[..n]` and `right[..n]`.
    pub fn downsample_stereo_chunk(
        &mut self,
        up_left: &[f32],
        up_right: &[f32],
        left: &mut [f32],
        right: &mut [f32],
        kernel: &Halfband2xKernel<TAPS, HISTORY>,
    ) {
        let n = left
            .len()
            .min(right.len())
            .min(up_left.len() / 2)
            .min(up_right.len() / 2)
            .min(HALFBAND_BLOCK_CHUNK);
        if n == 0 {
            return;
        }
        Self::downsample_channel(
            &mut self.out_hist[0],
            &up_left[..2 * n],
            &mut left[..n],
            kernel,
        );
        Self::downsample_channel(
            &mut self.out_hist[1],
            &up_right[..2 * n],
            &mut right[..n],
            kernel,
        );
    }

    fn downsample_channel(
        hist: &mut [f32; TAPS],
        up: &[f32],
        out: &mut [f32],
        kernel: &Halfband2xKernel<TAPS, HISTORY>,
    ) {
        let n = out.len();
        let pad = TAPS - 1;
        let mut even_buf = [0.0_f32; HALFBAND_BLOCK_CHUNK + 64];
        let mut odd_buf = [0.0_f32; HALFBAND_BLOCK_CHUNK + 64];
        // Split history (oldest-to-newest) into even/odd 2x streams:
        // hist[0] is oldest (`2n` samples ago), hist[pad - 1] is newest previous odd sample.
        // Because pad = TAPS - 1 is even, half = pad / 2 samples per phase.
        let half = pad / 2;
        for i in 0..half {
            even_buf[i] = hist[2 * i];
            odd_buf[i] = hist[2 * i + 1];
        }
        for i in 0..n {
            even_buf[half + i] = up[2 * i];
            odd_buf[half + i] = up[2 * i + 1];
        }
        // Decimate at the even sample `up[2*i]` (i.e. even_buf[half + i]):
        // tap 0 multiplies even_buf[half + i]
        // tap 1 multiplies odd_buf[half + i - 1]
        // tap 2 multiplies even_buf[half + i - 1]
        // tap 2k multiplies even_buf[half + i - k]
        // tap 2k+1 multiplies odd_buf[half + i - 1 - k]
        out.fill(0.0);
        for k in 0..HISTORY {
            let tap_e = kernel.taps_f32[2 * k];
            if tap_e != 0.0 {
                let offset = half - k;
                for (d, &s) in out.iter_mut().zip(&even_buf[offset..offset + n]) {
                    *d += s * tap_e;
                }
            }
            if let Some(&tap_o) = kernel.taps_f32.get(2 * k + 1)
                && tap_o != 0.0
            {
                let offset = half - 1 - k;
                for (d, &s) in out.iter_mut().zip(&odd_buf[offset..offset + n]) {
                    *d += s * tap_o;
                }
            }
        }
        // Save the newest `pad` samples back to hist (oldest-to-newest).
        for i in 0..half {
            hist[2 * i] = even_buf[n + i];
            hist[2 * i + 1] = odd_buf[n + i];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halfband_23_kernel_is_symmetric_and_unit_dc() {
        let k = Halfband2xKernel::<23, 12>::blackman();
        assert!((k.taps().iter().sum::<f64>() - 1.0).abs() < 1e-14);
        for i in 0..23 {
            assert!((k.taps()[i] - k.taps()[22 - i]).abs() < 1e-15);
        }
        assert_eq!(Halfband2xKernel::<23, 12>::latency_frames(), 11);
    }

    #[test]
    fn halfband_stereo_block_matches_scalar_reference() {
        let k = Halfband2xKernel::<23, 12>::blackman();
        let mut scalar_l = Halfband2x::<23, 12>::new();
        let mut scalar_r = Halfband2x::<23, 12>::new();
        let mut block = Halfband2xStereo::<23, 12>::new();

        let input_l: [f32; 48] = std::array::from_fn(|i| ((i as f32) * 0.21).sin());
        let input_r: [f32; 48] = std::array::from_fn(|i| ((i as f32) * 0.37).cos());
        let mut ref_dry_l = [0.0_f32; 48];
        let mut ref_wet_l = [0.0_f32; 48];
        let mut ref_dry_r = [0.0_f32; 48];
        let mut ref_wet_r = [0.0_f32; 48];
        for i in 0..48 {
            let (dl, wl) = scalar_l.process_sample(f64::from(input_l[i]), &k, |x| x.tanh());
            let (dr, wr) = scalar_r.process_sample(f64::from(input_r[i]), &k, |x| x.tanh());
            ref_dry_l[i] = dl as f32;
            ref_wet_l[i] = wl as f32;
            ref_dry_r[i] = dr as f32;
            ref_wet_r[i] = wr as f32;
        }

        let mut up_l = [0.0_f32; 96];
        let mut up_r = [0.0_f32; 96];
        let mut dry_l = [0.0_f32; 48];
        let mut dry_r = [0.0_f32; 48];
        let mut wet_l = [0.0_f32; 48];
        let mut wet_r = [0.0_f32; 48];
        // Process in two irregular chunks (17 + 31) to prove chunk independence.
        for (start, len) in [(0, 17), (17, 31)] {
            block.upsample_stereo_chunk(
                (&input_l[start..start + len], &input_r[start..start + len]),
                (&mut up_l[..2 * len], &mut up_r[..2 * len]),
                (
                    &mut dry_l[start..start + len],
                    &mut dry_r[start..start + len],
                ),
                &k,
            );
            for s in &mut up_l[..2 * len] {
                *s = s.tanh();
            }
            for s in &mut up_r[..2 * len] {
                *s = s.tanh();
            }
            block.downsample_stereo_chunk(
                &up_l[..2 * len],
                &up_r[..2 * len],
                &mut wet_l[start..start + len],
                &mut wet_r[start..start + len],
                &k,
            );
        }

        for i in 0..48 {
            assert_eq!(dry_l[i], ref_dry_l[i], "dry_l[{i}]");
            assert_eq!(dry_r[i], ref_dry_r[i], "dry_r[{i}]");
            assert!((wet_l[i] - ref_wet_l[i]).abs() < 2e-6, "wet_l[{i}]");
            assert!((wet_r[i] - ref_wet_r[i]).abs() < 2e-6, "wet_r[{i}]");
        }
    }
}
