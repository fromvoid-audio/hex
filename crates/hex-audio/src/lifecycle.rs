//! Bounded output lifecycle worker for stream retirement and recovery opens.
//! Stream pause/disposal and replacement device creation never run on realtime.
use crate::device::{Opened, PreparedOutput, WorkerPreparedOutput};
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

pub(crate) mod retained_families;

static OCCUPIED: AtomicBool = AtomicBool::new(false);
static OPEN_STATE: OnceLock<Arc<OpenState>> = OnceLock::new();
static SERVICE: OnceLock<Result<SyncSender<LifecycleMessage<Opened, Retirement>>, String>> =
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

/// Only lifecycle's completed stream-handle frontier constructs this witness.
/// Missing/exposed handles and pending/panicking Drop cannot produce one.
pub(crate) struct CompletedStreamHandle {
    root: u64,
}
impl CompletedStreamHandle {
    pub(crate) fn matches(&self, root: u64) -> bool {
        self.root == root
    }
}
struct Retirement {
    stream: Option<cpal::Stream>,
    #[cfg(feature = "allocation-probe")]
    headless_backend: Option<()>,
    #[cfg(feature = "allocation-probe")]
    headless: bool,
    spectrum_reaper: Option<SpectrumReaper>,
    _lease: Lease,
    native: Option<crate::native_callback_owner::NativeRetirement>,
    backend_exposed: bool,
    stream_drop_pending: bool,
    stream_disposed: bool,
    failed: bool,
    first_panic: Option<Box<dyn std::any::Any + Send>>,
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
        Self {
            retirement: Some(Retirement {
                stream: Some(stream),
                #[cfg(feature = "allocation-probe")]
                headless_backend: None,
                #[cfg(feature = "allocation-probe")]
                headless: false,
                spectrum_reaper,
                _lease: lease,
                native: None,
                backend_exposed: true,
                stream_drop_pending: false,
                stream_disposed: false,
                failed: false,
                first_panic: None,
            }),
        }
    }
    pub(crate) fn prepared_custody(
        lease: Lease,
        spectrum_reaper: Option<SpectrumReaper>,
        native: crate::native_callback_owner::NativeRetirement,
    ) -> Self {
        native.bind_spectrum_reaper(spectrum_reaper.as_ref());
        Self::unexposed_custody(lease, spectrum_reaper, Some(native))
    }
    pub(crate) fn failed_preparation_custody(
        lease: Lease,
        spectrum_reaper: Option<SpectrumReaper>,
    ) -> Self {
        Self::unexposed_custody(lease, spectrum_reaper, None)
    }
    fn unexposed_custody(
        lease: Lease,
        spectrum_reaper: Option<SpectrumReaper>,
        native: Option<crate::native_callback_owner::NativeRetirement>,
    ) -> Self {
        Self {
            retirement: Some(Retirement {
                stream: None,
                #[cfg(feature = "allocation-probe")]
                headless_backend: None,
                #[cfg(feature = "allocation-probe")]
                headless: false,
                spectrum_reaper,
                _lease: lease,
                native,
                backend_exposed: false,
                stream_drop_pending: false,
                stream_disposed: false,
                failed: false,
                first_panic: None,
            }),
        }
    }
    #[cfg(feature = "allocation-probe")]
    pub(crate) fn expose_headless_backend(&mut self) {
        if let Some(retirement) = self.retirement.as_mut() {
            retirement.headless = true;
            retirement.headless_backend = Some(());
            retirement.backend_exposed = true;
        }
    }
    pub(crate) fn expose_backend(&mut self) {
        if let Some(retirement) = self.retirement.as_mut() {
            retirement.backend_exposed = true;
        }
    }
    pub(crate) fn set_stream(&mut self, stream: cpal::Stream) {
        if let Some(retirement) = self.retirement.as_mut() {
            retirement.stream = Some(stream);
        }
    }
    pub(crate) fn play(&self) -> Result<(), String> {
        self.retirement
            .as_ref()
            .and_then(|r| r.stream.as_ref())
            .ok_or_else(|| "audio stream is unavailable".to_owned())?
            .play()
            .map_err(|error| error.to_string())
    }
}
impl Drop for StreamOwner {
    fn drop(&mut self) {
        let Some(retirement) = self.retirement.take() else {
            return;
        };
        if let Some(reaper) = retirement.spectrum_reaper.as_ref() {
            reaper.request_stop();
        }
        let cleanup = retirement.native.as_ref().map(|native| native.root());
        if let Some(root) = cleanup.as_ref() {
            root.announce_retirement();
        }
        // Capacity three: one Open, one coalesced Cleanup, one leased Retire.
        // Cleanup send and retirement announcement serialize on the root bank.
        let retirement = match service() {
            Ok(sender) => match sender.try_send(LifecycleMessage::Retire(retirement, cleanup)) {
                Ok(()) => return,
                Err(TrySendError::Full(LifecycleMessage::Retire(value, _)))
                | Err(TrySendError::Disconnected(LifecycleMessage::Retire(value, _))) => value,
                Err(TrySendError::Full(LifecycleMessage::Open(_)))
                | Err(TrySendError::Disconnected(LifecycleMessage::Open(_)))
                | Err(TrySendError::Full(LifecycleMessage::Cleanup(_)))
                | Err(TrySendError::Disconnected(LifecycleMessage::Cleanup(_))) => return,
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
    Busy,
    QueueFull,
    WorkerClosed,
}
impl fmt::Display for RecoveryOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidRate => "recovery sample rate must be 8000..192000 hz",
            Self::Busy => "audio recovery open is already pending",
            Self::QueueFull => "audio lifecycle queue is full",
            Self::WorkerClosed => "audio lifecycle worker is unavailable",
        })
    }
}
impl std::error::Error for RecoveryOpenError {}

