//! Original bounded Operator waveforms, prepared entirely on the control side.
//! Numeric compatibility with the reference instrument remains unmeasured.
use std::sync::OnceLock;

const TABLE_BITS: u32 = 11;
const TABLE_SIZE: usize = 1 << TABLE_BITS;
const TABLE_MASK: u64 = TABLE_SIZE as u64 - 1;
const TABLE_LEN: usize = TABLE_SIZE + 1;
const HARMONIC_CAPS: [u16; 10] = [1, 2, 4, 8, 16, 32, 64, 128, 256, 512];
const PERIODIC_SHAPES: usize = 20;
const BANK_SAMPLES: usize = PERIODIC_SHAPES * HARMONIC_CAPS.len() * TABLE_LEN;
const NYQUIST_PHASE: u128 = 1_u128 << 63;
pub(crate) const BUILTIN_CHOICE_COUNT: usize = 23;
pub(crate) const WAVEFORM_LABELS: [&str; BUILTIN_CHOICE_COUNT] = [
    "sine",
    "saw d",
    "square d",
    "triangle",
    "user",
    "sine 4bit",
    "sine 8bit",
    "saw 3",
    "saw 4",
    "saw 6",
    "saw 8",
    "saw 16",
    "saw 32",
    "saw 64",
    "square 3",
    "square 4",
    "square 6",
    "square 8",
    "square 16",
    "square 32",
    "square 64",
    "noise looped",
    "noise white",
];
const PERIODIC: [OperatorWaveform; PERIODIC_SHAPES] = [
    OperatorWaveform::Saw,
    OperatorWaveform::Square,
    OperatorWaveform::Triangle,
    OperatorWaveform::Sine4Bit,
    OperatorWaveform::Sine8Bit,
    OperatorWaveform::Saw3,
    OperatorWaveform::Saw4,
    OperatorWaveform::Saw6,
    OperatorWaveform::Saw8,
    OperatorWaveform::Saw16,
    OperatorWaveform::Saw32,
    OperatorWaveform::Saw64,
    OperatorWaveform::Square3,
    OperatorWaveform::Square4,
    OperatorWaveform::Square6,
    OperatorWaveform::Square8,
    OperatorWaveform::Square16,
    OperatorWaveform::Square32,
    OperatorWaveform::Square64,
    OperatorWaveform::NoiseLooped,
];

/// Stable saved choices. User remains choice 4 in native metadata, rather than
/// a builtin waveform. The first four variants retain their original PCM laws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum OperatorWaveform {
    #[default]
    Sine = 0,
    Saw = 1,
    Square = 2,
    Triangle = 3,
    Sine4Bit = 5,
    Sine8Bit = 6,
    Saw3 = 7,
    Saw4 = 8,
    Saw6 = 9,
    Saw8 = 10,
    Saw16 = 11,
    Saw32 = 12,
    Saw64 = 13,
    Square3 = 14,
    Square4 = 15,
    Square6 = 16,
    Square8 = 17,
    Square16 = 18,
    Square32 = 19,
    Square64 = 20,
    NoiseLooped = 21,
    NoiseWhite = 22,
}
impl OperatorWaveform {
    /// Native choice labels, including the separate User authoring choice.
    pub const fn labels() -> &'static [&'static str] {
        &WAVEFORM_LABELS
    }
    pub const fn choice(self) -> u32 {
        self as u32
    }
    pub const fn from_choice(value: u32) -> Option<Self> {
        Some(match value {
            0 => Self::Sine,
            1 => Self::Saw,
            2 => Self::Square,
            3 => Self::Triangle,
            5 => Self::Sine4Bit,
            6 => Self::Sine8Bit,
            7 => Self::Saw3,
            8 => Self::Saw4,
            9 => Self::Saw6,
            10 => Self::Saw8,
            11 => Self::Saw16,
            12 => Self::Saw32,
            13 => Self::Saw64,
            14 => Self::Square3,
            15 => Self::Square4,
            16 => Self::Square6,
            17 => Self::Square8,
            18 => Self::Square16,
            19 => Self::Square32,
            20 => Self::Square64,
            21 => Self::NoiseLooped,
            22 => Self::NoiseWhite,
            _ => return None,
        })
    }
    const fn bank_index(self) -> Option<usize> {
        match self {
            Self::Sine | Self::NoiseWhite => None,
            Self::Saw => Some(0),
            Self::Square => Some(1),
            Self::Triangle => Some(2),
            _ => Some(self as usize - 2),
        }
    }
    pub(crate) const fn named_cap(self) -> Option<(bool, u16)> {
        Some(match self {
            Self::Saw3 => (false, 3),
            Self::Saw4 => (false, 4),
            Self::Saw6 => (false, 6),
            Self::Saw8 => (false, 8),
            Self::Saw16 => (false, 16),
            Self::Saw32 => (false, 32),
            Self::Saw64 => (false, 64),
            Self::Square3 => (true, 5),
            Self::Square4 => (true, 7),
            Self::Square6 => (true, 11),
            Self::Square8 => (true, 15),
            Self::Square16 => (true, 31),
            Self::Square32 => (true, 63),
            Self::Square64 => (true, 127),
            _ => return None,
        })
    }
}

