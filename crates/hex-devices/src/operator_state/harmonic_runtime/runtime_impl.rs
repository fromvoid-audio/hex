use super::*;

impl Runtime {
    pub(super) fn spectrum(&self) -> Spectrum {
        Spectrum::at(
            self.source,
            self.factory.target,
            f64::from(self.elapsed.min(self.frames)) / f64::from(self.frames),
        )
    }
    pub(super) fn current_parameters(&self) -> [f64; 12] {
        self.parameters.map_or_else(
            || self.factory.parameters(),
            |r| r.at(self.factory.parameters()),
        )
    }
    pub(super) fn expire(&mut self) {
        let release = self.factory.configuration.release_frames();
        for voice in &mut self.voices {
            if voice.id == 0 || voice.gate.is_none() {
                continue;
            }
            let finished = if self.factory.settings.global.active {
                voice.drain >= quality::TAIL as u32
            } else if self.factory.companion_active {
                voice.machines.iter().all(|machine| {
                    machine
                        .as_ref()
                        .is_some_and(|machine| machine.finished(voice.age))
                })
            } else {
                voice
                    .gate
                    .is_some_and(|gate| voice.age >= gate.saturating_add(release))
            };
            if finished {
                *voice = Voice::default();
            }
        }
    }
    pub(super) fn apply(
        &mut self,
        event: NativeInstrumentEvent,
        origin: u64,
        beat: Option<u64>,
        running: Option<bool>,
    ) -> Result<(), Error> {
        if self.factory.settings.global.active {
            return self.apply_global(event, origin, beat, running);
        }
        match event {
            NativeInstrumentEvent::NotePressure { .. } => {
                return Err(Error::UnsupportedNotePressure);
            }
            NativeInstrumentEvent::NotePitchBend {
                occurrence_id,
                cents,
                ..
            } => {
                if !(-4_800..=4_800).contains(&cents) {
                    return Err(Error::InvalidEventBatch);
                }
                if let Some(voice) = self.voices.iter_mut().find(|v| v.id == occurrence_id.get()) {
                    voice.pitch_offset_cents = cents;
                }
            }
            NativeInstrumentEvent::NoteTimbre {
                occurrence_id,
                value,
                ..
            } => {
                if value > 127 {
                    return Err(Error::InvalidEventBatch);
                }
                if let Some(voice) = self.voices.iter_mut().find(|v| v.id == occurrence_id.get()) {
                    voice.timbre = value;
                }
            }
            NativeInstrumentEvent::ControlChange { cc, value, .. } => {
                if cc > 127 || value > 127 {
                    return Err(Error::InvalidEventBatch);
                }
                if cc == 1 || cc == 74 {
                    self.cc_timbre = value;
                } else if cc == 121 {
                    self.cc_timbre = 0;
                    self.channel_pitch_bend = 0;
                }
            }
            NativeInstrumentEvent::PitchBend { value, .. } => {
                if !(-8192..=8191).contains(&value) {
                    return Err(Error::InvalidEventBatch);
                }
                self.channel_pitch_bend = value;
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
                let prepared = self
                    .factory
                    .prepare_voice(pitch, velocity, origin, beat, running)?;
                let capacity = self.factory.capacity();
                let voice = self.voices[..capacity]
                    .iter_mut()
                    .find(|v| v.id == 0)
                    .ok_or(Error::EventCapacityExceeded)?;
                *voice = Voice {
                    id: occurrence_id,
                    ..prepared
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
                for machine in voice.machines.iter_mut().flatten() {
                    machine.note_off(voice.age);
                }
                self.expire();
            }
        }
        Ok(())
    }
}
impl MutableInstrumentRuntime for Runtime {
    fn parameter_automation(&mut self) -> Option<&mut dyn InstrumentParameterRuntime> {
        Some(self)
    }
    fn parameter_automation_readonly(&self) -> Option<&dyn InstrumentParameterRuntime> {
        Some(self)
    }
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }
    fn supports_occurrence_gain(&self) -> bool {
        true
    }
    fn active_occurrence_ids(
        &self,
        output: &mut [u64],
    ) -> Result<usize, hex_lattice::NativeInstrumentRuntimeError> {
        super::occurrence_gain::active_ids(
            self.voices
                .iter()
                .map(|v| v.id)
                .chain(self.ledger.held.iter().map(|v| v.id)),
            output,
        )
    }
    fn preflight_occurrence_gains(
        &self,
        events: &[NativeInstrumentEvent],
        gains: &[hex_lattice::NativeInstrumentOccurrenceGain],
    ) -> Result<(), hex_lattice::NativeInstrumentRuntimeError> {
        super::occurrence_gain::preflight(
            self.voices
                .iter()
                .map(|v| v.id)
                .chain(self.ledger.held.iter().map(|v| v.id)),
            events,
            gains,
        )
    }

