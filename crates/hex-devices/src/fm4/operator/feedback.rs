//! Sequential standalone Operator self-feedback voices.
use super::{PreparedOperatorConfiguration, PreparedOperatorNote};
use crate::{Invalid, SineTable, fm4::OPERATORS};

/// One stateful note. Process with the immutable configuration that prepared it.
/// This compact value owns note phase data, gate age and four feedback memories.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreparedOperatorFeedbackVoice {
    pub(super) note: PreparedOperatorNote,
    pub(super) configuration_id: u64,
    pub(super) feedback_turns: [f64; OPERATORS],
    pub(super) age_frames: u64,
    pub(super) gate_frame: Option<u64>,
    pub(super) previous_outputs: [f64; OPERATORS],
}

impl PreparedOperatorConfiguration {
    /// Prepare a sequential voice with bounded independent self-feedback.
    ///
    /// A feedback oscillator cannot also have an incoming cross-modulation
    /// edge. The returned voice owns only note, gate and recurrence state; the
    /// caller borrows this same configuration for every processed frame.
    pub fn prepare_feedback_voice(
        &self,
        pitch: u8,
        velocity: u8,
        feedback_turns: [f64; OPERATORS],
    ) -> Result<PreparedOperatorFeedbackVoice, Invalid> {
        if !self.feedback_amounts_are_valid(feedback_turns) {
            return Err(Invalid(
                "operator feedback must be finite in [0, 1] turns and cannot share an incoming cross-modulation edge",
            ));
        }
        let note = self.prepare_note(pitch, velocity)?;
        Ok(PreparedOperatorFeedbackVoice {
            note,
            configuration_id: self.configuration_id() << 1,
            feedback_turns,
            age_frames: 0,
            gate_frame: None,
            previous_outputs: [0.0; OPERATORS],
        })
    }
}

impl PreparedOperatorFeedbackVoice {
    pub(crate) fn harmonic_snapshot(&self) -> (PreparedOperatorNote, u64, Option<u64>, [f64; 4]) {
        (
            self.note,
            self.age_frames,
            self.gate_frame,
            self.previous_outputs,
        )
    }

    /// Current note-relative frame; advances once after each successful sample.
    pub const fn age_frames(&self) -> u64 {
        self.age_frames
    }

    /// The frame at which NoteOff was latched, if released.
    pub const fn gate_frame(&self) -> Option<u64> {
        self.gate_frame
    }

    pub(super) const fn configuration_id(&self) -> u64 {
        self.configuration_id >> 1
    }

    pub(super) const fn is_ramp_participant(&self) -> bool {
        self.configuration_id & 1 != 0
    }

    /// Latch NoteOff at the next frame to be rendered. A second NoteOff is an
    /// explicit error and leaves the voice unchanged.
    pub fn note_off(&mut self) -> Result<(), Invalid> {
        if self.gate_frame.is_some() {
            return Err(Invalid("operator feedback voice already received NoteOff"));
        }
        self.gate_frame = Some(self.age_frames);
        Ok(())
    }

    /// Render one frame with the prior sample's shaped oscillator outputs.
    /// Frame overflow and a mismatched prepared configuration are refused
    /// before advancing age or feedback history.
    #[inline]
    pub fn process_frame(
        &mut self,
        configuration: &PreparedOperatorConfiguration,
        table: &SineTable,
    ) -> Result<f32, Invalid> {
        if self.is_ramp_participant() {
            return Err(Invalid(
                "operator feedback voice has an active retarget ramp",
            ));
        }
        let next_age = self
            .age_frames
            .checked_add(1)
            .ok_or(Invalid("operator feedback voice age overflow"))?;
        if !configuration.note_matches_configuration(&self.note) {
            return Err(Invalid(
                "operator feedback voice does not match its prepared configuration",
            ));
        }
        let gate_frame = self.gate_frame.unwrap_or(u64::MAX);
        let (sample, outputs) = if self.feedback_turns == [0.0; OPERATORS] {
            // Preserve the existing analytic sampling route bit-for-bit.
            (
                configuration.sample(table, &self.note, self.age_frames, gate_frame),
                [0.0; OPERATORS],
            )
        } else {
            configuration.sample_with_feedback(
                table,
                &self.note,
                self.age_frames,
                gate_frame,
                self.feedback_turns,
                self.previous_outputs,
            )
        };
        self.previous_outputs = outputs;
        self.age_frames = next_age;
        Ok(sample)
    }

