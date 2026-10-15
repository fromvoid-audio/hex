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
    pub(crate) fn with_oscillator_levels(mut self, levels: [f64; OPERATORS]) -> Self {
        self.oscillator_levels = levels;
        self
    }

    pub(crate) fn interpolate(self, target: Self, elapsed: u32, frames: u32) -> Self {
        if elapsed == 0 {
            return self;
        }
        if elapsed >= frames {
            return target;
        }
        let t = f64::from(elapsed) / f64::from(frames);
        Self {
            oscillator_levels: interpolate(self.oscillator_levels, target.oscillator_levels, t),
            carrier_weights: interpolate(self.carrier_weights, target.carrier_weights, t),
            feedback_turns: interpolate(self.feedback_turns, target.feedback_turns, t),
        }
    }

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

    /// Prepare a fixed ramp from an actual shared runtime boundary without
    /// requiring a live voice. The caller must first select the source values
    /// from the committed runtime (authored values when idle, or the current
    /// ramp value). This control/graph-boundary helper preserves the source and
    /// target configuration identities while allocating no per-voice state.
    pub(crate) fn prepare_from_shared_parameters(
        current: &PreparedOperatorConfiguration,
        target: &PreparedOperatorConfiguration,
        start: OperatorFeedbackParameters,
        target_feedback_turns: [f64; OPERATORS],
        ramp_frames: u32,
    ) -> Result<Self, Invalid> {
        validate_frame_count(ramp_frames)?;
        if !current.retarget_compatible_with(target) {
            return Err(Invalid(
                "Operator retarget changes unsupported immutable configuration",
            ));
        }
        let target_parameters = target.feedback_parameters(target_feedback_turns)?;
        ensure_mutable_class(start, target_parameters)?;
        Ok(Self {
            start,
            target: target_parameters,
            frames: ramp_frames,
            source_configuration_id: current.configuration_id(),
            target_configuration_id: target.configuration_id(),
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
        {
            return Err(Invalid("invalid Operator retarget continuation"));
        }
        let actual_current = self.values_at(elapsed_frames)?;
        if !voice_matches_ramp_boundary(voice, self, elapsed_frames, actual_current.feedback_turns)
        {
            return Err(Invalid("invalid Operator retarget continuation"));
        }
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

    /// Prepare a continuation from the current shared ramp boundary without
    /// requiring a live voice. This is needed when every voice has expired
    /// while the global parameter ramp is still in flight. At elapsed zero,
    /// no voice has rendered the prior ramp yet, so the continuation preserves
    /// its original source identity. At later boundaries every host-managed
    /// voice has advanced together and carries the prior target identity.
    pub fn continue_from_shared_ramp(
        &self,
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
            || current_target.feedback_parameters(self.target.feedback_turns)? != self.target
        {
            return Err(Invalid("invalid shared Operator retarget continuation"));
        }
        let start = self.values_at(elapsed_frames)?;
        let target = next_target.feedback_parameters(next_feedback_turns)?;
        ensure_mutable_class(start, target)?;
        Ok(Self {
            start,
            target,
            frames: ramp_frames,
            source_configuration_id: if elapsed_frames == 0 {
                self.source_configuration_id
            } else {
                self.target_configuration_id
            },
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

    /// Prepare the global values for one sample boundary. Every voice in the
    /// runtime borrows this immutable frame, while retaining its own note,
    /// envelope, phase and feedback history. The borrow binds the frame to
    /// this exact ramp value and prevents reuse with another ramp or clock.
    #[inline]
    pub(crate) fn frame_at<'a>(
        &'a self,
        configuration: &PreparedOperatorConfiguration,
        elapsed_frames: u32,
    ) -> Result<OperatorFeedbackRetargetFrame<'a>, Invalid> {
        if elapsed_frames >= self.frames
            || configuration.configuration_id() != self.target_configuration_id
        {
            return Err(Invalid("invalid Operator retarget frame boundary"));
        }
        let parameters = self.values_at(elapsed_frames)?;
        let next_boundary = elapsed_frames.saturating_add(1).min(self.frames);
        let next_feedback_turns = self.values_at(next_boundary)?.feedback_turns;
        Ok(OperatorFeedbackRetargetFrame {
            retarget: self,
            configuration_id: configuration.configuration_id(),
            elapsed_frames,
            parameters,
            next_feedback_turns,
            next_boundary,
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

    /// Return the same ramp bound to a separately prepared but exactly equal
    /// target configuration. The caller must also rebind every carried voice
    /// using this returned ramp before processing the copied history.
    pub fn rebind_target_after_exact_history_copy(
        &self,
        source_target: &PreparedOperatorConfiguration,
        replacement_target: &PreparedOperatorConfiguration,
        elapsed_frames: u32,
    ) -> Result<Self, Invalid> {
        validate_rebind_boundary(self, source_target, replacement_target, elapsed_frames)?;
        let mut rebound = *self;
        rebound.target_configuration_id = replacement_target.configuration_id();
        Ok(rebound)
    }

    /// Read-only proof that `replacement` is this exact ramp rebound to an
    /// equal target configuration. The original source identity and all ramp
    /// coefficients, duration and progress remain unchanged.
    pub fn is_exact_target_rebind_of(
        &self,
        original: &Self,
        source_target: &PreparedOperatorConfiguration,
        replacement_target: &PreparedOperatorConfiguration,
        elapsed_frames: u32,
    ) -> bool {
        original
            .rebind_target_after_exact_history_copy(
                source_target,
                replacement_target,
                elapsed_frames,
            )
            .is_ok_and(|expected| *self == expected)
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

    /// Validate one shared-ramp frame without sampling or mutating voice state.
    /// This is the same compatibility preflight used by the renderer.
    #[inline]
    pub(crate) fn validate_retarget_frame(
        &self,
        configuration: &PreparedOperatorConfiguration,
        retarget: &PreparedOperatorFeedbackRetarget,
        elapsed_frames: u32,
    ) -> Result<(), Invalid> {
        self.preflight_retarget_frame(configuration, retarget, elapsed_frames)
            .map(|_| ())
    }

    #[inline]
    fn preflight_retarget_frame(
        &self,
        configuration: &PreparedOperatorConfiguration,
        retarget: &PreparedOperatorFeedbackRetarget,
        elapsed_frames: u32,
    ) -> Result<RetargetFrame, Invalid> {
        let shared = retarget.frame_at(configuration, elapsed_frames)?;
        self.preflight_shared_retarget_frame(configuration, retarget, &shared, elapsed_frames)
    }

    #[inline]
    fn preflight_shared_retarget_frame(
        &self,
        configuration: &PreparedOperatorConfiguration,
        retarget: &PreparedOperatorFeedbackRetarget,
        shared: &OperatorFeedbackRetargetFrame<'_>,
        elapsed_frames: u32,
    ) -> Result<RetargetFrame, Invalid> {
        if elapsed_frames >= retarget.frames
            || !std::ptr::eq(shared.retarget, retarget)
            || shared.configuration_id != configuration.configuration_id()
            || shared.elapsed_frames != elapsed_frames
            || configuration.configuration_id() != retarget.target_configuration_id
            || !configuration.note_matches_configuration(&self.note)
            || !voice_matches_ramp_boundary(
                self,
                retarget,
                elapsed_frames,
                shared.parameters.feedback_turns,
            )
        {
            return Err(Invalid(
                "Operator retarget does not match the prepared voice configuration",
            ));
        }
        let next_age = self
            .age_frames
            .checked_add(1)
            .ok_or(Invalid("operator feedback voice age overflow"))?;
        Ok(RetargetFrame {
            next_age,
            parameters: shared.parameters,
            next_feedback_turns: shared.next_feedback_turns,
            next_boundary: shared.next_boundary,
        })
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
        let shared = retarget.frame_at(configuration, elapsed_frames)?;
        self.process_frame_with_shared_retarget(
            configuration,
            table,
            retarget,
            &shared,
            elapsed_frames,
        )
    }

    /// Render with a frame shared by the runtime's active voices. All fallible
    /// checks, including note identity and age overflow, finish before state
    /// changes.
    #[inline]
    pub(crate) fn process_frame_with_shared_retarget(
        &mut self,
        configuration: &PreparedOperatorConfiguration,
        table: &crate::SineTable,
        retarget: &PreparedOperatorFeedbackRetarget,
        shared: &OperatorFeedbackRetargetFrame<'_>,
        elapsed_frames: u32,
    ) -> Result<f32, Invalid> {
        let frame =
            self.preflight_shared_retarget_frame(configuration, retarget, shared, elapsed_frames)?;
        let gate_frame = self.gate_frame.unwrap_or(u64::MAX);
        let (sample, outputs) = configuration.sample_with_feedback_parameters(
            table,
            &self.note,
            self.age_frames,
            gate_frame,
            frame.parameters,
            self.previous_outputs,
        );
        self.previous_outputs = outputs;
        self.feedback_turns = frame.next_feedback_turns;
        self.age_frames = frame.next_age;
        let ramp_active = frame.next_boundary < retarget.frames;
        self.configuration_id = (retarget.target_configuration_id << 1) | u64::from(ramp_active);
        Ok(sample)
    }

    /// Enroll a newly prepared note in the runtime's current shared ramp.
    /// Fresh notes begin with zero age, no gate and empty feedback history;
    /// they use the same interpolated parameter boundary as all carried voices.
    /// At the ramp endpoint the ordinary target-configuration path is used.
    #[inline]
    pub fn process_frame_joining_retarget(
        &mut self,
        configuration: &PreparedOperatorConfiguration,
        table: &crate::SineTable,
        retarget: &PreparedOperatorFeedbackRetarget,
        elapsed_frames: u32,
    ) -> Result<f32, Invalid> {
        let mut candidate = *self;
        candidate.enroll_in_retarget(configuration, retarget, elapsed_frames)?;
        let sample = if elapsed_frames == retarget.frames {
            candidate.process_frame(configuration, table)?
        } else {
            candidate.process_frame_with_retarget(configuration, table, retarget, elapsed_frames)?
        };
        *self = candidate;
        Ok(sample)
    }

    /// Enroll a newly prepared target voice at the current shared parameter
    /// boundary without rendering a sample. Hosts may call this immediately at
    /// NoteOn, before applying a same-offset NoteOff; normal ramp processing
    /// then observes that gate at age zero. Refusal is atomic.
    pub fn enroll_in_retarget(
        &mut self,
        configuration: &PreparedOperatorConfiguration,
        retarget: &PreparedOperatorFeedbackRetarget,
        elapsed_frames: u32,
    ) -> Result<(), Invalid> {
        if elapsed_frames > retarget.frames || !self.is_fresh_target_voice(configuration, retarget)
        {
            return Err(Invalid(
                "operator voice cannot join this retarget at its current state",
            ));
        }
        let parameters = retarget.values_at(elapsed_frames)?;
        self.feedback_turns = parameters.feedback_turns;
        let ramp_active = elapsed_frames < retarget.frames;
        self.configuration_id = (retarget.target_configuration_id << 1) | u64::from(ramp_active);
        Ok(())
    }

    fn is_fresh_target_voice(
        &self,
        configuration: &PreparedOperatorConfiguration,
        retarget: &PreparedOperatorFeedbackRetarget,
    ) -> bool {
        self.configuration_id() == configuration.configuration_id()
            && !self.is_ramp_participant()
            && configuration.configuration_id() == retarget.target_configuration_id
            && configuration.note_matches_configuration(&self.note)
            && self.age_frames == 0
            && self.gate_frame.is_none()
            && self.previous_outputs == [0.0; OPERATORS]
            && self.feedback_turns == retarget.target.feedback_turns
    }

    /// Check the identity and sample-boundary conditions for exact history
    /// copying without changing this voice. If an active shared ramp is
    /// present, its original and rebound copies must be supplied together.
    pub fn can_rebind_after_exact_history_copy(
        &self,
        source: &PreparedOperatorConfiguration,
        replacement: &PreparedOperatorConfiguration,
        ramp: Option<(
            &PreparedOperatorFeedbackRetarget,
            &PreparedOperatorFeedbackRetarget,
            u32,
        )>,
    ) -> bool {
        if !source.identical_to(replacement)
            || !source.note_matches_configuration(&self.note)
            || !replacement.note_matches_configuration(&self.note)
        {
            return false;
        }
        let Some((original, rebound, elapsed_frames)) = ramp else {
            return !self.is_ramp_participant()
                && self.configuration_id() == source.configuration_id();
        };
        if !rebound.is_exact_target_rebind_of(original, source, replacement, elapsed_frames) {
            return false;
        }
        let Ok(expected) = original.values_at(elapsed_frames) else {
            return false;
        };
        if self.feedback_turns != expected.feedback_turns {
            return false;
        }

        if elapsed_frames == 0 {
            // A freshly retargeted voice may already carry the participant bit
            // from the preceding ramp. The new ramp clock has not advanced, so
            // its source configuration token (and that bit) must be preserved.
            if self.is_ramp_participant() {
                self.configuration_id() == original.source_configuration_id
                    || self.configuration_id() == original.target_configuration_id
            } else {
                self.configuration_id() == original.source_configuration_id
            }
        } else if elapsed_frames < original.frames {
            self.is_ramp_participant()
                && self.configuration_id() == original.target_configuration_id
        } else {
            !self.is_ramp_participant()
                && self.configuration_id() == original.target_configuration_id
        }
    }

    /// Rebind a validated exact-history copy. Refusal leaves this voice
    /// unchanged. Hosts should preflight every voice and the shared ramp before
    /// applying this operation to any member of a runtime array.
    pub fn rebind_after_exact_history_copy_with_ramp(
        &mut self,
        source: &PreparedOperatorConfiguration,
        replacement: &PreparedOperatorConfiguration,
        ramp: Option<(
            &PreparedOperatorFeedbackRetarget,
            &PreparedOperatorFeedbackRetarget,
            u32,
        )>,
    ) -> Result<(), Invalid> {
        if !self.can_rebind_after_exact_history_copy(source, replacement, ramp) {
            return Err(Invalid(
                "Operator voice cannot rebind without exact history compatibility",
            ));
        }
        if let Some((original, _, elapsed_frames)) = ramp {
            if (elapsed_frames == 0
                && self.is_ramp_participant()
                && self.configuration_id() == original.target_configuration_id
                && original.source_configuration_id != original.target_configuration_id)
                || (elapsed_frames > 0 && elapsed_frames < original.frames)
            {
                self.configuration_id = (replacement.configuration_id() << 1) | 1;
            } else if elapsed_frames == original.frames {
                self.configuration_id = replacement.configuration_id() << 1;
            }
        } else {
            self.configuration_id = replacement.configuration_id() << 1;
        }
        Ok(())
    }
}

fn validate_rebind_boundary(
    ramp: &PreparedOperatorFeedbackRetarget,
    source_target: &PreparedOperatorConfiguration,
    replacement_target: &PreparedOperatorConfiguration,
    elapsed_frames: u32,
) -> Result<(), Invalid> {
    if elapsed_frames > ramp.frames
        || ramp.target_configuration_id != source_target.configuration_id()
        || !source_target.identical_to(replacement_target)
        || source_target.feedback_parameters(ramp.target.feedback_turns)? != ramp.target
    {
        return Err(Invalid(
            "Operator retarget cannot rebind to a different prepared target",
        ));
    }
    let _ = ramp.values_at(elapsed_frames)?;
    Ok(())
}

#[derive(Clone, Copy)]
struct RetargetFrame {
    next_age: u64,
    parameters: OperatorFeedbackParameters,
    next_feedback_turns: [f64; OPERATORS],
    next_boundary: u32,
}

/// Shared parameter values for one global retarget sample. The lifetime and
/// pointer check bind every consumer to the ramp used to mint this frame.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OperatorFeedbackRetargetFrame<'a> {
    retarget: &'a PreparedOperatorFeedbackRetarget,
    configuration_id: u64,
    elapsed_frames: u32,
    parameters: OperatorFeedbackParameters,
    next_feedback_turns: [f64; OPERATORS],
    next_boundary: u32,
}

fn voice_matches_ramp_boundary(
    voice: &PreparedOperatorFeedbackVoice,
    retarget: &PreparedOperatorFeedbackRetarget,
    elapsed_frames: u32,
    expected_feedback_turns: [f64; OPERATORS],
) -> bool {
    if voice.is_ramp_participant() {
        (voice.configuration_id() == retarget.source_configuration_id
            || voice.configuration_id() == retarget.target_configuration_id)
            && voice.feedback_turns == expected_feedback_turns
    } else {
        (voice.configuration_id() == retarget.source_configuration_id
            || (elapsed_frames == retarget.frames
                && voice.configuration_id() == retarget.target_configuration_id))
            && voice.feedback_turns == expected_feedback_turns
    }
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

    fn assert_preflight_matches_render(
        voice: PreparedOperatorFeedbackVoice,
        configuration: &PreparedOperatorConfiguration,
        ramp: &PreparedOperatorFeedbackRetarget,
        elapsed: u32,
    ) -> bool {
        let unchanged = voice;
        let validation = voice.validate_retarget_frame(configuration, ramp, elapsed);
        assert_eq!(voice, unchanged, "read-only validation mutated voice");

        let mut render_candidate = voice;
        let rendered = render_candidate.process_frame_with_retarget(
            configuration,
            &SineTable::new(),
            ramp,
            elapsed,
        );
        assert_eq!(validation.is_ok(), rendered.is_ok());
        if validation.is_err() {
            assert_eq!(render_candidate, unchanged, "refused render mutated voice");
        }
        validation.is_ok()
    }

    #[test]
    fn sample_free_preflight_matches_render_for_ramp_boundaries_and_refusals() {
        let a = standard_config(LEVELS_A, CARRIERS_A);
        let b = standard_config(LEVELS_B, CARRIERS_B);
        let c = standard_config([0.3, 0.45, 0.85, 0.6], [0.2, 0.1, -0.5, 0.7]);
        let mut voice = a
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_A)
            .expect("prepared voice");
        let first = voice
            .prepare_retarget(&a, &b, FEEDBACK_B, 12)
            .expect("prepared ramp");

        // Fresh source and successive carried-voice boundaries use the same
        // identity, harmonic-cap, feedback and age validation in both paths.
        assert!(assert_preflight_matches_render(voice, &b, &first, 0));
        for elapsed in 0..5 {
            assert!(assert_preflight_matches_render(voice, &b, &first, elapsed));
            voice
                .process_frame_with_retarget(&b, &SineTable::new(), &first, elapsed)
                .expect("advance first ramp");
        }

        // A rapid continuation starts at the actual boundary and validates
        // the carried source identity before its first sample.
        let second = first
            .retarget(&voice, &b, &c, [0.33, 0.0, 0.0, 0.71], 5, 9)
            .expect("prepared rapid continuation");
        assert!(assert_preflight_matches_render(voice, &c, &second, 0));
        let mut wrong_boundary = voice;
        wrong_boundary.feedback_turns = FEEDBACK_B;
        assert!(!assert_preflight_matches_render(
            wrong_boundary,
            &c,
            &second,
            0
        ));
        let mut advanced = voice;
        advanced
            .process_frame_with_retarget(&c, &SineTable::new(), &second, 0)
            .expect("first continued sample");
        assert!(assert_preflight_matches_render(advanced, &c, &second, 1));
        let mut missing_participant = advanced;
        missing_participant.configuration_id &= !1;
        assert!(!assert_preflight_matches_render(
            missing_participant,
            &c,
            &second,
            1
        ));

        // New notes are explicitly enrolled at the same shared boundary.
        let mut joined = c
            .prepare_feedback_voice(PITCH, VELOCITY, [0.33, 0.0, 0.0, 0.71])
            .expect("new target voice");
        joined
            .enroll_in_retarget(&c, &second, 3)
            .expect("enroll at shared boundary");
        assert!(assert_preflight_matches_render(joined, &c, &second, 3));

        // The read-only path refuses stale target identity, endpoint/expired
        // offsets, and age overflow without changing the voice.
        assert!(!assert_preflight_matches_render(voice, &b, &second, 0));
        assert!(!assert_preflight_matches_render(
            voice,
            &c,
            &second,
            second.frames()
        ));
        let mut overflow = voice;
        overflow.age_frames = u64::MAX;
        let before_overflow = overflow;
        assert!(overflow.validate_retarget_frame(&c, &second, 0).is_err());
        assert_eq!(overflow, before_overflow);
        assert!(!assert_preflight_matches_render(overflow, &c, &second, 0));

        // Exact active-ramp copy changes only prepared target identity; the
        // same sample-free checks accept the rebound voice and ramp together.
        let replacement = standard_config(LEVELS_B, CARRIERS_B);
        let rebound = first
            .rebind_target_after_exact_history_copy(&b, &replacement, 5)
            .expect("rebound ramp");
        let mut rebound_voice = voice;
        rebound_voice
            .rebind_after_exact_history_copy_with_ramp(
                &b,
                &replacement,
                Some((&first, &rebound, 5)),
            )
            .expect("rebound voice");
        assert!(assert_preflight_matches_render(
            rebound_voice,
            &replacement,
            &rebound,
            5
        ));
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
    fn one_shared_sample_frame_matches_independent_voice_validation_and_chained_ramps() {
        const FEEDBACK_C: [f64; OPERATORS] = [0.29, 0.0, 0.0, 0.77];
        let old = standard_config(LEVELS_A, CARRIERS_A);
        let target = standard_config(LEVELS_B, CARRIERS_B);
        let next = standard_config([0.91, 0.47, 0.62, 0.31], [0.2, -0.1, 0.45, 0.3]);
        let table = SineTable::new();
        let first_voice = old
            .prepare_feedback_voice(57, 61, FEEDBACK_A)
            .expect("first old voice");
        let first_ramp = first_voice
            .prepare_retarget(&old, &target, FEEDBACK_B, 9)
            .expect("first shared ramp");
        let second_voice = old
            .prepare_feedback_voice(86, 119, FEEDBACK_A)
            .expect("second old voice");
        let mut shared_voices = [Some(first_voice), Some(second_voice), None];
        let mut reference_voices = shared_voices;
        let mut ramp = first_ramp;
        let mut ramp_elapsed = 0;

        for elapsed in 0..18 {
            if elapsed == 3 {
                for voices in [&mut shared_voices, &mut reference_voices] {
                    let mut fresh = target
                        .prepare_feedback_voice(72, 93, FEEDBACK_B)
                        .expect("fresh target voice");
                    fresh
                        .enroll_in_retarget(&target, &ramp, ramp_elapsed)
                        .expect("enroll at current global boundary");
                    voices[2] = Some(fresh);
                }
            }
            if elapsed == 2 {
                shared_voices[0]
                    .as_mut()
                    .expect("first shared voice")
                    .note_off()
                    .expect("first note-off");
                reference_voices[0]
                    .as_mut()
                    .expect("first reference voice")
                    .note_off()
                    .expect("first note-off");
            }
            if elapsed == 6 {
                shared_voices[2]
                    .as_mut()
                    .expect("fresh shared voice")
                    .note_off()
                    .expect("fresh note-off");
                reference_voices[2]
                    .as_mut()
                    .expect("fresh reference voice")
                    .note_off()
                    .expect("fresh note-off");
            }
            if elapsed == 5 {
                ramp = ramp
                    .continue_from_shared_ramp(&target, &next, FEEDBACK_C, ramp_elapsed, 7)
                    .expect("retarget from the shared current boundary");
                ramp_elapsed = 0;
            }

            let configuration = if elapsed < 5 { &target } else { &next };
            let shared_frame = (ramp_elapsed < ramp.frames()).then(|| {
                ramp.frame_at(configuration, ramp_elapsed)
                    .expect("shared sample frame")
            });
            for index in 0..shared_voices.len() {
                let (Some(shared_voice), Some(reference_voice)) = (
                    shared_voices[index].as_mut(),
                    reference_voices[index].as_mut(),
                ) else {
                    continue;
                };
                let shared_sample = if let Some(frame) = shared_frame.as_ref() {
                    shared_voice
                        .process_frame_with_shared_retarget(
                            configuration,
                            &table,
                            &ramp,
                            frame,
                            ramp_elapsed,
                        )
                        .unwrap_or_else(|error| {
                            panic!("shared frame refused voice {index} at {elapsed}: {error}")
                        })
                } else {
                    shared_voice
                        .process_frame(configuration, &table)
                        .expect("ordinary endpoint sample")
                };
                let reference_sample = if ramp_elapsed < ramp.frames() {
                    reference_voice
                        .process_frame_with_retarget(configuration, &table, &ramp, ramp_elapsed)
                        .expect("independent checked interpolation frame")
                } else {
                    reference_voice
                        .process_frame(configuration, &table)
                        .expect("ordinary endpoint sample")
                };
                assert_eq!(shared_sample.to_bits(), reference_sample.to_bits());
                assert_eq!(shared_voice, reference_voice);
            }
            ramp_elapsed += 1;
        }
    }

    #[test]
    fn shared_sample_frame_rejects_a_copied_ramp_and_wrong_clock_atomically() {
        let old = standard_config(LEVELS_A, CARRIERS_A);
        let target = standard_config(LEVELS_B, CARRIERS_B);
        let mut voice = old
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_A)
            .expect("prepared voice");
        let ramp = voice
            .prepare_retarget(&old, &target, FEEDBACK_B, 11)
            .expect("prepared ramp");
        let foreign_ramp = ramp;
        let foreign_frame = foreign_ramp
            .frame_at(&target, 0)
            .expect("frame belongs to copied ramp");
        let unchanged = voice;
        assert!(
            voice
                .process_frame_with_shared_retarget(
                    &target,
                    &SineTable::new(),
                    &ramp,
                    &foreign_frame,
                    0,
                )
                .is_err()
        );
        assert_eq!(voice, unchanged);

        let frame = ramp.frame_at(&target, 0).expect("frame at boundary zero");
        assert!(
            voice
                .process_frame_with_shared_retarget(&target, &SineTable::new(), &ramp, &frame, 1,)
                .is_err()
        );
        assert_eq!(voice, unchanged);
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
    fn shared_ramp_continues_without_voices_and_new_note_joins_current_value() {
        let a = standard_config(LEVELS_A, CARRIERS_A);
        let b = standard_config(LEVELS_B, CARRIERS_B);
        let c = standard_config([0.3, 0.45, 0.85, 0.6], [0.2, 0.1, -0.5, 0.7]);
        let seed = a
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_A)
            .expect("seed voice");
        let first = seed
            .prepare_retarget(&a, &b, FEEDBACK_B, 20)
            .expect("first ramp");

        // No voice is retained: the shared parameter trajectory remains
        // sufficient to prepare a rapid edit and admit a later NoteOn.
        for elapsed in [0, 7, 20] {
            let expected_start = first.values_at(elapsed).expect("old ramp boundary");
            let next = first
                .continue_from_shared_ramp(&b, &c, FEEDBACK_A, elapsed, 9)
                .expect("continuation without live voices");
            assert_eq!(
                next.values_at(0).expect("continuation start"),
                expected_start
            );
            assert_eq!(
                next.source_configuration_id(),
                if elapsed == 0 {
                    first.source_configuration_id()
                } else {
                    first.target_configuration_id()
                }
            );
            assert_eq!(
                next.values_at(9).expect("continuation endpoint"),
                c.feedback_parameters(FEEDBACK_A).expect("target values")
            );

            let mut new_voice = c
                .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_A)
                .expect("new voice");
            new_voice
                .enroll_in_retarget(&c, &next, 0)
                .expect("new note joins current shared value");
            let table = SineTable::new();
            let mut previous = [0.0; OPERATORS];
            for age in 0..9 {
                let parameters = next.values_at(age).expect("continuation sample values");
                let expected = oracle_frame(age as u64, u64::MAX, parameters, &mut previous);
                let actual = new_voice
                    .process_frame_with_retarget(&c, &table, &next, age)
                    .expect("new voice follows shared continuation");
                assert!(
                    (f64::from(actual) - f64::from(expected)).abs() < 0.000_15,
                    "elapsed={elapsed}, age={age}: {actual} != {expected}"
                );
            }
            assert_eq!(new_voice.age_frames(), 9);
        }
    }

    #[test]
    fn shared_ramp_continuation_refuses_stale_or_unsupported_targets() {
        let a = standard_config(LEVELS_A, CARRIERS_A);
        let b = standard_config(LEVELS_B, CARRIERS_B);
        let c = standard_config([0.3, 0.45, 0.85, 0.6], [0.2, 0.1, -0.5, 0.7]);
        let envelope = OperatorEnvelope::new(0.0, 1.0, 0.7, 0.0, 0.0, 8.0 / f64::from(RATE))
            .expect("valid envelope");
        let wrong_law = config(
            LEVELS_B,
            CARRIERS_B,
            MODULATION,
            [envelope; OPERATORS],
            [OperatorWaveform::Square; OPERATORS],
            [OperatorTuning::Ratio; OPERATORS],
            [0.0; OPERATORS],
        );
        let seed = a
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_A)
            .expect("seed voice");
        let ramp = seed
            .prepare_retarget(&a, &b, FEEDBACK_B, 20)
            .expect("first ramp");
        let before = ramp;

        assert!(
            ramp.continue_from_shared_ramp(&a, &c, FEEDBACK_A, 7, 9)
                .is_err()
        );
        assert!(
            ramp.continue_from_shared_ramp(&wrong_law, &c, FEEDBACK_A, 7, 9)
                .is_err()
        );
        assert!(
            ramp.continue_from_shared_ramp(&b, &c, FEEDBACK_A, 21, 9)
                .is_err()
        );
        assert!(
            ramp.continue_from_shared_ramp(&b, &c, [0.0; OPERATORS], 7, 9)
                .is_err()
        );
        assert_eq!(ramp, before);
    }

    #[test]
    fn fresh_voice_joins_the_global_ramp_at_zero_middle_and_endpoint() {
        let old = standard_config(LEVELS_A, CARRIERS_A);
        let target = standard_config(LEVELS_B, CARRIERS_B);
        let table = SineTable::new();
        let duration = 12;
        let carried = old
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_A)
            .expect("prepared carried voice");
        let ramp = carried
            .prepare_retarget(&old, &target, FEEDBACK_B, duration)
            .expect("prepared shared ramp");

        for elapsed in [0, 5, duration] {
            let mut fresh = target
                .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_B)
                .expect("prepared new voice");
            let before = fresh;
            let sample = fresh
                .process_frame_joining_retarget(&target, &table, &ramp, elapsed)
                .expect("new voice joins shared ramp");
            let expected_parameters = ramp.values_at(elapsed).expect("parameter boundary");
            let expected = oracle_frame(0, u64::MAX, expected_parameters, &mut [0.0; OPERATORS]);
            assert!(
                (f64::from(sample) - f64::from(expected)).abs() < 0.000_15,
                "elapsed={elapsed}: {sample} != {expected}"
            );
            assert_eq!(fresh.age_frames(), 1);
            assert_eq!(fresh.gate_frame(), None);
            if elapsed < duration - 1 {
                assert!(fresh.is_ramp_participant());
            } else {
                assert!(!fresh.is_ramp_participant());
            }
            assert_eq!(before.age_frames(), 0);
        }

        let join_frame = 5;
        let total = duration + 7;
        let target_parameters = target
            .feedback_parameters(FEEDBACK_B)
            .expect("target parameters");
        let mut expected = Vec::new();
        let mut previous_outputs = [0.0; OPERATORS];
        for elapsed in join_frame..total {
            let parameters = if elapsed < duration {
                ramp.values_at(elapsed).expect("global ramp boundary")
            } else {
                target_parameters
            };
            expected.push(oracle_frame(
                u64::from(elapsed - join_frame),
                u64::MAX,
                parameters,
                &mut previous_outputs,
            ));
        }
        for partition in [1, 4, 7] {
            let mut fresh = target
                .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_B)
                .expect("prepared partitioned new voice");
            let mut actual = Vec::new();
            let mut cursor = 0;
            while cursor < total {
                let end = (cursor + partition).min(total);
                for elapsed in cursor..end {
                    if elapsed < join_frame {
                        continue;
                    }
                    let sample = if elapsed == join_frame {
                        fresh.process_frame_joining_retarget(&target, &table, &ramp, elapsed)
                    } else if elapsed < duration {
                        fresh.process_frame_with_retarget(&target, &table, &ramp, elapsed)
                    } else {
                        fresh.process_frame(&target, &table)
                    }
                    .expect("partitioned joined voice");
                    actual.push(sample);
                }
                cursor = end;
            }
            assert_eq!(actual.len(), expected.len());
            for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
                assert!(
                    (f64::from(*actual) - f64::from(*expected)).abs() < 0.000_15,
                    "partition={partition}, age={index}: {actual} != {expected}"
                );
            }
        }
    }

    #[test]
    fn fresh_voice_join_refusal_preserves_age_gate_and_feedback_history() {
        let target = standard_config(LEVELS_B, CARRIERS_B);
        let other = standard_config(LEVELS_A, CARRIERS_A);
        let ramp_source = standard_config(LEVELS_A, CARRIERS_A)
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_A)
            .expect("prepared ramp source");
        let ramp = ramp_source
            .prepare_retarget(
                &standard_config(LEVELS_A, CARRIERS_A),
                &target,
                FEEDBACK_B,
                8,
            )
            .expect_err("a voice from another configuration is refused");
        let _ = ramp;
        let source = standard_config(LEVELS_A, CARRIERS_A);
        let source_voice = source
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_A)
            .expect("prepared voice");
        let valid_ramp = source_voice
            .prepare_retarget(&source, &target, FEEDBACK_B, 8)
            .expect("valid ramp");
        let mut fresh = target
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_B)
            .expect("fresh target voice");
        fresh.note_off().expect("make gate non-fresh");
        let before = fresh;
        assert!(
            fresh
                .process_frame_joining_retarget(&target, &SineTable::new(), &valid_ramp, 3)
                .is_err()
        );
        assert_eq!(fresh, before);

        let mut too_late = target
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_B)
            .expect("fresh target voice");
        let before = too_late;
        assert!(
            too_late
                .process_frame_joining_retarget(&target, &SineTable::new(), &valid_ramp, 9)
                .is_err()
        );
        assert_eq!(too_late, before);

        let mut wrong_target_voice = other
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_A)
            .expect("prepared unrelated voice");
        let before = wrong_target_voice;
        assert!(
            wrong_target_voice
                .process_frame_joining_retarget(&target, &SineTable::new(), &valid_ramp, 3)
                .is_err()
        );
        assert_eq!(wrong_target_voice, before);
    }

    #[test]
    fn state_only_enrollment_allows_same_offset_noteoff_at_age_zero() {
        let source = standard_config(LEVELS_A, CARRIERS_A);
        let target = standard_config(LEVELS_B, CARRIERS_B);
        let source_voice = source
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_A)
            .expect("prepared source voice");
        let ramp = source_voice
            .prepare_retarget(&source, &target, FEEDBACK_B, 11)
            .expect("prepared shared ramp");
        let mut fresh = target
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_B)
            .expect("prepared target voice");
        fresh
            .enroll_in_retarget(&target, &ramp, 4)
            .expect("enroll before applying later events");
        let before_reenroll = fresh;
        assert!(fresh.enroll_in_retarget(&target, &ramp, 4).is_err());
        assert_eq!(fresh, before_reenroll);
        fresh.note_off().expect("same-offset NoteOff");
        assert_eq!(fresh.gate_frame(), Some(0));
        assert_eq!(fresh.age_frames(), 0);

        let parameters = ramp.values_at(4).expect("current parameter boundary");
        let expected = oracle_frame(0, 0, parameters, &mut [0.0; OPERATORS]);
        let actual = fresh
            .process_frame_with_retarget(&target, &SineTable::new(), &ramp, 4)
            .expect("sample age-zero release");
        assert!((f64::from(actual) - f64::from(expected)).abs() < 0.000_15);
        assert_eq!(fresh.gate_frame(), Some(0));
        assert_eq!(fresh.age_frames(), 1);
    }

    #[test]
    fn exact_active_ramp_copy_rebinds_target_and_voice_without_changing_pcm() {
        let old = standard_config(LEVELS_A, CARRIERS_A);
        let target = standard_config(LEVELS_B, CARRIERS_B);
        let replacement = standard_config(LEVELS_B, CARRIERS_B);

        for elapsed in [0, 1, 6, 13] {
            let mut source_voice = old
                .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_A)
                .expect("fresh source voice");
            let source_ramp = source_voice
                .prepare_retarget(&old, &target, FEEDBACK_B, 13)
                .expect("fresh source ramp");
            for offset in 0..elapsed {
                source_voice
                    .process_frame_with_retarget(&target, &SineTable::new(), &source_ramp, offset)
                    .expect("advance source ramp");
            }
            let reference_target = standard_config(LEVELS_B, CARRIERS_B);
            let mut reference_voice = old
                .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_A)
                .expect("fresh reference voice");
            let reference_ramp = reference_voice
                .prepare_retarget(&old, &reference_target, FEEDBACK_B, 13)
                .expect("fresh reference ramp");
            for offset in 0..elapsed {
                reference_voice
                    .process_frame_with_retarget(
                        &reference_target,
                        &SineTable::new(),
                        &reference_ramp,
                        offset,
                    )
                    .expect("advance reference ramp");
            }
            let rebound_ramp = source_ramp
                .rebind_target_after_exact_history_copy(&target, &replacement, elapsed)
                .expect("rebind exact target");
            assert!(rebound_ramp.is_exact_target_rebind_of(
                &source_ramp,
                &target,
                &replacement,
                elapsed
            ));
            assert!(source_voice.can_rebind_after_exact_history_copy(
                &target,
                &replacement,
                Some((&source_ramp, &rebound_ramp, elapsed))
            ));
            source_voice
                .rebind_after_exact_history_copy_with_ramp(
                    &target,
                    &replacement,
                    Some((&source_ramp, &rebound_ramp, elapsed)),
                )
                .expect("rebind copied voice");
            if elapsed == 0 {
                assert_eq!(
                    source_voice.configuration_id(),
                    source_ramp.source_configuration_id()
                );
                assert!(!source_voice.is_ramp_participant());
            } else if elapsed < source_ramp.frames() {
                assert_eq!(
                    source_voice.configuration_id(),
                    replacement.configuration_id()
                );
                assert!(source_voice.is_ramp_participant());
            } else {
                assert_eq!(
                    source_voice.configuration_id(),
                    replacement.configuration_id()
                );
                assert!(!source_voice.is_ramp_participant());
            }

            for offset in elapsed..(source_ramp.frames() + 4) {
                let rebound_sample = if offset < source_ramp.frames() {
                    source_voice.process_frame_with_retarget(
                        &replacement,
                        &SineTable::new(),
                        &rebound_ramp,
                        offset,
                    )
                } else {
                    source_voice.process_frame(&replacement, &SineTable::new())
                }
                .expect("rebound sample");
                let original_sample = if offset < reference_ramp.frames() {
                    reference_voice.process_frame_with_retarget(
                        &reference_target,
                        &SineTable::new(),
                        &reference_ramp,
                        offset,
                    )
                } else {
                    reference_voice.process_frame(&reference_target, &SineTable::new())
                }
                .expect("original sample");
                assert_eq!(rebound_sample, original_sample, "offset={offset}");
            }
        }
    }

    #[test]
    fn same_identity_ramp_rebind_preserves_pre_first_sample_source_tag() {
        let source_target = standard_config(LEVELS_A, CARRIERS_A);
        let replacement = standard_config(LEVELS_A, CARRIERS_A);
        let mut voice = source_target
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_A)
            .expect("prepared voice");
        let ramp = voice
            .prepare_retarget(&source_target, &source_target, FEEDBACK_B, 9)
            .expect("same-configuration feedback-only ramp");
        assert_eq!(
            ramp.source_configuration_id(),
            ramp.target_configuration_id()
        );
        let rebound = ramp
            .rebind_target_after_exact_history_copy(&source_target, &replacement, 0)
            .expect("rebind same-identity ramp");
        assert!(voice.can_rebind_after_exact_history_copy(
            &source_target,
            &replacement,
            Some((&ramp, &rebound, 0))
        ));
        voice
            .rebind_after_exact_history_copy_with_ramp(
                &source_target,
                &replacement,
                Some((&ramp, &rebound, 0)),
            )
            .expect("rebind source-tagged voice");
        assert_eq!(voice.configuration_id(), ramp.source_configuration_id());
        assert!(!voice.is_ramp_participant());
        assert!(
            voice
                .process_frame_with_retarget(&replacement, &SineTable::new(), &rebound, 0)
                .is_ok()
        );
        assert!(voice.is_ramp_participant());
        assert_eq!(voice.configuration_id(), replacement.configuration_id());

        let same_identity_continuation = rebound
            .retarget(&voice, &replacement, &replacement, FEEDBACK_A, 1, 6)
            .expect("same-id feedback continuation");
        assert_eq!(
            same_identity_continuation.source_configuration_id(),
            same_identity_continuation.target_configuration_id()
        );
        let fresh_replacement = standard_config(LEVELS_A, CARRIERS_A);
        let rebound_continuation = same_identity_continuation
            .rebind_target_after_exact_history_copy(&replacement, &fresh_replacement, 0)
            .expect("same-id continuation target rebind");
        let source_token = voice.configuration_id();
        voice
            .rebind_after_exact_history_copy_with_ramp(
                &replacement,
                &fresh_replacement,
                Some((&same_identity_continuation, &rebound_continuation, 0)),
            )
            .expect("same-id participant rebind");
        assert_eq!(voice.configuration_id(), source_token);
        assert!(voice.is_ramp_participant());
        assert!(
            voice
                .process_frame_with_retarget(
                    &fresh_replacement,
                    &SineTable::new(),
                    &rebound_continuation,
                    0,
                )
                .is_ok()
        );
    }

    #[test]
    fn exact_copy_at_second_ramp_zero_preserves_prior_participant_token() {
        let a = standard_config(LEVELS_A, CARRIERS_A);
        let b = standard_config(LEVELS_B, CARRIERS_B);
        let c = standard_config([0.3, 0.45, 0.85, 0.6], [0.2, 0.1, -0.5, 0.7]);
        let replacement = standard_config([0.3, 0.45, 0.85, 0.6], [0.2, 0.1, -0.5, 0.7]);
        let table = SineTable::new();
        let mut voice = a
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_A)
            .expect("prepared voice");
        let first = voice
            .prepare_retarget(&a, &b, FEEDBACK_B, 20)
            .expect("first ramp");
        for elapsed in 0..7 {
            voice
                .process_frame_with_retarget(&b, &table, &first, elapsed)
                .expect("first ramp sample");
        }
        assert!(voice.is_ramp_participant());
        let second = first
            .retarget(&voice, &b, &c, [0.33, 0.0, 0.0, 0.71], 7, 9)
            .expect("second ramp at current boundary");
        assert!(voice.is_ramp_participant());
        assert_eq!(voice.configuration_id(), second.source_configuration_id());

        let rebound = second
            .rebind_target_after_exact_history_copy(&c, &replacement, 0)
            .expect("rebind second ramp before first sample");
        assert!(voice.can_rebind_after_exact_history_copy(
            &c,
            &replacement,
            Some((&second, &rebound, 0))
        ));
        let source_identity = voice.configuration_id();
        voice
            .rebind_after_exact_history_copy_with_ramp(
                &c,
                &replacement,
                Some((&second, &rebound, 0)),
            )
            .expect("rebind copied carried voice");
        assert!(voice.is_ramp_participant());
        assert_eq!(voice.configuration_id(), source_identity);

        let mut reference = a
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_A)
            .expect("reference voice");
        for elapsed in 0..7 {
            reference
                .process_frame_with_retarget(&b, &table, &first, elapsed)
                .expect("advance reference first ramp");
        }
        // Rebuild the second-ramp source boundary from the reference copy.
        let reference_second = first
            .retarget(&reference, &b, &c, [0.33, 0.0, 0.0, 0.71], 7, 9)
            .expect("reference second ramp");
        for elapsed in 0..reference_second.frames() {
            assert_eq!(
                voice
                    .process_frame_with_retarget(&replacement, &table, &rebound, elapsed)
                    .expect("rebound second ramp"),
                reference
                    .process_frame_with_retarget(&c, &table, &reference_second, elapsed)
                    .expect("reference second ramp")
            );
        }
    }

    #[test]
    fn exact_copy_at_zero_clock_rebinds_a_freshly_enrolled_target_voice() {
        let old = standard_config(LEVELS_A, CARRIERS_A);
        let target = standard_config(LEVELS_B, CARRIERS_B);
        let replacement = standard_config(LEVELS_B, CARRIERS_B);
        let source_voice = old
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_A)
            .expect("prepared source voice");
        let ramp = source_voice
            .prepare_retarget(&old, &target, FEEDBACK_B, 10)
            .expect("prepared shared ramp");
        let mut enrolled = target
            .prepare_feedback_voice(PITCH, VELOCITY, FEEDBACK_B)
            .expect("prepared target voice");
        enrolled
            .enroll_in_retarget(&target, &ramp, 0)
            .expect("enroll at current boundary");
        let rebound = ramp
            .rebind_target_after_exact_history_copy(&target, &replacement, 0)
            .expect("rebind active ramp target");
        assert!(enrolled.can_rebind_after_exact_history_copy(
            &target,
            &replacement,
            Some((&ramp, &rebound, 0))
        ));
        let mut copied = enrolled;
        copied
            .rebind_after_exact_history_copy_with_ramp(
                &target,
                &replacement,
                Some((&ramp, &rebound, 0)),
            )
            .expect("rebind target participant");
        assert!(copied.is_ramp_participant());
        assert_eq!(copied.configuration_id(), replacement.configuration_id());

        for elapsed in 0..(ramp.frames() + 3) {
            let source_sample = if elapsed < ramp.frames() {
                enrolled.process_frame_with_retarget(&target, &SineTable::new(), &ramp, elapsed)
            } else {
                enrolled.process_frame(&target, &SineTable::new())
            }
            .expect("original target voice");
            let copied_sample = if elapsed < ramp.frames() {
                copied.process_frame_with_retarget(
                    &replacement,
                    &SineTable::new(),
                    &rebound,
                    elapsed,
                )
            } else {
                copied.process_frame(&replacement, &SineTable::new())
            }
            .expect("rebound target voice");
            assert_eq!(copied_sample, source_sample, "elapsed={elapsed}");
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
