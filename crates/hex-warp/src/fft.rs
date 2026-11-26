//! Radix-two FFT with precomputed bit-reversal and twiddle tables plus
//! real-valued STFT forward/inverse transforms with explicit inverse normalization.
use crate::Error;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Default)]
pub struct Complex {
    pub re: f64,
    pub im: f64,
}

impl Complex {
    pub const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }
    pub fn polar(magnitude: f64, phase: f64) -> Self {
        Self {
            re: magnitude * phase.cos(),
            im: magnitude * phase.sin(),
        }
    }
    pub fn magnitude(self) -> f64 {
        self.re.hypot(self.im)
    }
    pub fn phase(self) -> f64 {
        self.im.atan2(self.re)
    }
    #[inline]
    pub(crate) fn mul(self, other: Self) -> Self {
        Self {
            re: self.re * other.re - self.im * other.im,
            im: self.re * other.im + self.im * other.re,
        }
    }
    #[inline]
    pub(crate) fn conj(self) -> Self {
        Self {
            re: self.re,
            im: -self.im,
        }
    }
}

/// Precomputed bit-reversal permutation, per-stage exact twiddles, and real-FFT
/// unpack/pack twiddles for a single power-of-two size `n in 2..=8192`.
struct FftPlan {
    bit_rev: Vec<u16>,
    /// Stage twiddles packed sequentially for stage lengths `2, 4, ..., n`.
    /// Stage of length `len` occupies offset `len / 2 - 1 .. len - 1` (`len / 2` entries).
    fwd_twiddles: Vec<Complex>,
    inv_twiddles: Vec<Complex>,
    /// Real-FFT twiddles `e^{-i 2π k / n}` for `k in 0..=n/4` (used when `n >= 4`).
    real_fwd_twiddles: Vec<Complex>,
    /// Inverse real-FFT twiddles `e^{+i 2π k / n}` for `k in 0..=n/4` (used when `n >= 4`).
    real_inv_twiddles: Vec<Complex>,
}

impl FftPlan {
    fn build(n: usize) -> Self {
        let bits = n.trailing_zeros();
        let mut bit_rev = Vec::with_capacity(n);
        for i in 0..n {
            let rev = (i as u32).reverse_bits() >> (32 - bits);
            bit_rev.push(rev as u16);
        }

        let mut fwd_twiddles = Vec::with_capacity(n.saturating_sub(1));
        let mut inv_twiddles = Vec::with_capacity(n.saturating_sub(1));
        let mut length = 2;
        while length <= n {
            let half = length / 2;
            let step = 2.0 * std::f64::consts::PI / length as f64;
            for k in 0..half {
                let angle = step * k as f64;
                let (sin, cos) = angle.sin_cos();
                fwd_twiddles.push(Complex::new(cos, -sin));
                inv_twiddles.push(Complex::new(cos, sin));
            }
            length *= 2;
        }

        let quarter = n / 4;
        let mut real_fwd_twiddles = Vec::with_capacity(quarter + 1);
        let mut real_inv_twiddles = Vec::with_capacity(quarter + 1);
        let real_step = 2.0 * std::f64::consts::PI / n as f64;
        for k in 0..=quarter {
            let angle = real_step * k as f64;
            let (sin, cos) = angle.sin_cos();
            real_fwd_twiddles.push(Complex::new(cos, -sin));
            real_inv_twiddles.push(Complex::new(cos, sin));
        }

        Self {
            bit_rev,
            fwd_twiddles,
            inv_twiddles,
            real_fwd_twiddles,
            real_inv_twiddles,
        }
    }

