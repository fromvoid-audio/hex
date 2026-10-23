//! Joining control custody for the exact copied endpoint. The processor closure
//! runs only on this worker and must bound its own IPC calls/child supervision.
//! This is not installed-plugin process admission or crash recovery authority.
use hex_lattice_contract::ForeignProcessorBinding as Binding;
use hex_plugin_host::transport::*;
use std::{
    sync::atomic::Ordering::{Acquire, Release},
    sync::{Arc, atomic::AtomicU64},
    thread::{self, JoinHandle},
    time::Duration,
};

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
}
impl State {
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
    completed: Arc<AtomicU64>,
    terminal: crate::native_callback_owner::Terminal<State>,
    worker: Option<Box<State>>,
}
impl Reaper {
    /// Observation after matching reply publication; not a canonical/audio ACK
    /// or authority to consume a reply without its complete identity check.
    pub fn completed_blocks(&self) -> u64 {
        self.completed.load(Acquire)
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
    let completed = Arc::new(AtomicU64::new(0));
    let published = Arc::clone(&completed);
    // The existing once-only guard also covers worker unwinding. Even if the
    // callback carrier has retired first, a worker exit cannot finally free
    // its ring endpoints: the exact terminal remains on joining control.
    let (mut guard, terminal) = crate::native_callback_owner::reserve(State {
        worker,
        processor: Some(processor),
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        native_error: None,
    });
    let join = thread::Builder::new()
        .name("hex-plugin-transport".into())
        .spawn(move || {
            let mut fault = None;
            loop {
                let Some(state) = guard.owner_mut() else {
                    fault = Some(Fault::Protocol);
                    break;
                };
                match state.worker.receive() {
                    Ok(Some(block)) => {
                        let result = state
                            .process(block)
                            .and_then(|reply| state.worker.complete(reply));
                        if let Err(error) = result {
                            state.worker.revoke();
                            if error != Fault::Revoked {
                                fault = Some(error);
                            }
                            break;
                        }
                        published.store(state.worker.completed_blocks(), Release);
                    }
                    Ok(None) => thread::sleep(Duration::from_micros(50)),
                    Err(Fault::Revoked) => break,
                    Err(error) => {
                        fault = Some(error);
                        break;
                    }
                }
            }
            Completion { fault }
        })?;
    Ok(Reaper {
        control,
        join: Some(join),
        result: None,
        completed,
        terminal,
        worker: None,
    })
}

/// Joining native processor/endpoint custody. This is not graph admission.
/// The caller must retain this control owner until the matching callback has
/// returned through its actual terminal guard. Never join/drop it on callback.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub struct NativeWorker {
    reaper: Reaper,
    binding: Binding,
    quantum: u16,
    retirement_requested: bool,
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
        mut instance: hex_plugin_host::native::Prepared,
    ) -> Result<(Self, Callback), hex_plugin_host::Error> {
        instance.validate_worker_transfer_on_control()?;
        let binding = instance.configuration().binding;
        let quantum = instance.configuration().quantum;
        let prefix = instance.completed_blocks_on_control();
        let (control, callback, worker) =
            channel_from_completed_prefix(binding, quantum, prefix)
                .map_err(|fault| hex_plugin_host::Error::Protocol(fault.message()))?;
        let reaper = spawn_processor(control, worker, Processor::Native(Box::new(instance)))
            .map_err(|error| hex_plugin_host::Error::IoAt("native transport worker", error))?;
        Ok((
            Self {
                reaper,
                binding,
                quantum,
                retirement_requested: false,
            },
            callback,
        ))
    }
    pub fn binding(&self) -> Binding {
        self.binding
    }
    pub fn quantum_frames(&self) -> u16 {
        self.quantum
    }
    /// Actual constructor pairing; scalar equality alone is insufficient.
    pub fn owns_callback(&self, callback: &Callback) -> bool {
        callback.matches_control(&self.reaper.control)
    }
    pub fn revoke_on_control(&self) {
        self.reaper.revoke();
    }
    /// Diagnostic publication progress only; never native/root/graph readiness.
    pub fn published_blocks_on_control(&self) -> u64 {
        self.reaper.completed_blocks()
    }
    /// Join, retain the exact old ring endpoints, and consume the returned
    /// native processor into its actual quiescent protocol. Unwind/refusal is
    /// terminal and cannot become a retry. Old callback custody stays separate.
    pub fn quiesce_on_control(
        &mut self,
    ) -> Result<hex_plugin_host::native::Quiescent, hex_plugin_host::Error> {
        if self.retirement_requested {
            return Err(hex_plugin_host::Error::Protocol(
                "native worker retirement is terminal",
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
        let failure = state.native_error.take().or_else(|| {
            joined
                .err()
                .map(|fault| hex_plugin_host::Error::Protocol(fault.message()))
        });
        if let Some(error) = failure {
            if let Some(Processor::Native(instance)) = state.processor.as_mut() {
                if let Err(custody) = instance.cancel_and_reap_on_control() {
                    return Err(hex_plugin_host::Error::Preparation {
                        cause: Box::new(error),
                        exit: None,
                        custody_error: Some(custody.to_string()),
                    });
                }
            }
            return Err(error);
        }
        match state.processor.take() {
            Some(Processor::Native(instance)) => (*instance).quiesce_on_control(),
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
            Err(custody) => {
                let custody_error = Some(custody.to_string());
                Err(Error::Preparation {
                    cause: Box::new(failure.unwrap_or(custody)),
                    exit: None,
                    custody_error,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
