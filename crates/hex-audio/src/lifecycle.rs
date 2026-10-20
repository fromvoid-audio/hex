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
            AtomicBool, AtomicU8, AtomicU64,
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
// 0: not started, 1: successfully spawned, 2: terminal startup/worker failure.
// A late successful spawn notification cannot resurrect an exited worker.
static SERVICE_READINESS: ServiceReadiness = ServiceReadiness::new();
struct ServiceReadiness(AtomicU8);
impl ServiceReadiness {
    const fn new() -> Self {
        Self(AtomicU8::new(0))
    }
    fn spawned(&self) {
        let _ = self.0.compare_exchange(0, 1, AcqRel, Acquire);
    }
    fn failed(&self) {
        self.0.store(2, Release);
    }
    fn ready(&self) -> bool {
        self.0.load(Acquire) == 1
    }
}
struct ServiceExitGuard<'a>(&'a ServiceReadiness);
impl Drop for ServiceExitGuard<'_> {
    fn drop(&mut self) {
        self.0.failed();
    }
}
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
    spectrum_construction: SpectrumConstruction,
    #[cfg(all(test, feature = "allocation-probe"))]
    spectrum_test_entry: Option<fn()>,
    _lease: Lease,
    native: Option<crate::native_callback_owner::NativeRetirement>,
    backend_exposed: bool,
    stream_drop_pending: bool,
    stream_disposed: bool,
    failed: bool,
    first_panic: Option<Box<dyn std::any::Any + Send>>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum SpectrumConstruction {
    NotRequested,
    Preparing,
    Installed,
    Unavailable,
}

