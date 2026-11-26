use super::*;

pub(super) const GAIN_PAN_MASK: u16 = (1 << 0) | (1 << 1);
pub(super) const PITCH_MASK: u16 = (1 << 2) | (1 << 3);
pub(super) const REGION_MASK: u16 = (1 << 4) | (1 << 5) | (1 << 6) | (1 << 7);
pub(super) const ENV_MASK: u16 = (1 << 8) | (1 << 9) | (1 << 10) | (1 << 11);
pub(super) const DYNAMIC_CURSOR_MASK: u16 =
    (1 << 2) | (1 << 3) | (1 << 4) | (1 << 5) | (1 << 6) | (1 << 7) | (1 << 11);

#[derive(Clone, Copy)]
pub(super) struct RegionModulation {
    pub(super) sample_start: Option<f32>,
    pub(super) sample_end: Option<f32>,
    pub(super) loop_start: Option<f32>,
    pub(super) loop_length: Option<f32>,
}

/// Global modulated sampler parameters evaluated once per output frame rather
/// than once per voice per frame.
#[derive(Clone, Copy)]
pub(super) struct FrameModulation {
    pub(super) gain_pan: Option<[f64; 2]>,
    pub(super) pitch_factor: Option<f64>,
    pub(super) region: Option<RegionModulation>,
    pub(super) envelope: Option<envelope::PreparedEnvelope>,
}

impl FrameModulation {
    pub(super) fn sample(
        prepared: &PreparedSamplerRuntime,
        values: Option<&dyn NativeParameterAutomation>,
        mask: u16,
        offset: usize,
    ) -> Self {
        let gain_pan = if (mask & GAIN_PAN_MASK) != 0 {
            let gain = SamplerRuntime::read_param(values, mask, GAIN, offset)
                .map_or(prepared.base_gain, |v| f64::from(v).clamp(0.0, 4.0));
            let pan = SamplerRuntime::read_param(values, mask, PAN, offset)
                .map_or(prepared.pan, |v| f64::from(v).clamp(-1.0, 1.0));
            let left = 1.0 - pan.max(0.0);
            let right = 1.0 + pan.min(0.0);
            Some([gain * left, gain * right])
        } else {
            None
        };

        let pitch_factor = if (mask & PITCH_MASK) != 0 {
            let transpose = SamplerRuntime::read_param(values, mask, TRANSPOSE, offset)
                .map_or(prepared.authored_pitch.transpose_semitones, |v| {
                    f64::from(v).clamp(-48.0, 48.0)
                });
            let detune = SamplerRuntime::read_param(values, mask, DETUNE, offset)
                .map_or(prepared.authored_pitch.detune_cents, |v| {
                    f64::from(v).clamp(-100.0, 100.0)
                });
            let delta_semitones = (transpose - prepared.authored_pitch.transpose_semitones)
                + (detune - prepared.authored_pitch.detune_cents) / 100.0;
            Some((delta_semitones / 12.0).exp2())
        } else {
            None
        };

        let region = if (mask & REGION_MASK) != 0 {
            Some(RegionModulation {
                sample_start: SamplerRuntime::read_param(values, mask, SAMPLE_START, offset),
                sample_end: SamplerRuntime::read_param(values, mask, SAMPLE_END, offset),
                loop_start: SamplerRuntime::read_param(values, mask, LOOP_START, offset),
                loop_length: SamplerRuntime::read_param(values, mask, LOOP_LENGTH, offset),
            })
        } else {
            None
        };

        let envelope = if (mask & ENV_MASK) != 0 {
            let env = Envelope {
                attack_seconds: SamplerRuntime::read_param(values, mask, ATTACK, offset)
                    .map_or(prepared.base_envelope.attack_seconds, |v| {
                        f64::from(v).clamp(0.0, 60.0)
                    }),
                decay_seconds: SamplerRuntime::read_param(values, mask, DECAY, offset)
                    .map_or(prepared.base_envelope.decay_seconds, |v| {
                        f64::from(v).clamp(0.0, 60.0)
                    }),
                sustain: SamplerRuntime::read_param(values, mask, SUSTAIN, offset)
                    .map_or(prepared.base_envelope.sustain, |v| {
                        f64::from(v).clamp(0.0, 1.0)
                    }),
                release_seconds: SamplerRuntime::read_param(values, mask, RELEASE, offset)
                    .map_or(prepared.base_envelope.release_seconds, |v| {
                        f64::from(v).clamp(0.0, 60.0)
                    }),
            };
            env.prepare(prepared.authored_pitch.output_rate)
                .ok()
                .map(|prepared_env| prepared_env.clamp_release_frames(prepared.tail))
        } else {
            None
        };

        Self {
            gain_pan,
            pitch_factor,
            region,
            envelope,
        }
    }

