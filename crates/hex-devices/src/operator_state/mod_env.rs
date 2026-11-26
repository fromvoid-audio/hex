//! Reusable zero-allocation multi-stage sloped envelope with loop modes.
//!
//! Shared by Vector's per-voice pitch envelope, filter envelope, and LFO
//! envelope. All durations are validated on control and converted to bounded
//! frame counts (`<= 11,520,000` frames at 192 kHz, fitting cleanly in `u32`).
use hex_lattice::NativeInstrumentRuntimeError as Error;

pub(super) const LOOPS: [&str; 5] = ["none", "loop", "beat", "sync", "trigger"];
const SIXTEENTH: u64 = 1 << 30;

/// Monotonic segment curve mapping `t in [0, 1]` with `slope_pct in [-100, 100]`.
///
/// `0%` is exact linear; positive values bow convex (fast start), negative
/// values bow concave (slow start). Endpoints 0 and 1 are exact.
#[inline]
pub(super) fn shape(t: f64, slope_pct: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    if t == 0.0 || t == 1.0 || slope_pct == 0.0 {
        return t;
    }
    let k = -(slope_pct.clamp(-100.0, 100.0) / 100.0) * 4.0;
    if k.abs() < 1e-6 {
        return t;
    }
    let denom = k.exp_m1();
    if denom == 0.0 || !denom.is_finite() {
        t
    } else {
        ((k * t).exp_m1() / denom).clamp(0.0, 1.0)
    }
}

/// Validated authoring parameters for one multi-stage sloped envelope.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Spec {
    pub initial: f32,
    pub peak: f32,
    pub sustain: f32,
    pub end: f32,
    pub attack: f32,
    pub decay: f32,
    pub release: f32,
    pub attack_slope: f32,
    pub decay_slope: f32,
    pub release_slope: f32,
    pub loop_mode: u32,
    pub loop_time: f32,
    pub beat_choice: u32,
}

/// Compact inline per-voice envelope machine (zero heap allocation).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Machine {
    initial: f32,
    cycle_initial: f32,
    peak: f32,
    sustain: f32,
    end: f32,
    attack_slope: f32,
    decay_slope: f32,
    release_slope: f32,
    attack_frames: u32,
    decay_frames: u32,
    release_frames: u32,
    loop_frames: u32,
    mode: u8,
    first: bool,
    interval: u64,
    next_beat: Option<u64>,
    origin: u64,
    release: Option<(u64, f32)>,
}

impl Machine {
    pub(super) fn prepare(
        spec: Spec,
        rate: u32,
        beat: Option<u64>,
        running: Option<bool>,
    ) -> Result<Self, Error> {
        if !(8_000..=192_000).contains(&rate) {
            return Err(Error::InvalidEventBatch);
        }
        let to_frames = |seconds: f32| -> u32 {
            (f64::from(seconds.max(0.0)) * f64::from(rate))
                .round()
                .clamp(0.0, f64::from(u32::MAX)) as u32
        };
        let beat_idx = (spec.beat_choice as usize).min(super::companion::BEATS.len() - 1);
        let interval = (super::companion::BEATS[beat_idx] * 4_294_967_296.0).round() as u64;
        let next_beat = if matches!(spec.loop_mode, 2 | 3) {
            if spec.loop_mode == 3 && running.is_none() {
                return Err(Error::InvalidEventBatch);
            }
            Some(
                beat.ok_or(Error::InvalidEventBatch)?
                    .checked_add(interval)
                    .ok_or(Error::InvalidEventBatch)?,
            )
        } else {
            None
        };
        Ok(Self {
            initial: spec.initial,
            cycle_initial: spec.initial,
            peak: spec.peak,
            sustain: spec.sustain,
            end: spec.end,
            attack_slope: spec.attack_slope,
            decay_slope: spec.decay_slope,
            release_slope: spec.release_slope,
            attack_frames: to_frames(spec.attack),
            decay_frames: to_frames(spec.decay),
            release_frames: to_frames(spec.release),
            loop_frames: to_frames(spec.loop_time).max(1),
            mode: spec.loop_mode as u8,
            first: true,
            interval,
            next_beat,
            origin: 0,
            release: None,
        })
    }

