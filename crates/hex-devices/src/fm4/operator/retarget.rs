//! Control-prepared shared ramps for supported mutable Operator parameters.
use super::{PreparedOperatorConfiguration, PreparedOperatorFeedbackVoice};
use crate::{Invalid, fm4::OPERATORS};

pub const MAX_OPERATOR_RETARGET_FRAMES: u32 = 4_096;

/// Validated oscillator levels, carrier weights and self-feedback depths.
/// Values are prepared from an immutable Operator configuration on control.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OperatorFeedbackParameters {
    pub(super) oscillator_levels: [f64; OPERATORS],
    pub(super) carrier_weights: [f64; OPERATORS],
    pub(super) feedback_turns: [f64; OPERATORS],
}

impl OperatorFeedbackParameters {
    pub const fn oscillator_levels(&self) -> &[f64; OPERATORS] {
        &self.oscillator_levels
    }

    pub const fn carrier_weights(&self) -> &[f64; OPERATORS] {
        &self.carrier_weights
    }

    pub const fn feedback_turns(&self) -> &[f64; OPERATORS] {
        &self.feedback_turns
    }
}

/// One bounded linear ramp shared by all voices that were active at the edit.
/// It owns only fixed coefficients and identifies the exact prepared target
/// configuration; no per-voice ramp storage or audio-thread allocation is needed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreparedOperatorFeedbackRetarget {
    start: OperatorFeedbackParameters,
    target: OperatorFeedbackParameters,
    frames: u32,
    source_configuration_id: u64,
    target_configuration_id: u64,
}

impl PreparedOperatorFeedbackRetarget {
    /// Original default: 5 ms, rounded to the nearest sample. At 48 kHz this
    /// is exactly 240 frames. The caller must pass this result to `prepare`.
    pub fn default_frames(sample_rate: u32) -> Result<u32, Invalid> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(Invalid("invalid Operator retarget sample rate"));
        }
        Ok(((u64::from(sample_rate) * 5 + 500) / 1_000) as u32)
    }

    /// Prepare a bounded ramp from a voice's current configuration to a
    /// compatible prepared target. The voice and both configurations remain
    /// unchanged if validation fails. `ramp_frames` is in `[1, 4096]`;
    /// use `default_frames` for the original 5 ms default.
    pub fn prepare(
        voice: &PreparedOperatorFeedbackVoice,
        current: &PreparedOperatorConfiguration,
        target: &PreparedOperatorConfiguration,
        target_feedback_turns: [f64; OPERATORS],
        ramp_frames: u32,
    ) -> Result<Self, Invalid> {
        validate_frame_count(ramp_frames)?;
        if !current.retarget_compatible_with(target)
            || voice.configuration_id() != current.configuration_id()
            || voice.is_ramp_participant()
            || !current.note_matches_configuration(&voice.note)
            || !target.note_matches_configuration(&voice.note)
        {
            return Err(Invalid(
                "Operator retarget changes unsupported configuration or note metadata",
            ));
        }
        let target_configuration_id = target.configuration_id();
        let start = current.feedback_parameters(voice.feedback_turns)?;
        let target = target.feedback_parameters(target_feedback_turns)?;
        ensure_mutable_class(start, target)?;
        Ok(Self {
            start,
            target,
            frames: ramp_frames,
            source_configuration_id: current.configuration_id(),
            target_configuration_id,
        })
    }

    /// Retarget an in-progress ramp from its actual current interpolated
    /// values. `elapsed_frames` is the number of samples already rendered from
    /// the prior ramp, so zero means its first sample has not yet been used.
    pub fn retarget(
        &self,
        voice: &PreparedOperatorFeedbackVoice,
        current_target: &PreparedOperatorConfiguration,
        next_target: &PreparedOperatorConfiguration,
        next_feedback_turns: [f64; OPERATORS],
        elapsed_frames: u32,
        ramp_frames: u32,
    ) -> Result<Self, Invalid> {
        validate_frame_count(ramp_frames)?;
        if elapsed_frames > self.frames
            || current_target.configuration_id() != self.target_configuration_id
            || !current_target.retarget_compatible_with(next_target)
            || !voice_matches_ramp_boundary(voice, self, elapsed_frames)?
        {
            return Err(Invalid("invalid Operator retarget continuation"));
        }
        let actual_current = self.values_at(elapsed_frames)?;
        if current_target.feedback_parameters(self.target.feedback_turns)? != self.target {
            return Err(Invalid(
                "Operator retarget source no longer matches its ramp",
            ));
        }
        let target = next_target.feedback_parameters(next_feedback_turns)?;
        ensure_mutable_class(actual_current, target)?;
        Ok(Self {
            start: actual_current,
            target,
            frames: ramp_frames,
            source_configuration_id: voice.configuration_id(),
            target_configuration_id: next_target.configuration_id(),
        })
    }

    /// Return the parameter value at a sample boundary. Zero is the exact old
    /// value; `frames` is the exact new endpoint. Values beyond the endpoint
    /// are refused rather than silently changing the transition clock.
    pub fn values_at(&self, elapsed_frames: u32) -> Result<OperatorFeedbackParameters, Invalid> {
        if elapsed_frames > self.frames {
            return Err(Invalid("Operator retarget elapsed frame exceeds ramp"));
        }
        if elapsed_frames == 0 {
            return Ok(self.start);
        }
        if elapsed_frames == self.frames {
            return Ok(self.target);
        }
        let progress = f64::from(elapsed_frames) / f64::from(self.frames);
        Ok(OperatorFeedbackParameters {
            oscillator_levels: interpolate(
                self.start.oscillator_levels,
                self.target.oscillator_levels,
                progress,
            ),
            carrier_weights: interpolate(
                self.start.carrier_weights,
                self.target.carrier_weights,
                progress,
            ),
            feedback_turns: interpolate(
                self.start.feedback_turns,
                self.target.feedback_turns,
                progress,
            ),
        })
    }

    pub const fn frames(&self) -> u32 {
        self.frames
    }

    pub const fn target_configuration_id(&self) -> u64 {
        self.target_configuration_id
    }

    pub const fn source_configuration_id(&self) -> u64 {
        self.source_configuration_id
    }
}

