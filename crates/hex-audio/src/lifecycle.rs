//! Bounded output lifecycle worker for stream retirement and recovery opens.
//! Stream pause/disposal and replacement device creation never run on realtime.
use crate::device::PreparedOutput;
use crate::spectrum_worker::SpectrumReaper;
use cpal::traits::StreamTrait;
use std::{
    fmt,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{
            AtomicBool, AtomicU64,
            Ordering::{AcqRel, Acquire, Release},
        },
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread,
    time::Duration,
};

static STREAM_DEVICE_EPOCH: AtomicU64 = AtomicU64::new(0);
static ACTIVE_STREAM_ROOT: Mutex<Option<crate::prepared::NativeStreamRoot>> = Mutex::new(None);

pub(crate) fn next_stream_device_epoch() -> Result<u64, String> {
    STREAM_DEVICE_EPOCH
        .fetch_update(AcqRel, Acquire, |epoch| epoch.checked_add(1))
        .map(|old| old + 1)
        .map_err(|_| "stream device epoch exhausted".into())
}
pub(crate) fn register_stream_root(root: crate::prepared::NativeStreamRoot) {
    let mut active = ACTIVE_STREAM_ROOT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(
        active.is_none(),
        "exclusive device lease retains one media root"
    );
    *active = Some(root);
}
fn tick_stream_root() {
    let root = ACTIVE_STREAM_ROOT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(root) = root {
        let _phase = root.tick(false);
    }
}

static OCCUPIED: AtomicBool = AtomicBool::new(false);
static OPEN_STATE: OnceLock<Arc<OpenState>> = OnceLock::new();
static SERVICE: OnceLock<Result<SyncSender<LifecycleMessage<PreparedOutput, Retirement>>, String>> =
    OnceLock::new();
// Defensive fallback if the service unexpectedly exits. Static storage retains
// at most the one leased stream until process exit; reopening stays disabled.
static QUARANTINE: Mutex<Option<Retirement>> = Mutex::new(None);

pub(crate) struct Lease;
impl Lease {
    pub fn acquire() -> Result<Self, String> {
        service()?;
        Self::try_acquire().ok_or_else(|| "audio output is open or still shutting down".into())
    }

    fn try_acquire() -> Option<Self> {
        OCCUPIED
            .compare_exchange(false, true, AcqRel, Acquire)
            .ok()
            .map(|_| Self)
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        OCCUPIED.store(false, Release);
    }
}

struct Retirement {
    stream: Option<cpal::Stream>,
    media_root: Option<crate::prepared::NativeStreamRoot>,
    spectrum_reaper: Option<SpectrumReaper>,
    _lease: Lease,
}

pub(crate) struct StreamOwner {
    retirement: Option<Retirement>,
}
impl StreamOwner {
    pub fn new(stream: cpal::Stream, lease: Lease) -> Self {
        Self::with_spectrum(stream, lease, None)
    }

