//! Bounded control-side sample queries for visible waveform columns.
//!
//! This worker owns no UI or audio callback state. An editor creates it outside
//! paint, submits finite requests without waiting and drains replies on control.
//! Replacing a clip request cancels older work in bounded chunks; the result
//! identity also prevents an already-completed stale reply from being installed.
use super::waveform_source::bounded_clip_overrange;
use hex_project::{Clip, ClipFade};
use hex_session::CachedOverrange as Overrange;
use hex_session::CachedWaveform;
use std::{
    collections::HashMap,
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
};

const QUEUE_CAPACITY: usize = 8;
const MAX_COLUMNS: usize = 4_096;
const MAX_FRAMES: usize = 4_194_304;
const CHUNK_FRAMES: u64 = 4_096;

#[derive(Clone)]
pub(super) struct Request {
    /// Opaque caller identity for the exact projection and source generation.
    pub id: u64,
    /// A caller-owned view slot. Distinct views of the same clip must not
    /// cancel each other; the caller cancels a slot when it is retired.
    pub slot: u64,
    pub clip: Clip,
    pub waveform: Arc<CachedWaveform>,
    pub timeline_origin: u64,
    pub fade: Option<ClipFade>,
    pub columns: Vec<Range<u64>>,
}

pub(super) struct Job {
    request: Request,
    cancelled: Arc<AtomicBool>,
}