impl PreparedOperatorFeedbackVoice {
    /// Prepare a parameter ramp for this held or released voice without
    /// changing its age, gate, phase history or feedback memory. The returned
    /// ramp is shared control state; the caller advances its sample offset and
    /// borrows it while processing every voice already active at the edit.
    pub fn prepare_retarget(
        &self,
        current: &PreparedOperatorConfiguration,
        target: &PreparedOperatorConfiguration,
        target_feedback_turns: [f64; OPERATORS],
        ramp_frames: u32,
    ) -> Result<PreparedOperatorFeedbackRetarget, Invalid> {
        PreparedOperatorFeedbackRetarget::prepare(
            self,
            current,
            target,
            target_feedback_turns,
            ramp_frames,
        )
    }

    /// Render one frame against a prepared shared ramp. The exact target
    /// configuration identity is checked in constant time before any voice
    /// state changes. Each frame uses the immutable shared ramp coefficients,
    /// keeps oscillator feedback history continuous, then records the effective
    /// feedback value so normal processing resumes at the target endpoint.
    #[inline]
    pub fn process_frame_with_retarget(
        &mut self,
        configuration: &PreparedOperatorConfiguration,
        table: &crate::SineTable,
        retarget: &PreparedOperatorFeedbackRetarget,
        elapsed_frames: u32,
    ) -> Result<f32, Invalid> {
        if elapsed_frames >= retarget.frames
            || configuration.configuration_id() != retarget.target_configuration_id
            || !configuration.note_matches_configuration(&self.note)
            || !voice_matches_ramp_boundary(self, retarget, elapsed_frames)?
        {
            return Err(Invalid(
                "Operator retarget does not match the prepared voice configuration",
            ));
        }
        let next_age = self
            .age_frames
            .checked_add(1)
            .ok_or(Invalid("operator feedback voice age overflow"))?;
        let parameters = retarget.values_at(elapsed_frames)?;
        let gate_frame = self.gate_frame.unwrap_or(u64::MAX);
        let (sample, outputs) = configuration.sample_with_feedback_parameters(
            table,
            &self.note,
            self.age_frames,
            gate_frame,
            parameters,
            self.previous_outputs,
        );
        self.previous_outputs = outputs;
        let next_boundary = elapsed_frames.saturating_add(1).min(retarget.frames);
        self.feedback_turns = retarget.values_at(next_boundary)?.feedback_turns;
        self.age_frames = next_age;
        let ramp_active = next_boundary < retarget.frames;
        self.configuration_id = (retarget.target_configuration_id << 1) | u64::from(ramp_active);
        Ok(sample)
    }
}

