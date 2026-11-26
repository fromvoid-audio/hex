use super::*;

impl SamplerRuntime {
    /// Frames this voice contributes before source, gate, or optional steal
    /// fade retirement. Preflight has validated the entire enclosing span.
    pub(super) fn sounding_frames(
        &self,
        voice: &ActiveVoice,
        midi: &MidiState,
        limit: usize,
        fade_start: Option<u64>,
    ) -> Result<usize, Error> {
        let Some(note) = self.prepared.notes[usize::from(voice.pitch)] else {
            return Ok(0);
        };
        let mut count = limit as u128;
        if note.loop_offsets.is_none() {
            let end = (note.region.end - note.region.start) as u128 * ONE;
            let remaining = end.saturating_sub(voice.position);
            let step = u128::from(midi.effective_step(note.step, voice.pitch_offset_cents));
            let whole = remaining
                .checked_div(step)
                .ok_or(Error::InvalidEventBatch)?;
            let partial = remaining
                .checked_rem(step)
                .ok_or(Error::InvalidEventBatch)?
                != 0;
            count = count.min(
                whole
                    .checked_add(u128::from(partial))
                    .ok_or(Error::InvalidEventBatch)?,
            );
        }
        if note.gate == GateMode::Gate
            && let Some(gate) = voice.gate
        {
            count = count.min(u128::from(
                note.release_frames()
                    .saturating_sub(voice.age.saturating_sub(gate)),
            ));
        }
        if let Some(start) = fade_start {
            count = count.min(u128::from(
                self.prepared
                    .steal_fade_frames()
                    .saturating_sub(voice.age.saturating_sub(start)),
            ));
        }
        // count is bounded above by the original usize limit.
        Ok(count as usize)
    }

