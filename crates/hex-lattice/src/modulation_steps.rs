//! Bounded sample-clock step source with an inline, signed pattern.
//! Each step owns exactly the prepared number of frames. Ping-pong reverses
//! at the endpoints without repeating them. Handoff retains cursor and age,
//! clamped to the successor's length and step duration.
use hex_lattice_contract::DeviceContractError as Error;
use std::num::NonZeroU32;

pub const MAX_MODULATION_STEPS: usize = 16;

/// Traversal of the prepared steps on the admitted render clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModulationStepDirection {
    Forward,
    Backward,
    PingPong,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Steps {
    pattern: [i16; MAX_MODULATION_STEPS],
    count: u8,
    direction: ModulationStepDirection,
    duration: NonZeroU32,
    cursor: u8,
    age: u32,
    reversing: bool,
}

impl Steps {
    pub(crate) fn new(
        pattern: &[i16],
        duration: NonZeroU32,
        direction: ModulationStepDirection,
    ) -> Result<Self, Error> {
        if pattern.is_empty() || pattern.len() > MAX_MODULATION_STEPS || pattern.contains(&i16::MIN)
        {
            return Err(Error::InvalidParameterValue);
        }
        let mut values = [0; MAX_MODULATION_STEPS];
        values[..pattern.len()].copy_from_slice(pattern);
        let count = pattern.len() as u8;
        let cursor = match direction {
            ModulationStepDirection::Backward => count - 1,
            _ => 0,
        };
        Ok(Self {
            pattern: values,
            count,
            direction,
            duration,
            cursor,
            age: 0,
            reversing: false,
        })
    }

    pub(crate) fn inherit(&mut self, previous: Self) {
        self.cursor = previous.cursor.min(self.count - 1);
        self.age = previous.age.min(self.duration.get() - 1);
        self.reversing = previous.direction == ModulationStepDirection::PingPong
            && self.direction == ModulationStepDirection::PingPong
            && previous.reversing;
    }

    pub(crate) fn render(&mut self, output: &mut [f32]) {
        for sample in output {
            *sample = f32::from(self.pattern[usize::from(self.cursor)]) / f32::from(i16::MAX);
            self.age += 1;
            if self.age == self.duration.get() {
                self.age = 0;
                self.advance();
            }
        }
    }

    fn advance(&mut self) {
        if self.count == 1 {
            return;
        }
        let last = self.count - 1;
        self.cursor = match self.direction {
            ModulationStepDirection::Forward if self.cursor == last => 0,
            ModulationStepDirection::Backward if self.cursor == 0 => last,
            ModulationStepDirection::Backward => self.cursor - 1,
            ModulationStepDirection::PingPong => {
                if self.cursor == last {
                    self.reversing = true;
                } else if self.cursor == 0 {
                    self.reversing = false;
                }
                if self.reversing {
                    self.cursor - 1
                } else {
                    self.cursor + 1
                }
            }
            ModulationStepDirection::Forward => self.cursor + 1,
        };
    }
}
