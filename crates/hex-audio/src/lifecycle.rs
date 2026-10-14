//! Bounded output lifecycle worker for stream retirement and recovery opens.
//! Stream pause/disposal and replacement device creation never run on realtime.
use crate::device::PreparedOutput;
use crate::native_callback_owner::{ErrorCallbackOwner, NativeCallbackOwner, NativeTerminals};
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
    time::{Duration, Instant},
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
fn tick_stream_root(execution: &mut RetirementExecution) {
    // Retry only existing bounded retirement custody, outside its mutex.
    if execution.is_none() {
        let mut pending = QUARANTINE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending.as_ref().is_some_and(|value| {
            !value.failed_incomplete && value.retry_at.is_none_or(|at| Instant::now() >= at)
        }) {
            *execution = pending.take(); // persistent worker destination before work
        }
    }
    retire_prepared_output(execution);

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
    terminal: Option<NativeTerminals>,
    backend_exposed: bool,
    stop_proved: bool,
    failed_incomplete: bool,
    retry_at: Option<Instant>,
    disposal: TerminalDisposal,
    first_execution_panic: Option<Box<dyn std::any::Any + Send>>,
    stream_disposal: DisposalState,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum DisposalState {
    #[default]
    Unreceived,
    Received,
    Pending,
    Complete,
}
#[derive(Default)]
struct TerminalDisposal {
    data: Option<Box<NativeCallbackOwner>>,
    errors: Option<Box<ErrorCallbackOwner>>,
    retained_payload: Option<Box<dyn std::any::Any + Send>>,
    data_state: DisposalState,
    errors_state: DisposalState,
    payload_state: DisposalState,
}
// The sole execution slot lives in the existing lifecycle loop, never in the
// unwinding closure. No additional queue/thread/registry or UI-owned request.
type RetirementExecution = Option<Retirement>;
pub(crate) fn retirement_execution_storage_bytes() -> Option<usize> {
    std::mem::size_of::<RetirementExecution>()
        .checked_add(std::mem::size_of::<Option<crate::prepared::NativeStreamRoot>>())?
        .checked_add(crate::prepared::NativeStreamRoot::cleanup_permit_storage_bytes())
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
                terminal: None,
                backend_exposed: true,
                stop_proved: false,
                failed_incomplete: false,
                retry_at: None,
                disposal: TerminalDisposal::default(),
                first_execution_panic: None,
                stream_disposal: DisposalState::Received,
            }),
        }
    }
    /// Install terminal receiver and lease custody before exposing the guard to CPAL.
    pub(crate) fn prepared_custody(
        lease: Lease,
        spectrum_reaper: Option<SpectrumReaper>,
        media_root: Option<crate::prepared::NativeStreamRoot>,
        terminal: NativeTerminals,
    ) -> Self {
        Self {
            retirement: Some(Retirement {
                stream: None,
                media_root,
                spectrum_reaper,
                _lease: lease,
                terminal: Some(terminal),
                backend_exposed: false,
                stop_proved: false,
                failed_incomplete: false,
                retry_at: None,
                disposal: TerminalDisposal::default(),
                first_execution_panic: None,
                stream_disposal: DisposalState::Unreceived,
            }),
        }
    }
    pub(crate) fn expose_prepared_backend(&mut self) {
        self.retirement
            .as_mut()
            .expect("unretired construction custody")
            .backend_exposed = true;
    }
    pub(crate) fn install_prepared_stream(&mut self, stream: cpal::Stream) {
        let retirement = self
            .retirement
            .as_mut()
            .expect("unretired construction custody");
        retirement.stream = Some(stream);
        retirement.stream_disposal = DisposalState::Received;
    }
    pub(crate) fn play_prepared_stream(&self) -> Result<(), cpal::PlayStreamError> {
        self.retirement
            .as_ref()
            .expect("unretired construction custody")
            .stream
            .as_ref()
            .expect("installed prepared stream")
            .play()
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
    mut retire: impl FnMut(R),
) {
    run_lifecycle_with_tick(
        receiver,
        is_occupied,
        |rate, _extent| try_open(rate),
        |slot| {
            if let Some(value) = slot.take() {
                retire(value);
            }
        },
        |_| {},
    )
}
fn run_lifecycle_with_tick<T: Send + 'static, R: Send + 'static>(
    receiver: Receiver<LifecycleMessage<T, R>>,
    is_occupied: impl Fn() -> bool,
    mut try_open: impl FnMut(Option<u32>, Option<usize>) -> Option<Result<T, String>>,
    mut retire: impl FnMut(&mut Option<R>),
    mut tick: impl FnMut(&mut Option<R>),
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
    let mut retirement: Option<R> = None;
    loop {
        tick(&mut retirement);
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

        // Do not consume another queue owner while execution remains occupied.
        // Accepted messages retain their original storage in the existing ring.
        if retirement.is_some() {
            thread::sleep(Duration::from_millis(5));
            continue;
        }
        match receiver.recv_timeout(Duration::from_millis(5)) {
            Ok(LifecycleMessage::Retire(value)) => {
                retirement = Some(value); // vacant preflight above, no replacement
                retire(&mut retirement);
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum RetirementProgress {
    Pending,
    Complete,
    FailedIncomplete,
}
fn retire_prepared_output(execution: &mut RetirementExecution) {
    let Some(retirement) = execution.as_ref() else {
        return;
    };
    if retirement.failed_incomplete || retirement.first_execution_panic.is_some() {
        reconcile_retirement(execution);
        return;
    }
    let disposal_root = retirement.media_root.clone(); // preflight control anchor
    if let Some(root) = disposal_root.as_ref() {
        root.cancel_all_preparation();
    }
    let mut permit = match disposal_root.as_ref() {
        Some(root) => match root.cleanup_permit() {
            Ok(permit) => Some(permit),
            Err(phase) => {
                if let Some(retirement) = execution.as_mut() {
                    retirement.failed_incomplete |=
                        phase == hex_stream::MediaCleanupPhase::FailedIncomplete;
                }
                reconcile_retirement(execution);
                return;
            }
        },
        None => None,
    };
    // The permit lives OUTSIDE unwind. Each owner/payload lives in separate
    // persistent fields; failure is recorded/latches root before permit release.
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match execution.as_mut() {
            Some(retirement) => retirement_dispose(retirement, permit.as_mut()),
            None => RetirementProgress::FailedIncomplete,
        }));
    let progress = match result {
        Ok(progress) => progress,
        Err(payload) => {
            if let Some(retirement) = execution.as_mut() {
                retirement.first_execution_panic = Some(payload); // None preflight, no overwrite
                retirement.failed_incomplete = true;
            }
            RetirementProgress::FailedIncomplete
        }
    };
    if progress == RetirementProgress::FailedIncomplete {
        if let Some(retirement) = execution.as_mut() {
            retirement.failed_incomplete = true;
        }
        if let Some(permit) = permit.as_mut() {
            permit.fail_retirement_on_control();
        }
    }
    drop(permit); // root/payload failure is durable before releasing custody
    if progress != RetirementProgress::Complete {
        if let Some(retirement) = execution.as_mut() {
            retirement.retry_at = Some(Instant::now() + Duration::from_millis(250));
        }
        reconcile_retirement(execution);
        return;
    }
    // Root cleanup needs its own custody lock, after the disposal permit ended.
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match execution.as_mut() {
            Some(retirement) => retirement_finish(retirement),
            None => RetirementProgress::FailedIncomplete,
        }));
    match result {
        Ok(RetirementProgress::Complete) => {
            drop(execution.take());
        }
        Ok(progress) => {
            if let Some(retirement) = execution.as_mut() {
                retirement.failed_incomplete |= progress == RetirementProgress::FailedIncomplete;
                retirement.retry_at = Some(Instant::now() + Duration::from_millis(250));
                if retirement.failed_incomplete {
                    if let Some(root) = retirement.media_root.as_ref() {
                        root.fail_retirement_on_control();
                    }
                }
            }
            reconcile_retirement(execution);
        }
        Err(payload) => {
            if let Some(retirement) = execution.as_mut() {
                retirement.first_execution_panic = Some(payload); // first phase succeeded, destination still None
                retirement.failed_incomplete = true;
                if let Some(root) = retirement.media_root.as_ref() {
                    root.fail_retirement_on_control();
                }
            }
            reconcile_retirement(execution);
        }
    }
}