/// One nonblocking result slot for a requested output open.
pub struct RecoveryOpen {
    waiter: OpenWaiter<Opened>,
}
impl RecoveryOpen {
    /// Poll once; `None` means the lifecycle worker is still preparing the output.
    pub fn try_take(
        &mut self,
    ) -> Result<Option<Result<PreparedOutput, String>>, RecoveryOpenError> {
        Ok(self
            .waiter
            .try_take()?
            .map(|result| result.and_then(Opened::into_legacy)))
    }
}

/// Worker-only completion on the same exclusive lifecycle open lane.
/// It must be polled and retained on the Session worker, never delivered to UI.
pub struct WorkerRecoveryOpen {
    waiter: OpenWaiter<Opened>,
}
impl WorkerRecoveryOpen {
    pub fn try_take(
        &mut self,
    ) -> Result<Option<Result<WorkerPreparedOutput, String>>, RecoveryOpenError> {
        Ok(self
            .waiter
            .try_take()?
            .map(|result| result.and_then(Opened::into_worker)))
    }
}

/// Queue a virgin worker-owned output. None negotiates the initial device rate;
/// Some validates the current document rate before exposing callbacks.
pub fn request_worker_open(
    expected_sample_rate: Option<u32>,
) -> Result<WorkerRecoveryOpen, RecoveryOpenError> {
    if expected_sample_rate.is_some_and(|rate| !(8_000..=192_000).contains(&rate)) {
        return Err(RecoveryOpenError::InvalidRate);
    }
    let sender = service().map_err(|_| RecoveryOpenError::WorkerClosed)?;
    let waiter = enqueue_open_kind(sender, state(), expected_sample_rate, true)?;
    Ok(WorkerRecoveryOpen { waiter })
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

fn request_open(expected_sample_rate: Option<u32>) -> Result<RecoveryOpen, RecoveryOpenError> {
    let sender = service().map_err(|_| RecoveryOpenError::WorkerClosed)?;
    let waiter = enqueue_open_kind(sender, state(), expected_sample_rate, false)?;
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

#[cfg(test)]
fn enqueue_open<T: Send + 'static, R: Send + 'static>(
    sender: &SyncSender<LifecycleMessage<T, R>>,
    state: &Arc<OpenState>,
    expected_sample_rate: Option<u32>,
) -> Result<OpenWaiter<T>, RecoveryOpenError> {
    enqueue_open_kind(sender, state, expected_sample_rate, false)
}

fn enqueue_open_kind<T: Send + 'static, R: Send + 'static>(
    sender: &SyncSender<LifecycleMessage<T, R>>,
    state: &Arc<OpenState>,
    expected_sample_rate: Option<u32>,
    worker_endpoints: bool,
) -> Result<OpenWaiter<T>, RecoveryOpenError> {
    if state.in_service.load(Acquire) {
        return Err(RecoveryOpenError::Busy);
    }
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
        worker_endpoints,
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
        Err(TrySendError::Full(LifecycleMessage::Retire(_, _)))
        | Err(TrySendError::Disconnected(LifecycleMessage::Retire(_, _)))
        | Err(TrySendError::Full(LifecycleMessage::Cleanup(_)))
        | Err(TrySendError::Disconnected(LifecycleMessage::Cleanup(_))) => {
            state.in_service.store(false, Release);
            state.release(id);
            Err(RecoveryOpenError::QueueFull)
        }
    }
}

struct OpenRequest<T> {
    id: u64,
    expected_sample_rate: Option<u32>,
    worker_endpoints: bool,
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
    Retire(R, Option<crate::candidate_custody::CandidateRoot>),
    Cleanup(crate::candidate_custody::CandidateRoot),
    Open(OpenRequest<T>),
}

#[cfg(test)]
fn run_lifecycle<T: Send + 'static, R: Send + 'static>(
    receiver: Receiver<LifecycleMessage<T, R>>,
    is_occupied: impl Fn() -> bool,
    mut try_open: impl FnMut(Option<u32>) -> Option<Result<T, String>>,
    retire: impl FnMut(R),
) {
    run_lifecycle_selected(receiver, is_occupied, |rate, _| try_open(rate), retire)
}

fn run_lifecycle_selected<T: Send + 'static, R: Send + 'static>(
    receiver: Receiver<LifecycleMessage<T, R>>,
    is_occupied: impl Fn() -> bool,
    try_open: impl FnMut(Option<u32>, bool) -> Option<Result<T, String>>,
    retire: impl FnMut(R),
) {
    run_lifecycle_with_control_jobs(
        receiver,
        is_occupied,
        try_open,
        retire,
        retained_families::advance_registered_family_on_control,
    );
}

fn run_lifecycle_with_control_jobs<T: Send + 'static, R: Send + 'static>(
    receiver: Receiver<LifecycleMessage<T, R>>,
    is_occupied: impl Fn() -> bool,
    mut try_open: impl FnMut(Option<u32>, bool) -> Option<Result<T, String>>,
    mut retire: impl FnMut(R),
    mut advance_control_family: impl FnMut(),
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
    let mut cleanup: Option<crate::candidate_custody::CandidateRoot> = None;
    // On an impossible mismatched-root dispatch, retain all owning messages
    // instead of releasing a lease or replacing the pending root.
    let mut failed_retirement: Option<R> = None;
    let mut failed_cleanup: Option<crate::candidate_custody::CandidateRoot> = None;
    loop {
        advance_control_family();
        if failed_retirement.is_some() || failed_cleanup.is_some() {
            thread::sleep(Duration::from_millis(250));
            continue;
        }
        if let Some(root) = cleanup.as_ref() {
            root.advance_on_control();
            if root.job_idle() {
                cleanup = None;
            }
        }
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
                        current.request.worker_endpoints,
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
            Ok(LifecycleMessage::Retire(value, root)) => {
                if let Some(root) = root {
                    if cleanup
                        .as_ref()
                        .is_some_and(|pending| !pending.same_root(&root))
                    {
                        failed_retirement = Some(value);
                        failed_cleanup = Some(root);
                        continue;
                    }
                    // FIFO + announcement-before-enqueue ensures no old cleanup
                    // can trail this Retire or overlap the next output's lease.
                    cleanup = None;
                    root.adopt_retirement_job();
                }
                retire(value);
            }
            Ok(LifecycleMessage::Cleanup(root)) => {
                if cleanup
                    .as_ref()
                    .is_some_and(|pending| !pending.same_root(&root))
                {
                    failed_cleanup = Some(root);
                    continue;
                }
                if cleanup.is_none() {
                    cleanup = Some(root);
                }
            }
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

fn service() -> Result<&'static SyncSender<LifecycleMessage<Opened, Retirement>>, String> {
    SERVICE
        .get_or_init(|| {
            let (sender, receiver) = mpsc::sync_channel::<LifecycleMessage<Opened, Retirement>>(3);
            thread::Builder::new()
                .name("hex-audio-lifecycle".into())
                .spawn(move || {
                    run_lifecycle_selected(
                        receiver,
                        || OCCUPIED.load(Acquire),
                        |expected_rate, worker_endpoints| {
                            let lease = Lease::try_acquire()?;
                            Some(crate::device::open_prepared(
                                expected_rate,
                                lease,
                                worker_endpoints,
                            ))
                        },
                        retire_prepared_output,
                    );
                })
                .map_err(|error| format!("could not start audio lifecycle worker: {error}"))?;
            Ok(sender)
        })
        .as_ref()
        .map_err(Clone::clone)
}

pub(crate) fn prepare_candidate_cleanup() -> Result<(), String> {
    service().map(|_| ())
}
pub(crate) fn request_candidate_cleanup(root: &crate::candidate_custody::CandidateRoot) {
    if !root.needs_cleanup_request() {
        return;
    }
    // UI/Drop polling must never lazily initialize a thread/executor. Native
    // enablement prepared this exact sender before any backend exposure.
    let Some(Ok(sender)) = SERVICE.get() else {
        return;
    };
    root.schedule(|root| sender.try_send(LifecycleMessage::Cleanup(root)).is_ok());
}

fn retire_prepared_output(mut retirement: Retirement) {
    // Existing single leased lifecycle lane is also the persistent execution
    // slot. It stops consuming additional messages while uncertain ownership is
    // retained; no new thread/registry or destructor retry is introduced.
    loop {
        if !retirement.failed {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                retirement_tick(&mut retirement)
            }));
            match result {
                Ok(Ok(true)) => return,
                Ok(Ok(false)) => {}
                Ok(Err(())) => retirement.failed = true,
                Err(payload) => {
                    retirement.first_panic = Some(payload);
                    retirement.failed = true;
                }
            }
        }
        thread::sleep(Duration::from_millis(250));
    }
}
fn retirement_tick(retirement: &mut Retirement) -> Result<bool, ()> {
    #[cfg(feature = "allocation-probe")]
    if retirement.headless {
        return dispose_stream_and_native_on_control(
            &mut retirement.headless_backend,
            true,
            &mut retirement.stream_drop_pending,
            &mut retirement.stream_disposed,
            retirement.native.as_mut(),
            |_| Ok(()),
        );
    }
    if !dispose_stream_and_native_on_control(
        &mut retirement.stream,
        retirement.backend_exposed,
        &mut retirement.stream_drop_pending,
        &mut retirement.stream_disposed,
        retirement.native.as_mut(),
        |stream| stream.pause().map_err(|_| ()),
    )? {
        return Ok(false);
    }
    if let Some(reaper) = retirement.spectrum_reaper.as_mut() {
        if reaper.advance_shutdown_on_control().is_err() {
            return Err(());
        }
    }
    drop(retirement.spectrum_reaper.take());
    Ok(true)
}

