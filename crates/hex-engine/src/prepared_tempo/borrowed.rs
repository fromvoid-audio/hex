//! The ordinary compiled Project already owns its canonical map. Borrow it
//! with the maximum tempo validated and cached during compilation; rendering
//! must not allocate another map or scan its markers to recover that bound.
use super::breaks::TempoRebaseBoundary;
use super::{BeatTime, MidiTempoBreaks, MidiTiming, PreparedTempoError, TempoBreaks, TempoMap};

#[derive(Clone, Copy)]
pub(crate) struct CanonicalMidiTiming<'a> {
    canonical: &'a TempoMap,
    max_micro_bpm: u32,
}
impl<'a> CanonicalMidiTiming<'a> {
    /// The map and cached bound must come from the same immutable compilation.
    pub(crate) fn for_validated(canonical: &'a TempoMap, max_micro_bpm: u32) -> Self {
        Self {
            canonical,
            max_micro_bpm,
        }
    }
}
impl MidiTiming for CanonicalMidiTiming<'_> {
    fn tempo_micro_bpm_at_frame(&self, frame: u64) -> Option<u32> {
        Some(
            self.canonical
                .tempo_at(self.canonical.frame_to_beat(frame).ok()?)
                .micro_bpm(),
        )
    }
    fn sample_rate(&self) -> u32 {
        self.canonical.sample_rate()
    }
    fn max_micro_bpm(&self) -> u64 {
        u64::from(self.max_micro_bpm)
    }
    fn beat_to_frame(&self, beat: BeatTime) -> Result<u64, PreparedTempoError> {
        self.canonical
            .beat_to_frame(beat)
            .map_err(|e| PreparedTempoError::BeatToFrame(e.0))
    }
    fn frame_to_beat(&self, frame: u64) -> Result<BeatTime, PreparedTempoError> {
        self.canonical
            .frame_to_beat(frame)
            .map_err(|e| PreparedTempoError::FrameToBeat(e.0))
    }
    fn beat_lower_bound_for_frame(&self, frame: u64) -> Result<u128, PreparedTempoError> {
        self.canonical
            .beat_lower_bound_for_frame(frame)
            .map_err(|e| PreparedTempoError::BeatLowerBound(e.0))
    }
}
impl MidiTempoBreaks for CanonicalMidiTiming<'_> {
    fn rebase_boundary(&self) -> Result<Option<TempoRebaseBoundary>, PreparedTempoError> {
        Ok(None)
    }
    fn tempo_breaks_around(&self, beat: BeatTime) -> Result<TempoBreaks, PreparedTempoError> {
        let points = self.canonical.points();
        let index = points.partition_point(|p| p.beat() <= beat);
        Ok(TempoBreaks {
            previous: index
                .checked_sub(1)
                .and_then(|i| points.get(i))
                .map(|p| p.beat()),
            next: points.get(index).map(|p| p.beat()),
        })
    }
    fn max_tempo_breaks_per_window(&self, frames: u64) -> Result<usize, PreparedTempoError> {
        let frames = usize::try_from(frames)
            .map_err(|_| PreparedTempoError::BeatToFrame("tempo boundary work budget overflow"))?;
        Ok(self.canonical.points().len().min(frames))
    }
}