    pub(super) fn observe(
        &mut self,
        age: u64,
        beat: Option<u64>,
        running: Option<bool>,
    ) -> Result<(), Error> {
        if self.release.is_some() || !matches!(self.mode, 2 | 3) {
            return Ok(());
        }
        let beat = beat.ok_or(Error::InvalidEventBatch)?;
        let mut due = self.next_beat.ok_or(Error::InvalidEventBatch)?;
        if self.mode == 3 && running.ok_or(Error::InvalidEventBatch)? && self.first {
            due = due
                .checked_add(SIXTEENTH - 1)
                .ok_or(Error::InvalidEventBatch)?
                / SIXTEENTH
                * SIXTEENTH;
        }
        if beat >= due {
            self.cycle_initial = self.held(age) as f32;
            self.origin = age;
            self.first = false;
            let skipped = (beat - due) / self.interval;
            self.next_beat = Some(
                due.checked_add(
                    skipped
                        .checked_add(1)
                        .and_then(|n| n.checked_mul(self.interval))
                        .ok_or(Error::InvalidEventBatch)?,
                )
                .ok_or(Error::InvalidEventBatch)?,
            );
        }
        Ok(())
    }

    #[inline]
    fn eval_ad(&self, initial: f64, at: u64) -> f64 {
        let attack = u64::from(self.attack_frames);
        let peak = f64::from(self.peak);
        let sustain = f64::from(self.sustain);
        if at < attack {
            let t = at as f64 / attack as f64;
            return initial + (peak - initial) * shape(t, f64::from(self.attack_slope));
        }
        let decay = u64::from(self.decay_frames);
        let decay_at = at - attack;
        if decay_at < decay {
            let t = decay_at as f64 / decay as f64;
            return peak + (sustain - peak) * shape(t, f64::from(self.decay_slope));
        }
        sustain
    }

    #[inline]
    fn held(&self, age: u64) -> f64 {
        match self.mode {
            1 => {
                let ad = u64::from(self.attack_frames) + u64::from(self.decay_frames);
                let loop_f = u64::from(self.loop_frames).max(1);
                let period = ad + loop_f;
                let at = age % period;
                if at < ad {
                    self.eval_ad(f64::from(self.initial), at)
                } else {
                    let t = (at - ad) as f64 / loop_f as f64;
                    f64::from(self.sustain)
                        + (f64::from(self.initial) - f64::from(self.sustain)) * t
                }
            }
            2 | 3 => self.eval_ad(
                f64::from(self.cycle_initial),
                age.saturating_sub(self.origin),
            ),
            _ => self.eval_ad(f64::from(self.initial), age),
        }
    }

    pub(super) fn note_off(&mut self, age: u64) {
        if self.release.is_none() {
            self.release = Some((age, self.held(age) as f32));
        }
    }

    #[inline]
    pub(super) fn value(&self, age: u64) -> f64 {
        if self.mode == 4 {
            let ad = u64::from(self.attack_frames) + u64::from(self.decay_frames);
            if age < ad {
                return self.eval_ad(f64::from(self.initial), age);
            }
            let rel_age = age - ad;
            return self.eval_release(f64::from(self.sustain), rel_age);
        }
        let Some((gate, start_level)) = self.release else {
            return self.held(age);
        };
        self.eval_release(f64::from(start_level), age.saturating_sub(gate))
    }

    #[inline]
    fn eval_release(&self, start_level: f64, elapsed: u64) -> f64 {
        let release = u64::from(self.release_frames);
        let end = f64::from(self.end);
        if elapsed >= release || release == 0 {
            end
        } else {
            let t = elapsed as f64 / release as f64;
            start_level + (end - start_level) * shape(t, f64::from(self.release_slope))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sloped_bipolar_envelope_and_loop_modes_match_exact_boundaries() {
        let spec = Spec {
            initial: -12.0,
            peak: 24.0,
            sustain: 6.0,
            end: -24.0,
            attack: 0.001,
            decay: 0.002,
            release: 0.002,
            attack_slope: 50.0,
            decay_slope: -50.0,
            release_slope: 0.0,
            loop_mode: 0,
            loop_time: 0.001,
            beat_choice: 0,
        };
        let mut m = Machine::prepare(spec, 8_000, None, None).unwrap();
        assert!((m.value(0) - (-12.0)).abs() < 1e-6);
        assert!((m.value(8) - 24.0).abs() < 1e-6);
        assert!((m.value(24) - 6.0).abs() < 1e-6);
        assert!((m.value(40) - 6.0).abs() < 1e-6);
        // Release from mid-decay starts at exact latched level and ends at `end`.
        let mid_level = m.value(16);
        m.note_off(16);
        assert!((m.value(16) - mid_level).abs() < 1e-5);
        assert!((m.value(24) - (mid_level + (-24.0 - mid_level) * 0.5)).abs() < 1e-4);
        assert!((m.value(32) - (-24.0)).abs() < 1e-6);
        assert!((m.value(100) - (-24.0)).abs() < 1e-6);

        // Positive slope bows above linear at midpoint; negative slope bows below.
        assert!(shape(0.5, 80.0) > 0.5);
        assert!(shape(0.5, -80.0) < 0.5);
        assert_eq!(shape(0.5, 0.0), 0.5);
    }
}
