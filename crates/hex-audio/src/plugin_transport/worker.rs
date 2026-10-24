//! Joining control custody for the exact copied endpoint. The processor closure
//! runs only on this worker and must bound its own IPC calls/child supervision.
//! This is not installed-plugin process admission or crash recovery authority.
use super::*;
use std::{
    sync::{Arc, atomic::AtomicU64},
    thread::{self, JoinHandle},
    time::Duration,
};

struct Completion {
    fault: Option<Fault>,
}

/// Kept by the existing publication control owner, never by the callback.
/// Joining and final queue/endpoint destruction occur on that control lane.
pub(crate) struct Reaper {
    control: Control,
    join: Option<JoinHandle<Completion>>,
    result: Option<thread::Result<Completion>>,
    completed: Arc<AtomicU64>,
    terminal: crate::native_callback_owner::Terminal<Worker>,
    worker: Option<Box<Worker>>,
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
    mut process: impl FnMut(Block) -> Result<Block, Fault> + Send + 'static,
) -> std::io::Result<Reaper> {
    let completed = Arc::new(AtomicU64::new(0));
    let published = Arc::clone(&completed);
    // The existing once-only guard also covers worker unwinding. Even if the
    // callback carrier has retired first, a worker exit cannot finally free
    // its ring endpoints: the exact terminal remains on joining control.
    let (mut guard, terminal) = crate::native_callback_owner::reserve(worker);
    let join = thread::Builder::new()
        .name("hex-plugin-transport".into())
        .spawn(move || {
            let mut fault = None;
            loop {
                let Some(worker) = guard.owner_mut() else {
                    fault = Some(Fault::Protocol);
                    break;
                };
                match worker.receive() {
                    Ok(Some(block)) => {
                        let result = process(block).and_then(|reply| worker.complete(reply));
                        if let Err(error) = result {
                            let _ = worker.fail::<()>(error);
                            if error != Fault::Revoked {
                                fault = Some(error);
                            }
                            break;
                        }
                        published.store(worker.sequence, Release);
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

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(reaper.worker.as_ref().unwrap().revoked.load(Acquire));
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
