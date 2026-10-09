use crate::Invalid;

pub const MAX_TEMPO_POINTS: usize = 1_024;
pub const MAX_METER_POINTS: usize = 1_024;
const FRACTION_BITS: u32 = 32;
const ONE_RAW: u64 = 1_u64 << FRACTION_BITS;

/// Nonnegative quarter-note beats in unsigned Q32.32 fixed point.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BeatTime(u64);

impl BeatTime {
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(ONE_RAW);

    /// Construct directly from its canonical Q32.32 representation.
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Convert an integral number of quarter notes without floating-point rounding.
    pub fn from_quarters(quarters: u64) -> Result<Self, Invalid> {
        quarters
            .checked_mul(ONE_RAW)
            .map(Self)
            .ok_or(Invalid("beat position exceeds the Q32.32 range"))
    }

    /// Convert a nonnegative rational number of quarter notes, rounding to the
    /// nearest Q32.32 unit with exact half ties toward the later beat.
    pub fn from_ratio(numerator: u64, denominator: u64) -> Result<Self, Invalid> {
        if denominator == 0 {
            return Err(Invalid("beat ratio denominator must be positive"));
        }
        let scaled = u128::from(numerator)
            .checked_mul(u128::from(ONE_RAW))
            .ok_or(Invalid("beat ratio exceeds the Q32.32 range"))?;
        let denominator = u128::from(denominator);
        let quotient = scaled / denominator;
        let remainder = scaled % denominator;
        let rounded = if remainder >= denominator - remainder {
            quotient
                .checked_add(1)
                .ok_or(Invalid("beat ratio exceeds the Q32.32 range"))?
        } else {
            quotient
        };
        u64::try_from(rounded)
            .map(Self)
            .map_err(|_| Invalid("beat ratio exceeds the Q32.32 range"))
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Snap to the nearest positive raw-Q32.32 grid step; ties move later.
    pub fn snap_to(self, grid: Self) -> Result<Self, Invalid> {
        let step = grid.0;
        if step == 0 {
            return Err(Invalid("snap grid must be positive"));
        }
        let quotient = self.0 / step;
        let remainder = self.0 % step;
        let rounded = if remainder >= step - remainder {
            quotient
                .checked_add(1)
                .ok_or(Invalid("snapped beat exceeds the Q32.32 range"))?
        } else {
            quotient
        };
        rounded
            .checked_mul(step)
            .map(Self)
            .ok_or(Invalid("snapped beat exceeds the Q32.32 range"))
    }
}

/// Integer micro-BPM tempo in the original Hex range 1..=1,000 BPM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tempo(u32);

impl Tempo {
    pub const DEFAULT: Self = Self(120_000_000);

    pub fn new(micro_bpm: u32) -> Result<Self, Invalid> {
        if !(1_000_000..=1_000_000_000).contains(&micro_bpm) {
            return Err(Invalid("tempo must be within 1..1000 BPM"));
        }
        Ok(Self(micro_bpm))
    }

    pub const fn micro_bpm(self) -> u32 {
        self.0
    }
}

/// Time signature. Numerators up to 99 and power-of-two denominators are Hex's
/// supported display range; this is not a claim about Live's internal limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Meter {
    numerator: u8,
    denominator: u8,
}

impl Meter {
    pub const COMMON_TIME: Self = Self {
        numerator: 4,
        denominator: 4,
    };

    pub fn new(numerator: u8, denominator: u8) -> Result<Self, Invalid> {
        if !(1..=99).contains(&numerator) {
            return Err(Invalid("meter numerator must be 1..99"));
        }
        if !matches!(denominator, 1 | 2 | 4 | 8 | 16 | 32 | 64) {
            return Err(Invalid("meter denominator must be a power of two up to 64"));
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }

    pub const fn numerator(self) -> u8 {
        self.numerator
    }

    pub const fn denominator(self) -> u8 {
        self.denominator
    }

    pub(crate) fn beat_length_raw(self) -> u64 {
        (4_u64 * ONE_RAW) / u64::from(self.denominator)
    }

    pub(crate) fn bar_length_raw(self) -> u64 {
        self.beat_length_raw() * u64::from(self.numerator)
    }
}
