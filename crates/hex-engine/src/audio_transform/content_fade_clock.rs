//! Forward destination clock of the legacy envelope on directed content.
use crate::Invalid;
use hex_project::{AudioViewOccurrence, AudioViewProjection};
use std::ops::Range;

pub(super) struct DirectedFadeClock {
    consumed: i128,
    length: i128,
    bounds: (u64, u64),
}

impl DirectedFadeClock {
    pub(super) fn new(projection: &AudioViewProjection<'_>) -> Result<Self, Invalid> {
        let view = projection
            .clip()
            .audio_edit()
            .and_then(|edit| edit.content_view)
            .ok_or(Invalid("directed audio view missing"))?;
        let consumed = view
            .base_start_delta
            .checked_neg()
            .ok_or(Invalid("audio content fade displacement overflow"))?;
        Ok(Self {
            consumed,
            length: i128::from(view.window.length()?),
            bounds: projection.outer_frame_bounds(),
        })
    }

    pub(super) fn destination(
        &self,
        projection: &AudioViewProjection<'_>,
        occurrence: AudioViewOccurrence,
    ) -> Result<Range<u64>, Invalid> {
        let frame = |elapsed| -> Result<u64, Invalid> {
            let age = i128::from(elapsed)
                .checked_sub(self.consumed)
                .ok_or(Invalid("directed fade clock overflow"))?
                .clamp(0, self.length) as u64;
            Ok(projection
                .frame_at_elapsed(age)?
                .clamp(self.bounds.0, self.bounds.1))
        };
        Ok(frame(occurrence.elapsed_start)?..frame(occurrence.elapsed_end)?)
    }

    pub(super) fn root_sample(
        &self,
        projection: &AudioViewProjection<'_>,
        occurrence: AudioViewOccurrence,
        frame: u64,
    ) -> Result<u64, Invalid> {
        let elapsed = self
            .consumed
            .checked_add(i128::from(projection.elapsed_at_frame(frame)?))
            .and_then(|elapsed| u64::try_from(elapsed).ok())
            .ok_or(Invalid("directed fade clock overflow"))?;
        let base = projection.base_frame_at_elapsed(elapsed)?;
        let sample = occurrence
            .root_start
            .checked_add(base.saturating_sub(occurrence.destination_start))
            .ok_or(Invalid("directed fade sample overflow"))?;
        Ok(sample.clamp(occurrence.root_start, occurrence.root_end - 1))
    }
}