    /// Rebind a quiescent retarget identity after a caller has already proved
    /// exact native history compatibility. This rejects active ramp voices;
    /// their shared ramp object must be copied and rebound with them.
    pub fn rebind_after_exact_history_copy(
        &mut self,
        source: &PreparedOperatorConfiguration,
        replacement: &PreparedOperatorConfiguration,
    ) -> Result<(), Invalid> {
        if self.is_ramp_participant()
            || self.configuration_id() != source.configuration_id()
            || !source.identical_to(replacement)
            || !source.note_matches_configuration(&self.note)
            || !replacement.note_matches_configuration(&self.note)
        {
            return Err(Invalid(
                "Operator voice cannot rebind without exact inactive history compatibility",
            ));
        }
        self.configuration_id = replacement.configuration_id() << 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operator_waveform::{OperatorWaveform, harmonic_cap_index, prepared_bank};
    use crate::{
        PHASE_SCALE,
        fm4::{OperatorTuning, Oscillator, Routing},
        operator_envelope::OperatorEnvelope,
    };

    const RATE: u32 = 48_000;
    const OPERATORS: usize = 4;
    const RATIOS: [f64; OPERATORS] = [1.0, 1.5, 2.0, 0.75];
    const LEVELS: [f64; OPERATORS] = [0.7, 0.8, 0.55, 0.9];
    const PHASES: [f64; OPERATORS] = [0.125, 0.0, 0.375, 0.625];
    const CARRIERS: [f64; OPERATORS] = [0.5, -0.25, 0.75, 0.2];
    const ENVELOPE_LEVELS: [[f64; 3]; OPERATORS] = [
        [0.05, 0.9, 0.3],
        [0.2, 0.7, 0.6],
        [0.0, 1.0, 0.2],
        [0.1, 0.8, 0.5],
    ];
    const ENVELOPE_FRAMES: [[u64; 3]; OPERATORS] =
        [[3, 7, 17], [5, 0, 21], [0, 13, 9], [11, 8, 16]];
    const VELOCITY_AMOUNTS: [f32; OPERATORS] = [0.25, 0.5, 0.75, 1.0];
    const FEEDBACK: [f64; OPERATORS] = [0.17, 0.31, 0.43, 0.59];
    const VELOCITY: u8 = 101;

    fn fixture(waveforms: [OperatorWaveform; OPERATORS]) -> PreparedOperatorConfiguration {
        routed_fixture(waveforms, [[0.0; OPERATORS]; OPERATORS], CARRIERS)
    }

    fn routed_fixture(
        waveforms: [OperatorWaveform; OPERATORS],
        modulation: [[f64; OPERATORS]; OPERATORS],
        carriers: [f64; OPERATORS],
    ) -> PreparedOperatorConfiguration {
        let oscillators = std::array::from_fn(|index| {
            Oscillator::new(RATIOS[index], LEVELS[index], PHASES[index]).expect("oscillator")
        });
        let envelopes = std::array::from_fn(|index| {
            let [initial, peak, sustain] = ENVELOPE_LEVELS[index];
            let [attack, decay, release] = ENVELOPE_FRAMES[index];
            OperatorEnvelope::new(
                initial,
                peak,
                sustain,
                attack as f64 / f64::from(RATE),
                decay as f64 / f64::from(RATE),
                release as f64 / f64::from(RATE),
            )
            .expect("envelope")
        });
        let routing = Routing::new(modulation, carriers).expect("routing");
        PreparedOperatorConfiguration::new_with_tuning_and_amplitude_velocity(
            RATE,
            oscillators,
            routing,
            envelopes,
            waveforms,
            [OperatorTuning::Ratio; OPERATORS],
            VELOCITY_AMOUNTS,
        )
        .expect("configuration")
    }

    fn oracle_envelope(index: usize, age: u64, gate: u64) -> f64 {
        let [initial, peak, sustain] = ENVELOPE_LEVELS[index];
        let [attack, decay, release] = ENVELOPE_FRAMES[index];
        let held = |at: u64| {
            if attack > 0 && at < attack {
                initial + (peak - initial) * at as f64 / attack as f64
            } else if decay > 0 && at.saturating_sub(attack) < decay {
                let decay_age = at - attack;
                sustain + (peak - sustain) * 0.00001_f64.powf(decay_age as f64 / decay as f64)
            } else {
                sustain
            }
        };
        if age < gate {
            held(age)
        } else {
            let released = age - gate;
            if released >= release {
                0.0
            } else {
                held(gate) * 0.00001_f64.powf(released as f64 / release as f64)
            }
        }
    }

    fn oracle_sine(gate: u64, frames: usize) -> Vec<f32> {
        let mut previous = [0.0; OPERATORS];
        let mut output = Vec::with_capacity(frames);
        let velocity = f64::from(VELOCITY) / 127.0;
        for age in 0..frames as u64 {
            let mut current = [0.0; OPERATORS];
            for operator in 0..OPERATORS {
                let hz = 440.0 * RATIOS[operator];
                let phase = PHASES[operator]
                    + hz * age as f64 / f64::from(RATE)
                    + previous[operator] * FEEDBACK[operator];
                let normalized_velocity = f32::from(VELOCITY) / 127.0;
                let velocity_scale = 1.0 - VELOCITY_AMOUNTS[operator]
                    + VELOCITY_AMOUNTS[operator] * normalized_velocity;
                current[operator] = (std::f64::consts::TAU * phase).sin()
                    * LEVELS[operator]
                    * oracle_envelope(operator, age, gate)
                    * f64::from(velocity_scale);
            }
            let mixed: f64 = current
                .iter()
                .zip(CARRIERS)
                .map(|(signal, carrier)| signal * carrier)
                .sum();
            output.push((mixed * velocity) as f32);
            previous = current;
        }
        output
    }

    fn oracle_periodic(
        waveforms: [OperatorWaveform; OPERATORS],
        gate: u64,
        frames: usize,
        sine: &SineTable,
    ) -> Vec<f32> {
        let bank = prepared_bank().expect("prepared table bank");
        let caps: [u8; OPERATORS] = std::array::from_fn(|index| {
            if waveforms[index] == OperatorWaveform::Sine {
                0
            } else {
                let phase_step =
                    (440.0 * RATIOS[index] / f64::from(RATE) * PHASE_SCALE).round() as u64;
                harmonic_cap_index(phase_step).expect("valid harmonic cap")
            }
        });
        let mut previous = [0.0; OPERATORS];
        let mut output = Vec::with_capacity(frames);
        let velocity = f64::from(VELOCITY) / 127.0;
        for age in 0..frames as u64 {
            let mut current = [0.0; OPERATORS];
            for operator in 0..OPERATORS {
                let phase_step =
                    (440.0 * RATIOS[operator] / f64::from(RATE) * PHASE_SCALE).round() as u64;
                let phase = phase_step
                    .wrapping_mul(age)
                    .wrapping_add((PHASES[operator] * PHASE_SCALE).round() as u64)
                    .wrapping_add(
                        ((previous[operator] * FEEDBACK[operator]).rem_euclid(1.0) * PHASE_SCALE)
                            .round() as u64,
                    );
                let oscillator = match waveforms[operator] {
                    OperatorWaveform::Sine => sine.sample_phase(phase),
                    waveform => bank.sample_phase(waveform, caps[operator], phase),
                };
                let velocity_scale = 1.0 - VELOCITY_AMOUNTS[operator]
                    + VELOCITY_AMOUNTS[operator] * (f32::from(VELOCITY) / 127.0);
                current[operator] = f64::from(oscillator)
                    * LEVELS[operator]
                    * oracle_envelope(operator, age, gate)
                    * f64::from(velocity_scale);
            }
            let mixed: f64 = current
                .iter()
                .zip(CARRIERS)
                .map(|(signal, carrier)| signal * carrier)
                .sum();
            output.push((mixed * velocity) as f32);
            previous = current;
        }
        output
    }

    fn oracle_feedback_with_outgoing_cross_edge(
        modulation_depth: f64,
        gate: u64,
        frames: usize,
    ) -> Vec<f32> {
        let mut previous_source = 0.0;
        let mut output = Vec::with_capacity(frames);
        let velocity = f64::from(VELOCITY) / 127.0;
        for age in 0..frames as u64 {
            let velocity_scale = f64::from(
                1.0 - VELOCITY_AMOUNTS[0] + VELOCITY_AMOUNTS[0] * (f32::from(VELOCITY) / 127.0),
            );
            let source_phase = PHASES[0]
                + 440.0 * RATIOS[0] * age as f64 / f64::from(RATE)
                + FEEDBACK[0] * previous_source;
            let source = (std::f64::consts::TAU * source_phase).sin()
                * LEVELS[0]
                * oracle_envelope(0, age, gate)
                * velocity_scale;
            let target_phase = PHASES[1]
                + 440.0 * RATIOS[1] * age as f64 / f64::from(RATE)
                + modulation_depth * source;
            let target = (std::f64::consts::TAU * target_phase).sin()
                * LEVELS[1]
                * oracle_envelope(1, age, gate)
                * f64::from(
                    1.0 - VELOCITY_AMOUNTS[1] + VELOCITY_AMOUNTS[1] * (f32::from(VELOCITY) / 127.0),
                );
            output.push(((source * 0.5 + target * 0.75) * velocity) as f32);
            previous_source = source;
        }
        output
    }

    #[test]
    fn sine_feedback_matches_independent_full_output_oracle_across_noteoff_and_partitions() {
        let config = fixture([OperatorWaveform::Sine; OPERATORS]);
        assert!(std::mem::size_of::<PreparedOperatorFeedbackVoice>() <= 192);
        assert_eq!(std::mem::size_of::<PreparedOperatorConfiguration>(), 744);
        println!(
            "feedback voice={}B, compact note={}B, configuration={}B",
            std::mem::size_of::<PreparedOperatorFeedbackVoice>(),
            std::mem::size_of::<PreparedOperatorNote>(),
            std::mem::size_of::<PreparedOperatorConfiguration>()
        );
        for gate in [0, 4, 9, 23, 63] {
            let expected = oracle_sine(gate, 160);
            let table = SineTable::new();
            for partition in [1, 17, 64, 127, 128, 256] {
                let mut voice = config
                    .prepare_feedback_voice(69, VELOCITY, FEEDBACK)
                    .expect("feedback voice");
                let mut actual = Vec::with_capacity(expected.len());
                let mut cursor = 0;
                while cursor < expected.len() {
                    if cursor as u64 == gate {
                        voice.note_off().expect("note off");
                        assert_eq!(voice.gate_frame(), Some(gate));
                    }
                    let mut end = (cursor + partition).min(expected.len());
                    if (cursor as u64) < gate && (end as u64) > gate {
                        end = gate as usize;
                    }
                    for _ in cursor..end {
                        actual.push(voice.process_frame(&config, &table).expect("frame"));
                    }
                    cursor = end;
                }
                for (age, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
                    assert!(
                        (f64::from(*actual) - f64::from(*expected)).abs() < 0.000_15,
                        "gate={gate}, partition={partition}, age={age}: {actual} != {expected}"
                    );
                }
                if gate < expected.len() as u64 {
                    assert!(voice.note_off().is_err());
                    assert_eq!(voice.gate_frame(), Some(gate));
                }
            }
        }
    }

    #[test]
    fn periodic_feedback_uses_previous_shaped_output_and_keeps_release_bounded() {
        let waveforms = [
            OperatorWaveform::Saw,
            OperatorWaveform::Square,
            OperatorWaveform::Triangle,
            OperatorWaveform::Saw,
        ];
        let config = fixture(waveforms);
        let table = SineTable::new();
        let expected = oracle_periodic(waveforms, 31, 256, &table);
        for partition in [1, 17, 64, 127, 128, 256] {
            let mut voice = config
                .prepare_feedback_voice(69, VELOCITY, FEEDBACK)
                .expect("feedback voice");
            let mut actual = Vec::with_capacity(expected.len());
            let mut cursor = 0;
            while cursor < expected.len() {
                if cursor == 31 {
                    voice.note_off().expect("note off");
                }
                let mut end = (cursor + partition).min(expected.len());
                if cursor < 31 && end > 31 {
                    end = 31;
                }
                for _ in cursor..end {
                    actual.push(voice.process_frame(&config, &table).expect("frame"));
                }
                cursor = end;
            }
            for (age, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
                assert!(
                    (f64::from(*actual) - f64::from(*expected)).abs() < 0.000_001,
                    "partition={partition}, age={age}: {actual} != {expected}"
                );
            }
            assert!(actual[31..].iter().any(|sample| *sample != 0.0));
            assert!(
                actual[31 + config.release_frames() as usize..]
                    .iter()
                    .all(|sample| *sample == 0.0)
            );
        }
    }

    #[test]
    fn zero_feedback_is_bit_exact_to_the_existing_compact_path() {
        let mut modulation = [[0.0; OPERATORS]; OPERATORS];
        modulation[0][1] = 0.37;
        let config = routed_fixture(
            [
                OperatorWaveform::Saw,
                OperatorWaveform::Triangle,
                OperatorWaveform::Square,
                OperatorWaveform::Sine,
            ],
            modulation,
            [0.5, 0.75, 0.0, 0.0],
        );
        let table = SineTable::new();
        let mut voice = config
            .prepare_feedback_voice(69, VELOCITY, [0.0; OPERATORS])
            .expect("zero feedback voice");
        for age in 0..512_u64 {
            if age == 79 {
                voice.note_off().expect("note off");
            }
            let gate = voice.gate_frame().unwrap_or(u64::MAX);
            let expected = config.sample(&table, &voice.note, age, gate);
            let actual = voice.process_frame(&config, &table).expect("frame");
            assert_eq!(actual.to_bits(), expected.to_bits(), "age={age}");
        }
    }

    #[test]
    fn feedback_source_keeps_outgoing_dag_cross_modulation_against_independent_oracle() {
        const DEPTH: f64 = 0.37;
        let mut modulation = [[0.0; OPERATORS]; OPERATORS];
        modulation[0][1] = DEPTH;
        let config = routed_fixture(
            [OperatorWaveform::Sine; OPERATORS],
            modulation,
            [0.5, 0.75, 0.0, 0.0],
        );
        let expected = oracle_feedback_with_outgoing_cross_edge(DEPTH, 37, 192);
        let table = SineTable::new();
        for partition in [1, 17, 64, 127, 128, 256] {
            let mut voice = config
                .prepare_feedback_voice(69, VELOCITY, [FEEDBACK[0], 0.0, 0.0, 0.0])
                .expect("feedback source with outgoing cross edge");
            let mut actual = Vec::with_capacity(expected.len());
            let mut cursor = 0;
            while cursor < expected.len() {
                if cursor == 37 {
                    voice.note_off().expect("note off");
                }
                let mut end = (cursor + partition).min(expected.len());
                if cursor < 37 && end > 37 {
                    end = 37;
                }
                for _ in cursor..end {
                    actual.push(voice.process_frame(&config, &table).expect("frame"));
                }
                cursor = end;
            }
            for (age, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
                assert!(
                    (f64::from(*actual) - f64::from(*expected)).abs() < 0.000_15,
                    "partition={partition}, age={age}: {actual} != {expected}"
                );
            }
        }
    }

    #[test]
    fn feedback_domain_incoming_edges_and_age_failures_are_atomic() {
        let no_cross = fixture([OperatorWaveform::Sine; OPERATORS]);
        for invalid in [
            [f64::NAN, 0.0, 0.0, 0.0],
            [f64::INFINITY, 0.0, 0.0, 0.0],
            [-0.001, 0.0, 0.0, 0.0],
            [1.001, 0.0, 0.0, 0.0],
        ] {
            assert!(
                no_cross
                    .prepare_feedback_voice(69, VELOCITY, invalid)
                    .is_err()
            );
        }
        assert!(
            no_cross
                .prepare_feedback_voice(69, VELOCITY, [1.0; OPERATORS])
                .is_ok()
        );

        let mut modulation = [[0.0; OPERATORS]; OPERATORS];
        modulation[0][1] = 0.25;
        let oscillators = std::array::from_fn(|index| {
            Oscillator::new(RATIOS[index], LEVELS[index], PHASES[index]).expect("oscillator")
        });
        let envelopes = std::array::from_fn(|index| {
            let [initial, peak, sustain] = ENVELOPE_LEVELS[index];
            let [attack, decay, release] = ENVELOPE_FRAMES[index];
            OperatorEnvelope::new(
                initial,
                peak,
                sustain,
                attack as f64 / f64::from(RATE),
                decay as f64 / f64::from(RATE),
                release as f64 / f64::from(RATE),
            )
            .expect("envelope")
        });
        let cross_config = PreparedOperatorConfiguration::new_with_tuning_and_amplitude_velocity(
            RATE,
            oscillators,
            Routing::new(modulation, CARRIERS).expect("acyclic cross route"),
            envelopes,
            [OperatorWaveform::Sine; OPERATORS],
            [OperatorTuning::Ratio; OPERATORS],
            VELOCITY_AMOUNTS,
        )
        .expect("cross configuration");
        assert!(
            cross_config
                .prepare_feedback_voice(69, VELOCITY, [0.5, 0.0, 0.0, 0.0])
                .is_ok()
        );
        assert!(
            cross_config
                .prepare_feedback_voice(69, VELOCITY, [0.0, 0.5, 0.0, 0.0])
                .is_err()
        );

        let mut voice = no_cross
            .prepare_feedback_voice(69, VELOCITY, FEEDBACK)
            .expect("feedback voice");
        voice.age_frames = u64::MAX;
        let old_age = voice.age_frames;
        let old_gate = voice.gate_frame;
        let old_outputs = voice.previous_outputs;
        assert!(voice.process_frame(&no_cross, &SineTable::new()).is_err());
        assert_eq!(voice.age_frames, old_age);
        assert_eq!(voice.gate_frame, old_gate);
        assert_eq!(voice.previous_outputs, old_outputs);
        assert!(voice.process_frame(&no_cross, &SineTable::new()).is_err());
        assert_eq!(voice.age_frames, old_age);
        assert_eq!(voice.previous_outputs, old_outputs);
    }
}
