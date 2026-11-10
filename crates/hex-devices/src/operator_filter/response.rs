//! Static transfer of the same prepared TPT sections used by the carrier.
use super::Prepared;

impl Prepared {
    /// Authored preparation has binary wet/cascade switches, unlike a live ramp.
    pub(crate) fn magnitude_at_hz(self, frequency: f32) -> f32 {
        let frame = self.frame;
        if frame.wet == 0.0 {
            return 1.0;
        }
        let s =
            (std::f64::consts::PI * f64::from(frequency) / f64::from(self.rate)).tan() / frame.g;
        let [low, band, high] = frame.weights;
        let numerator = (low - high * s * s).powi(2) + (band * s).powi(2);
        let denominator = (1.0 - s * s).powi(2) + (frame.k * s).powi(2);
        let squared = numerator / denominator;
        if frame.cascade == 0.0 {
            squared.sqrt() as f32
        } else {
            squared as f32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Carrier, FIRST, Settings};
    use hex_lattice_contract::{ParameterId, ParameterValue};

    #[test]
    fn prepared_magnitude_matches_independent_impulse_dft_for_every_topology() {
        for rate in [8_000, 48_000] {
            for mode in 0..5 {
                for slope in 0..2 {
                    for resonance in [0.0, 100.0] {
                        for morph in [25.0, 75.0] {
                            let mut settings = Settings::default();
                            let cutoff = rate as f32 * 0.1;
                            for (offset, value) in [
                                (0, ParameterValue::Bool(true)),
                                (1, ParameterValue::Choice(mode)),
                                (2, ParameterValue::Choice(slope)),
                                (4, ParameterValue::Float(cutoff)),
                                (5, ParameterValue::Float(resonance)),
                                (6, ParameterValue::Float(morph)),
                            ] {
                                settings
                                    .patch(ParameterId::from_static(FIRST + offset), value)
                                    .unwrap();
                            }
                            let prepared = settings.prepare(rate).unwrap();
                            let mut carrier = Carrier::default();
                            let impulse: Vec<_> = (0..8192)
                                .map(|i| {
                                    carrier.process(if i == 0 { 1.0 } else { 0.0 }, prepared.frame)
                                })
                                .collect();
                            for frequency in [cutoff * 0.25, cutoff, cutoff * 2.0] {
                                let phase =
                                    std::f64::consts::TAU * f64::from(frequency) / f64::from(rate);
                                let (sin, cos) = phase.sin_cos();
                                let (mut x, mut y, mut re, mut im) = (1.0, 0.0, 0.0, 0.0);
                                for sample in &impulse {
                                    re += sample * x;
                                    im += sample * y;
                                    (x, y) = (x * cos + y * sin, y * cos - x * sin);
                                }
                                let measured = re.hypot(im);
                                let response = f64::from(prepared.magnitude_at_hz(frequency));
                                assert!(
                                    (response - measured).abs() < 2e-5 * (1.0 + measured),
                                    "mode={mode} slope={slope} resonance={resonance} morph={morph} rate={rate} hz={frequency}: response={response}, dft={measured}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
