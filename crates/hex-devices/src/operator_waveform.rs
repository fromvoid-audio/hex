//! Original bounded periodic wave tables for native Operator.
//!
//! Tables are initialized once on the control side. Callback sampling only
//! borrows immutable data through a prepared context.

use std::sync::OnceLock;

const TABLE_BITS: u32 = 11;
const TABLE_SIZE: usize = 1 << TABLE_BITS;
const TABLE_MASK: u64 = TABLE_SIZE as u64 - 1;
const TABLE_LEN: usize = TABLE_SIZE + 1;
const HARMONIC_CAPS: [u16; 10] = [1, 2, 4, 8, 16, 32, 64, 128, 256, 512];
const WAVEFORM_COUNT: usize = 3;
const BANK_SAMPLES: usize = WAVEFORM_COUNT * HARMONIC_CAPS.len() * TABLE_LEN;
const NYQUIST_PHASE: u128 = 1_u128 << 63;

const WAVEFORM_LABELS: [&str; 4] = ["sine", "saw", "square", "triangle"];

/// Waveforms available to each native Operator oscillator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum OperatorWaveform {
    #[default]
    Sine = 0,
    Saw = 1,
    Square = 2,
    Triangle = 3,
}

impl OperatorWaveform {
    pub const fn labels() -> &'static [&'static str] {
        &WAVEFORM_LABELS
    }

    pub const fn choice(self) -> u32 {
        self as u32
    }

    pub const fn from_choice(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Sine),
            1 => Some(Self::Saw),
            2 => Some(Self::Square),
            3 => Some(Self::Triangle),
            _ => None,
        }
    }

    const fn bank_index(self) -> Option<usize> {
        match self {
            Self::Sine => None,
            Self::Saw => Some(0),
            Self::Square => Some(1),
            Self::Triangle => Some(2),
        }
    }
}

/// The one process-wide immutable bank; `samples` owns 245,880 bytes.
#[derive(Debug)]
pub(crate) struct OperatorWaveformBank {
    samples: Box<[f32]>,
}

impl OperatorWaveformBank {
    pub(crate) fn sample_phase(&self, waveform: OperatorWaveform, cap: u8, phase: u64) -> f32 {
        let Some(shape) = waveform.bank_index() else {
            return 0.0;
        };
        if usize::from(cap) >= HARMONIC_CAPS.len() {
            return 0.0;
        }
        let table = (shape * HARMONIC_CAPS.len() + usize::from(cap)) * TABLE_LEN;
        let index = ((phase >> (64 - TABLE_BITS)) & TABLE_MASK) as usize;
        let fraction_bits = (phase >> (64 - TABLE_BITS - 24)) & 0x00ff_ffff;
        let fraction = fraction_bits as f32 / 16_777_216.0;
        let Some(first) = self.samples.get(table + index).copied() else {
            return 0.0;
        };
        let Some(second) = self.samples.get(table + index + 1).copied() else {
            return 0.0;
        };
        first + (second - first) * fraction
    }
}

static OPERATOR_WAVEFORM_BANK: OnceLock<Result<OperatorWaveformBank, ()>> = OnceLock::new();

/// Control-side only. The callback receives the returned static reference from
/// its prepared context and never calls this initializer.
pub(crate) fn prepared_bank() -> Result<&'static OperatorWaveformBank, ()> {
    match OPERATOR_WAVEFORM_BANK.get_or_init(build_bank) {
        Ok(bank) => Ok(bank),
        Err(()) => Err(()),
    }
}

/// Return the highest table cap whose highest harmonic remains strictly below
/// Nyquist for a quantized wrapping phase increment. A zero step is DC and may
/// use the richest table; a step at or above half a cycle has no valid cap.
pub(crate) fn harmonic_cap_index(phase_step: u64) -> Option<u8> {
    if phase_step == 0 {
        return Some((HARMONIC_CAPS.len() - 1) as u8);
    }
    HARMONIC_CAPS
        .iter()
        .rposition(|cap| u128::from(phase_step) * u128::from(*cap) < NYQUIST_PHASE)
        .map(|index| index as u8)
}

