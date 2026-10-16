use crate::types::{PageKey, StreamFailure};
use hex_engine::{BorrowedMediaPage, Frame, MediaPages, STREAM_PAGE_FRAMES, StreamSourceKey};
use rtrb::{Consumer, Producer, PushError};
#[cfg(test)]
use std::cell::Cell;
use std::hash::{Hash, Hasher};

#[derive(Clone, Copy, Debug)]
pub(crate) struct PageRequest(pub(crate) PageKey);

pub(crate) struct PageBuffer {
    pub(crate) key: Option<PageKey>,
    pub(crate) frames: Box<[Frame]>,
    pub(crate) valid_frames: usize,
}

impl PageBuffer {
    #[cfg(test)]
    pub(crate) fn new() -> Result<Box<Self>, ()> {
        Self::new_with_capacity().map(|(page, _)| page)
    }
    pub(crate) fn new_with_capacity() -> Result<(Box<Self>, usize), ()> {
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(STREAM_PAGE_FRAMES)
            .map_err(|_| ())?;
        frames.resize(STREAM_PAGE_FRAMES, [0.0; 2]);
        let old_capacity = frames.capacity();
        Ok((
            Box::new(Self {
                key: None,
                frames: frames.into_boxed_slice(),
                valid_frames: 0,
            }),
            old_capacity,
        ))
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
    Occupied { page: Box<PageBuffer>, pinned: bool },
}

#[derive(Clone, Copy)]
enum PendingSlot {
    Empty,
    Key(PageKey),
}

pub(crate) struct Endpoint {
    cache: Vec<CacheSlot>,
    max_cached_pages: usize,
    cache_count: usize,
    eviction_cursor: usize,
    pending: Vec<PendingSlot>,
    pending_mask: usize,
    source_keys: Vec<StreamSourceKey>,
    request_tx: Producer<PageRequest>,
    ready_rx: Consumer<Box<PageBuffer>>,
    return_tx: Producer<Box<PageBuffer>>,
    failure_rx: Consumer<StreamFailure>,
    held_returns: Vec<Box<PageBuffer>>,
    misses: u64,
    queued_requests: u64,
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
    pub(crate) fn new(
        cache_pages: usize,
        request_capacity: usize,
        sources: Vec<StreamSourceKey>,
        queues: EndpointQueues,
    ) -> Result<Self, ()> {
        let max_cached_pages = cache_pages.saturating_sub(1);
        let cache_count = max_cached_pages.checked_next_power_of_two().ok_or(())?;
        let pending_count = request_capacity
            .checked_mul(2)
            .ok_or(())?
            .checked_next_power_of_two()
            .ok_or(())?;
        let mut cache = Vec::new();
        let cache_slots = cache_count.checked_mul(2).ok_or(())?;
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
            source_keys: sources,
            request_tx: queues.request_tx,
            ready_rx: queues.ready_rx,
            return_tx: queues.return_tx,
            failure_rx: queues.failure_rx,
            held_returns,
            misses: 0,
            queued_requests: 0,
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
            if self.insert_ready(page) {
                installed += 1;
            }
        }
        self.flush_held_returns();
        installed
    }

    pub fn take_failures(&mut self, output: &mut [StreamFailure]) -> usize {
        let mut count = 0;
        for item in output {
            let Ok(failure) = self.failure_rx.pop() else {
                break;
            };
            *item = failure;
            self.pending_remove(PageKey {
                source: failure.key,
                page_index: failure.page_index,
            });
            count += 1;
        }
        count
    }

