//! One held counter step for each Note On at its exact sample offset.
//!
//! The instrument host owns the bounded, ordered event slice. Counter state
//! belongs to the bank transaction and never retains the borrowed events.
use crate::{NativeInstrumentEvent, modulation_note_events::note_on_offsets};
use hex_lattice_contract::DeviceContractError as Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NoteCounter {
    steps: u16,
    increment: u16,
    bipolar: bool,
    step: u16,
}

impl NoteCounter {
    pub(crate) fn new(steps: u16, increment: u16, bipolar: bool) -> Result<Self, Error> {
        if !(2..=128).contains(&steps) || !(1..=128).contains(&increment) {
            return Err(Error::InvalidParameterValue);
        }
        Ok(Self {
            steps,
            increment,
            bipolar,
            step: 0,
        })
    }

    pub(crate) fn inherit(&mut self, previous: Self) {
        self.step = previous.step % self.steps;
    }

    pub(crate) fn render(&mut self, events: &[NativeInstrumentEvent], output: &mut [f32]) {
        let mut onsets = note_on_offsets(events).peekable();
        for (offset, sample) in output.iter_mut().enumerate() {
            while onsets.next_if_eq(&offset).is_some() {
                self.step = (self.step + self.increment) % self.steps;
            }
            let value = f32::from(self.step) / f32::from(self.steps - 1);
            *sample = if self.bipolar {
                value * 2.0 - 1.0
            } else {
                value
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unipolar_counter_wraps_from_full_scale_and_rejects_invalid_settings() {
        assert!(NoteCounter::new(1, 1, false).is_err());
        assert!(NoteCounter::new(129, 1, false).is_err());
        assert!(NoteCounter::new(4, 0, false).is_err());
        assert!(NoteCounter::new(4, 129, false).is_err());
        let mut counter = NoteCounter::new(4, 3, false).unwrap();
        let onset = |frame_offset, occurrence_id| NativeInstrumentEvent::NoteOn {
            frame_offset,
            occurrence_id,
            pitch: 60,
            velocity: 100,
        };
        let mut output = [0.0; 5];
        counter.render(
            &[onset(1, 1), onset(2, 2), onset(3, 3), onset(4, 4)],
            &mut output,
        );
        assert_eq!(output, [0.0, 1.0, 2.0 / 3.0, 1.0 / 3.0, 0.0]);
    }
}
