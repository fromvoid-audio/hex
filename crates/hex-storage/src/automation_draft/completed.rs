//! Structurally completed draft custody and bounded performance-time predecessor queries.
//! Session must separately validate the continuous content/loop clock before interpreting a
//! completed source as a take. This owner performs file I/O only on control; it has no callback
//! query, cached event collection, content-time index or file-deletion responsibility.
use super::*;

/// Owns the existing locked source/file only after a genuine, undamaged Finished record.
/// Completion is structural: captured context is caller-supplied and is not live admission proof.
pub struct CompletedDraft {
    reader: DraftReader,
    end: DraftClock,
}

impl DraftReader {
    /// Refuse interrupted, aborted or damaged input without changing its retained bytes.
    /// The ordinary reader remains the recovery path for those drafts.
    pub fn into_completed(self) -> Result<CompletedDraft, DraftError> {
        let DraftOutcome::Finished(end) = self.outcome else {
            return Err(self.error(StorageError::Invalid(
                "automation draft has no undamaged finished terminal; retain it for recovery",
            )));
        };
        Ok(CompletedDraft { reader: self, end })
    }
}

impl CompletedDraft {
    pub fn open(path: impl AsRef<Path>, limits: DraftReadLimits) -> Result<Self, DraftError> {
        DraftReader::open(path, limits)?.into_completed()
    }
    pub fn source(&self) -> &Project {
        self.reader.source()
    }
    pub fn target(&self) -> DraftTarget {
        self.reader.target()
    }
    pub fn provenance(&self) -> DraftProvenance {
        self.reader.provenance()
    }
    pub fn capture_clock(&self) -> Option<CaptureClock> {
        self.reader.capture_clock()
    }
    pub fn end_clock(&self) -> DraftClock {
        self.end
    }
    pub fn event_count(&self) -> u64 {
        self.reader.progress().durable_events
    }
    pub fn path(&self) -> &Path {
        self.reader.path()
    }
    pub fn read_events(
        &mut self,
        first_ordinal: u64,
        output: &mut [Option<AppliedEvent>],
    ) -> Result<usize, DraftError> {
        self.reader.read_events(first_ordinal, output)
    }
    /// Read the last event at or before a performance frame. Equal-frame applications resolve
    /// to the greatest sequence, as validated by the source's strict record order. This takes
    /// at most logarithmic record queries plus one result read, using a single-event scratch
    /// buffer. It does not apply half-open take coverage or interpret a loop/content clock.
    pub fn event_at_or_before(
        &mut self,
        performance_frame: u64,
    ) -> Result<Option<AppliedEvent>, DraftError> {
        let mut left = 0;
        let mut right = self.event_count();
        while left < right {
            let middle = left + (right - left) / 2;
            let event = self.event(middle)?;
            if event.performance_frame <= performance_frame {
                left = middle + 1;
            } else {
                right = middle;
            }
        }
        left.checked_sub(1)
            .map(|ordinal| self.event(ordinal))
            .transpose()
    }
    fn event(&mut self, ordinal: u64) -> Result<AppliedEvent, DraftError> {
        let mut event = [None];
        self.reader.read_events(ordinal, &mut event)?;
        event[0].ok_or_else(|| {
            self.reader.error(StorageError::Invalid(
                "completed automation draft event is unavailable; retain it for recovery",
            ))
        })
    }
}
