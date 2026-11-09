//! Sample-offset restart and held or repeating ramp for one instrument bank.
use crate::{NativeInstrumentEvent, modulation_note_events::note_on_offsets};
use std::num::NonZeroU32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModulationRampCurve {
    Linear,
    EaseIn,
    EaseOut,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Ramp {
    frames: NonZeroU32,
    curve: ModulationRampCurve,
    falling: bool,
    looping: bool,
    elapsed: Option<u32>,
}

impl Ramp {
    pub(crate) fn new(
        frames: NonZeroU32,
        curve: ModulationRampCurve,
        falling: bool,
        looping: bool,
    ) -> Self {
        Self {
            frames,
            curve,
            falling,
            looping,
            elapsed: None,
        }
    }

    pub(crate) fn inherit(&mut self, previous: Self) {
        self.elapsed = previous.elapsed.map(|elapsed| self.position(elapsed));
    }

    fn position(self, elapsed: u32) -> u32 {
        if self.looping {
            elapsed % self.frames.get()
        } else {
            elapsed.min(self.frames.get())
        }
    }

    pub(crate) fn render(&mut self, events: &[NativeInstrumentEvent], output: &mut [f32]) {
        let mut onsets = note_on_offsets(events).peekable();
        let duration = self.frames.get();
        let inverse = 1.0 / duration as f32;
        for (offset, sample) in output.iter_mut().enumerate() {
            while onsets.next_if_eq(&offset).is_some() {
                self.elapsed = Some(0);
            }
            let Some(elapsed) = self.elapsed else {
                *sample = 0.0;
                continue;
            };
            let t = if elapsed == duration {
                1.0
            } else {
                elapsed as f32 * inverse
            };
            let value = match self.curve {
                ModulationRampCurve::Linear => t,
                ModulationRampCurve::EaseIn => t * t,
                ModulationRampCurve::EaseOut => t * (2.0 - t),
            };
            *sample = if self.falling { 1.0 - value } else { value };
            let next = elapsed + 1;
            self.elapsed = Some(if next < duration {
                next
            } else if self.looping {
                0
            } else {
                duration
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_shot_reaches_and_holds_exact_endpoint_and_retriggers() {
        let mut ramp = Ramp::new(
            NonZeroU32::new(4).unwrap(),
            ModulationRampCurve::EaseOut,
            false,
            false,
        );
        let onset = |frame_offset, occurrence_id| NativeInstrumentEvent::NoteOn {
            frame_offset,
            occurrence_id,
            pitch: 60,
            velocity: 100,
        };
        let mut output = [0.0; 8];
        ramp.render(&[onset(2, 1), onset(2, 2)], &mut output);
        assert_eq!(output, [0.0, 0.0, 0.0, 0.4375, 0.75, 0.9375, 1.0, 1.0]);
        ramp.render(&[onset(1, 3)], &mut output[..3]);
        assert_eq!(&output[..3], &[1.0, 0.0, 0.4375]);
    }
}
