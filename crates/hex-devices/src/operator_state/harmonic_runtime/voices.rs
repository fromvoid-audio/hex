//! Fixed occurrence ledger, independent of audible voice stealing.
use super::*;
#[derive(Clone, Copy, Default)]
pub(super) struct Held {
    pub id: u64,
    pub pitch: u8,
    pub velocity: u8,
    pub sequence: u64,
}
#[derive(Clone, Copy)]
pub(super) struct Ledger {
    pub held: [Held; 64],
    pub sequence: u64,
}
impl Default for Ledger {
    fn default() -> Self {
        Self {
            held: [Held::default(); 64],
            sequence: 0,
        }
    }
}
impl Ledger {
    pub fn on(&mut self, id: u64, pitch: u8, velocity: u8) -> Result<Held, Error> {
        if id == 0 || self.held.iter().any(|h| h.id == id) {
            return Err(Error::InvalidEventBatch);
        }
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or(Error::InvalidEventBatch)?;
        let slot = self
            .held
            .iter_mut()
            .find(|h| h.id == 0)
            .ok_or(Error::EventCapacityExceeded)?;
        *slot = Held {
            id,
            pitch,
            velocity,
            sequence,
        };
        self.sequence = sequence;
        Ok(*slot)
    }
    pub fn off(&mut self, id: u64) -> Result<(), Error> {
        let slot = self
            .held
            .iter_mut()
            .find(|h| h.id == id && id != 0)
            .ok_or(Error::InvalidEventBatch)?;
        *slot = Held::default();
        Ok(())
    }
    pub fn latest(&self) -> Option<Held> {
        self.held
            .iter()
            .filter(|h| h.id != 0)
            .max_by_key(|h| h.sequence)
            .copied()
    }
}
fn slot(voices: &[EventVoice; VOICES], settings: global::Settings, pitch: u8) -> usize {
    let capacity = settings.capacity();
    if capacity == 1 {
        return 0;
    }
    if settings.retrigger
        && let Some(i) = voices[..capacity]
            .iter()
            .position(|v| v.id != 0 && v.gate.is_none() && v.pitch == pitch)
    {
        return i;
    }
    voices[..capacity]
        .iter()
        .position(|v| v.id == 0)
        .unwrap_or_else(|| {
            voices[..capacity]
                .iter()
                .enumerate()
                .min_by_key(|(_, v)| v.sequence)
                .map_or(0, |(i, _)| i)
        })
}
impl EventProof {
    pub(super) fn apply_global(
        &mut self,
        factory: &Factory,
        event: NativeInstrumentEvent,
    ) -> Result<(), Error> {
        match event {
            NativeInstrumentEvent::NotePressure { .. } => {
                return Err(Error::UnsupportedNotePressure);
            }
            NativeInstrumentEvent::Expression { expression, .. } => {
                let next = modulation::expression_after(self.expression, expression)?;
                if next == self.expression {
                    return Ok(());
                }
                self.expression = next;
                for voice in &mut self.voices {
                    if voice.id != 0 {
                        let effective = factory
                            .settings
                            .modulation
                            .evaluate(voice.pitch, voice.velocity, self.expression)
                            .map_err(|_| Error::InvalidEventBatch)?;
                        factory.modulated_filter(effective)?;
                        voice.time_rate = effective.time_rate;
                    }
                }
            }
            NativeInstrumentEvent::NoteOn {
                occurrence_id,
                pitch,
                velocity,
                ..
            } => {
                if self
                    .voices
                    .iter()
                    .any(|v| v.id == occurrence_id && occurrence_id != 0)
                {
                    return Err(Error::InvalidEventBatch);
                }
                let mut prepared = factory.prepare_voice(
                    pitch,
                    velocity,
                    self.local_frame,
                    Some(0),
                    Some(false),
                )?;
                prepared.update_effective(factory, self.expression)?;
                let held = self.ledger.on(occurrence_id, pitch, velocity)?;
                let index = slot(&self.voices, factory.settings.global, pitch);
                let legato = factory.settings.global.capacity() == 1
                    && self.voices[index].id != 0
                    && self.voices[index].gate.is_none()
                    && (!factory.settings.global.retrigger || self.voices[index].pitch != pitch);
                if !legato {
                    self.voices[index] = EventVoice::read(&prepared, factory)
                }
                self.voices[index].id = held.id;
                self.voices[index].pitch = pitch;
                self.voices[index].sequence = held.sequence;
                self.voices[index].velocity = velocity;
                self.voices[index].time_rate = prepared.effective.time_rate;
            }
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                self.ledger.off(occurrence_id)?;
                if let Some(index) = self
                    .voices
                    .iter()
                    .position(|v| v.id == occurrence_id && occurrence_id != 0)
                {
                    if factory.settings.global.capacity() == 1
                        && let Some(held) = self.ledger.latest()
                    {
                        let mut prepared = factory.prepare_voice(
                            held.pitch,
                            held.velocity,
                            self.local_frame,
                            Some(0),
                            Some(false),
                        )?;
                        prepared.update_effective(factory, self.expression)?;
                        if factory.settings.global.retrigger
                            && self.voices[index].pitch == held.pitch
                        {
                            self.voices[index] = EventVoice::read(&prepared, factory)
                        }
                        self.voices[index].id = held.id;
                        self.voices[index].pitch = held.pitch;
                        self.voices[index].sequence = held.sequence;
                        self.voices[index].velocity = held.velocity;
                        self.voices[index].time_rate = prepared.effective.time_rate;
                    } else {
                        self.voices[index].gate = Some(self.voices[index].age);
                        self.voices[index].envelope_gate =
                            Some(self.voices[index].envelope_age as u64);
                        self.voices[index].expire();
                    }
                }
            }
        }
        Ok(())
    }
}
impl Runtime {
    pub(super) fn apply_global(
        &mut self,
        event: NativeInstrumentEvent,
        origin: u64,
        beat: Option<u64>,
        running: Option<bool>,
    ) -> Result<(), Error> {
        match event {
            NativeInstrumentEvent::NotePressure { .. } => {
                return Err(Error::UnsupportedNotePressure);
            }
            NativeInstrumentEvent::Expression { expression, .. } => {
                return self.expression(expression);
            }
            NativeInstrumentEvent::NoteOn {
                occurrence_id,
                pitch,
                velocity,
                ..
            } => {
                if self
                    .voices
                    .iter()
                    .any(|v| v.id == occurrence_id && occurrence_id != 0)
                {
                    return Err(Error::InvalidEventBatch);
                }
                let prepared = self
                    .factory
                    .prepare_voice(pitch, velocity, origin, beat, running)?;
                let held = self.ledger.on(occurrence_id, pitch, velocity)?;
                let views =
                    std::array::from_fn(|i| EventVoice::read(&self.voices[i], &self.factory));
                let index = slot(&views, self.factory.settings.global, pitch);
                let voice = &mut self.voices[index];
                let legato = self.factory.settings.global.capacity() == 1
                    && voice.id != 0
                    && voice.gate.is_none()
                    && (!self.factory.settings.global.retrigger
                        || voice.note.midi_origin().0 != pitch);
                if legato {
                    voice.note = prepared.note;
                } else {
                    *voice = prepared;
                }
                voice.id = held.id;
                voice.sequence = held.sequence;
                voice.update_effective(&self.factory, self.expression)?;
                if !legato && !self.factory.settings.lfo.retrigger() {
                    voice.lfo_fraction = self.phase.0;
                    voice.lfo_cycle = self.phase.1;
                }
            }
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                self.ledger.off(occurrence_id)?;
                if let Some(index) = self
                    .voices
                    .iter()
                    .position(|v| v.id == occurrence_id && occurrence_id != 0)
                {
                    if self.factory.settings.global.capacity() == 1
                        && let Some(held) = self.ledger.latest()
                    {
                        let prepared = self.factory.prepare_voice(
                            held.pitch,
                            held.velocity,
                            origin,
                            beat,
                            running,
                        )?;
                        let voice = &mut self.voices[index];
                        if self.factory.settings.global.retrigger
                            && voice.note.midi_origin().0 == held.pitch
                        {
                            *voice = prepared;
                        } else {
                            voice.note = prepared.note;
                        }
                        voice.id = held.id;
                        voice.sequence = held.sequence;
                        voice.update_effective(&self.factory, self.expression)?;
                    } else {
                        let voice = &mut self.voices[index];
                        voice.gate = Some(voice.age);
                        voice.envelope_gate = Some(voice.envelope_frame());
                        for machine in voice.machines.iter_mut().flatten() {
                            machine.note_off(voice.envelope_age as u64);
                        }
                        self.expire();
                    }
                }
            }
        }
        Ok(())
    }
    /// Preserve the latest voices, retiring stolen occurrences only on their note-off.
    pub(super) fn enforce_polyphony(&mut self) {
        let capacity = self.factory.settings.global.capacity();
        while self.voices.iter().filter(|v| v.id != 0).count() > capacity {
            if let Some(index) = self
                .voices
                .iter()
                .enumerate()
                .filter(|(_, v)| v.id != 0)
                .min_by_key(|(_, v)| v.sequence)
                .map(|(i, _)| i)
            {
                self.voices[index] = Voice::default();
            }
        }
        for i in 0..capacity {
            if self.voices[i].id == 0
                && let Some(j) = (i + 1..VOICES).find(|j| self.voices[*j].id != 0)
            {
                self.voices[i] = self.voices[j];
                self.voices[j] = Voice::default();
            }
        }
    }
    pub(super) fn seed_ledger(&mut self) {
        self.ledger = Ledger::default();
        // Analytic histories have monotonically ordered ages; older notes receive earlier onsets.
        let mut indices: [usize; 64] = std::array::from_fn(|i| i);
        indices.sort_unstable_by_key(|i| {
            (
                std::cmp::Reverse(self.voices[*i].age),
                self.voices[*i].id,
                *i,
            )
        });
        for i in indices {
            let voice = &mut self.voices[i];
            if voice.id != 0 {
                let (pitch, velocity) = voice.note.midi_origin();
                if let Ok(held) = self.ledger.on(voice.id, pitch, velocity) {
                    voice.sequence = held.sequence;
                }
                voice.envelope_age = voice.age as f64;
                voice.envelope_gate = voice.gate;
                let _ = voice.update_effective(&self.factory, self.expression);
                if voice.gate.is_some() {
                    let _ = self.ledger.off(voice.id);
                }
            }
        }
    }
}
