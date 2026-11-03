//! Bounded callback page cache over worker-owned fixed buffers.
//!
//! Requests carry copyable catalog-generation/source/page keys. Ready and return queues move
//! existing PageBuffer owners; stale completions return to the same pool without entering another
//! catalog generation. Full queues keep pages in reserved held slots. Pump/request work is
//! bounded, lookup borrows cached PCM, and callback paths neither allocate nor perform file I/O,
//! wait, log, join or finally drop a source/page owner. Control retires the endpoint only after
//! callback quiescence.
use crate::types::{PageKey, ResourcePageKey, StreamFailure};
use hex_engine::{
    BorrowedMediaPage, BorrowedScalarPage, Frame, MediaPages, STREAM_PAGE_FRAMES, ScalarSourceKey,
    StreamResourceKey, StreamSourceKey,
};
use rtrb::{Consumer, Producer, PushError};
#[cfg(test)]
use std::cell::Cell;
use std::hash::{Hash, Hasher};

// Installed roles use bits 0–2, legacy pinning uses bit 3, and pending
// readiness roles use bits 4–6. Legacy pins retain their replacement policy.
const INSTALLED_PINS: u8 = 0b0000_0111;
const PENDING_PINS: u8 = 0b0111_0000;

#[derive(Clone, Copy, Debug)]
pub(crate) struct PageRequest(pub(crate) ResourcePageKey);

pub(crate) struct PageBuffer {
    pub(crate) key: Option<ResourcePageKey>,
    pub(crate) frames: Box<[Frame]>,
    pub(crate) scalar_values: Box<[f32]>,
    pub(crate) scalar_coverage: Box<[u64]>,
    pub(crate) valid_frames: usize,
}

impl PageBuffer {
    #[cfg(test)]
    pub(crate) fn new() -> Result<Box<Self>, ()> {
        Self::new_with_capacity(false).map(|(page, _)| page)
    }
    pub(crate) fn new_with_capacity(scalar: bool) -> Result<(Box<Self>, usize), ()> {
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(STREAM_PAGE_FRAMES)
            .map_err(|_| ())?;
        frames.resize(STREAM_PAGE_FRAMES, [0.0; 2]);
        let old_capacity = frames.capacity() * std::mem::size_of::<Frame>();
        let mut scalar_values = Vec::new();
        let mut scalar_coverage = Vec::new();
        if scalar {
            scalar_values
                .try_reserve_exact(STREAM_PAGE_FRAMES)
                .map_err(|_| ())?;
            scalar_values.resize(STREAM_PAGE_FRAMES, 0.0);
            scalar_coverage
                .try_reserve_exact(STREAM_PAGE_FRAMES.div_ceil(64))
                .map_err(|_| ())?;
            scalar_coverage.resize(STREAM_PAGE_FRAMES.div_ceil(64), 0);
        }
        // Vec-to-box may briefly retain the old backing beside the new box.
        let old_capacity = old_capacity
            .checked_add(
                scalar_values
                    .capacity()
                    .checked_mul(std::mem::size_of::<f32>())
                    .ok_or(())?,
            )
            .and_then(|bytes| {
                bytes.checked_add(
                    scalar_coverage
                        .capacity()
                        .checked_mul(std::mem::size_of::<u64>())?,
                )
            })
            .ok_or(())?;
        Ok((
            Box::new(Self {
                key: None,
                frames: frames.into_boxed_slice(),
                scalar_values: scalar_values.into_boxed_slice(),
                scalar_coverage: scalar_coverage.into_boxed_slice(),
                valid_frames: 0,
            }),
            old_capacity,
        ))
    }
    fn payload_len(&self) -> usize {
        match self.key.map(|key| key.source) {
            Some(StreamResourceKey::Scalar(_)) => self
                .scalar_values
                .len()
                .min(self.scalar_coverage.len().saturating_mul(64)),
            _ => self.frames.len(),
        }
    }
}

pub(crate) struct EndpointQueues {
    pub(crate) request_tx: Producer<PageRequest>,
    pub(crate) ready_rx: Consumer<Box<PageBuffer>>,
    pub(crate) return_tx: Producer<Box<PageBuffer>>,
    pub(crate) failure_rx: Consumer<StreamFailure>,
}

enum CacheSlot {
    Empty,
    Occupied { page: Box<PageBuffer>, pinned: u8 },
}

#[derive(Clone, Copy)]
enum PendingSlot {
    Empty,
    Key(ResourcePageKey),
}

