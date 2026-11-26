//! Private control-side math proof. No native state/provider/UI consumes it.
//! Semitone frequency ratio follows the existing MIDI tuning law, not Fine.

const PHASE_SCALE: f64 = 18_446_744_073_709_551_616.0;
const NYQUIST_STEP: u64 = 1_u64 << 63;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    InvalidSemitones,
    UnrepresentableMultiplier,
    InvalidFrequency,
    InvalidRate,
    UnrepresentableFrequency,
    Nyquist,
    UnrepresentablePhase,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Multiplier(f64);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Frequency {
    pub(super) hz: f64,
    pub(super) phase_step: u64,
}

impl Multiplier {
    /// No invented musical domain: finite semitones whose ratio is representable.
    /// This preparation owns powf; no sampling/clock/voice behavior is added.
    pub(super) fn prepare(semitones: f64) -> Result<Self, Error> {
        if !semitones.is_finite() {
            return Err(Error::InvalidSemitones);
        }
        let ratio = 2.0_f64.powf(semitones / 12.0);
        if !ratio.is_finite() || ratio <= 0.0 {
            return Err(Error::UnrepresentableMultiplier);
        }
        Ok(Self(ratio))
    }

    pub(super) const fn ratio(self) -> f64 {
        self.0
    }

    /// Validate the final shifted frequency, not an inferred Fine/fixed policy.
    /// Like existing tuning, quantize positive phase with nearest/half-up law.
    pub(super) fn frequency(self, base_hz: f64, rate: u32) -> Result<Frequency, Error> {
        if !(8_000..=192_000).contains(&rate) {
            return Err(Error::InvalidRate);
        }
        if !base_hz.is_finite() || base_hz <= 0.0 {
            return Err(Error::InvalidFrequency);
        }
        let hz = if self.0 == 1.0 {
            base_hz
        } else {
            base_hz * self.0
        };
        if !hz.is_finite() || hz <= 0.0 {
            return Err(Error::UnrepresentableFrequency);
        }
        if hz >= f64::from(rate) * 0.5 {
            return Err(Error::Nyquist);
        }
        let rounded = (hz / f64::from(rate) * PHASE_SCALE).round();
        if !rounded.is_finite() || rounded < 1.0 {
            return Err(Error::UnrepresentablePhase);
        }
        // Check before the integer cast; never rely on saturating conversion.
        if rounded >= NYQUIST_STEP as f64 {
            return Err(Error::Nyquist);
        }
        Ok(Frequency {
            hz,
            phase_step: rounded as u64,
        })
    }

    /// Atomic control-prepared value replacement, not musical history retarget.
    pub(super) fn prepare_into(
        self,
        base_hz: f64,
        rate: u32,
        destination: &mut Frequency,
    ) -> Result<(), Error> {
        let candidate = self.frequency(base_hz, rate)?;
        *destination = candidate;
        Ok(())
    }
}