fn voice_matches_ramp_boundary(
    voice: &PreparedOperatorFeedbackVoice,
    retarget: &PreparedOperatorFeedbackRetarget,
    elapsed_frames: u32,
) -> Result<bool, Invalid> {
    let expected = retarget.values_at(elapsed_frames)?;
    Ok(if voice.is_ramp_participant() {
        (voice.configuration_id() == retarget.source_configuration_id
            || voice.configuration_id() == retarget.target_configuration_id)
            && voice.feedback_turns == expected.feedback_turns
    } else {
        (voice.configuration_id() == retarget.source_configuration_id
            || (elapsed_frames == retarget.frames
                && voice.configuration_id() == retarget.target_configuration_id))
            && voice.feedback_turns == expected.feedback_turns
    })
}

fn validate_frame_count(frames: u32) -> Result<(), Invalid> {
    if (1..=MAX_OPERATOR_RETARGET_FRAMES).contains(&frames) {
        Ok(())
    } else {
        Err(Invalid(
            "Operator retarget ramp must be between 1 and 4096 frames",
        ))
    }
}

fn ensure_mutable_class(
    current: OperatorFeedbackParameters,
    target: OperatorFeedbackParameters,
) -> Result<(), Invalid> {
    if current.feedback_turns == [0.0; OPERATORS] || target.feedback_turns == [0.0; OPERATORS] {
        return Err(Invalid(
            "Operator retarget cannot enter or leave the mutable feedback class",
        ));
    }
    Ok(())
}

