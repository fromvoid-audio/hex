//! Unregistered, private proof of an original Hex output-volume law.
//! Only the test harness compiles this file. No native provider uses it.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    InvalidDb,
    InvalidRate,
    RateMismatch,
    RetrogradeClock,
    ClockOverflow,
    NonfiniteInput,
    UnrepresentableOutput,
}

/// Control-prepared finite gain and five-millisecond ramp duration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PreparedVolume {
    gain: f64,
    sample_rate: u32,
    ramp_frames: u32,
}

impl PreparedVolume {
    pub(crate) fn prepare(db: f32, sample_rate: u32) -> Result<Self, Error> {
        if !db.is_finite() || !(-60.0..=6.0).contains(&db) {
            return Err(Error::InvalidDb);
        }
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(Error::InvalidRate);
        }
        // Integer rounding to nearest frame, half upward. Preparation only.
        let ramp_frames = sample_rate
            .checked_mul(5)
            .and_then(|value| value.checked_add(500))
            .ok_or(Error::InvalidRate)?
            / 1000;
        let gain = if db == 0.0 {
            1.0
        } else {
            10.0_f64.powf(f64::from(db) / 20.0)
        };
        Ok(Self {
            gain,
            sample_rate,
            ramp_frames,
        })
    }

    pub(crate) const fn gain(self) -> f64 {
        self.gain
    }
    pub(crate) const fn ramp_frames(self) -> u32 {
        self.ramp_frames
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Ramp {
    from: f64,
    to: f64,
    start: u64,
    end: u64,
}

/// Fixed scalar checkpoint, not an engine owner, state codec or application ACK.
/// The future caller must supply the actual committed performance boundary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Volume {
    target: PreparedVolume,
    ramp: Option<Ramp>,
    boundary: u64,
}

impl Volume {
    pub(crate) const fn new(target: PreparedVolume, boundary: u64) -> Self {
        Self {
            target,
            ramp: None,
            boundary,
        }
    }

    pub(crate) fn gain_at(self, frame: u64) -> Result<f64, Error> {
        if frame < self.boundary {
            return Err(Error::RetrogradeClock);
        }
        match self.ramp {
            Some(ramp) if frame < ramp.end => {
                let fraction = (frame - ramp.start) as f64 / (ramp.end - ramp.start) as f64;
                Ok(ramp.from + (ramp.to - ramp.from) * fraction)
            }
            _ => Ok(self.target.gain),
        }
    }

    /// Validate every refusal before replacing the checkpoint. Repeating the
    /// same endpoint does not restart an already running transition.
    pub(crate) fn retarget(&mut self, target: PreparedVolume, boundary: u64) -> Result<(), Error> {
        if target.sample_rate != self.target.sample_rate {
            return Err(Error::RateMismatch);
        }
        let from = self.gain_at(boundary)?;
        if target == self.target {
            return Ok(());
        }
        let ramp = if from == target.gain {
            None
        } else {
            let end = boundary
                .checked_add(u64::from(target.ramp_frames))
                .ok_or(Error::ClockOverflow)?;
            Some(Ramp {
                from,
                to: target.gain,
                start: boundary,
                end,
            })
        };
        *self = Self {
            target,
            ramp,
            boundary,
        };
        Ok(())
    }

    /// Copy captures this exact ramp; subsequent edits of the current checkpoint
    /// cannot change the detached copy. No provider/context ownership is implied.
    pub(crate) const fn snapshot(self) -> Self {
        self
    }

    /// Compose after the caller's complete carrier sum. Never alter its input,
    /// oscillator/FM/feedback history, gate, envelope or phase.
    pub(crate) fn apply(self, stereo: [f32; 2], frame: u64) -> Result<[f32; 2], Error> {
        if stereo.iter().any(|sample| !sample.is_finite()) {
            return Err(Error::NonfiniteInput);
        }
        let gain = self.gain_at(frame)?;
        if gain == 1.0 {
            return Ok(stereo);
        }
        let output = stereo.map(|sample| (f64::from(sample) * gain) as f32);
        if output.iter().any(|sample| !sample.is_finite()) {
            return Err(Error::UnrepresentableOutput);
        }
        Ok(output)
    }
}