/// Process-lifetime immutable payload: 1,639,200 table bytes and 5,120 authoring
/// magnitude bytes. Owner headers/cell storage are accounted separately in tests.
#[derive(Debug)]
pub(crate) struct OperatorWaveformBank {
    samples: Box<[f32]>,
    magnitudes: [[f32; 64]; PERIODIC_SHAPES],
}
impl OperatorWaveformBank {
    /// Callback-only lookup. White noise uses absolute note age and operator
    /// stream seed, independent of modulation phase and block partitioning.
    /// Equal pitch/velocity/onset is deterministic; no stochastic parity claim.
    pub(crate) fn sample_frame(
        &self,
        waveform: OperatorWaveform,
        cap: u8,
        phase: u64,
        age: u64,
        stream: u64,
    ) -> f32 {
        if waveform == OperatorWaveform::NoiseWhite {
            return noise(age, stream);
        }
        self.sample_phase(waveform, cap, phase)
    }
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
    pub(crate) fn authoring_amplitude(&self, waveform: OperatorWaveform, harmonic: usize) -> f32 {
        if waveform == OperatorWaveform::NoiseWhite {
            return 1.0;
        }
        waveform
            .bank_index()
            .and_then(|shape| self.magnitudes[shape].get(harmonic.wrapping_sub(1)))
            .copied()
            .unwrap_or(0.0)
    }
}
static OPERATOR_WAVEFORM_BANK: OnceLock<Result<OperatorWaveformBank, ()>> = OnceLock::new();
/// Control-side only. No initializer runs from an audio sampler.
pub(crate) fn prepared_bank() -> Result<&'static OperatorWaveformBank, ()> {
    OPERATOR_WAVEFORM_BANK
        .get_or_init(build_bank)
        .as_ref()
        .map_err(|_| ())
}

/// Bounded control-side display samples of the actual builtin law. Native UI
/// must request this through its engine view boundary, not import device DSP.
pub fn builtin_waveform_preview(choice: u32) -> Result<[f32; 256], crate::Invalid> {
    let waveform = OperatorWaveform::from_choice(choice)
        .ok_or(crate::Invalid("invalid builtin waveform choice"))?;
    let bank = prepared_bank().map_err(|_| crate::Invalid("waveform table allocation failed"))?;
    let sine = crate::shared_sine_table();
    Ok(std::array::from_fn(|index| {
        let phase = (index as u64) << 56;
        if waveform == OperatorWaveform::Sine {
            sine.sample_phase(phase)
        } else {
            bank.sample_frame(waveform, 9, phase, index as u64, 0)
        }
    }))
}

