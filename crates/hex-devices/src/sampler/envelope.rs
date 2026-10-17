//! Original linear ADSR law in output-frame units, not measured Live curves.
use super::{Invalid, finite_range};

#[derive(Clone, Copy, Debug)]
pub struct Envelope {
    pub attack_seconds: f64,
    pub decay_seconds: f64,
    pub sustain: f64,
    pub release_seconds: f64,
}

impl Default for Envelope {
    fn default() -> Self {
        Self {
            attack_seconds: 0.0,
            decay_seconds: 0.0,
            sustain: 1.0,
            release_seconds: 0.0,
        }
    }
}

impl Envelope {
    pub(super) fn prepare(self, rate: u32) -> Result<PreparedEnvelope, Invalid> {
        if !finite_range(self.attack_seconds, 0.0, 60.0)
            || !finite_range(self.decay_seconds, 0.0, 60.0)
            || !finite_range(self.sustain, 0.0, 1.0)
            || !finite_range(self.release_seconds, 0.0, 60.0)
        {
            return Err(Invalid("sample envelope is outside its finite domain"));
        }
        let frames = |seconds: f64| (seconds * f64::from(rate)).round() as u64;
        Ok(PreparedEnvelope {
            attack: frames(self.attack_seconds),
            decay: frames(self.decay_seconds),
            sustain: self.sustain,
            release: frames(self.release_seconds),
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PreparedEnvelope {
    attack: u64,
    decay: u64,
    sustain: f64,
    release: u64,
}

impl PreparedEnvelope {
    pub(super) const fn release_frames(self) -> u64 {
        self.release
    }

    pub(super) fn held(self, age: u64) -> f64 {
        if age < self.attack {
            return age as f64 / self.attack as f64;
        }
        let age = age - self.attack;
        if age < self.decay {
            return 1.0 + (self.sustain - 1.0) * age as f64 / self.decay as f64;
        }
        self.sustain
    }

    pub(super) fn sample(self, age: u64, gate: u64) -> f64 {
        if age < gate {
            return self.held(age);
        }
        let release_age = age - gate;
        if release_age >= self.release {
            return 0.0;
        }
        self.held(gate) * (1.0 - release_age as f64 / self.release as f64)
    }
}
