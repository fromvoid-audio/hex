//! Original bounded additive evaluation and immutable process-lifetime phase basis.
use super::*;
use crate::operator_waveform::BUILTIN_CHOICE_COUNT;

const PHASE_BITS: u32 = 17;
pub(super) struct HarmonicPhaseTable {
    sine: Vec<f64>,
}
impl HarmonicPhaseTable {
    fn lookup(&self, phase: u64, interpolate: bool) -> f64 {
        let index = (phase >> (64 - PHASE_BITS)) as usize;
        if !interpolate {
            return self.sine[index];
        }
        let fraction = (phase & ((1_u64 << (64 - PHASE_BITS)) - 1)) as f64
            / (1_u64 << (64 - PHASE_BITS)) as f64;
        self.sine[index] + (self.sine[index + 1] - self.sine[index]) * fraction
    }
    fn sample(&self, phase: u64, interpolate: bool) -> (f64, f64) {
        (
            self.lookup(phase, interpolate),
            self.lookup(phase.wrapping_add(1_u64 << 62), interpolate),
        )
    }
}
fn build_phase_table(
    reserve: impl FnOnce(&mut Vec<f64>, usize) -> Result<(), DeviceContractError>,
) -> Result<HarmonicPhaseTable, DeviceContractError> {
    let count = 1_usize << PHASE_BITS;
    let mut sine = Vec::new();
    reserve(&mut sine, count + 1)?;
    for i in 0..=count {
        sine.push((i as f64 * std::f64::consts::TAU / count as f64).sin());
    }
    Ok(HarmonicPhaseTable { sine })
}
pub(super) fn harmonic_phase_table() -> Result<&'static HarmonicPhaseTable, DeviceContractError> {
    static TABLE: std::sync::OnceLock<Result<HarmonicPhaseTable, DeviceContractError>> =
        std::sync::OnceLock::new();
    TABLE
        .get_or_init(|| {
            build_phase_table(|sine, count| {
                sine.try_reserve_exact(count)
                    .map_err(|_| DeviceContractError::RuntimeAllocation)
            })
        })
        .as_ref()
        .map_err(|error| *error)
}