    pub(super) fn apply_to(
        &self,
        base: &VoiceCoefficients,
        velocity_gain: f64,
        base_channel_gain: [f64; 2],
    ) -> VoiceCoefficients {
        let mut note = *base;
        if let Some([left, right]) = self.gain_pan {
            note.channel_gain = [left * velocity_gain, right * velocity_gain];
        } else {
            note.channel_gain = base_channel_gain;
        }
        if let Some(factor) = self.pitch_factor {
            let scaled = (base.step as f64 * factor).round();
            note.step = (scaled as u64).max(1);
        }
        if let Some(RegionModulation {
            sample_start,
            sample_end,
            loop_start,
            loop_length,
        }) = self.region
        {
            let extent = base.expected_frames.max(1);
            let mut region = base.region;
            if let Some(s) = sample_start {
                region.start = ((f64::from(s).clamp(0.0, 1.0) * extent as f64).round() as usize)
                    .min(extent.saturating_sub(1));
            }
            if let Some(e) = sample_end {
                region.end = ((f64::from(e).clamp(0.0, 1.0) * extent as f64).round() as usize)
                    .clamp(1, extent);
            }
            if region.start >= region.end {
                region.start = region.end.saturating_sub(1);
            }
            note.region = region;
            let region_len = region.end.saturating_sub(region.start).max(1);
            if let Some((base_ls, base_le)) = base.loop_offsets {
                let mut ls = base_ls.min(region_len.saturating_sub(1));
                let mut loop_len = base_le.saturating_sub(base_ls).max(1);
                if let Some(s) = loop_start {
                    ls = ((f64::from(s).clamp(0.0, 1.0) * region_len as f64).round() as usize)
                        .min(region_len.saturating_sub(1));
                }
                if let Some(l) = loop_length {
                    loop_len = ((f64::from(l).clamp(0.0, 1.0) * region_len as f64).round()
                        as usize)
                        .max(1);
                }
                let le = ls.saturating_add(loop_len).min(region_len);
                let ls = ls.min(le.saturating_sub(1));
                let le = le.max(ls.saturating_add(1));
                note.loop_offsets = Some((ls, le));
                note.loop_crossfade = base.loop_crossfade.min((le - ls).min(ls));
            }
        }
        if let Some(prepared_env) = self.envelope {
            note.envelope = prepared_env;
        }
        note
    }
}

impl SamplerRuntime {
    pub(super) fn present_mask(values: &dyn NativeParameterAutomation) -> u16 {
        let mut mask: u16 = 0;
        for &id in values.parameter_ids() {
            if let Some(index) = parameter_index(id) {
                mask |= 1_u16 << index;
            }
        }
        mask
    }

    pub(super) fn read_param(
        values: Option<&dyn NativeParameterAutomation>,
        mask: u16,
        id: ParameterId,
        offset: usize,
    ) -> Option<f32> {
        let index = parameter_index(id)?;
        if (mask & (1_u16 << index)) == 0 {
            return None;
        }
        match values?.value_at(id, offset) {
            Some(ParameterValue::Float(v)) if v.is_finite() => Some(v),
            _ => None,
        }
    }

    pub(super) fn voice_retired(cursor: &ActiveVoice, note: &VoiceCoefficients) -> bool {
        let source_ended = note.loop_offsets.is_none()
            && cursor.position >= (note.region.end - note.region.start) as u128 * ONE;
        let release_ended = note.gate == GateMode::Gate
            && cursor
                .gate
                .is_some_and(|gate| cursor.age.saturating_sub(gate) >= note.release_frames());
        source_ended || release_ended
    }

