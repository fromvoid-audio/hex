//! Hardware events share the admitted keyboard pool, with distinct occurrence
//! ownership. Timing is explicitly block-quantized: pending events begin at the
//! next render boundary, in wire order, at most one per sample. This preserves
//! positive gates for short on/off pairs without claiming arrival timestamps.
use super::*;

#[cfg(test)]
mod tests;

struct OffsetClock<'a> {
    clock: &'a dyn hex_lattice::NativeInstrumentClock,
    offset: usize,
}
impl hex_lattice::NativeInstrumentClock for OffsetClock<'_> {
    fn tempo_micro_bpm_at_offset(&self, offset: usize) -> Option<u32> {
        self.clock
            .tempo_micro_bpm_at_offset(self.offset.checked_add(offset)?)
    }

    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock.beat_at_offset(self.offset.checked_add(offset)?)
    }
    fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock
            .arrangement_beat_at_offset(self.offset.checked_add(offset)?)
    }
    fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock
            .arrangement_frame_at_offset(self.offset.checked_add(offset)?)
    }
    fn loop_pass_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock
            .loop_pass_at_offset(self.offset.checked_add(offset)?)
    }
    fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock
            .performance_frame_at_offset(self.offset.checked_add(offset)?)
    }
    fn transport_is_running(&self) -> Option<bool> {
        self.clock.transport_is_running()
    }
}

impl ComputerKeyboardRuntime {
    pub(super) fn render_live_spans(
        &mut self,
        output: &mut [Frame],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), Invalid> {
        if output.is_empty() {
            return Ok(());
        }
        let cancelled = self.input.lease().is_none_or(|input| {
            input.live_notes.as_ref().is_none_or(|live| {
                live.is_cancelled()
                    || self
                        .live_batch
                        .as_ref()
                        .is_none_or(|batch| !live.same_current_activation(batch.activation))
            }) || Some(input.live_output_epoch.load(Acquire)) != self.activation_epoch
        }) || self.activation_epoch.is_none()
            || self.lease_detached
            || self.faulted;
        let mut offset = 0;
        while offset < output.len() {
            let live = (!cancelled)
                .then(|| {
                    self.live_batch
                        .as_ref()
                        .and_then(|batch| batch.events().get(self.live_consumed))
                        .copied()
                })
                .flatten();
            let key = self
                .capture
                .as_ref()
                .and_then(|capture| {
                    capture
                        .key_batch
                        .as_ref()
                        .and_then(|batch| batch.events.get(capture.keys_consumed))
                })
                .copied()
                .flatten();
            let choose_key = key.is_some()
                && (live.is_none()
                    || self
                        .capture
                        .as_ref()
                        .is_some_and(|capture| capture.scratch_next_keys));
            let (live_event, key_event) = if choose_key {
                (None, key)
            } else {
                (live, None)
            };
            let has_event = live_event.is_some() || key_event.is_some();
            let end = if has_event { offset + 1 } else { output.len() };
            let shifted = clock.map(|clock| OffsetClock { clock, offset });
            // A scoped hardware fault releases only hardware voices. Key overflow
            // seals its own activation and cannot cancel the hardware root.
            let live_event = if cancelled && offset == 0 {
                Some(LiveNoteEvent::AllNotesOff)
            } else {
                live_event
            };
            self.render_voice_span(
                &mut output[offset..end],
                shifted
                    .as_ref()
                    .map(|clock| clock as &dyn hex_lattice::NativeInstrumentClock),
                resources,
                live_event,
                key_event,
            )?;
            if has_event {
                self.live_consumed += usize::from(!choose_key);
                if let Some(capture) = &mut self.capture {
                    capture.keys_consumed += usize::from(choose_key);
                    capture.scratch_next_keys = !choose_key;
                }
            }
            offset = end;
        }
        Ok(())
    }

