use super::*;

pub(super) fn allocate_ring(capacity: usize) -> Result<Box<[f32]>, DeviceContractError> {
    let mut samples = Vec::new();
    samples
        .try_reserve_exact(capacity)
        .map_err(|_| DeviceContractError::RuntimeAllocation)?;
    samples.resize(capacity, 0.0);
    Ok(samples.into_boxed_slice())
}
/// Paired committed/speculative delay ring with O(written frames) commit and
/// rollback. Between closed transactions, `committed` and `scratch` remain
/// identical at every slot.
pub(super) struct RingPair {
    pub(super) committed: Box<[f32]>,
    pub(super) scratch: Box<[f32]>,
    pub(super) committed_cursor: usize,
    pub(super) committed_filled: bool,
    pub(super) cursor: usize,
    pub(super) filled: bool,
    pub(super) dirty_start: usize,
    pub(super) dirty_len: usize,
    pub(super) cleared_in_block: bool,
    pub(super) post_clear_writes: usize,
    pub(super) post_clear_max: usize,
}
impl RingPair {
    pub(super) fn try_new(capacity: usize) -> Result<Self, DeviceContractError> {
        Ok(Self {
            committed: allocate_ring(capacity)?,
            scratch: allocate_ring(capacity)?,
            committed_cursor: 0,
            committed_filled: false,
            cursor: 0,
            filled: false,
            dirty_start: 0,
            dirty_len: 0,
            cleared_in_block: false,
            post_clear_writes: 0,
            post_clear_max: 0,
        })
    }
    pub(super) fn copy_interval(
        source: &[f32],
        destination: &mut [f32],
        start: usize,
        length: usize,
    ) {
        if length == 0 {
            return;
        }
        let first = length.min(source.len() - start);
        destination[start..start + first].copy_from_slice(&source[start..start + first]);
        let rest = length - first;
        if rest > 0 {
            destination[..rest].copy_from_slice(&source[..rest]);
        }
    }
    pub(super) fn begin_block(&mut self) {
        self.dirty_start = self.cursor;
        self.dirty_len = 0;
        self.cleared_in_block = false;
        self.post_clear_writes = 0;
        self.post_clear_max = 0;
    }
    pub(super) fn read<const MATURE: bool>(&self, delay: f32) -> f32 {
        let delay = delay.clamp(1.0, (self.scratch.len() - 2) as f32);
        let whole = delay.floor() as usize;
        let fraction = delay - whole as f32;
        let length = self.scratch.len();
        let unwrapped = self.cursor + length - whole;
        // The delay clamp and the ring cursor bound permit a single wrap.
        let a = if unwrapped >= length {
            unwrapped - length
        } else {
            unwrapped
        };
        let b = if a == 0 { length - 1 } else { a - 1 };
        let (newer, older) = if MATURE || self.filled || whole < self.cursor {
            (self.scratch[a], self.scratch[b])
        } else if whole == self.cursor {
            (self.scratch[a], 0.0)
        } else {
            (0.0, 0.0)
        };
        newer + fraction * (older - newer)
    }
    pub(super) fn push(&mut self, value: f32) {
        let length = self.scratch.len();
        self.scratch[self.cursor] = clean(value);
        self.cursor += 1;
        if self.cursor == length {
            self.cursor = 0;
            self.filled = true;
        }
        if self.cleared_in_block {
            self.post_clear_writes = self.post_clear_writes.saturating_add(1).min(length);
            self.post_clear_max = self.post_clear_max.max(self.post_clear_writes);
        } else {
            self.dirty_len = self.dirty_len.saturating_add(1).min(length);
        }
    }
    pub(super) fn clear(&mut self) {
        self.cursor = 0;
        self.filled = false;
        self.cleared_in_block = true;
        self.post_clear_writes = 0;
    }
    pub(super) fn commit(&mut self) {
        Self::copy_interval(
            &self.scratch,
            &mut self.committed,
            self.dirty_start,
            self.dirty_len,
        );
        if self.post_clear_max > 0 {
            self.committed[..self.post_clear_max]
                .copy_from_slice(&self.scratch[..self.post_clear_max]);
        }
        self.committed_cursor = self.cursor;
        self.committed_filled = self.filled;
        self.dirty_start = self.cursor;
        self.dirty_len = 0;
        self.cleared_in_block = false;
        self.post_clear_writes = 0;
        self.post_clear_max = 0;
    }
    pub(super) fn rollback(&mut self) {
        Self::copy_interval(
            &self.committed,
            &mut self.scratch,
            self.dirty_start,
            self.dirty_len,
        );
        if self.post_clear_max > 0 {
            self.scratch[..self.post_clear_max]
                .copy_from_slice(&self.committed[..self.post_clear_max]);
        }
        self.cursor = self.committed_cursor;
        self.filled = self.committed_filled;
        self.dirty_start = self.cursor;
        self.dirty_len = 0;
        self.cleared_in_block = false;
        self.post_clear_writes = 0;
        self.post_clear_max = 0;
    }
    pub(super) fn reset(&mut self) {
        self.committed_cursor = 0;
        self.committed_filled = false;
        self.cursor = 0;
        self.filled = false;
        self.dirty_start = 0;
        self.dirty_len = 0;
        self.cleared_in_block = false;
        self.post_clear_writes = 0;
        self.post_clear_max = 0;
    }
    pub(super) fn copy_committed_from(&mut self, source: &Self) {
        let valid = if source.committed_filled {
            source.committed.len()
        } else {
            source.committed_cursor
        };
        self.committed[..valid].copy_from_slice(&source.committed[..valid]);
        self.scratch[..valid].copy_from_slice(&source.committed[..valid]);
        self.committed_cursor = source.committed_cursor;
        self.committed_filled = source.committed_filled;
        self.cursor = source.committed_cursor;
        self.filled = source.committed_filled;
        self.dirty_start = self.cursor;
        self.dirty_len = 0;
        self.cleared_in_block = false;
        self.post_clear_writes = 0;
        self.post_clear_max = 0;
    }
    pub(super) fn swap_committed_from(&mut self, source: &mut Self) {
        std::mem::swap(&mut self.committed, &mut source.committed);
        std::mem::swap(&mut self.scratch, &mut source.scratch);
        self.committed_cursor = source.committed_cursor;
        self.committed_filled = source.committed_filled;
        self.cursor = source.committed_cursor;
        self.filled = source.committed_filled;
        self.dirty_start = self.cursor;
        self.dirty_len = 0;
        self.cleared_in_block = false;
        self.post_clear_writes = 0;
        self.post_clear_max = 0;
        source.reset();
    }
}
pub(super) fn clean(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        hex_dsp::flush_quiet_f32(value).clamp(-32.0, 32.0)
    }
}
