//! Deterministic slow-worker evidence for the bounded page lookahead (#4075).
use super::*;
use hex_project::{Command, Created, Editor, Project};
use rtrb::RingBuffer;
use std::collections::VecDeque;

const BLOCK: usize = 256;
const PAGES: u64 = 12;
/// Worker latency in callbacks: 6 blocks, 1536 frames, about 32 ms at 48 kHz,
/// longer than a block but shorter than half a page.
const LATENCY: u64 = 6;

fn source() -> StreamSourceKey {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 4).expect("editor");
    match editor
        .execute(Command::AddAsset {
            path: "prefetch-test.wav".into(),
            sample_rate: 48_000,
            frames: PAGES * STREAM_PAGE_FRAMES as u64,
        })
        .expect("asset")
    {
        Created::Asset(asset) => StreamSourceKey::new(91, asset),
        other => panic!("unexpected asset result: {other:?}"),
    }
}

/// An endpoint with a test-owned worker that publishes each request exactly
/// `LATENCY` callbacks after it was queued, recycling returned buffers.
struct SlowWorker {
    endpoint: Endpoint,
    requests: Consumer<PageRequest>,
    ready: Producer<Box<PageBuffer>>,
    returns: Consumer<Box<PageBuffer>>,
    in_flight: VecDeque<(u64, ResourcePageKey)>,
    max_in_flight: usize,
}

impl SlowWorker {
    fn new(cache_pages: usize, key: StreamSourceKey) -> Self {
        let (request_tx, requests) = RingBuffer::new(4);
        let (ready, ready_rx) = RingBuffer::new(cache_pages);
        let (return_tx, returns) = RingBuffer::new(cache_pages);
        let (_failures, failure_rx) = RingBuffer::new(4);
        let endpoint = Endpoint::new(
            cache_pages,
            4,
            vec![key.into()],
            EndpointQueues {
                request_tx,
                ready_rx,
                return_tx,
                failure_rx,
            },
        )
        .expect("preallocated endpoint");
        Self {
            endpoint,
            requests,
            ready,
            returns,
            in_flight: VecDeque::new(),
            max_in_flight: 0,
        }
    }

    fn work(&mut self, tick: u64) {
        while self.returns.pop().is_ok() {}
        while let Ok(PageRequest(key)) = self.requests.pop() {
            self.in_flight.push_back((tick + LATENCY, key));
        }
        self.max_in_flight = self.max_in_flight.max(self.in_flight.len());
        while self.in_flight.front().is_some_and(|(due, _)| *due <= tick) {
            let Some((_, key)) = self.in_flight.pop_front() else {
                break;
            };
            let mut page = PageBuffer::new().expect("page buffer");
            page.key = Some(key);
            page.valid_frames = STREAM_PAGE_FRAMES;
            page.frames.fill([key.page_index as f32; 2]);
            assert!(
                self.ready.push(page).is_ok(),
                "ready queue sized to the pool"
            );
        }
    }
}

struct Run {
    missed_blocks: u64,
    stats: EndpointStats,
    max_in_flight: usize,
}

/// Play the whole source once in blocks, rendering silence for a missing
/// page (the playhead keeps moving, as the session renderer does).
fn play(cache_pages: usize, lookahead: bool) -> Run {
    let key = source();
    let mut worker = SlowWorker::new(cache_pages, key);
    let total = PAGES * STREAM_PAGE_FRAMES as u64;
    let mut missed_blocks = 0;
    let mut position = 0_u64;
    let mut tick = 0_u64;
    // Page 0 is warmed before Play, as readiness does.
    worker.endpoint.request_page(key, 0);
    while worker.endpoint.page(key, 0).is_none() {
        worker.work(tick);
        worker.endpoint.pump_ready(4);
        tick += 1;
    }
    let warm_misses = worker.endpoint.stats().misses;
    while position < total {
        worker.work(tick);
        worker.endpoint.pump_ready(4);
        let page_index = position / STREAM_PAGE_FRAMES as u64;
        let offset = (position % STREAM_PAGE_FRAMES as u64) as usize;
        match worker.endpoint.page(key, page_index) {
            Some(page) => assert_eq!(page.frames[offset], [page_index as f32; 2]),
            None => {
                missed_blocks += 1;
                worker.endpoint.request_page(key, page_index);
            }
        }
        let read_to = offset + BLOCK;
        if lookahead && read_to >= STREAM_PAGE_FRAMES / 2 && page_index + 1 < PAGES {
            worker.endpoint.prefetch_page(key, page_index + 1);
        }
        position += BLOCK as u64;
        tick += 1;
    }
    let mut stats = worker.endpoint.stats();
    stats.misses -= warm_misses;
    Run {
        missed_blocks,
        stats,
        max_in_flight: worker.max_in_flight,
    }
}