pub(crate) struct Endpoint {
    cache: Vec<CacheSlot>,
    max_cached_pages: usize,
    cache_count: usize,
    eviction_cursor: usize,
    pending: Vec<PendingSlot>,
    pending_mask: usize,
    /// Occupied `pending` slots; gates speculative prefetch.
    pending_count: usize,
    source_keys: Vec<StreamResourceKey>,
    request_tx: Producer<PageRequest>,
    ready_rx: Consumer<Box<PageBuffer>>,
    return_tx: Producer<Box<PageBuffer>>,
    failure_rx: Consumer<StreamFailure>,
    held_returns: Vec<Box<PageBuffer>>,
    misses: u64,
    queued_requests: u64,
    prefetched_requests: u64,
    coalesced_requests: u64,
    rejected_requests: u64,
    stale_pages: u64,
    #[cfg(test)]
    cache_probe_count: Cell<usize>,
    #[cfg(test)]
    pending_probe_count: Cell<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EndpointStats {
    pub misses: u64,
    pub queued_requests: u64,
    /// Lookahead requests attempted (`Endpoint::prefetch`); a subset of
    /// `queued_requests` when the queue accepted them.
    pub prefetched_requests: u64,
    pub coalesced_requests: u64,
    pub rejected_requests: u64,
    pub stale_pages: u64,
}

impl Endpoint {
    /// Control-only after worker join. Reserve for the entire actual pool before
    /// entering cleanup. Never destroy a page while collecting/conserving it.
    pub(crate) fn collect_disposal_pages(
        &mut self,
        output: &mut Vec<Box<PageBuffer>>,
    ) -> Option<(usize, usize)> {
        let mut ready = 0;
        while !self.ready_rx.is_empty() {
            if output.len() == output.capacity() {
                return None;
            }
            let page = self.ready_rx.pop().ok()?;
            output.push(page);
            ready += 1;
        }
        let mut held = 0;
        while !self.held_returns.is_empty() {
            if output.len() == output.capacity() {
                return None;
            }
            let page = self.held_returns.pop()?;
            output.push(page);
            held += 1;
        }
        Some((ready, held))
    }
    pub(crate) fn requested_data(
        cache_pages: usize,
        request_capacity: usize,
        source_count: usize,
    ) -> Option<crate::EndpointData> {
        let (cache_slots, pending_count) = Self::geometry(cache_pages, request_capacity)?;
        Some(crate::EndpointData {
            cache: crate::AllocationData::of::<CacheSlot>(cache_slots),
            pending: crate::AllocationData::of::<PendingSlot>(pending_count),
            source_keys: crate::AllocationData::of::<StreamResourceKey>(source_count),
            held_returns: crate::AllocationData::of::<Box<PageBuffer>>(cache_pages.checked_add(1)?),
        })
    }
    fn geometry(cache_pages: usize, request_capacity: usize) -> Option<(usize, usize)> {
        let cache = cache_pages
            .checked_sub(1)?
            .checked_next_power_of_two()?
            .checked_mul(2)?;
        let pending = request_capacity
            .checked_mul(2)?
            .checked_next_power_of_two()?;
        Some((cache, pending))
    }
    pub(crate) fn new(
        cache_pages: usize,
        request_capacity: usize,
        sources: Vec<StreamResourceKey>,
        queues: EndpointQueues,
    ) -> Result<Self, ()> {
        let max_cached_pages = cache_pages.saturating_sub(1);
        let (cache_slots, pending_count) =
            Self::geometry(cache_pages, request_capacity).ok_or(())?;
        let mut cache = Vec::new();
        cache.try_reserve_exact(cache_slots).map_err(|_| ())?;
        cache.resize_with(cache_slots, || CacheSlot::Empty);
        let mut pending = Vec::new();
        pending.try_reserve_exact(pending_count).map_err(|_| ())?;
        pending.resize_with(pending_count, || PendingSlot::Empty);
        let mut held_returns = Vec::new();
        held_returns
            .try_reserve_exact(cache_pages.checked_add(1).ok_or(())?)
            .map_err(|_| ())?;
        Ok(Self {
            cache,
            max_cached_pages,
            cache_count: 0,
            eviction_cursor: 0,
            pending,
            pending_mask: pending_count - 1,
            pending_count: 0,
            source_keys: sources,
            request_tx: queues.request_tx,
            ready_rx: queues.ready_rx,
            return_tx: queues.return_tx,
            failure_rx: queues.failure_rx,
            held_returns,
            misses: 0,
            queued_requests: 0,
            prefetched_requests: 0,
            coalesced_requests: 0,
            rejected_requests: 0,
            stale_pages: 0,
            #[cfg(test)]
            cache_probe_count: Cell::new(0),
            #[cfg(test)]
            pending_probe_count: Cell::new(0),
        })
    }

    pub fn pump_ready(&mut self, limit: usize) -> usize {
        self.pump_ready_for_pending(limit, &[])
    }

    /// Install bounded ready pages while retaining any complete extent needed
    /// by an outstanding readiness intent. This is not a readiness receipt.
    pub(crate) fn pump_ready_for_pending(
        &mut self,
        limit: usize,
        pending: &[(u8, &[crate::StreamResourceExtent])],
    ) -> usize {
        self.flush_held_returns();
        let mut installed = 0;
        for _ in 0..limit {
            let Ok(page) = self.ready_rx.pop() else { break };
            let Some(key) = page.key else {
                self.stale_pages = self.stale_pages.saturating_add(1);
                self.return_buffer(page);
                continue;
            };
            self.pending_remove(key);
            if self.source_keys.binary_search(&key.source).is_err() {
                self.stale_pages = self.stale_pages.saturating_add(1);
                self.return_buffer(page);
                continue;
            }
            let mut pending_mask = 0;
            for (mask, extents) in pending {
                if let Ok(index) = extents
                    .binary_search_by_key(&(key.source, key.page_index), |extent| {
                        (extent.source, extent.page_index)
                    })
                {
                    let extent = extents[index];
                    if extent.start < extent.end
                        && extent.end <= page.valid_frames
                        && page.valid_frames <= page.payload_len()
                    {
                        pending_mask |= mask;
                    }
                }
            }
            if self.insert_ready_with_mask(page, pending_mask) {
                installed += 1;
            }
        }
        self.flush_held_returns();
        installed
    }

    /// Protect already-installed complete extents while their set is pending.
    pub(crate) fn protect_present(&mut self, mask: u8, extents: &[crate::StreamResourceExtent]) {
        for extent in extents {
            let key = ResourcePageKey {
                source: extent.source,
                page_index: extent.page_index,
            };
            if let Some(index) = self.cache_lookup(key)
                && let CacheSlot::Occupied { page, pinned } = &mut self.cache[index]
                && extent.start < extent.end
                && extent.end <= page.valid_frames
                && page.valid_frames <= page.payload_len()
            {
                *pinned |= mask;
            }
        }
    }

    /// Return only unprotected cache victims before requesting missing pages.
    /// The limit bounds evictions and all page ownership stays in the pool.
    pub(crate) fn reclaim_for_missing(&mut self, missing: usize, limit: usize) {
        let available = self.max_cached_pages.saturating_sub(self.cache_count);
        for _ in 0..missing.saturating_sub(available).min(limit) {
            let Some(index) = self.unpinned_victim() else {
                break;
            };
            if let Some((page, _)) = self.cache_remove(index) {
                self.return_buffer(page);
            }
        }
        self.flush_held_returns();
    }

    pub fn take_failures(&mut self, output: &mut [StreamFailure]) -> usize {
        let mut count = 0;
        for item in output {
            let Some(failure) = self.take_failure() else {
                break;
            };
            *item = failure;
            count += 1;
        }
        count
    }

    pub(crate) fn take_failure(&mut self) -> Option<StreamFailure> {
        let failure = self.failure_rx.pop().ok()?;
        self.pending_remove(ResourcePageKey {
            source: failure.key,
            page_index: failure.page_index,
        });
        Some(failure)
    }

    pub fn stats(&self) -> EndpointStats {
        EndpointStats {
            misses: self.misses,
            queued_requests: self.queued_requests,
            prefetched_requests: self.prefetched_requests,
            coalesced_requests: self.coalesced_requests,
            rejected_requests: self.rejected_requests,
            stale_pages: self.stale_pages,
        }
    }

