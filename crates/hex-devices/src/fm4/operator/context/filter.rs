//! Filtered carrier output is separate from the shaped FM/feedback signals.
use super::*;
use crate::operator_filter::{Carrier, Frame};
impl PreparedOperatorConfiguration {
    pub(crate) fn with_runtime_parameters(
        &self,
        p: super::super::OperatorFeedbackParameters,
    ) -> Self {
        let mut result = self.clone();
        result.configuration.levels = *p.oscillator_levels();
        result.configuration.routing.carriers = *p.carrier_weights();
        result
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn sample_filtered_carriers(
        &self,
        table: &SineTable,
        note: &PreparedOperatorNote,
        age: u64,
        gate: u64,
        phases: &[u64; 4],
        feedback: [f64; 4],
        previous: [f64; 4],
        filters: &mut [Carrier; 4],
        filter: Frame,
    ) -> (f32, [f64; 4]) {
        let mut amplitudes = self.envelopes.levels(age, gate);
        if note.amplitude_velocity_active {
            super::super::scale_operator_amplitudes(
                &mut amplitudes,
                note.amplitude_velocity_scales,
            );
        }
        let config = &self.configuration;
        let mut raw = [0.0; 4];
        let mut signals = [0.0; 4];
        let caps = unpack_caps(note.harmonic_caps);
        for &target in &config.routing.order {
            if config.levels[target] == 0.0 {
                continue;
            }
            let mut modulation: f64 = signals
                .iter()
                .enumerate()
                .map(|(source, signal)| signal * config.routing.modulation[source][target])
                .sum();
            let recurrence = previous[target] * feedback[target];
            if recurrence != 0.0 {
                modulation += recurrence;
            }
            let phase = phases[target]
                .wrapping_add(config.phases[target])
                .wrapping_add(crate::fm4::phase_from_turns(modulation));
            let oscillator = match self.waveforms[target] {
                OperatorWaveform::Sine => table.sample_phase(phase),
                waveform => self
                    .waveform_bank
                    .map_or(0.0, |bank| bank.sample_phase(waveform, caps[target], phase)),
            };
            raw[target] = f64::from(oscillator) * config.levels[target];
            signals[target] = raw[target] * amplitudes[target];
        }
        let mut output = 0.0;
        for i in 0..4 {
            let carrier = filters[i].process(raw[i], filter) * amplitudes[i];
            output += carrier * config.routing.carriers[i];
        }
        ((output * note.velocity) as f32, signals)
    }
}