    pub fn with_spectrum(
        stream: cpal::Stream,
        lease: Lease,
        spectrum_reaper: Option<SpectrumReaper>,
    ) -> Self {
        Self::with_media(stream, lease, spectrum_reaper, None)
    }
    pub(crate) fn with_media(
        stream: cpal::Stream,
        lease: Lease,
        spectrum_reaper: Option<SpectrumReaper>,
        media_root: Option<crate::prepared::NativeStreamRoot>,
    ) -> Self {
        Self {
            retirement: Some(Retirement {
                stream: Some(stream),
                media_root,
                spectrum_reaper,
                _lease: lease,
            }),
        }
    }
}
impl Drop for StreamOwner {
    fn drop(&mut self) {
        let Some(retirement) = self.retirement.take() else {
            return;
        };
        // The lifecycle worker is initialized before acquiring the lease or
        // creating a stream. Capacity two holds at most one open plus one retire.
        let retirement = match service() {
            Ok(sender) => match sender.try_send(LifecycleMessage::Retire(retirement)) {
                Ok(()) => return,
                Err(TrySendError::Full(LifecycleMessage::Retire(value)))
                | Err(TrySendError::Disconnected(LifecycleMessage::Retire(value))) => value,
                Err(TrySendError::Full(LifecycleMessage::Open(_)))
                | Err(TrySendError::Disconnected(LifecycleMessage::Open(_))) => return,
            },
            Err(_) => retirement,
        };
        if let Some(reaper) = retirement.spectrum_reaper.as_ref() {
            reaper.request_stop();
        }
        let mut quarantine = match QUARANTINE.lock() {
            Ok(guard) => guard,
            Err(poison) => poison.into_inner(),
        };
        // The exclusive lease ensures no prior retirement can occupy this slot.
        *quarantine = Some(retirement);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryOpenError {
    InvalidRate,
    InvalidStreamingReservation,
    Busy,
    QueueFull,
    WorkerClosed,
}
impl fmt::Display for RecoveryOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidRate => "recovery sample rate must be 8000..192000 hz",
            Self::InvalidStreamingReservation => {
                "streaming reservation must fit the existing page cache"
            }
            Self::Busy => "audio recovery open is already pending",
            Self::QueueFull => "audio lifecycle queue is full",
            Self::WorkerClosed => "audio lifecycle worker is unavailable",
        })
    }
}
impl std::error::Error for RecoveryOpenError {}

/// One nonblocking result slot for a requested output open.
pub struct RecoveryOpen {
    waiter: OpenWaiter<PreparedOutput>,
}
impl RecoveryOpen {
    /// Poll once; `None` means the lifecycle worker is still preparing the output.
    pub fn try_take(
        &mut self,
    ) -> Result<Option<Result<PreparedOutput, String>>, RecoveryOpenError> {
        self.waiter.try_take()
    }
}

/// Queue a default-device reopen for the expected project rate without waiting.
/// Drop the handle to abandon interest in its result; any late stream is retired
/// by the lifecycle worker.
pub fn request_recovery_open(expected_sample_rate: u32) -> Result<RecoveryOpen, RecoveryOpenError> {
    if !(8_000..=192_000).contains(&expected_sample_rate) {
        return Err(RecoveryOpenError::InvalidRate);
    }
    request_open(Some(expected_sample_rate))
}

/// Queue the first default-device open without assuming a project sample rate.
/// The returned output reports the device's actual negotiated rate.
pub fn request_initial_open() -> Result<RecoveryOpen, RecoveryOpenError> {
    request_open(None)
}

/// Opt-in async native open; resident callers retain their current constructors.
pub fn request_streaming_open(
    expected_sample_rate: Option<u32>,
    extent_reservation: usize,
) -> Result<RecoveryOpen, RecoveryOpenError> {
    if expected_sample_rate.is_some_and(|rate| !(8_000..=192_000).contains(&rate)) {
        return Err(RecoveryOpenError::InvalidRate);
    }
    if extent_reservation == 0
        || extent_reservation >= hex_stream::StreamConfig::initial().page_buffers
    {
        return Err(RecoveryOpenError::InvalidStreamingReservation);
    }
    let sender = service().map_err(|_| RecoveryOpenError::WorkerClosed)?;
    let waiter = enqueue_open_options(
        sender,
        state(),
        expected_sample_rate,
        Some(extent_reservation),
    )?;
    Ok(RecoveryOpen { waiter })
}

fn request_open(expected_sample_rate: Option<u32>) -> Result<RecoveryOpen, RecoveryOpenError> {
    let sender = service().map_err(|_| RecoveryOpenError::WorkerClosed)?;
    let waiter = enqueue_open(sender, state(), expected_sample_rate)?;
    Ok(RecoveryOpen { waiter })
}

