use super::*;

impl PreparedMutableInstrumentRuntime for Factory {
    fn supports_note_pressure(&self) -> bool {
        self.config.accepts_note_pressure()
    }
    fn parameter_automation_descriptors(&self) -> &[ParameterDescriptor] {
        &TARGETS
    }
    fn parameter_modulation_semantics(
        &self,
        parameter: ParameterId,
    ) -> Option<hex_lattice_contract::ParameterSemantics> {
        self.config.modulation_semantics(parameter)
    }
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }
    fn supports_occurrence_gain(&self) -> bool {
        true
    }
    fn resource_factory_inline_bytes(&self) -> Option<usize> {
        Some(size_of::<Self>())
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn runtime_version(&self) -> u32 {
        3
    }
    fn prepare_history_retarget_target(
        &self,
    ) -> Result<InstrumentHistoryRetargetPreparation, DeviceContractError> {
        Ok(InstrumentHistoryRetargetPreparation::Prepared(Box::new(
            Retarget {
                identity: self.config.identity,
                tuning: self.config.tuning,
                sample_rate: self.config.sample_rate,
            },
        )))
    }
    fn declared_twin_bytes(&self) -> usize {
        size_of::<Runtime>()
    }
    fn max_events_per_block(&self) -> usize {
        EVENTS
    }
    fn max_active_occurrences(&self) -> usize {
        VOICES
    }
    fn max_release_tail_frames(&self) -> u64 {
        self.config.envelope.release
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(Runtime::new(self.config))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl MutableInstrumentRuntime for Runtime {
    fn supports_note_pressure(&self) -> bool {
        self.config.accepts_note_pressure()
    }
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
    fn active_occurrence_ids(&self, output: &mut [u64]) -> Result<usize, Error> {
        let count = self.voices.iter().filter(|voice| voice.id != 0).count();
        if count > output.len() {
            return Err(Error::ActiveOccurrenceCapacityExceeded);
        }
        for (slot, voice) in output
            .iter_mut()
            .zip(self.voices.iter().filter(|voice| voice.id != 0))
        {
            *slot = voice.id;
        }
        output[..count].sort_unstable();
        Ok(count)
    }
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
        _: &dyn DeviceResources,
    ) -> Result<(), Error> {
        if events.len() > EVENTS {
            return Err(Error::EventCapacityExceeded);
        }
        let mut voices = self.voices;
        let mut channel_pitch_bend = self.channel_pitch_bend;
        let mut cc_timbre = self.cc_timbre;
        let mut previous = 0;
        for event in events {
            let offset = event.frame_offset();
            if offset < previous || offset >= frame_count {
                return Err(Error::InvalidEventBatch);
            }
            for voice in &mut voices {
                voice.advance((offset - previous) as u64);
            }
            Self::apply(
                &self.config,
                &mut voices,
                &mut channel_pitch_bend,
                &mut cc_timbre,
                *event,
            )?;
            previous = offset;
        }
        Ok(())
    }
    fn preflight_occurrence_gains(
        &self,
        events: &[NativeInstrumentEvent],
        gains: &[NativeInstrumentOccurrenceGain],
    ) -> Result<(), Error> {
        if gains.iter().any(|gain| !self.voices.iter().any(|voice| voice.id == gain.occurrence_id()) && !events.iter().any(|event| matches!(event, NativeInstrumentEvent::NoteOn { occurrence_id, .. } if *occurrence_id == gain.occurrence_id()))) { return Err(Error::InvalidOccurrenceGain); }
        Ok(())
    }
    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _: &dyn DeviceResources,
    ) {
        self.render(events, frames, &[], None);
    }
    fn process_block_with_clock_and_occurrence_gains(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _: Option<&dyn NativeInstrumentClock>,
        gains: &[NativeInstrumentOccurrenceGain],
        _: &dyn DeviceResources,
    ) {
        self.render(events, frames, gains, None);
    }
    fn reset(&mut self) {
        *self = Self::new(self.config);
    }
    fn is_quiescent(&self) -> bool {
        self.voices.iter().all(|voice| voice.id == 0)
    }
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| self.config.same_target(&previous.config))
    }
    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.inherit_voices(previous);
            self.controls = previous.controls;
            self.levels = previous.levels;
            self.phase_targets = previous.phase_targets;
            self.edge_values_selected = previous.edge_values_selected;
            self.effective = previous.effective;
            self.cold = previous.cold;
        }
    }
    fn can_retarget_history_from(
        &self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> bool {
        let (Some(previous), Some(proof)) = (
            previous.as_any().downcast_ref::<Self>(),
            proof.as_any().downcast_ref::<Retarget>(),
        ) else {
            return false;
        };
        (self.config.identity == proof.identity
            && self.config.tuning == proof.tuning
            && self.config.sample_rate == proof.sample_rate)
            && self.config.sample_rate == previous.config.sample_rate
            && previous
                .voices
                .iter()
                .filter(|voice| voice.id != 0)
                .all(|voice| {
                    self.config
                        .steps
                        .get(usize::from(voice.pitch))
                        .is_some_and(Option::is_some)
                })
    }
    fn retarget_history_from(
        &mut self,
        previous: &dyn MutableInstrumentRuntime,
        _: &dyn PreparedInstrumentHistoryRetarget,
    ) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.inherit_voices(previous);
            for voice in self.voices.iter_mut().filter(|voice| voice.id != 0) {
                voice.release_ceiling = voice.release_ceiling.min(self.config.envelope.release);
                voice.advance(0);
                if voice.id == 0 {
                    continue;
                }
                let Some(target) = self.config.steps[usize::from(voice.pitch)] else {
                    continue;
                };
                voice.retarget_steps(target, self.channel_pitch_bend);
            }
            self.controls = previous.controls;
            self.set_controls(
                self.config.full.levels,
                previous.controls.map(|control| control.current),
            );
            self.phase_targets = self.config.authored_phases;
            self.edge_values_selected = false;
            self.cold = previous.cold;
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
    fn unclamped_modulation_target(&self) -> Option<(ParameterId, f64)> {
        self.unclamped_pressure_target
    }

    fn copy_targets(&self, output: &mut [Option<ParameterValue>]) -> Result<(), Error> {
        if output.len() != TARGETS.len() {
            return Err(Error::InvalidParameterAutomation);
        }
        let (oscillators, routing) = output.split_at_mut(LEVELS.len() + PHASES.len());
        let (edges, carriers) = routing.split_at_mut(EDGES.len());
        for ((slots, level), phase) in oscillators
            .chunks_exact_mut(2)
            .zip(self.levels)
            .zip(self.phase_targets)
        {
            slots[0] = Some(ParameterValue::Float(level as f32));
            slots[1] = Some(ParameterValue::Float(phase));
        }
        for (slot, descriptor) in edges.iter_mut().zip(EDGES) {
            let (source, target) =
                modulation_edge(descriptor.id).ok_or(Error::InvalidParameterAutomation)?;
            *slot = Some(ParameterValue::Float(
                self.effective.routing.modulation[source][target] as f32,
            ));
        }
        for (slot, control) in carriers.iter_mut().zip(self.controls) {
            *slot = Some(ParameterValue::Float(control.target as f32));
        }
        Ok(())
    }

    fn descriptors(&self) -> &[ParameterDescriptor] {
        &TARGETS
    }
    fn parameter_modulation_semantics(
        &self,
        parameter: ParameterId,
    ) -> Option<hex_lattice_contract::ParameterSemantics> {
        self.config.modulation_semantics(parameter)
    }
    fn preflight(
        &self,
        _: &[NativeInstrumentEvent],
        frames: usize,
        _: Option<&dyn NativeInstrumentClock>,
        _: &[NativeInstrumentOccurrenceGain],
        values: &dyn NativeParameterAutomation,
        _: &dyn DeviceResources,
    ) -> Result<(), Error> {
        // Frozen views also carry zero for absent edges. Refuse any borrowed
        // value that would introduce an edge beyond the prepared topology.
        for &id in values.parameter_ids() {
            if let Some((source, target)) = modulation_edge(id)
                && self.config.full.routing.modulation[source][target] == 0.0
                && (0..frames).any(|offset| {
                    values
                        .value_at(id, offset)
                        .is_some_and(|value| value != ParameterValue::Float(0.0))
                })
            {
                return Err(Error::InvalidParameterAutomation);
            }
        }
        if let Some(pressure) = self.config.pressure {
            let id = pressure.route.descriptor().id;
            if (0..frames).any(|offset| {
                values
                    .normalized_before_clamp(id, offset)
                    .is_some_and(|v| !v.is_finite())
            }) {
                return Err(Error::InvalidParameterAutomation);
            }
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
        _: &dyn DeviceResources,
    ) {
        self.render(events, frames, gains, Some(values));
    }
}
