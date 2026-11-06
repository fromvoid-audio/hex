//! Joining control custody for the exact copied endpoint. The processor closure
//! runs only on this worker and must bound its own IPC calls/child supervision.
//! This is not installed-plugin process admission or crash recovery authority.
#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
use hex_lattice_contract::ForeignProcessorBinding as Binding;
use hex_plugin_host::transport::*;
use std::{
    sync::atomic::Ordering::{Acquire, Release},
    sync::{Arc, atomic::AtomicU64},
    thread::{self, JoinHandle},
    time::Duration,
};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod capture;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod controller;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use controller::{NativeWorkerControllerProgress, NativeWorkerControllerQuery};
pub use hex_plugin_host::protocol::{ControllerRequest, ParameterPageQuery, PluginEditorState};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod edits;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod parameter_change;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod parameters;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use parameter_change::{
    NativeWorkerParameterChange, NativeWorkerParameterProgress, ProcessedParameterChange,
};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use parameters::{MAX_OBSERVED_PARAMETERS, NativeParameterValue, NativeParameterValues};
mod storage;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use capture::{NativeWorkerCapture, NativeWorkerCaptureProgress};
/// Non-terminal helper refusal carried by [`NativeWorkerCaptureProgress::Refused`].
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use hex_plugin_host::protocol::StateCaptureRefusal;

// Original completion allocation, shared only by joining control and worker.
// The callback never sees the capture mailbox or takes its mutex.
struct Signal {
    blocks: AtomicU64,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    capture: std::sync::Mutex<capture::Slot>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    capture_requested: std::sync::atomic::AtomicBool,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    edits_requested: std::sync::atomic::AtomicBool,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    controller_requested: std::sync::atomic::AtomicBool,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    failure_diagnostic: std::sync::Mutex<Option<String>>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    parameter_change_requested: std::sync::atomic::AtomicBool,
}
impl Signal {
    fn new() -> Self {
        Self {
            blocks: AtomicU64::new(0),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            capture: std::sync::Mutex::new(capture::Slot::default()),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            capture_requested: std::sync::atomic::AtomicBool::new(false),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            edits_requested: std::sync::atomic::AtomicBool::new(false),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            controller_requested: std::sync::atomic::AtomicBool::new(false),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            failure_diagnostic: std::sync::Mutex::new(None),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            parameter_change_requested: std::sync::atomic::AtomicBool::new(false),
        }
    }
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use storage::{NativeWorkerResourceBound, NativeWorkerStorage};

struct Completion {
    fault: Option<Fault>,
}
// The same terminal protects both endpoints and the processor's actual owner.
// In particular, unwind/exit cannot drop a native child before joining control.
enum Processor {
    Function(Box<dyn FnMut(Block) -> Result<Block, Fault> + Send>),
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    Native(Box<hex_plugin_host::native::Prepared>),
}
struct State {
    worker: Worker,
    processor: Option<Processor>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    native_error: Option<hex_plugin_host::Error>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    capture: capture::Work,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    parameters: parameters::Work,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    edits: edits::Work,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    controller: controller::Work,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    control_turn: u8,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    parameter_change: parameter_change::Work,
}
impl State {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn report_native_failure(
        &self,
        signal: &Signal,
        exchange: &str,
        error: &hex_plugin_host::Error,
    ) {
        use std::io::Write as _;

        let Some(Processor::Native(instance)) = self.processor.as_ref() else {
            return;
        };
        let configuration = instance.configuration();
        let bundle = std::path::Path::new(&configuration.bundle)
            .file_name()
            .unwrap_or_default();
        let message = format!(
            "vst3 worker failed: exchange={exchange} bundle={bundle:?} binding={:?} child={:?} completed_blocks={} cause={error}",
            configuration.binding,
            instance.child_id_on_control(),
            instance.completed_blocks_on_control(),
        );
        if let Ok(mut diagnostic) = signal.failure_diagnostic.lock() {
            *diagnostic = Some(message.clone());
        }
        let _ = writeln!(std::io::stderr().lock(), "{message}");
    }