/// Constant-time validation used by the sample path. It verifies both that the
/// selected table is below Nyquist and that the next richer table is not.
pub(crate) fn is_canonical_harmonic_cap(phase_step: u64, index: u8) -> bool {
    let Some(cap) = HARMONIC_CAPS.get(usize::from(index)).copied() else {
        return false;
    };
    if phase_step == 0 {
        return usize::from(index) == HARMONIC_CAPS.len() - 1;
    }
    if u128::from(phase_step) * u128::from(cap) >= NYQUIST_PHASE {
        return false;
    }
    match HARMONIC_CAPS.get(usize::from(index) + 1) {
        Some(next) => u128::from(phase_step) * u128::from(*next) >= NYQUIST_PHASE,
        None => true,
    }
}

fn build_bank() -> Result<OperatorWaveformBank, ()> {
    let mut samples = Vec::new();
    samples.try_reserve_exact(BANK_SAMPLES).map_err(|_| ())?;
    for shape in [
        OperatorWaveform::Saw,
        OperatorWaveform::Square,
        OperatorWaveform::Triangle,
    ] {
        for cap in HARMONIC_CAPS {
            for index in 0..TABLE_SIZE {
                let phase = std::f64::consts::TAU * index as f64 / TABLE_SIZE as f64;
                let value = match shape {
                    OperatorWaveform::Saw => saw(phase, cap),
                    OperatorWaveform::Square => square(phase, cap),
                    OperatorWaveform::Triangle => triangle(phase, cap),
                    OperatorWaveform::Sine => 0.0,
                };
                samples.push(value as f32);
            }
            let start = samples.len() - TABLE_SIZE;
            samples.push(samples[start]);
        }
    }
    let samples = samples.into_boxed_slice();
    if samples.len() != BANK_SAMPLES {
        return Err(());
    }
    Ok(OperatorWaveformBank { samples })
}

fn saw(phase: f64, cap: u16) -> f64 {
    let sum: f64 = (1..=u32::from(cap))
        .map(|harmonic| {
            let sign = if harmonic % 2 == 1 { 1.0 } else { -1.0 };
            sign * (phase * f64::from(harmonic)).sin() / f64::from(harmonic)
        })
        .sum();
    2.0 * sum / std::f64::consts::PI
}

fn square(phase: f64, cap: u16) -> f64 {
    let sum: f64 = (1..=u32::from(cap))
        .filter(|harmonic| harmonic % 2 == 1)
        .map(|harmonic| (phase * f64::from(harmonic)).sin() / f64::from(harmonic))
        .sum();
    4.0 * sum / std::f64::consts::PI
}

