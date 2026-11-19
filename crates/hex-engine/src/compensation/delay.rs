use super::*;

#[derive(Clone, Copy, Debug, Default)]
struct Cursor {
    next: usize,
    valid: usize,
}

/// Both banks are identical outside a transaction. Writes form one circular
/// interval regardless of block partitioning; after a full turn it is the whole
/// ring. Finishing therefore needs at most two slice copies and no checkpoints.
#[derive(Debug)]
pub(super) struct RingPair<T> {
    committed: Vec<T>,
    scratch: Vec<T>,
    committed_cursor: Cursor,
    scratch_cursor: Cursor,
    dirty_start: usize,
    dirty_len: usize,
}

impl<T: Copy + Default> RingPair<T> {
    pub(super) fn new(capacity: usize) -> Result<Self, CompensationError> {
        let mut committed = reserved(capacity)?;
        let mut scratch = reserved(capacity)?;
        committed.resize(capacity, T::default());
        scratch.resize(capacity, T::default());
        Ok(Self {
            committed,
            scratch,
            committed_cursor: Cursor::default(),
            scratch_cursor: Cursor::default(),
            dirty_start: 0,
            dirty_len: 0,
        })
    }

    pub(super) fn storage_bytes(&self) -> Result<usize, CompensationError> {
        let Self {
            committed,
            scratch,
            committed_cursor: _,
            scratch_cursor: _,
            dirty_start: _,
            dirty_len: _,
        } = self;
        add_bytes(vector_bytes(committed)?, vector_bytes(scratch)?)
    }

    pub(super) fn begin(&mut self) {
        self.dirty_start = self.scratch_cursor.next;
        self.dirty_len = 0;
    }

    pub(super) fn push(&mut self, value: T) -> T {
        let capacity = self.scratch.len();
        if capacity == 0 {
            return value;
        }
        let slot = &mut self.scratch[self.scratch_cursor.next];
        let previous = *slot;
        *slot = value;
        self.scratch_cursor.next += 1;
        if self.scratch_cursor.next == capacity {
            self.scratch_cursor.next = 0;
        }
        self.scratch_cursor.valid = self.scratch_cursor.valid.saturating_add(1).min(capacity);
        self.dirty_len = self.dirty_len.saturating_add(1).min(capacity);
        previous
    }

    pub(super) fn valid_len(&self) -> usize {
        self.scratch_cursor.valid
    }

    pub(super) fn at_delay(&self, delay: usize) -> Option<T> {
        self.at_delay_ref(delay).copied()
    }

    pub(super) fn at_delay_ref(&self, delay: usize) -> Option<&T> {
        if delay >= self.scratch_cursor.valid {
            return None;
        }
        let capacity = self.scratch.len();
        let index = if self.scratch_cursor.next > delay {
            self.scratch_cursor.next - delay - 1
        } else {
            capacity - (delay + 1 - self.scratch_cursor.next)
        };
        self.scratch.get(index)
    }

    fn copy_dirty(source: &[T], destination: &mut [T], start: usize, len: usize) {
        let first = len.min(source.len() - start);
        destination[start..start + first].copy_from_slice(&source[start..start + first]);
        let second = len - first;
        destination[..second].copy_from_slice(&source[..second]);
    }

    pub(super) fn commit(&mut self) {
        Self::copy_dirty(
            &self.scratch,
            &mut self.committed,
            self.dirty_start,
            self.dirty_len,
        );
        self.committed_cursor = self.scratch_cursor;
        self.dirty_len = 0;
    }

    pub(super) fn rollback(&mut self) {
        Self::copy_dirty(
            &self.committed,
            &mut self.scratch,
            self.dirty_start,
            self.dirty_len,
        );
        self.scratch_cursor = self.committed_cursor;
        self.dirty_len = 0;
    }

    pub(super) fn capacity(&self) -> usize {
        self.committed.len()
    }
    pub(super) fn copy_history_from(&mut self, source: &Self) {
        self.committed.fill(T::default());
        self.scratch.fill(T::default());
        let valid = source.committed_cursor.valid.min(self.committed.len());
        for index in 0..valid {
            let delay = valid - 1 - index;
            let source_index = (source.committed_cursor.next + source.committed.len() - 1 - delay)
                % source.committed.len();
            self.committed[index] = source.committed[source_index];
            self.scratch[index] = source.committed[source_index];
        }
        let cursor = Cursor {
            next: if self.committed.is_empty() {
                0
            } else {
                valid % self.committed.len()
            },
            valid,
        };
        self.committed_cursor = cursor;
        self.scratch_cursor = cursor;
        self.dirty_start = 0;
        self.dirty_len = 0;
    }
    pub(super) fn reset(&mut self) {
        self.committed.fill(T::default());
        self.scratch.fill(T::default());
        self.committed_cursor = Cursor::default();
        self.scratch_cursor = Cursor::default();
        self.dirty_start = 0;
        self.dirty_len = 0;
    }
}

