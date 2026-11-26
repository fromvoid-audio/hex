//! Control-side admission and selection of instance-bound granular checkpoints.
use crate::{GranularSeekProgress, VerifiedGranularCursor, VerifiedGranularStepError};
use hex_media::{ResamplePlan, VerifiedWavReader};
use hex_warp::{Error, WarpCheckpoint};

pub(crate) const MAX_ENTRIES: usize = 64;

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
    checkpoint: WarpCheckpoint,
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
/// Pure zero-allocation victim selector over `(position, age)` pairs.
/// Small caches (`<= 4` entries) use pure LRU; larger caches use
/// gap-minimizing strided thinning across the timeline while protecting slot 0,
/// the furthest frontier position, and a small recent hot set so loop-start
/// anchors and the immediate tail are retained.
fn select_eviction_from_pairs(pairs: &[(u64, u64)]) -> Option<usize> {
    let len = pairs.len();
    if len <= 1 || len > MAX_ENTRIES {
        return None;
    }
    if len <= 4 {
        return (1..len).min_by_key(|&i| pairs[i].1);
    }
    let mut by_pos = [0_usize; MAX_ENTRIES];
    for (slot, dst) in by_pos.iter_mut().take(len).enumerate() {
        *dst = slot;
    }
    by_pos[..len].sort_unstable_by_key(|&i| pairs[i].0);

    let recent_protect = (len.saturating_sub(3)).min(4);
    let mut by_age = [0_usize; MAX_ENTRIES];
    let mut age_count = 0_usize;
    for i in 1..len {
        by_age[age_count] = i;
        age_count += 1;
    }
    by_age[..age_count].sort_unstable_by(|&a, &b| pairs[b].1.cmp(&pairs[a].1));
    let mut protected = [false; MAX_ENTRIES];
    protected[0] = true;
    protected[by_pos[len - 1]] = true;
    for &idx in by_age.iter().take(recent_protect) {
        protected[idx] = true;
    }

    let mut best: Option<(u64, u64, usize)> = None;
    for rank in 1..len - 1 {
        let idx = by_pos[rank];
        if protected[idx] {
            continue;
        }
        let prev_pos = pairs[by_pos[rank - 1]].0;
        let next_pos = pairs[by_pos[rank + 1]].0;
        let merged_gap = next_pos.saturating_sub(prev_pos);
        let candidate = (merged_gap, pairs[idx].1, idx);
        if best.is_none_or(|current| candidate < current) {
            best = Some(candidate);
        }
    }
    best.map(|(_, _, idx)| idx)
        .or_else(|| (1..len).min_by_key(|&i| pairs[i].1))
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
        let checkpoint = cursor.warp_checkpoint(remaining)?;
        entries.push(Entry { checkpoint, age: 0 });
        Ok(Self {
            entries,
            limit,
            budget,
            clock: 0,
        })
    }
    /// Initial state plus one reusable noninitial checkpoint; no allocation.
    pub(crate) fn minimum_recording_budget(&self) -> Option<usize> {
        self.entries
            .capacity()
            .checked_mul(std::mem::size_of::<Entry>())?
            .checked_add(
                self.entries
                    .first()?
                    .checkpoint
                    .backing_bytes()?
                    .checked_mul(2)?,
            )
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
    fn select_eviction_index(&self) -> Option<usize> {
        let len = self.entries.len();
        if len <= 1 || len > MAX_ENTRIES {
            return None;
        }
        let mut pairs = [(0_u64, 0_u64); MAX_ENTRIES];
        for (dst, entry) in pairs.iter_mut().zip(&self.entries) {
            *dst = (entry.checkpoint.position(), entry.age);
        }
        select_eviction_from_pairs(&pairs[..len])
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
            match cursor.warp_checkpoint(remaining) {
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
        let index = self.select_eviction_index().ok_or(Error::ScratchLimit)?;
        let total_bytes = self.backing_bytes().ok_or(Error::ScratchLimit)?;
        let evicted_bytes = self
            .entries
            .get(index)
            .and_then(|entry| entry.checkpoint.backing_bytes())
            .ok_or(Error::ScratchLimit)?;
        let other_bytes = total_bytes
            .checked_sub(evicted_bytes)
            .ok_or(Error::ScratchLimit)?;
        let slot_budget = self
            .budget
            .checked_sub(other_bytes)
            .ok_or(Error::ScratchLimit)?;
        let entry = self
            .entries
            .get_mut(index)
            .ok_or(Error::Invalid("invalid granular checkpoint cache"))?;
        let evicted_position = entry.checkpoint.position();
        cursor.refresh_checkpoint(&mut entry.checkpoint, slot_budget)?;
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
        let progress = cursor.begin_seek_warp(
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

#[cfg(test)]
mod tests {
    use super::select_eviction_from_pairs;

    /// Exercises `select_eviction_from_pairs` over 160 sequential 4,096-frame
    /// pages (> 13.6 s at 48 kHz, > 2.5x the 5.4 s pure-LRU cliff) with a
    /// 16-entry cache: every backward seek anywhere in the timeline must land
    /// within a small bounded replay distance instead of replaying from frame 0.
    #[test]
    fn strided_checkpoint_eviction_keeps_long_timeline_backward_seeks_bounded() {
        const LIMIT: usize = super::MAX_ENTRIES;
        const PAGE_FRAMES: u64 = 4_096;
        const TOTAL_PAGES: u64 = 256; // 1,048,576 frames (~21.8 s at 48 kHz, 4x past 5.4 s LRU cliff)

        let mut positions: Vec<(u64, u64)> = vec![(0, 0)];
        let mut clock = 0_u64;

        for page in 1..=TOTAL_PAGES {
            clock += 1;
            let pos = page * PAGE_FRAMES;
            if positions.len() < LIMIT {
                positions.push((pos, clock));
            } else {
                let victim = select_eviction_from_pairs(&positions).unwrap();
                positions[victim] = (pos, clock);
            }
        }

        // Check backward seeks across the entire 21.8 s timeline, including
        // positions well beyond the old 5.4 s pure-LRU cliff.
        let mut max_replay_frames = 0_u64;
        for probe_page in [10_u64, 30, 60, 80, 120, 160, 200, 240] {
            let target = probe_page * PAGE_FRAMES;
            let nearest = positions
                .iter()
                .map(|&(p, _)| p)
                .filter(|&p| p <= target)
                .max()
                .unwrap();
            assert!(
                nearest > 0,
                "backward seek to page {probe_page} fell back to frame 0"
            );
            max_replay_frames = max_replay_frames.max(target - nearest);
        }
        assert!(
            max_replay_frames <= 8 * PAGE_FRAMES,
            "expected bounded backward-seek replay <= 8 pages, got {max_replay_frames} frames"
        );
    }
}