fn reconcile_retirement(execution: &mut RetirementExecution) {
    let mut pending = QUARANTINE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if pending.is_none() {
        *pending = execution.take();
    }
    // Pressure retains the existing quarantine AND worker slot. No overwrite,
    // owner drop or repeated destructor; the runner stops popping more messages.
}
/// Actual persistent execution fixture; never enables production admission.
#[cfg(all(test, feature = "allocation-probe"))]
pub(crate) fn tracked_retirement_execution_for_test(
    callback: crate::prepared::Callback,
    status: Arc<crate::prepared::Status>,
    root: &crate::prepared::NativeStreamRoot,
    mut inspect: impl FnMut(&NativeCallbackOwner, usize),
) {
    let _serialized = tests::LEASE_TEST.lock().unwrap();
    let lease = Lease::try_acquire().unwrap();
    let (mut data, errors, terminal) =
        crate::native_callback_owner::reserve_native(callback, status);
    assert!(
        data.owner_mut()
            .run(|_| panic!("tracked retirement callback fault"))
            .is_err()
    );
    let identity = data.owner_mut() as *mut NativeCallbackOwner;
    thread::spawn(move || {
        drop(data);
        drop(errors);
    })
    .join()
    .unwrap();
    let mut owner = StreamOwner::prepared_custody(lease, None, Some(root.clone()), terminal);
    let mut execution: RetirementExecution = owner.retirement.take();
    drop(owner);
    let mut permit = root.cleanup_permit().unwrap();
    for step in 0..2 {
        let retirement = execution.as_mut().unwrap();
        assert!(matches!(
            retirement_dispose(retirement, Some(&mut permit)),
            RetirementProgress::Pending
        ));
        assert_eq!(retirement.disposal.data_state, DisposalState::Received);
        assert_eq!(retirement.disposal.errors_state, DisposalState::Received);
        let retained = retirement.disposal.data.as_ref().unwrap();
        assert_eq!(
            retained.as_ref() as *const NativeCallbackOwner,
            identity.cast_const()
        );
        assert!(retained.callback_fault_latched());
        inspect(retained, step);
    }
    drop(permit);
    // Explicit fixture teardown only. Production keeps the same execution
    // anchored until actual cleanup authority; no success fact is fabricated.
    drop(execution);
}

