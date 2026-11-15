//! Bounded sample-clock step source with an inline, signed pattern.
//! Each step owns exactly the prepared number of frames. Ping-pong reverses
//! at the endpoints without repeating them. Handoff retains cursor and age,
//! clamped to the successor's length and step duration.
use crate::NativeInstrumentClock;
use hex_lattice_contract::DeviceContractError as Error;
use std::num::{NonZeroU32, NonZeroU64};

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
    pattern: Pattern,
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
        let pattern = Pattern::new(pattern)?;
        let cursor = pattern.first(direction);
        Ok(Self {
            pattern,
            direction,
            duration,
            cursor,
            age: 0,
            reversing: false,
        })
    }

    pub(crate) fn inherit(&mut self, previous: Self) {
        self.cursor = previous.cursor.min(self.pattern.count - 1);
        self.age = previous.age.min(self.duration.get() - 1);
        self.reversing = previous.direction == ModulationStepDirection::PingPong
            && self.direction == ModulationStepDirection::PingPong
            && previous.reversing;
    }

    pub(crate) fn render(&mut self, output: &mut [f32]) {
        for sample in output {
            *sample = self.pattern.sample(self.cursor);
            self.age += 1;
            if self.age == self.duration.get() {
                self.age = 0;
                self.advance();
            }
        }
    }

    fn advance(&mut self) {
        if self.pattern.count == 1 {
            return;
        }
        let last = self.pattern.count - 1;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pattern {
    values: [i16; MAX_MODULATION_STEPS],
    count: u8,
}

impl Pattern {
    fn new(pattern: &[i16]) -> Result<Self, Error> {
        if pattern.is_empty() || pattern.len() > MAX_MODULATION_STEPS || pattern.contains(&i16::MIN)
        {
            return Err(Error::InvalidParameterValue);
        }
        let mut values = [0; MAX_MODULATION_STEPS];
        values[..pattern.len()].copy_from_slice(pattern);
        Ok(Self {
            values,
            count: pattern.len() as u8,
        })
    }

    fn first(&self, direction: ModulationStepDirection) -> u8 {
        match direction {
            ModulationStepDirection::Backward => self.count - 1,
            _ => 0,
        }
    }

    fn sample(&self, cursor: u8) -> f32 {
        f32::from(self.values[usize::from(cursor)]) / f32::from(i16::MAX)
    }
}

/// A supplied Arrangement beat selects a step directly, including after
/// loops and seeks. A missing beat holds the last selected step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TransportSteps {
    pattern: Pattern,
    duration: NonZeroU64,
    direction: ModulationStepDirection,
    cursor: u8,
}

impl TransportSteps {
    pub(crate) fn new(
        pattern: &[i16],
        duration: NonZeroU64,
        direction: ModulationStepDirection,
    ) -> Result<Self, Error> {
        let pattern = Pattern::new(pattern)?;
        Ok(Self {
            pattern,
            duration,
            direction,
            cursor: pattern.first(direction),
        })
    }

    pub(crate) fn inherit(&mut self, previous: Self) {
        self.cursor = previous.cursor.min(self.pattern.count - 1);
    }

    pub(crate) fn render(&mut self, clock: Option<&dyn NativeInstrumentClock>, output: &mut [f32]) {
        let Some(clock) = clock else {
            output.fill(self.pattern.sample(self.cursor));
            return;
        };
        let count = u64::from(self.pattern.count);
        for (offset, sample) in output.iter_mut().enumerate() {
            if let Some(beat) = clock.arrangement_beat_at_offset(offset) {
                let step = beat / self.duration.get();
                self.cursor = match self.direction {
                    ModulationStepDirection::Forward => step % count,
                    ModulationStepDirection::Backward => count - 1 - step % count,
                    ModulationStepDirection::PingPong if count == 1 => 0,
                    ModulationStepDirection::PingPong => {
                        let period = 2 * (count - 1);
                        let step = step % period;
                        step.min(period - step)
                    }
                } as u8;
            }
            *sample = self.pattern.sample(self.cursor);
        }
    }
}