    pub(super) fn apply_key_event(
        &mut self,
        event: ordered_keys::KeyEvent,
        events: &mut [NativeInstrumentEvent; 259],
        len: &mut usize,
    ) -> Result<(), Invalid> {
        // Full-word identity owns a physical voice; an old Off cannot release
        // a later press of the same key, even if both arrived in one quantum.
        for voice in self.scratch.iter_mut().flatten() {
            if voice.source == VoiceSource::Physical
                && voice.key == event.key
                && voice.gate.is_none()
                && (event.on || voice.token >> 16 == event.word >> 16)
            {
                voice.gate = Some(voice.age);
                events[*len] = NativeInstrumentEvent::NoteOff {
                    frame_offset: 0,
                    occurrence_id: voice.id,
                };
                *len += 1;
            }
        }
        if !event.on || self.scratch_rejected_desired[event.key] == event.word {
            return Ok(());
        }
        let pitch = ((event.word & 255) - 1) as u8;
        let velocity = ((event.word >> 8) & 255) as u8;
        let sampler = self.input.sampler_index(event.key, pitch, velocity)?;
        let capacity = self.voice_capacity();
        if !self.supported.get(sampler).copied().unwrap_or(false)
            || self.scratch.iter().flatten().count() >= capacity
        {
            self.input.reject();
            self.fail_capture(crate::midi_capture::MidiCaptureFault::Capacity);
            self.scratch_rejected_desired[event.key] = event.word;
            if let Some((queue, batch)) = self
                .input
                .lease()
                .and_then(|input| input.ordered_keys.as_ref())
                .zip(
                    self.capture
                        .as_ref()
                        .and_then(|capture| capture.key_batch.as_ref()),
                )
            {
                queue.fault(batch.activation);
            }
            return Ok(());
        }
        let next = self
            .scratch_next_id
            .checked_add(1)
            .ok_or(Invalid("computer keyboard identity exhausted"))?;
        let slot = self
            .scratch
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(Invalid("computer keyboard voice capacity is unavailable"))?;
        *slot = Some(Voice {
            source: VoiceSource::Physical,
            inline_override: None,
            key: event.key,
            token: event.word,
            sampler,
            age: 0,
            gate: None,
            id: next,
        });
        self.scratch_next_id = next;
        events[*len] = NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: next,
            pitch,
            velocity,
        };
        *len += 1;
        Ok(())
    }

    pub(super) fn release_hardware(
        &mut self,
        source: Option<VoiceSource>,
        events: &mut [NativeInstrumentEvent; 259],
        len: &mut usize,
    ) {
        for voice in self.scratch.iter_mut().flatten() {
            if voice.source != VoiceSource::Physical
                && voice.gate.is_none()
                && source.is_none_or(|source| source == voice.source)
            {
                voice.gate = Some(voice.age);
                events[*len] = NativeInstrumentEvent::NoteOff {
                    frame_offset: 0,
                    occurrence_id: voice.id,
                };
                *len += 1;
            }
        }
    }

    pub(super) fn apply_live_event(
        &mut self,
        event: LiveNoteEvent,
        events: &mut [NativeInstrumentEvent; 259],
        len: &mut usize,
    ) -> Result<(), Invalid> {
        let (channel, pitch, velocity) = match event {
            LiveNoteEvent::AllNotesOff => {
                self.release_hardware(None, events, len);
                return Ok(());
            }
            LiveNoteEvent::NoteOff { channel, pitch, .. } => {
                self.release_hardware(Some(VoiceSource::Hardware { channel, pitch }), events, len);
                return Ok(());
            }
            LiveNoteEvent::NoteOn {
                channel,
                pitch,
                velocity,
            } => (channel, pitch, velocity),
            LiveNoteEvent::PolyPressure {
                channel,
                pitch,
                value,
            } => {
                let supported = self.source_input.as_ref().map_or_else(
                    || {
                        self.mutable
                            .as_ref()
                            .is_some_and(NativeInstrumentRuntimePair::supports_note_pressure)
                    },
                    |source| source.note_pressure,
                );
                if supported {
                    let source = VoiceSource::Hardware { channel, pitch };
                    if let Some(occurrence_id) = self
                        .scratch
                        .iter()
                        .flatten()
                        .find(|voice| voice.source == source && voice.gate.is_none())
                        .and_then(|voice| std::num::NonZeroU64::new(voice.id))
                    {
                        let slot = events
                            .get_mut(*len)
                            .ok_or(Invalid("live note event capacity is unavailable"))?;
                        *slot = NativeInstrumentEvent::NotePressure {
                            frame_offset: 0,
                            occurrence_id,
                            value,
                        };
                        *len += 1;
                    }
                }
                return Ok(());
            }
        };
        let source = VoiceSource::Hardware { channel, pitch };
        // Repeated same-key on releases the prior occurrence before retriggering.
        self.release_hardware(Some(source), events, len);
        let sampler = (usize::from(velocity) - 1) * 128 + usize::from(pitch);
        let capacity = self.voice_capacity();
        if !self.supported.get(sampler).copied().unwrap_or(false)
            || self.scratch.iter().flatten().count() >= capacity
        {
            self.fail_capture(crate::midi_capture::MidiCaptureFault::Capacity);
            if let Some(input) = self
                .input
                .lease()
                .and_then(|input| input.live_notes.as_ref())
                && let Some(batch) = &self.live_batch
            {
                let _ = input.fail_activation(batch.activation);
            }
            self.release_hardware(None, events, len);
            return Ok(());
        }
        let slot = self
            .scratch
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(Invalid("live note voice capacity is unavailable"))?;
        self.scratch_next_id = self
            .scratch_next_id
            .checked_add(1)
            .ok_or(Invalid("live note occurrence identity exhausted"))?;
        *slot = Some(Voice {
            source,
            inline_override: None,
            key: 0,
            token: 0,
            sampler,
            age: 0,
            gate: None,
            id: self.scratch_next_id,
        });
        events[*len] = NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: self.scratch_next_id,
            pitch,
            velocity,
        };
        *len += 1;
        Ok(())
    }
}
