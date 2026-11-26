use super::*;

// Admission needs identity, age and completion bounds, not oscillator/filter history.
// Keeping the proof separate prevents full 64-voice machine banks on callback stacks.
#[derive(Clone, Copy, Default)]
pub(super) struct EventVoice {
    pub(super) id: u64,
    pub(super) age: u64,
    pub(super) gate: Option<u64>,
    pub(super) completion: Option<(u64, u64)>,
    pub(super) pitch: u8,
    pub(super) sequence: u64,
    pub(super) velocity: u8,
    pub(super) active: bool,
    pub(super) envelope_age: f64,
    pub(super) envelope_gate: Option<u64>,
    pub(super) time_rate: f64,
    pub(super) drain: u32,
}
impl EventVoice {
    pub(super) fn read(voice: &Voice, factory: &Factory) -> Self {
        let completion = if factory.companion_active {
            voice
                .machines
                .iter()
                .try_fold((0, 0), |(relative, absolute), machine| {
                    let (r, a) = machine.as_ref()?.completion_bounds();
                    Some((relative.max(r), absolute.max(a)))
                })
        } else {
            Some((factory.configuration.release_frames(), 0))
        };
        Self {
            id: voice.id,
            age: voice.age,
            gate: voice.gate,
            completion,
            pitch: voice.note.midi_origin().0,
            sequence: voice.sequence,
            velocity: voice.note.midi_origin().1,
            active: factory.settings.global.active,
            envelope_age: voice.envelope_age,
            envelope_gate: voice.envelope_gate,
            time_rate: factory.effective_time_rate(voice.effective.time_rate),
            drain: voice.drain,
        }
    }
    pub(super) fn envelope_finished(&self) -> bool {
        self.envelope_gate
            .zip(self.completion)
            .is_some_and(|(gate, (relative, absolute))| {
                self.envelope_age as u64 >= gate.saturating_add(relative).max(absolute)
            })
    }
    pub(super) fn expire(&mut self) {
        if self.active {
            if self.id != 0 && self.drain >= quality::TAIL as u32 {
                *self = Self::default();
            }
            return;
        }
        if self.id != 0
            && self
                .gate
                .zip(self.completion)
                .is_some_and(|(gate, (relative, absolute))| {
                    self.age >= gate.saturating_add(relative).max(absolute)
                })
        {
            *self = Self::default();
        }
    }
}
pub(super) struct EventProof {
    pub(super) expression: [f32; 3],
    pub(super) ledger: Ledger,
    pub(super) voices: [EventVoice; VOICES],
    pub(super) local_frame: u64,
}
impl EventProof {
    pub(super) fn new(runtime: &Runtime) -> Self {
        Self {
            ledger: runtime.ledger,
            expression: runtime.expression,
            voices: std::array::from_fn(|i| EventVoice::read(&runtime.voices[i], &runtime.factory)),
            local_frame: runtime.local_frame,
        }
    }
    pub(super) fn advance(&mut self, frames: usize) -> Result<(), Error> {
        self.local_frame = self
            .local_frame
            .checked_add(frames as u64)
            .ok_or(Error::InvalidEventBatch)?;
        for voice in &mut self.voices {
            if voice.id != 0 {
                if voice.active {
                    for _ in 0..frames {
                        voice.age = voice.age.checked_add(1).ok_or(Error::InvalidEventBatch)?;
                        voice.envelope_age =
                            (voice.envelope_age + voice.time_rate).min(u64::MAX as f64);
                        if voice.envelope_finished() {
                            voice.drain = voice.drain.saturating_add(1);
                        } else {
                            voice.drain = 0;
                        }
                        voice.expire();
                        if voice.id == 0 {
                            break;
                        }
                    }
                } else {
                    voice.age = voice
                        .age
                        .checked_add(frames as u64)
                        .ok_or(Error::InvalidEventBatch)?;
                }
                voice.expire();
            }
        }
        Ok(())
    }
    pub(super) fn apply(
        &mut self,
        factory: &Factory,
        event: NativeInstrumentEvent,
    ) -> Result<(), Error> {
        if factory.settings.global.active {
            return self.apply_global(factory, event);
        }
        match event {
            NativeInstrumentEvent::NotePressure { .. } => {
                return Err(Error::UnsupportedNotePressure);
            }
            NativeInstrumentEvent::NotePitchBend { cents, .. } => {
                if !(-4_800..=4_800).contains(&cents) {
                    return Err(Error::InvalidEventBatch);
                }
            }
            NativeInstrumentEvent::NoteTimbre { value, .. } => {
                if value > 127 {
                    return Err(Error::InvalidEventBatch);
                }
            }
            NativeInstrumentEvent::ControlChange { cc, value, .. } => {
                if cc > 127 || value > 127 {
                    return Err(Error::InvalidEventBatch);
                }
            }
            NativeInstrumentEvent::PitchBend { value, .. } => {
                if !(-8192..=8191).contains(&value) {
                    return Err(Error::InvalidEventBatch);
                }
            }
            NativeInstrumentEvent::Expression { .. } => return Err(Error::InvalidEventBatch),
            NativeInstrumentEvent::NoteOn {
                occurrence_id,
                pitch,
                velocity,
                ..
            } => {
                if occurrence_id == 0 || self.voices.iter().any(|v| v.id == occurrence_id) {
                    return Err(Error::InvalidEventBatch);
                }
                // Musical clock validation remains separate; this proves all note
                // preparation plus identity/capacity without mutating live history.
                let prepared = factory.prepare_voice(
                    pitch,
                    velocity,
                    self.local_frame,
                    Some(0),
                    Some(false),
                )?;
                let voice = self.voices[..factory.capacity()]
                    .iter_mut()
                    .find(|v| v.id == 0)
                    .ok_or(Error::EventCapacityExceeded)?;
                *voice = EventVoice {
                    id: occurrence_id,
                    ..EventVoice::read(&prepared, factory)
                };
            }
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                let voice = self
                    .voices
                    .iter_mut()
                    .find(|v| v.id == occurrence_id)
                    .ok_or(Error::InvalidEventBatch)?;
                if voice.gate.is_some() {
                    return Err(Error::InvalidEventBatch);
                }
                voice.gate = Some(voice.age);
                voice.expire();
            }
        }
        Ok(())
    }
}