    /// Pin all requested pages if every page is already resident. This is a
    /// control-side admission primitive for a future warmed-playback publisher.
    pub fn pin_pages(&mut self, keys: &[PageKey]) -> bool {
        if keys.iter().any(|key| self.cache_lookup(*key).is_none()) {
            return false;
        }
        for key in keys {
            if let Some(index) = self.cache_lookup(*key) {
                if let CacheSlot::Occupied { pinned, .. } = &mut self.cache[index] {
                    *pinned |= 8;
                }
            }
        }
        true
    }

    pub(crate) fn cache_capacity(&self) -> usize {
        self.max_cached_pages
    }
    pub(crate) fn has_extent(&self, extent: impl Into<crate::StreamResourceExtent>) -> bool {
        let extent = extent.into();
        let key = ResourcePageKey {
            source: extent.source,
            page_index: extent.page_index,
        };
        self.cache_lookup(key)
            .and_then(|index| self.cache.get(index))
            .is_some_and(|slot| {
                matches!(slot, CacheSlot::Occupied { page, .. }
                if page.key == Some(key) && extent.start < extent.end
                    && extent.end <= page.valid_frames && page.valid_frames <= page.payload_len())
            })
    }
    /// The caller validates the entire extent set before this nonfallible role
    /// replacement. Other roles and legacy pins remain independent.
    pub(crate) fn replace_protection(&mut self, mask: u8, extents: &[crate::StreamResourceExtent]) {
        for slot in &mut self.cache {
            if let CacheSlot::Occupied { pinned, .. } = slot {
                *pinned &= !mask;
            }
        }
        for extent in extents {
            if let Some(index) = self.cache_lookup(ResourcePageKey {
                source: extent.source,
                page_index: extent.page_index,
            }) && let CacheSlot::Occupied { pinned, .. } = &mut self.cache[index]
            {
                *pinned |= mask;
            }
        }
    }
    pub fn unpin_all(&mut self) {
        for slot in &mut self.cache {
            if let CacheSlot::Occupied { pinned, .. } = slot {
                *pinned &= !8;
            }
        }
    }

    pub(crate) fn return_cached_pages(&mut self) -> usize {
        let mut returned = 0;
        for index in 0..self.cache.len() {
            let slot = std::mem::replace(&mut self.cache[index], CacheSlot::Empty);
            if let CacheSlot::Occupied { page, .. } = slot {
                self.return_buffer(page);
                returned += 1;
            }
        }
        self.cache_count = 0;
        self.flush_held_returns();
        returned
    }

    pub(crate) fn drop_held_returns_after_join(&mut self) -> usize {
        let count = self.held_returns.len();
        self.held_returns.clear();
        count
    }

    pub(crate) fn drain_ready_after_join(&mut self) -> usize {
        let mut count = 0;
        while let Ok(page) = self.ready_rx.pop() {
            drop(page);
            count += 1;
        }
        count
    }

    fn pending_remove(&mut self, key: impl Into<ResourcePageKey>) {
        let key = key.into();
        let mut slot = page_slot(key, self.pending_mask);
        for _ in 0..self.pending.len() {
            #[cfg(test)]
            self.pending_probe_count
                .set(self.pending_probe_count.get().saturating_add(1));
            match self.pending[slot] {
                PendingSlot::Key(found) if found == key => {
                    self.pending[slot] = PendingSlot::Empty;
                    self.pending_count = self.pending_count.saturating_sub(1);
                    self.pending_shift_delete(slot);
                    return;
                }
                PendingSlot::Key(_) => slot = (slot + 1) & self.pending_mask,
                PendingSlot::Empty => return,
            }
        }
    }

    fn pending_shift_delete(&mut self, mut hole: usize) {
        let mut scan = (hole + 1) & self.pending_mask;
        for _ in 0..self.pending.len().saturating_sub(1) {
            let PendingSlot::Key(key) = self.pending[scan] else {
                break;
            };
            let home = page_slot(key, self.pending_mask);
            if probe_distance(home, hole, self.pending_mask)
                < probe_distance(home, scan, self.pending_mask)
            {
                self.pending[hole] = PendingSlot::Key(key);
                self.pending[scan] = PendingSlot::Empty;
                hole = scan;
            }
            scan = (scan + 1) & self.pending_mask;
        }
    }

    fn queue_request(&mut self, key: impl Into<ResourcePageKey>) {
        let key = key.into();
        if self.source_keys.binary_search(&key.source).is_err() {
            self.rejected_requests = self.rejected_requests.saturating_add(1);
            return;
        }
        let mut first_available = None;
        let mut slot = page_slot(key, self.pending_mask);
        for _ in 0..self.pending.len() {
            #[cfg(test)]
            self.pending_probe_count
                .set(self.pending_probe_count.get().saturating_add(1));
            match self.pending[slot] {
                PendingSlot::Key(found) if found == key => {
                    self.coalesced_requests = self.coalesced_requests.saturating_add(1);
                    return;
                }
                PendingSlot::Key(_) => slot = (slot + 1) & self.pending_mask,
                PendingSlot::Empty => {
                    first_available.get_or_insert(slot);
                    break;
                }
            }
        }
        let Some(slot) = first_available else {
            self.rejected_requests = self.rejected_requests.saturating_add(1);
            return;
        };
        match self.request_tx.push(PageRequest(key)) {
            Ok(()) => {
                self.pending[slot] = PendingSlot::Key(key);
                self.pending_count += 1;
                self.queued_requests = self.queued_requests.saturating_add(1);
            }
            Err(PushError::Full(_)) => {
                self.rejected_requests = self.rejected_requests.saturating_add(1);
            }
        }
    }

    /// Speculatively request `page_index` ahead of the playhead, which is
    /// reading `page_index - 1` of the same source.
    ///
    /// Lookahead contract (#4075): at most one request is ever outstanding
    /// because of it, since it queues only while no other request is pending,
    /// so it never delays a demand request by more than one page decode and
    /// never fills the bounded request queue. It skips pages already cached or
    /// pending and unknown sources (without counting a rejection). The page
    /// arrives unpinned, so readiness reclaim and ordinary eviction can take
    /// it back: a prefetch never holds pool pages against demand. To make
    /// room in a full cache it evicts at most one unpinned page other than
    /// the one being read, so it cannot evict the playhead's own page. A full
    /// request queue drops it like any rejected request; the callback never
    /// waits.
    pub(crate) fn prefetch(&mut self, key: impl Into<ResourcePageKey>) {
        let key = key.into();
        if self.pending_count != 0
            || self.source_keys.binary_search(&key.source).is_err()
            || self.cache_lookup(key).is_some()
        {
            return;
        }
        let reading = ResourcePageKey {
            source: key.source,
            page_index: key.page_index.wrapping_sub(1),
        };
        if self.cache_count >= self.max_cached_pages {
            let Some(index) = self.unpinned_victim_except(reading) else {
                return;
            };
            if let Some((page, _)) = self.cache_remove(index) {
                self.return_buffer(page);
            }
        }
        self.prefetched_requests = self.prefetched_requests.saturating_add(1);
        self.queue_request(key);
    }

