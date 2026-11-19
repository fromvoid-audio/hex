//! Bounded, control-owned display summaries of prepared audio.
use super::budget::{Budget, Reservation};
use crate::{Frame, Invalid, PreparedAudioPeak};
use hex_project::{BeatTime, TempoMap, audio_projection_frame};
use std::sync::{Arc, OnceLock};

const EVENT_SOURCE_BINS: usize = 1_024;
const EVENT_SOURCE_PEAK_BYTES: usize = 8 * 1024 * 1024;

pub(super) fn event_source_budget() -> &'static Arc<Budget> {
    static BUDGET: OnceLock<Arc<Budget>> = OnceLock::new();
    BUDGET.get_or_init(|| Arc::new(Budget::new(EVENT_SOURCE_PEAK_BYTES)))
}

/// Stereo extrema of the transformed source before event composition, event
/// gain, and any container envelope. Frames are in the processing root clock.
#[derive(Debug)]
pub struct PreparedEventSourcePeaks {
    frames: usize,
    bins: Vec<PreparedAudioPeak>,
    musical_clock: Option<PreparedMusicalEventSourceClock>,
    _reservation: Reservation,
}

/// Clock used by the warped PCM before event composition. The source may
/// retain a different origin from the destination after a signed event move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedMusicalEventSourceClock {
    tempo: Arc<TempoMap>,
    origin: i128,
    first_frame: i128,
    length: BeatTime,
}

impl PreparedMusicalEventSourceClock {
    pub(super) fn new(
        tempo: Arc<TempoMap>,
        origin: i128,
        first_frame: i128,
        length: BeatTime,
    ) -> Self {
        Self {
            tempo,
            origin,
            first_frame,
            length,
        }
    }

    /// Resolve a source beat boundary to a peak-relative PCM frame, including
    /// the exclusive end. Uses the same signed projection as event playback.
    pub fn frame_at(&self, position: BeatTime) -> Result<u64, Invalid> {
        if position > self.length {
            return Err(Invalid(
                "audio event source position exceeds prepared clock",
            ));
        }
        let absolute = self
            .origin
            .checked_add(i128::from(position.raw()))
            .ok_or(Invalid("audio event source beat overflow"))?;
        let frame =
            audio_projection_frame(&self.tempo, absolute).map_err(|error| Invalid(error.0))?;
        u64::try_from(frame - self.first_frame)
            .map_err(|_| Invalid("audio event source frame overflow"))
    }
}

impl PreparedEventSourcePeaks {
    pub(super) fn capture(
        frames: &[Frame],
        musical_clock: Option<PreparedMusicalEventSourceClock>,
        budget: &Arc<Budget>,
        mut cancelled: impl FnMut() -> bool,
    ) -> Option<Arc<Self>> {
        if frames.is_empty()
            || frames.len() as u64 > hex_project::MAX_AUDIO_DOMAIN_FRAMES
            || cancelled()
        {
            return None;
        }
        let count = EVENT_SOURCE_BINS.min(frames.len());
        let reservation = budget.reserve(Self::storage_bytes(count)).ok()?;
        let mut bins = PeakBins::new(frames.len(), count)?;
        for (index, frame) in frames.iter().copied().enumerate() {
            if index % 4_096 == 0 && cancelled() {
                return None;
            }
            bins.observe(index, frame);
        }
        Some(Arc::new(Self {
            frames: frames.len(),
            bins: bins.into_peaks(),
            musical_clock,
            _reservation: reservation,
        }))
    }

    fn storage_bytes(count: usize) -> usize {
        count * std::mem::size_of::<PreparedAudioPeak>()
            + std::mem::size_of::<Self>()
            + 2 * std::mem::size_of::<usize>()
    }

    pub fn source_frames(&self) -> usize {
        self.frames
    }

    /// Frame-clock sources have no beat projection.
    pub fn musical_clock(&self) -> Option<&PreparedMusicalEventSourceClock> {
        self.musical_clock.as_ref()
    }

    /// Exact extrema per bin. The first `source_frames() % bins().len()` bins
    /// each cover one more frame than the others.
    pub fn bins(&self) -> &[PreparedAudioPeak] {
        &self.bins
    }
}

pub(super) struct PeakBins {
    peaks: Vec<PreparedAudioPeak>,
    end: usize,
    width: usize,
    extra: usize,
}

impl PeakBins {
    pub(super) fn into_peaks(self) -> Vec<PreparedAudioPeak> {
        self.peaks
    }

    pub(super) fn new(frames: usize, limit: usize) -> Option<Self> {
        let count = limit.min(frames);
        if count == 0 {
            return None;
        }
        let mut peaks = Vec::new();
        peaks.try_reserve_exact(count).ok()?;
        Some(Self {
            peaks,
            end: 0,
            width: frames / count,
            extra: frames % count,
        })
    }

    pub(super) fn observe(&mut self, index: usize, sample: Frame) {
        if index == self.end {
            self.peaks.push(PreparedAudioPeak {
                min: sample,
                max: sample,
            });
            self.end += self.width + usize::from(self.peaks.len() <= self.extra);
        } else if let Some(peak) = self.peaks.last_mut() {
            for (channel, value) in sample.into_iter().enumerate() {
                peak.min[channel] = peak.min[channel].min(value);
                peak.max[channel] = peak.max[channel].max(value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_source_summary_releases_its_independent_budget_on_cancel_and_drop() {
        let frames = [[0.125, -0.5]; 128];
        let bytes = PreparedEventSourcePeaks::storage_bytes(frames.len());
        let budget = Arc::new(Budget::new(bytes));
        let mut polls = 0;
        assert!(
            PreparedEventSourcePeaks::capture(&frames, None, &budget, || {
                polls += 1;
                polls == 2
            })
            .is_none()
        );
        assert_eq!(budget.used(), 0);
        let summary = PreparedEventSourcePeaks::capture(&frames, None, &budget, || false).unwrap();
        assert_eq!(budget.used(), bytes);
        assert!(PreparedEventSourcePeaks::capture(&frames, None, &budget, || false).is_none());
        let retained = Arc::clone(&summary);
        drop(summary);
        assert_eq!(budget.used(), bytes);
        drop(retained);
        assert_eq!(budget.used(), 0);
    }
}
