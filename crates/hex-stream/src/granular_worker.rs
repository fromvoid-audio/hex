//! Dedicated granular page loop. Thread and queue lifecycle remain with the caller.
use crate::{
    CompletedGranularPage, GranularPageError, GranularPageProgress, GranularWorkerBank,
    VerifiedGranularSource,
};
use hex_engine::{Frame, StreamSourceKey};
use rtrb::{Consumer, Producer};
use std::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    thread,
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GranularWorkerSeek {
    Strict { max_preroll_frames: u64 },
    Segmented { segment_frames: u64 },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GranularWorkerKey {
    pub source: StreamSourceKey,
    /// Demand epoch captured when this request was submitted.
    pub epoch: u64,
    pub page_index: u64,
    pub seek: GranularWorkerSeek,
}
pub struct GranularWorkerRequest {
    pub key: GranularWorkerKey,
    pub frames: Box<[Frame]>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GranularWorkerError {
    UnknownSource,
    Page(GranularPageError),
}
pub enum GranularWorkerResult {
    Ready(CompletedGranularPage),
    /// Obsolete demand; the page contents must not be published.
    Cancelled {
        frames: Box<[Frame]>,
    },
    Failed {
        error: GranularWorkerError,
        frames: Box<[Frame]>,
    },
}
pub struct GranularWorkerOutcome {
    pub key: GranularWorkerKey,
    pub result: GranularWorkerResult,
}
pub struct GranularWorkerQueues {
    pub requests: Consumer<GranularWorkerRequest>,
    pub results: Producer<GranularWorkerOutcome>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GranularWorkerExitReason {
    Stopped,
    /// An internal page-custody invariant failed. Do not publish the page.
    CustodyFault(GranularWorkerKey),
}
pub struct GranularWorkerExit {
    pub reason: GranularWorkerExitReason,
    pub sources: Vec<VerifiedGranularSource>,
    pub queues: GranularWorkerQueues,
    pub interrupted: Option<GranularWorkerRequest>,
    pub held: Option<GranularWorkerOutcome>,
}

/// Run a bounded bank of verified sources on one dedicated worker thread.
///
/// The caller creates finite SPSC queues and admits their storage, the page pool,
/// source bank and thread before starting. The bank must pass control-side
/// DATA admission before moving here; that quote excludes the page pool,
/// queues, earlier source construction and opaque/native/thread costs.
/// Each source retains its own renderer and
/// checkpoint history. Requests are handled FIFO, one at a time;
/// a full results queue holds one outcome and prevents further dequeue. Idle and
/// full queues wait up to 2 ms between polls, subject to thread scheduling; the
/// joining control owner may unpark the worker. Stop is checked between bounded
/// rendering steps and during source operations, but an operating-system I/O
/// call can delay it. On return, the joining control owner must drain the
/// returned request consumer, result consumer,
/// interrupted request and held outcome, and destroy them and every returned source on control.
/// The caller advances `demand_epoch` to supersede all requests from older
/// demand and must never reuse an epoch during this worker lifetime. A stale
/// active request is cancelled between and within steps; returned frames remain
/// caller-owned. A held outcome rechecks demand after reserving its result slot.
/// A later epoch change can race publication into that slot:
/// the consumer must serialize demand changes with publication and compare each
/// outcome epoch with its current demand before publication. Retire the worker
/// before epoch wrap. This loop does not admit sources to playback or establish
/// a catalog lease.
pub fn run_granular_worker(
    bank: GranularWorkerBank,
    queues: GranularWorkerQueues,
    stop: &AtomicBool,
    demand_epoch: &AtomicU64,
) -> GranularWorkerExit {
    run_inner(bank, queues, stop, demand_epoch, false, || {})
}

/// Event-driven worker entry used by `GranularWorkerControl`, which unparks
/// this thread on every submit, supersede, poll, and shutdown so idle/stopped
/// granular workers consume zero polling wakeups.
pub(crate) fn run_granular_worker_signaled(
    bank: GranularWorkerBank,
    queues: GranularWorkerQueues,
    stop: &AtomicBool,
    demand_epoch: &AtomicU64,
) -> GranularWorkerExit {
    run_inner(bank, queues, stop, demand_epoch, true, || {})
}

#[cfg(feature = "test-support")]
pub(crate) fn run_with_step_hook(
    bank: GranularWorkerBank,
    queues: GranularWorkerQueues,
    stop: &AtomicBool,
    demand_epoch: &AtomicU64,
    before_step: impl FnMut(),
) -> GranularWorkerExit {
    run_inner(bank, queues, stop, demand_epoch, false, before_step)
}

#[cfg(feature = "test-support")]
pub(crate) fn run_with_step_hook_signaled(
    bank: GranularWorkerBank,
    queues: GranularWorkerQueues,
    stop: &AtomicBool,
    demand_epoch: &AtomicU64,
    before_step: impl FnMut(),
) -> GranularWorkerExit {
    run_inner(bank, queues, stop, demand_epoch, true, before_step)
}

/// Cap on simultaneously open file descriptors inside the dedicated granular
/// worker so large warped banks stay well below macOS `RLIMIT_NOFILE = 256`.
const MAX_OPEN_GRANULAR_READERS: usize = 16;

fn enforce_open_reader_cap(
    sources: &mut [VerifiedGranularSource],
    last_used: &mut [u64],
    active_index: usize,
    tick: &mut u64,
) {
    *tick = tick.saturating_add(1);
    if let Some(slot) = last_used.get_mut(active_index) {
        *slot = *tick;
    }
    let mut open_count = sources.iter().filter(|s| s.is_reader_open()).count();
    let reserve = usize::from(!sources[active_index].is_reader_open());
    while open_count + reserve > MAX_OPEN_GRANULAR_READERS {
        let victim = sources
            .iter()
            .enumerate()
            .filter(|(idx, s)| *idx != active_index && s.is_reader_open())
            .min_by_key(|(idx, _)| last_used.get(*idx).copied().unwrap_or(0))
            .map(|(idx, _)| idx);
        let Some(victim) = victim else {
            break;
        };
        sources[victim].park_reader();
        if let Some(slot) = last_used.get_mut(victim) {
            *slot = 0;
        }
        open_count -= 1;
    }
}

fn run_inner(
    bank: GranularWorkerBank,
    mut queues: GranularWorkerQueues,
    stop: &AtomicBool,
    demand_epoch: &AtomicU64,
    signaled: bool,
    mut before_step: impl FnMut(),
) -> GranularWorkerExit {
    let wait_for_event = || {
        if stop.load(Ordering::Acquire) {
            return;
        }
        if signaled {
            thread::park();
        } else {
            thread::park_timeout(Duration::from_millis(2));
        }
    };
    let mut sources = bank.into_sources();
    let mut last_used = vec![0_u64; sources.len()];
    let mut reader_tick = 0_u64;
    // The bank is never resized or reordered while a job holds its index.
    let mut active: Option<(usize, GranularWorkerKey)> = None;
    let mut held: Option<GranularWorkerOutcome> = None;
    let mut interrupted = None;
    let mut fault = None;
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        if let Some(mut outcome) = held.take() {
            // Acquiring a freed slot observes the consumer's preceding demand change.
            // Sampling the epoch first could publish obsolete work as ready or failed.
            let Ok(slot) = queues.results.write_chunk_uninit(1) else {
                held = Some(outcome);
                wait_for_event();
                continue;
            };
            if outcome.key.epoch != demand_epoch.load(Ordering::Acquire) {
                let frames = match outcome.result {
                    GranularWorkerResult::Ready(page) => page.frames,
                    GranularWorkerResult::Failed { frames, .. }
                    | GranularWorkerResult::Cancelled { frames } => frames,
                };
                outcome.result = GranularWorkerResult::Cancelled { frames };
            }
            slot.fill_from_iter(std::iter::once(outcome));
            continue;
        }
        if let Some((index, key)) = active {
            let Some(source) = sources.get_mut(index) else {
                fault = Some(key);
                break;
            };
            if key.epoch != demand_epoch.load(Ordering::Acquire) {
                if let Some(frames) = source.reclaim() {
                    active = None;
                    held = Some(GranularWorkerOutcome {
                        key,
                        result: GranularWorkerResult::Cancelled { frames },
                    });
                } else {
                    fault = Some(key);
                    break;
                }
                continue;
            }
            before_step();
            match source.step(|| {
                stop.load(Ordering::Acquire) || key.epoch != demand_epoch.load(Ordering::Acquire)
            }) {
                Ok(GranularPageProgress::Pending) => {}
                Ok(GranularPageProgress::Complete) => {
                    if let Some(page) = source.take_complete() {
                        active = None;
                        held = Some(GranularWorkerOutcome {
                            key,
                            result: GranularWorkerResult::Ready(page),
                        });
                    } else {
                        fault = Some(key);
                        break;
                    }
                }
                Err(error) => {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    if let Some(frames) = source.reclaim() {
                        active = None;
                        let result = if key.epoch != demand_epoch.load(Ordering::Acquire) {
                            GranularWorkerResult::Cancelled { frames }
                        } else {
                            GranularWorkerResult::Failed {
                                error: GranularWorkerError::Page(error),
                                frames,
                            }
                        };
                        held = Some(GranularWorkerOutcome { key, result });
                    } else {
                        fault = Some(key);
                        break;
                    }
                }
            }
            continue;
        }
        let Ok(request) = queues.requests.pop() else {
            wait_for_event();
            continue;
        };
        let GranularWorkerRequest { key, frames } = request;
        if key.epoch != demand_epoch.load(Ordering::Acquire) {
            held = Some(GranularWorkerOutcome {
                key,
                result: GranularWorkerResult::Cancelled { frames },
            });
            continue;
        }
        let Some(index) = sources
            .iter()
            .position(|source| source.metadata().key() == key.source)
        else {
            held = Some(GranularWorkerOutcome {
                key,
                result: GranularWorkerResult::Failed {
                    error: GranularWorkerError::UnknownSource,
                    frames,
                },
            });
            continue;
        };
        enforce_open_reader_cap(&mut sources, &mut last_used, index, &mut reader_tick);
        let source = &mut sources[index];
        let result = match key.seek {
            GranularWorkerSeek::Strict { max_preroll_frames } => {
                source.begin_page(key.page_index, frames, max_preroll_frames, || {
                    stop.load(Ordering::Acquire)
                        || key.epoch != demand_epoch.load(Ordering::Acquire)
                })
            }
            GranularWorkerSeek::Segmented { segment_frames } => {
                source.begin_page_replaying(key.page_index, frames, segment_frames, || {
                    stop.load(Ordering::Acquire)
                        || key.epoch != demand_epoch.load(Ordering::Acquire)
                })
            }
        };
        match result {
            Ok(()) => active = Some((index, key)),
            Err((error, frames)) => {
                if stop.load(Ordering::Acquire) {
                    interrupted = Some(GranularWorkerRequest { key, frames });
                    break;
                }
                let result = if key.epoch != demand_epoch.load(Ordering::Acquire) {
                    GranularWorkerResult::Cancelled { frames }
                } else {
                    GranularWorkerResult::Failed {
                        error: GranularWorkerError::Page(error),
                        frames,
                    }
                };
                held = Some(GranularWorkerOutcome { key, result });
            }
        }
    }
    if let Some((index, key)) = active {
        if let Some(frames) = sources
            .get_mut(index)
            .and_then(VerifiedGranularSource::reclaim)
        {
            interrupted = Some(GranularWorkerRequest { key, frames });
        } else {
            fault = Some(key);
        }
    }
    GranularWorkerExit {
        reason: fault.map_or(
            GranularWorkerExitReason::Stopped,
            GranularWorkerExitReason::CustodyFault,
        ),
        sources,
        queues,
        interrupted,
        held,
    }
}