    #[inline]
    fn execute_complex(&self, values: &mut [Complex], inverse: bool) {
        let n = values.len();
        for (i, &rev) in self.bit_rev.iter().enumerate() {
            let j = usize::from(rev);
            if i < j {
                values.swap(i, j);
            }
        }
        let twiddles = if inverse {
            &self.inv_twiddles
        } else {
            &self.fwd_twiddles
        };
        let mut length = 2;
        let mut twiddle_offset = 0;
        while length <= n {
            let half = length / 2;
            let stage = &twiddles[twiddle_offset..twiddle_offset + half];
            twiddle_offset += half;
            for block in values.chunks_exact_mut(length) {
                let (lo, hi) = block.split_at_mut(half);
                for ((a, b), &w) in lo.iter_mut().zip(hi.iter_mut()).zip(stage) {
                    let t = b.mul(w);
                    let a_val = *a;
                    *a = Complex::new(a_val.re + t.re, a_val.im + t.im);
                    *b = Complex::new(a_val.re - t.re, a_val.im - t.im);
                }
            }
            length *= 2;
        }
        if inverse {
            let scale = 1.0 / n as f64;
            for value in values {
                value.re *= scale;
                value.im *= scale;
            }
        }
    }
}

/// Process-wide cache of precomputed FFT plans for sizes `2^1 ..= 2^13` (`2..=8192`).
static PLANS: [OnceLock<FftPlan>; 14] = [const { OnceLock::new() }; 14];

fn plan_for(n: usize) -> Result<&'static FftPlan, Error> {
    if !(2..=8_192).contains(&n) || !n.is_power_of_two() {
        return Err(Error::Invalid(
            "fft length must be a power of two within 2..8192",
        ));
    }
    let idx = n.trailing_zeros() as usize;
    Ok(PLANS[idx].get_or_init(|| FftPlan::build(n)))
}

pub fn transform(values: &mut [Complex], inverse: bool) -> Result<(), Error> {
    let plan = plan_for(values.len())?;
    plan.execute_complex(values, inverse);
    Ok(())
}

/// Real-valued forward FFT for real input staged in `values[i].re` (`im == 0.0`).
///
/// Computes exact positive-frequency bins `0..=n/2` using a single `n/2`-point
/// complex FFT and precomputed unpack twiddles, cutting STFT analysis work in half.
/// Bins `n/2 + 1 .. n` are left unspecified (callers only consume `0..=n/2`).
pub fn transform_real_forward(values: &mut [Complex]) -> Result<(), Error> {
    let n = values.len();
    if n == 2 {
        return transform(values, false);
    }
    let full_plan = plan_for(n)?;
    let half = n / 2;
    let half_plan = plan_for(half)?;

    // Pack real pairs `(x[2k], x[2k+1])` into `values[0..half]`.
    for k in 0..half {
        values[k] = Complex::new(values[2 * k].re, values[2 * k + 1].re);
    }
    half_plan.execute_complex(&mut values[..half], false);

    // Unpack DC (`0`) and Nyquist (`half`).
    let z0 = values[0];
    values[0] = Complex::new(z0.re + z0.im, 0.0);
    values[half] = Complex::new(z0.re - z0.im, 0.0);

    // Unpack interior pairs `(k, half - k)` for `k in 1..=quarter`.
    let quarter = half / 2;
    let twiddles = &full_plan.real_fwd_twiddles;
    for k in 1..quarter {
        let m = half - k;
        let zk = values[k];
        let zm_conj = values[m].conj();
        let even = Complex::new(0.5 * (zk.re + zm_conj.re), 0.5 * (zk.im + zm_conj.im));
        // -0.5i * (zk - conj(zm))
        let odd_rot = Complex::new(0.5 * (zk.im - zm_conj.im), -0.5 * (zk.re - zm_conj.re));
        let t = odd_rot.mul(twiddles[k]);
        values[k] = Complex::new(even.re + t.re, even.im + t.im);
        values[m] = Complex::new(even.re - t.re, -(even.im - t.im));
    }
    if quarter > 0 {
        let zq = values[quarter];
        values[quarter] = zq.conj();
    }
    Ok(())
}

