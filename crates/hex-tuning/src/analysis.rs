//! Harmonic relationship estimates used by the Freqs editor.

use std::sync::OnceLock;

use crate::scale::{MAX_DEGREES, Scale};

/// Scale-wide coordinates in the palette landscape.
#[derive(Clone, Copy, Debug, Default)]
pub struct Coordinates {
    /// Mean pair simplicity.
    pub cohesion: f32,
    /// Mean pair spectral interaction.
    pub energy: f32,
    /// Mean non-root affinity to the selected root.
    pub root_gravity: f32,
}

/// Pair, note, and scale metrics for one scale snapshot.
#[derive(Clone, Copy, Debug)]
pub struct Analysis {
    /// Symmetric harmonic simplicity matrix.
    pub pair_fit: [[f32; MAX_DEGREES]; MAX_DEGREES],
    /// Symmetric spectral interaction matrix.
    pub pair_energy: [[f32; MAX_DEGREES]; MAX_DEGREES],
    /// Mean fit of each degree to every other degree.
    pub note_cohesion: [f32; MAX_DEGREES],
    /// Mean spectral interaction of each degree.
    pub note_energy: [f32; MAX_DEGREES],
    /// Harmonic affinity of each degree to the root.
    pub root_fit: [f32; MAX_DEGREES],
    /// Whole-scale metrics.
    pub coordinates: Coordinates,
}

impl Analysis {
    /// Analyze all active notes in the scale.
    pub fn new(scale: Scale) -> Self {
        let mut result = Self {
            pair_fit: [[0.0; MAX_DEGREES]; MAX_DEGREES],
            pair_energy: [[0.0; MAX_DEGREES]; MAX_DEGREES],
            note_cohesion: [0.0; MAX_DEGREES],
            note_energy: [0.0; MAX_DEGREES],
            root_fit: [0.0; MAX_DEGREES],
            coordinates: Coordinates::default(),
        };
        let mut fit_total = 0.0;
        let mut energy_total = 0.0;
        let mut pair_count = 0;
        for a in 0..scale.count() {
            for b in a + 1..scale.count() {
                let fit = pair_fit((scale.cents()[a] - scale.cents()[b]).abs());
                let energy = roughness(degree_frequency(scale, a), degree_frequency(scale, b));
                result.pair_fit[a][b] = fit;
                result.pair_fit[b][a] = fit;
                result.pair_energy[a][b] = energy;
                result.pair_energy[b][a] = energy;
                result.note_cohesion[a] += fit;
                result.note_cohesion[b] += fit;
                result.note_energy[a] += energy;
                result.note_energy[b] += energy;
                fit_total += fit;
                energy_total += energy;
                pair_count += 1;
            }
        }
        for index in 0..scale.count() {
            result.note_cohesion[index] /= (scale.count() - 1) as f32;
            result.note_energy[index] /= (scale.count() - 1) as f32;
            result.root_fit[index] = if index == scale.root_degree() {
                1.0
            } else {
                result.pair_fit[index][scale.root_degree()]
            };
        }
        result.coordinates.cohesion = fit_total / pair_count as f32;
        result.coordinates.energy = energy_total / pair_count as f32;
        result.coordinates.root_gravity = result.root_fit[..scale.count()]
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != scale.root_degree())
            .map(|(_, value)| *value)
            .sum::<f32>()
            / (scale.count() - 1) as f32;
        result
    }

    /// Three strongest three-note combinations containing the root.
    pub fn root_shapes(&self, scale: Scale) -> [(usize, usize, f32); 3] {
        let mut shapes = Vec::new();
        for a in 0..scale.count() {
            if a == scale.root_degree() {
                continue;
            }
            for b in a + 1..scale.count() {
                if b == scale.root_degree() {
                    continue;
                }
                let values = [
                    self.pair_fit[scale.root_degree()][a],
                    self.pair_fit[scale.root_degree()][b],
                    self.pair_fit[a][b],
                ];
                let weakest = values.into_iter().fold(f32::INFINITY, f32::min);
                let geometric = (values[0] * values[1] * values[2]).cbrt();
                let roughness = (self.pair_energy[scale.root_degree()][a]
                    + self.pair_energy[scale.root_degree()][b]
                    + self.pair_energy[a][b])
                    / 3.0;
                shapes.push((
                    a,
                    b,
                    0.58 * weakest + 0.27 * geometric + 0.15 * (1.0 - roughness),
                ));
            }
        }
        shapes.sort_by(|a, b| b.2.total_cmp(&a.2));
        std::array::from_fn(|index| shapes.get(index).copied().unwrap_or((0, 0, 0.0)))
    }
}

fn degree_frequency(scale: Scale, degree: usize) -> f64 {
    let interval = f64::from(scale.cents()[degree] - scale.cents()[scale.root_degree()]);
    scale.root_hz() * 2.0_f64.powf(interval / 1200.0)
}

fn roughness(a: f64, b: f64) -> f32 {
    let mut energy = 0.0;
    for partial_a in 1..=8 {
        for partial_b in 1..=8 {
            let f1 = a * f64::from(partial_a);
            let f2 = b * f64::from(partial_b);
            let x = (f2 - f1).abs() * (0.24 / (0.021 * f1.min(f2) + 19.0));
            let interaction = ((-3.5 * x).exp() - (-5.75 * x).exp()).max(0.0);
            energy += interaction / f64::from(partial_a * partial_b).powf(1.15);
        }
    }
    (energy / 0.18).clamp(0.0, 1.0) as f32
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

fn landmarks() -> &'static [(f32, f32)] {
    static RATIOS: OnceLock<Vec<(f32, f32)>> = OnceLock::new();
    RATIOS.get_or_init(|| {
        let mut ratios = Vec::new();
        for q in 1..=16 {
            for p in q..=4 * q {
                if gcd(p, q) != 1 {
                    continue;
                }
                let cents = 1200.0 * (p as f32 / q as f32).log2();
                let simplicity = 1.0 / (1.0 + 0.17 * ((p * q) as f32).log2());
                ratios.push((cents, simplicity));
            }
        }
        ratios
    })
}

/// Simplicity score for one absolute interval in cents.
pub fn pair_fit(interval: f32) -> f32 {
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        (0..=2400)
            .map(|cent| {
                landmarks()
                    .iter()
                    .map(|(landmark, simplicity)| {
                        simplicity * (-((cent as f32) - landmark).abs() / 38.0).exp()
                    })
                    .fold(0.0, f32::max)
                    .clamp(0.0, 1.0)
            })
            .collect()
    });
    let position = interval.clamp(0.0, 2400.0);
    let lower = position.floor() as usize;
    let upper = (lower + 1).min(2400);
    table[lower] + (table[upper] - table[lower]) * position.fract()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_simple_ratios_score_better_than_nearby_mistunings() {
        let exact = 1200.0 * (3.0_f32 / 2.0).log2();
        assert!(pair_fit(exact) > pair_fit(exact + 24.0));
        assert!(pair_fit(exact) > pair_fit(exact - 24.0));
    }

    #[test]
    fn analysis_is_symmetric_and_root_dependent() {
        let scale = Scale::default();
        let before = Analysis::new(scale);
        assert_eq!(before.pair_fit[0][4], before.pair_fit[4][0]);
        let mut scale = scale;
        scale
            .set_reference(scale.root_key(), scale.root_hz(), 4)
            .unwrap();
        let after = Analysis::new(scale);
        assert_eq!(after.root_fit[4], 1.0);
        assert_eq!(after.root_fit[0], before.pair_fit[0][4]);
    }
}