/// A column is `None` when its exact result is unavailable, not when silent.
pub(super) struct Reply {
    pub id: u64,
    pub slot: u64,
    /// None means the result could not be allocated.
    pub columns: Option<Vec<Option<Overrange>>>,
    cancelled: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Admission {
    Queued,
    Full,
    Unavailable,
}

pub(super) struct Worker {
    sender: SyncSender<Job>,
    replies: Receiver<Reply>,
    latest: HashMap<u64, Arc<AtomicBool>>,
    #[cfg(all(test, feature = "ui-unit-tier"))]
    exited: Arc<AtomicBool>,
}

impl Worker {
    #[cfg(all(test, feature = "ui-unit-tier"))]
    pub(super) fn saturated_for_test(
        request: &Request,
    ) -> (Self, Receiver<Job>, SyncSender<Reply>) {
        let (sender, jobs) = mpsc::sync_channel(QUEUE_CAPACITY);
        let (reply_tx, replies) = mpsc::sync_channel(QUEUE_CAPACITY);
        for _ in 0..QUEUE_CAPACITY {
            sender
                .try_send(Job {
                    request: request.clone(),
                    cancelled: Arc::new(AtomicBool::new(false)),
                })
                .unwrap_or_else(|_| panic!("queue capacity"));
        }
        (
            Self {
                sender,
                replies,
                latest: HashMap::new(),
                exited: Arc::new(AtomicBool::new(false)),
            },
            jobs,
            reply_tx,
        )
    }
    /// Start on the editor control side, never from a paint or audio callback.
    pub fn new() -> std::io::Result<Self> {
        let (sender, jobs) = mpsc::sync_channel::<Job>(QUEUE_CAPACITY);
        let (reply_tx, replies) = mpsc::sync_channel::<Reply>(QUEUE_CAPACITY);
        #[cfg(all(test, feature = "ui-unit-tier"))]
        let exited = Arc::new(AtomicBool::new(false));
        #[cfg(all(test, feature = "ui-unit-tier"))]
        let thread_exited = Arc::clone(&exited);
        std::thread::Builder::new()
            .name("hex-waveform-overrange".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    if let Some(reply) = calculate(job) {
                        // Only this background thread may wait for a full reply
                        // queue. Dropping the receiver wakes it during shutdown.
                        if reply_tx.send(reply).is_err() {
                            break;
                        }
                    }
                }
                #[cfg(all(test, feature = "ui-unit-tier"))]
                thread_exited.store(true, Ordering::Release);
            })?;
        Ok(Self {
            sender,
            replies,
            latest: HashMap::new(),
            #[cfg(all(test, feature = "ui-unit-tier"))]
            exited,
        })
    }

    /// Invalidates the old request in this slot even if replacement is full.
    /// The caller must deduplicate identical requests to avoid cancelling a
    /// still-current query on every repaint.
    pub fn submit(&mut self, request: Request) -> Admission {
        let slot = request.slot;
        self.cancel(slot);
        if request.columns.len() > MAX_COLUMNS
            || request.columns.iter().any(|range| range.start >= range.end)
            || self.latest.try_reserve(1).is_err()
        {
            return Admission::Unavailable;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let job = Job {
            request,
            cancelled: Arc::clone(&cancelled),
        };
        match self.sender.try_send(job) {
            Ok(()) => {
                self.latest.insert(slot, cancelled);
                Admission::Queued
            }
            Err(TrySendError::Full(_)) => Admission::Full,
            Err(TrySendError::Disconnected(_)) => Admission::Unavailable,
        }
    }

    /// Invalidate a view slot when it disappears or its source is replaced.
    pub fn cancel(&mut self, slot: u64) {
        if let Some(old) = self.latest.remove(&slot) {
            old.store(true, Ordering::Release);
        }
    }

    /// Drain one current result. Cancelled or superseded results are discarded.
    pub fn try_recv(&mut self) -> Result<Reply, TryRecvError> {
        loop {
            let reply = self.replies.try_recv()?;
            let current = self.latest.get(&reply.slot).is_some_and(|token| {
                Arc::ptr_eq(token, &reply.cancelled) && !token.load(Ordering::Acquire)
            });
            if current {
                self.latest.remove(&reply.slot);
                return Ok(reply);
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        for token in self.latest.values() {
            token.store(true, Ordering::Release);
        }
        // Sender/receiver destruction disconnects both queues without a UI join.
    }
}

fn calculate(job: Job) -> Option<Reply> {
    let Job { request, cancelled } = job;
    let mut results = Vec::new();
    if results.try_reserve_exact(request.columns.len()).is_err() {
        return Some(Reply {
            id: request.id,
            slot: request.slot,
            columns: None,
            cancelled,
        });
    }
    let mut budget = MAX_FRAMES;
    for column in request.columns {
        if cancelled.load(Ordering::Acquire) {
            return None;
        }
        let Ok(frames) = usize::try_from(column.end - column.start) else {
            results.push(None);
            continue;
        };
        if frames > budget {
            results.push(None);
            continue;
        }
        budget -= frames;
        let mut result = Some(Overrange::default());
        let mut start = column.start;
        while start < column.end {
            if cancelled.load(Ordering::Acquire) {
                return None;
            }
            let end = start.saturating_add(CHUNK_FRAMES).min(column.end);
            let next = bounded_clip_overrange(
                &request.waveform,
                &request.clip,
                request.timeline_origin,
                start..end,
                request.fade,
                // Each chunk is no larger than CHUNK_FRAMES.
                CHUNK_FRAMES as usize,
            );
            result = result.zip(next).map(|(mut current, next)| {
                for channel in 0..2 {
                    current.positive[channel] |= next.positive[channel];
                    current.negative[channel] |= next.negative[channel];
                }
                current
            });
            if result.is_none() {
                break;
            }
            start = end;
        }
        results.push(result);
    }
    Some(Reply {
        id: request.id,
        slot: request.slot,
        columns: Some(results),
        cancelled,
    })
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project};

    fn request(id: u64) -> Request {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "test".into(),
                path: "test.wav".into(),
                sample_rate: 48_000,
                frames: 8,
            })
            .unwrap()
        else {
            panic!("unexpected import result")
        };
        let audio = hex_engine::Audio::new(48_000, vec![[2.0, -2.0]; 8]).unwrap();
        Request {
            id,
            slot: 1,
            clip: editor.project().clip(clip).unwrap().clone(),
            waveform: Arc::new(hex_media::waveform::summarize(&audio, 1).unwrap()),
            timeline_origin: 0,
            fade: Some(ClipFade::new(8, 0, 8, 0).unwrap()),
            columns: vec![0..1, 7..8],
        }
    }

    fn harness() -> (Worker, Receiver<Job>, SyncSender<Reply>) {
        let (sender, jobs) = mpsc::sync_channel(QUEUE_CAPACITY);
        let (reply_tx, replies) = mpsc::sync_channel(QUEUE_CAPACITY);
        (
            Worker {
                sender,
                replies,
                latest: HashMap::new(),
                exited: Arc::new(AtomicBool::new(false)),
            },
            jobs,
            reply_tx,
        )
    }

    #[test]
    fn completed_old_result_never_replaces_newer_preview() {
        let (mut worker, jobs, replies) = harness();
        let first = request(1);
        let second = request(2);
        assert_eq!(worker.submit(first), Admission::Queued);
        let old = calculate(jobs.try_recv().unwrap()).unwrap();
        assert_eq!(worker.submit(second), Admission::Queued);
        replies
            .try_send(old)
            .unwrap_or_else(|_| panic!("reply capacity"));
        let latest = calculate(jobs.try_recv().unwrap()).unwrap();
        replies
            .try_send(latest)
            .unwrap_or_else(|_| panic!("reply capacity"));
        let reply = worker.try_recv().unwrap();
        assert_eq!(reply.id, 2);
        let columns = reply.columns.unwrap();
        assert_eq!(columns[0], Some(Overrange::default()));
        assert_eq!(
            columns[1],
            Some(Overrange {
                positive: [true, false],
                negative: [false, true]
            })
        );
        assert!(matches!(worker.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn simultaneous_views_of_the_same_clip_do_not_cancel_each_other() {
        let (mut worker, jobs, replies) = harness();
        let first = request(1);
        let mut second = request(2);
        second.slot = 2;
        assert_eq!(worker.submit(first), Admission::Queued);
        assert_eq!(worker.submit(second), Admission::Queued);
        for _ in 0..2 {
            let result = calculate(jobs.try_recv().unwrap()).unwrap();
            replies
                .try_send(result)
                .unwrap_or_else(|_| panic!("reply capacity"));
        }
        assert_eq!(worker.try_recv().unwrap().id, 1);
        assert_eq!(worker.try_recv().unwrap().id, 2);
    }

    #[test]
    fn full_queue_invalidates_superseded_work_and_never_blocks() {
        let (mut worker, jobs, _) = harness();
        for id in 0..QUEUE_CAPACITY as u64 {
            assert_eq!(worker.submit(request(id)), Admission::Queued);
        }
        assert_eq!(worker.submit(request(99)), Admission::Full);
        while let Ok(job) = jobs.try_recv() {
            assert!(calculate(job).is_none());
        }
        assert!(worker.latest.is_empty());
        assert_eq!(worker.submit(request(100)), Admission::Queued);
    }

    #[test]
    fn cancelled_and_over_budget_work_are_explicit() {
        let cancelled = Arc::new(AtomicBool::new(true));
        assert!(
            calculate(Job {
                request: request(1),
                cancelled
            })
            .is_none()
        );
        let mut oversized = request(2);
        oversized.columns = std::iter::once(0..MAX_FRAMES as u64 + 1).collect();
        let result = calculate(Job {
            request: oversized,
            cancelled: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
        assert_eq!(result.columns.unwrap(), vec![None]);

        let mut peak_only = request(3);
        peak_only.waveform =
            Arc::new(CachedWaveform::from_peaks(8, peak_only.waveform.bins().to_vec()).unwrap());
        let result = calculate(Job {
            request: peak_only,
            cancelled: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
        assert_eq!(result.columns.unwrap(), vec![None, None]);
    }

    #[test]
    fn background_worker_delivers_a_finite_request() {
        let mut worker = Worker::new().unwrap();
        assert_eq!(worker.submit(request(7)), Admission::Queued);
        let reply = worker
            .replies
            .recv_timeout(std::time::Duration::from_millis(500))
            .expect("background reply");
        assert_eq!(reply.id, 7);
        assert!(reply.columns.is_some());
    }

    #[test]
    fn closing_the_owner_cancels_outstanding_work() {
        let (mut worker, jobs, _replies) = harness();
        let request = request(7);
        let slot = request.slot;
        assert_eq!(worker.submit(request), Admission::Queued);
        let token = Arc::clone(worker.latest.get(&slot).unwrap());
        drop(worker);
        assert!(token.load(Ordering::Acquire));
        assert!(calculate(jobs.try_recv().unwrap()).is_none());
    }

    #[test]
    fn background_thread_exits_after_owner_is_dropped() {
        let mut worker = Worker::new().unwrap();
        let exited = Arc::clone(&worker.exited);
        assert_eq!(worker.submit(request(7)), Admission::Queued);
        drop(worker);
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        while !exited.load(Ordering::Acquire) {
            assert!(std::time::Instant::now() < deadline, "worker did not stop");
            std::thread::yield_now();
        }
    }
}