    pub(super) fn retire_zero_with_modulation(
        &self,
        active: &mut Active,
        fading: &mut Fading,
        fm: &FrameModulation,
    ) {
        let fade = self.prepared.steal_fade_frames();
        for entry in active.iter_mut() {
            let Some(voice) = *entry else { continue };
            let Some(base_note) = self.prepared.notes[usize::from(voice.pitch)] else {
                *entry = None;
                continue;
            };
            let note = fm.apply_to(&base_note, voice.gain, base_note.channel_gain);
            if Self::voice_retired(&voice, &note) {
                *entry = None;
            }
        }
        for entry in fading.iter_mut() {
            let Some(item) = *entry else { continue };
            let Some(base_note) = self.prepared.notes[usize::from(item.voice.pitch)] else {
                *entry = None;
                continue;
            };
            let note = fm.apply_to(&base_note, item.voice.gain, base_note.channel_gain);
            if Self::voice_retired(&item.voice, &note)
                || item.voice.age.saturating_sub(item.start) >= fade
            {
                *entry = None;
            }
        }
    }

    pub(super) fn event_with_modulation(
        &self,
        active: &mut Active,
        fading: &mut Fading,
        event: NativeInstrumentEvent,
        gain_at: &dyn Fn(u64) -> f64,
        fm: Option<&FrameModulation>,
    ) -> Result<(), Error> {
        match event {
            NativeInstrumentEvent::NotePressure { .. } => {
                return Err(Error::UnsupportedNotePressure);
            }
            NativeInstrumentEvent::ControlChange { .. }
            | NativeInstrumentEvent::PitchBend { .. }
            | NativeInstrumentEvent::NotePitchBend { .. }
            | NativeInstrumentEvent::NoteTimbre { .. } => {}
            NativeInstrumentEvent::Expression { expression, .. } => {
                if !expression.is_valid() {
                    return Err(Error::InvalidEventBatch);
                }
            }
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                for voice in active
                    .iter_mut()
                    .flatten()
                    .filter(|voice| voice.occurrence == occurrence_id)
                {
                    if voice.gate.is_none() {
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
                        Steal::FadePoolFull => {
                            if let Some(fm) = fm {
                                self.retire_zero_with_modulation(active, fading, fm);
                                return Ok(());
                            }
                            return self.advance(active, fading, 0);
                        }
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
                });
            }
        }
        if let Some(fm) = fm {
            self.retire_zero_with_modulation(active, fading, fm);
            Ok(())
        } else {
            self.advance(active, fading, 0)
        }
    }

    pub(super) fn advance_dynamic_frame(
        &self,
        active: &mut Active,
        fading: &mut Fading,
        fm: &FrameModulation,
        fade: u64,
    ) {
        for entry in active.iter_mut() {
            let Some(mut cursor) = *entry else { continue };
            let Some(base_note) = self.prepared.notes[usize::from(cursor.pitch)] else {
                *entry = None;
                continue;
            };
            let note = fm.apply_to(&base_note, cursor.gain, base_note.channel_gain);
            if Self::voice_retired(&cursor, &note) {
                *entry = None;
                continue;
            }
            cursor.age = cursor.age.saturating_add(1);
            cursor.position = cursor.position.saturating_add(u128::from(note.step));
            *entry = (!Self::voice_retired(&cursor, &note)).then_some(cursor);
        }
        for entry in fading.iter_mut() {
            let Some(mut item) = *entry else { continue };
            let Some(base_note) = self.prepared.notes[usize::from(item.voice.pitch)] else {
                *entry = None;
                continue;
            };
            let note = fm.apply_to(&base_note, item.voice.gain, base_note.channel_gain);
            if Self::voice_retired(&item.voice, &note)
                || item.voice.age.saturating_sub(item.start) >= fade
            {
                *entry = None;
                continue;
            }
            item.voice.age = item.voice.age.saturating_add(1);
            item.voice.position = item.voice.position.saturating_add(u128::from(note.step));
            *entry = (!Self::voice_retired(&item.voice, &note)
                && item.voice.age.saturating_sub(item.start) < fade)
                .then_some(item);
        }
    }

    pub(super) fn process_with_gains<const MODULATED: bool>(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        gains: &[NativeInstrumentOccurrenceGain],
        resources: &dyn DeviceResources,
        values: Option<&dyn NativeParameterAutomation>,
        mask: u16,
    ) {
        let pcm = (0..resources
            .len()
            .min(hex_lattice_contract::MAX_DEVICE_ASSET_REFERENCES))
            .filter_map(|index| resources.get(index))
            .find(|view| self.prepared.source.matches(*view));
        let Some(view) = pcm else { return };
        if events.is_empty() && self.is_quiescent() {
            if MODULATED && !frames.is_empty() {
                self.record_last_targets(values, mask, frames.len() - 1);
            }
            return;
        }
        let mut active = self.active;
        let mut fading = self.fading;
        let fade_u64 = self.prepared.steal_fade_frames();
        let fade = fade_u64 as f64;
        let mut next = 0;

        if MODULATED {
            // Frame-major modulated pass: evaluates global parameters once per
            // sample across all active and steal-fading voices, bounds RELEASE
            // by `max_release_tail_frames()`, advances fading cursors with the
            // exact modulated `note.step`, and never aborts mid-block on voice
            // limit so subsequent NoteOffs and `self.active`/`self.fading`
            // always commit cleanly.
            for (offset, frame) in frames.iter_mut().enumerate() {
                let fm = FrameModulation::sample(&self.prepared, values, mask, offset);
                let gain_at = |occurrence: u64| {
                    gains
                        .binary_search_by_key(&occurrence, |gain| gain.occurrence_id())
                        .ok()
                        .map_or(1.0, |index| gains[index].value_at_offset(offset as u64))
                };
                while events
                    .get(next)
                    .is_some_and(|event| event.frame_offset() == offset)
                {
                    let _ = self.event_with_modulation(
                        &mut active,
                        &mut fading,
                        events[next],
                        &gain_at,
                        Some(&fm),
                    );
                    next += 1;
                }
                for entry in &mut active {
                    let Some(mut cursor) = *entry else { continue };
                    let Some(base_note) = self.prepared.notes[usize::from(cursor.pitch)] else {
                        *entry = None;
                        continue;
                    };
                    let base_gain = [
                        base_note.channel_gain[0] * cursor.gain,
                        base_note.channel_gain[1] * cursor.gain,
                    ];
                    let note = fm.apply_to(&base_note, cursor.gain, base_gain);
                    if Self::voice_retired(&cursor, &note) {
                        *entry = None;
                        continue;
                    }
                    let gain = gain_at(cursor.occurrence);
                    Self::mix_prepared(frame, view.pcm, &cursor, gain, &note);
                    cursor.age = cursor.age.saturating_add(1);
                    cursor.position = cursor.position.saturating_add(u128::from(note.step));
                    *entry = (!Self::voice_retired(&cursor, &note)).then_some(cursor);
                }
                for entry in &mut fading {
                    let Some(mut item) = *entry else { continue };
                    let Some(base_note) = self.prepared.notes[usize::from(item.voice.pitch)] else {
                        *entry = None;
                        continue;
                    };
                    let base_gain = [
                        base_note.channel_gain[0] * item.voice.gain,
                        base_note.channel_gain[1] * item.voice.gain,
                    ];
                    let note = fm.apply_to(&base_note, item.voice.gain, base_gain);
                    if Self::voice_retired(&item.voice, &note)
                        || item.voice.age.saturating_sub(item.start) >= fade_u64
                    {
                        *entry = None;
                        continue;
                    }
                    let declick = 1.0 - item.voice.age.saturating_sub(item.start) as f64 / fade;
                    Self::mix_prepared(frame, view.pcm, &item.voice, item.gain * declick, &note);
                    item.voice.age = item.voice.age.saturating_add(1);
                    item.voice.position = item.voice.position.saturating_add(u128::from(note.step));
                    *entry = (!Self::voice_retired(&item.voice, &note)
                        && item.voice.age.saturating_sub(item.start) < fade_u64)
                        .then_some(item);
                }
            }
        } else {
            let mut offset = 0;
            while offset < frames.len() {
                let gain_at = |occurrence: u64| {
                    gains
                        .binary_search_by_key(&occurrence, |gain| gain.occurrence_id())
                        .ok()
                        .map_or(1.0, |index| gains[index].value_at_offset(offset as u64))
                };
                while events
                    .get(next)
                    .is_some_and(|event| event.frame_offset() == offset)
                {
                    if self
                        .event(&mut active, &mut fading, events[next], &gain_at)
                        .is_err()
                    {
                        return;
                    }
                    next += 1;
                }
                let end = events
                    .get(next)
                    .map_or(frames.len(), |event| event.frame_offset());
                if end <= offset || end > frames.len() {
                    return;
                }
                let span = end - offset;
                for entry in &mut active {
                    let Some(voice) = *entry else { continue };
                    let Some(base_note) = self.prepared.notes[usize::from(voice.pitch)] else {
                        *entry = None;
                        continue;
                    };
                    let mut note = base_note;
                    for channel_gain in &mut note.channel_gain {
                        *channel_gain *= voice.gain;
                    }
                    let gain_index = gains
                        .binary_search_by_key(&voice.occurrence, |gain| gain.occurrence_id())
                        .ok();
                    let Ok(count) = self.sounding_frames(&voice, span, None) else {
                        return;
                    };
                    let mut final_voice = voice;
                    let Ok(alive) = self.advance_voice(&mut final_voice, span as u64) else {
                        return;
                    };
                    let mut cursor = voice;
                    for (relative, frame) in frames[offset..offset + count].iter_mut().enumerate() {
                        let gain = gain_index.map_or(1.0, |index| {
                            gains[index].value_at_offset((offset + relative) as u64)
                        });
                        Self::mix_prepared(frame, view.pcm, &cursor, gain, &note);
                        cursor.age = cursor.age.wrapping_add(1);
                        cursor.position = cursor.position.wrapping_add(u128::from(note.step));
                    }
                    *entry = alive.then_some(final_voice);
                }
                for entry in &mut fading {
                    let Some(item) = *entry else { continue };
                    let Some(base_note) = self.prepared.notes[usize::from(item.voice.pitch)] else {
                        *entry = None;
                        continue;
                    };
                    let mut note = base_note;
                    for channel_gain in &mut note.channel_gain {
                        *channel_gain *= item.voice.gain;
                    }
                    let Ok(count) = self.sounding_frames(&item.voice, span, Some(item.start))
                    else {
                        return;
                    };
                    let mut final_item = item;
                    let Ok(alive) = self.advance_voice(&mut final_item.voice, span as u64) else {
                        return;
                    };
                    let mut cursor = item.voice;
                    for frame in &mut frames[offset..offset + count] {
                        let declick = 1.0 - cursor.age.saturating_sub(item.start) as f64 / fade;
                        Self::mix_prepared(frame, view.pcm, &cursor, item.gain * declick, &note);
                        cursor.age = cursor.age.wrapping_add(1);
                        cursor.position = cursor.position.wrapping_add(u128::from(note.step));
                    }
                    *entry = (alive && final_item.voice.age.saturating_sub(item.start) < fade_u64)
                        .then_some(final_item);
                }
                offset = end;
            }
        }
        self.active = active;
        self.fading = fading;
        if !frames.is_empty() {
            if MODULATED {
                self.record_last_targets(values, mask, frames.len() - 1);
            } else {
                self.last_targets = self.prepared.base_targets();
            }
        }
    }

    pub(super) fn record_last_targets(
        &mut self,
        values: Option<&dyn NativeParameterAutomation>,
        mask: u16,
        last_offset: usize,
    ) {
        let defaults = self.prepared.base_targets();
        for (index, descriptor) in DESCRIPTORS.iter().enumerate() {
            self.last_targets[index] = Self::read_param(values, mask, descriptor.id, last_offset)
                .unwrap_or(defaults[index]);
        }
    }

    #[cfg(test)]
    pub(super) fn mix(
        &self,
        frame: &mut StereoFrame,
        pcm: hex_lattice::DeviceResourcePcm<'_>,
        voice: &ActiveVoice,
        gain: f64,
    ) {
        let Some(mut note) = self.prepared.notes[usize::from(voice.pitch)] else {
            return;
        };
        for channel_gain in &mut note.channel_gain {
            *channel_gain *= voice.gain;
        }
        Self::mix_prepared(frame, pcm, voice, gain, &note);
    }

    pub(super) fn mix_prepared(
        frame: &mut StereoFrame,
        pcm: hex_lattice::DeviceResourcePcm<'_>,
        voice: &ActiveVoice,
        gain: f64,
        note: &VoiceCoefficients,
    ) {
        let sample = note.sample_at_position(
            pcm,
            voice.age,
            voice.gate.unwrap_or(u64::MAX),
            voice.position,
        );
        for channel in 0..2 {
            frame[channel] = (f64::from(frame[channel]) + f64::from(sample[channel]) * gain)
                .clamp(-f64::from(f32::MAX), f64::from(f32::MAX))
                as f32;
        }
    }
}