/// Real-valued inverse FFT from positive-frequency Hermitian bins `values[0..=n/2]`.
///
/// Writes normalized real time-domain output to `values[0..n].re` (with `.im = 0.0`)
/// using a single `n/2`-point complex inverse FFT, cutting STFT synthesis work in half.
pub fn transform_real_inverse(values: &mut [Complex]) -> Result<(), Error> {
    let n = values.len();
    if n == 2 {
        values[0].im = 0.0;
        values[1].im = 0.0;
        return transform(values, true);
    }
    let full_plan = plan_for(n)?;
    let half = n / 2;
    let half_plan = plan_for(half)?;

    let dc = values[0].re;
    let nyq = values[half].re;
    values[0] = Complex::new(0.5 * (dc + nyq), 0.5 * (dc - nyq));

    let quarter = half / 2;
    let twiddles = &full_plan.real_inv_twiddles;
    for k in 1..quarter {
        let m = half - k;
        let xk = values[k];
        let xm_conj = values[m].conj();
        let even = Complex::new(0.5 * (xk.re + xm_conj.re), 0.5 * (xk.im + xm_conj.im));
        let diff = Complex::new(0.5 * (xk.re - xm_conj.re), 0.5 * (xk.im - xm_conj.im));
        let rot = diff.mul(twiddles[k]);
        // +i * rot = (-rot.im, rot.re)
        let i_rot = Complex::new(-rot.im, rot.re);
        values[k] = Complex::new(even.re + i_rot.re, even.im + i_rot.im);
        values[m] = Complex::new(even.re - i_rot.re, -(even.im - i_rot.im));
    }
    if quarter > 0 {
        let xq = values[quarter];
        values[quarter] = xq.conj();
    }

    half_plan.execute_complex(&mut values[..half], true);

    // Unpack in reverse order so in-place writes never overwrite unread `values[..half]`.
    for k in (0..half).rev() {
        let z = values[k];
        values[2 * k + 1] = Complex::new(z.im, 0.0);
        values[2 * k] = Complex::new(z.re, 0.0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_dft_inverse_and_normalization() {
        for n in [2, 8, 64] {
            let input: Vec<_> = (0..n)
                .map(|i| Complex::new((i as f64 * 0.37).sin(), (i as f64 * 0.19).cos()))
                .collect();
            let mut fft = input.clone();
            transform(&mut fft, false).unwrap();
            for (k, actual) in fft.iter().enumerate() {
                let mut expected = Complex::default();
                for (i, value) in input.iter().enumerate() {
                    let term = value.mul(Complex::polar(
                        1.0,
                        -2.0 * std::f64::consts::PI * k as f64 * i as f64 / n as f64,
                    ));
                    expected.re += term.re;
                    expected.im += term.im;
                }
                assert!((actual.re - expected.re).abs() < 1e-9);
                assert!((actual.im - expected.im).abs() < 1e-9);
            }
            transform(&mut fft, true).unwrap();
            for (actual, expected) in fft.iter().zip(input) {
                assert!((actual.re - expected.re).abs() < 1e-10);
                assert!((actual.im - expected.im).abs() < 1e-10);
            }
        }
        assert!(transform(&mut [Complex::default(); 3], false).is_err());
    }

    #[test]
    fn real_fft_matches_complex_fft_across_stft_sizes_up_to_8192() {
        for n in [2, 4, 8, 64, 512, 4096, 8192] {
            let input: Vec<_> = (0..n)
                .map(|i| {
                    let x = (i as f64 * 0.23).sin() + 0.4 * (i as f64 * 0.71).cos();
                    Complex::new(x, 0.0)
                })
                .collect();
            let mut complex_spec = input.clone();
            transform(&mut complex_spec, false).unwrap();

            let mut real_spec = input.clone();
            transform_real_forward(&mut real_spec).unwrap();
            for bin in 0..=n / 2 {
                assert!(
                    (real_spec[bin].re - complex_spec[bin].re).abs() < 1e-10,
                    "n={n} bin={bin} re mismatch: {} vs {}",
                    real_spec[bin].re,
                    complex_spec[bin].re
                );
                assert!(
                    (real_spec[bin].im - complex_spec[bin].im).abs() < 1e-10,
                    "n={n} bin={bin} im mismatch: {} vs {}",
                    real_spec[bin].im,
                    complex_spec[bin].im
                );
            }

            // Round-trip via real inverse FFT.
            transform_real_inverse(&mut real_spec).unwrap();
            for (i, (actual, expected)) in real_spec.iter().zip(&input).enumerate() {
                assert!(
                    (actual.re - expected.re).abs() < 1e-11,
                    "n={n} sample={i} inverse mismatch: {} vs {}",
                    actual.re,
                    expected.re
                );
                assert_eq!(actual.im, 0.0);
            }
        }
    }
}