pub(crate) fn harmonic_cap_index(phase_step: u64) -> Option<u8> {
    if phase_step == 0 {
        return Some((HARMONIC_CAPS.len() - 1) as u8);
    }
    HARMONIC_CAPS
        .iter()
        .rposition(|cap| u128::from(phase_step) * u128::from(*cap) < NYQUIST_PHASE)
        .map(|index| index as u8)
}
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
/// Stable pitch/operator stream identity, never derived from modulated phase.
pub(crate) fn noise_stream(step: u64, operator: usize) -> u64 {
    step ^ 0x9e3779b97f4a7c15_u64.wrapping_mul(operator as u64 + 1)
}
fn noise(counter: u64, stream: u64) -> f32 {
    let mut word =
        counter.wrapping_add(0x9e3779b97f4a7c15_u64.wrapping_mul(stream.wrapping_add(1)));
    word = (word ^ (word >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    word = (word ^ (word >> 27)).wrapping_mul(0x94d049bb133111eb);
    word ^= word >> 31;
    ((word >> 40) as f32 / 16_777_215.0) * 2.0 - 1.0
}
struct Fourier {
    sine: [f64; 512],
    cosine: [f64; 512],
}
impl Fourier {
    fn for_waveform(waveform: OperatorWaveform) -> Self {
        let raw: [f64; TABLE_SIZE] = std::array::from_fn(|index| {
            if waveform == OperatorWaveform::NoiseLooped {
                return f64::from(noise(index as u64, 7));
            }
            let levels = if waveform == OperatorWaveform::Sine4Bit {
                16.0
            } else {
                256.0
            };
            let sine = (std::f64::consts::TAU * index as f64 / TABLE_SIZE as f64).sin();
            ((sine * 0.5 + 0.5) * (levels - 1.0)).round() / (levels - 1.0) * 2.0 - 1.0
        });
        let mut out = Self {
            sine: [0.0; 512],
            cosine: [0.0; 512],
        };
        for harmonic in 1..=512 {
            for (index, value) in raw.iter().enumerate() {
                let phase =
                    std::f64::consts::TAU * index as f64 * harmonic as f64 / TABLE_SIZE as f64;
                let (sine, cosine) = phase.sin_cos();
                out.sine[harmonic - 1] += value * sine * (2.0 / TABLE_SIZE as f64);
                out.cosine[harmonic - 1] += value * cosine * (2.0 / TABLE_SIZE as f64);
            }
        }
        out
    }
    fn sample(&self, phase: f64, cap: u16) -> f64 {
        self.sine[..usize::from(cap)]
            .iter()
            .zip(&self.cosine)
            .enumerate()
            .map(|(index, (sine, cosine))| {
                let (s, c) = (phase * (index + 1) as f64).sin_cos();
                sine * s + cosine * c
            })
            .sum()
    }
}
fn build_bank() -> Result<OperatorWaveformBank, ()> {
    let mut samples = Vec::new();
    samples.try_reserve_exact(BANK_SAMPLES).map_err(|_| ())?;
    let mut magnitudes = [[0.0; 64]; PERIODIC_SHAPES];
    for (shape_index, shape) in PERIODIC.into_iter().enumerate() {
        let fourier = matches!(
            shape,
            OperatorWaveform::Sine4Bit | OperatorWaveform::Sine8Bit | OperatorWaveform::NoiseLooped
        )
        .then(|| Fourier::for_waveform(shape));
        for cap in HARMONIC_CAPS {
            let start = samples.len();
            for index in 0..TABLE_SIZE {
                let phase = std::f64::consts::TAU * index as f64 / TABLE_SIZE as f64;
                let value = match shape {
                    OperatorWaveform::Saw => saw(phase, cap),
                    OperatorWaveform::Square => square(phase, cap),
                    OperatorWaveform::Triangle => triangle(phase, cap),
                    _ => {
                        if let Some(series) = fourier.as_ref() {
                            series.sample(phase, cap)
                        } else if let Some((is_square, named)) = shape.named_cap() {
                            if is_square {
                                square(phase, cap.min(named))
                            } else {
                                saw(phase, cap.min(named))
                            }
                        } else {
                            return Err(());
                        }
                    }
                };
                samples.push(value as f32);
            }
            // New quantized/noise families have a bounded periodic output at
            // every cap. Preserve legacy tables and their PCM exactly.
            if fourier.is_some() {
                let peak = samples[start..]
                    .iter()
                    .fold(1.0_f32, |peak, value| peak.max(value.abs()));
                for value in &mut samples[start..] {
                    *value /= peak;
                }
            }
            samples.push(samples[start]);
        }
        if let Some(series) = fourier {
            for (index, value) in magnitudes[shape_index].iter_mut().enumerate() {
                *value = series.sine[index].hypot(series.cosine[index]) as f32;
            }
            let peak = magnitudes[shape_index]
                .iter()
                .copied()
                .fold(1.0_f32, f32::max);
            for value in &mut magnitudes[shape_index] {
                *value /= peak;
            }
        }
    }
    if samples.len() != BANK_SAMPLES {
        return Err(());
    }
    Ok(OperatorWaveformBank {
        samples: samples.into_boxed_slice(),
        magnitudes,
    })
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
        assert_eq!(BANK_SAMPLES * size_of::<f32>(), 1_639_200);
        assert_eq!(size_of_val(&bank.magnitudes), 5_120);
        assert_eq!(
            size_of::<OperatorWaveformBank>(),
            size_of::<Box<[f32]>>() + 5_120
        );
        println!(
            "waveform bank data={}B owner-header={}B static-cell={}B",
            BANK_SAMPLES * size_of::<f32>(),
            size_of::<OperatorWaveformBank>(),
            size_of::<OnceLock<Result<OperatorWaveformBank, ()>>>()
        );
        for waveform in PERIODIC {
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

#[cfg(test)]
mod catalog_tests {
    use super::*;
    fn bin(samples: &[f32], harmonic: usize) -> (f64, f64) {
        let mut sine = 0.0;
        let mut cosine = 0.0;
        for (index, value) in samples.iter().enumerate() {
            let phase =
                std::f64::consts::TAU * harmonic as f64 * index as f64 / samples.len() as f64;
            sine += f64::from(*value) * phase.sin() * 2.0 / samples.len() as f64;
            cosine += f64::from(*value) * phase.cos() * 2.0 / samples.len() as f64;
        }
        (sine, cosine)
    }
    #[test]
    fn named_saw_and_square_presets_have_independent_expected_spectra() {
        let bank = prepared_bank().unwrap();
        for (start, square) in [(7, false), (14, true)] {
            for (offset, count) in [3, 4, 6, 8, 16, 32, 64].into_iter().enumerate() {
                let shape = OperatorWaveform::from_choice(start + offset as u32).unwrap();
                let highest = if square { 2 * count - 1 } else { count };
                for (cap_index, cap) in [(3, 8), (9, 512)] {
                    let table = (shape.bank_index().unwrap() * 10 + cap_index) * TABLE_LEN;
                    let samples = &bank.samples[table..table + TABLE_SIZE];
                    for harmonic in 1..=highest + 1 {
                        let (sine, cosine) = bin(samples, harmonic);
                        let expected =
                            if harmonic <= highest.min(cap) && (!square || harmonic % 2 == 1) {
                                if square {
                                    4.0 / (std::f64::consts::PI * harmonic as f64)
                                } else {
                                    (if harmonic % 2 == 1 { 2.0 } else { -2.0 })
                                        / (std::f64::consts::PI * harmonic as f64)
                                }
                            } else {
                                0.0
                            };
                        assert!(
                            (sine - expected).abs() < 3e-7,
                            "{shape:?} h{harmonic}: {sine} != {expected}"
                        );
                        assert!(cosine.abs() < 3e-7);
                    }
                }
            }
        }
    }
    #[test]
    fn quantized_sine_and_noise_are_real_bounded_distinct_outputs() {
        let sine = builtin_waveform_preview(0).unwrap();
        let four = builtin_waveform_preview(5).unwrap();
        let eight = builtin_waveform_preview(6).unwrap();
        let looped = builtin_waveform_preview(21).unwrap();
        let white = builtin_waveform_preview(22).unwrap();
        let distance = |a: &[f32; 256], b: &[f32; 256]| {
            a.iter()
                .zip(b)
                .map(|(a, b)| f64::from(a - b).powi(2))
                .sum::<f64>()
                / 256.0
        };
        assert!(distance(&four, &sine) > 1e-4);
        assert!(distance(&eight, &sine) > 1e-7);
        assert!(distance(&four, &eight) > 1e-4);
        for noise in [looped, white] {
            assert!(noise.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
            assert!(noise.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() > 1.0);
        }
        let bank = prepared_bank().unwrap();
        for cap in 0..10 {
            assert_eq!(
                bank.sample_frame(OperatorWaveform::NoiseLooped, cap, 0, 0, 0),
                bank.sample_frame(OperatorWaveform::NoiseLooped, cap, 0, 999999, 3)
            );
        }
        let white: Vec<_> = (0..65536)
            .map(|age| bank.sample_frame(OperatorWaveform::NoiseWhite, 9, 0, age, 0))
            .collect();
        let mean = white.iter().map(|v| f64::from(*v)).sum::<f64>() / white.len() as f64;
        let lag = white
            .windows(2)
            .map(|p| f64::from(p[0]) * f64::from(p[1]))
            .sum::<f64>()
            / (white.len() - 1) as f64;
        assert!(mean.abs() < 0.01);
        assert!(lag.abs() < 0.01);
        assert_ne!(
            bank.sample_frame(OperatorWaveform::NoiseWhite, 9, 0, 17, 0),
            bank.sample_frame(OperatorWaveform::NoiseWhite, 9, 0, 17, 1)
        );
        assert_eq!(
            bank.sample_frame(OperatorWaveform::NoiseWhite, 9, 0, 17, 0),
            bank.sample_frame(OperatorWaveform::NoiseWhite, 0, u64::MAX, 17, 0)
        );
        for invalid in [4, 23, u32::MAX] {
            assert!(builtin_waveform_preview(invalid).is_err());
        }
    }
}
