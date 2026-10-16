//! Original bounded additive kernel for harmonic authoring. Not yet registered
//! as an Operator state/waveform; authoring domain and controls are separate.
use crate::{Invalid, SineTable};
pub(crate) const PARTIALS: usize = 64;
const NYQUIST: u128 = 1_u128 << 63;

/// Prepared normalized target, with f64 applied coefficients. Source-boundary
/// interpolation must retain f64 values rather than requantize these to f32.
/// L1 normalization bounds waveform peak by one at every phase, including after
/// rate-dependent partial removal; it is an original Hex law, not Live parity.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HarmonicSpectrum {
    coefficients: [f64; PARTIALS],
}
impl HarmonicSpectrum {
    /// Kernel domain permits signed bounded coefficients; selecting a visible
    /// painter domain does not follow from this mathematical representation.
    pub(crate) fn prepare(authored: [f32; PARTIALS]) -> Result<Self, Invalid> {
        if authored.iter().any(|v| !v.is_finite() || v.abs() > 1.0) {
            return Err(Invalid("invalid harmonic coefficient"));
        }
        let sum: f64 = authored.iter().map(|v| f64::from(*v).abs()).sum();
        let divisor = sum.max(1.0);
        Ok(Self {
            coefficients: authored.map(|v| f64::from(v) / divisor),
        })
    }
    pub(crate) const fn coefficients(&self) -> &[f64; PARTIALS] {
        &self.coefficients
    }

    /// Validated maximum unmodulated phase step, including the provider's
    /// conservative LFO excursion bound. This excludes harmonics at Nyquist.
    /// PM/FM sidebands are not proven bandlimited by this base-frequency cap;
    /// the production renderer needs separate modulation/aliasing acceptance.
    pub(crate) fn cap(max_step: u64) -> Result<usize, Invalid> {
        if max_step == 0 || u128::from(max_step) >= NYQUIST {
            return Err(Invalid("invalid harmonic phase bound"));
        }
        Ok((1..=PARTIALS)
            .take_while(|h| u128::from(max_step) * (*h as u128) < NYQUIST)
            .count())
    }
    /// Caller supplies a prepared cap1..64. Invalid caps are rejected, never
    /// treated as silent sound. No allocation, initialization or mutable state.
    pub(crate) fn sample(&self, sine: &SineTable, phase: u64, cap: usize) -> Result<f64, Invalid> {
        if !(1..=PARTIALS).contains(&cap) {
            return Err(Invalid("invalid harmonic cap"));
        }
        Ok(self.coefficients[..cap]
            .iter()
            .enumerate()
            .map(|(index, amplitude)| {
                amplitude * f64::from(sine.sample_phase(phase.wrapping_mul(index as u64 + 1)))
            })
            .sum())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    const SCALE: f64 = 18_446_744_073_709_551_616.0;
    #[test]
    fn fundamental_is_exact_original_sine_and_invalid_inputs_are_refused() {
        let table = SineTable::new();
        let mut raw = [0.0; PARTIALS];
        raw[0] = 1.0;
        let spectrum = HarmonicSpectrum::prepare(raw).unwrap();
        for phase in [0, 1, 1_u64 << 62, 1_u64 << 63, u64::MAX] {
            assert_eq!(
                spectrum.sample(&table, phase, 64).unwrap(),
                f64::from(table.sample_phase(phase))
            );
        }
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.0001, -1.0001] {
            raw[63] = bad;
            assert!(HarmonicSpectrum::prepare(raw).is_err());
        }
        assert!(spectrum.sample(&table, 0, 0).is_err());
        assert!(spectrum.sample(&table, 0, 65).is_err());
        assert!(HarmonicSpectrum::cap(0).is_err());
        assert!(HarmonicSpectrum::cap(1 << 63).is_err());
    }
    #[test]
    fn normalization_and_convex_source_interpolation_have_finite_peak_bound() {
        let a = HarmonicSpectrum::prepare([1.0; PARTIALS]).unwrap();
        let b =
            HarmonicSpectrum::prepare(std::array::from_fn(|i| if i % 2 == 0 { -1.0 } else { 0.5 }))
                .unwrap();
        let table = SineTable::new();
        for fraction in [0.0, 0.01, 0.5, 0.99, 1.0] {
            let mid = HarmonicSpectrum {
                coefficients: std::array::from_fn(|i| {
                    a.coefficients()[i] + (b.coefficients()[i] - a.coefficients()[i]) * fraction
                }),
            };
            assert!(mid.coefficients().iter().map(|v| v.abs()).sum::<f64>() <= 1.0 + 1e-14);
            for index in 0..2048 {
                let phase = (index as u64) << 53;
                let value = mid.sample(&table, phase, 64).unwrap();
                assert!(value.is_finite() && value.abs() <= 1.0 + 1e-14);
            }
        }
    }
    #[test]
    fn harmonic_64_is_real_output_and_at_nyquist_is_excluded() {
        let mut raw = [0.0; PARTIALS];
        raw[63] = 1.0;
        let spectrum = HarmonicSpectrum::prepare(raw).unwrap();
        let table = SineTable::new();
        let step = 1_u64 << 50;
        let mut energy = 0.0;
        for age in 0..4096_u64 {
            let value = spectrum.sample(&table, age.wrapping_mul(step), 64).unwrap();
            energy += value * value;
        }
        assert!(energy > 2000.0);
        assert_eq!(HarmonicSpectrum::cap(1_u64 << 57).unwrap(), 63);
        assert_eq!(HarmonicSpectrum::cap((1_u64 << 57) - 1).unwrap(), 64);
        assert_eq!(spectrum.sample(&table, 1 << 55, 63).unwrap(), 0.0);
    }
    #[test]
    fn coherent_pcm_spectrum_matches_authored_bins_without_other_harmonics() {
        let table = SineTable::new();
        let mut raw = [0.0; PARTIALS];
        raw[0] = 0.5;
        raw[3] = 0.25;
        raw[63] = 0.25;
        let spectrum = HarmonicSpectrum::prepare(raw).unwrap();
        let n = 4096;
        let pcm: Vec<_> = (0..n)
            .map(|i| spectrum.sample(&table, (i as u64) << 52, 64).unwrap())
            .collect();
        for h in 1..=64 {
            let measured: f64 = pcm
                .iter()
                .enumerate()
                .map(|(i, v)| v * (std::f64::consts::TAU * i as f64 * h as f64 / n as f64).sin())
                .sum::<f64>()
                * 2.0
                / n as f64;
            assert!(
                (measured - f64::from(raw[h - 1])).abs() < 2e-6,
                "harmonic={h} amplitude={measured}"
            );
        }
        for rate in [8000, 44100, 48000, 192000] {
            let max_step = (440.0 / f64::from(rate) * SCALE).round() as u64;
            let cap = HarmonicSpectrum::cap(max_step).unwrap();
            assert!(u128::from(max_step) * (cap as u128) < NYQUIST);
            if cap < 64 {
                assert!(u128::from(max_step) * (cap as u128 + 1) >= NYQUIST);
            }
        }
    }
}