#[test]
fn lookahead_hides_worker_latency_with_bounded_requests() {
    for cache_pages in [3, 4, 8] {
        let demand_only = play(cache_pages, false);
        // Every page after the first is first touched by the playhead and
        // misses for the whole worker latency, plus the block in which the
        // worker first sees the request.
        assert_eq!(
            demand_only.missed_blocks,
            (PAGES - 1) * (LATENCY + 1),
            "pool {cache_pages}"
        );
        let ahead = play(cache_pages, true);
        assert_eq!(ahead.missed_blocks, 0, "pool {cache_pages}");
        assert_eq!(ahead.stats.misses, 0, "pool {cache_pages}");
        // One request per page, never more than one speculative in flight.
        assert_eq!(ahead.stats.queued_requests, PAGES, "pool {cache_pages}");
        assert_eq!(ahead.stats.rejected_requests, 0, "pool {cache_pages}");
        assert_eq!(ahead.max_in_flight, 1, "pool {cache_pages}");
        assert!(
            ahead.stats.prefetched_requests < PAGES,
            "pool {cache_pages}: {:?}",
            ahead.stats
        );
    }
}

fn full_page(key: StreamSourceKey, page_index: u64) -> Box<PageBuffer> {
    let mut page = PageBuffer::new().expect("page buffer");
    page.key = Some(ResourcePageKey {
        source: key.into(),
        page_index,
    });
    page.valid_frames = STREAM_PAGE_FRAMES;
    page
}

#[test]
fn prefetch_yields_to_demand_and_never_evicts_the_read_or_pinned_page() {
    let key = source();
    let mut worker = SlowWorker::new(4, key);
    let endpoint = &mut worker.endpoint;
    // A pending demand request suppresses lookahead.
    endpoint.request_page(key, 7);
    endpoint.prefetch_page(key, 3);
    assert_eq!(endpoint.stats().queued_requests, 1);
    assert_eq!(endpoint.stats().prefetched_requests, 0);
    assert!(worker.requests.pop().is_ok());
    worker.endpoint.pending_remove(ResourcePageKey {
        source: key.into(),
        page_index: 7,
    });
    let endpoint = &mut worker.endpoint;
    // Fill the cache (3 pages): the playhead reads page 1, page 5 is pinned.
    for page_index in [0, 1, 5] {
        assert!(endpoint.insert_ready_with_mask(full_page(key, page_index), 0));
    }
    assert!(endpoint.pin_pages(&[PageKey {
        source: key,
        page_index: 5,
    }]));
    // Cached pages are never re-requested.
    endpoint.prefetch_page(key, 1);
    assert_eq!(endpoint.stats().prefetched_requests, 0);
    // Scheduling lookahead keeps all resident pages until the worker returns.
    endpoint.prefetch_page(key, 2);
    let cached = |endpoint: &Endpoint, page_index| {
        endpoint
            .cache_lookup(PageKey {
                source: key,
                page_index,
            })
            .is_some()
    };
    assert!(cached(endpoint, 0) && cached(endpoint, 1) && cached(endpoint, 5));
    assert_eq!(endpoint.stats().prefetched_requests, 1);
    assert_eq!(
        worker
            .requests
            .pop()
            .map(|request| request.0.page_index)
            .ok(),
        Some(2)
    );
    assert!(worker.ready.push(full_page(key, 2)).is_ok());
    assert_eq!(worker.endpoint.pump_ready(1), 1);
    assert!(!cached(&worker.endpoint, 0));
    assert!(cached(&worker.endpoint, 1) && cached(&worker.endpoint, 5));
    // With every other page protected nothing is evicted or requested.
    let mut pinned = SlowWorker::new(3, key);
    for page_index in [4, 5] {
        assert!(
            pinned
                .endpoint
                .insert_ready_with_mask(full_page(key, page_index), 0)
        );
    }
    assert!(pinned.endpoint.pin_pages(&[PageKey {
        source: key,
        page_index: 4,
    }]));
    pinned.endpoint.prefetch_page(key, 6);
    assert_eq!(pinned.endpoint.stats().prefetched_requests, 0);
    assert!(pinned.requests.pop().is_err());
    // Unknown sources are ignored without counting a rejection.
    let other = StreamSourceKey::new(92, key.asset());
    pinned.endpoint.prefetch_page(other, 1);
    assert_eq!(pinned.endpoint.stats().rejected_requests, 0);
}