fn triangle(phase: f64, cap: u16) -> f64 {
    let sum: f64 = (1..=u32::from(cap))
        .filter(|harmonic| harmonic % 2 == 1)
        .map(|harmonic| {
            let sign = if (harmonic / 2) % 2 == 0 { 1.0 } else { -1.0 };
            sign * (phase * f64::from(harmonic)).sin() / f64::from(harmonic).powi(2)
        })
        .sum();
    8.0 * sum / std::f64::consts::PI.powi(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bank_has_expected_fixed_size_and_periodic_guard_samples() {
        let bank = build_bank().expect("bounded bank");
        assert_eq!(bank.samples.len(), BANK_SAMPLES);
        assert_eq!(BANK_SAMPLES * size_of::<f32>(), 245_880);
        println!(
            "waveform bank data={}B owner-header={}B static-cell={}B",
            BANK_SAMPLES * size_of::<f32>(),
            size_of::<OperatorWaveformBank>(),
            size_of::<OnceLock<Result<OperatorWaveformBank, ()>>>()
        );
        for waveform in [
            OperatorWaveform::Saw,
            OperatorWaveform::Square,
            OperatorWaveform::Triangle,
        ] {
            for cap in 0..HARMONIC_CAPS.len() as u8 {
                let start = (waveform.bank_index().expect("non-sine") * HARMONIC_CAPS.len()
                    + usize::from(cap))
                    * TABLE_LEN;
                assert_eq!(bank.samples[start], bank.samples[start + TABLE_SIZE]);
            }
        }
    }

    #[test]
    fn phase_sampling_uses_only_fraction_bits_after_the_table_index() {
        let bank = build_bank().expect("bounded bank");
        let waveform = OperatorWaveform::Saw;
        let cap = 3;
        let start = (waveform.bank_index().expect("non-sine") * HARMONIC_CAPS.len()
            + usize::from(cap))
            * TABLE_LEN;
        let phase_per_index = 1_u64 << (64 - TABLE_BITS);
        for index in [0, 1, 17, 512, 1023, 2047] {
            assert_eq!(
                bank.sample_phase(waveform, cap, index as u64 * phase_per_index),
                bank.samples[start + index],
                "table index {index}"
            );
            let halfway = index as u64 * phase_per_index + phase_per_index / 2;
            let expected = (bank.samples[start + index] + bank.samples[start + index + 1]) / 2.0;
            assert_eq!(bank.sample_phase(waveform, cap, halfway), expected);
        }
    }

    #[test]
    fn phase_step_cap_is_strict_and_uses_integer_bounds() {
        assert_eq!(harmonic_cap_index(0), Some(9));
        assert_eq!(harmonic_cap_index(1_u64 << 62), Some(0));
        assert_eq!(harmonic_cap_index((1_u64 << 63) - 1), Some(0));
        assert_eq!(harmonic_cap_index(1_u64 << 63), None);
        assert_eq!(harmonic_cap_index((1_u64 << 63) + 1), None);
        assert_eq!(harmonic_cap_index((1_u64 << 62) - 1), Some(1));
        for step in [0, 1, (1_u64 << 62) - 1, 1_u64 << 62, (1_u64 << 63) - 1] {
            let selected = harmonic_cap_index(step);
            for index in 0..HARMONIC_CAPS.len() as u8 {
                assert_eq!(
                    is_canonical_harmonic_cap(step, index),
                    selected == Some(index),
                    "phase step {step}, cap index {index}"
                );
            }
        }
    }

    #[test]
    fn capped_tables_match_independent_fourier_spectra() {
        let bank = build_bank().expect("bounded bank");
        let cap_index = 3;
        let cap = f64::from(HARMONIC_CAPS[cap_index]);
        let shapes = [
            OperatorWaveform::Saw,
            OperatorWaveform::Square,
            OperatorWaveform::Triangle,
        ];
        for shape in shapes {
            let offset = (shape.bank_index().expect("non-sine") * HARMONIC_CAPS.len() + cap_index)
                * TABLE_LEN;
            let samples = &bank.samples[offset..offset + TABLE_SIZE];
            let mean =
                samples.iter().map(|sample| f64::from(*sample)).sum::<f64>() / TABLE_SIZE as f64;
            assert!(mean.abs() < 2e-7, "{shape:?} dc={mean}");
            for harmonic in 1..=12_u32 {
                let mut sine = 0.0;
                let mut cosine = 0.0;
                for (index, sample) in samples.iter().enumerate() {
                    let phase = std::f64::consts::TAU * f64::from(harmonic) * index as f64
                        / TABLE_SIZE as f64;
                    sine += f64::from(*sample) * phase.sin();
                    cosine += f64::from(*sample) * phase.cos();
                }
                sine *= 2.0 / TABLE_SIZE as f64;
                cosine *= 2.0 / TABLE_SIZE as f64;
                let expected = match shape {
                    OperatorWaveform::Saw if f64::from(harmonic) <= cap => {
                        (if harmonic % 2 == 1 { 2.0 } else { -2.0 })
                            / (std::f64::consts::PI * f64::from(harmonic))
                    }
                    OperatorWaveform::Square if harmonic % 2 == 1 && f64::from(harmonic) <= cap => {
                        4.0 / (std::f64::consts::PI * f64::from(harmonic))
                    }
                    OperatorWaveform::Triangle
                        if harmonic % 2 == 1 && f64::from(harmonic) <= cap =>
                    {
                        let sign = if (harmonic / 2) % 2 == 0 { 1.0 } else { -1.0 };
                        sign * 8.0 / (std::f64::consts::PI.powi(2) * f64::from(harmonic).powi(2))
                    }
                    _ => 0.0,
                };
                assert!(
                    (sine - expected).abs() < 3e-7,
                    "{shape:?} h{harmonic}: {sine} != {expected}"
                );
                assert!(cosine.abs() < 3e-7, "{shape:?} h{harmonic} cosine={cosine}");
            }
        }
    }
}
