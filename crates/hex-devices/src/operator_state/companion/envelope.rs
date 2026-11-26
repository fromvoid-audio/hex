//! Inline original envelope machines. Absolute frame/beat queries have bounded work.
use super::*;
use crate::operator_envelope::{OperatorEnvelopeShape, PreparedOperatorEnvelope};
use hex_lattice::NativeInstrumentRuntimeError as Error;
const SIXTEENTH: u64 = 1 << 30;
#[derive(Clone, Copy)]
pub(in crate::operator_state) struct Machine {
    envelope: PreparedOperatorEnvelope,
    cycle: PreparedOperatorEnvelope,
    shape: OperatorEnvelopeShape,
    levels: [f64; 3],
    mode: u32,
    loop_frames: u64,
    interval: u64,
    next_beat: Option<u64>,
    first: bool,
    origin: u64,
    release: Option<(u64, f64)>,
}
impl Machine {
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_with_note_pitch(
        settings: Settings,
        operator: usize,
        envelope: OperatorEnvelope,
        shape: OperatorEnvelopeShape,
        levels: [f64; 3],
        rate: u32,
        beat: Option<u64>,
        running: Option<bool>,
        note_hz: f64,
        loop_note_ratio: f64,
    ) -> Result<Self, Error> {
        let envelope = envelope
            .prepare(rate)
            .map_err(|_| Error::InvalidEventBatch)?;
        let mode = settings.words[operator][5];
        let interval =
            (BEATS[settings.words[operator][7] as usize] * 4_294_967_296.0).round() as u64;
        let next_beat = if matches!(mode, 2 | 3) {
            if mode == 3 && running.is_none() {
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
        let (cycle, loop_frames) = if mode == 5 {
            let hz = if note_hz.is_finite() && note_hz > 1.0 {
                note_hz
            } else {
                440.0
            };
            let ratio = if loop_note_ratio.is_finite() {
                loop_note_ratio.clamp(0.25, 4.0)
            } else {
                1.0
            };
            let period = (f64::from(rate) / (hz * ratio))
                .round()
                .clamp(2.0, f64::from(rate) * 60.0) as u64;
            let raw_a = envelope.attack_frames();
            let raw_d = envelope.decay_frames();
            let raw_ad = raw_a.saturating_add(raw_d);
            let scaled_a = if raw_ad == 0 {
                0
            } else {
                ((period as f64) * (raw_a as f64 / raw_ad as f64))
                    .round()
                    .clamp(0.0, period as f64) as u64
            };
            let scaled_d = period.saturating_sub(scaled_a);
            (envelope.with_ad_frames(scaled_a, scaled_d), period)
        } else {
            (
                envelope,
                (settings.float(operator, 6) * f64::from(rate)).round() as u64,
            )
        };
        Ok(Self {
            envelope,
            cycle,
            shape,
            levels,
            mode,
            loop_frames,
            interval,
            next_beat,
            first: true,
            origin: 0,
            release: None,
        })
    }

    #[cfg(test)]
    pub fn prepare(
        settings: Settings,
        operator: usize,
        envelope: OperatorEnvelope,
        levels: [f64; 3],
        rate: u32,
        beat: Option<u64>,
        running: Option<bool>,
    ) -> Result<Self, Error> {
        Self::prepare_with_note_pitch(
            settings,
            operator,
            envelope,
            OperatorEnvelopeShape::default(),
            levels,
            rate,
            beat,
            running,
            440.0,
            1.0,
        )
    }
    pub fn observe(
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
            let level = self.held(age);
            self.cycle = self.envelope.with_initial(level);
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
    fn held(&self, age: u64) -> f64 {
        match self.mode {
            1 => {
                let ad = self.envelope.attack_frames() + self.envelope.decay_frames();
                let period = ad + self.loop_frames;
                let at = age % period;
                if at < ad {
                    self.envelope.level_shaped(at, u64::MAX, self.shape)
                } else {
                    self.levels[2]
                        + (self.levels[0] - self.levels[2]) * (at - ad) as f64
                            / self.loop_frames as f64
                }
            }
            2 | 3 => {
                self.cycle
                    .level_shaped(age.saturating_sub(self.origin), u64::MAX, self.shape)
            }
            5 => self
                .cycle
                .level_shaped(age % self.loop_frames.max(1), u64::MAX, self.shape),
            _ => self.envelope.level_shaped(age, u64::MAX, self.shape),
        }
    }
    // Factory proves equal loop timing; released voices use the ordinary path.
    #[cold]
    pub fn shared_loop_levels(machines: &[Option<Self>; 4], age: u64) -> Option<[f64; 4]> {
        let [Some(a), Some(b), Some(c), Some(d)] = machines else {
            return None;
        };
        if [a, b, c, d].iter().any(|m| m.release.is_some()) {
            return None;
        }
        let ad = a.envelope.attack_frames() + a.envelope.decay_frames();
        let at = age % (ad + a.loop_frames);
        if at < ad {
            Some(PreparedOperatorEnvelope::shared_held_levels(
                [&a.envelope, &b.envelope, &c.envelope, &d.envelope],
                at,
            ))
        } else {
            Some([a, b, c, d].map(|m| {
                m.levels[2] + (m.levels[0] - m.levels[2]) * (at - ad) as f64 / m.loop_frames as f64
            }))
        }
    }
    pub fn note_off(&mut self, age: u64) {
        if self.release.is_none() {
            // Note-off takes priority over a simultaneous note-pitch loop reset,
            // starting release from the pre-reset level at the end of decay.
            let level = if self.mode == 5
                && age > 0
                && age.is_multiple_of(self.loop_frames.max(1))
            {
                self.levels[2]
            } else {
                self.held(age)
            };
            self.release = Some((age, level));
        }
    }
    pub fn level(&self, age: u64) -> f64 {
        if self.mode == 4 {
            return self.envelope.level_shaped(
                age,
                self.envelope.attack_frames() + self.envelope.decay_frames(),
                self.shape,
            );
        }
        let Some((gate, level)) = self.release else {
            return self.held(age);
        };
        if self.mode == 0 {
            return self.envelope.level_shaped(age, gate, self.shape);
        }
        let elapsed = age.saturating_sub(gate);
        self.envelope
            .released_level_shaped(level, elapsed, self.shape)
    }
    // Completion is independent of oscillator history and held/release levels.
    // The event admission proof uses the same bounds without cloning machines.
    pub fn completion_bounds(&self) -> (u64, u64) {
        if self.mode == 4 {
            (
                0,
                self.envelope.attack_frames()
                    + self.envelope.decay_frames()
                    + self.envelope.release_frames(),
            )
        } else {
            (self.envelope.release_frames(), 0)
        }
    }
    pub fn finished(&self, age: u64) -> bool {
        let (relative, absolute) = self.completion_bounds();
        if self.mode == 4 {
            age >= absolute
        } else {
            self.release
                .is_some_and(|(gate, _)| age >= gate.saturating_add(relative))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn machine(mode: u32, beat: Option<u64>, running: Option<bool>) -> Machine {
        let mut settings = Settings::default();
        settings.words[0][5] = mode;
        settings.words[0][6] = 0.001_f32.to_bits();
        settings.words[0][7] = 0;
        Machine::prepare(
            settings,
            0,
            OperatorEnvelope::new(0.1, 1.0, 0.4, 0.001, 0.002, 0.003).unwrap(),
            [0.1, 1.0, 0.4],
            8000,
            beat,
            running,
        )
        .unwrap()
    }
    #[test]
    fn loop_return_and_noteoff_release_have_exact_stage_boundaries() {
        let envelope = machine(1, None, None);
        for age in 0..96 {
            assert_eq!(envelope.level(age), envelope.level(age % 32));
        }
        assert_eq!(envelope.level(0), 0.1);
        assert_eq!(envelope.level(8), 1.0);
        assert_eq!(envelope.level(24), 0.4);
        assert!((envelope.level(28) - 0.25).abs() < 1e-12);
        for gate in [2, 8, 14, 24, 28, 40] {
            let mut released = envelope;
            let level = envelope.level(gate);
            released.note_off(gate);
            assert_eq!(released.level(gate), level);
            assert!((released.level(gate + 12) - level * 0.00001_f64.sqrt()).abs() < 1e-12);
            assert_eq!(released.level(gate + 24), 0.0);
            assert!(released.finished(gate + 24));
        }
    }
    #[test]
    fn shared_loop_timing_matches_independent_levels_and_refuses_release() {
        for (attack, decay) in [(0.0, 0.0), (0.001, 0.0), (0.0, 0.002), (0.001, 0.002)] {
            let mut machines = [machine(1, None, None); 4];
            for (index, machine) in machines.iter_mut().enumerate() {
                let sustain = [-0.0, 0.2, 0.7, 1.0][index];
                machine.envelope = OperatorEnvelope::new(0.1, 1.0, sustain, attack, decay, 0.003)
                    .unwrap()
                    .prepare(8000)
                    .unwrap();
                machine.levels = [0.1, 1.0, sustain];
            }
            let mut options = machines.map(Some);
            for age in 0..512 {
                assert_eq!(
                    Machine::shared_loop_levels(&options, age)
                        .unwrap()
                        .map(f64::to_bits),
                    machines.map(|m| m.level(age).to_bits())
                );
            }
            options[2].as_mut().unwrap().note_off(9);
            assert!(Machine::shared_loop_levels(&options, 10).is_none());
            options[2] = None;
            assert!(Machine::shared_loop_levels(&options, 10).is_none());
        }
    }
    #[test]
    fn beat_and_sync_restart_from_current_level_and_jump_without_replay() {
        let interval = 1 << 26;
        let mut beat = machine(2, Some(0), None);
        let before = beat.level(4);
        beat.observe(4, Some(interval), None).unwrap();
        assert_eq!(beat.level(4), before);
        assert_eq!(beat.level(12), 1.0);
        beat.observe(19, Some(1000u64 << 32), None).unwrap();
        assert!(beat.next_beat.unwrap() > (1000u64 << 32));
        assert!(beat.next_beat.unwrap() <= (1000u64 << 32) + interval);
        let mut sync = machine(3, Some(1 << 29), Some(true));
        sync.observe(2, Some((1 << 29) + interval), Some(true))
            .unwrap();
        assert!(sync.first);
        let before = sync.level(4);
        sync.observe(4, Some(1 << 30), Some(true)).unwrap();
        assert_eq!(sync.level(4), before);
        assert!(!sync.first);
        let mut stopped = machine(3, Some(1 << 29), Some(false));
        stopped
            .observe(2, Some((1 << 29) + interval), Some(false))
            .unwrap();
        assert!(!stopped.first);
        let mut invalid = Settings::default();
        invalid.words[0][5] = 3;
        let env = OperatorEnvelope::new(0.0, 1.0, 0.4, 0.001, 0.002, 0.003).unwrap();
        assert!(Machine::prepare(invalid, 0, env, [0.0, 1.0, 0.4], 8000, Some(0), None).is_err());
        invalid.words[0][5] = 2;
        assert!(
            Machine::prepare(invalid, 0, env, [0.0, 1.0, 0.4], 8000, None, Some(true)).is_err()
        );
    }
    #[test]
    fn trigger_is_finite_and_ignores_early_or_late_noteoff() {
        let trigger = machine(4, None, None);
        for gate in [0, 2, 8, 16, 32, 48, 80] {
            let mut released = trigger;
            released.note_off(gate);
            for age in 0..96 {
                assert_eq!(trigger.level(age), released.level(age));
            }
        }
        assert!(!trigger.finished(47));
        assert!(trigger.finished(48));
        assert_eq!(trigger.level(48), 0.0);
    }
    #[test]
    fn maximum_trigger_tail_extends_past_ten_seconds_but_is_finite_at_140_seconds() {
        let mut settings = Settings::default();
        settings.words[0][5] = 4;
        let machine = Machine::prepare(
            settings,
            0,
            OperatorEnvelope::new(0.0, 1.0, 0.4, 20.0, 60.0, 60.0).unwrap(),
            [0.0, 1.0, 0.4],
            8000,
            None,
            None,
        )
        .unwrap();
        assert!(machine.level(80001) > 0.0);
        assert!(!machine.finished(80001));
        assert!(!machine.finished(1_119_999));
        assert!(machine.finished(1_120_000));
        assert_eq!(machine.level(1_120_000), 0.0);
    }

    #[test]
    fn note_pitch_sync_fits_period_preserves_ratio_and_note_off_priority() {
        let mut settings = Settings::default();
        settings.words[0][5] = 5; // note-pitch
        let env = OperatorEnvelope::new(0.1, 1.0, 0.4, 0.002, 0.004, 0.002).unwrap();
        let shape = OperatorEnvelopeShape::new(0.2, 50.0, -50.0, 0.0).unwrap();
        // 400 Hz at 8000 Hz, ratio 0.5x => period = 8000 / 200 = 40 frames.
        // Attack:Decay = 1:2 => scaled_a = 13, scaled_d = 27.
        let mut m = Machine::prepare_with_note_pitch(
            settings,
            0,
            env,
            shape,
            [0.1, 1.0, 0.4],
            8_000,
            None,
            None,
            400.0,
            0.5,
        )
        .unwrap();
        assert_eq!(m.loop_frames, 40);
        for age in 0..120 {
            assert_eq!(m.level(age), m.level(age % 40));
        }
        assert!((m.level(0) - 0.1).abs() < 1e-12);
        assert!((m.level(13) - 1.0).abs() < 1e-12);
        // Simultaneous note-off at loop reset boundary (age = 40) takes priority
        // and starts release from pre-reset sustain (0.4), NOT reset initial (0.1).
        m.note_off(40);
        assert!((m.level(40) - 0.4).abs() < 1e-12);
        assert!((m.level(56) - 0.2).abs() < 1e-6);
        assert!(m.finished(56));
    }
}
