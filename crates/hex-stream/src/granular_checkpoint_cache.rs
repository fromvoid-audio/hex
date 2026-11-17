//! Control-side admission and selection of instance-bound granular checkpoints.
use crate::{GranularSeekProgress, VerifiedGranularCursor, VerifiedGranularStepError};
use hex_media::{ResamplePlan, VerifiedWavReader};
use hex_warp::{Error, GranularCheckpoint};

const MAX_ENTRIES: usize = 64;

/// Cache operations preserve the cursor's source and warp error details.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckpointCacheError {
    Warp(Error),
    Seek(VerifiedGranularStepError),
    NotCovered,
}
impl From<Error> for CheckpointCacheError {
    fn from(value: Error) -> Self {
        Self::Warp(value)
    }
}
impl From<VerifiedGranularStepError> for CheckpointCacheError {
    fn from(value: VerifiedGranularStepError) -> Self {
        Self::Seek(value)
    }
}

/// Effect of recording the current cursor position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckpointRecord {
    Existing,
    Added,
    Replaced { evicted_position: u64 },
}
struct Entry {
    checkpoint: GranularCheckpoint,
    age: u64,
}

/// Bounded, cursor-bound cache. The initial checkpoint is never evicted; other
/// slots use least-recently-used replacement. The caller determines when to
/// record, owns source invalidation and must budget reader, conversion and
/// other source allocations separately. No full seek coverage is implied.
pub struct VerifiedGranularCheckpointCache {
    entries: Vec<Entry>,
    limit: usize,
    budget: usize,
    clock: u64,
}
impl VerifiedGranularCheckpointCache {
    /// Capture a pristine cursor's starting state. The entry limit is 2..=64.
    /// `budget` includes retained entry storage and ring payloads, and excludes
    /// this inline value. Caller-owned source buffers and other memory require
    /// separate admission against the worker's aggregate limit.
    pub fn new(
        cursor: &VerifiedGranularCursor,
        limit: usize,
        budget: usize,
    ) -> Result<Self, CheckpointCacheError> {
        if !cursor.is_pristine() || !(2..=MAX_ENTRIES).contains(&limit) {
            return Err(Error::Invalid("invalid granular checkpoint cache").into());
        }
        if cursor
            .checkpoint_storage_limit()
            .is_none_or(|available| budget > available)
        {
            return Err(Error::ScratchLimit.into());
        }
        let requested = limit
            .checked_mul(std::mem::size_of::<Entry>())
            .ok_or(Error::ScratchLimit)?;
        if requested > budget {
            return Err(Error::ScratchLimit.into());
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(limit)
            .map_err(|_| Error::Allocation)?;
        let vector_bytes = entries
            .capacity()
            .checked_mul(std::mem::size_of::<Entry>())
            .ok_or(Error::ScratchLimit)?;
        let remaining = budget
            .checked_sub(vector_bytes)
            .ok_or(Error::ScratchLimit)?;
        let checkpoint = cursor.checkpoint(remaining)?;
        entries.push(Entry { checkpoint, age: 0 });
        Ok(Self {
            entries,
            limit,
            budget,
            clock: 0,
        })
    }
    pub(crate) fn reserved_budget(&self) -> usize {
        self.budget
    }
    /// Actual retained checkpoint and vector payload; excludes this inline value.
    pub fn backing_bytes(&self) -> Option<usize> {
        self.entries.iter().try_fold(
            self.entries
                .capacity()
                .checked_mul(std::mem::size_of::<Entry>())?,
            |n, entry| n.checked_add(entry.checkpoint.backing_bytes()?),
        )
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    fn check_cursor(&self, cursor: &VerifiedGranularCursor) -> Result<(), CheckpointCacheError> {
        let first = self
            .entries
            .first()
            .ok_or(Error::Invalid("empty granular checkpoint cache"))?;
        if !cursor.owns_checkpoint(&first.checkpoint) {
            return Err(Error::Invalid("foreign granular cursor").into());
        }
        Ok(())
    }
    /// Record one position, reusing an old unpinned slot when at capacity or
    /// when no new snapshot fits. Failure leaves existing entries unchanged.
    pub fn record(
        &mut self,
        cursor: &VerifiedGranularCursor,
    ) -> Result<CheckpointRecord, CheckpointCacheError> {
        self.check_cursor(cursor)?;
        if cursor.seek_target().is_some() || cursor.is_poisoned() {
            return Err(Error::Invalid("granular cursor is not ready for checkpoint").into());
        }
        self.clock = self.clock.saturating_add(1);
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.checkpoint.position() == cursor.position())
        {
            entry.age = self.clock;
            return Ok(CheckpointRecord::Existing);
        }
        if self.entries.len() < self.limit {
            let remaining = self
                .budget
                .checked_sub(self.backing_bytes().ok_or(Error::ScratchLimit)?)
                .ok_or(Error::ScratchLimit)?;
            match cursor.checkpoint(remaining) {
                Ok(checkpoint) => {
                    self.entries.push(Entry {
                        checkpoint,
                        age: self.clock,
                    });
                    return Ok(CheckpointRecord::Added);
                }
                Err(Error::ScratchLimit) if self.entries.len() > 1 => {}
                Err(error) => return Err(error.into()),
            }
        }
        let index = (1..self.entries.len())
            .min_by_key(|&i| self.entries[i].age)
            .ok_or(Error::ScratchLimit)?;
        let entry = self
            .entries
            .get_mut(index)
            .ok_or(Error::Invalid("invalid granular checkpoint cache"))?;
        let evicted_position = entry.checkpoint.position();
        cursor.refresh_checkpoint(&mut entry.checkpoint)?;
        entry.age = self.clock;
        Ok(CheckpointRecord::Replaced { evicted_position })
    }
    /// Restore the nearest retained checkpoint at or before `target`, within
    /// the caller's preroll bound. Uncovered targets are explicitly refused.
    /// Reader checks and all replay/cancellation semantics remain with cursor.
    pub fn begin_seek(
        &mut self,
        cursor: &mut VerifiedGranularCursor,
        target: u64,
        max_preroll_frames: u64,
        reader: &mut VerifiedWavReader,
        plan: &ResamplePlan,
        cancelled: impl FnMut() -> bool,
    ) -> Result<GranularSeekProgress, CheckpointCacheError> {
        self.check_cursor(cursor)?;
        if max_preroll_frames > crate::MAX_GRANULAR_PREROLL_FRAMES {
            return Err(Error::Invalid("granular seek exceeds bounded preroll").into());
        }
        let index = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.checkpoint.position() <= target)
            .max_by_key(|(_, e)| e.checkpoint.position())
            .map(|(i, _)| i)
            .ok_or(CheckpointCacheError::NotCovered)?;
        let checkpoint = &self.entries[index].checkpoint;
        if target - checkpoint.position() > max_preroll_frames {
            return Err(CheckpointCacheError::NotCovered);
        }
        let progress = cursor.begin_seek(
            checkpoint,
            target,
            max_preroll_frames,
            reader,
            plan,
            cancelled,
        )?;
        self.clock = self.clock.saturating_add(1);
        self.entries[index].age = self.clock;
        Ok(progress)
    }
    /// Start one bounded segment toward a distant target, selecting the latest
    /// retained checkpoint. This does not promise a deadline for the full replay.
    pub(crate) fn begin_seek_toward(
        &mut self,
        cursor: &mut VerifiedGranularCursor,
        target: u64,
        segment_frames: u64,
        reader: &mut VerifiedWavReader,
        plan: &ResamplePlan,
        cancelled: impl FnMut() -> bool,
    ) -> Result<GranularSeekProgress, CheckpointCacheError> {
        self.check_cursor(cursor)?;
        if segment_frames == 0 || segment_frames > crate::MAX_GRANULAR_PREROLL_FRAMES {
            return Err(Error::Invalid("invalid granular replay segment").into());
        }
        let position = self
            .entries
            .iter()
            .map(|entry| entry.checkpoint.position())
            .filter(|&position| position <= target)
            .max()
            .ok_or(CheckpointCacheError::NotCovered)?;
        let waypoint = position.saturating_add(segment_frames).min(target);
        self.begin_seek(cursor, waypoint, segment_frames, reader, plan, cancelled)
    }
}