pub(crate) struct StreamOwner {
    retirement: Option<Retirement>,
}
pub(crate) type ConstructedChannel = (
    StreamOwner,
    crate::prepared::Control,
    crate::prepared::Callback,
    Option<crate::spectrum_worker::SpectrumReader>,
    Option<crate::spectrum_worker::SpectrumWorkerError>,
);
impl StreamOwner {
    // The real device and headless discriminator use this SAME closed fixed
    // constructor. No approved totals/callback/plan escapes the preflight.
    pub(crate) fn construct_prepared_channel_on_control(
        lease: Lease,
        rate: u32,
        native_mode: bool,
        reservation: Option<crate::AudioCandidatePublicationReservation>,
    ) -> Result<ConstructedChannel, hex_engine::Invalid> {
        crate::prepared::preflight_native_worker_birth(native_mode, reservation)?;
        let (control, mut callback) =
            crate::prepared::channel_with_factory_reservation(rate, None, reservation)?;
        let root = control.native_candidate_root();
        let mut parent = Self::begin_prepared_custody(lease, root);
        let stamp = crate::spectrum_worker::SpectrumStamp {
            target: crate::spectrum_worker::SpectrumTargetId(0),
            generation: 0,
            sample_rate_hz: rate,
            applied_revision: 0,
        };
        let (reader, error) = match parent.construct_spectrum_on_control(stamp, native_mode) {
            Ok((capture, reader)) => {
                let mut capture = Some(capture);
                if callback
                    .attach_constructor_capture_on_control(&mut capture)
                    .is_err()
                {
                    drop(capture);
                    drop(reader);
                    drop(callback);
                    drop(control);
                    let _aborted = parent.abort_before_carriers();
                    return Err(hex_engine::Invalid(
                        "spectrum constructor capture installation refused",
                    ));
                }
                (Some(reader), None)
            }
            Err(error) => (None, Some(error)),
        };
        Ok((parent, control, callback, reader, error))
    }
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
                spectrum_construction: if spectrum_reaper.is_some() {
                    SpectrumConstruction::Installed
                } else {
                    SpectrumConstruction::NotRequested
                },
                spectrum_reaper,
                #[cfg(all(test, feature = "allocation-probe"))]
                spectrum_test_entry: None,
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
    #[cfg(feature = "allocation-probe")]
    pub(crate) fn prepared_custody(
        lease: Lease,
        spectrum_reaper: Option<SpectrumReaper>,
        native: crate::native_callback_owner::NativeRetirement,
    ) -> Self {
        native.bind_spectrum_reaper(spectrum_reaper.as_ref());
        Self::unexposed_custody(lease, spectrum_reaper, Some(native))
    }
    pub(crate) fn begin_prepared_custody(
        lease: Lease,
        root: crate::candidate_custody::CandidateRoot,
    ) -> Self {
        Self::unexposed_custody(
            lease,
            None,
            Some(crate::native_callback_owner::NativeRetirement::begin_construction(root)),
        )
    }
    // This is called only inside the same preflighted device construction.
    // The actual JoinHandle is installed before returning any capture/reader.
    pub(crate) fn construct_spectrum_on_control(
        &mut self,
        stamp: crate::spectrum_worker::SpectrumStamp,
        native_mode: bool,
    ) -> Result<
        (
            crate::spectrum_worker::SpectrumCapture,
            crate::spectrum_worker::SpectrumReader,
        ),
        crate::spectrum_worker::SpectrumWorkerError,
    > {
        let parent = self
            .retirement
            .as_mut()
            .filter(|p| {
                p.spectrum_construction == SpectrumConstruction::NotRequested
                    && p.spectrum_reaper.is_none()
                    && p.native.is_some()
            })
            .ok_or(crate::spectrum_worker::SpectrumWorkerError::PoolCapacity)?;
        parent.spectrum_construction = SpectrumConstruction::Preparing;
        #[cfg(all(test, feature = "allocation-probe"))]
        let entry = parent.spectrum_test_entry.take();
        #[cfg(not(all(test, feature = "allocation-probe")))]
        let entry = None;
        let result = crate::spectrum_worker::spawn_worker_with_parent(
            stamp,
            native_mode,
            entry,
            parent.native.as_ref(),
        );
        match result {
            Ok((capture, reader, reaper)) => {
                parent.spectrum_reaper = Some(reaper);
                parent.spectrum_construction = SpectrumConstruction::Installed;
                Ok((capture, reader))
            }
            Err(error) => {
                // The existing constructor has returned only after its real
                // partial closure/payload values have normally dropped. An
                // unwind instead leaves Preparing and is never absence proof.
                parent.spectrum_construction = SpectrumConstruction::Unavailable;
                Err(error)
            }
        }
    }
    pub(crate) fn install_constructed_native_carriers_on_control(
        &mut self,
        incoming: &mut Option<crate::native_callback_owner::NativeTerminals>,
    ) -> Result<(), ()> {
        let parent = self.retirement.as_mut().ok_or(())?;
        let native = parent.native.as_mut().ok_or(())?;
        native.install_constructed_carriers(incoming)?;
        native.bind_spectrum_reaper(parent.spectrum_reaper.as_ref());
        Ok(())
    }
    pub(crate) fn abort_before_carriers(&mut self) -> Result<(), ()> {
        self.retirement
            .as_mut()
            .and_then(|p| p.native.as_mut())
            .ok_or(())?
            .abort_before_carriers()
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
                spectrum_construction: if spectrum_reaper.is_some() {
                    SpectrumConstruction::Installed
                } else {
                    SpectrumConstruction::NotRequested
                },
                spectrum_reaper,
                #[cfg(all(test, feature = "allocation-probe"))]
                spectrum_test_entry: None,
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
    let waiter = enqueue_open_kind(sender, state(), expected_sample_rate, true, None)?;
    Ok(WorkerRecoveryOpen { waiter })
}

/// Native worker path carries the existing caller ceilings to the first Root
/// and queue births. This is not a complete native-root admission certificate.
pub fn request_worker_open_with_reservation(
    expected_sample_rate: Option<u32>,
    reservation: crate::AudioCandidatePublicationReservation,
) -> Result<WorkerRecoveryOpen, RecoveryOpenError> {
    if expected_sample_rate.is_some_and(|rate| !(8_000..=192_000).contains(&rate)) {
        return Err(RecoveryOpenError::InvalidRate);
    }
    let sender = service().map_err(|_| RecoveryOpenError::WorkerClosed)?;
    let waiter = enqueue_open_kind(
        sender,
        state(),
        expected_sample_rate,
        true,
        Some(reservation),
    )?;
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
    let waiter = enqueue_open_kind(sender, state(), expected_sample_rate, false, None)?;
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
    enqueue_open_kind(sender, state, expected_sample_rate, false, None)
}

fn enqueue_open_kind<T: Send + 'static, R: Send + 'static>(
    sender: &SyncSender<LifecycleMessage<T, R>>,
    state: &Arc<OpenState>,
    expected_sample_rate: Option<u32>,
    worker_endpoints: bool,
    factory_reservation: Option<crate::AudioCandidatePublicationReservation>,
) -> Result<OpenWaiter<T>, RecoveryOpenError> {
    enqueue_open_payload(
        sender,
        state,
        expected_sample_rate,
        worker_endpoints,
        factory_reservation,
        &mut None,
    )
}

// The SAME existing lane can transport the actual prepared parent/carriers.
// Validation/busy/full refusal leaves its producer-owned field populated.
// This private operation does not perform or certify whole-host admission.
fn enqueue_open_payload<T: Send + 'static, R: Send + 'static>(
    sender: &SyncSender<LifecycleMessage<T, R>>,
    state: &Arc<OpenState>,
    expected_sample_rate: Option<u32>,
    worker_endpoints: bool,
    factory_reservation: Option<crate::AudioCandidatePublicationReservation>,
    prepared: &mut Option<Box<crate::device::PreparedBackend>>,
) -> Result<OpenWaiter<T>, RecoveryOpenError> {
    if let Some(payload) = prepared.as_ref() {
        // These actual constructor fields must be inspected BEFORE request
        // completion/flag allocation, reservation or producer-field transfer.
        if !worker_endpoints || !payload.has_worker_endpoints() {
            return Err(RecoveryOpenError::WorkerClosed);
        }
        if expected_sample_rate != Some(payload.sample_rate()) {
            return Err(RecoveryOpenError::InvalidRate);
        }
    }
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
        factory_reservation,
        completion,
        canceled: Arc::clone(&canceled),
        consumed: Arc::clone(&consumed),
        abandoned: Arc::clone(&abandoned),
        state: Arc::clone(state),
        prepared: prepared.take(),
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
        Err(TrySendError::Full(LifecycleMessage::Open(mut request))) => {
            *prepared = request.prepared.take();
            finish_request(&request);
            Err(RecoveryOpenError::QueueFull)
        }
        Err(TrySendError::Disconnected(LifecycleMessage::Open(mut request))) => {
            *prepared = request.prepared.take();
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
    factory_reservation: Option<crate::AudioCandidatePublicationReservation>,
    completion: SyncSender<Result<T, String>>,
    canceled: Arc<AtomicBool>,
    consumed: Arc<AtomicBool>,
    abandoned: Arc<AtomicBool>,
    state: Arc<OpenState>,
    prepared: Option<Box<crate::device::PreparedBackend>>,
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
    run_lifecycle_selected(
        receiver,
        is_occupied,
        |rate, _, _, _| try_open(rate),
        retire,
    )
}

fn run_lifecycle_selected<T: Send + 'static, R: Send + 'static>(
    receiver: Receiver<LifecycleMessage<T, R>>,
    is_occupied: impl Fn() -> bool,
    try_open: impl FnMut(
        Option<u32>,
        bool,
        Option<crate::AudioCandidatePublicationReservation>,
        &mut Option<Box<crate::device::PreparedBackend>>,
    ) -> Option<Result<T, String>>,
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
    mut try_open: impl FnMut(
        Option<u32>,
        bool,
        Option<crate::AudioCandidatePublicationReservation>,
        &mut Option<Box<crate::device::PreparedBackend>>,
    ) -> Option<Result<T, String>>,
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
                    // This payload itself holds OCCUPIED. Relinquish it to its
                    // existing retirement route before waiting for that actual
                    // frontier; otherwise cancellation would wait on itself.
                    drop(current.request.prepared.take());
                    if !is_occupied() {
                        finish_request(&current.request);
                        active = None;
                    }
                }
                Phase::Queued => {
                    if let Some(result) = try_open(
                        current.request.expected_sample_rate,
                        current.request.worker_endpoints,
                        current.request.factory_reservation,
                        &mut current.request.prepared,
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
                    let _exit = ServiceExitGuard(&SERVICE_READINESS);
                    run_lifecycle_selected(
                        receiver,
                        || OCCUPIED.load(Acquire),
                        |expected_rate, worker_endpoints, factory_reservation, prepared| {
                            if let Some(prepared) = prepared.take() {
                                return Some(crate::device::finish_prepared_backend(prepared));
                            }
                            let lease = Lease::try_acquire()?;
                            Some(crate::device::open_prepared(
                                expected_rate,
                                lease,
                                worker_endpoints,
                                factory_reservation,
                            ))
                        },
                        retire_prepared_output,
                    );
                })
                .map_err(|error| {
                    SERVICE_READINESS.failed();
                    format!("could not start audio lifecycle worker: {error}")
                })?;
            SERVICE_READINESS.spawned();
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
    let disposed = if retirement.headless {
        dispose_stream_on_control(
            &mut retirement.headless_backend,
            true,
            &mut retirement.stream_drop_pending,
            &mut retirement.stream_disposed,
            |_| Ok(()),
        )?
    } else {
        dispose_retirement_stream(retirement)?
    };
    #[cfg(not(feature = "allocation-probe"))]
    let disposed = dispose_retirement_stream(retirement)?;
    if !disposed {
        return Ok(false);
    }
    if let Some(native) = retirement.native.as_mut()
        && !native.advance_native_fields_after_stream_disposal(&CompletedStreamHandle {
            root: native.root().root_id(),
        })?
    {
        return Ok(false);
    }
    retirement.advance_worker_then_final_native()
}
fn dispose_retirement_stream(retirement: &mut Retirement) -> Result<bool, ()> {
    dispose_stream_on_control(
        &mut retirement.stream,
        retirement.backend_exposed,
        &mut retirement.stream_drop_pending,
        &mut retirement.stream_disposed,
        |stream| stream.pause().map_err(|_| ()),
    )
}
impl Retirement {
    // Only this owning-parent driver crosses from actual analyzer join to
    // final Root completion. No caller-supplied join flag/receipt is accepted.
    fn advance_worker_then_final_native(&mut self) -> Result<bool, ()> {
        match (self.spectrum_construction, self.spectrum_reaper.as_ref()) {
            (SpectrumConstruction::Preparing, _) | (SpectrumConstruction::Installed, None) => {
                return Err(());
            }
            (SpectrumConstruction::NotRequested | SpectrumConstruction::Unavailable, Some(_)) => {
                return Err(());
            }
            _ => {}
        }
        if let Some(reaper) = self.spectrum_reaper.as_mut() {
            if let Some(native) = self.native.as_ref() {
                reaper
                    .validate_actual_stop_on_control(native)
                    .map_err(|_| ())?;
            }
            reaper.advance_shutdown_on_control().map_err(|_| ())?;
            if let Some(native) = self.native.as_ref() {
                reaper
                    .finalize_captured_frame_on_control(native)
                    .map_err(|_| ())?;
            }
        }
        if let Some(native) = self.native.as_mut()
            && !native.finish_root_after_native_fields_on_control()?
        {
            return Ok(false);
        }
        // Keep the completed Reaper in its actual parent until that parent
        // normally retires; repeated final-Pending calls inspect the same join.
        Ok(true)
    }
}

// One matched retirement bundle owns both the actual stream handle and native
// terminal root. The witness is never exposed to callers or detached from this
// completed take/Drop frontier; a root cannot be reused for another incarnation.
#[cfg(all(test, feature = "allocation-probe"))]
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