fn retirement_dispose(
    retirement: &mut Retirement,
    permit: Option<&mut crate::prepared::NativeCleanupPermit<'_>>,
) -> RetirementProgress {
    if let Some(root) = retirement.media_root.as_ref() {
        root.cancel_all_preparation();
    }
    if retirement.terminal.is_some() || retirement.disposal.data_state != DisposalState::Unreceived
    {
        match native_quiescence(
            &mut retirement.stream,
            retirement.backend_exposed,
            &mut retirement.stop_proved,
            &mut retirement.stream_disposal,
            |stream| stream.pause().is_ok(),
        ) {
            NativeQuiescence::Pending => return RetirementProgress::Pending,
            NativeQuiescence::Unknown => return RetirementProgress::FailedIncomplete,
            NativeQuiescence::Proved => {}
        }
        let disposal = &mut retirement.disposal;
        if disposal.data_state == DisposalState::Unreceived {
            if disposal.data.is_some()
                || disposal.errors.is_some()
                || disposal.retained_payload.is_some()
            {
                return RetirementProgress::FailedIncomplete;
            }
            let Some(terminal) = retirement.terminal.as_mut() else {
                return RetirementProgress::FailedIncomplete;
            };
            let Some((mut data, errors)) = terminal.try_take() else {
                return RetirementProgress::Pending;
            };
            // Both owners and detached original panic allocation move into the
            // persistent slot before any destructor. No fallible operation here.
            disposal.retained_payload = data.take_retained_panic_on_control();
            disposal.data = Some(data);
            disposal.errors = Some(errors);
            disposal.data_state = DisposalState::Received;
            disposal.errors_state = DisposalState::Received;
            disposal.payload_state = DisposalState::Received;
        }
        if disposal.data_state == DisposalState::Received {
            if let Some(data) = disposal.data.as_mut() {
                // Every media-root callback must pass terminal extraction,
                // including active/deferred/queued tracked owners. A legacy
                // queue head is not evidence that later owners are untracked.
                if let Some(permit) = permit {
                    match permit.stage_held_claimed_on_control(data) {
                        Ok(()) => {}
                        Err(hex_stream::MediaCleanupPhase::Pending) => {
                            return RetirementProgress::Pending;
                        }
                        Err(_) => return RetirementProgress::FailedIncomplete,
                    }
                } else if data.held_claimed_on_control().is_some()
                    || data.peek_tracked_on_control().is_some()
                {
                    return RetirementProgress::FailedIncomplete;
                }
            }
            if disposal.data.is_none() {
                return RetirementProgress::FailedIncomplete;
            }
            disposal.data_state = DisposalState::Pending;
            drop(disposal.data.take());
            disposal.data_state = DisposalState::Complete;
        }
        if disposal.data_state != DisposalState::Complete {
            return RetirementProgress::FailedIncomplete;
        }
        if disposal.errors_state == DisposalState::Received {
            if disposal.errors.is_none() {
                return RetirementProgress::FailedIncomplete;
            }
            disposal.errors_state = DisposalState::Pending;
            drop(disposal.errors.take());
            disposal.errors_state = DisposalState::Complete;
        }
        if disposal.errors_state != DisposalState::Complete {
            return RetirementProgress::FailedIncomplete;
        }
        if disposal.payload_state == DisposalState::Received {
            // Payload is independent of callback/error owners. A panic cannot
            // run their destructors again or drop another retained payload.
            disposal.payload_state = DisposalState::Pending;
            drop(disposal.retained_payload.take());
            disposal.payload_state = DisposalState::Complete;
        }
        if disposal.payload_state != DisposalState::Complete {
            return RetirementProgress::FailedIncomplete;
        }
        retirement.terminal = None; // cells consumed, no owner/payload remains
    } else {
        if retirement
            .stream
            .as_ref()
            .is_some_and(|stream| stream.pause().is_err())
        {
            return RetirementProgress::Pending;
        }
        retirement.stop_proved = true;
        if retirement.stream_disposal == DisposalState::Pending {
            return RetirementProgress::FailedIncomplete;
        }
        retirement.stream_disposal = DisposalState::Pending;
        drop(retirement.stream.take());
        retirement.stream_disposal = DisposalState::Complete;
    }
    RetirementProgress::Complete
}
fn retirement_finish(retirement: &mut Retirement) -> RetirementProgress {
    if let Some(root) = retirement.media_root.as_ref() {
        match root.tick(true) {
            hex_stream::MediaCleanupPhase::Complete => {}
            hex_stream::MediaCleanupPhase::Pending => return RetirementProgress::Pending,
            hex_stream::MediaCleanupPhase::FailedIncomplete => {
                return RetirementProgress::FailedIncomplete;
            }
        }
        let active = {
            ACTIVE_STREAM_ROOT
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        };
        drop(active); // outside global/ordinary state mutexes
    }
    if let Some(reaper) = retirement.spectrum_reaper.take() {
        if reaper.shutdown_and_join().is_err() {
            return RetirementProgress::FailedIncomplete;
        }
    }
    RetirementProgress::Complete
}