    fn unpinned_victim_except(&mut self, keep: ResourcePageKey) -> Option<usize> {
        let mask = self.cache.len() - 1;
        for offset in 0..self.cache.len() {
            let index = (self.eviction_cursor + offset) & mask;
            if let CacheSlot::Occupied { page, pinned: 0 } = &self.cache[index]
                && page.key != Some(keep)
            {
                self.eviction_cursor = (index + 1) & mask;
                return Some(index);
            }
        }
        None
    }

    fn cache_lookup(&self, key: impl Into<ResourcePageKey>) -> Option<usize> {
        let key = key.into();
        let mask = self.cache.len() - 1;
        let mut slot = page_slot(key, mask);
        for _ in 0..self.cache.len() {
            #[cfg(test)]
            self.cache_probe_count
                .set(self.cache_probe_count.get().saturating_add(1));
            match &self.cache[slot] {
                CacheSlot::Occupied { page, .. } if page.key == Some(key) => return Some(slot),
                CacheSlot::Occupied { .. } => slot = (slot + 1) & mask,
                CacheSlot::Empty => return None,
            }
        }
        None
    }

    #[cfg(test)]
    fn insert_ready(&mut self, page: Box<PageBuffer>) -> bool {
        self.insert_ready_with_mask(page, 0)
    }
    fn insert_ready_with_mask(&mut self, page: Box<PageBuffer>, pending_mask: u8) -> bool {
        let Some(key) = page.key else {
            self.stale_pages = self.stale_pages.saturating_add(1);
            self.return_buffer(page);
            return false;
        };
        if let Some(index) = self.cache_lookup(key) {
            // Installed pages retain their original owner. A pending-only page
            // has no receipt or borrowed consumer, so a strictly longer decode
            // may replace it only if it covers every existing pending role.
            let refuse = match &self.cache[index] {
                CacheSlot::Occupied { page: old, pinned } => {
                    let pending = *pinned & PENDING_PINS;
                    *pinned & INSTALLED_PINS != 0
                        || (pending != 0
                            && (page.valid_frames <= old.valid_frames
                                || pending_mask & pending != pending))
                }
                CacheSlot::Empty => true,
            };
            if refuse {
                self.return_buffer(page);
                return false;
            }
            let old = std::mem::replace(
                &mut self.cache[index],
                CacheSlot::Occupied {
                    page,
                    pinned: pending_mask,
                },
            );
            if let CacheSlot::Occupied { page, pinned } = old {
                if let CacheSlot::Occupied {
                    pinned: current, ..
                } = &mut self.cache[index]
                {
                    *current |= pinned;
                }
                self.return_buffer(page);
            }
            return true;
        }
        if self.cache_count >= self.max_cached_pages {
            let Some(index) = self.unpinned_victim() else {
                self.return_buffer(page);
                return false;
            };
            if let Some((page, _pinned)) = self.cache_remove(index) {
                self.return_buffer(page);
            }
        }
        let Some(index) = self.cache_insertion_slot(key) else {
            self.return_buffer(page);
            return false;
        };
        self.cache[index] = CacheSlot::Occupied {
            page,
            pinned: pending_mask,
        };
        self.cache_count += 1;
        true
    }

    fn cache_remove(&mut self, index: usize) -> Option<(Box<PageBuffer>, u8)> {
        let mask = self.cache.len() - 1;
        let removed = std::mem::replace(&mut self.cache[index], CacheSlot::Empty);
        let CacheSlot::Occupied { page, pinned } = removed else {
            return None;
        };
        self.cache_count = self.cache_count.saturating_sub(1);
        let mut hole = index;
        let mut scan = (hole + 1) & mask;
        for _ in 0..self.cache.len().saturating_sub(1) {
            let CacheSlot::Occupied {
                page: following, ..
            } = &self.cache[scan]
            else {
                break;
            };
            let Some(key) = following.key else {
                scan = (scan + 1) & mask;
                continue;
            };
            let home = page_slot(key, mask);
            if probe_distance(home, hole, mask) < probe_distance(home, scan, mask) {
                self.cache[hole] = std::mem::replace(&mut self.cache[scan], CacheSlot::Empty);
                hole = scan;
            }
            scan = (scan + 1) & mask;
        }
        Some((page, pinned))
    }

    fn cache_insertion_slot(&self, key: impl Into<ResourcePageKey>) -> Option<usize> {
        let key = key.into();
        let mask = self.cache.len() - 1;
        let mut slot = page_slot(key, mask);
        for _ in 0..self.cache.len() {
            match &self.cache[slot] {
                CacheSlot::Occupied { .. } => slot = (slot + 1) & mask,
                CacheSlot::Empty => return Some(slot),
            }
        }
        None
    }

    fn unpinned_victim(&mut self) -> Option<usize> {
        for offset in 0..self.cache.len() {
            let index = (self.eviction_cursor + offset) & (self.cache.len() - 1);
            if matches!(self.cache[index], CacheSlot::Occupied { pinned: 0, .. }) {
                self.eviction_cursor = (index + 1) & (self.cache.len() - 1);
                return Some(index);
            }
        }
        None
    }

    fn return_buffer(&mut self, page: Box<PageBuffer>) {
        match self.return_tx.push(page) {
            Ok(()) => {}
            Err(PushError::Full(page)) => {
                self.held_returns.push(page);
            }
        }
    }

    fn flush_held_returns(&mut self) {
        while !self.held_returns.is_empty() {
            let Some(page) = self.held_returns.pop() else {
                break;
            };
            match self.return_tx.push(page) {
                Ok(()) => {}
                Err(PushError::Full(page)) => {
                    self.held_returns.push(page);
                    break;
                }
            }
        }
    }
}

impl MediaPages for Endpoint {
    fn page(&mut self, key: StreamSourceKey, page_index: u64) -> Option<BorrowedMediaPage<'_>> {
        let key = ResourcePageKey {
            source: key.into(),
            page_index,
        };
        if let Some(slot) = self.cache_lookup(key) {
            if let CacheSlot::Occupied { page, .. } = &self.cache[slot] {
                return Some(BorrowedMediaPage {
                    page_index,
                    frames: &page.frames[..page.valid_frames],
                });
            }
        }
        self.misses = self.misses.saturating_add(1);
        None
    }

