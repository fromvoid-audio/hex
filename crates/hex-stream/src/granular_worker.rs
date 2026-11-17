//! Dedicated granular page loop. Thread and queue lifecycle remain with the caller.
use crate::{
    CompletedGranularPage, GranularPageError, GranularPageProgress, VerifiedGranularSource,
};
use hex_engine::{Frame, StreamSourceKey};
use rtrb::{Consumer, Producer, PushError};
use std::{
    sync::atomic::{AtomicBool, Ordering},
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
    pub source: VerifiedGranularSource,
    pub queues: GranularWorkerQueues,
    pub interrupted: Option<GranularWorkerRequest>,
    pub held: Option<GranularWorkerOutcome>,
}

/// Run one verified source on a caller-owned dedicated worker thread.
///
/// The caller creates finite SPSC queues and admits their storage, the page pool,
/// source and thread before starting. Requests are handled FIFO, one at a time;
/// a full results queue holds one outcome and prevents further dequeue. Idle and
/// full queues wait up to 2 ms between polls, subject to thread scheduling; the
/// joining control owner may unpark the worker. Stop is checked between bounded rendering steps and during source
/// operations, but an operating-system I/O call can delay it. On return, the
/// joining control owner must drain the returned request consumer, result consumer,
/// interrupted request and held outcome, and destroy them and the source on control.
/// This loop does not admit sources to playback or establish a catalog lease.
pub fn run_granular_worker(
    mut source: VerifiedGranularSource,
    mut queues: GranularWorkerQueues,
    stop: &AtomicBool,
) -> GranularWorkerExit {
    let mut active: Option<GranularWorkerKey> = None;
    let mut held: Option<GranularWorkerOutcome> = None;
    let mut interrupted = None;
    let mut fault = None;
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        if let Some(outcome) = held.take() {
            if let Err(PushError::Full(outcome)) = queues.results.push(outcome) {
                held = Some(outcome);
                thread::park_timeout(Duration::from_millis(2));
            }
            continue;
        }
        if let Some(key) = active {
            match source.step(|| stop.load(Ordering::Acquire)) {
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
                        held = Some(GranularWorkerOutcome {
                            key,
                            result: GranularWorkerResult::Failed {
                                error: GranularWorkerError::Page(error),
                                frames,
                            },
                        });
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
        if key.source != source.metadata().key() {
            held = Some(GranularWorkerOutcome {
                key,
                result: GranularWorkerResult::Failed {
                    error: GranularWorkerError::UnknownSource,
                    frames,
                },
            });
            continue;
        }
        let result = match key.seek {
            GranularWorkerSeek::Strict { max_preroll_frames } => {
                source.begin_page(key.page_index, frames, max_preroll_frames, || {
                    stop.load(Ordering::Acquire)
                })
            }
            GranularWorkerSeek::Segmented { segment_frames } => {
                source.begin_page_replaying(key.page_index, frames, segment_frames, || {
                    stop.load(Ordering::Acquire)
                })
            }
        };
        match result {
            Ok(()) => active = Some(key),
            Err((error, frames)) => {
                if stop.load(Ordering::Acquire) {
                    interrupted = Some(GranularWorkerRequest { key, frames });
                    break;
                }
                held = Some(GranularWorkerOutcome {
                    key,
                    result: GranularWorkerResult::Failed {
                        error: GranularWorkerError::Page(error),
                        frames,
                    },
                });
            }
        }
    }
    if let Some(key) = active {
        if let Some(frames) = source.reclaim() {
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
        source,
        queues,
        interrupted,
        held,
    }
}
