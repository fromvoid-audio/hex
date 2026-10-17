//! Effective controller parameters belong to voices, never authored state.
use super::*;
pub(super) fn expression_after(
    mut values: [f32; 3],
    value: hex_lattice::NativeInstrumentExpression,
) -> Result<[f32; 3], Error> {
    use hex_lattice::NativeInstrumentExpression::*;
    if !value.is_valid() {
        return Err(Error::InvalidEventBatch);
    }
    match value {
        ChannelPressure(v) => values[0] = f32::from(v) / 127.0,
        PitchBend(v) => values[1] = f32::from(v) / if v < 0 { 8192.0 } else { 8191.0 },
        ModWheel(v) => values[2] = f32::from(v) / 127.0,
    }
    Ok(values)
}
impl Factory {
    pub(super) fn modulated_filter(
        &self,
        e: super::super::modulation::Effective,
    ) -> Result<FilterTrajectory, Error> {
        let mut settings = self.settings.filter;
        for (id, delta, octaves, min, max) in [
            (20005, e.cutoff_octaves, true, 20.0, 20000.0),
            (20006, e.resonance_delta, false, 0.0, 100.0),
            (20007, e.morph_delta, false, 0.0, 100.0),
        ] {
            if delta == 0.0 {
                continue;
            }
            let id = ParameterId::from_static(id);
            let Some(ParameterValue::Float(base)) = settings.read(id) else {
                return Err(Error::InvalidEventBatch);
            };
            let value = if octaves {
                f64::from(base) * delta.exp2()
            } else {
                f64::from(base) + delta
            };
            settings
                .patch(id, ParameterValue::Float(value.clamp(min, max) as f32))
                .map_err(|_| Error::InvalidEventBatch)?;
        }
        let mut trajectory = FilterTrajectory::oversampled(settings, self.rate)
            .map_err(|_| Error::InvalidEventBatch)?;
        trajectory.frames = (self.rate * 5 + 500) / 1000;
        Ok(trajectory)
    }
    pub(super) fn effective(
        &self,
        note: PreparedOperatorNote,
        expression: [f32; 3],
    ) -> Result<super::super::modulation::Effective, Error> {
        let (pitch, velocity) = note.midi_origin();
        self.settings
            .modulation
            .evaluate(pitch, velocity, expression)
            .map_err(|_| Error::InvalidEventBatch)
    }
}
impl Voice {
    pub(super) fn update_effective(
        &mut self,
        factory: &Factory,
        expression: [f32; 3],
    ) -> Result<(), Error> {
        let effective = factory.effective(self.note, expression)?;
        let filter = factory.modulated_filter(effective)?;
        if let Some(old) = &mut self.modulated_filter {
            old.retarget(filter);
        } else {
            self.modulated_filter = Some(filter);
        }
        let pitch_factor = (effective.pitch_semitones / 12.0).exp2();
        self.pitch_steps = PreparedOperatorConfiguration::lfo_steps(&self.note)
            .map(|step| ((step as f64 * pitch_factor).round() as u64).clamp(1, (1u64 << 63) - 1));
        let mut global = factory.settings.global;
        global.pan = (f64::from(global.pan) + effective.pan_delta).clamp(-50.0, 50.0) as f32;
        self.pan_gains = global.gains(self.note.midi_origin().0, self.sequence);
        self.lfo_rate_multiplier = effective.lfo_rate_octaves.exp2();
        self.effective = effective;
        Ok(())
    }
    pub(super) fn envelope_frame(&self) -> u64 {
        self.envelope_age.min(u64::MAX as f64) as u64
    }
}
impl Runtime {
    pub(super) fn expression(
        &mut self,
        value: hex_lattice::NativeInstrumentExpression,
    ) -> Result<(), Error> {
        if !self.factory.settings.global.active {
            return Err(Error::InvalidEventBatch);
        }
        let expression = expression_after(self.expression, value)?;
        if expression == self.expression {
            return Ok(());
        }
        for voice in &mut self.voices {
            if voice.id != 0 {
                voice.update_effective(&self.factory, expression)?;
            }
        }
        self.expression = expression;
        Ok(())
    }
}
