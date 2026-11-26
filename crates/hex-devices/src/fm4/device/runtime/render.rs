use super::*;

impl Runtime {
    pub(super) fn new(config: Config) -> Self {
        Self {
            config,
            voices: [Voice::default(); VOICES],
            controls: config.base.routing.carriers.map(|value| Control {
                current: value,
                target: value,
                remaining: 0,
            }),
            levels: config.full.levels,
            phase_targets: config.authored_phases,
            edge_values_selected: false,
            effective: config.base,
            unclamped_pressure_target: None,
            channel_pitch_bend: 0,
            cc_timbre: 0,
            cold: true,
        }
    }
    pub(super) fn apply(
        config: &Config,
        voices: &mut [Voice; VOICES],
        channel_pitch_bend: &mut i16,
        cc_timbre: &mut u8,
        event: NativeInstrumentEvent,
    ) -> Result<(), Error> {
        match event {
            NativeInstrumentEvent::NoteOn {
                occurrence_id,
                pitch,
                velocity,
                ..
            } => {
                let Some(base_steps) = config.steps.get(usize::from(pitch)).copied().flatten()
                else {
                    return Err(Error::InvalidEventBatch);
                };
                if !(1..=127).contains(&velocity)
                    || occurrence_id == 0
                    || voices.iter().any(|voice| voice.id == occurrence_id)
                {
                    return Err(Error::InvalidEventBatch);
                }
                let voice = voices
                    .iter_mut()
                    .find(|voice| voice.id == 0)
                    .ok_or(Error::EventCapacityExceeded)?;
                *voice = Voice {
                    id: occurrence_id,
                    pitch,
                    velocity,
                    age: 0,
                    gate: None,
                    phase_offsets: [0; 4],
                    effective_steps: Voice::bent_steps(base_steps, 0, *channel_pitch_bend),
                    release_ceiling: config.envelope.release,
                    pressure: 0,
                    pitch_offset_cents: 0,
                    timbre: 0,
                };
            }
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                if let Some(voice) = voices.iter_mut().find(|voice| voice.id == occurrence_id) {
                    if voice.gate.is_none() {
                        voice.gate = Some(voice.age);
                    }
                    voice.advance(0);
                }
            }
            NativeInstrumentEvent::NotePressure {
                occurrence_id,
                value,
                ..
            } => {
                if !config.accepts_note_pressure() {
                    return Err(Error::UnsupportedNotePressure);
                }
                let voice = voices
                    .iter_mut()
                    .find(|voice| voice.id == occurrence_id.get() && voice.gate.is_none())
                    .ok_or(Error::InvalidEventBatch)?;
                voice.pressure = value;
            }
            NativeInstrumentEvent::NotePitchBend {
                occurrence_id,
                cents,
                ..
            } => {
                if !(-4_800..=4_800).contains(&cents) {
                    return Err(Error::InvalidEventBatch);
                }
                if let Some(voice) = voices
                    .iter_mut()
                    .find(|voice| voice.id == occurrence_id.get())
                    && voice.pitch_offset_cents != cents
                {
                    voice.pitch_offset_cents = cents;
                    if let Some(base_steps) = config.steps[usize::from(voice.pitch)] {
                        voice.retarget_steps(base_steps, *channel_pitch_bend);
                    }
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
                if let Some(voice) = voices
                    .iter_mut()
                    .find(|voice| voice.id == occurrence_id.get())
                {
                    voice.timbre = value;
                }
            }
            NativeInstrumentEvent::ControlChange { cc, value, .. } => {
                if cc > 127 || value > 127 {
                    return Err(Error::InvalidEventBatch);
                }
                if cc == 1 || cc == 74 {
                    *cc_timbre = value;
                } else if cc == 121 {
                    *cc_timbre = 0;
                    if *channel_pitch_bend != 0 {
                        *channel_pitch_bend = 0;
                        Self::retarget_all_voice_steps(config, voices, 0);
                    }
                }
            }
            NativeInstrumentEvent::PitchBend { value, .. } => {
                if !(-8192..=8191).contains(&value) {
                    return Err(Error::InvalidEventBatch);
                }
                if *channel_pitch_bend != value {
                    *channel_pitch_bend = value;
                    Self::retarget_all_voice_steps(config, voices, value);
                }
            }
            NativeInstrumentEvent::Expression { expression, .. } => match expression {
                hex_lattice::NativeInstrumentExpression::PitchBend(value) => {
                    if *channel_pitch_bend != value {
                        *channel_pitch_bend = value;
                        Self::retarget_all_voice_steps(config, voices, value);
                    }
                }
                hex_lattice::NativeInstrumentExpression::ModWheel(value) => {
                    *cc_timbre = value;
                }
                hex_lattice::NativeInstrumentExpression::ChannelPressure(_) => {}
            },
        }
        Ok(())
    }
    pub(super) fn retarget_all_voice_steps(
        config: &Config,
        voices: &mut [Voice; VOICES],
        channel_pitch_bend: i16,
    ) {
        for voice in voices.iter_mut().filter(|voice| voice.id != 0) {
            if let Some(base_steps) = config.steps[usize::from(voice.pitch)] {
                voice.retarget_steps(base_steps, channel_pitch_bend);
            }
        }
    }
    pub(super) fn set_controls(&mut self, levels: [f64; 4], carriers: [f64; 4]) {
        self.levels = levels;
        self.effective = self.config.full;
        self.effective.levels = levels;
        self.effective.routing.carriers = carriers;
        let audible = audible_operator_ancestors(&self.effective.levels, &self.effective.routing);
        for (index, audible) in audible.into_iter().enumerate() {
            if !audible {
                self.effective.levels[index] = 0.0;
            }
        }
    }
    pub(super) fn active_slots(&self, slots: &mut [usize; VOICES]) -> usize {
        let mut count = 0;
        for (slot, (index, _)) in slots.iter_mut().zip(
            self.voices
                .iter()
                .enumerate()
                .filter(|(_, voice)| voice.id != 0),
        ) {
            *slot = index;
            count += 1;
        }
        count
    }
    pub(super) fn inherit_voices(&mut self, previous: &Self) {
        self.voices = previous.voices;
        self.channel_pitch_bend = previous.channel_pitch_bend;
        self.cc_timbre = previous.cc_timbre;
        self.unclamped_pressure_target = None;
        if self.config.same_pressure_source(&previous.config) {
            self.unclamped_pressure_target =
                previous.unclamped_pressure_target.filter(|(id, _)| {
                    self.config
                        .pressure
                        .is_some_and(|route| route.route.descriptor().id == *id)
                });
        } else {
            for voice in &mut self.voices {
                voice.pressure = 0;
            }
        }
    }
    pub(super) fn has_steady_base_carriers(&self) -> bool {
        self.controls
            .iter()
            .zip(self.config.base.routing.carriers)
            .zip(self.effective.routing.carriers)
            .all(|((control, base), effective)| {
                control.remaining == 0
                    && control.current.to_bits() == base.to_bits()
                    && control.target.to_bits() == base.to_bits()
                    && effective.to_bits() == base.to_bits()
            })
    }
    pub(super) fn render(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        gains: &[NativeInstrumentOccurrenceGain],
        values: Option<&dyn NativeParameterAutomation>,
    ) {
        if let Some(pressure) = self.config.pressure {
            self.render_block::<true>(events, frames, gains, values, Some(pressure));
        } else {
            self.render_block::<false>(events, frames, gains, values, None);
        }
    }

