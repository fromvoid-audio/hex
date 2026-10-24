//! Control-only re-admission of normalized immutable MIDI metadata. No notes,
//! factories or device resources are prepared here. Queries keep the existing
//! bounded template contract; new timing receives a new density proof.
use super::*;
use std::mem::size_of;

impl PreparedMidiSourceTemplate {
    pub(crate) fn readmission_quote(&self) -> Result<(usize, usize), MidiTemplateError> {
        let edges = self
            .intro
            .len()
            .checked_add(self.repeats.len())
            .ok_or(MidiTemplateError::ArithmeticOverflow)?;
        let spans = self
            .intro_spans
            .len()
            .checked_add(self.repeat_spans.len())
            .ok_or(MidiTemplateError::ArithmeticOverflow)?;
        let retained = size_of::<Self>()
            .checked_add(
                edges
                    .checked_mul(size_of::<SourceEdge>())
                    .ok_or(MidiTemplateError::ArithmeticOverflow)?,
            )
            .and_then(|n| n.checked_add(spans.checked_mul(size_of::<SourceSpan>())?))
            .ok_or(MidiTemplateError::ArithmeticOverflow)?;
        // Both density inputs stay live while one repeat-phase array is used.
        let scratch = edges
            .checked_mul(size_of::<DensityEvent>())
            .and_then(|n| n.checked_add(self.repeats.len().checked_mul(size_of::<u64>())?))
            .ok_or(MidiTemplateError::ArithmeticOverflow)?;
        Ok((retained, scratch))
    }

    pub(crate) fn retained_readmission_bytes(&self) -> Option<usize> {
        size_of::<Self>()
            .checked_add(self.intro.capacity().checked_mul(size_of::<SourceEdge>())?)?
            .checked_add(
                self.repeats
                    .capacity()
                    .checked_mul(size_of::<SourceEdge>())?,
            )?
            .checked_add(
                self.intro_spans
                    .capacity()
                    .checked_mul(size_of::<SourceSpan>())?,
            )?
            .checked_add(
                self.repeat_spans
                    .capacity()
                    .checked_mul(size_of::<SourceSpan>())?,
            )
    }

    pub(crate) fn reprepare_for_timing_on_control(
        &self,
        max_micro_bpm: u64,
    ) -> Result<Self, MidiTemplateError> {
        if max_micro_bpm == 0 {
            return Err(MidiTemplateError::InvalidSource(
                "invalid midi admission timing",
            ));
        }
        if let Some(period) = self.period {
            require_positive_frame_advance(period, self.sample_rate, max_micro_bpm)
                .map_err(|_| MidiTemplateError::LoopPeriodTooShortForAdmission)?;
        }
        for span in self.intro_spans.iter().chain(&self.repeat_spans) {
            require_positive_frame_advance(
                span.gate_end - span.start,
                self.sample_rate,
                max_micro_bpm,
            )
            .map_err(|_| MidiTemplateError::GateTooShortForAdmission)?;
        }
        let mut intro_density = Vec::new();
        intro_density
            .try_reserve_exact(self.intro.len())
            .map_err(|_| MidiTemplateError::Allocation)?;
        intro_density.extend(self.intro.iter().map(|edge| DensityEvent {
            phase: edge.phase,
            onset: edge.is_onset(),
        }));
        let mut repeat_density = Vec::new();
        repeat_density
            .try_reserve_exact(self.repeats.len())
            .map_err(|_| MidiTemplateError::Allocation)?;
        repeat_density.extend(self.repeats.iter().map(|edge| DensityEvent {
            phase: edge.phase,
            onset: edge.is_onset(),
        }));
        let density = admit_density(
            &intro_density,
            &repeat_density,
            self.first_pass,
            self.period,
            conservative_window_raw(self.sample_rate, max_micro_bpm)?,
        )?;
        let result = Self {
            track: self.track,
            clip: self.clip,
            sample_rate: self.sample_rate,
            admitted_max_micro_bpm: max_micro_bpm,
            first_pass: self.first_pass,
            intro_iteration: self.intro_iteration,
            repeat_iteration: self.repeat_iteration,
            period: self.period,
            source: self.source,
            intro: readmission_copy(&self.intro)?,
            repeats: readmission_copy(&self.repeats)?,
            intro_spans: readmission_copy(&self.intro_spans)?,
            repeat_spans: readmission_copy(&self.repeat_spans)?,
            density,
        };
        if result
            .retained_readmission_bytes()
            .is_none_or(|bytes| bytes > self.readmission_quote().map_or(0, |quote| quote.0))
        {
            return Err(MidiTemplateError::Allocation);
        }
        Ok(result)
    }
}

fn readmission_copy<T: Copy>(source: &[T]) -> Result<Vec<T>, MidiTemplateError> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(source.len())
        .map_err(|_| MidiTemplateError::Allocation)?;
    result.extend_from_slice(source);
    Ok(result)
}
