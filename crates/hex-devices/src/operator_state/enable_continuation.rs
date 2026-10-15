//! Provider-defined enable-only continuation for immutable Operator contexts.
use super::*;
use hex_lattice::{
    ANALYTIC_CONTINUATION_WORDS, AnalyticContinuation, AnalyticContinuationError,
    AnalyticContinuationFormat, AnalyticContinuationId, AnalyticParameterFrame,
    PreparedAnalyticContinuation,
};
use std::any::Any;

impl PreparedOperatorContext {
    fn continuation_levels(
        &self,
        target: AnalyticContinuationId,
        frame: &AnalyticParameterFrame,
    ) -> Result<[f64; OPERATORS], AnalyticContinuationError> {
        if frame.target() != target {
            return Err(AnalyticContinuationError::WrongTarget);
        }
        let levels = std::array::from_fn(|index| f64::from_bits(frame.words()[index]));
        if levels
            .iter()
            .any(|level| !level.is_finite() || !(0.0..=8.0).contains(level))
            || frame.words()[OPERATORS..].iter().any(|word| *word != 0)
        {
            return Err(AnalyticContinuationError::InvalidParameters);
        }
        Ok(levels)
    }

    fn continuation_frame(
        target: AnalyticContinuationId,
        levels: [f64; OPERATORS],
    ) -> AnalyticParameterFrame {
        let mut words = [0; ANALYTIC_CONTINUATION_WORDS];
        for (word, level) in words.iter_mut().zip(levels) {
            *word = level.to_bits();
        }
        AnalyticParameterFrame::new(target, words)
    }

    fn continuation_note(
        &self,
        note: &PreparedSharedAnalyticNote,
    ) -> Result<PreparedOperatorNote, AnalyticContinuationError> {
        let decoded = PreparedOperatorNote::from_shared_payload(note.payload())
            .ok_or(AnalyticContinuationError::InvalidNote)?;
        if note.release_frames() != self.configuration.release_frames()
            || !self.configuration.note_matches_configuration(&decoded)
        {
            return Err(AnalyticContinuationError::InvalidNote);
        }
        Ok(decoded)
    }
}

impl PreparedAnalyticContinuation for PreparedOperatorContext {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn format(&self) -> AnalyticContinuationFormat {
        self.continuation_format
    }

    fn endpoint(&self, target: AnalyticContinuationId) -> AnalyticParameterFrame {
        Self::continuation_frame(target, self.effective_levels)
    }

    fn prepare_start(
        &self,
        target: AnalyticContinuationId,
        source: &dyn PreparedAnalyticContinuation,
        source_target: AnalyticContinuationId,
        source_boundary: &AnalyticParameterFrame,
    ) -> Result<AnalyticParameterFrame, AnalyticContinuationError> {
        let source = source
            .as_any()
            .downcast_ref::<Self>()
            .ok_or(AnalyticContinuationError::IncompatibleSource)?;
        // Compare complete authored state, including future append-only laws.
        // Effective endpoints alone cannot distinguish a toggle from a level edit.
        let current = self.authored_state.payload();
        let previous = source.authored_state.payload();
        if self.authored_state.state_version() != source.authored_state.state_version()
            || current.len() != previous.len()
            || current.get(..LFO_PAYLOAD_BYTES) != previous.get(..LFO_PAYLOAD_BYTES)
            || current.get(LFO_PAYLOAD_BYTES + enable::BYTES..)
                != previous.get(LFO_PAYLOAD_BYTES + enable::BYTES..)
            || !self
                .configuration
                .retarget_compatible_with(&source.configuration)
        {
            return Err(AnalyticContinuationError::IncompatibleSource);
        }
        let levels = source.continuation_levels(source_target, source_boundary)?;
        Ok(Self::continuation_frame(target, levels))
    }

    fn validate_note(
        &self,
        note: &PreparedSharedAnalyticNote,
    ) -> Result<(), AnalyticContinuationError> {
        self.continuation_note(note).map(|_| ())
    }

    fn parameters_at(
        &self,
        target: AnalyticContinuationId,
        transition: &AnalyticContinuation,
        performance_frame: u64,
    ) -> Result<AnalyticParameterFrame, AnalyticContinuationError> {
        let start = self.continuation_levels(target, transition.start())?;
        let elapsed = transition.elapsed_at(performance_frame)?;
        if elapsed == transition.duration() {
            return Ok(self.endpoint(target));
        }
        if elapsed == 0 {
            return Ok(*transition.start());
        }
        let amount = f64::from(elapsed) / f64::from(transition.duration());
        let levels = std::array::from_fn(|index| {
            start[index] + (self.effective_levels[index] - start[index]) * amount
        });
        Ok(Self::continuation_frame(target, levels))
    }

    fn sample_with_parameters(
        &self,
        target: AnalyticContinuationId,
        parameters: &AnalyticParameterFrame,
        note: &PreparedSharedAnalyticNote,
        age_frames: u64,
        gate_frames: u64,
    ) -> Result<StereoFrame, AnalyticContinuationError> {
        let levels = self.continuation_levels(target, parameters)?;
        let note = self.continuation_note(note)?;
        // Preserve the ordinary endpoint's established arithmetic exactly.
        let sample = if levels == self.effective_levels {
            self.configuration
                .sample(self.sine_table, &note, age_frames, gate_frames)
        } else {
            self.configuration
                .sample_with_oscillator_levels(
                    self.sine_table,
                    &note,
                    age_frames,
                    gate_frames,
                    levels,
                )
                .map_err(|_| AnalyticContinuationError::InvalidParameters)?
        };
        Ok([sample, sample])
    }
}
