//! Original bounded additive evaluation and immutable process-lifetime phase basis.
use super::*;

const PHASE_BITS: u32 = 17;
pub(super) struct HarmonicPhaseTable {
    sine: Box<[f64]>,
}
impl HarmonicPhaseTable {
    fn lookup(&self, phase: u64) -> f64 {
        let index = (phase >> (64 - PHASE_BITS)) as usize;
        let fraction = (phase & ((1_u64 << (64 - PHASE_BITS)) - 1)) as f64
            / (1_u64 << (64 - PHASE_BITS)) as f64;
        self.sine[index] + (self.sine[index + 1] - self.sine[index]) * fraction
    }
    fn sample(&self, phase: u64) -> (f64, f64) {
        (
            self.lookup(phase),
            self.lookup(phase.wrapping_add(1_u64 << 62)),
        )
    }
}
pub(super) fn harmonic_phase_table() -> &'static HarmonicPhaseTable {
    static TABLE: std::sync::OnceLock<HarmonicPhaseTable> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let count = 1_usize << PHASE_BITS;
        let sine = (0..=count)
            .map(|i| (i as f64 * std::f64::consts::TAU / count as f64).sin())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        HarmonicPhaseTable { sine }
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::operator_state) struct Spectrum {
    pub(super) coefficients: [[f64; 64]; 4],
    pub(super) builtin: [[f64; 4]; 4],
}
impl Spectrum {
    pub(super) fn prepare(settings: &OperatorSettings) -> Self {
        let mut out = Self {
            coefficients: [[0.0; 64]; 4],
            builtin: [[0.0; 4]; 4],
        };
        for operator in 0..4 {
            if !settings.harmonics.custom[operator] {
                out.builtin[operator][settings.waveforms[operator].choice() as usize] = 1.0;
                continue;
            }
            let count = settings.harmonics.active_count(operator);
            let copies = if settings.harmonics.repeat[operator] == 0 {
                1
            } else {
                1 << settings.harmonics.repeat[operator]
            };
            let pattern = count / copies;
            for index in 0..count {
                out.coefficients[operator][index] =
                    f64::from(settings.harmonics.amplitudes[operator][index % pattern])
                        / (index / pattern + 1) as f64;
            }
            let normalization = out.coefficients[operator].iter().sum::<f64>().max(1.0);
            for value in &mut out.coefficients[operator] {
                *value /= normalization;
            }
        }
        out
    }
    pub(super) fn at(source: Self, target: Self, amount: f64) -> Self {
        if amount == 0.0 {
            return source;
        }
        if amount == 1.0 {
            return target;
        }
        Self {
            coefficients: std::array::from_fn(|i| {
                std::array::from_fn(|j| {
                    source.coefficients[i][j]
                        + (target.coefficients[i][j] - source.coefficients[i][j]) * amount
                })
            }),
            builtin: std::array::from_fn(|i| {
                std::array::from_fn(|j| {
                    source.builtin[i][j] + (target.builtin[i][j] - source.builtin[i][j]) * amount
                })
            }),
        }
    }
    pub(super) fn sample(
        &self,
        operator: usize,
        phase: u64,
        step: u64,
        sine: &SineTable,
        bank: &OperatorWaveformBank,
        phase_table: &HarmonicPhaseTable,
    ) -> f32 {
        let mut value = 0.0;
        // Keep the original full builtin bank, including its 512-partial caps.
        for (shape, weight) in self.builtin[operator].iter().copied().enumerate() {
            if weight == 0.0 {
                continue;
            }
            let sample = if shape == 0 {
                sine.sample_phase(phase)
            } else {
                bank.sample_phase(
                    OperatorWaveform::from_choice(shape as u32).unwrap_or(OperatorWaveform::Sine),
                    harmonic_cap_index(step).unwrap_or(0),
                    phase,
                )
            };
            value += f64::from(sample) * weight;
        }
        let cap = if step == 0 {
            64
        } else {
            (((1u128 << 63) - 1) / u128::from(step)).min(64) as usize
        };
        // Clenshaw's sine-series recurrence: two immutable basis lookups, no per-partial trig.
        if self.coefficients[operator][..cap].iter().all(|v| *v == 0.0) {
            return value as f32;
        }
        let (sin, cosine) = phase_table.sample(phase);
        if cap <= 8 {
            let mut b1 = 0.0;
            let mut b2 = 0.0;
            for coefficient in self.coefficients[operator][..cap].iter().rev() {
                let b0 = coefficient + 2.0 * cosine * b1 - b2;
                b2 = b1;
                b1 = b0;
            }
            return (value + sin * b1) as f32;
        }
        // Split n = 8j+r into eight independent Clenshaw recurrences. This
        // shortens dependency chains and lets portable LLVM vectorization work
        // across residues without unsafe code or a CPU-specific requirement.
        let cos2 = 2.0 * cosine * cosine - 1.0;
        let cos4 = 2.0 * cos2 * cos2 - 1.0;
        let sin4 = 4.0 * sin * cosine * cos2;
        let cos8 = 2.0 * cos4 * cos4 - 1.0;
        let sin8 = 2.0 * sin4 * cos4;
        let groups = cap.div_ceil(8);
        let mut b1: [f64; 8] = std::array::from_fn(|r| {
            let index = (groups - 1) * 8 + r;
            if index < cap {
                self.coefficients[operator][index]
            } else {
                0.0
            }
        });
        let mut b2 = [0.0; 8];
        for group in (0..groups - 1).rev() {
            let next = std::array::from_fn(|r| {
                self.coefficients[operator][group * 8 + r] + 2.0 * cos8 * b1[r] - b2[r]
            });
            b2 = b1;
            b1 = next;
        }
        let mut sr = sin;
        let mut cr = cosine;
        for r in 0..8 {
            value += sr * (b1[r] - cos8 * b2[r]) + cr * sin8 * b2[r];
            (sr, cr) = (sr * cosine + cr * sin, cr * cosine - sr * sin);
        }
        value as f32
    }
}