fn interpolate<const N: usize>(from: [f64; N], to: [f64; N], progress: f64) -> [f64; N] {
    std::array::from_fn(|index| from[index] + (to[index] - from[index]) * progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        SineTable,
        fm4::{OperatorTuning, Oscillator, Routing},
        operator_envelope::OperatorEnvelope,
        operator_waveform::OperatorWaveform,
    };

    const RATE: u32 = 48_000;
    const PITCH: u8 = 69;
    const VELOCITY: u8 = 103;
    const RATIOS: [f64; OPERATORS] = [1.0, 1.5, 2.0, 0.75];
    const LEVELS_A: [f64; OPERATORS] = [0.8, 0.65, 0.5, 0.9];
    const LEVELS_B: [f64; OPERATORS] = [0.25, 0.9, 0.7, 0.4];
    const CARRIERS_A: [f64; OPERATORS] = [0.5, -0.25, 0.7, 0.2];
    const CARRIERS_B: [f64; OPERATORS] = [-0.3, 0.65, 0.1, 0.8];
    const FEEDBACK_A: [f64; OPERATORS] = [0.17, 0.0, 0.0, 0.23];
    const FEEDBACK_B: [f64; OPERATORS] = [0.61, 0.0, 0.0, 0.42];
    const MODULATION: [[f64; OPERATORS]; OPERATORS] = [[0.0; OPERATORS]; OPERATORS];

    fn config(
        levels: [f64; OPERATORS],
        carriers: [f64; OPERATORS],
        modulation: [[f64; OPERATORS]; OPERATORS],
        envelopes: [OperatorEnvelope; OPERATORS],
        waveforms: [OperatorWaveform; OPERATORS],
        tuning: [OperatorTuning; OPERATORS],
        amplitude_velocity: [f32; OPERATORS],
    ) -> PreparedOperatorConfiguration {
        let oscillators = std::array::from_fn(|index| {
            Oscillator::new(RATIOS[index], levels[index], 0.0).expect("valid oscillator")
        });
        PreparedOperatorConfiguration::new_with_tuning_and_amplitude_velocity(
            RATE,
            oscillators,
            Routing::new(modulation, carriers).expect("valid routing"),
            envelopes,
            waveforms,
            tuning,
            amplitude_velocity,
        )
        .expect("valid Operator configuration")
    }

    fn standard_config(
        levels: [f64; OPERATORS],
        carriers: [f64; OPERATORS],
    ) -> PreparedOperatorConfiguration {
        let envelope = OperatorEnvelope::new(0.0, 1.0, 0.7, 0.0, 0.0, 8.0 / f64::from(RATE))
            .expect("valid envelope");
        config(
            levels,
            carriers,
            MODULATION,
            [envelope; OPERATORS],
            [OperatorWaveform::Sine; OPERATORS],
            [OperatorTuning::Ratio; OPERATORS],
            [0.0; OPERATORS],
        )
    }

    fn oracle_level(age: u64, gate: u64) -> f64 {
        if age < gate {
            0.7
        } else if age - gate >= 8 {
            0.0
        } else {
            0.7 * 0.00001_f64.powf((age - gate) as f64 / 8.0)
        }
    }

    fn oracle_frame(
        age: u64,
        gate: u64,
        parameters: OperatorFeedbackParameters,
        previous: &mut [f64; OPERATORS],
    ) -> f32 {
        let mut signals = [0.0; OPERATORS];
        for operator in 0..OPERATORS {
            let phase_turns = 440.0 * RATIOS[operator] * age as f64 / f64::from(RATE)
                + previous[operator] * parameters.feedback_turns[operator];
            signals[operator] = (std::f64::consts::TAU * phase_turns).sin()
                * parameters.oscillator_levels[operator]
                * oracle_level(age, gate);
        }
        *previous = signals;
        (signals
            .iter()
            .zip(parameters.carrier_weights)
            .map(|(signal, carrier)| signal * carrier)
            .sum::<f64>()
            * (f64::from(VELOCITY) / 127.0)) as f32
    }

    #[test]
    fn shared_ramp_matches_independent_recurrence_through_noteoff_and_partitions() {
        let old = standard_config(LEVELS_A, CARRIERS_A);
        let target = standard_config(LEVELS_B, CARRIERS_B);
        let table = SineTable::new();
        let duration = 19;
        let total = 45;
        let gate = 7_u32;
        let expected_start = old
            .feedback_parameters(FEEDBACK_A)
            .expect("valid old values");
        let expected_end = target
            .feedback_parameters(FEEDBACK_B)
            .expect("valid new values");
        let mut expected = Vec::new();
        let mut previous = [0.0; OPERATORS];
        for age in 0..total {
            let parameters = if age < duration {
                let progress = age as f64 / duration as f64;
                OperatorFeedbackParameters {
                    oscillator_levels: std::array::from_fn(|index| {
                        expected_start.oscillator_levels[index]
                            + (expected_end.oscillator_levels[index]
                                - expected_start.oscillator_levels[index])
                                * progress
                    }),
                    carrier_weights: std::array::from_fn(|index| {
                        expected_start.carrier_weights[index]
                            + (expected_end.carrier_weights[index]
                                - expected_start.carrier_weights[index])
                                * progress
                    }),
                    feedback_turns: std::array::from_fn(|index| {
                        expected_start.feedback_turns[index]
                            + (expected_end.feedback_turns[index]
                                - expected_start.feedback_turns[index])
                                * progress
                    }),
                }
            } else {
                expected_end
            };
            expected.push(oracle_frame(
                age as u64,
                u64::from(gate),
                parameters,
                &mut previous,
            ));
        }

        for partition in [1, 4, 7, 16, 32] {
            let mut voice = old
                .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_A)
                .expect("prepared voice");
            let ramp = voice
                .prepare_retarget(&old, &target, FEEDBACK_B, duration)
                .expect("prepared shared ramp");
            assert_eq!(ramp.values_at(0).expect("start"), expected_start);
            assert_eq!(ramp.values_at(duration).expect("endpoint"), expected_end);
            assert!(ramp.values_at(duration + 1).is_err());
            let mut actual = Vec::new();
            let mut cursor = 0;
            while cursor < total {
                if cursor == gate {
                    voice.note_off().expect("NoteOff at exact frame");
                }
                let mut end = (cursor + partition).min(total);
                if cursor < gate && end > gate {
                    end = gate;
                }
                for age in cursor..end {
                    let sample = if age < duration {
                        voice.process_frame_with_retarget(&target, &table, &ramp, age)
                    } else {
                        voice.process_frame(&target, &table)
                    }
                    .expect("voice sample");
                    actual.push(sample);
                }
                cursor = end;
            }
            for (age, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
                assert!(
                    (f64::from(*actual) - f64::from(*expected)).abs() < 0.000_15,
                    "partition={partition}, age={age}: {actual} != {expected}"
                );
            }
            assert_eq!(voice.gate_frame(), Some(u64::from(gate)));
            assert_eq!(voice.age_frames(), total as u64);
        }
    }

    #[test]
    fn rapid_retarget_starts_at_actual_current_values_and_copy_keeps_future_pcm() {
        let a = standard_config(LEVELS_A, CARRIERS_A);
        let b = standard_config(LEVELS_B, CARRIERS_B);
        let c = standard_config([0.3, 0.45, 0.85, 0.6], [0.2, 0.1, -0.5, 0.7]);
        let mut voice = a
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_A)
            .expect("prepared voice");
        let first = voice
            .prepare_retarget(&a, &b, FEEDBACK_B, 20)
            .expect("first ramp");
        let actual = first.values_at(7).expect("current intermediate values");
        for elapsed in 0..7 {
            voice
                .process_frame_with_retarget(&b, &SineTable::new(), &first, elapsed)
                .expect("first ramp sample");
        }
        let second = first
            .retarget(&voice, &b, &c, [0.33, 0.0, 0.0, 0.71], 7, 9)
            .expect("rapid second ramp");
        assert_eq!(second.values_at(0).expect("new start"), actual);
        assert_eq!(
            second.values_at(9).expect("new endpoint"),
            c.feedback_parameters([0.33, 0.0, 0.0, 0.71])
                .expect("target")
        );
        let mut copied = voice;
        let table = SineTable::new();
        for elapsed in 0..9 {
            assert_eq!(
                voice
                    .process_frame_with_retarget(&c, &table, &second, elapsed)
                    .expect("original"),
                copied
                    .process_frame_with_retarget(&c, &table, &second, elapsed)
                    .expect("copy")
            );
        }
    }

    #[test]
    fn completed_ramp_hands_off_exactly_and_allows_a_fresh_ramp() {
        let a = standard_config(LEVELS_A, CARRIERS_A);
        let b = standard_config(LEVELS_B, CARRIERS_B);
        let c = standard_config([0.3, 0.45, 0.85, 0.6], [0.2, 0.1, -0.5, 0.7]);
        let table = SineTable::new();
        let mut voice = a
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_A)
            .expect("prepared voice");
        let first = voice
            .prepare_retarget(&a, &b, FEEDBACK_B, 4)
            .expect("first ramp");
        let mut last = 0.0;
        for elapsed in 0..first.frames() {
            last = voice
                .process_frame_with_retarget(&b, &table, &first, elapsed)
                .expect("ramp sample");
        }
        assert_eq!(
            voice.feedback_turns,
            b.feedback_parameters(FEEDBACK_B)
                .expect("target")
                .feedback_turns
        );
        let mut copied = voice;
        assert_eq!(
            voice.process_frame(&b, &table).expect("normal handoff"),
            copied
                .process_frame(&b, &table)
                .expect("copied target sample")
        );
        let endpoint_retarget = first
            .retarget(&voice, &b, &c, [0.31, 0.0, 0.0, 0.67], first.frames(), 6)
            .expect("endpoint retarget");
        assert_eq!(
            endpoint_retarget.values_at(0).expect("endpoint start"),
            b.feedback_parameters(FEEDBACK_B).expect("endpoint target")
        );
        let second = voice
            .prepare_retarget(&b, &c, [0.31, 0.0, 0.0, 0.67], 6)
            .expect("fresh ramp after endpoint");
        assert_eq!(
            second.values_at(0).expect("second start"),
            b.feedback_parameters(FEEDBACK_B)
                .expect("second start target")
        );
        assert!(last.is_finite());
        assert!(
            voice
                .process_frame_with_retarget(&c, &table, &second, second.frames())
                .is_err()
        );
    }

    #[test]
    fn exact_history_copy_rebind_preserves_legacy_future_pcm() {
        let source = standard_config(LEVELS_A, CARRIERS_A);
        let equivalent = standard_config(LEVELS_A, CARRIERS_A);
        let changed = standard_config(LEVELS_B, CARRIERS_A);
        let table = SineTable::new();
        let mut original = source
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_A)
            .expect("prepared voice");
        let mut copied = original;
        for _ in 0..9 {
            assert_eq!(
                original.process_frame(&source, &table).expect("source"),
                copied
                    .process_frame(&equivalent, &table)
                    .expect("legacy copied history")
            );
        }
        copied
            .rebind_after_exact_history_copy(&source, &equivalent)
            .expect("exact inactive history rebind");
        for _ in 0..19 {
            assert_eq!(
                original.process_frame(&source, &table).expect("source"),
                copied
                    .process_frame(&equivalent, &table)
                    .expect("rebound copy")
            );
        }
        let before = copied;
        assert!(
            copied
                .rebind_after_exact_history_copy(&equivalent, &changed)
                .is_err()
        );
        assert_eq!(copied, before);
    }

    #[test]
    fn unsupported_changes_bad_endpoints_and_wrong_voice_or_target_refuse_atomically() {
        let old = standard_config(LEVELS_A, CARRIERS_A);
        let target = standard_config(LEVELS_B, CARRIERS_B);
        let mut voice = old
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_A)
            .expect("prepared voice");
        let before = voice;
        assert!(
            voice
                .prepare_retarget(&old, &target, [0.0; OPERATORS], 8)
                .is_err()
        );
        assert!(
            voice
                .prepare_retarget(&old, &target, FEEDBACK_B, 0)
                .is_err()
        );
        assert!(
            voice
                .prepare_retarget(&old, &target, FEEDBACK_B, MAX_OPERATOR_RETARGET_FRAMES + 1)
                .is_err()
        );
        assert!(
            voice
                .prepare_retarget(&old, &target, [f64::NAN, 0.0, 0.0, 0.2], 8)
                .is_err()
        );

        let changed_envelope =
            OperatorEnvelope::new(0.0, 1.0, 0.6, 0.0, 0.0, 8.0 / f64::from(RATE))
                .expect("valid changed envelope");
        let unsupported = config(
            LEVELS_B,
            CARRIERS_B,
            MODULATION,
            [changed_envelope; OPERATORS],
            [OperatorWaveform::Sine; OPERATORS],
            [OperatorTuning::Ratio; OPERATORS],
            [0.0; OPERATORS],
        );
        assert!(
            voice
                .prepare_retarget(&old, &unsupported, FEEDBACK_B, 8)
                .is_err()
        );
        assert_eq!(voice, before);

        let ramp = voice
            .prepare_retarget(&old, &target, FEEDBACK_B, 8)
            .expect("valid ramp");
        voice
            .process_frame_with_retarget(&target, &SineTable::new(), &ramp, 0)
            .expect("first ramp sample");
        let before_legacy = voice;
        assert!(voice.process_frame(&target, &SineTable::new()).is_err());
        assert_eq!(voice, before_legacy);
        let unrelated = standard_config(LEVELS_B, CARRIERS_B);
        let original = voice;
        assert!(
            voice
                .process_frame_with_retarget(&unrelated, &SineTable::new(), &ramp, 0)
                .is_err()
        );
        assert_eq!(voice, original);
        let mut target_born = target
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_B)
            .expect("new target voice");
        let target_born_before = target_born;
        assert!(
            target_born
                .process_frame_with_retarget(&target, &SineTable::new(), &ramp, 0)
                .is_err()
        );
        assert_eq!(target_born, target_born_before);
        assert_eq!(
            ramp.values_at(0).expect("ramp unchanged"),
            old.feedback_parameters(FEEDBACK_A).expect("old")
        );
    }

    #[test]
    fn default_duration_is_five_milliseconds_and_storage_stays_bounded() {
        assert_eq!(
            PreparedOperatorFeedbackRetarget::default_frames(RATE),
            Ok(240)
        );
        assert_eq!(
            PreparedOperatorFeedbackRetarget::default_frames(8_000),
            Ok(40)
        );
        assert_eq!(
            PreparedOperatorFeedbackRetarget::default_frames(44_100),
            Ok(221)
        );
        assert_eq!(
            PreparedOperatorFeedbackRetarget::default_frames(192_000),
            Ok(960)
        );
        assert!(PreparedOperatorFeedbackRetarget::default_frames(7_999).is_err());
        assert_eq!(std::mem::size_of::<OperatorFeedbackParameters>(), 96);
        println!(
            "retarget={}B, voice={}B, configuration={}B",
            std::mem::size_of::<PreparedOperatorFeedbackRetarget>(),
            std::mem::size_of::<PreparedOperatorFeedbackVoice>(),
            std::mem::size_of::<PreparedOperatorConfiguration>()
        );
        assert_eq!(std::mem::size_of::<PreparedOperatorFeedbackRetarget>(), 216);
        assert_eq!(std::mem::size_of::<PreparedOperatorFeedbackVoice>(), 160);
        assert!(std::mem::size_of::<PreparedOperatorFeedbackRetarget>() <= 256);
        assert!(
            16 * std::mem::size_of::<PreparedOperatorFeedbackVoice>()
                + std::mem::size_of::<PreparedOperatorFeedbackRetarget>()
                <= 3_072
        );
    }
}
