//! Prepared scalar amplitude/pan LFO; no PCM ownership or callback allocation.
use super::*;
impl SamplerRuntime {
    fn modulation_delta(
        &self,
        clock: Option<&dyn NativeInstrumentClock>,
        offset: usize,
    ) -> Option<f64> {
        let law = self.prepared.modulation;
        if !law.active() {
            return Some(0.0);
        }
        let beats = if law.sync() {
            let clock = clock?;
            let a = clock.beat_at_offset(offset)?;
            let b = clock.beat_at_offset(offset.checked_add(1)?)?;
            Some(b.checked_sub(a)? as f64 / 4_294_967_296.0)
        } else {
            None
        };
        law.delta(self.prepared.output_rate, beats)
    }
    pub(super) fn check_modulation_clock(
        &self,
        clock: Option<&dyn NativeInstrumentClock>,
        frames: usize,
    ) -> Result<(), Error> {
        let law = self.prepared.modulation;
        if !law.active() || !law.sync() {
            return Ok(());
        }
        let clock = clock.ok_or(Error::InvalidEventBatch)?;
        clock.beat_at_offset(0).ok_or(Error::InvalidEventBatch)?;
        for offset in 0..frames {
            self.modulation_delta(Some(clock), offset)
                .ok_or(Error::InvalidEventBatch)?;
        }
        Ok(())
    }
    pub(super) fn process_modulated(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        resources: &dyn DeviceResources,
    ) {
        if self.check_modulation_clock(clock, frames.len()).is_err() {
            return;
        }
        let law = self.prepared.modulation;
        let mut phase = self.modulation_phase;
        let Some(source) = self.prepared.source else {
            return;
        };
        let pcm = (0..resources
            .len()
            .min(hex_lattice_contract::MAX_DEVICE_ASSET_REFERENCES))
            .filter_map(|index| resources.get(index))
            .find(|view| source.matches(*view));
        let Some(view) = pcm else { return };
        let mut active = self.active;
        let mut next = 0;
        for (offset, frame) in frames.iter_mut().enumerate() {
            let Some(delta) = self.modulation_delta(clock, offset) else {
                return;
            };
            while events
                .get(next)
                .is_some_and(|event| event.frame_offset() == offset)
            {
                if self.event(&mut active, events[next]).is_err() {
                    return;
                }
                next += 1;
            }
            for voice in active.iter_mut().flatten() {
                let Some(mut note) = self.prepared.note(voice.pitch) else {
                    continue;
                };
                let gains = law.gains(if law.retrigger() {
                    voice.modulation_phase
                } else {
                    phase
                });
                for (channel_gain, modulation_gain) in note.channel_gain.iter_mut().zip(gains) {
                    *channel_gain *= voice.gain * modulation_gain;
                }
                if law.active() {
                    voice.modulation_phase.advance(delta);
                }
                let gate = voice.gate.unwrap_or(u64::MAX);
                let sample = if let Some(filter) = self.prepared.filter {
                    let Some(input) = note.interpolated_source(view.pcm, voice.age) else {
                        continue;
                    };
                    let filtered = std::array::from_fn(|channel| {
                        voice.filter[channel].process(input[channel], filter)
                    });
                    note.apply_amplitude(filtered, note.envelope_gain(voice.age, gate))
                } else {
                    note.sample(view.pcm, voice.age, gate)
                };
                for channel in 0..2 {
                    frame[channel] = (f64::from(frame[channel]) + f64::from(sample[channel]))
                        .clamp(-f64::from(f32::MAX), f64::from(f32::MAX))
                        as f32;
                }
            }
            if law.active() {
                phase.advance(delta);
            }
            if self.advance(&mut active, 1).is_err() {
                return;
            }
        }
        self.active = active;
        self.modulation_phase = phase;
    }
}