    #[cfg(feature = "allocation-probe")]
    fn prepared_request_payload(worker: bool) -> Box<crate::device::PreparedBackend> {
        let (retained, peak) = crate::prepared::native_worker_birth_bound(worker).unwrap();
        let reservation = crate::AudioCandidatePublicationReservation {
            runtime_slots: 3,
            archive_slots: 3,
            member_rows: 128,
            transfer_rows: 128,
            receipt_rows: 3,
            data_rows: 256,
            handle_rows: 256,
            max_retained_data_bytes: retained,
            max_construction_peak_data_bytes: peak,
        };
        let lease = Lease::acquire().unwrap();
        if worker {
            crate::device::prepare_backend_data(lease, 48000, true, Some(reservation))
        } else {
            crate::device::prepare_headless_legacy_test_stage(lease, 48000, reservation)
        }
        .unwrap()
    }

    #[cfg(feature = "allocation-probe")]
    fn wait_for_prepared_parent_retirement() {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while OCCUPIED.load(Acquire) {
            assert!(
                std::time::Instant::now() < deadline,
                "same parent did not retire"
            );
            thread::yield_now();
        }
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn prepared_worker_request_refuses_actual_legacy_stage_before_metadata_birth() {
        let _serial = crate::prepared::candidate_tests::serial();
        let mut payload = Some(prepared_request_payload(false));
        let original_identity = payload.as_ref().unwrap().test_output_identity();
        let state = Arc::new(OpenState::new());
        let (sender, receiver) = mpsc::sync_channel::<LifecycleMessage<(), ()>>(1);
        let mut outcome = None;
        let measured = allocation_counter::measure(|| {
            outcome = Some(enqueue_open_payload(
                &sender,
                &state,
                Some(48000),
                true,
                None,
                &mut payload,
            ));
        });
        let refused = matches!(outcome, Some(Err(RecoveryOpenError::WorkerClosed)));
        let retained_identity = payload.as_ref().map(|p| p.test_output_identity());
        let lane_untouched = state.reserved.load(Acquire) == 0 && !state.in_service.load(Acquire);
        // Even the RED case must normally drop/join its actual parent. Drop the
        // queued request too if a regression consumed the producer's stage.
        drop(outcome);
        drop(payload);
        drop(receiver);
        drop(sender);
        wait_for_prepared_parent_retirement();
        assert!(
            refused,
            "actual legacy-only stage was accepted as a worker payload"
        );
        assert_eq!(retained_identity, Some(original_identity));
        assert!(lane_untouched);
        assert_eq!(measured.count_total, 0);
        assert_eq!(measured.bytes_total, 0);
        assert_eq!(measured.count_current, 0);
        assert_eq!(measured.bytes_current, 0);
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn prepared_request_full_busy_mismatch_disconnect_retain_then_transfer_same_parent() {
        let _serial = crate::prepared::candidate_tests::serial();
        let mut payload = Some(prepared_request_payload(true));
        let identity = payload.as_ref().unwrap().test_output_identity();
        let state = Arc::new(OpenState::new());
        let (full, _receiver) = mpsc::sync_channel::<LifecycleMessage<u64, ()>>(0);
        assert!(matches!(
            enqueue_open_payload(&full, &state, Some(48000), true, None, &mut payload),
            Err(RecoveryOpenError::QueueFull)
        ));
        assert_eq!(payload.as_ref().unwrap().test_output_identity(), identity);
        assert_eq!(state.reserved.load(Acquire), 0);
        assert!(!state.in_service.load(Acquire));

        let refusal = allocation_counter::measure(|| {
            assert!(matches!(
                enqueue_open_payload(&full, &state, Some(44100), true, None, &mut payload),
                Err(RecoveryOpenError::InvalidRate)
            ));
            state.in_service.store(true, Release);
            assert!(matches!(
                enqueue_open_payload(&full, &state, Some(48000), true, None, &mut payload),
                Err(RecoveryOpenError::Busy)
            ));
            state.in_service.store(false, Release);
        });
        assert_eq!(refusal.count_total, 0);
        assert_eq!(refusal.bytes_total, 0);
        assert_eq!(payload.as_ref().unwrap().test_output_identity(), identity);
        let (closed, receiver) = mpsc::sync_channel::<LifecycleMessage<u64, ()>>(1);
        drop(receiver);
        assert!(matches!(
            enqueue_open_payload(&closed, &state, Some(48000), true, None, &mut payload),
            Err(RecoveryOpenError::WorkerClosed)
        ));
        assert_eq!(payload.as_ref().unwrap().test_output_identity(), identity);
        assert_eq!(state.reserved.load(Acquire), 0);

        let (sender, receiver) = mpsc::sync_channel::<LifecycleMessage<u64, ()>>(3);
        let mut waiter =
            enqueue_open_payload(&sender, &state, Some(48000), true, None, &mut payload).unwrap();
        assert!(payload.is_none());
        let worker = thread::spawn(move || {
            run_lifecycle_selected(
                receiver,
                || OCCUPIED.load(Acquire),
                |rate, worker, _, payload| {
                    assert_eq!(rate, Some(48000));
                    assert!(worker);
                    let actual = payload.take().expect("actual queued parent/carriers");
                    let actual_identity = actual.test_output_identity();
                    drop(actual); // SAME existing lifecycle retirement, no backend fixture.
                    Some(Ok(actual_identity))
                },
                |_| {},
            );
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(result) = waiter.try_take().unwrap() {
                assert_eq!(result.unwrap(), identity);
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            thread::yield_now();
        }
        while state.reserved.load(Acquire) != 0 {
            assert!(std::time::Instant::now() < deadline);
            thread::yield_now();
        }
        drop(waiter);
        drop(sender);
        worker.join().unwrap();
        wait_for_prepared_parent_retirement();
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn prepared_request_cancel_retires_its_occupied_parent_before_releasing_lane() {
        let _serial = crate::prepared::candidate_tests::serial();
        let mut payload = Some(prepared_request_payload(true));
        let state = Arc::new(OpenState::new());
        let (sender, receiver) = mpsc::sync_channel::<LifecycleMessage<(), ()>>(3);
        let waiter =
            enqueue_open_payload(&sender, &state, Some(48000), true, None, &mut payload).unwrap();
        assert!(payload.is_none());
        assert!(OCCUPIED.load(Acquire));
        drop(waiter); // Cancellation BEFORE the consumer claims the request.
        let worker = thread::spawn(move || {
            run_lifecycle_selected(
                receiver,
                || OCCUPIED.load(Acquire),
                |_, _, _, _| panic!("cancelled payload must not build a backend"),
                |_| {},
            );
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while state.reserved.load(Acquire) != 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "cancelled lane waited on its own lease"
            );
            thread::yield_now();
        }
        assert!(!OCCUPIED.load(Acquire));
        assert!(!state.in_service.load(Acquire));
        drop(sender);
        worker.join().unwrap();
    }
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
    #[cfg(feature = "allocation-probe")]
    #[test]
    fn actual_worker_joins_before_live_client_and_publication_endpoint_retirement() {
        let _serial = crate::prepared::candidate_tests::serial();
        let lease = Lease::acquire().unwrap();
        let (retained, peak) = crate::prepared::native_worker_birth_bound(true).unwrap();
        let reservation = worker_reservation(retained, peak);
        let (mut stream_owner, control, callback, reader, error) =
            StreamOwner::construct_prepared_channel_on_control(
                lease,
                48000,
                true,
                Some(reservation),
            )
            .unwrap();
        assert!(error.is_none());
        assert_eq!(
            stream_owner
                .retirement
                .as_ref()
                .unwrap()
                .native
                .as_ref()
                .unwrap()
                .root()
                .root_id(),
            control.native_candidate_root().root_id()
        );
        assert!(
            stream_owner
                .retirement
                .as_ref()
                .unwrap()
                .spectrum_reaper
                .is_some()
        );
        let root = control.native_candidate_root();
        let (disposed, full_capture_data) = root.test_spectrum_capture_facts().unwrap().unwrap();
        assert!(!disposed);
        let (mut data, errors, terminals, mut client, mut publication) =
            crate::native_callback_owner::reserve_native_worker(control, callback, reader).unwrap();
        publication
            .control()
            .unwrap()
            .enable_native_custody()
            .unwrap();
        stream_owner
            .install_constructed_native_carriers_on_control(&mut Some(terminals))
            .unwrap();
        // Real nonempty Project result from the actual worker and capture
        // field, using the real Client's issued selection. Direct capture test
        // input is not a qualified Service source/render admission claim.
        let selection = client.project_spectrum_select(&[], 1, 1, 48000).unwrap();
        let callback_measurement = allocation_counter::measure(|| {
            for _ in 0..16 {
                data.owner_mut().unwrap().run(|callback, _| {
                    assert!(matches!(
                        callback.test_submit_project_capture(selection, &[[0.25; 2]; 256]),
                        Some(crate::spectrum_worker::CaptureOutcome::Queued { .. })
                    ));
                });
            }
        });
        assert_eq!(
            (
                callback_measurement.count_total,
                callback_measurement.count_current,
                callback_measurement.bytes_total,
                callback_measurement.bytes_current
            ),
            (0, 0, 0, 0),
            "actual callback capture path allocates/frees nothing on its thread"
        );
        let mut project_snapshot = None;
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            client.refresh_project_spectrum(&mut project_snapshot, true);
            if project_snapshot.is_some() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "actual worker produced no Project result"
            );
            std::thread::yield_now();
        }
        let retirement = stream_owner.retirement.as_mut().unwrap();
        let mut backend = Some(Backend {
            data: Some(data),
            errors: Some(errors),
        });
        assert_eq!(
            dispose_stream_on_control(
                &mut backend,
                true,
                &mut retirement.stream_drop_pending,
                &mut retirement.stream_disposed,
                |_| Ok(())
            ),
            Ok(true)
        );
        let reaper = retirement.spectrum_reaper.as_ref().unwrap();
        reaper.request_stop();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !reaper.join_finished_for_capture_test() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let (disposed, frame_data) = root.test_spectrum_capture_facts().unwrap().unwrap();
        assert!(
            !disposed,
            "normal buffer destruction does not close captured frame before join"
        );
        assert_eq!(
            full_capture_data - frame_data,
            4 * std::mem::size_of::<[[f32; 2]; 4096]>()
                + 3 * std::mem::size_of::<[f64; 4096]>()
                + std::mem::size_of::<[f64; 2049]>()

                // Actual Project rolling/ordered, independent kernel four,
                // and the inline-only envelope Box; no result Box inferred.
                + std::mem::size_of::<[[f32; 2]; crate::spectrum_worker::MAX_PROJECT_LAYERS * 4096]>()
                + std::mem::size_of::<[[f32; 2]; 4096]>()
                + 3 * std::mem::size_of::<[f64; 4096]>()
                + std::mem::size_of::<[f64; 2049]>()
                + std::mem::size_of::<[Option<crate::spectrum::SpectrumFrame>; crate::spectrum_worker::MAX_PROJECT_LAYERS]>()
        );
        assert_eq!(retirement_tick(retirement), Ok(false));
        assert!(
            retirement
                .spectrum_reaper
                .as_ref()
                .unwrap()
                .test_join_completed(),
            "actual join must be reached while both control endpoints remain live"
        );
        let (disposed, metadata_data) = root.test_spectrum_capture_facts().unwrap().unwrap();
        assert!(disposed);
        assert!(
            frame_data > metadata_data,
            "actual join closes captured frame, not registration metadata"
        );
        assert!(client.owner_ref().is_some());
        assert!(publication.owner_ref().is_some());
        assert!(
            {
                client.refresh_project_spectrum(&mut project_snapshot, true);
                project_snapshot.is_some()
            },
            "actual cached Project Box remains in the live reader AFTER join"
        );
        // The callback's closing fence rejects a late producer before escape.
        let late = crate::prepared::candidate_tests::candidate().unwrap();
        let refused = publication
            .control()
            .unwrap()
            .stage_audio_candidate(late, vec![])
            .err()
            .unwrap();
        assert_eq!(refused.reason, "candidate native custody unavailable");
        assert_eq!(
            refused.owner.phase(),
            hex_engine::AudioCandidateDisposalPhase::Owned
        );
        drop(refused);
        publication.close();
        client.close();
        assert_eq!(
            retirement_tick(retirement),
            Ok(false),
            "the actual external UI result still owns its buffer after join and endpoint retirement"
        );
        drop(project_snapshot.take());
        assert_eq!(retirement_tick(retirement), Ok(true));
        assert_eq!(retirement_tick(retirement), Ok(true));
        // Avoid handing an already completed fixture to the global executor.
        drop(stream_owner.retirement.take());
    }
    #[cfg(feature = "allocation-probe")]
    fn worker_reservation(
        retained: usize,
        peak: usize,
    ) -> crate::AudioCandidatePublicationReservation {
        crate::AudioCandidatePublicationReservation {
            runtime_slots: 3,
            archive_slots: 3,
            member_rows: 128,
            transfer_rows: 128,
            receipt_rows: 3,
            data_rows: 256,
            handle_rows: 256,
            max_retained_data_bytes: retained,
            max_construction_peak_data_bytes: peak,
        }
    }
    #[cfg(feature = "allocation-probe")]
    #[test]
    fn worker_cohort_both_minus_one_refuse_before_any_root_or_analyzer_birth() {
        let _serial = crate::prepared::candidate_tests::serial();
        let (retained, peak) = crate::prepared::native_worker_birth_bound(true).unwrap();
        for limits in [
            worker_reservation(retained - 1, peak),
            worker_reservation(retained, peak - 1),
        ] {
            let lease = Lease::acquire().unwrap();
            let measured = allocation_counter::measure(|| {
                assert!(
                    StreamOwner::construct_prepared_channel_on_control(
                        lease,
                        48000,
                        true,
                        Some(limits)
                    )
                    .is_err()
                );
            });
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        assert!(
            crate::prepared::preflight_native_worker_birth(
                true,
                Some(worker_reservation(retained, peak))
            )
            .is_ok()
        );
        println!(
            "SUPPORTED local complete fixed cohort retained{retained}/peak{peak}; both-minus1 BEFORE Root/queues/analyzer births0alloc; actual external host union still mandatory, not Service admission"
        );
    }
    #[cfg(feature = "allocation-probe")]
    #[test]
    fn constructor_unavailable_requires_actual_precarrier_cleanup_in_same_parent() {
        let _serial = crate::prepared::candidate_tests::serial();
        let lease = Lease::acquire().unwrap();
        let (control, callback) = crate::prepared::channel(48000).unwrap();
        let root = control.native_candidate_root();
        let id = root.root_id();
        let mut owner = StreamOwner::begin_prepared_custody(lease, root);
        let stamp = crate::spectrum_worker::SpectrumStamp {
            target: crate::spectrum_worker::SpectrumTargetId(0),
            generation: 0,
            sample_rate_hz: 0,
            applied_revision: 0,
        };
        let measured = allocation_counter::measure(|| {
            assert!(matches!(
                owner.construct_spectrum_on_control(stamp, true),
                Err(crate::spectrum_worker::SpectrumWorkerError::InvalidSampleRate(0))
            ));
        });
        assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
        let parent = owner.retirement.as_mut().unwrap();
        assert!(parent.spectrum_construction == SpectrumConstruction::Unavailable);
        assert!(parent.spectrum_reaper.is_none());
        assert_eq!(parent.native.as_ref().unwrap().root().root_id(), id);
        assert!(retirement_tick(parent).is_err());
        drop(callback);
        drop(control);
        owner.abort_before_carriers().unwrap();
        assert_eq!(
            retirement_tick(owner.retirement.as_mut().unwrap()),
            Ok(true)
        );
        assert_eq!(
            retirement_tick(owner.retirement.as_mut().unwrap()),
            Ok(true)
        );
        drop(owner.retirement.take());
    }
    #[cfg(feature = "allocation-probe")]
    #[test]
    fn actual_parent_keeps_worker_first_panic_and_remaining_endpoints_without_rejoin() {
        let _serial = crate::prepared::candidate_tests::serial();
        fn fail() {
            std::panic::panic_any(30711_u64);
        }
        let lease = Lease::acquire().unwrap();
        let (control, mut callback) = crate::prepared::channel(48000).unwrap();
        let mut owner = StreamOwner::begin_prepared_custody(lease, control.native_candidate_root());
        owner.retirement.as_mut().unwrap().spectrum_test_entry = Some(fail);
        let stamp = crate::spectrum_worker::SpectrumStamp {
            target: crate::spectrum_worker::SpectrumTargetId(0),
            generation: 0,
            sample_rate_hz: 48000,
            applied_revision: 0,
        };
        let (capture, reader) = owner.construct_spectrum_on_control(stamp, true).unwrap();
        callback
            .attach_constructor_capture_on_control(&mut Some(capture))
            .unwrap();
        let (data, errors, terminals, mut client, mut publication) =
            crate::native_callback_owner::reserve_native_worker(control, callback, Some(reader))
                .unwrap();
        owner
            .install_constructed_native_carriers_on_control(&mut Some(terminals))
            .unwrap();
        drop(data);
        drop(errors);
        let parent = owner.retirement.as_mut().unwrap();
        assert_eq!(retirement_tick(parent), Err(()));
        let first = parent
            .spectrum_reaper
            .as_ref()
            .unwrap()
            .test_first_panic()
            .unwrap();
        assert_eq!(first.downcast_ref::<u64>(), Some(&30711));
        let address = first as *const _ as *const ();
        assert!(client.owner_ref().is_some() && publication.owner_ref().is_some());
        assert_eq!(retirement_tick(parent), Err(()));
        let retained = parent
            .spectrum_reaper
            .as_ref()
            .unwrap()
            .test_first_panic()
            .unwrap();
        assert_eq!(retained as *const _ as *const (), address);
        assert_eq!(retained.downcast_ref::<u64>(), Some(&30711));
        client.close();
        publication.close();
        assert_eq!(retirement_tick(parent), Err(()));
        // Failed fixture teardown, not a successful resource retirement claim.
        drop(owner.retirement.take());
    }
    #[cfg(feature = "allocation-probe")]
    #[test]
    fn foreign_carriers_are_retained_before_failed_association_and_duplicate_keeps_input() {
        let _serial = crate::prepared::candidate_tests::serial();
        let lease = Lease::acquire().unwrap();
        let (control, callback) = crate::prepared::channel(48000).unwrap();
        let mut owner = StreamOwner::begin_prepared_custody(lease, control.native_candidate_root());
        let (foreign_control, foreign_callback) = crate::prepared::channel(48000).unwrap();
        let (data, errors, terminals, mut client, mut publication) =
            crate::native_callback_owner::reserve_native_worker(
                foreign_control,
                foreign_callback,
                None,
            )
            .unwrap();
        let mut incoming = Some(terminals);
        assert!(
            owner
                .install_constructed_native_carriers_on_control(&mut incoming)
                .is_err()
        );
        assert!(incoming.is_none());
        assert!(retirement_tick(owner.retirement.as_mut().unwrap()).is_err());
        let (data2, errors2, terminals2) =
            crate::native_callback_owner::reserve_native(callback, &control.status).unwrap();
        let mut second = Some(terminals2);
        assert!(
            owner
                .install_constructed_native_carriers_on_control(&mut second)
                .is_err()
        );
        assert!(second.is_some());
        publication.close();
        client.close();
        drop(data);
        drop(errors);
        drop(control);
        drop(data2);
        drop(errors2);
        drop(second);
        // Explicit failed-fixture teardown, never successful cleanup proof.
        drop(owner.retirement.take());
    }
    #[cfg(feature = "allocation-probe")]
    #[test]
    fn once_only_capture_installation_keeps_actual_rejected_capture_in_caller() {
        let _serial = crate::prepared::candidate_tests::serial();
        let stamp = crate::spectrum_worker::SpectrumStamp {
            target: crate::spectrum_worker::SpectrumTargetId(0),
            generation: 0,
            sample_rate_hz: 48000,
            applied_revision: 0,
        };
        let (capture, reader, reaper) =
            crate::spectrum_worker::spawn_native_spectrum_worker(stamp).unwrap();
        let (other_capture, other_reader, other_reaper) =
            crate::spectrum_worker::spawn_native_spectrum_worker(stamp).unwrap();
        let expected = other_capture.native_receipt().unwrap().identity;
        let (control, mut callback) = crate::prepared::channel(48000).unwrap();
        let mut first = Some(capture);
        callback
            .attach_constructor_capture_on_control(&mut first)
            .unwrap();
        assert!(first.is_none());
        let mut rejected = Some(other_capture);
        let measured = allocation_counter::measure(|| {
            assert!(
                callback
                    .attach_constructor_capture_on_control(&mut rejected)
                    .is_err()
            );
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(
            rejected
                .as_ref()
                .unwrap()
                .native_receipt()
                .unwrap()
                .identity,
            expected
        );
        drop(rejected);
        drop(other_reader);
        other_reaper.shutdown_and_join().unwrap();
        drop(callback);
        drop(control);
        drop(reader);
        reaper.shutdown_and_join().unwrap();
        // This duplicate-installation fixture exercises actual owners, but does
        // not claim whole-parent admission for its two independent workers.
    }
    #[cfg(feature = "allocation-probe")]
    #[test]
    fn actual_after_spawn_foreign_channel_error_retires_original_parent_once() {
        let _serial = crate::prepared::candidate_tests::serial();
        let lease = Lease::acquire().unwrap();
        let (retained, peak) = crate::prepared::native_worker_birth_bound(true).unwrap();
        let (mut parent, control, callback, reader, error) =
            StreamOwner::construct_prepared_channel_on_control(
                lease,
                48000,
                true,
                Some(worker_reservation(retained, peak)),
            )
            .unwrap();
        assert!(error.is_none());
        let original = control.native_candidate_root().root_id();
        let (foreign_control, foreign_callback) = crate::prepared::channel(48000).unwrap();
        let result =
            crate::native_callback_owner::reserve_native_worker(foreign_control, callback, reader);
        assert!(matches!(
            result,
            Err(hex_engine::Invalid("native worker callback source differs"))
        ));
        drop(control);
        drop(foreign_callback);
        parent.abort_before_carriers().unwrap();
        let custody = parent.retirement.as_mut().unwrap();
        assert_eq!(custody.native.as_ref().unwrap().root().root_id(), original);
        assert_eq!(retirement_tick(custody), Ok(true));
        assert!(
            custody
                .spectrum_reaper
                .as_ref()
                .unwrap()
                .test_join_completed()
        );
        assert_eq!(retirement_tick(custody), Ok(true));
        drop(parent.retirement.take());
    }
    #[test]
    fn worker_open_uses_same_lane_and_preserves_negotiated_completion_and_abandonment() {
        let (sender, receiver) = mpsc::sync_channel::<LifecycleMessage<u32, ()>>(3);
        let state = Arc::new(OpenState::new());
        let mut waiter = enqueue_open_kind(&sender, &state, None, true, None).unwrap();
        assert!(matches!(
            enqueue_open(&sender, &state, None),
            Err(RecoveryOpenError::Busy)
        ));
        let worker = std::thread::spawn(move || {
            run_lifecycle_selected(
                receiver,
                || false,
                |rate, worker, factory_reservation, prepared| {
                    assert!(prepared.is_none());
                    assert!(factory_reservation.is_none());
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

#[cfg(all(test, feature = "allocation-probe"))]
mod factory_route_tests {
    use super::*;
    #[test]
    fn real_worker_open_reservation_survives_queue_and_drives_actual_root_queue_constructor() {
        type Output = (crate::prepared::Control, crate::prepared::Callback);
        let (sender, receiver) = mpsc::sync_channel::<LifecycleMessage<Output, ()>>(3);
        let state = Arc::new(OpenState::new());
        let reservation = crate::AudioCandidatePublicationReservation {
            runtime_slots: 3,
            archive_slots: 3,
            member_rows: 128,
            transfer_rows: 128,
            receipt_rows: 3,
            data_rows: 256,
            handle_rows: 256,
            max_retained_data_bytes: 1 << 20,
            max_construction_peak_data_bytes: 2 << 20,
        };
        let mut waiter =
            enqueue_open_kind(&sender, &state, Some(48000), true, Some(reservation)).unwrap();
        let worker = std::thread::spawn(move || {
            run_lifecycle_selected(
                receiver,
                || false,
                |rate, worker, carried, prepared| {
                    assert!(prepared.is_none());
                    assert!(worker);
                    let carried = carried.expect("actual queued scalar reservation");
                    assert_eq!(carried.max_retained_data_bytes, 1 << 20);
                    assert_eq!(carried.max_construction_peak_data_bytes, 2 << 20);
                    Some(
                        crate::prepared::channel_with_factory_reservation(
                            rate.unwrap(),
                            None,
                            Some(carried),
                        )
                        .map_err(|e| e.to_string()),
                    )
                },
                |_| {},
            )
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let output = loop {
            if let Some(result) = waiter.try_take().unwrap() {
                break result.unwrap();
            }
            assert!(
                std::time::Instant::now() < deadline,
                "local worker must advance"
            );
            std::thread::yield_now();
        };
        assert_eq!(
            output
                .0
                .native_candidate_root()
                .test_prepared_ring_data()
                .unwrap()
                .0,
            9
        );
        drop(output);
        drop(waiter);
        drop(sender);
        worker.join().unwrap();
        eprintln!(
            "SUPPORTED actual existing OpenRequest lane carries both ceilings into real prepared Root+9records birth; no OS device opened, no native/global admission claim"
        );
    }
}
