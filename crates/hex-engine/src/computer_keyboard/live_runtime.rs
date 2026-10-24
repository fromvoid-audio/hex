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
        let cancelled = self
            .input
            .lease()
            .and_then(|input| input.live_notes.as_ref())
            .is_none_or(|live| {
                live.is_cancelled()
                    || self
                        .live_batch
                        .as_ref()
                        .is_none_or(|batch| !live.same_current_activation(batch.activation))
            })
            || self.input.lease().is_none_or(|input| {
                Some(input.live_output_epoch.load(Acquire)) != self.activation_epoch
            });
        if cancelled || self.activation_epoch.is_none() || self.lease_detached || self.faulted {
            return self.render_voice_span(
                output,
                clock,
                resources,
                Some(LiveNoteEvent::AllNotesOff),
            );
        }
        let mut offset = 0;
        while offset < output.len() {
            let event = self
                .live_batch
                .as_ref()
                .and_then(|batch| batch.events().get(self.live_consumed))
                .copied();
            let end = if event.is_some() {
                offset + 1
            } else {
                output.len()
            };
            let shifted = clock.map(|clock| OffsetClock { clock, offset });
            self.render_voice_span(
                &mut output[offset..end],
                shifted
                    .as_ref()
                    .map(|clock| clock as &dyn hex_lattice::NativeInstrumentClock),
                resources,
                event,
            )?;
            self.live_consumed += usize::from(event.is_some());
            offset = end;
        }
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
        };
        let source = VoiceSource::Hardware { channel, pitch };
        // Repeated same-key on releases the prior occurrence before retriggering.
        self.release_hardware(Some(source), events, len);
        let sampler = (usize::from(velocity) - 1) * 128 + usize::from(pitch);
        let capacity = self
            .mutable
            .as_ref()
            .map_or(VOICES, NativeInstrumentRuntimePair::max_active_occurrences);
        if !self.supported.get(sampler).copied().unwrap_or(false)
            || self.scratch.iter().flatten().count() >= capacity
        {
            if let Some(input) = self
                .input
                .lease()
                .and_then(|input| input.live_notes.as_ref())
            {
                if let Some(batch) = &self.live_batch {
                    let _ = input.fail_activation(batch.activation);
                }
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