#[derive(Debug, PartialEq, Eq)]
enum NativeQuiescence {
    Pending,
    Unknown,
    Proved,
}
fn native_quiescence<S>(
    stream: &mut Option<S>,
    exposed: bool,
    proved: &mut bool,
    disposal: &mut DisposalState,
    pause: impl FnOnce(&S) -> bool,
) -> NativeQuiescence {
    if *disposal == DisposalState::Pending {
        return NativeQuiescence::Unknown;
    }
    if *disposal == DisposalState::Complete {
        return if *proved && stream.is_none() {
            NativeQuiescence::Proved
        } else {
            NativeQuiescence::Unknown
        };
    }
    if !*proved {
        if let Some(stream) = stream.as_ref() {
            if !pause(stream) {
                return NativeQuiescence::Pending;
            }
            *proved = true;
        } else if !exposed {
            *proved = true; // No backend ever received the guard.
        } else {
            // CoreAudio can start inside construction then return no handle.
            return NativeQuiescence::Unknown;
        }
    }
    *disposal = DisposalState::Pending;
    drop(stream.take());
    *disposal = DisposalState::Complete;
    NativeQuiescence::Proved
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
pub(crate) mod tests {
    pub(crate) static LEASE_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());
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
        let _serialized = LEASE_TEST.lock().unwrap();
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

#[cfg(test)]
mod terminal_custody_tests {
    use super::*;
    use crate::native_callback_owner::reserve_native;
    use std::sync::atomic::AtomicUsize;
    #[test]
    fn constructor_play_pause_and_unknown_stop_paths_preserve_actual_owners() {
        struct Handle(Arc<AtomicUsize>);
        impl Drop for Handle {
            fn drop(&mut self) {
                self.0.fetch_add(1, Acquire);
            }
        }
        let drops = Arc::new(AtomicUsize::new(0));
        let mut stream = Some(Handle(Arc::clone(&drops)));
        let mut proved = false;
        let mut disposal = DisposalState::Unreceived;
        // Play failure still retains a real handle; failed pause never drops it.
        assert_eq!(
            native_quiescence(&mut stream, true, &mut proved, &mut disposal, |_| false),
            NativeQuiescence::Pending
        );
        assert!(stream.is_some());
        assert_eq!(drops.load(Acquire), 0);
        assert_eq!(
            native_quiescence(&mut stream, true, &mut proved, &mut disposal, |_| true),
            NativeQuiescence::Proved
        );
        assert!(stream.is_none());
        assert_eq!(drops.load(Acquire), 1);
        let mut unexposed: Option<Handle> = None;
        let mut proved = false;
        let mut disposal = DisposalState::Unreceived;
        assert_eq!(
            native_quiescence(&mut unexposed, false, &mut proved, &mut disposal, |_| true),
            NativeQuiescence::Proved
        );
        let mut unknown: Option<Handle> = None;
        let mut proved = false;
        let mut disposal = DisposalState::Unreceived;
        assert_eq!(
            native_quiescence(&mut unknown, true, &mut proved, &mut disposal, |_| true),
            NativeQuiescence::Unknown
        );
        assert!(!proved);
    }
    #[test]
    fn ordinary_handle_drop_retains_existing_custody_until_delayed_guard_release() {
        let _serialized = super::tests::LEASE_TEST.lock().unwrap();
        let lease = Lease::acquire().unwrap();
        let (control, callback) = crate::prepared::channel(48_000).unwrap();
        let (guard, errors, terminal) = reserve_native(callback, Arc::clone(&control.status));
        // Unexposed headless construction has no native stream to stop. This
        // tests actual service/quarantine/lease custody, not native pause.
        let owner = StreamOwner::prepared_custody(lease, None, None, terminal);
        drop(control);
        drop(owner);
        assert!(Lease::acquire().is_err());
        let (release, wait) = mpsc::sync_channel(1);
        let backend = thread::spawn(move || {
            wait.recv().unwrap();
            drop(guard);
        });
        thread::sleep(Duration::from_millis(20));
        assert!(OCCUPIED.load(Acquire));
        release.send(()).unwrap();
        backend.join().unwrap();
        assert!(OCCUPIED.load(Acquire));
        thread::spawn(move || drop(errors)).join().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while OCCUPIED.load(Acquire) && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(!OCCUPIED.load(Acquire));
        assert!(QUARANTINE.lock().unwrap().is_none());
    }
}

#[cfg(test)]
mod retirement_execution_tests {
    use super::*;
    use crate::native_callback_owner::reserve_native;
    use std::sync::atomic::AtomicUsize;

    struct Payload {
        panic_on_drop: Arc<AtomicBool>,
        drops: Arc<AtomicUsize>,
        dropped_on: Arc<Mutex<Vec<thread::ThreadId>>>,
    }
    impl Drop for Payload {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Acquire);
            self.dropped_on.lock().unwrap().push(thread::current().id());
            if self.panic_on_drop.load(Acquire) {
                panic!("retained callback payload destructor");
            }
        }
    }

    #[test]
    fn actual_coupled_owners_and_original_payload_survive_each_teardown_failure() {
        let _serialized = super::tests::LEASE_TEST.lock().unwrap();
        // 0 normal, 1 data owner panic, 2 error owner panic, 3 original
        // payload panic. These are actual boxed native owners/real lease/root.
        for failure in 0..4 {
            assert!(QUARANTINE.lock().unwrap().is_none());
            let lease = Lease::try_acquire().unwrap();
            let (mut control, mut callback, bridge) =
                crate::prepared::channel_with_streaming(48_000, 27, 4).unwrap();
            let root =
                crate::prepared::NativeStreamRoot::attach(&mut control, &mut callback, bridge);
            let (mut data, mut errors, terminal) =
                reserve_native(callback, Arc::clone(&control.status));
            let data_id = data.owner_mut() as *mut NativeCallbackOwner as usize;
            let error_id = errors.owner_mut() as *mut ErrorCallbackOwner as usize;
            let observations = Arc::new(Mutex::new(Vec::new()));
            let data_observations = Arc::clone(&observations);
            let data_root = root.clone();
            data.owner_mut().observe_disposal(move || {
                assert!(data_root.phase().is_some()); // ordinary mutex is not held
                assert_eq!(
                    data_root.cleanup_permit().err(),
                    Some(hex_stream::MediaCleanupPhase::Pending)
                );
                data_observations
                    .lock()
                    .unwrap()
                    .push((data_id, thread::current().id()));
                if failure == 1 {
                    panic!("actual data owner destructor");
                }
            });
            let error_observations = Arc::clone(&observations);
            let error_root = root.clone();
            errors.owner_mut().observe_disposal(move || {
                assert!(error_root.phase().is_some()); // ordinary mutex is not held
                assert_eq!(
                    error_root.cleanup_permit().err(),
                    Some(hex_stream::MediaCleanupPhase::Pending)
                );
                error_observations
                    .lock()
                    .unwrap()
                    .push((error_id, thread::current().id()));
                if failure == 2 {
                    panic!("actual error owner destructor");
                }
            });
            let panic_on_drop = Arc::new(AtomicBool::new(failure != 0));
            let drops = Arc::new(AtomicUsize::new(0));
            let payload_threads = Arc::new(Mutex::new(Vec::new()));
            let original = Payload {
                panic_on_drop: Arc::clone(&panic_on_drop),
                drops: Arc::clone(&drops),
                dropped_on: Arc::clone(&payload_threads),
            };
            assert!(
                data.owner_mut()
                    .run(|_| std::panic::panic_any(original))
                    .is_err()
            );
            let expected_original_id = data.owner_mut().retained_panic_identity();
            assert!(expected_original_id.is_some());
            // Backend release cannot dispose boxed owners or the original payload.
            thread::spawn(move || {
                drop(data);
                drop(errors);
            })
            .join()
            .unwrap();
            assert_eq!(drops.load(Acquire), 0);
            let mut owner =
                StreamOwner::prepared_custody(lease, None, Some(root.clone()), terminal);
            let mut execution = owner.retirement.take();
            drop(owner);
            drop(control);
            let control_thread = thread::current().id();
            retire_prepared_output(&mut execution);
            assert!(execution.is_none());
            let mut retained = QUARANTINE.lock().unwrap().take();
            if failure == 0 {
                assert!(retained.is_none());
                assert!(!OCCUPIED.load(Acquire));
                assert_eq!(drops.load(Acquire), 1);
                assert_eq!(*payload_threads.lock().unwrap(), [control_thread]);
                assert_eq!(
                    *observations.lock().unwrap(),
                    [(data_id, control_thread), (error_id, control_thread)]
                );
                continue;
            }
            let retirement = retained.as_ref().unwrap();
            assert!(OCCUPIED.load(Acquire));
            assert!(retirement.failed_incomplete);
            assert!(retirement.stop_proved);
            assert_eq!(retirement.stream_disposal, DisposalState::Complete);
            assert!(retirement.first_execution_panic.is_some());
            assert_eq!(
                root.tick(true),
                hex_stream::MediaCleanupPhase::FailedIncomplete
            );
            assert_eq!(
                root.cleanup_permit().err(),
                Some(hex_stream::MediaCleanupPhase::FailedIncomplete)
            );
            assert_eq!(
                retirement.disposal.data_state,
                if failure == 1 {
                    DisposalState::Pending
                } else {
                    DisposalState::Complete
                }
            );
            assert_eq!(
                retirement.disposal.errors_state,
                if failure == 1 {
                    DisposalState::Received
                } else if failure == 2 {
                    DisposalState::Pending
                } else {
                    DisposalState::Complete
                }
            );
            assert_eq!(
                retirement.disposal.payload_state,
                if failure == 3 {
                    DisposalState::Pending
                } else {
                    DisposalState::Received
                }
            );
            assert_eq!(drops.load(Acquire), usize::from(failure == 3));
            let original_id = retirement
                .disposal
                .retained_payload
                .as_ref()
                .map(|p| p.as_ref() as *const dyn std::any::Any as *const () as usize);
            assert_eq!(
                original_id,
                if failure == 3 {
                    None
                } else {
                    expected_original_id
                }
            );
            assert_eq!(
                *payload_threads.lock().unwrap(),
                if failure == 3 {
                    vec![control_thread]
                } else {
                    vec![]
                }
            );
            let teardown_id = retirement
                .first_execution_panic
                .as_ref()
                .map(|p| p.as_ref() as *const dyn std::any::Any as *const () as usize);
            let observations_before = observations.lock().unwrap().clone();
            // Faulted reconciliation does not execute any remaining destructor,
            // reattempt the destroyed Box or overwrite either panic allocation.
            retire_prepared_output(&mut retained);
            assert!(retained.is_none());
            let mut retained = QUARANTINE.lock().unwrap().take().unwrap();
            assert_eq!(
                retained
                    .disposal
                    .retained_payload
                    .as_ref()
                    .map(|p| p.as_ref() as *const dyn std::any::Any as *const () as usize),
                original_id
            );
            assert_eq!(
                retained
                    .first_execution_panic
                    .as_ref()
                    .map(|p| p.as_ref() as *const dyn std::any::Any as *const () as usize),
                teardown_id
            );
            assert_eq!(*observations.lock().unwrap(), observations_before);
            assert!(Lease::try_acquire().is_none());
            // Explicit test-fixture disarm/disposal, NOT production recovery:
            // incomplete state is never relabelled complete or retried.
            panic_on_drop.store(false, Release);
            if let Some(errors) = retained.disposal.errors.as_mut() {
                errors.observe_disposal(|| {});
            }
            drop(retained);
            assert!(!OCCUPIED.load(Acquire));
        }
    }

    #[test]
    fn persistent_execution_pressure_keeps_queued_owner_until_slot_is_vacant() {
        // Queue/slot model: no second native Lease or hardware is fabricated.
        struct Owner(usize, Arc<Mutex<Vec<(usize, thread::ThreadId)>>>);
        impl Drop for Owner {
            fn drop(&mut self) {
                self.1
                    .lock()
                    .unwrap()
                    .push((self.0, thread::current().id()));
            }
        }
        let drops = Arc::new(Mutex::new(Vec::new()));
        let observations = Arc::new(Mutex::new(Vec::new()));
        let (sender, receiver) = mpsc::sync_channel::<LifecycleMessage<(), Owner>>(2);
        sender
            .send(LifecycleMessage::Retire(Owner(1, Arc::clone(&drops))))
            .unwrap();
        sender
            .send(LifecycleMessage::Retire(Owner(2, Arc::clone(&drops))))
            .unwrap();
        drop(sender);
        let retired = Arc::clone(&observations);
        let tick_drops = Arc::clone(&drops);
        let worker = thread::spawn(move || {
            let mut attempts = 0;
            run_lifecycle_with_tick(
                receiver,
                || false,
                |_, _| None,
                |slot| retired.lock().unwrap().push(slot.as_ref().unwrap().0),
                |slot| {
                    if let Some(owner) = slot.as_ref() {
                        if owner.0 == 1 {
                            attempts += 1;
                            assert!(tick_drops.lock().unwrap().is_empty());
                            if attempts < 3 {
                                return;
                            }
                        }
                        drop(slot.take()); // exclusive model control disposal
                    }
                },
            );
            thread::current().id()
        })
        .join()
        .unwrap();
        assert_eq!(*observations.lock().unwrap(), [1, 2]);
        assert_eq!(*drops.lock().unwrap(), [(1, worker), (2, worker)]);
        assert_ne!(worker, thread::current().id());
    }

    #[test]
    fn pause_success_does_not_complete_panicking_stream_disposal_or_retry_it() {
        struct Stream(Arc<AtomicUsize>);
        impl Drop for Stream {
            fn drop(&mut self) {
                self.0.fetch_add(1, Acquire);
                panic!("stream destructor");
            }
        }
        let drops = Arc::new(AtomicUsize::new(0));
        let mut stream = Some(Stream(Arc::clone(&drops)));
        let mut proved = false;
        let mut phase = DisposalState::Received;
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            native_quiescence(&mut stream, true, &mut proved, &mut phase, |_| true)
        }));
        assert!(failure.is_err());
        assert!(proved);
        assert!(stream.is_none());
        assert_eq!(phase, DisposalState::Pending);
        assert_eq!(
            native_quiescence(&mut stream, true, &mut proved, &mut phase, |_| panic!(
                "no retry"
            )),
            NativeQuiescence::Unknown
        );
        assert_eq!(drops.load(Acquire), 1);
    }
}