    pub fn stats(&self) -> EndpointStats {
        EndpointStats {
            misses: self.misses,
            queued_requests: self.queued_requests,
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
                    *pinned = true;
                }
            }
        }
        true
    }

    pub fn unpin_all(&mut self) {
        for slot in &mut self.cache {
            if let CacheSlot::Occupied { pinned, .. } = slot {
                *pinned = false;
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

    fn pending_remove(&mut self, key: PageKey) {
        let mut slot = page_slot(key, self.pending_mask);
        for _ in 0..self.pending.len() {
            #[cfg(test)]
            self.pending_probe_count
                .set(self.pending_probe_count.get().saturating_add(1));
            match self.pending[slot] {
                PendingSlot::Key(found) if found == key => {
                    self.pending[slot] = PendingSlot::Empty;
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

    fn queue_request(&mut self, key: PageKey) {
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
                self.queued_requests = self.queued_requests.saturating_add(1);
            }
            Err(PushError::Full(_)) => {
                self.rejected_requests = self.rejected_requests.saturating_add(1);
            }
        }
    }

    fn cache_lookup(&self, key: PageKey) -> Option<usize> {
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

    fn insert_ready(&mut self, page: Box<PageBuffer>) -> bool {
        let Some(key) = page.key else {
            self.stale_pages = self.stale_pages.saturating_add(1);
            self.return_buffer(page);
            return false;
        };
        if let Some(index) = self.cache_lookup(key) {
            let old = std::mem::replace(
                &mut self.cache[index],
                CacheSlot::Occupied {
                    page,
                    pinned: false,
                },
            );
            if let CacheSlot::Occupied { page, pinned } = old {
                if let CacheSlot::Occupied {
                    pinned: current, ..
                } = &mut self.cache[index]
                {
                    *current = pinned;
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
            pinned: false,
        };
        self.cache_count += 1;
        true
    }

    fn cache_remove(&mut self, index: usize) -> Option<(Box<PageBuffer>, bool)> {
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

    fn cache_insertion_slot(&self, key: PageKey) -> Option<usize> {
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
            if matches!(self.cache[index], CacheSlot::Occupied { pinned: false, .. }) {
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
        let key = PageKey {
            source: key,
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
        self.queue_request(PageKey {
            source: key,
            page_index,
        });
    }
}

fn probe_distance(home: usize, slot: usize, mask: usize) -> usize {
    slot.wrapping_sub(home) & mask
}

fn page_slot(key: PageKey, mask: usize) -> usize {
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
            source_keys,
            request_tx: _,
            ready_rx: _,
            return_tx: _,
            failure_rx: _,
            held_returns,
            misses: _,
            queued_requests: _,
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
            source_keys: crate::AllocationData::of::<StreamSourceKey>(source_keys.capacity()),
            held_returns: crate::AllocationData::of::<Box<PageBuffer>>(held_returns.capacity()),
        }
    }
}

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
            vec![key],
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
        page.key = Some(key);
        page.valid_frames = 1;
        page.frames[0] = [key.page_index as f32, -(key.page_index as f32)];
        page
    }

    #[test]
    fn cache_open_addressing_preserves_colliding_pages() {
        let mut test = endpoint(4, 4, 4);
        let source = test.endpoint.source_keys[0];
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
        let source = test.endpoint.source_keys[0];
        let keys = colliding_pages(source, test.endpoint.pending_mask, 3);
        for key in &keys {
            test.endpoint.request_page(key.source, key.page_index);
        }
        test.endpoint.pending_remove(keys[1]);
        test.endpoint
            .request_page(keys[2].source, keys[2].page_index);
        assert_eq!(test.endpoint.stats().coalesced_requests, 1);
        assert_eq!(test.requests.pop().expect("first request").0, keys[0]);
        assert_eq!(
            test.requests.pop().expect("middle original request").0,
            keys[1]
        );
        assert_eq!(
            test.requests.pop().expect("third original request").0,
            keys[2]
        );
        assert!(test.requests.pop().is_err());

        // The home slot is the last index, so this cluster crosses the end of
        // the table. Repeated middle deletion must keep the later key
        // discoverable without leaving tombstones behind.
        let mut turnover = endpoint(3, 8, 2);
        let source = turnover.endpoint.source_keys[0];
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
                assert_eq!(turnover.requests.pop().expect("queued request").0, *key);
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
        assert_eq!(turnover.requests.pop().expect("fresh request").0, fresh);

        let mut full = endpoint(3, 1, 2);
        let full_source = full.endpoint.source_keys[0];
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
        assert_eq!(full.requests.pop().expect("drain bounded request").0, first);
        full.endpoint.request_page(second.source, second.page_index);
        assert_eq!(full.endpoint.stats().queued_requests, 2);
        assert_eq!(full.requests.pop().expect("retry after drain").0, second);
    }

    #[test]
    fn cache_backward_shift_survives_wraparound_turnover_without_dead_entries() {
        let mut test = endpoint(5, 4, 8);
        let source = test.endpoint.source_keys[0];
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
    fn pinned_pages_refuse_replacement_and_full_return_ring_uses_held_slot() {
        let mut test = endpoint(2, 2, 1);
        let source = test.endpoint.source_keys[0];
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
            Some(second)
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
            Some(second)
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
        let source = pending.endpoint.source_keys[0];
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
            assert_eq!(pending.requests.pop().expect("queued page").0, *key);
        }
        let failure = StreamFailure {
            key: source,
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
        assert_eq!(pending.requests.pop().expect("retried failure").0, keys[1]);

        let mut ready = endpoint(4, 4, 4);
        let source = ready.endpoint.source_keys[0];
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
        assert!(initial.contains(&returned_key));

        let resident_keys = ready
            .endpoint
            .cache
            .iter()
            .filter_map(|slot| match slot {
                CacheSlot::Occupied { page, .. } => page.key,
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