    fn can_import_analytic(&self, notes: &[hex_lattice::NativeInstrumentAnalyticSeed]) -> bool {
        let release = self.factory.configuration.release_frames();
        self.factory.capacity() == VOICES
            && self.filter.armed
            && !self.factory.settings.lfo.active()
            && self.factory.settings.effective_feedback() == [0.0; OPERATORS]
            && notes.len() <= VOICES
            && self.is_quiescent()
            && notes.iter().enumerate().all(|(i, seed)| {
                seed.occurrence_id != 0
                    && !notes[..i]
                        .iter()
                        .any(|other| other.occurrence_id == seed.occurrence_id)
                    && seed.note.release_frames() == release
                    && seed.age_frames < u64::MAX
                    && seed.gate_frame.is_none_or(|gate| {
                        gate <= seed.age_frames
                            && gate
                                .checked_add(release)
                                .is_some_and(|end| seed.age_frames < end)
                    })
                    && PreparedOperatorNote::from_shared_payload(seed.note.payload()).is_some_and(
                        |note| self.factory.configuration.note_matches_configuration(&note),
                    )
            })
    }

    fn import_analytic(&mut self, notes: &[hex_lattice::NativeInstrumentAnalyticSeed]) {
        if !self.can_import_analytic(notes) {
            return;
        }
        for (voice, seed) in self.voices.iter_mut().zip(notes) {
            if let Some(note) = PreparedOperatorNote::from_shared_payload(seed.note.payload()) {
                *voice = Voice {
                    note,
                    phases: PreparedOperatorConfiguration::harmonic_initial_phases(
                        &note,
                        seed.age_frames,
                    ),
                    age: seed.age_frames,
                    envelope_age: seed.age_frames as f64,
                    envelope_gate: seed.gate_frame,
                    gate: seed.gate_frame,
                    id: seed.occurrence_id,
                    ..Voice::default()
                };
            }
        }
        if self.factory.settings.global.active {
            self.seed_ledger();
            self.enforce_polyphony();
        }
        self.filter.from = self.filter.target.with_dry();
        self.filter.elapsed = 0;
    }