    pub(super) fn render_block<const PRESSURE: bool>(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        gains: &[NativeInstrumentOccurrenceGain],
        values: Option<&dyn NativeParameterAutomation>,
        pressure: Option<PressureRoute>,
    ) {
        let pressure = if PRESSURE { pressure } else { None };
        let mut next_event = 0;
        let mut slots = [0; VOICES];
        let mut active_count = self.active_slots(&mut slots);
        let steady_base = values.is_none()
            && self.levels == self.config.full.levels
            && self.has_steady_base_carriers();
        let level_lanes = LEVELS.map(|descriptor| {
            values
                .is_some_and(|values| values.parameter_ids().binary_search(&descriptor.id).is_ok())
        });
        let level_values = values.filter(|_| level_lanes.contains(&true));
        let phase_values = values.filter(|values| {
            PHASES
                .iter()
                .any(|descriptor| values.parameter_ids().binary_search(&descriptor.id).is_ok())
        });
        if phase_values.is_none() {
            self.phase_targets = self.config.authored_phases;
            self.effective.phases = self.config.full.phases;
        }
        let mut edge_lanes = [(ParameterId::from_static(0), 0, 0); EDGES.len()];
        let mut edge_lane_count = 0;
        if let Some(values) = values {
            let active = values.parameter_ids().iter().filter_map(|&id| {
                let (source, target) = modulation_edge(id)?;
                (self.config.full.routing.modulation[source][target] != 0.0)
                    .then_some((id, source, target))
            });
            for (slot, lane) in edge_lanes.iter_mut().zip(active) {
                *slot = lane;
                edge_lane_count += 1;
            }
        }
        if self.edge_values_selected {
            self.effective.routing.modulation = self.config.full.routing.modulation;
        }
        self.edge_values_selected = edge_lane_count != 0;
        let edge_values = values.filter(|_| self.edge_values_selected);
        let modulated = CARRIERS.map(|descriptor| {
            values.is_some_and(|values| values.has_sample_rate_modulation(descriptor.id))
        });
        for (offset, frame) in frames.iter_mut().enumerate() {
            if !steady_base {
                let carriers = std::array::from_fn(|index| {
                    let target = match values
                        .and_then(|values| values.value_at(CARRIERS[index].id, offset))
                    {
                        Some(ParameterValue::Float(value)) => f64::from(value),
                        _ => self.config.base.routing.carriers[index],
                    };
                    self.controls[index].sample(
                        target,
                        self.config.smoothing,
                        self.cold || modulated[index],
                    )
                });
                let levels = level_values.map_or(self.config.full.levels, |values| {
                    std::array::from_fn(|index| match values.value_at(LEVELS[index].id, offset) {
                        Some(ParameterValue::Float(value)) => f64::from(value),
                        _ => self.config.full.levels[index],
                    })
                });
                if levels != self.levels || carriers != self.effective.routing.carriers {
                    self.set_controls(levels, carriers);
                }
            }
            if let Some(values) = phase_values {
                self.phase_targets = self.config.authored_phases;
                self.effective.phases = self.config.full.phases;
                for (index, descriptor) in PHASES.into_iter().enumerate() {
                    if let Some(ParameterValue::Float(turns)) =
                        values.value_at(descriptor.id, offset)
                    {
                        self.phase_targets[index] = turns;
                        self.effective.phases[index] = phase_from_turns(f64::from(turns));
                    }
                }
            }
            if let Some(values) = edge_values {
                for &(id, source, target) in &edge_lanes[..edge_lane_count] {
                    self.effective.routing.modulation[source][target] =
                        match values.value_at(id, offset) {
                            Some(ParameterValue::Float(value)) => f64::from(value),
                            _ => self.config.full.routing.modulation[source][target],
                        };
                }
            }
            self.cold = false;
            let first_event = next_event;
            while let Some(event) = events
                .get(next_event)
                .filter(|event| event.frame_offset() == offset)
            {
                // Complete preflight proved this infallible for this exact batch.
                let _ = Self::apply(
                    &self.config,
                    &mut self.voices,
                    &mut self.channel_pitch_bend,
                    &mut self.cc_timbre,
                    *event,
                );
                next_event += 1;
            }
            if next_event != first_event {
                active_count = self.active_slots(&mut slots);
            }
            let unclamped = pressure.and_then(|pressure| {
                let id = pressure.route.descriptor().id;
                values
                    .and_then(|values| values.normalized_before_clamp(id, offset))
                    .map(|value| (id, value))
            });
            if PRESSURE {
                self.unclamped_pressure_target = unclamped;
            }
            let unclamped = unclamped.map(|(_, value)| value);
            let mut sum = 0.0_f64;
            for &slot in &slots[..active_count] {
                // Collected from this fixed array; no caller-supplied index.
                let Some(voice) = self.voices.get_mut(slot) else {
                    continue;
                };
                if voice.id == 0 {
                    continue;
                }
                let steps = voice.effective_steps;
                let timbre_amount = voice.timbre.max(self.cc_timbre);
                let mut envelope = self.config.envelope;
                envelope.release = voice.release_ceiling;
                let amplitude = envelope.level(voice.age, voice.gate.unwrap_or(u64::MAX))
                    * (f64::from(voice.velocity) / 127.0);
                let sample = if amplitude == 0.0 {
                    0.0
                } else {
                    let override_carrier = pressure.and_then(|pressure| {
                        let base = self.effective.routing.carriers[pressure.index];
                        let canonical = ParameterValue::Float(base as f32);
                        match pressure.route.compose(canonical, unclamped, voice.pressure) {
                            Ok(ParameterValue::Float(value)) if value != base as f32 => {
                                Some((pressure, base, f64::from(value)))
                            }
                            _ => None,
                        }
                    });
                    let output = if voice.phase_offsets == [0; 4]
                        && override_carrier.is_none()
                        && timbre_amount == 0
                    {
                        self.effective.routed_output::<false>(
                            self.config.table,
                            voice.age,
                            &steps,
                            [1.0; 4],
                        )
                    } else {
                        let mut configuration = self.effective;
                        if let Some((pressure, base, value)) = override_carrier {
                            // Restore only the ancestors of a newly audible carrier.
                            if base == 0.0 {
                                for (index, required) in pressure.required.into_iter().enumerate() {
                                    if required {
                                        configuration.levels[index] = self.levels[index];
                                    }
                                }
                            }
                            configuration.routing.carriers[pressure.index] = value;
                        }
                        if timbre_amount != 0 {
                            let boost = 1.0 + 0.5 * (f64::from(timbre_amount) / 127.0);
                            for (level, carrier) in configuration
                                .levels
                                .iter_mut()
                                .zip(configuration.routing.carriers)
                            {
                                if carrier == 0.0 {
                                    *level *= boost;
                                }
                            }
                        }
                        for (phase, offset) in
                            configuration.phases.iter_mut().zip(voice.phase_offsets)
                        {
                            *phase = phase.wrapping_add(offset);
                        }
                        configuration.routed_output::<false>(
                            self.config.table,
                            voice.age,
                            &steps,
                            [1.0; 4],
                        )
                    };
                    (output * amplitude) as f32
                };
                let gain = if gains.is_empty() {
                    1.0
                } else {
                    gains
                        .binary_search_by_key(&voice.id, |gain| gain.occurrence_id())
                        .ok()
                        .map_or(1.0, |index| gains[index].value_at_offset(offset as u64))
                };
                sum += f64::from(sample) * gain;
                voice.advance(1);
            }
            frame[0] += sum as f32;
            frame[1] += sum as f32;
        }
    }
}