/// A fixed stereo delay with committed/scratch banks. Zero delay allocates no
/// ring buffers. Begin once per complete output request, process all subchunks,
/// and commit only after every downstream renderer and final validator succeeds.
#[derive(Debug)]
pub struct DelayPair {
    ring: RingPair<Frame>,
    committed_nonzero: usize,
    scratch_nonzero: usize,
    delay_frames: u32,
    storage_bytes: usize,
    transaction_active: bool,
}

impl DelayPair {
    pub(super) fn history_is_silent(&self) -> bool {
        self.committed_nonzero == 0
    }
    pub(super) fn owned_data_bytes(&self) -> Result<usize, CompensationError> {
        let Self {
            ring,
            committed_nonzero: _,
            scratch_nonzero: _,
            delay_frames: _,
            storage_bytes: _,
            transaction_active: _,
        } = self;
        ring.storage_bytes()
    }
    pub fn new(delay_frames: u32, limits: CompensationLimits) -> Result<Self, CompensationError> {
        limits.validate()?;
        if delay_frames > limits.max_cumulative_frames {
            return Err(CompensationError::LatencyLimit);
        }
        let capacity =
            usize::try_from(delay_frames).map_err(|_| CompensationError::ArithmeticOverflow)?;
        let requested_bytes = requested_ring_bytes::<Frame>(capacity)?;
        if requested_bytes > limits.max_storage_bytes {
            return Err(CompensationError::StorageLimit);
        }
        let ring = RingPair::new(capacity)?;
        let storage_bytes = ring.storage_bytes()?;
        if storage_bytes > limits.max_storage_bytes {
            return Err(CompensationError::StorageLimit);
        }
        Ok(Self {
            ring,
            committed_nonzero: 0,
            scratch_nonzero: 0,
            delay_frames,
            storage_bytes,
            transaction_active: false,
        })
    }

    pub const fn delay_frames(&self) -> u32 {
        self.delay_frames
    }
    pub const fn storage_bytes(&self) -> usize {
        self.storage_bytes
    }
    pub fn history_ready(&self) -> bool {
        self.ring.scratch_cursor.valid == self.ring.scratch.len()
    }

    pub fn begin_transaction(&mut self) -> Result<(), CompensationError> {
        if self.transaction_active {
            return Err(CompensationError::TransactionActive);
        }
        self.ring.begin();
        self.transaction_active = true;
        Ok(())
    }

    pub fn process(
        &mut self,
        input: &[Frame],
        output: &mut [Frame],
    ) -> Result<(), CompensationError> {
        if !self.transaction_active {
            return Err(CompensationError::TransactionInactive);
        }
        if input.len() != output.len() {
            return Err(CompensationError::BufferLength);
        }
        for (source, target) in input.iter().zip(output) {
            *target = self.process_frame(*source);
        }
        Ok(())
    }

    pub fn process_in_place(&mut self, frames: &mut [Frame]) -> Result<(), CompensationError> {
        if !self.transaction_active {
            return Err(CompensationError::TransactionInactive);
        }
        for frame in frames {
            *frame = self.process_frame(*frame);
        }
        Ok(())
    }

    fn process_frame(&mut self, input: Frame) -> Frame {
        let previous = self.ring.push(input);
        // For zero delay, previous equals input and the count stays zero.
        self.scratch_nonzero = self.scratch_nonzero + usize::from(input != [0.0; 2])
            - usize::from(previous != [0.0; 2]);
        previous
    }

    pub fn commit_transaction(&mut self) -> Result<(), CompensationError> {
        if !self.transaction_active {
            return Err(CompensationError::TransactionInactive);
        }
        self.ring.commit();
        self.committed_nonzero = self.scratch_nonzero;
        self.transaction_active = false;
        Ok(())
    }

    pub fn rollback_transaction(&mut self) -> Result<(), CompensationError> {
        if !self.transaction_active {
            return Err(CompensationError::TransactionInactive);
        }
        self.ring.rollback();
        self.scratch_nonzero = self.committed_nonzero;
        self.transaction_active = false;
        Ok(())
    }

    pub(super) fn can_copy_history_from(&self, source: &Self) -> bool {
        !self.transaction_active
            && !source.transaction_active
            && self.delay_frames == source.delay_frames
            && self.ring.capacity() == source.ring.capacity()
    }
    pub(super) fn copy_history_from(&mut self, source: &Self) {
        self.ring.copy_history_from(&source.ring);
        // The transfer preflight requires equal ring lengths.
        self.committed_nonzero = source.committed_nonzero;
        self.scratch_nonzero = self.committed_nonzero;
    }
    pub fn reset_epoch(&mut self) {
        self.ring.reset();
        self.committed_nonzero = 0;
        self.scratch_nonzero = 0;
        self.transaction_active = false;
    }
}