    fn request_page(&mut self, key: StreamSourceKey, page_index: u64) {
        self.queue_request(ResourcePageKey {
            source: key.into(),
            page_index,
        });
    }

    fn prefetch_page(&mut self, key: StreamSourceKey, page_index: u64) {
        self.prefetch(ResourcePageKey {
            source: key.into(),
            page_index,
        });
    }
    fn scalar_page(
        &mut self,
        key: ScalarSourceKey,
        page_index: u64,
    ) -> Option<BorrowedScalarPage<'_>> {
        let resource = ResourcePageKey {
            source: key.into(),
            page_index,
        };
        if let Some(slot) = self.cache_lookup(resource) {
            if let CacheSlot::Occupied { page, .. } = &self.cache[slot] {
                return Some(BorrowedScalarPage {
                    key,
                    page_index,
                    values: &page.scalar_values[..page.valid_frames],
                    coverage: &page.scalar_coverage[..page.valid_frames.div_ceil(64)],
                });
            }
        }
        self.misses = self.misses.saturating_add(1);
        None
    }
    fn request_scalar_page(&mut self, key: ScalarSourceKey, page_index: u64) {
        self.queue_request(ResourcePageKey {
            source: key.into(),
            page_index,
        });
    }
}

fn probe_distance(home: usize, slot: usize, mask: usize) -> usize {
    slot.wrapping_sub(home) & mask
}

fn page_slot(key: impl Into<ResourcePageKey>, mask: usize) -> usize {
    let key = key.into();
    struct FastHasher(u64);
    impl Hasher for FastHasher {
        fn finish(&self) -> u64 {
            self.0
        }
        fn write(&mut self, bytes: &[u8]) {
            for byte in bytes {
                self.0 ^= u64::from(*byte);
                self.0 = self.0.wrapping_mul(0x100_0000_01b3);
            }
        }
    }
    let mut hasher = FastHasher(0xcbf2_9ce4_8422_2325);
    key.hash(&mut hasher);
    hasher.finish() as usize & mask
}

impl Endpoint {
    pub(crate) fn data_footprint(&self) -> crate::EndpointData {
        let Self {
            cache,
            max_cached_pages: _,
            cache_count: _,
            eviction_cursor: _,
            pending,
            pending_mask: _,
            pending_count: _,
            source_keys,
            request_tx: _,
            ready_rx: _,
            return_tx: _,
            failure_rx: _,
            held_returns,
            misses: _,
            queued_requests: _,
            prefetched_requests: _,
            coalesced_requests: _,
            rejected_requests: _,
            stale_pages: _,
            #[cfg(test)]
                cache_probe_count: _,
            #[cfg(test)]
                pending_probe_count: _,
        } = self;
        crate::EndpointData {
            cache: crate::AllocationData::of::<CacheSlot>(cache.capacity()),
            pending: crate::AllocationData::of::<PendingSlot>(pending.capacity()),
            source_keys: crate::AllocationData::of::<StreamResourceKey>(source_keys.capacity()),
            held_returns: crate::AllocationData::of::<Box<PageBuffer>>(held_returns.capacity()),
        }
    }
}

