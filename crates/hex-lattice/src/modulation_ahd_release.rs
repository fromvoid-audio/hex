//! A bounded attack, hold and decay envelope restarted by note releases.
use crate::{ModulationRampCurve, NativeInstrumentEvent, modulation_note_events::note_off_offsets};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AhdRelease {
    attack: u32,
    hold: u32,
    total: u32,
    attack_inverse: f32,
    decay_inverse: f32,
    curves: [ModulationRampCurve; 2],
    single_trigger: bool,
    elapsed: Option<u32>,
}

impl AhdRelease {
    pub(crate) fn new(
        durations: [u32; 3],
        curves: [ModulationRampCurve; 2],
        single_trigger: bool,
    ) -> Self {
        let [attack, hold, decay] = durations;
        let inverse = |frames| {
            if frames == 0 {
                0.0
            } else {
                1.0 / frames as f32
            }
        };
        Self {
            attack,
            hold,
            total: attack + hold + decay,
            attack_inverse: inverse(attack),
            decay_inverse: inverse(decay),
            curves,
            single_trigger,
            elapsed: None,
        }
    }

    pub(crate) fn inherit(&mut self, previous: Self) {
        self.elapsed = previous.elapsed.filter(|elapsed| *elapsed < self.total);
    }

    pub(crate) fn render(&mut self, events: &[NativeInstrumentEvent], output: &mut [f32]) {
        let mut releases = note_off_offsets(events).peekable();
        let decay_start = self.attack + self.hold;
        for (offset, sample) in output.iter_mut().enumerate() {
            while releases.next_if_eq(&offset).is_some() {
                if !self.single_trigger || self.elapsed.is_none() {
                    self.elapsed = Some(0);
                }
            }
            let Some(age) = self.elapsed else {
                *sample = 0.0;
                continue;
            };
            *sample = if age < self.attack {
                self.curves[0].at(age as f32 * self.attack_inverse)
            } else if age < decay_start {
                1.0
            } else {
                1.0 - self.curves[1].at((age - decay_start) as f32 * self.decay_inverse)
            };
            let next = age + 1;
            self.elapsed = (next < self.total).then_some(next);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ModulationRampCurve::{EaseIn, EaseOut, Linear};

    #[test]
    fn note_releases_obey_curve_retrigger_and_zero_duration_segments() {
        let off = |frame_offset, occurrence_id| NativeInstrumentEvent::NoteOff {
            frame_offset,
            occurrence_id,
        };
        let on = NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 60,
            velocity: 100,
        };
        let events = [on, off(2, 1), off(5, 2)];
        let mut output = [0.0; 12];
        for (single, expected) in [
            (
                true,
                [0.0, 0.0, 0.0, 0.25, 1.0, 1.0, 1.0, 0.25, 0.0, 0.0, 0.0, 0.0],
            ),
            (
                false,
                [
                    0.0, 0.0, 0.0, 0.25, 1.0, 0.0, 0.25, 1.0, 1.0, 1.0, 0.25, 0.0,
                ],
            ),
        ] {
            AhdRelease::new([2, 2, 2], [EaseIn, EaseOut], single).render(&events, &mut output);
            assert_eq!(output, expected);
        }
        AhdRelease::new([0, 1, 0], [Linear; 2], true).render(&[off(1, 1)], &mut output[..3]);
        assert_eq!(&output[..3], &[0.0, 1.0, 0.0]);
    }
}