struct OpenState {
    next_id: AtomicU64,
    reserved: AtomicU64,
    in_service: AtomicBool,
}
impl OpenState {
    fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            reserved: AtomicU64::new(0),
            in_service: AtomicBool::new(false),
        }
    }

    fn release(&self, id: u64) {
        let _ = self.reserved.compare_exchange(id, 0, AcqRel, Acquire);
    }
}

fn state() -> &'static Arc<OpenState> {
    OPEN_STATE.get_or_init(|| Arc::new(OpenState::new()))
}

struct OpenWaiter<T> {
    receiver: Receiver<Result<T, String>>,
    state: Arc<OpenState>,
    id: u64,
    canceled: Arc<AtomicBool>,
    consumed: Arc<AtomicBool>,
    abandoned: Arc<AtomicBool>,
    released: bool,
}
impl<T> OpenWaiter<T> {
    fn try_take(&mut self) -> Result<Option<Result<T, String>>, RecoveryOpenError> {
        match self.receiver.try_recv() {
            Ok(result) => {
                self.consumed.store(true, Release);
                self.released = true;
                Ok(Some(result))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.abandoned.store(true, Release);
                self.canceled.store(true, Release);
                // The lifecycle thread is gone, so it cannot finish this reservation.
                // No queued operation can still produce a late stream in this case.
                self.state.release(self.id);
                self.state.in_service.store(false, Release);
                self.released = true;
                Err(RecoveryOpenError::WorkerClosed)
            }
        }
    }

    fn abandon(&mut self) {
        if !self.released {
            self.canceled.store(true, Release);
            self.abandoned.store(true, Release);
            self.released = true;
        }
    }
}
impl<T> Drop for OpenWaiter<T> {
    fn drop(&mut self) {
        self.abandon();
    }
}

fn enqueue_open<T: Send + 'static, R: Send + 'static>(
    sender: &SyncSender<LifecycleMessage<T, R>>,
    state: &Arc<OpenState>,
    expected_sample_rate: Option<u32>,
) -> Result<OpenWaiter<T>, RecoveryOpenError> {
    if state.in_service.load(Acquire) {
        return Err(RecoveryOpenError::Busy);
    }
    enqueue_open_options(sender, state, expected_sample_rate, None)
}
fn enqueue_open_options<T: Send + 'static, R: Send + 'static>(
    sender: &SyncSender<LifecycleMessage<T, R>>,
    state: &Arc<OpenState>,
    expected_sample_rate: Option<u32>,
    streaming_extent: Option<usize>,
) -> Result<OpenWaiter<T>, RecoveryOpenError> {
    let id = state.next_id.fetch_add(1, AcqRel).max(1);
    state
        .reserved
        .compare_exchange(0, id, AcqRel, Acquire)
        .map_err(|_| RecoveryOpenError::Busy)?;
    if state
        .in_service
        .compare_exchange(false, true, AcqRel, Acquire)
        .is_err()
    {
        state.release(id);
        return Err(RecoveryOpenError::Busy);
    }

    let (completion, receiver) = mpsc::sync_channel(1);
    let canceled = Arc::new(AtomicBool::new(false));
    let consumed = Arc::new(AtomicBool::new(false));
    let abandoned = Arc::new(AtomicBool::new(false));
    let request = OpenRequest {
        id,
        expected_sample_rate,
        streaming_extent,
        completion,
        canceled: Arc::clone(&canceled),
        consumed: Arc::clone(&consumed),
        abandoned: Arc::clone(&abandoned),
        state: Arc::clone(state),
    };
    match sender.try_send(LifecycleMessage::Open(request)) {
        Ok(()) => Ok(OpenWaiter {
            receiver,
            state: Arc::clone(state),
            id,
            canceled,
            consumed,
            abandoned,
            released: false,
        }),
        Err(TrySendError::Full(LifecycleMessage::Open(request))) => {
            finish_request(&request);
            Err(RecoveryOpenError::QueueFull)
        }
        Err(TrySendError::Disconnected(LifecycleMessage::Open(request))) => {
            finish_request(&request);
            Err(RecoveryOpenError::WorkerClosed)
        }
        Err(TrySendError::Full(LifecycleMessage::Retire(_)))
        | Err(TrySendError::Disconnected(LifecycleMessage::Retire(_))) => {
            state.in_service.store(false, Release);
            state.release(id);
            Err(RecoveryOpenError::QueueFull)
        }
    }
}

