//! Private, unregistered original linear signed-envelope law (Pulse #2327).
//! Levels are normalized modulation values, not semitones, cents or amplitudes.
//! No codec, provider, voice runtime or UI consumes this precursor.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    InvalidLevel,
    InvalidDuration,
    InvalidRate,
    FrameOverflow,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SignedEnvelope {
    levels: [f64; 4],
    seconds: [f64; 3],
}

impl SignedEnvelope {
    /// Initial, peak, sustain, end; attack, decay, release in seconds.
    pub(super) fn new(levels: [f64; 4], seconds: [f64; 3]) -> Result<Self, Error> {
        if levels
            .iter()
            .any(|level| !level.is_finite() || !(-1.0..=1.0).contains(level))
        {
            return Err(Error::InvalidLevel);
        }
        if seconds
            .iter()
            .zip([20.0, 60.0, 60.0])
            .any(|(seconds, max)| !seconds.is_finite() || !(0.0..=max).contains(seconds))
        {
            return Err(Error::InvalidDuration);
        }
        Ok(Self { levels, seconds })
    }

    pub(super) fn prepare(self, sample_rate: u32) -> Result<PreparedSignedEnvelope, Error> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(Error::InvalidRate);
        }
        let attack = checked_frames(self.seconds[0], sample_rate)?;
        let decay = checked_frames(self.seconds[1], sample_rate)?;
        let release = checked_frames(self.seconds[2], sample_rate)?;
        let decay_end = attack.checked_add(decay).ok_or(Error::FrameOverflow)?;
        Ok(PreparedSignedEnvelope {
            levels: self.levels,
            attack,
            decay_end,
            release,
        })
    }
}

/// Reject overflow before casting: u64::MAX rounds UP to 2^64 as f64.
pub(super) fn checked_frames(seconds: f64, sample_rate: u32) -> Result<u64, Error> {
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(Error::FrameOverflow);
    }
    let frames = (seconds * f64::from(sample_rate)).round();
    if !frames.is_finite() || frames < 0.0 || frames >= u64::MAX as f64 {
        return Err(Error::FrameOverflow);
    }
    Ok(frames as u64)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PreparedSignedEnvelope {
    levels: [f64; 4],
    attack: u64,
    decay_end: u64,
    release: u64,
}

impl PreparedSignedEnvelope {
    /// Optional NoteOff age is independent of the age being queried. None and
    /// Some(u64::MAX) remain distinct; no held-gate sentinel or replay is used.
    ///
    /// Instantaneous precedence: zero attack reaches peak immediately; zero
    /// decay then reaches sustain immediately. NoteOff samples that held law at
    /// its exact boundary. Zero release reaches end immediately, overriding it.
    pub(super) fn level(self, age: u64, gate: Option<u64>) -> f64 {
        match gate {
            Some(gate) if age >= gate => {
                linear(self.held(gate), self.levels[3], age - gate, self.release)
            }
            _ => self.held(age),
        }
    }

    /// A future host must refuse an unrepresentable lifetime before mutation.
    /// The sampler itself subtracts age/gate and never wraps a gate+release sum.
    pub(super) fn release_end(self, gate: u64) -> Result<u64, Error> {
        gate.checked_add(self.release).ok_or(Error::FrameOverflow)
    }

    fn held(self, age: u64) -> f64 {
        if age < self.attack {
            linear(self.levels[0], self.levels[1], age, self.attack)
        } else if age < self.decay_end {
            linear(
                self.levels[1],
                self.levels[2],
                age - self.attack,
                self.decay_end - self.attack,
            )
        } else {
            self.levels[2]
        }
    }
}

fn linear(start: f64, end: f64, elapsed: u64, duration: u64) -> f64 {
    if duration == 0 || elapsed >= duration {
        end
    } else if elapsed == 0 {
        start
    } else {
        // Bounded finite endpoints and ratio; clamp only roundoff at endpoints.
        (start + (end - start) * (elapsed as f64 / duration as f64))
            .clamp(start.min(end), start.max(end))
    }
}
