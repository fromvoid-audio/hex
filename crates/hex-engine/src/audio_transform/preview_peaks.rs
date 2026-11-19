//! Bounded, control-owned display summaries of prepared audio.
use super::budget::{Budget, Reservation};
use crate::{Frame, PreparedAudioPeak};
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
    _reservation: Reservation,
}

impl PreparedEventSourcePeaks {
    pub(super) fn capture(
        frames: &[Frame],
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

    /// Exact extrema per bin. The first `source_frames() % bins().len()` bins
    /// each cover one more frame than the others.
    pub fn bins(&self) -> &[PreparedAudioPeak] {
        &self.bins
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
            PreparedEventSourcePeaks::capture(&frames, &budget, || {
                polls += 1;
                polls == 2
            })
            .is_none()
        );
        assert_eq!(budget.used(), 0);
        let summary = PreparedEventSourcePeaks::capture(&frames, &budget, || false).unwrap();
        assert_eq!(budget.used(), bytes);
        assert!(PreparedEventSourcePeaks::capture(&frames, &budget, || false).is_none());
        let retained = Arc::clone(&summary);
        drop(summary);
        assert_eq!(budget.used(), bytes);
        drop(retained);
        assert_eq!(budget.used(), 0);
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