    fn prepare_transport(&mut self) -> Result<(), Fault> {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            self.prepare_transport_with(|| {
                qos_threads::set_current_thread(qos_threads::Qos::High)
                    .map_err(|error| std::io::Error::from_raw_os_error(error.errno()))
            })
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            Ok(())
        }
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn prepare_transport_with(
        &mut self,
        set_class: impl FnOnce() -> std::io::Result<()>,
    ) -> Result<(), Fault> {
        if matches!(self.processor, Some(Processor::Native(_))) {
            // The copied bridge is latency-sensitive IPC, not a real-time DSP
            // callback. Request macOS user-initiated QoS once on this joining
            // worker; retain its original source on any scheduling refusal.
            if let Err(error) = set_class() {
                self.native_error = Some(hex_plugin_host::Error::IoAt(
                    "native transport scheduling",
                    error,
                ));
                self.worker.revoke();
                return Err(Fault::Protocol);
            }
        }
        Ok(())
    }
    fn process(&mut self, block: Block) -> Result<Block, Fault> {
        match self.processor.as_mut() {
            Some(Processor::Function(process)) => process(block),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            Some(Processor::Native(instance)) => match instance.exchange_on_worker(block) {
                Ok(reply) => Ok(reply),
                Err(error) => {
                    let fault = if matches!(error, hex_plugin_host::Error::Cancelled) {
                        Fault::Revoked
                    } else {
                        Fault::Protocol
                    };
                    self.native_error = Some(error);
                    Err(fault)
                }
            },
            None => Err(Fault::Protocol),
        }
    }
}

/// Kept by the existing publication control owner, never by the callback.
/// Joining and final queue/endpoint destruction occur on that control lane.
pub(crate) struct Reaper {
    control: Control,
    join: Option<JoinHandle<Completion>>,
    result: Option<thread::Result<Completion>>,
    completed: Arc<Signal>,
    terminal: crate::native_callback_owner::Terminal<State>,
    worker: Option<Box<State>>,
    storage: Option<crate::native_callback_owner::ChannelStorage>,
}
impl Reaper {
    /// Observation after matching reply publication; not a canonical/audio ACK
    /// or authority to consume a reply without its complete identity check.
    pub fn completed_blocks(&self) -> u64 {
        self.completed.blocks.load(Acquire)
    }
    pub fn revoke(&self) {
        self.control.revoke();
    }
    #[cfg(test)]
    pub fn is_finished(&self) -> bool {
        self.join.as_ref().is_none_or(JoinHandle::is_finished)
    }
    /// Control-only. Preserve a failed completion and never retry foreign DSP.
    pub fn join_on_control(&mut self) -> Result<(), Fault> {
        self.control.revoke();
        if let Some(join) = self.join.take() {
            self.result = Some(join.join());
        }
        if self.worker.is_none() {
            self.worker = self.terminal.take();
        }
        if self.worker.is_none() {
            return Err(Fault::Protocol);
        }
        match self.result.as_ref() {
            Some(Ok(completion)) => completion.fault.map_or(Ok(()), Err),
            _ => Err(Fault::Protocol),
        }
    }
}
impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.join_on_control();
    }
}

