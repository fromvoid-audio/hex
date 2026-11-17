//! Dedicated granular page loop. Thread and queue lifecycle remain with the caller.
use crate::{
    CompletedGranularPage, GranularPageError, GranularPageProgress, VerifiedGranularSource,
};
use hex_engine::{Frame, StreamSourceKey};
use rtrb::{Consumer, Producer, PushError};
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
    /// Empty, duplicate-key or oversized bank; requests remain untouched.
    InvalidSourceBank,
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
/// source bank and thread before starting. At most 512 source slots (including
/// spare vector capacity) are accepted; keys must be distinct and the bank must
/// be nonempty. Invalid banks return without consuming requests. The caller
/// admits all source readers, scratch and checkpoints in aggregate; the slot
/// limit alone is not a memory quote. Each source retains its own renderer and
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
/// caller-owned. Outcomes already in the result queue can race an epoch change:
/// the consumer must serialize demand changes with publication and compare each
/// outcome epoch with its current demand before publication. Retire the worker
/// before epoch wrap. This loop does not admit sources to playback or establish
/// a catalog lease.
pub fn run_granular_worker(
    sources: Vec<VerifiedGranularSource>,
    queues: GranularWorkerQueues,
    stop: &AtomicBool,
    demand_epoch: &AtomicU64,
) -> GranularWorkerExit {
    run_inner(sources, queues, stop, demand_epoch, || {})
}

#[cfg(feature = "test-support")]
pub(crate) fn run_with_step_hook(
    sources: Vec<VerifiedGranularSource>,
    queues: GranularWorkerQueues,
    stop: &AtomicBool,
    demand_epoch: &AtomicU64,
    before_step: impl FnMut(),
) -> GranularWorkerExit {
    run_inner(sources, queues, stop, demand_epoch, before_step)
}

fn run_inner(
    mut sources: Vec<VerifiedGranularSource>,
    mut queues: GranularWorkerQueues,
    stop: &AtomicBool,
    demand_epoch: &AtomicU64,
    mut before_step: impl FnMut(),
) -> GranularWorkerExit {
    if sources.is_empty()
        || sources.capacity() > 512
        || sources.iter().enumerate().any(|(index, source)| {
            sources[..index]
                .iter()
                .any(|previous| previous.metadata().key() == source.metadata().key())
        })
    {
        return GranularWorkerExit {
            reason: GranularWorkerExitReason::InvalidSourceBank,
            sources,
            queues,
            interrupted: None,
            held: None,
        };
    }
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
            if outcome.key.epoch != demand_epoch.load(Ordering::Acquire) {
                let frames = match outcome.result {
                    GranularWorkerResult::Ready(page) => page.frames,
                    GranularWorkerResult::Failed { frames, .. }
                    | GranularWorkerResult::Cancelled { frames } => frames,
                };
                outcome.result = GranularWorkerResult::Cancelled { frames };
            }
            if let Err(PushError::Full(outcome)) = queues.results.push(outcome) {
                held = Some(outcome);
                thread::park_timeout(Duration::from_millis(2));
            }
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
            thread::park_timeout(Duration::from_millis(2));
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
        let Some((index, source)) = sources
            .iter_mut()
            .enumerate()
            .find(|(_, source)| source.metadata().key() == key.source)
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