#[cfg(test)]
#[test]
fn phase_basis_reservation_failure_is_returned_and_cached() {
    let table = std::sync::OnceLock::new();
    let failed = table.get_or_init(|| {
        build_phase_table(|sine, _| {
            sine.try_reserve_exact(usize::MAX)
                .map_err(|_| DeviceContractError::RuntimeAllocation)
        })
    });
    assert!(matches!(
        failed,
        Err(DeviceContractError::RuntimeAllocation)
    ));
    let cached = table.get_or_init(|| build_phase_table(|_, _| panic!("must retain failure")));
    assert!(matches!(
        cached,
        Err(DeviceContractError::RuntimeAllocation)
    ));
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::operator_state) struct Spectrum {
    pub(super) coefficients: [[f64; 64]; 4],
    pub(super) builtin: [[f64; BUILTIN_CHOICE_COUNT]; 4],
    pub(super) builtin_active: [u32; 4],
    // Conservative across transitions: false guarantees every custom coefficient is zero.
    pub(super) custom_active: [bool; 4],
}
impl Spectrum {
    pub(super) fn prepare(settings: &OperatorSettings) -> Self {
        let mut out = Self {
            coefficients: [[0.0; 64]; 4],
            builtin: [[0.0; BUILTIN_CHOICE_COUNT]; 4],
            builtin_active: [0; 4],
            custom_active: [false; 4],
        };
        for operator in 0..4 {
            if !settings.harmonics.custom[operator] {
                let choice = settings.waveforms[operator].choice();
                out.builtin[operator][choice as usize] = 1.0;
                out.builtin_active[operator] = 1 << choice;
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
            out.custom_active[operator] =
                out.coefficients[operator].iter().any(|value| *value != 0.0);
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
            builtin_active: std::array::from_fn(|i| {
                source.builtin_active[i] | target.builtin_active[i]
            }),
            custom_active: std::array::from_fn(|i| {
                source.custom_active[i] || target.custom_active[i]
            }),
        }
    }
    #[cfg(test)]
    #[inline(always)]
    #[allow(clippy::too_many_arguments)] // Explicit absolute age keeps white noise partition-independent.
    pub(super) fn sample(
        &self,
        operator: usize,
        phase: u64,
        step: u64,
        age: u64,
        base_step: u64,
        sine: &SineTable,
        bank: &OperatorWaveformBank,
        phase_table: &HarmonicPhaseTable,
    ) -> f32 {
        self.sample_policy(
            operator,
            phase,
            step,
            age,
            base_step,
            sine,
            bank,
            phase_table,
            true,
        )
    }
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn sample_policy(
        &self,
        operator: usize,
        phase: u64,
        step: u64,
        age: u64,
        base_step: u64,
        sine: &SineTable,
        bank: &OperatorWaveformBank,
        phase_table: &HarmonicPhaseTable,
        interpolate: bool,
    ) -> f32 {
        if self.builtin_active[operator] == 1
            && self.builtin[operator][0] == 1.0
            && !self.custom_active[operator]
        {
            return (0.0_f64 + f64::from(sine.sample_phase_policy(phase, interpolate))) as f32;
        }
        let mut value = 0.0;
        // Keep the original full builtin bank, including its 512-partial caps.
        let mut active = self.builtin_active[operator];
        while active != 0 {
            let shape = active.trailing_zeros() as usize;
            active &= active - 1;
            let weight = self.builtin[operator][shape];
            if weight == 0.0 {
                continue;
            }
            let sample = if shape == 0 {
                sine.sample_phase_policy(phase, interpolate)
            } else {
                bank.sample_frame_policy(
                    OperatorWaveform::from_choice(shape as u32).unwrap_or(OperatorWaveform::Sine),
                    harmonic_cap_index(step).unwrap_or(0),
                    phase,
                    age,
                    crate::operator_waveform::noise_stream(base_step, operator),
                    interpolate,
                )
            };
            value += f64::from(sample) * weight;
        }
        if !self.custom_active[operator] {
            return value as f32;
        }
        let cap = harmonic_cap(step);
        // Clenshaw's sine-series recurrence: two immutable basis lookups, no per-partial trig.
        if self.coefficients[operator][..cap].iter().all(|v| *v == 0.0) {
            return value as f32;
        }
        let (sin, cosine) = phase_table.sample(phase, interpolate);
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
        let coefficients = &self.coefficients[operator];
        let (b1, b2) = if cap == 64 {
            let mut b1: [f64; 8] = std::array::from_fn(|r| coefficients[56 + r]);
            let mut b2 = [0.0; 8];
            let twice_cos8 = 2.0 * cos8;
            for row in coefficients[..56].chunks_exact(8).rev() {
                let mut next = [0.0; 8];
                for r in 0..8 {
                    next[r] = twice_cos8.mul_add(b1[r], row[r] - b2[r]);
                }
                b2 = b1;
                b1 = next;
            }
            (b1, b2)
        } else {
            let groups = cap.div_ceil(8);
            let mut b1: [f64; 8] = std::array::from_fn(|r| {
                let index = (groups - 1) * 8 + r;
                if index < cap {
                    coefficients[index]
                } else {
                    0.0
                }
            });
            let mut b2 = [0.0; 8];
            for group in (0..groups - 1).rev() {
                let next = std::array::from_fn(|r| {
                    coefficients[group * 8 + r] + 2.0 * cos8 * b1[r] - b2[r]
                });
                b2 = b1;
                b1 = next;
            }
            (b1, b2)
        };
        // sin(r*x)*b1 + sin((8-r)*x)*b2 is the same Clenshaw
        // reconstruction without an eight-step serial angle recurrence.
        let sin2 = 2.0 * sin * cosine;
        let sin3 = sin * cos2 + cosine * sin2;
        let sin5 = sin4 * cosine + cos4 * sin;
        let sin6 = sin2 * cos4 + cos2 * sin4;
        let sin7 = sin8 * cosine - cos8 * sin;
        let basis = [0.0, sin, sin2, sin3, sin4, sin5, sin6, sin7, sin8];
        for r in 0..8 {
            value += basis[r + 1] * b1[r] + basis[7 - r] * b2[r];
        }
        value as f32
    }
}

/// Strict Nyquist cap. The full-bank fast path avoids division for low notes;
/// the remaining quotient fits u64 and is at most 63.
pub(super) fn harmonic_cap(step: u64) -> usize {
    const BELOW_NYQUIST: u64 = (1_u64 << 63) - 1;
    if step <= BELOW_NYQUIST / 64 {
        64
    } else {
        (BELOW_NYQUIST / step) as usize
    }
}