    fn can_import_analytic_history(
        &self,
        history: &hex_lattice::AnalyticInstrumentHistory<'_>,
    ) -> bool {
        let Some(provider) = history.source.analytic_continuation() else {
            return false;
        };
        let Some(source) = provider.as_any().downcast_ref::<PreparedOperatorContext>() else {
            return false;
        };
        let Ok(source_settings) = OperatorSettings::from_state(&source.authored_state) else {
            return false;
        };
        let base = source_settings.encode_base_prefix();
        if self.factory.rate != source.configuration.sample_rate()
            || (!history.voices.is_empty() && self.factory.settings.companion != source.companion)
            || !compatible_base(
                &self.factory.base,
                &base,
                self.factory.settings.global.active,
            )
            || !std::ptr::eq(self.factory.table, source.sine_table)
            || history.voices.len() > self.factory.capacity()
        {
            return false;
        }
        let parameters = if let Some(transition) = history.transition {
            provider.parameters_at(history.source_id, &transition, history.performance_frame)
        } else {
            Ok(provider.endpoint(history.source_id))
        };
        if parameters
            .and_then(|p| source.continuation_levels(history.source_id, &p))
            .is_err()
        {
            return false;
        }
        history.voices.iter().enumerate().all(|(index, voice)| {
            voice.occurrence_id != 0
                && !history.voices[..index]
                    .iter()
                    .any(|other| other.occurrence_id == voice.occurrence_id)
                && voice.gate_frame.is_none_or(|gate| {
                    gate <= voice.age_frames
                        && gate
                            .checked_add(voice.note.release_frames())
                            .is_some_and(|end| end > voice.age_frames)
                })
                && source.continuation_note(&voice.note).is_ok_and(|note| {
                    self.factory.retuned_note(note).is_ok_and(|target| {
                        !self.factory.settings.lfo.active()
                            || PreparedOperatorConfiguration::lfo_steps(&target)
                                .iter()
                                .all(|step| *step < (1u64 << 62))
                    })
                })
        })
    }
    fn import_analytic_history(&mut self, history: &hex_lattice::AnalyticInstrumentHistory<'_>) {
        if !self.can_import_analytic_history(history) {
            return;
        }
        let Some(provider) = history.source.analytic_continuation() else {
            return;
        };
        let Some(source) = provider.as_any().downcast_ref::<PreparedOperatorContext>() else {
            return;
        };
        let Ok(source_settings) = OperatorSettings::from_state(&source.authored_state) else {
            return;
        };
        let base = source_settings.encode_base_prefix();
        let Some(spectrum) = builtin_spectrum(&base) else {
            return;
        };
        self.voices.fill(Voice::default());
        self.local_frame = history.performance_frame;
        for (target, voice) in self.voices.iter_mut().zip(history.voices) {
            let Ok(source_note) = source.continuation_note(&voice.note) else {
                return;
            };
            // Shared analytic contexts are neutral; custom tables opt into the
            // wide mutable owner before any note is prepared.
            let note = if self.factory.tuning.is_some()
                || self.factory.settings.fine.values() != source.configuration.fine_offsets_hz()
                || self.factory.settings.pitch_keytrack.values()
                    != source.configuration.pitch_keytrack_percent()
            {
                let Ok(note) = self.factory.retuned_note(source_note) else {
                    return;
                };
                note
            } else {
                source_note
            };
            *target = Voice {
                note,
                phases: PreparedOperatorConfiguration::harmonic_initial_phases(
                    &source_note,
                    voice.age_frames,
                ),
                age: voice.age_frames,
                envelope_age: voice.age_frames as f64,
                envelope_gate: voice.gate_frame,
                gate: voice.gate_frame,
                id: voice.occurrence_id,
                ..Voice::default()
            };
        }
        if self.factory.settings.global.active {
            self.seed_ledger();
            self.enforce_polyphony();
        }
        self.filter.from = self.filter.from.with_dry();
        self.filter.elapsed = 0;
        self.source = spectrum;
        self.frames = ((u64::from(self.factory.rate) * 5 + 500) / 1000) as u32;
        self.elapsed = 0;
        let Ok(p) = source.configuration.feedback_parameters([0.0; 4]) else {
            return;
        };
        let mut target = parameter_words(p);
        target[..4].copy_from_slice(&source.effective_levels);
        self.parameters = None;
        if let Some(transition) = history.transition {
            let Ok(start) = source.continuation_levels(history.source_id, transition.start())
            else {
                return;
            };
            let mut words = target;
            words[..4].copy_from_slice(&start);
            let Ok(elapsed) = transition.elapsed_at(history.performance_frame) else {
                return;
            };
            self.parameters = Some(ParameterRamp {
                source: words,
                elapsed,
                frames: transition.duration(),
            });
        }
        if target != self.factory.parameters() {
            let current = self.parameters.map_or(target, |r| r.at(target));
            self.parameters = Some(ParameterRamp {
                source: current,
                elapsed: 0,
                frames: self.frames,
            });
        }
        if history.voices.is_empty() {
            self.source = self.factory.target;
            self.elapsed = self.frames;
            self.parameters = None;
        }
    }
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frames: usize,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), Error> {
        if events.len() > self.factory.max_events_per_block()
            || events
                .iter()
                .filter(|event| !matches!(event, NativeInstrumentEvent::Expression { .. }))
                .count()
                > 256
        {
            return Err(Error::EventCapacityExceeded);
        }
        // With no events and only held voices, no proof voice can expire or
        // affect later admission in this block. Check the same frame and age
        // overflow bounds without replaying each held voice sample by sample.
        if events.is_empty()
            && self.voices.iter().all(|voice| {
                voice.id == 0 || (voice.gate.is_none() && voice.envelope_gate.is_none())
            })
        {
            self.local_frame
                .checked_add(frames as u64)
                .ok_or(Error::InvalidEventBatch)?;
            for voice in self.voices.iter().filter(|voice| voice.id != 0) {
                voice
                    .age
                    .checked_add(frames as u64)
                    .ok_or(Error::InvalidEventBatch)?;
            }
            return Ok(());
        }
        let mut proof = EventProof::new(self);
        let mut offset = 0;
        for event in events {
            let at = event.frame_offset();
            if at < offset || at >= frames {
                return Err(Error::InvalidEventBatch);
            }
            proof.advance(at - offset)?;
            proof.apply(&self.factory, *event)?;
            if self.factory.settings.global.active {
                let mut ids = [0; VOICES];
                super::occurrence_gain::active_ids(
                    proof
                        .voices
                        .iter()
                        .map(|v| v.id)
                        .chain(proof.ledger.held.iter().map(|v| v.id)),
                    &mut ids,
                )
                .map_err(|_| Error::EventCapacityExceeded)?;
            }
            offset = at;
        }
        proof.advance(frames - offset)
    }

    fn preflight_clock(
        &self,
        clock: Option<&dyn NativeInstrumentClock>,
        frames: usize,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), Error> {
        let companion = self.factory.settings.companion;
        let beat_required = companion.needs_clock()
            || (self.factory.settings.lfo.active() && self.factory.settings.lfo.sync());
        if companion.needs_running()
            && clock
                .and_then(|clock| clock.transport_is_running())
                .is_none()
        {
            return Err(Error::InvalidEventBatch);
        }
        if beat_required {
            let clock = clock.ok_or(Error::InvalidEventBatch)?;
            let mut before = clock.beat_at_offset(0).ok_or(Error::InvalidEventBatch)?;
            let observations =
                if self.factory.settings.lfo.active() && self.factory.settings.lfo.sync() {
                    frames.checked_add(1).ok_or(Error::InvalidEventBatch)?
                } else {
                    frames
                };
            for offset in 0..observations {
                let next = clock
                    .beat_at_offset(offset)
                    .ok_or(Error::InvalidEventBatch)?;
                // Reserve a full maximum interval plus sixteenth rounding;
                // every machine's next-beat arithmetic is then safe before PCM mutation.
                if next < before || next > u64::MAX - (17u64 << 32) {
                    return Err(Error::InvalidEventBatch);
                }
                before = next;
            }
        }
        self.local_frame
            .checked_add(frames as u64)
            .ok_or(Error::InvalidEventBatch)?;
        Ok(())
    }
    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        self.process_block_with_clock(events, frames, None, _resources);
    }
    fn process_block_with_clock(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        self.process_block_with_clock_and_occurrence_gains(events, frames, clock, &[], _resources);
    }
    fn process_block_with_clock_and_occurrence_gains(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        gains: &[hex_lattice::NativeInstrumentOccurrenceGain],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        self.process_inner(events, frames, clock, gains, None, 0);
    }
    fn reset(&mut self) {
        self.voices.fill(Voice::default());
        self.fading.fill(StealFade::default());
        self.source = self.factory.target;
        self.elapsed = 1;
        self.frames = 1;
        self.parameters = None;
        self.filter = self.factory.filter;
        self.phase = (0.0, 0);
        self.local_frame = 0;
        self.ledger = Ledger::default();
        self.expression = [0.0; 3];
        self.channel_pitch_bend = 0;
        self.cc_timbre = 0;
        self.last_targets = automation::base_target_values(&self.factory.settings);
        self.tone_filter.reset();
    }
    fn is_quiescent(&self) -> bool {
        self.voices.iter().all(|v| v.id == 0)
            && self.fading.iter().all(|f| f.remaining == 0)
            && (!self.factory.settings.global.active || self.ledger.held.iter().all(|h| h.id == 0))
    }
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous.as_any().downcast_ref::<Self>().is_some_and(|p| {
            self.factory.compatible(&p.factory)
                && self.factory.settings == p.factory.settings
                && self.factory.tuning.as_ref() == p.factory.tuning.as_ref()
        })
    }
    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(p) = previous.as_any().downcast_ref::<Self>()
            && self.can_copy_history_from(p)
        {
            // Empty slots are reset wholesale on construction, expiry and
            // compaction. The legacy note-off path can still set the gate of an
            // ID-zero slot, so copy those slots too to preserve its history.
            for (target, source) in self.voices.iter_mut().zip(&p.voices) {
                if target.id != 0
                    || source.id != 0
                    || target.gate.is_some()
                    || source.gate.is_some()
                {
                    *target = *source;
                }
            }
            for (target, source) in self.fading.iter_mut().zip(&p.fading) {
                if target.remaining != 0 || source.remaining != 0 {
                    *target = *source;
                }
            }
            self.source = p.source;
            self.elapsed = p.elapsed;
            self.frames = p.frames;
            self.parameters = p.parameters;
            self.filter = p.filter;
            self.phase = p.phase;
            self.local_frame = p.local_frame;
            self.ledger = p.ledger;
            self.expression = p.expression;
            self.channel_pitch_bend = p.channel_pitch_bend;
            self.cc_timbre = p.cc_timbre;
            self.last_targets = p.last_targets;
            self.tone_filter = p.tone_filter;
        }
    }
    fn can_retarget_history_from(
        &self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> bool {
        if let Some(source) = previous.as_any().downcast_ref::<Self>()
            && source.factory.settings.global.active
            && !self.factory.settings.global.active
            && !source.is_quiescent()
        {
            return false;
        }
        let expression = previous
            .as_any()
            .downcast_ref::<Self>()
            .map_or([0.0; 3], |runtime| runtime.expression);
        proof
            .as_any()
            .downcast_ref::<Target>()
            .is_some_and(|t| t.0 == self.factory.configuration.configuration_id())
            && Admission::read(previous).is_some_and(|p| {
                self.factory.rate == p.rate
                    && (p.admission_voices().all(|(id, _)| id == 0)
                        || self.factory.settings.companion == p.companion)
                    && compatible_base(
                        &self.factory.base,
                        &p.base,
                        self.factory.settings.global.active,
                    )
                    && p.admission_voices().filter(|(id, _)| *id != 0).count()
                        <= self.factory.capacity()
                    && ((p.tuning == self.factory.tuning.as_ref()
                        && p.fine_offsets == self.factory.settings.fine.values()
                        && p.pitch_keytrack == self.factory.settings.pitch_keytrack.values())
                        || p.admission_voices()
                            .filter(|(id, _)| *id != 0)
                            .all(|(_, note)| {
                                self.factory.retuned_note(note).is_ok_and(|note| {
                                    (!self.factory.settings.lfo.active()
                                        || PreparedOperatorConfiguration::lfo_steps(&note)
                                            .iter()
                                            .all(|step| *step < (1u64 << 62)))
                                        && (!self.factory.settings.global.active
                                            || self
                                                .factory
                                                .effective(note, expression)
                                                .and_then(|e| self.factory.modulated_filter(e))
                                                .is_ok())
                                })
                            }))
                    && (!self.factory.settings.lfo.active()
                        || p.admission_voices()
                            .filter(|(id, _)| *id != 0)
                            .all(|(_, note)| {
                                PreparedOperatorConfiguration::lfo_steps(&note)
                                    .iter()
                                    .all(|step| *step < (1u64 << 62))
                            }))
            })
    }
    fn retarget_history_from(
        &mut self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) {
        if self.can_retarget_history_from(previous, proof)
            && let Some(p) = Legacy::read(previous)
        {
            self.voices.fill(Voice::default());
            self.fading.fill(StealFade::default());
            let retune = p.tuning != self.factory.tuning.as_ref()
                || p.fine_offsets != self.factory.settings.fine.values()
                || p.pitch_keytrack != self.factory.settings.pitch_keytrack.values();
            for (target, source) in self
                .voices
                .iter_mut()
                .zip(p.voices().filter(|voice| voice.id != 0))
            {
                *target = source;
                if retune {
                    let Ok(note) = self.factory.retuned_note(source.note) else {
                        return;
                    };
                    // Phase accumulators, envelopes, feedback, filters and expression
                    // remain at the actual applied boundary; only future increments change.
                    target.note = note;
                }
            }
            if let Some(old) = previous.as_any().downcast_ref::<Self>() {
                self.channel_pitch_bend = old.channel_pitch_bend;
                self.cc_timbre = old.cc_timbre;
                self.tone_filter = old.tone_filter;
                for (target, source) in self.fading.iter_mut().zip(&old.fading) {
                    if source.remaining != 0 {
                        let mut copied = *source;
                        if retune {
                            if let Ok(note) = self.factory.retuned_note(copied.voice.note) {
                                copied.voice.note = note;
                            } else {
                                continue;
                            }
                        }
                        *target = copied;
                    }
                }
            }
            if self.factory.settings.global.active {
                if let Some(old) = previous.as_any().downcast_ref::<Self>() {
                    self.ledger = old.ledger;
                    if !old.factory.settings.global.active {
                        self.seed_ledger();
                    }
                } else {
                    self.seed_ledger();
                }
                self.enforce_polyphony();
                if let Some(old) = previous.as_any().downcast_ref::<Self>() {
                    self.expression = old.expression;
                }
                for voice in &mut self.voices {
                    if voice.id != 0 {
                        let _ = voice.update_effective(&self.factory, self.expression);
                    }
                }
                for fade in &mut self.fading {
                    if fade.remaining != 0 {
                        let _ = fade.voice.update_effective(&self.factory, self.expression);
                    }
                }
            }
            self.phase = p.phase;
            self.local_frame = p.local_frame;
            self.filter = p.filter;
            self.filter.retarget(self.factory.filter);
            if self.factory.target == p.target {
                self.source = p.source;
                self.elapsed = p.elapsed;
                self.frames = p.frames;
            } else {
                self.source = Spectrum::at(
                    p.source,
                    p.target,
                    f64::from(p.elapsed.min(p.frames)) / f64::from(p.frames),
                );
                self.frames = ((u64::from(self.factory.rate) * 5 + 500) / 1000) as u32;
                self.elapsed = 0;
            }
            self.parameters = if p.parameters != self.factory.parameters() {
                Some(ParameterRamp {
                    source: p.ramp.map_or(p.parameters, |r| r.at(p.parameters)),
                    elapsed: 0,
                    frames: ((u64::from(self.factory.rate) * 5 + 500) / 1000) as u32,
                })
            } else {
                p.ramp
            };
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl InstrumentParameterRuntime for Runtime {
    fn descriptors(&self) -> &[ParameterDescriptor] {
        &automation::TARGETS
    }
    fn parameter_modulation_semantics(&self, parameter: ParameterId) -> Option<ParameterSemantics> {
        automation::parameter_semantics(parameter)
    }
    fn copy_targets(&self, output: &mut [Option<ParameterValue>]) -> Result<(), Error> {
        if output.len() != automation::TARGET_COUNT {
            return Err(Error::InvalidParameterAutomation);
        }
        for (slot, value) in output.iter_mut().zip(self.last_targets) {
            *slot = Some(ParameterValue::Float(value));
        }
        Ok(())
    }
    fn preflight(
        &self,
        _: &[NativeInstrumentEvent],
        _: usize,
        _: Option<&dyn NativeInstrumentClock>,
        _: &[NativeInstrumentOccurrenceGain],
        _: &dyn NativeParameterAutomation,
        _: &dyn DeviceResources,
    ) -> Result<(), Error> {
        Ok(())
    }
    fn process(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        gains: &[NativeInstrumentOccurrenceGain],
        values: &dyn NativeParameterAutomation,
        _: &dyn DeviceResources,
    ) {
        if frames.len() > MAX_INSTRUMENT_AUTOMATION_FRAMES {
            return;
        }
        let mut present_mask: u32 = 0;
        for &id in values.parameter_ids() {
            if let Some(index) = automation::target_index(id) {
                present_mask |= 1_u32 << index;
            }
        }
        self.process_inner(events, frames, clock, gains, Some(values), present_mask);
    }
}
