//! Original iterative radix-two FFT with explicit inverse normalization.
use crate::Error;
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
    pub(crate) fn mul(self, other: Self) -> Self {
        Self {
            re: self.re * other.re - self.im * other.im,
            im: self.re * other.im + self.im * other.re,
        }
    }
}
pub fn transform(values: &mut [Complex], inverse: bool) -> Result<(), Error> {
    let n = values.len();
    if !(2..=8_192).contains(&n) || !n.is_power_of_two() {
        return Err(Error::Invalid(
            "fft length must be a power of two within 2..8192",
        ));
    }
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            values.swap(i, j);
        }
    }
    let sign = if inverse { 1.0 } else { -1.0 };
    let mut length = 2;
    while length <= n {
        let rotation = Complex::polar(1.0, sign * 2.0 * std::f64::consts::PI / length as f64);
        for start in (0..n).step_by(length) {
            let mut twiddle = Complex::new(1.0, 0.0);
            for offset in 0..length / 2 {
                let a = values[start + offset];
                let b = values[start + offset + length / 2].mul(twiddle);
                values[start + offset] = Complex::new(a.re + b.re, a.im + b.im);
                values[start + offset + length / 2] = Complex::new(a.re - b.re, a.im - b.im);
                twiddle = twiddle.mul(rotation);
            }
        }
        length *= 2;
    }
    if inverse {
        for value in values {
            value.re /= n as f64;
            value.im /= n as f64;
        }
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
}