#[cfg(feature = "allocation-probe")]
#[test]
fn prefetch_eviction_and_request_are_allocation_free() {
    let key = source();
    let mut worker = SlowWorker::new(3, key);
    for page_index in [0, 1] {
        assert!(
            worker
                .endpoint
                .insert_ready_with_mask(full_page(key, page_index), 0)
        );
    }
    let info = allocation_counter::measure(|| {
        worker.endpoint.prefetch_page(key, 2);
        worker.endpoint.prefetch_page(key, 3);
    });
    assert_eq!(
        (
            info.count_total,
            info.bytes_total,
            info.count_current,
            info.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(worker.endpoint.stats().prefetched_requests, 1);
}

#[test]
fn ready_lookahead_waits_for_demand_and_yields_its_buffer_to_a_missing_page() {
    let key = source();
    let mut worker = SlowWorker::new(3, key);
    for index in [0, 1] {
        assert!(
            worker
                .endpoint
                .insert_ready_with_mask(full_page(key, index), 0)
        );
    }
    worker.endpoint.prefetch_page(key, 2);
    assert_eq!(worker.requests.pop().unwrap().0.page_index, 2);
    assert!(worker.ready.push(full_page(key, 2)).is_ok());
    let current = [crate::StreamResourceExtent {
        source: key.into(),
        page_index: 1,
        start: 0,
        end: 1,
    }];
    worker.endpoint.protect_present(16, &current);
    #[cfg(feature = "allocation-probe")]
    let allocation = allocation_counter::measure(|| {
        assert_eq!(
            worker.endpoint.pump_ready_for_pending(3, &[(16, &current)]),
            0
        );
    });
    #[cfg(not(feature = "allocation-probe"))]
    assert_eq!(
        worker.endpoint.pump_ready_for_pending(3, &[(16, &current)]),
        0
    );
    #[cfg(feature = "allocation-probe")]
    assert_eq!(
        (
            allocation.count_total,
            allocation.bytes_total,
            allocation.count_current,
            allocation.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(worker.endpoint.ready_rx.peek().is_ok_and(|page| {
        page.key
            == Some(ResourcePageKey {
                source: key.into(),
                page_index: 2,
            })
    }));
    assert!(worker.endpoint.has_extent(current[0]));
    assert!(worker.returns.pop().is_err());

    let missing = [crate::StreamResourceExtent {
        page_index: 3,
        ..current[0]
    }];
    assert_eq!(
        worker.endpoint.pump_ready_for_pending(3, &[(16, &missing)]),
        0
    );
    assert_eq!(worker.returns.pop().unwrap().key.unwrap().page_index, 2);
    worker.endpoint.request_page(key, 3);
    assert_eq!(worker.requests.pop().unwrap().0.page_index, 3);
    assert!(worker.ready.push(full_page(key, 3)).is_ok());
    assert_eq!(
        worker.endpoint.pump_ready_for_pending(3, &[(16, &missing)]),
        1
    );
    assert!(worker.endpoint.has_extent(missing[0]));
    assert!(worker.endpoint.has_extent(current[0]));
}

#[test]
fn ready_lookahead_is_installed_when_exactly_demanded() {
    let key = source();
    let mut worker = SlowWorker::new(3, key);
    for index in [0, 1] {
        assert!(
            worker
                .endpoint
                .insert_ready_with_mask(full_page(key, index), 0)
        );
    }
    worker.endpoint.prefetch_page(key, 2);
    assert_eq!(worker.requests.pop().unwrap().0.page_index, 2);
    assert!(worker.ready.push(full_page(key, 2)).is_ok());
    let demanded = [crate::StreamResourceExtent {
        source: key.into(),
        page_index: 2,
        start: 0,
        end: 1,
    }];
    assert_eq!(
        worker
            .endpoint
            .pump_ready_for_pending(3, &[(16, &demanded)]),
        1
    );
    assert!(worker.endpoint.has_extent(demanded[0]));
    assert_eq!(worker.endpoint.pending_count, 0);
    assert!(worker.endpoint.prefetch_key.is_none());
}

#[test]
fn unrelated_queued_demand_cannot_be_blocked_by_ready_lookahead() {
    let key = source();
    let mut worker = SlowWorker::new(3, key);
    for index in [0, 1] {
        assert!(
            worker
                .endpoint
                .insert_ready_with_mask(full_page(key, index), 0)
        );
    }
    worker.endpoint.prefetch_page(key, 2);
    assert_eq!(worker.requests.pop().unwrap().0.page_index, 2);
    assert!(worker.ready.push(full_page(key, 2)).is_ok());
    worker.endpoint.request_page(key, 3);
    assert_eq!(worker.requests.pop().unwrap().0.page_index, 3);
    let resident = [crate::StreamResourceExtent {
        source: key.into(),
        page_index: 1,
        start: 0,
        end: 1,
    }];
    assert_eq!(
        worker
            .endpoint
            .pump_ready_for_pending(3, &[(16, &resident)]),
        0
    );
    assert_eq!(worker.returns.pop().unwrap().key.unwrap().page_index, 2);
    assert!(worker.endpoint.prefetch_key.is_none());
    assert_eq!(worker.endpoint.pending_count, 1);
}

#[test]
fn direct_missing_demand_releases_ready_lookahead_before_requesting_worker() {
    let key = source();
    let mut worker = SlowWorker::new(3, key);
    for index in [0, 1] {
        assert!(
            worker
                .endpoint
                .insert_ready_with_mask(full_page(key, index), 0)
        );
    }
    worker.endpoint.prefetch_page(key, 2);
    assert_eq!(worker.requests.pop().unwrap().0.page_index, 2);
    assert!(worker.ready.push(full_page(key, 2)).is_ok());
    #[cfg(feature = "allocation-probe")]
    let allocation = allocation_counter::measure(|| worker.endpoint.request_page(key, 3));
    #[cfg(not(feature = "allocation-probe"))]
    worker.endpoint.request_page(key, 3);
    #[cfg(feature = "allocation-probe")]
    assert_eq!(
        (
            allocation.count_total,
            allocation.bytes_total,
            allocation.count_current,
            allocation.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(worker.returns.pop().unwrap().key.unwrap().page_index, 2);
    assert_eq!(worker.requests.pop().unwrap().0.page_index, 3);
    assert!(worker.endpoint.prefetch_key.is_none());
}

#[test]
fn cancelled_coalesced_demand_does_not_evict_a_resident_page() {
    let key = source();
    let mut worker = SlowWorker::new(3, key);
    for index in [0, 1] {
        assert!(
            worker
                .endpoint
                .insert_ready_with_mask(full_page(key, index), 0)
        );
    }
    worker.endpoint.prefetch_page(key, 2);
    assert_eq!(worker.requests.pop().unwrap().0.page_index, 2);
    // A readiness intent for the prefetched page coalesces with its worker request.
    worker.endpoint.queue_readiness_demand(
        ResourcePageKey {
            source: key.into(),
            page_index: 2,
        },
        16,
    );
    worker.endpoint.cancel_readiness_demand(16);
    // Before the worker returns, that intent is cancelled and current audio
    // again needs only page 1; page 0 is resident for a later loop boundary.
    assert!(worker.ready.push(full_page(key, 2)).is_ok());
    let current = [crate::StreamResourceExtent {
        source: key.into(),
        page_index: 1,
        start: 0,
        end: 1,
    }];
    worker.endpoint.protect_present(16, &current);
    worker.endpoint.pump_ready_for_pending(3, &[(16, &current)]);
    assert!(worker.endpoint.has_extent(current[0]));
    assert!(
        worker.endpoint.has_extent(crate::StreamResourceExtent {
            page_index: 0,
            ..current[0]
        }),
        "cancelled lookahead evicted a resident future page"
    );
}

#[test]
fn coalesced_direct_demand_survives_competing_request() {
    let key = source();
    let mut worker = SlowWorker::new(3, key);
    for index in [0, 1] {
        assert!(
            worker
                .endpoint
                .insert_ready_with_mask(full_page(key, index), 0)
        );
    }
    worker.endpoint.prefetch_page(key, 2);
    assert_eq!(worker.requests.pop().unwrap().0.page_index, 2);
    worker.endpoint.request_page(key, 2);
    assert!(worker.ready.push(full_page(key, 2)).is_ok());
    worker.endpoint.request_page(key, 3);
    assert_eq!(worker.endpoint.pump_ready(1), 1);
    assert!(worker.endpoint.page(key, 2).is_some());
    assert_eq!(worker.requests.pop().unwrap().0.page_index, 3);
}

#[test]
fn competing_demand_preserves_a_page_still_wanted_by_another_role() {
    let key = source();
    let mut worker = SlowWorker::new(3, key);
    worker.endpoint.prefetch_page(key, 2);
    assert_eq!(worker.requests.pop().unwrap().0.page_index, 2);
    let page = ResourcePageKey {
        source: key.into(),
        page_index: 2,
    };
    worker.endpoint.queue_readiness_demand(page, 16);
    worker.endpoint.queue_readiness_demand(page, 32);
    worker.endpoint.cancel_readiness_demand(16);
    assert!(worker.ready.push(full_page(key, 2)).is_ok());
    worker.endpoint.request_page(key, 3);
    assert!(
        worker
            .endpoint
            .ready_rx
            .peek()
            .is_ok_and(|ready| ready.key == Some(page))
    );
    assert_eq!(worker.requests.pop().unwrap().0.page_index, 3);
}