struct OpenRequest<T> {
    id: u64,
    expected_sample_rate: Option<u32>,
    streaming_extent: Option<usize>,
    completion: SyncSender<Result<T, String>>,
    canceled: Arc<AtomicBool>,
    consumed: Arc<AtomicBool>,
    abandoned: Arc<AtomicBool>,
    state: Arc<OpenState>,
}
fn finish_request<T>(request: &OpenRequest<T>) {
    request.state.in_service.store(false, Release);
    request.state.release(request.id);
}

enum LifecycleMessage<T, R> {
    Retire(R),
    Open(OpenRequest<T>),
}

#[cfg(test)]
fn run_lifecycle<T: Send + 'static, R: Send + 'static>(
    receiver: Receiver<LifecycleMessage<T, R>>,
    is_occupied: impl Fn() -> bool,
    mut try_open: impl FnMut(Option<u32>) -> Option<Result<T, String>>,
    retire: impl FnMut(R),
) {
    run_lifecycle_with_tick(
        receiver,
        is_occupied,
        |rate, _extent| try_open(rate),
        retire,
        || {},
    )
}
fn run_lifecycle_with_tick<T: Send + 'static, R: Send + 'static>(
    receiver: Receiver<LifecycleMessage<T, R>>,
    is_occupied: impl Fn() -> bool,
    mut try_open: impl FnMut(Option<u32>, Option<usize>) -> Option<Result<T, String>>,
    mut retire: impl FnMut(R),
    mut tick: impl FnMut(),
) {
    enum Phase {
        Queued,
        Delivered,
    }
    struct Active<T> {
        request: OpenRequest<T>,
        phase: Phase,
    }
    let mut active: Option<Active<T>> = None;
    loop {
        tick();
        if let Some(current) = active.as_mut() {
            match current.phase {
                Phase::Queued if current.request.canceled.load(Acquire) => {
                    if !is_occupied() {
                        finish_request(&current.request);
                        active = None;
                    }
                }
                Phase::Queued => {
                    if let Some(result) = try_open(
                        current.request.expected_sample_rate,
                        current.request.streaming_extent,
                    ) {
                        if current.request.canceled.load(Acquire) {
                            drop(result);
                            current.phase = Phase::Delivered;
                        } else {
                            match current.request.completion.try_send(result) {
                                Ok(()) => current.phase = Phase::Delivered,
                                Err(
                                    TrySendError::Full(result) | TrySendError::Disconnected(result),
                                ) => {
                                    drop(result);
                                    current.request.abandoned.store(true, Release);
                                    current.phase = Phase::Delivered;
                                }
                            }
                        }
                    }
                }
                Phase::Delivered if current.request.consumed.load(Acquire) => {
                    finish_request(&current.request);
                    active = None;
                }
                Phase::Delivered if current.request.abandoned.load(Acquire) && !is_occupied() => {
                    finish_request(&current.request);
                    active = None;
                }
                Phase::Delivered => {}
            }
        }

        match receiver.recv_timeout(Duration::from_millis(5)) {
            Ok(LifecycleMessage::Retire(value)) => retire(value),
            Ok(LifecycleMessage::Open(request)) => {
                if active.is_none() {
                    active = Some(Active {
                        request,
                        phase: Phase::Queued,
                    });
                } else {
                    request.abandoned.store(true, Release);
                    finish_request(&request);
                    let _ = request
                        .completion
                        .try_send(Err("another recovery open is pending".into()));
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn service() -> Result<&'static SyncSender<LifecycleMessage<PreparedOutput, Retirement>>, String> {
    SERVICE
        .get_or_init(|| {
            let (sender, receiver) =
                mpsc::sync_channel::<LifecycleMessage<PreparedOutput, Retirement>>(2);
            thread::Builder::new()
                .name("hex-audio-lifecycle".into())
                .spawn(move || {
                    run_lifecycle_with_tick(
                        receiver,
                        || OCCUPIED.load(Acquire),
                        |expected_rate, extent| {
                            let lease = Lease::try_acquire()?;
                            Some(PreparedOutput::open_with_options(
                                expected_rate,
                                extent,
                                lease,
                            ))
                        },
                        retire_prepared_output,
                        tick_stream_root,
                    );
                })
                .map_err(|error| format!("could not start audio lifecycle worker: {error}"))?;
            Ok(sender)
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn retire_prepared_output(retirement: Retirement) {
    let mut retirement = retirement;
    while retirement
        .stream
        .as_ref()
        .is_some_and(|stream| stream.pause().is_err())
    {
        thread::sleep(Duration::from_millis(250));
    }
    // Only successful pause allows closure disposal and its terminal transfer.
    drop(retirement.stream.take());
    if let Some(root) = retirement.media_root.as_ref() {
        loop {
            match root.tick(true) {
                hex_stream::MediaCleanupPhase::Complete => break,
                hex_stream::MediaCleanupPhase::FailedIncomplete => {
                    // Retain actual roots and the exclusive device lease; no
                    // unknown buffer count or failed join is called complete.
                    let mut quarantine = QUARANTINE
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    *quarantine = Some(retirement);
                    return;
                }
                hex_stream::MediaCleanupPhase::Pending => thread::sleep(Duration::from_millis(250)),
            }
        }
        *ACTIVE_STREAM_ROOT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
    let Retirement {
        stream: _,
        media_root: _,
        spectrum_reaper,
        _lease,
    } = retirement;
    if retire_after_callback_stops(
        (),
        spectrum_reaper,
        _lease,
        drop,
        |reaper| reaper.shutdown_and_join(),
        drop,
    )
    .is_err()
    {
        eprintln!("hex audio spectrum worker failed during output retirement");
    }
}

fn retire_after_callback_stops<S, R, L, E>(
    stream: S,
    reaper: Option<R>,
    lease: L,
    drop_stream: impl FnOnce(S),
    join_reaper: impl FnOnce(R) -> Result<(), E>,
    drop_lease: impl FnOnce(L),
) -> Result<(), E> {
    drop_stream(stream);
    let joined = reaper.map_or(Ok(()), join_reaper);
    // Keep the exclusive device lease until callback and worker ownership are
    // fully retired, even if the worker reports a panic.
    drop_lease(lease);
    joined
}

/// Successful pause precedes disposal. Never retry by dropping the stream: the
/// pinned backend ignores stop errors in Drop. Retry executes only off realtime.
#[cfg(test)]
fn retire_when_stopped<T>(value: T, mut pause: impl FnMut(&T) -> bool, mut retry: impl FnMut()) {
    while !pause(&value) {
        retry();
    }
    drop(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering::{Acquire, Release};
    use std::time::Instant;

    #[test]
    fn failed_stop_retains_owner_and_disposal_runs_on_worker() {
        struct Resource(Arc<AtomicBool>, Arc<Mutex<Option<thread::ThreadId>>>);
        impl Drop for Resource {
            fn drop(&mut self) {
                self.0.store(true, Release);
                *self.1.lock().unwrap() = Some(thread::current().id());
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let owner = Arc::new(Mutex::new(None));
        let resource = Resource(dropped.clone(), owner.clone());
        let result = thread::spawn(move || {
            let mut attempts = 0;
            retire_when_stopped(
                resource,
                |value| {
                    assert!(!value.0.load(Acquire));
                    attempts += 1;
                    attempts == 4
                },
                || {},
            );
            assert_eq!(attempts, 4);
            thread::current().id()
        })
        .join()
        .unwrap();
        assert!(dropped.load(Acquire));
        assert_eq!(*owner.lock().unwrap(), Some(result));
        assert_ne!(result, thread::current().id());
    }

    #[test]
    fn spectrum_retirement_drops_callback_before_join_and_lease() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let stream_order = Arc::clone(&order);
        let worker_order = Arc::clone(&order);
        let lease_order = Arc::clone(&order);
        let stream = 1_u8;
        let reaper = 2_u8;
        let lease = 3_u8;
        let result = retire_after_callback_stops(
            stream,
            Some(reaper),
            lease,
            move |_| stream_order.lock().unwrap().push("stream"),
            move |_| {
                assert_eq!(*worker_order.lock().unwrap(), ["stream"]);
                worker_order.lock().unwrap().push("worker");
                Ok::<(), ()>(())
            },
            move |_| lease_order.lock().unwrap().push("lease"),
        );
        assert!(result.is_ok());
        assert_eq!(*order.lock().unwrap(), ["stream", "worker", "lease"]);

        let error_order = Arc::new(Mutex::new(Vec::new()));
        let stream_order = Arc::clone(&error_order);
        let worker_order = Arc::clone(&error_order);
        let lease_order = Arc::clone(&error_order);
        let result = retire_after_callback_stops(
            1_u8,
            Some(2_u8),
            3_u8,
            move |_| stream_order.lock().unwrap().push("stream"),
            move |_| {
                worker_order.lock().unwrap().push("worker-error");
                Err("worker panic")
            },
            move |_| lease_order.lock().unwrap().push("lease"),
        );
        assert_eq!(result, Err("worker panic"));
        assert_eq!(
            *error_order.lock().unwrap(),
            ["stream", "worker-error", "lease"]
        );
    }

    #[test]
    fn exclusive_lease_prevents_accumulating_live_or_retained_streams() {
        let first = Lease::acquire().unwrap();
        assert!(Lease::acquire().is_err());
        drop(first);
        let second = Lease::acquire().unwrap();
        drop(second);
    }

    #[test]
    fn abandoned_late_output_retires_on_worker_before_reservation_releases() {
        struct LateOutput {
            occupied: Arc<AtomicBool>,
            dropped_on: Arc<Mutex<Option<thread::ThreadId>>>,
        }
        impl Drop for LateOutput {
            fn drop(&mut self) {
                *self.dropped_on.lock().unwrap() = Some(thread::current().id());
                self.occupied.store(false, Release);
            }
        }

        let (sender, receiver) = mpsc::sync_channel::<LifecycleMessage<LateOutput, ()>>(2);
        let state = Arc::new(OpenState::new());
        let occupied = Arc::new(AtomicBool::new(false));
        let worker_occupied = Arc::clone(&occupied);
        let open_occupied = Arc::clone(&worker_occupied);
        let dropped_on = Arc::new(Mutex::new(None));
        let worker_dropped_on = Arc::clone(&dropped_on);
        let (open_started, open_started_rx) = mpsc::channel();
        let (finish_open, finish_open_rx) = mpsc::channel();
        let (worker_id_tx, worker_id_rx) = mpsc::channel();
        thread::spawn(move || {
            let worker_id = thread::current().id();
            let _ = worker_id_tx.send(worker_id);
            run_lifecycle(
                receiver,
                || worker_occupied.load(Acquire),
                move |_| {
                    let _ = open_started.send(());
                    let _ = finish_open_rx.recv();
                    open_occupied.store(true, Release);
                    Some(Ok(LateOutput {
                        occupied: Arc::clone(&open_occupied),
                        dropped_on: Arc::clone(&worker_dropped_on),
                    }))
                },
                |_| {},
            );
        });
        let worker_id = worker_id_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let waiter = enqueue_open::<LateOutput, ()>(&sender, &state, Some(48_000)).unwrap();
        assert!(open_started_rx.recv_timeout(Duration::from_secs(1)).is_ok());
        drop(waiter);
        assert!(matches!(
            enqueue_open::<LateOutput, ()>(&sender, &state, Some(48_000)),
            Err(RecoveryOpenError::Busy)
        ));
        assert!(finish_open.send(()).is_ok());

        let deadline = Instant::now() + Duration::from_secs(1);
        while state.reserved.load(Acquire) != 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(state.reserved.load(Acquire), 0);
        assert_eq!(*dropped_on.lock().unwrap(), Some(worker_id));
        assert!(!occupied.load(Acquire));
        let _retry = enqueue_open::<LateOutput, ()>(&sender, &state, Some(48_000)).unwrap();
    }

    #[test]
    fn full_open_queue_rolls_back_reservation() {
        let (sender, _receiver) = mpsc::sync_channel::<LifecycleMessage<(), ()>>(0);
        let state = Arc::new(OpenState::new());
        assert!(matches!(
            enqueue_open(&sender, &state, Some(48_000)),
            Err(RecoveryOpenError::QueueFull)
        ));
        assert_eq!(state.reserved.load(Acquire), 0);
        assert!(!state.in_service.load(Acquire));
    }

    #[test]
    fn initial_open_passes_no_assumed_project_rate() {
        let (sender, receiver) = mpsc::sync_channel::<LifecycleMessage<u32, ()>>(2);
        let state = Arc::new(OpenState::new());
        let (rate_sender, rate_receiver) = mpsc::channel();
        thread::spawn(move || {
            run_lifecycle(
                receiver,
                || false,
                move |expected_rate| {
                    let _ = rate_sender.send(expected_rate);
                    Some(Ok(44_100))
                },
                |_| {},
            );
        });

        let mut waiter = enqueue_open(&sender, &state, None).unwrap();
        assert_eq!(
            rate_receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            None
        );
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut output_rate = None;
        while Instant::now() < deadline {
            if let Some(Ok(rate)) = waiter.try_take().unwrap() {
                output_rate = Some(rate);
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(output_rate, Some(44_100));
    }

    #[test]
    fn recovery_open_waits_until_failed_retirement_releases_exclusive_lease() {
        let (sender, receiver) = mpsc::sync_channel::<LifecycleMessage<(), ()>>(2);
        let state = Arc::new(OpenState::new());
        let occupied = Arc::new(AtomicBool::new(true));
        let worker_occupied = Arc::clone(&occupied);
        let open_occupied = Arc::clone(&worker_occupied);
        let attempts = Arc::new(AtomicU64::new(0));
        let worker_attempts = Arc::clone(&attempts);
        let (opened, opened_rx) = mpsc::channel();
        thread::spawn(move || {
            run_lifecycle(
                receiver,
                || worker_occupied.load(Acquire),
                move |_| {
                    if open_occupied.load(Acquire) {
                        return None;
                    }
                    let _ = opened.send(());
                    Some(Ok(()))
                },
                |()| {
                    let mut local_attempts = 0;
                    retire_when_stopped(
                        (),
                        |_| {
                            local_attempts += 1;
                            worker_attempts.store(local_attempts, Release);
                            local_attempts == 3
                        },
                        || {},
                    );
                    worker_occupied.store(false, Release);
                },
            );
        });

        assert!(sender.try_send(LifecycleMessage::Retire(())).is_ok());
        let mut waiter = enqueue_open(&sender, &state, Some(48_000)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut result = None;
        while Instant::now() < deadline {
            if let Some(value) = waiter.try_take().unwrap() {
                result = Some(value);
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(matches!(result, Some(Ok(()))));
        assert_eq!(attempts.load(Acquire), 3);
        assert!(opened_rx.try_recv().is_ok());
    }
}