pub(crate) fn spawn(
    control: Control,
    worker: Worker,
    process: impl FnMut(Block) -> Result<Block, Fault> + Send + 'static,
) -> std::io::Result<Reaper> {
    spawn_processor(control, worker, Processor::Function(Box::new(process)))
}
fn spawn_processor(
    control: Control,
    worker: Worker,
    processor: Processor,
) -> std::io::Result<Reaper> {
    spawn_processor_with(control, worker, processor, spawn_transport).map_err(|failure| {
        let SpawnFailure { error, reaper } = failure;
        drop(reaper); // Legacy synchronous control cleanup; not a disposal receipt.
        error
    })
}
struct SpawnFailure {
    error: std::io::Error,
    reaper: Reaper,
}
struct SpawnedTransport {
    join: JoinHandle<Completion>,
    capture_data_bytes: usize,
}
fn spawn_processor_with(
    control: Control,
    worker: Worker,
    processor: Processor,
    spawn: impl FnOnce(
        crate::native_callback_owner::Guard<State>,
        Arc<Signal>,
    ) -> std::io::Result<SpawnedTransport>,
) -> Result<Reaper, SpawnFailure> {
    let completed = Arc::new(Signal::new());
    let published = Arc::clone(&completed);
    let (guard, terminal) = crate::native_callback_owner::reserve(State {
        worker,
        processor: Some(processor),
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        native_error: None,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        capture: capture::Work::default(),
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        parameters: parameters::Work::default(),
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        edits: edits::Work::default(),
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        controller: controller::Work::default(),
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        control_turn: 0,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        parameter_change: parameter_change::Work::default(),
    });
    let storage = storage::reserved_backing(&guard, &completed);
    let join = spawn(guard, published);
    let mut reaper = Reaper {
        control,
        join: None,
        result: None,
        completed,
        terminal,
        worker: None,
        storage,
    };
    match join {
        Ok(spawned) => {
            reaper.storage = reaper.storage.and_then(|mut storage| {
                storage.data_bytes = storage
                    .data_bytes
                    .checked_add(spawned.capture_data_bytes)?
                    .checked_add(std::mem::size_of::<Completion>())?;
                Some(storage)
            });
            reaper.join = Some(spawned.join);
            Ok(reaper)
        }
        Err(error) => {
            reaper.revoke();
            Err(SpawnFailure { error, reaper })
        }
    }
}
fn spawn_transport(
    guard: crate::native_callback_owner::Guard<State>,
    published: Arc<Signal>,
) -> std::io::Result<SpawnedTransport> {
    let run = transport_runner(guard, published);
    let capture_data_bytes = std::mem::size_of_val(&run);
    thread::Builder::new()
        .name("hex-plugin-transport".into())
        .spawn(run)
        .map(|join| SpawnedTransport {
            join,
            capture_data_bytes,
        })
}
fn transport_runner(
    guard: crate::native_callback_owner::Guard<State>,
    published: Arc<Signal>,
) -> impl FnOnce() -> Completion + Send + 'static {
    transport_runner_with_setup(guard, published, State::prepare_transport)
}
fn transport_runner_with_setup(
    mut guard: crate::native_callback_owner::Guard<State>,
    published: Arc<Signal>,
    setup: fn(&mut State) -> Result<(), Fault>,
) -> impl FnOnce() -> Completion + Send + 'static {
    move || {
        let mut fault = None;
        match guard.owner_mut() {
            Some(state) => {
                if let Err(error) = setup(state) {
                    return Completion { fault: Some(error) };
                }
            }
            None => {
                return Completion {
                    fault: Some(Fault::Protocol),
                };
            }
        }
        loop {
            let Some(state) = guard.owner_mut() else {
                fault = Some(Fault::Protocol);
                break;
            };
            match state.worker.receive_with_parameter_capacity() {
                Ok(Some((block, parameter_capacity))) => {
                    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
                    let _ = parameter_capacity;
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    let mut block = block;
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    if let Err(error) =
                        state.stage_parameter_change(&published, &mut block, parameter_capacity)
                    {
                        state.worker.revoke();
                        fault = Some(error);
                        break;
                    }
                    let result = state.process(block).and_then(|reply| {
                        // Native exchange already validated the complete SDK
                        // reply. Retain its private input receipt even if Stop
                        // revokes the transport before callback publication.
                        // The live terminal is still published only below.
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        state.complete_parameter_change(&reply)?;
                        state.worker.complete(reply)?;
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        state.observe_parameter_reply(&reply)?;
                        Ok(())
                    });
                    if let Err(error) = result {
                        state.worker.revoke();
                        if error != Fault::Revoked {
                            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                            if let Some(error) = &state.native_error {
                                state.report_native_failure(&published, "pcm", error);
                            }
                            fault = Some(error);
                        }
                        break;
                    }
                    published
                        .blocks
                        .store(state.worker.completed_blocks(), Release);
                }
                Ok(None) => thread::sleep(Duration::from_micros(50)),
                Err(Fault::Revoked) => break,
                Err(error) => {
                    fault = Some(error);
                    break;
                }
            }
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            if let Err(error) = state.publish_parameter_observations(&published) {
                state.worker.revoke();
                fault = Some(error);
                break;
            }
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            if let Err(error) = state.publish_parameter_change(&published) {
                state.worker.revoke();
                fault = Some(error);
                break;
            }
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            if let Err(error) = state.advance_native_control(&published) {
                let refused = matches!(error, hex_plugin_host::Error::Cancelled);
                state.worker.revoke();
                if !refused {
                    state.report_native_failure(&published, "control", &error);
                    fault = Some(Fault::Protocol);
                }
                state.native_error = Some(error);
                break;
            }
        }
        Completion { fault }
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod start;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use start::NativeWorkerStartFailure;

/// Joining native processor/endpoint custody. This is not graph admission.
/// The caller must retain this control owner until the matching callback has
/// returned through its actual terminal guard. Never join/drop it on callback.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub struct NativeWorker {
    reaper: Reaper,
    binding: Binding,
    quantum: u16,
    retirement_requested: bool,
    source_storage: Option<hex_plugin_host::native::PreparedStorage>,
}
/// Actual native process retirement after joining its transport worker. A
/// processing/stop failure remains explicit even when cancellation reaps the
/// child. This is not callback-carrier return or Root/publication disposal.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[derive(Debug)]
pub struct NativeWorkerRetirement {
    pub native: hex_plugin_host::native::Retirement,
    pub failure: Option<hex_plugin_host::Error>,
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl NativeWorker {
    /// Construct exact paired queues from the consumed actual native lease.
    /// No caller-supplied identity/prefix is treated as source authority.
    pub fn start_on_control(
        instance: hex_plugin_host::native::Prepared,
    ) -> Result<(Self, Callback), hex_plugin_host::Error> {
        Self::start_retained_on_control(instance)
            .map_err(NativeWorkerStartFailure::finish_on_control)
    }
    pub fn binding(&self) -> Binding {
        self.binding
    }
    /// The unique loan control retains this worker while its paired callback
    /// is elsewhere. Source handoff clears the original constructor receipt.
    /// This observation alone is not callback pairing or publication authority.
    pub(crate) fn retains_original_source_on_control(&self) -> bool {
        self.source_storage.is_some() && self.reaper.storage.is_some()
    }
    pub fn quantum_frames(&self) -> u16 {
        self.quantum
    }
    /// Actual constructor pairing; scalar equality alone is insufficient.
    pub fn owns_callback(&self, callback: &Callback) -> bool {
        callback.matches_control(&self.reaper.control)
    }
    /// Actual copied-channel payload only, retained through worker/native
    /// retirement until callback/control destruction. Native instance, worker
    /// terminal and thread resources require their own constructor accounting.
    pub fn transport_storage_on_control(&self, callback: &Callback) -> Option<ConstructorStorage> {
        callback.constructor_storage_on_control(&self.reaper.control)
    }
    /// Original native instance's immutable backing, inspected before transfer
    /// into this worker's actual State. Requires the constructor-paired callback
    /// and a still-retained source; quiescent handoff clears the receipt. This
    /// excludes the source Box, worker/terminal, diagnostics and temporaries and
    /// cannot establish complete resource/publication admission.
    pub fn retained_source_storage_on_control(
        &self,
        callback: &Callback,
    ) -> Option<hex_plugin_host::native::PreparedStorage> {
        self.owns_callback(callback).then_some(())?;
        self.source_storage
    }
    pub fn revoke_on_control(&self) {
        self.reaper.revoke();
    }
    /// Same paired transport's actual revocation state. This grants no source
    /// custody or native ACK; graph admission also validates the retained pair.
    pub(crate) fn transport_is_live_on_control(&self) -> bool {
        !self.retirement_requested && !self.reaper.control.is_revoked()
    }

    pub(crate) fn take_failure_diagnostic_on_control(&self) -> Option<String> {
        if let Some(diagnostic) = self
            .reaper
            .completed
            .failure_diagnostic
            .try_lock()
            .ok()?
            .take()
        {
            return Some(diagnostic);
        }
        self.reaper.control.take_callback_fault().map(|fault| {
            let deadline = self.reaper.control.deadline_miss().map_or_else(
                String::new,
                |miss| format!(
                    " reply_sequence={} worker_phase={:?} quantum={}",
                    miss.reply_sequence, miss.worker_phase, self.quantum
                ),
            );
            format!(
                "vst3 callback transport failed: binding={:?} completed_blocks={} fault={fault:?} cause={}{}",
                self.binding,
                self.reaper.completed_blocks(),
                fault.message(),
                deadline,
            )
        })
    }
    /// Diagnostic publication progress only; never native/root/graph readiness.
    pub fn published_blocks_on_control(&self) -> u64 {
        self.reaper.completed_blocks()
    }
    /// Join, retain the exact old ring endpoints, and consume the returned
    /// native processor into its actual quiescent protocol. An admitted capture
    /// first drains through its existing bounded terminal. Unwind/refusal is
    /// terminal and cannot become a retry. Old callback custody stays separate.
    pub fn quiesce_on_control(
        &mut self,
    ) -> Result<hex_plugin_host::native::Quiescent, hex_plugin_host::Error> {
        if self.retirement_requested {
            return Err(hex_plugin_host::Error::Protocol(
                "native worker retirement is terminal",
            ));
        }
        if self
            .parameter_change_pending_on_control()
            .map_err(hex_plugin_host::Error::Protocol)?
        {
            return Err(hex_plugin_host::Error::Protocol(
                "native parameter input terminal is pending",
            ));
        }
        let joined = self.reaper.join_on_control();
        let state = self
            .reaper
            .worker
            .as_mut()
            .ok_or(hex_plugin_host::Error::Protocol(
                "native worker terminal missing",
            ))?;
        if joined.is_ok()
            && state.native_error.is_none()
            && let Err(error) = state.drain_capture_before_quiescence(&self.reaper.completed)
        {
            state.native_error = Some(error);
        }
        let failure = state.native_error.take().or_else(|| {
            joined
                .err()
                .map(|fault| hex_plugin_host::Error::Protocol(fault.message()))
        });
        if let Some(error) = failure {
            if let Some(Processor::Native(instance)) = state.processor.as_mut()
                && let Err(custody) = instance.cancel_and_reap_on_control()
            {
                return Err(hex_plugin_host::Error::Preparation {
                    cause: Box::new(error),
                    exit: None,
                    custody_error: Some(Box::new(custody)),
                });
            }
            return Err(error);
        }
        match state.processor.take() {
            Some(Processor::Native(instance)) => {
                self.source_storage = None;
                (*instance).quiesce_on_control()
            }
            other => {
                state.processor = other;
                Err(hex_plugin_host::Error::Protocol(
                    "native worker already returned its source",
                ))
            }
        }
    }

    /// Join and retire this exact native source without returning a reusable
    /// lease. Normal completion requires the SDK terminal; processing/stop
    /// failure cancels and observes this same child's actual reap. A refusal
    /// must remain retained as failed custody, never retried or relabelled.
    /// The caller still retains this worker/rings until its matching callback
    /// carrier actually returns; this result alone cannot release a Root slot.
    pub fn retire_on_control(&mut self) -> Result<NativeWorkerRetirement, hex_plugin_host::Error> {
        use hex_plugin_host::Error;
        if self.retirement_requested {
            return Err(Error::Protocol(
                "native worker retirement already requested",
            ));
        }
        self.retirement_requested = true;
        let joined = self.reaper.join_on_control();
        let state = self
            .reaper
            .worker
            .as_mut()
            .ok_or(Error::Protocol("native worker terminal missing"))?;
        let mut failure = state
            .native_error
            .take()
            .or_else(|| joined.err().map(|fault| Error::Protocol(fault.message())));
        if failure.is_none() && (state.capture.is_in_flight() || state.controller.is_in_flight()) {
            // Retirement explicitly cancels the outstanding original request;
            // never send normal SDK Stop while the main lane owns it.
            failure = Some(Error::Cancelled);
        }
        let Some(Processor::Native(instance)) = state.processor.as_mut() else {
            return Err(Error::Protocol("native worker already returned its source"));
        };
        if failure.is_none() {
            match instance.stop_and_reap_on_control() {
                Ok(native) => return Ok(NativeWorkerRetirement { native, failure }),
                Err(error) => failure = Some(error),
            }
        }
        match instance.cancel_and_reap_on_control() {
            Ok(native) => Ok(NativeWorkerRetirement { native, failure }),
            Err(custody) => match failure {
                Some(cause) => Err(Error::Preparation {
                    cause: Box::new(cause),
                    exit: None,
                    custody_error: Some(Box::new(custody)),
                }),
                None => Err(custody),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reaper_constructor_lists_boxes_terminal_counter_and_actual_thread_capture() {
        let captured = [42_u8; 173];
        let process = move |block| {
            std::hint::black_box(&captured);
            Ok(block)
        };
        let processor_bytes = std::mem::size_of_val(&process);
        let (control, callback, worker) = channel(
            Binding {
                root: 1,
                instance: 2,
                graph: 3,
                epoch: 4,
            },
            8,
        )
        .unwrap();
        let capture_bytes = std::cell::Cell::new(0);
        let result = spawn_processor_with(
            control,
            worker,
            Processor::Function(Box::new(process)),
            |guard, published| {
                let spawned = spawn_transport(guard, published)?;
                capture_bytes.set(spawned.capture_data_bytes);
                Ok(spawned)
            },
        );
        let mut reaper = match result {
            Ok(reaper) => reaper,
            Err(failure) => panic!("original worker spawn: {}", failure.error),
        };
        let storage = reaper.storage.unwrap();
        assert!(
            capture_bytes.get()
                >= std::mem::size_of::<crate::native_callback_owner::Guard<State>>()
                    + std::mem::size_of::<Arc<Signal>>()
        );
        assert_eq!(
            storage.data_bytes,
            std::mem::size_of::<State>()
                + processor_bytes
                + std::mem::size_of::<std::sync::OnceLock<std::sync::Mutex<Box<State>>>>()
                + std::mem::size_of::<Signal>()
                + capture_bytes.get()
                + std::mem::size_of::<Completion>()
        );
        assert_eq!((storage.shared_groups, storage.shared_handles), (2, 4));
        assert_eq!(reaper.join_on_control(), Ok(()));
        assert_eq!(
            reaper.storage.unwrap().data_bytes,
            storage.data_bytes,
            "constructor charge persists through actual worker return until control disposal"
        );
        drop((callback, reaper));
    }
    #[test]
    fn spawn_refusal_retains_the_actual_processor_until_control_disposes_the_return() {
        struct Capture(Arc<AtomicU64>);
        impl Drop for Capture {
            fn drop(&mut self) {
                self.0.fetch_add(1, Release);
            }
        }
        let dropped = Arc::new(AtomicU64::new(0));
        let capture = Capture(Arc::clone(&dropped));
        let (control, callback, worker) = channel(
            Binding {
                root: 1,
                instance: 2,
                graph: 3,
                epoch: 4,
            },
            8,
        )
        .unwrap();
        let returned = spawn_processor_with(
            control,
            worker,
            Processor::Function(Box::new(move |block| {
                let _capture = &capture;
                Ok(block)
            })),
            |guard, _| {
                drop(guard); // Same closure-ownership return as std thread spawn refusal.
                Err(std::io::Error::other("original thread creation refusal"))
            },
        );
        assert!(returned.is_err());
        assert_eq!(
            dropped.load(Acquire),
            0,
            "failed spawn must return original processor custody"
        );
        drop((returned, callback));
        assert_eq!(dropped.load(Acquire), 1);
    }
    #[test]
    fn processor_capture_returns_to_control_after_exit_or_unwind() {
        struct Capture(Arc<std::sync::Mutex<Option<thread::ThreadId>>>);
        impl Drop for Capture {
            fn drop(&mut self) {
                *self.0.lock().unwrap() = Some(thread::current().id());
            }
        }
        for unwind in [false, true] {
            let destroyed = Arc::new(std::sync::Mutex::new(None));
            let capture = Capture(Arc::clone(&destroyed));
            let (control, mut callback, worker) = channel(
                Binding {
                    root: 1,
                    instance: 2,
                    graph: 3,
                    epoch: 4,
                },
                8,
            )
            .unwrap();
            let mut reaper = spawn(control, worker, move |_| {
                let _retained = &capture;
                assert!(!unwind, "original processor unwind fixture");
                Err(Fault::InvalidSamples)
            })
            .unwrap();
            callback
                .stage(&[[0.5; 2]; 8], &[], &mut [[0.0; 2]; 8])
                .unwrap();
            callback.finish(true).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while !reaper.is_finished() {
                assert!(std::time::Instant::now() < deadline);
                thread::yield_now();
            }
            assert_eq!(
                reaper.join_on_control(),
                Err(if unwind {
                    Fault::Protocol
                } else {
                    Fault::InvalidSamples
                })
            );
            assert_eq!(
                *destroyed.lock().unwrap(),
                None,
                "join retains processor custody"
            );
            drop(callback);
            drop(reaper);
            assert_eq!(*destroyed.lock().unwrap(), Some(thread::current().id()));
        }
    }
    #[test]
    fn transport_review_backend_cancel_revokes_before_next_audio_request() {
        let binding = Binding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        };
        let (control, mut callback, worker) = channel(binding, 8).unwrap();
        let mut reaper = spawn(control, worker, |_| Err(Fault::Revoked)).unwrap();
        callback
            .stage(&[[0.5; 2]; 8], &[], &mut [[0.0; 2]; 8])
            .unwrap();
        callback.finish(true).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !reaper.is_finished() {
            assert!(std::time::Instant::now() < deadline);
            thread::yield_now();
        }
        assert!(
            matches!(
                callback.stage(&[[0.5; 2]; 8], &[], &mut [[0.0; 2]; 8]),
                Err(Fault::Revoked)
            ),
            "a backend cancellation must publish revocation before worker exit"
        );
        assert_eq!(reaper.join_on_control(), Ok(()));
    }
    #[test]
    fn joining_control_retains_endpoints_after_real_worker_exit_and_refusal() {
        let binding = Binding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        };
        let (control, mut callback, worker) = channel(binding, 8).unwrap();
        let mut reaper = spawn(control, worker, |_| Err(Fault::InvalidSamples)).unwrap();
        callback
            .stage(&[[0.5; 2]; 8], &[], &mut [[0.0; 2]; 8])
            .unwrap();
        callback.finish(true).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !reaper.is_finished() {
            assert!(std::time::Instant::now() < deadline);
            thread::yield_now();
        }
        assert_eq!(reaper.join_on_control(), Err(Fault::InvalidSamples));
        assert_eq!(reaper.join_on_control(), Err(Fault::InvalidSamples));
        assert!(matches!(
            reaper.worker.as_mut().unwrap().worker.receive(),
            Err(Fault::Revoked)
        ));
        drop((callback, reaper));
    }

    #[test]
    fn worker_unwind_returns_the_original_guarded_endpoint_after_callback_retirement() {
        let binding = Binding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        };
        let (control, mut callback, worker) = channel(binding, 8).unwrap();
        let gate = Arc::new(std::sync::Barrier::new(2));
        let processing = Arc::clone(&gate);
        let mut reaper = spawn(control, worker, move |_| {
            processing.wait();
            processing.wait();
            panic!("original worker unwind fixture");
        })
        .unwrap();
        callback
            .stage(&[[0.5; 2]; 8], &[], &mut [[0.0; 2]; 8])
            .unwrap();
        callback.finish(true).unwrap();
        gate.wait();
        drop(callback); // worker endpoint is now the only owner of both rings
        gate.wait();
        assert_eq!(reaper.join_on_control(), Err(Fault::Protocol));
        assert!(reaper.result.as_ref().unwrap().is_err());
        assert!(
            reaper.worker.is_some(),
            "the exact guard returned the endpoint Box despite unwind"
        );
        assert_eq!(reaper.join_on_control(), Err(Fault::Protocol));
    }
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
pub(crate) mod native_tests;
