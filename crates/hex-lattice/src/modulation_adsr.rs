//! Linear ADSR over the receiving instrument's aggregate note gate.
//! Attack restarts from the previous output. Release starts on the last
//! note-off sample and retains the last output as its starting value.
use crate::modulation_note_gate::GateFrame;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Segment {
    Idle,
    Attack { from: f32, age: u32 },
    Decay(u32),
    Sustain,
    Release { from: f32, age: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Adsr {
    attack: u32,
    decay: u32,
    release: u32,
    sustain: f32,
    single_trigger: bool,
    segment: Segment,
    level: f32,
}

impl Adsr {
    pub(crate) fn new(durations: [u32; 3], sustain: f32, single_trigger: bool) -> Self {
        let [attack, decay, release] = durations;
        Self {
            attack,
            decay,
            release,
            sustain,
            single_trigger,
            segment: Segment::Idle,
            level: 0.0,
        }
    }

    fn start_decay(self) -> Segment {
        if self.decay == 0 {
            Segment::Sustain
        } else {
            Segment::Decay(0)
        }
    }

    pub(crate) fn inherit(&mut self, previous: Self) {
        self.level = previous.level;
        self.segment = match previous.segment {
            Segment::Attack { age, .. } if age >= self.attack => self.start_decay(),
            Segment::Decay(age) if age >= self.decay => Segment::Sustain,
            Segment::Release { age, .. } if age >= self.release => Segment::Idle,
            segment => segment,
        };
    }

    pub(crate) fn render(&mut self, gate: &[GateFrame], output: &mut [f32]) {
        for (gate, sample) in gate.iter().zip(output) {
            if gate.held {
                if gate.opened || (gate.onset && !self.single_trigger) {
                    self.segment = if self.attack == 0 {
                        self.start_decay()
                    } else {
                        Segment::Attack {
                            from: self.level,
                            age: 0,
                        }
                    };
                }
            } else if matches!(
                self.segment,
                Segment::Attack { .. } | Segment::Decay(_) | Segment::Sustain
            ) {
                self.segment = if self.release == 0 {
                    Segment::Idle
                } else {
                    Segment::Release {
                        from: self.level,
                        age: 0,
                    }
                };
            }
            let (value, next) = match self.segment {
                Segment::Idle => (0.0, Segment::Idle),
                Segment::Attack { from, age } => {
                    let value = from + (1.0 - from) * (age as f32 / self.attack as f32);
                    let next = if age + 1 == self.attack {
                        self.start_decay()
                    } else {
                        Segment::Attack { from, age: age + 1 }
                    };
                    (value, next)
                }
                Segment::Decay(age) => {
                    let value = 1.0 - (1.0 - self.sustain) * (age as f32 / self.decay as f32);
                    let next = if age + 1 == self.decay {
                        Segment::Sustain
                    } else {
                        Segment::Decay(age + 1)
                    };
                    (value, next)
                }
                Segment::Sustain => (self.sustain, Segment::Sustain),
                Segment::Release { from, age } => {
                    let value = from * (1.0 - age as f32 / self.release as f32);
                    let next = if age + 1 == self.release {
                        Segment::Idle
                    } else {
                        Segment::Release { from, age: age + 1 }
                    };
                    (value, next)
                }
            };
            self.level = value;
            self.segment = next;
            *sample = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_durations_use_the_sustain_while_held_and_clear_on_release() {
        let mut envelope = Adsr::new([0; 3], 0.75, true);
        let gate = [
            GateFrame::default(),
            GateFrame {
                held: true,
                opened: true,
                onset: true,
            },
            GateFrame {
                held: true,
                ..GateFrame::default()
            },
            GateFrame::default(),
        ];
        let mut output = [0.0; 4];
        envelope.render(&gate, &mut output);
        assert_eq!(output, [0.0, 0.75, 0.75, 0.0]);
    }
}