#[cfg(test)]
mod prefetch_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project};
    use rtrb::RingBuffer;
    use std::collections::BTreeMap;

    fn source() -> StreamSourceKey {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 4).expect("editor");
        match editor
            .execute(Command::AddAsset {
                path: "endpoint-test.wav".into(),
                sample_rate: 48_000,
                frames: 4096,
            })
            .expect("asset")
        {
            Created::Asset(asset) => StreamSourceKey::new(73, asset),
            other => panic!("unexpected asset result: {other:?}"),
        }
    }

    struct TestEndpoint {
        endpoint: Endpoint,
        requests: Consumer<PageRequest>,
        ready: Producer<Box<PageBuffer>>,
        returns: Consumer<Box<PageBuffer>>,
        #[cfg(feature = "allocation-probe")]
        failures: Producer<StreamFailure>,
    }

    fn endpoint(
        cache_pages: usize,
        request_capacity: usize,
        return_capacity: usize,
    ) -> TestEndpoint {
        let key = source();
        let (request_tx, requests) = RingBuffer::new(request_capacity);
        let (ready, ready_rx) = RingBuffer::new(cache_pages);
        let (return_tx, returns) = RingBuffer::new(return_capacity);
        let (failures, failure_rx) = RingBuffer::new(request_capacity.max(1));
        #[cfg(not(feature = "allocation-probe"))]
        let _ = failures;
        let endpoint = Endpoint::new(
            cache_pages,
            request_capacity,
            vec![key.into()],
            EndpointQueues {
                request_tx,
                ready_rx,
                return_tx,
                failure_rx,
            },
        )
        .expect("preallocated endpoint");
        TestEndpoint {
            endpoint,
            requests,
            ready,
            returns,
            #[cfg(feature = "allocation-probe")]
            failures,
        }
    }

    fn colliding_pages(source: StreamSourceKey, mask: usize, count: usize) -> Vec<PageKey> {
        let mut by_slot = BTreeMap::<usize, Vec<PageKey>>::new();
        for page_index in 0..100_000_u64 {
            let key = PageKey { source, page_index };
            let group = by_slot.entry(page_slot(key, mask)).or_default();
            group.push(key);
            if group.len() == count {
                return group.clone();
            }
        }
        panic!("could not produce deterministic hash collisions");
    }

    fn colliding_at_home(
        source: StreamSourceKey,
        mask: usize,
        home: usize,
        count: usize,
    ) -> Vec<PageKey> {
        let mut keys = Vec::with_capacity(count);
        for page_index in 0..100_000_u64 {
            let key = PageKey { source, page_index };
            if page_slot(key, mask) == home {
                keys.push(key);
                if keys.len() == count {
                    return keys;
                }
            }
        }
        panic!("could not produce enough deterministic wraparound collisions");
    }

    fn page(key: PageKey) -> Box<PageBuffer> {
        let mut page = PageBuffer::new().expect("page buffer");
        page.key = Some(key.into());
        page.valid_frames = 1;
        page.frames[0] = [key.page_index as f32, -(key.page_index as f32)];
        page
    }

    #[test]
    fn cache_open_addressing_preserves_colliding_pages() {
        let mut test = endpoint(4, 4, 4);
        let StreamResourceKey::Audio(source) = test.endpoint.source_keys[0] else {
            panic!("audio fixture")
        };
        let keys = colliding_pages(source, test.endpoint.cache.len() - 1, 3);
        for key in &keys {
            assert!(test.endpoint.insert_ready(page(*key)));
        }
        for key in keys.iter().copied() {
            assert!(test.endpoint.cache_lookup(key).is_some());
            let found = test
                .endpoint
                .page(key.source, key.page_index)
                .expect("colliding cached page");
            assert_eq!(
                found.frames[0],
                [key.page_index as f32, -(key.page_index as f32)]
            );
        }
    }

    #[test]
    fn backward_shift_preserves_pending_coalescing_and_full_request_queue_can_retry() {
        let mut test = endpoint(4, 4, 4);
        let StreamResourceKey::Audio(source) = test.endpoint.source_keys[0] else {
            panic!("audio fixture")
        };
        let keys = colliding_pages(source, test.endpoint.pending_mask, 3);
        for key in &keys {
            test.endpoint.request_page(key.source, key.page_index);
        }
        test.endpoint.pending_remove(keys[1]);
        test.endpoint
            .request_page(keys[2].source, keys[2].page_index);
        assert_eq!(test.endpoint.stats().coalesced_requests, 1);
        assert_eq!(
            test.requests.pop().expect("first request").0,
            keys[0].into()
        );
        assert_eq!(
            test.requests.pop().expect("middle original request").0,
            keys[1].into()
        );
        assert_eq!(
            test.requests.pop().expect("third original request").0,
            keys[2].into()
        );
        assert!(test.requests.pop().is_err());

        // The home slot is the last index, so this cluster crosses the end of
        // the table. Repeated middle deletion must keep the later key
        // discoverable without leaving tombstones behind.
        let mut turnover = endpoint(3, 8, 2);
        let StreamResourceKey::Audio(source) = turnover.endpoint.source_keys[0] else {
            panic!("audio fixture")
        };
        let mask = turnover.endpoint.pending_mask;
        let keys = colliding_at_home(source, mask, mask, 3);
        for _ in 0..512 {
            for key in &keys {
                turnover.endpoint.request_page(key.source, key.page_index);
            }
            turnover.endpoint.pending_remove(keys[1]);
            let before = turnover.endpoint.stats().coalesced_requests;
            turnover
                .endpoint
                .request_page(keys[2].source, keys[2].page_index);
            assert_eq!(turnover.endpoint.stats().coalesced_requests, before + 1);
            for key in &keys {
                assert_eq!(
                    turnover.requests.pop().expect("queued request").0,
                    (*key).into()
                );
            }
            turnover.endpoint.pending_remove(keys[0]);
            turnover.endpoint.pending_remove(keys[2]);
            assert!(
                turnover
                    .endpoint
                    .pending
                    .iter()
                    .all(|slot| matches!(slot, PendingSlot::Empty))
            );
        }
        turnover.endpoint.pending_probe_count.set(0);
        let fresh = PageKey {
            source,
            page_index: 999_999,
        };
        turnover
            .endpoint
            .request_page(fresh.source, fresh.page_index);
        assert_eq!(turnover.endpoint.pending_probe_count.get(), 1);
        assert_eq!(
            turnover.requests.pop().expect("fresh request").0,
            fresh.into()
        );

        let mut full = endpoint(3, 1, 2);
        let StreamResourceKey::Audio(full_source) = full.endpoint.source_keys[0] else {
            panic!("audio fixture")
        };
        let first = PageKey {
            source: full_source,
            page_index: 0,
        };
        let second = PageKey {
            source: full_source,
            page_index: 1,
        };
        full.endpoint.request_page(first.source, first.page_index);
        full.endpoint.request_page(second.source, second.page_index);
        assert_eq!(full.endpoint.stats().queued_requests, 1);
        assert_eq!(full.endpoint.stats().rejected_requests, 1);
        assert_eq!(
            full.requests.pop().expect("drain bounded request").0,
            first.into()
        );
        full.endpoint.request_page(second.source, second.page_index);
        assert_eq!(full.endpoint.stats().queued_requests, 2);
        assert_eq!(
            full.requests.pop().expect("retry after drain").0,
            second.into()
        );
    }

    #[test]
    fn cache_backward_shift_survives_wraparound_turnover_without_dead_entries() {
        let mut test = endpoint(5, 4, 8);
        let StreamResourceKey::Audio(source) = test.endpoint.source_keys[0] else {
            panic!("audio fixture")
        };
        let mask = test.endpoint.cache.len() - 1;
        let keys = colliding_at_home(source, mask, mask, 517);
        let mut live = keys[..4].to_vec();
        for key in &live {
            assert!(test.endpoint.insert_ready(page(*key)));
        }

        for incoming in keys[4..516].iter().copied() {
            assert!(test.endpoint.insert_ready(page(incoming)));
            assert_eq!(test.endpoint.cache_count, 4);
            assert!(test.endpoint.cache_lookup(incoming).is_some());
            live.retain(|key| test.endpoint.cache_lookup(*key).is_some());
            live.push(incoming);
            assert_eq!(live.len(), 4);
            while let Ok(_returned) = test.returns.pop() {}
        }

        for key in &live {
            assert!(test.endpoint.cache_lookup(*key).is_some());
        }
        test.endpoint.cache_probe_count.set(0);
        let absent = keys[516];
        assert!(test.endpoint.cache_lookup(absent).is_none());
        assert_eq!(test.endpoint.cache_probe_count.get(), 5);

        assert_eq!(test.endpoint.return_cached_pages(), 4);
        while let Ok(_returned) = test.returns.pop() {}
        assert_eq!(test.endpoint.cache_count, 0);
        assert!(
            test.endpoint
                .cache
                .iter()
                .all(|slot| matches!(slot, CacheSlot::Empty))
        );
    }

    #[test]
    fn longer_duplicate_can_satisfy_overlapping_pending_extents() {
        let mut test = endpoint(3, 4, 4);
        let StreamResourceKey::Audio(source) = test.endpoint.source_keys[0] else {
            panic!("audio fixture")
        };
        let key = PageKey {
            source,
            page_index: 0,
        };
        let shorter = crate::StreamPageExtent {
            source,
            page_index: 0,
            start: 0,
            end: 7,
        };
        let longer = crate::StreamPageExtent { end: 8, ..shorter };
        let mut first = page(key);
        first.valid_frames = 7;
        assert!(test.endpoint.insert_ready(first));
        test.endpoint.protect_present(16, &[shorter.into()]);
        let mut complete = page(key);
        complete.valid_frames = 8;
        assert!(test.ready.push(complete).is_ok());
        let a = [shorter.into()];
        let b = [longer.into()];
        assert_eq!(
            test.endpoint
                .pump_ready_for_pending(1, &[(16, &a), (32, &b)]),
            1
        );
        assert!(test.endpoint.has_extent(shorter));
        assert!(test.endpoint.has_extent(longer));
        assert!(
            matches!(&test.endpoint.cache[test.endpoint.cache_lookup(key).unwrap()], CacheSlot::Occupied { pinned, .. } if pinned & 48 == 48)
        );
    }

    #[test]
    fn pending_ready_page_survives_later_insertion_in_same_pump() {
        let mut test = endpoint(3, 4, 4);
        let StreamResourceKey::Audio(source) = test.endpoint.source_keys[0] else {
            panic!("audio fixture")
        };
        let key = |page_index| PageKey { source, page_index };
        assert!(test.endpoint.insert_ready(page(key(0))));
        assert!(test.endpoint.insert_ready(page(key(1))));
        let active = crate::StreamPageExtent {
            source,
            page_index: 0,
            start: 0,
            end: 1,
        };
        let pending = crate::StreamPageExtent {
            page_index: 2,
            ..active
        };
        test.endpoint.replace_protection(1, &[active.into()]);
        assert!(test.ready.push(page(key(2))).is_ok());
        assert!(test.ready.push(page(key(3))).is_ok());
        let desired = [pending.into()];
        #[cfg(feature = "allocation-probe")]
        let mut installed = 0;
        #[cfg(feature = "allocation-probe")]
        let allocations = allocation_counter::measure(|| {
            installed = test.endpoint.pump_ready_for_pending(2, &[(16, &desired)])
        });
        #[cfg(not(feature = "allocation-probe"))]
        let installed = test.endpoint.pump_ready_for_pending(2, &[(16, &desired)]);
        #[cfg(feature = "allocation-probe")]
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(installed, 1);
        assert!(test.endpoint.has_extent(active));
        assert!(test.endpoint.has_extent(pending));
        assert!(test.endpoint.cache_lookup(key(3)).is_none());
        assert!(test.returns.pop().is_ok());
        assert!(test.returns.pop().is_ok());
    }

    #[test]
    fn reclaim_missing_pages_returns_only_unprotected_victims_with_a_bounded_limit() {
        let mut test = endpoint(5, 8, 8);
        let StreamResourceKey::Audio(source) = test.endpoint.source_keys[0] else {
            panic!("audio fixture")
        };
        let keys: Vec<_> = (0..4)
            .map(|page_index| PageKey { source, page_index })
            .collect();
        for key in &keys {
            assert!(test.endpoint.insert_ready(page(*key)));
        }
        let active = crate::StreamPageExtent {
            source,
            page_index: 0,
            start: 0,
            end: 1,
        };
        let pending = crate::StreamPageExtent {
            page_index: 1,
            ..active
        };
        test.endpoint.replace_protection(1, &[active.into()]);
        test.endpoint.protect_present(16, &[pending.into()]);
        #[cfg(feature = "allocation-probe")]
        let allocations = allocation_counter::measure(|| test.endpoint.reclaim_for_missing(2, 1));
        #[cfg(not(feature = "allocation-probe"))]
        test.endpoint.reclaim_for_missing(2, 1);
        #[cfg(feature = "allocation-probe")]
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(test.endpoint.cache_count, 3);
        assert!(test.endpoint.has_extent(active));
        assert!(test.endpoint.has_extent(pending));
        assert!(test.returns.pop().is_ok());
        test.endpoint.reclaim_for_missing(2, 1);
        assert_eq!(test.endpoint.cache_count, 2);
        assert!(test.endpoint.has_extent(active));
        assert!(test.endpoint.has_extent(pending));
        assert!(test.returns.pop().is_ok());
        test.endpoint.reclaim_for_missing(3, 8);
        assert_eq!(test.endpoint.cache_count, 2);
        assert!(test.returns.pop().is_err());
    }

    #[test]
    fn exact_extent_and_role_checks_refuse_short_wrong_pages_without_replacing_protected_pcm() {
        let mut test = endpoint(3, 4, 4);
        let StreamResourceKey::Audio(source) = test.endpoint.source_keys[0] else {
            panic!("audio fixture")
        };
        let key = PageKey {
            source,
            page_index: 0,
        };
        let extent = crate::StreamPageExtent {
            source,
            page_index: 0,
            start: 0,
            end: 8,
        };
        let mut short = page(key);
        short.valid_frames = 7;
        assert!(test.endpoint.insert_ready(short));
        assert!(!test.endpoint.has_extent(extent));
        assert!(!test.endpoint.has_extent(crate::StreamPageExtent {
            page_index: 1,
            ..extent
        }));
        let mut complete = page(key);
        complete.valid_frames = 8;
        assert!(test.endpoint.insert_ready(complete));
        assert!(test.endpoint.has_extent(extent));
        let index = test.endpoint.cache_lookup(key).unwrap();
        let original = match &test.endpoint.cache[index] {
            CacheSlot::Occupied { page, .. } => page.frames.as_ptr(),
            CacheSlot::Empty => panic!("page"),
        };
        test.endpoint.replace_protection(1, &[extent.into()]);
        test.endpoint.replace_protection(2, &[extent.into()]);
        test.endpoint.replace_protection(1, &[]);
        assert!(matches!(
            test.endpoint.cache[index],
            CacheSlot::Occupied { pinned: 2, .. }
        ));
        let mut short = page(key);
        short.valid_frames = 7;
        assert!(!test.endpoint.insert_ready(short));
        assert!(test.endpoint.has_extent(extent));
        assert!(
            matches!(&test.endpoint.cache[index], CacheSlot::Occupied { page, .. } if page.frames.as_ptr() == original)
        );
        test.endpoint.replace_protection(2, &[]);
        assert!(matches!(
            test.endpoint.cache[index],
            CacheSlot::Occupied { pinned: 0, .. }
        ));
    }

    #[test]
    fn pinned_pages_refuse_replacement_and_full_return_ring_uses_held_slot() {
        let mut test = endpoint(2, 2, 1);
        let StreamResourceKey::Audio(source) = test.endpoint.source_keys[0] else {
            panic!("audio fixture")
        };
        let first = PageKey {
            source,
            page_index: 1,
        };
        let second = PageKey {
            source,
            page_index: 2,
        };
        assert!(test.endpoint.insert_ready(page(first)));
        assert!(test.endpoint.pin_pages(&[first]));
        assert!(test.ready.push(page(second)).is_ok());
        assert_eq!(test.endpoint.pump_ready(1), 0);
        assert!(test.endpoint.cache_lookup(first).is_some());
        assert!(test.endpoint.cache_lookup(second).is_none());
        assert_eq!(
            test.returns.pop().expect("refused page returned").key,
            Some(second.into())
        );

        assert!(test.ready.push(page(second)).is_ok());
        test.endpoint.unpin_all();
        assert_eq!(test.endpoint.pump_ready(1), 1);
        assert!(test.endpoint.cache_lookup(second).is_some());
        let _ = test.returns.pop().expect("evicted first page returned");

        // Keep the return queue full while cached ownership is retired. The
        // bounded held slot must preserve the page until control drains it.
        let sentinel = page(PageKey {
            source,
            page_index: 99,
        });
        assert!(test.endpoint.return_tx.push(sentinel).is_ok());
        assert_eq!(test.endpoint.return_cached_pages(), 1);
        assert_eq!(test.endpoint.held_returns.len(), 1);
        let _ = test.returns.pop().expect("remove return sentinel");
        test.endpoint.flush_held_returns();
        assert_eq!(test.endpoint.held_returns.len(), 0);
        assert_eq!(
            test.returns.pop().expect("held return flushed").key,
            Some(second.into())
        );
    }

    #[cfg(feature = "allocation-probe")]
    fn assert_zero_probe(info: allocation_counter::AllocationInfo) {
        assert_eq!(info.count_total, 0);
        assert_eq!(info.bytes_total, 0);
        assert_eq!(info.count_current, 0);
        assert_eq!(info.bytes_current, 0);
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn endpoint_request_failure_retry_install_eviction_and_pins_are_allocation_free() {
        use allocation_counter::measure;

        let mut pending = endpoint(4, 4, 4);
        let StreamResourceKey::Audio(source) = pending.endpoint.source_keys[0] else {
            panic!("audio fixture")
        };
        let keys = colliding_at_home(
            source,
            pending.endpoint.pending_mask,
            pending.endpoint.pending_mask,
            3,
        );
        for key in keys.iter().copied() {
            let info = measure(|| pending.endpoint.request_page(key.source, key.page_index));
            assert_zero_probe(info);
        }
        for key in &keys {
            assert_eq!(
                pending.requests.pop().expect("queued page").0,
                (*key).into()
            );
        }
        let failure = StreamFailure {
            key: source.into(),
            page_index: keys[1].page_index,
            kind: crate::types::StreamFailureKind::SourceChanged,
        };
        assert!(pending.failures.push(failure).is_ok());
        let mut failure_output = [failure; 1];
        let mut taken = 0;
        let info = measure(|| taken = pending.endpoint.take_failures(&mut failure_output));
        assert_zero_probe(info);
        assert_eq!(taken, 1);
        assert_eq!(failure_output, [failure]);

        let coalesced_before = pending.endpoint.stats().coalesced_requests;
        let info = measure(|| {
            pending
                .endpoint
                .request_page(keys[2].source, keys[2].page_index);
        });
        assert_zero_probe(info);
        assert_eq!(
            pending.endpoint.stats().coalesced_requests,
            coalesced_before + 1
        );

        let info = measure(|| {
            pending
                .endpoint
                .request_page(keys[1].source, keys[1].page_index)
        });
        assert_zero_probe(info);
        assert_eq!(
            pending.requests.pop().expect("retried failure").0,
            keys[1].into()
        );

        let mut ready = endpoint(4, 4, 4);
        let StreamResourceKey::Audio(source) = ready.endpoint.source_keys[0] else {
            panic!("audio fixture")
        };
        let initial = [
            PageKey {
                source,
                page_index: 1,
            },
            PageKey {
                source,
                page_index: 2,
            },
            PageKey {
                source,
                page_index: 3,
            },
        ];
        for key in initial {
            assert!(ready.endpoint.insert_ready(page(key)));
        }
        for index in 100..104 {
            assert!(
                ready
                    .endpoint
                    .return_tx
                    .push(page(PageKey {
                        source,
                        page_index: index,
                    }))
                    .is_ok()
            );
        }
        let incoming = [
            PageKey {
                source,
                page_index: 20,
            },
            PageKey {
                source,
                page_index: 21,
            },
            PageKey {
                source,
                page_index: 22,
            },
            PageKey {
                source,
                page_index: 23,
            },
        ];
        for key in incoming {
            assert!(ready.ready.push(page(key)).is_ok());
        }
        let mut installed = 0;
        let info = measure(|| installed = ready.endpoint.pump_ready(1));
        assert_zero_probe(info);
        assert_eq!(installed, 1);
        assert_eq!(ready.endpoint.held_returns.len(), 1);
        assert!(ready.endpoint.cache_lookup(incoming[0]).is_some());
        assert_eq!(ready.endpoint.cache_count, 3);

        for _ in 0..4 {
            let _ = ready.returns.pop().expect("drain full return ring");
        }
        let info = measure(|| ready.endpoint.flush_held_returns());
        assert_zero_probe(info);
        assert!(ready.endpoint.held_returns.is_empty());
        let returned_key = ready
            .returns
            .pop()
            .expect("held victim returned")
            .key
            .expect("cached page key");
        assert!(
            initial
                .iter()
                .any(|key| ResourcePageKey::from(*key) == returned_key)
        );

        let resident_keys = ready
            .endpoint
            .cache
            .iter()
            .filter_map(|slot| match slot {
                CacheSlot::Occupied { page, .. } => page.key.map(|key| {
                    let StreamResourceKey::Audio(source) = key.source else {
                        panic!("PCM pin fixture")
                    };
                    PageKey {
                        source,
                        page_index: key.page_index,
                    }
                }),
                CacheSlot::Empty => None,
            })
            .collect::<Vec<_>>();
        let mut pinned = false;
        let info = measure(|| pinned = ready.endpoint.pin_pages(&resident_keys));
        assert_zero_probe(info);
        assert!(pinned);
        let info = measure(|| ready.endpoint.unpin_all());
        assert_zero_probe(info);

        // The probe itself must notice an allocation that is freed before its
        // measurement scope ends, so a false-zero result cannot pass silently.
        let observed = measure(|| {
            let value = std::hint::black_box(Box::new(std::hint::black_box(42_u64)));
            std::hint::black_box(&value);
            drop(value);
        });
        assert_eq!(observed.count_total, 1);
        assert_eq!(observed.count_current, 0);
        assert_eq!(observed.bytes_total, 8);
        assert_eq!(observed.bytes_current, 0);
    }
}