    /// Advance one voice by `frames`; `Ok(false)` means it has retired.
    pub(super) fn advance_voice(
        &self,
        voice: &mut ActiveVoice,
        midi: &MidiState,
        frames: u64,
    ) -> Result<bool, Error> {
        let Some(note) = self.prepared.notes[usize::from(voice.pitch)] else {
            return Ok(false);
        };
        voice.age = voice
            .age
            .checked_add(frames)
            .ok_or(Error::InvalidEventBatch)?;
        let effective_step = midi.effective_step(note.step, voice.pitch_offset_cents);
        let distance = u128::from(effective_step)
            .checked_mul(u128::from(frames))
            .ok_or(Error::InvalidEventBatch)?;
        voice.position = voice
            .position
            .checked_add(distance)
            .ok_or(Error::InvalidEventBatch)?;
        let source_ended = note.loop_offsets.is_none()
            && voice.position >= (note.region.end - note.region.start) as u128 * ONE;
        let release_ended = match (note.gate, voice.gate) {
            (GateMode::Gate, Some(gate)) => {
                // A looping release still needs a representable deadline,
                // as before cursor-based source exhaustion was introduced.
                if note.loop_offsets.is_some() && gate.checked_add(note.release_frames()).is_none()
                {
                    return Err(Error::InvalidEventBatch);
                }
                voice
                    .age
                    .checked_sub(gate)
                    .ok_or(Error::InvalidEventBatch)?
                    >= note.release_frames()
            }
            _ => false,
        };
        Ok(!(source_ended || release_ended))
    }
    pub(super) fn advance(
        &self,
        active: &mut Active,
        fading: &mut Fading,
        midi: &MidiState,
        frames: u64,
    ) -> Result<(), Error> {
        for entry in active.iter_mut() {
            if let Some(voice) = entry
                && !self.advance_voice(voice, midi, frames)?
            {
                *entry = None;
            }
        }
        let fade = self.prepared.steal_fade_frames();
        for entry in fading.iter_mut() {
            if let Some(fading) = entry
                && (!self.advance_voice(&mut fading.voice, midi, frames)?
                    || fading.voice.age.saturating_sub(fading.start) >= fade)
            {
                *entry = None;
            }
        }
        Ok(())
    }
    /// Move the oldest releasing voice into a free fade slot, freeing its
    /// voice slot. Changes nothing unless it returns `Steal::Stolen`.
    pub(super) fn steal(
        &self,
        active: &mut Active,
        fading: &mut Fading,
        gain_at: &dyn Fn(u64) -> f64,
    ) -> Steal {
        let mut oldest: Option<(usize, u64)> = None;
        for (index, entry) in active[..self.prepared.voices].iter().enumerate() {
            if let Some(voice) = entry
                && voice.gate.is_some()
                && oldest.is_none_or(|(_, age)| voice.age > age)
            {
                oldest = Some((index, voice.age));
            }
        }
        let Some((index, _)) = oldest else {
            return Steal::AllHeld;
        };
        let Some(target) = fading.iter().position(Option::is_none) else {
            return Steal::FadePoolFull;
        };
        let Some(voice) = active[index].take() else {
            return Steal::AllHeld;
        };
        fading[target] = Some(FadingVoice {
            voice,
            start: voice.age,
            gain: gain_at(voice.occurrence),
        });
        Steal::Stolen
    }
    pub(super) fn apply_midi_control(
        active: &mut Active,
        midi: &mut MidiState,
        event: NativeInstrumentEvent,
    ) -> Result<bool, Error> {
        match event {
            NativeInstrumentEvent::ControlChange { cc, value, .. } => {
                if cc > 127 || value > 127 {
                    return Err(Error::InvalidEventBatch);
                }
                match cc {
                    1 => midi.cc_mod_wheel = value,
                    7 => midi.cc_volume = value,
                    11 => midi.cc_expression = value,
                    64 => {
                        let down = value >= 64;
                        if midi.sustain_pedal && !down {
                            for voice in active.iter_mut().flatten() {
                                if voice.pedal_held {
                                    voice.pedal_held = false;
                                    if voice.gate.is_none() {
                                        voice.gate = Some(voice.age);
                                    }
                                }
                            }
                        }
                        midi.sustain_pedal = down;
                    }
                    74 => midi.cc_timbre = value,
                    121 => {
                        let was_pedal = midi.sustain_pedal;
                        *midi = MidiState::default();
                        if was_pedal {
                            for voice in active.iter_mut().flatten() {
                                if voice.pedal_held {
                                    voice.pedal_held = false;
                                    if voice.gate.is_none() {
                                        voice.gate = Some(voice.age);
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
                Ok(true)
            }
            NativeInstrumentEvent::PitchBend { value, .. } => {
                if !(-8192..=8191).contains(&value) {
                    return Err(Error::InvalidEventBatch);
                }
                midi.channel_pitch_bend = value;
                Ok(true)
            }
            NativeInstrumentEvent::NotePitchBend {
                occurrence_id,
                cents,
                ..
            } => {
                if !(-4_800..=4_800).contains(&cents) {
                    return Err(Error::InvalidEventBatch);
                }
                for voice in active
                    .iter_mut()
                    .flatten()
                    .filter(|v| v.occurrence == occurrence_id.get())
                {
                    voice.pitch_offset_cents = cents;
                }
                Ok(true)
            }
            NativeInstrumentEvent::NoteTimbre {
                occurrence_id,
                value,
                ..
            } => {
                if value > 127 {
                    return Err(Error::InvalidEventBatch);
                }
                for voice in active
                    .iter_mut()
                    .flatten()
                    .filter(|v| v.occurrence == occurrence_id.get())
                {
                    voice.timbre = value;
                }
                Ok(true)
            }
            NativeInstrumentEvent::Expression { expression, .. } => {
                if !expression.is_valid() {
                    return Err(Error::InvalidEventBatch);
                }
                match expression {
                    hex_lattice::NativeInstrumentExpression::PitchBend(value) => {
                        midi.channel_pitch_bend = value;
                    }
                    hex_lattice::NativeInstrumentExpression::ModWheel(value) => {
                        midi.cc_mod_wheel = value;
                    }
                    hex_lattice::NativeInstrumentExpression::ChannelPressure(value) => {
                        midi.channel_aftertouch = value;
                    }
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub(super) fn event(
        &self,
        active: &mut Active,
        fading: &mut Fading,
        midi: &mut MidiState,
        event: NativeInstrumentEvent,
        gain_at: &dyn Fn(u64) -> f64,
    ) -> Result<(), Error> {
        if Self::apply_midi_control(active, midi, event)? {
            return self.advance(active, fading, midi, 0);
        }
        match event {
            NativeInstrumentEvent::NotePressure { .. } => {
                return Err(Error::UnsupportedNotePressure);
            }
            NativeInstrumentEvent::ControlChange { .. }
            | NativeInstrumentEvent::PitchBend { .. }
            | NativeInstrumentEvent::NotePitchBend { .. }
            | NativeInstrumentEvent::NoteTimbre { .. }
            | NativeInstrumentEvent::Expression { .. } => {}
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                for voice in active
                    .iter_mut()
                    .flatten()
                    .filter(|voice| voice.occurrence == occurrence_id)
                {
                    if midi.sustain_pedal {
                        if voice.gate.is_none() {
                            voice.pedal_held = true;
                        }
                    } else if voice.gate.is_none() {
                        voice.gate = Some(voice.age);
                    }
                }
            }
            NativeInstrumentEvent::NoteOn {
                occurrence_id,
                pitch,
                velocity,
                ..
            } => {
                if pitch > 127
                    || velocity > 127
                    || occurrence_id == 0
                    || active
                        .iter()
                        .flatten()
                        .any(|voice| voice.occurrence == occurrence_id)
                {
                    return Err(Error::InvalidEventBatch);
                }
                if velocity == 0 || self.prepared.notes[usize::from(pitch)].is_none() {
                    return Ok(());
                }
                if !active[..self.prepared.voices].iter().any(Option::is_none) {
                    match self.steal(active, fading, gain_at) {
                        Steal::Stolen => {}
                        Steal::FadePoolFull => return self.advance(active, fading, midi, 0),
                        Steal::AllHeld => return Err(Error::EventCapacityExceeded),
                    }
                }
                let entry = active[..self.prepared.voices]
                    .iter_mut()
                    .find(|entry| entry.is_none())
                    .ok_or(Error::EventCapacityExceeded)?;
                *entry = Some(ActiveVoice {
                    occurrence: occurrence_id,
                    pitch,
                    gain: 1.0 - self.prepared.velocity_amount
                        + self.prepared.velocity_amount * f64::from(velocity) / 127.0,
                    age: 0,
                    position: 0,
                    gate: None,
                    pitch_offset_cents: 0,
                    timbre: 0,
                    pedal_held: false,
                });
            }
        }
        self.advance(active, fading, midi, 0)
    }
}
impl MutableInstrumentRuntime for SamplerRuntime {
    fn parameter_automation(&mut self) -> Option<&mut dyn InstrumentParameterRuntime> {
        self.prepared.gain_modulation.then_some(self)
    }
    fn parameter_automation_readonly(&self) -> Option<&dyn InstrumentParameterRuntime> {
        self.prepared.gain_modulation.then_some(self)
    }
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }
    fn active_occurrence_ids(&self, output: &mut [u64]) -> Result<usize, Error> {
        let count = self.active.iter().flatten().count();
        if output.len() < count {
            return Err(Error::ActiveOccurrenceCapacityExceeded);
        }
        for (slot, voice) in output[..count].iter_mut().zip(self.active.iter().flatten()) {
            *slot = voice.occurrence;
        }
        output[..count].sort_unstable();
        Ok(count)
    }
    fn supports_occurrence_gain(&self) -> bool {
        true
    }
    fn preflight_occurrence_gains(
        &self,
        events: &[NativeInstrumentEvent],
        gains: &[NativeInstrumentOccurrenceGain],
    ) -> Result<(), Error> {
        if events.len() > self.prepared.max_events
            || gains.len() > MAX_EVENTS + MAX_SAMPLER_RUNTIME_VOICES
        {
            return Err(Error::EventCapacityExceeded);
        }
        // Mark the bounded union once instead of scanning the entire event
        // batch for every gain (up to 1,024 incoming identities). The caller
        // already proved sorted unique gain IDs. This is local validation
        // scratch, not retained voice/gain state.
        let mut known = [false; MAX_EVENTS + MAX_SAMPLER_RUNTIME_VOICES];
        let mut mark = |id| {
            if let Ok(index) = gains.binary_search_by_key(&id, |gain| gain.occurrence_id()) {
                known[index] = true;
            }
        };
        for voice in self.active.iter().flatten() {
            mark(voice.occurrence);
        }
        for event in events {
            if let NativeInstrumentEvent::NoteOn { occurrence_id, .. } = event {
                mark(*occurrence_id);
            }
        }
        if !known[..gains.len()].iter().all(|known| *known) {
            return Err(Error::InvalidOccurrenceGain);
        }
        Ok(())
    }
    fn process_block_with_clock_and_occurrence_gains(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        gains: &[NativeInstrumentOccurrenceGain],
        resources: &dyn DeviceResources,
    ) {
        self.process_with_gains::<false>(events, frames, gains, resources, None, 0);
    }
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frames: usize,
        resources: &dyn DeviceResources,
    ) -> Result<(), Error> {
        if events.len() > self.prepared.max_events {
            return Err(Error::EventCapacityExceeded);
        }
        // Resolve the original slot from actual inventory, not a same-index substitute.
        if !(0..resources
            .len()
            .min(hex_lattice_contract::MAX_DEVICE_ASSET_REFERENCES))
            .filter_map(|index| resources.get(index))
            .any(|view| self.prepared.source.matches(view))
        {
            return Err(Error::ResourceUnavailable);
        }
        if events.is_empty() && self.is_quiescent() {
            return Ok(());
        }
        let mut active = self.active;
        let mut fading = self.fading;
        let mut midi = self.midi;
        let mut cursor = 0;
        // Gain values never affect admission; only processing freezes them.
        let unity = |_| 1.0;
        for &event in events {
            let offset = event.frame_offset();
            if offset < cursor || offset >= frames {
                return Err(Error::InvalidEventBatch);
            }
            self.advance(&mut active, &mut fading, &midi, (offset - cursor) as u64)?;
            self.event(&mut active, &mut fading, &mut midi, event, &unity)?;
            cursor = offset;
        }
        self.advance(&mut active, &mut fading, &midi, (frames - cursor) as u64)
    }
    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        resources: &dyn DeviceResources,
    ) {
        self.process_with_gains::<false>(events, frames, &[], resources, None, 0);
    }
    fn reset(&mut self) {
        self.active.fill(None);
        self.fading.fill(None);
        self.midi = MidiState::default();
        self.last_targets = self.prepared.base_targets();
    }
    fn is_quiescent(&self) -> bool {
        self.active.iter().all(Option::is_none) && self.fading.iter().all(Option::is_none)
    }
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| self.prepared == previous.prepared)
    }
    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(previous) = previous
            .as_any()
            .downcast_ref::<Self>()
            .filter(|previous| self.prepared == previous.prepared)
        {
            self.active = previous.active;
            self.fading = previous.fading;
            self.midi = previous.midi;
            self.last_targets = previous.last_targets;
        }
    }
    fn can_retarget_history_from(
        &self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> bool {
        if previous.type_id() != TypeId::of::<Self>()
            || PreparedSamplerRuntime::history_retarget_inline_bytes(proof).is_none()
        {
            return false;
        }
        let (Some(previous), Some(proof)) = (
            previous.as_any().downcast_ref::<Self>(),
            proof.as_any().downcast_ref::<SamplerHistoryRetarget>(),
        ) else {
            return false;
        };
        self.prepared.configuration == proof.0
            && self.prepared.can_retarget_from(&previous.prepared)
    }
    fn retarget_history_from(
        &mut self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) {
        // Gain/pan and ADS use the carried envelope age/gate. Gated tuning
        // uses the carried source cursor and the candidate step from now on.
        // Authored pitch, effective coordinates, velocity and release stay exact.
        if self.can_retarget_history_from(previous, proof)
            && let Some(previous) = previous.as_any().downcast_ref::<Self>()
        {
            self.active = previous.active;
            self.fading = previous.fading;
            self.midi = previous.midi;
            self.last_targets = previous.last_targets;
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl InstrumentParameterRuntime for SamplerRuntime {
    fn descriptors(&self) -> &[ParameterDescriptor] {
        &DESCRIPTORS
    }
    fn parameter_modulation_semantics(&self, parameter: ParameterId) -> Option<ParameterSemantics> {
        parameter_semantics(parameter)
    }
    fn copy_targets(&self, output: &mut [Option<ParameterValue>]) -> Result<(), Error> {
        if output.len() != PARAMETER_COUNT {
            return Err(Error::InvalidParameterAutomation);
        }
        for (slot, value) in output.iter_mut().zip(self.last_targets) {
            *slot = Some(ParameterValue::Float(value));
        }
        Ok(())
    }
    fn preflight(
        &self,
        events: &[NativeInstrumentEvent],
        frames: usize,
        _: Option<&dyn NativeInstrumentClock>,
        _: &[NativeInstrumentOccurrenceGain],
        values: &dyn NativeParameterAutomation,
        _: &dyn DeviceResources,
    ) -> Result<(), Error> {
        let mask = Self::present_mask(values);
        if (mask & DYNAMIC_CURSOR_MASK) == 0 || frames == 0 {
            return Ok(());
        }
        if frames > MAX_INSTRUMENT_AUTOMATION_FRAMES {
            return Err(Error::InvalidParameterAutomation);
        }
        if events.is_empty() && self.is_quiescent() {
            return Ok(());
        }
        let mut active = self.active;
        let mut fading = self.fading;
        let mut midi = self.midi;
        let fade = self.prepared.steal_fade_frames();
        let unity = |_| 1.0;
        let mut next = 0;
        for offset in 0..frames {
            let fm = FrameModulation::sample(&self.prepared, Some(values), mask, offset);
            while events
                .get(next)
                .is_some_and(|event| event.frame_offset() == offset)
            {
                self.event_with_modulation(
                    &mut active,
                    &mut fading,
                    &mut midi,
                    events[next],
                    &unity,
                    Some(&fm),
                )?;
                next += 1;
            }
            self.advance_dynamic_frame(&mut active, &mut fading, &midi, &fm, fade);
        }
        Ok(())
    }
    fn process(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _: Option<&dyn NativeInstrumentClock>,
        gains: &[NativeInstrumentOccurrenceGain],
        values: &dyn NativeParameterAutomation,
        resources: &dyn DeviceResources,
    ) {
        let present_mask = Self::present_mask(values);
        if present_mask == 0 {
            self.process_with_gains::<false>(events, frames, gains, resources, None, 0);
            return;
        }
        if frames.len() > MAX_INSTRUMENT_AUTOMATION_FRAMES {
            return;
        }
        self.process_with_gains::<true>(
            events,
            frames,
            gains,
            resources,
            Some(values),
            present_mask,
        );
    }
}