// One matched retirement bundle owns both the actual stream handle and native
// terminal root. The witness is never exposed to callers or detached from this
// completed take/Drop frontier; a root cannot be reused for another incarnation.
fn dispose_stream_and_native_on_control<S>(
    stream: &mut Option<S>,
    backend_exposed: bool,
    pending: &mut bool,
    disposed: &mut bool,
    native: Option<&mut crate::native_callback_owner::NativeRetirement>,
    pause: impl FnOnce(&S) -> Result<(), ()>,
) -> Result<bool, ()> {
    if !dispose_stream_on_control(stream, backend_exposed, pending, disposed, pause)? {
        return Ok(false);
    }
    if let Some(native) = native {
        return native.advance_after_stream_disposal(&CompletedStreamHandle {
            root: native.root().root_id(),
        });
    }
    Ok(true)
}
#[cfg(all(test, feature = "allocation-probe"))]
pub(crate) fn test_dispose_backend<S>(
    stream: &mut Option<S>,
    pending: &mut bool,
    disposed: &mut bool,
    native: &mut crate::native_callback_owner::NativeRetirement,
    pause: impl FnOnce(&S) -> Result<(), ()>,
) -> Result<bool, ()> {
    dispose_stream_and_native_on_control(stream, true, pending, disposed, Some(native), pause)
}

// Same physical owner frontier for legacy and prepared native output. A missing
// handle after backend exposure is uncertainty, including failed CPAL build.
fn dispose_stream_on_control<S>(
    stream: &mut Option<S>,
    backend_exposed: bool,
    pending: &mut bool,
    disposed: &mut bool,
    pause: impl FnOnce(&S) -> Result<(), ()>,
) -> Result<bool, ()> {
    if *pending {
        return Err(());
    }
    if *disposed {
        return Ok(true);
    }
    if let Some(stream) = stream.as_ref() {
        if pause(stream).is_err() {
            return Ok(false);
        }
    } else if backend_exposed {
        return Err(());
    }
    *pending = true;
    drop(stream.take());
    *disposed = true;
    *pending = false;
    Ok(true)
}

#[cfg(test)]
mod candidate_retirement_tests {
    use super::*;
    use crate::native_callback_owner::{ErrorCallbackOwner, Guard, NativeCallbackOwner};
    struct Backend {
        data: Option<Guard<NativeCallbackOwner>>,
        errors: Option<Guard<ErrorCallbackOwner>>,
    }
    impl Drop for Backend {
        fn drop(&mut self) {
            drop(self.data.take());
            drop(self.errors.take());
        }
    }
    #[test]
    fn worker_open_uses_same_lane_and_preserves_negotiated_completion_and_abandonment() {
        let (sender, receiver) = mpsc::sync_channel::<LifecycleMessage<u32, ()>>(3);
        let state = Arc::new(OpenState::new());
        let mut waiter = enqueue_open_kind(&sender, &state, None, true).unwrap();
        assert!(matches!(
            enqueue_open(&sender, &state, None),
            Err(RecoveryOpenError::Busy)
        ));
        let worker = std::thread::spawn(move || {
            run_lifecycle_selected(
                receiver,
                || false,
                |rate, worker| {
                    assert!(worker);
                    assert_eq!(rate, None);
                    Some(Ok(44100))
                },
                |_| {},
            )
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(result) = waiter.try_take().unwrap() {
                assert_eq!(result.unwrap(), 44100);
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        drop(waiter);
        // The real service sender is static. Wait for its acknowledgement tick
        // before terminating this isolated test lane.
        while state.reserved.load(Acquire) != 0 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        drop(sender);
        worker.join().unwrap();
        assert_eq!(state.reserved.load(Acquire), 0);
    }
    #[test]
    fn bounded_open_cleanup_retire_order_adopts_old_job_before_next_open() {
        let _serial = crate::prepared::candidate_tests::serial();
        let root = crate::candidate_custody::CandidateRoot::new(2).unwrap();
        let (sender, receiver) = mpsc::sync_channel::<LifecycleMessage<(), ()>>(3);
        let state = Arc::new(OpenState::new());
        let mut open = enqueue_open(&sender, &state, None).unwrap();
        assert!(
            sender
                .try_send(LifecycleMessage::Cleanup(root.clone()))
                .is_ok()
        );
        root.announce_retirement();
        assert!(
            sender
                .try_send(LifecycleMessage::Retire((), Some(root.clone())))
                .is_ok()
        );
        let occupied = Arc::new(AtomicBool::new(true));
        let occupied_read = occupied.clone();
        let occupied_open = occupied.clone();
        let worker = std::thread::spawn(move || {
            run_lifecycle(
                receiver,
                || occupied_read.load(Acquire),
                |_| (!occupied_open.load(Acquire)).then_some(Ok(())),
                |_| {
                    occupied.store(false, Release);
                },
            )
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(result) = open.try_take().unwrap() {
                result.unwrap();
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        // No later cleanup send is possible after retirement announcement.
        let mut late_send = false;
        root.schedule(|_| {
            late_send = true;
            true
        });
        assert!(!late_send);
        drop(open);
        drop(sender);
        worker.join().unwrap();
    }
    #[test]
    fn pre_exposure_build_failure_and_exposed_missing_handle_have_distinct_custody() {
        let _serial = crate::prepared::candidate_tests::serial();
        for exposed in [false, true] {
            let (control, data, errors, mut native) = crate::prepared::candidate_tests::anchored();
            drop(control);
            drop(data);
            drop(errors);
            let mut stream: Option<Backend> = None;
            let (mut pending, mut disposed) = (false, false);
            let result = dispose_stream_on_control(
                &mut stream,
                exposed,
                &mut pending,
                &mut disposed,
                |_| Ok(()),
            );
            assert_eq!(result, if exposed { Err(()) } else { Ok(true) });
            assert_eq!(disposed, !exposed);
            // Receipt availability cannot turn exposed BuildErr into stop proof.
            crate::prepared::candidate_tests::finish(&mut native);
            assert!(!pending);
        }
    }
    #[test]
    fn returned_stream_play_failure_pause_pressure_and_release_orders_keep_real_queued_owner() {
        let _serial = crate::prepared::candidate_tests::serial();
        let Some(owner) = crate::prepared::candidate_tests::candidate() else {
            return;
        };
        let (mut control, data, errors, mut native) = crate::prepared::candidate_tests::anchored();
        let root = control.native_candidate_root();
        let _ticket = control.stage_audio_candidate(owner, vec![]).ok().unwrap();
        drop(control);
        let mut stream = Some(Backend {
            data: Some(data),
            errors: Some(errors),
        });
        let (mut pending, mut disposed) = (false, false);
        // Returned handle exists even when play fails. Only actual successful
        // pause and destruction of that handle advance this frontier.
        assert_eq!(
            dispose_stream_on_control(&mut stream, true, &mut pending, &mut disposed, |_| Err(())),
            Ok(false)
        );
        assert_eq!(root.report().occupied_slots, 1);
        assert!(!native.advance().unwrap());
        assert!(stream.is_some());
        assert_eq!(
            dispose_stream_on_control(&mut stream, true, &mut pending, &mut disposed, |_| Ok(())),
            Ok(true)
        );
        assert_eq!(root.report().occupied_slots, 1); // guards moved, not destroyed contents
        crate::prepared::candidate_tests::finish(&mut native);
        assert_eq!(root.report().occupied_slots, 0);
    }
    #[test]
    fn stream_destructor_unwind_marks_pending_before_take_and_never_retries() {
        let _serial = crate::prepared::candidate_tests::serial();
        struct Panics(Arc<std::sync::atomic::AtomicU64>);
        impl Drop for Panics {
            fn drop(&mut self) {
                self.0.fetch_add(1, Release);
                panic!("actual stream destructor");
            }
        }
        let calls = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut stream = Some(Panics(calls.clone()));
        let (mut pending, mut disposed) = (false, false);
        let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dispose_stream_on_control(&mut stream, true, &mut pending, &mut disposed, |_| Ok(()))
        }))
        .err()
        .unwrap();
        assert!(pending && !disposed && stream.is_none());
        assert_eq!(
            dispose_stream_on_control(&mut stream, true, &mut pending, &mut disposed, |_| Ok(())),
            Err(())
        );
        assert_eq!(calls.load(Acquire), 1);
        drop(first);
    }
}

#[cfg(test)]
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
        let _serial = crate::prepared::candidate_tests::serial();
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

        assert!(sender.try_send(LifecycleMessage::Retire((), None)).is_ok());
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
