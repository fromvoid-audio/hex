//! Failed construction/epoch keeps the same source/queues in existing control custody.
//! No callback has been loaned by this path; native disposal is independent of
//! final control-side queue/metadata destruction by the enclosing Root owner.
use super::*;
use hex_plugin_host::{
    Error,
    native::{PreparationDisposal, PreparationFailure, Prepared},
};

#[allow(clippy::large_enum_variant)] // Root reserves the whole endpoint packet inline before startup; no late error-path Box.
enum Custody {
    Source(PreparationFailure),
    Pair {
        reaper: Reaper,
        _callback: Callback,
        // None: the unique source is still inside the actual returned State.
        // Some: it moved exactly once into retained native disposal custody.
        native: Option<PreparationFailure>,
    },
}

/// Original startup/epoch refusal and its actual native/endpoint owners. Retain this
/// on control until disposal is observed, then destroy it on control. Timeout
/// and lost custody never return a reusable source or permit a startup retry.
pub struct NativeWorkerStartFailure {
    cause: Error,
    binding: Binding,
    custody: Custody,
}
impl std::fmt::Debug for NativeWorkerStartFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeWorkerStartFailure")
            .field("cause", &self.cause)
            .field("binding", &self.binding)
            .finish_non_exhaustive()
    }
}
impl std::fmt::Display for NativeWorkerStartFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.cause.fmt(f)
    }
}
impl std::error::Error for NativeWorkerStartFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}
impl NativeWorkerStartFailure {
    pub(crate) fn retain_source_on_control(cause: Error, instance: Prepared) -> Self {
        let binding = instance.configuration().binding;
        Self {
            cause,
            binding,
            custody: Custody::Source(instance.cancel_retained_on_control()),
        }
    }
    pub fn cause(&self) -> &Error {
        &self.cause
    }
    /// Move the original diagnostic into enclosing control custody without
    /// formatting or copying its backing. The failure retains all native and
    /// endpoint owners; taking a cause proves no disposal or callback return.
    /// A repeated take returns only the static transferred-cause marker.
    pub fn take_cause_on_control(&mut self) -> Error {
        std::mem::replace(
            &mut self.cause,
            Error::Protocol("native preparation cause transferred"),
        )
    }
    pub fn binding(&self) -> Binding {
        self.binding
    }

    /// One retained native disposal observation. A failed spawn still owns the
    /// original guard's returned State and both paired queues. No endpoint is
    /// extracted or destroyed when this returns an error or a disposal receipt.
    pub fn poll_disposal_on_control(&mut self) -> Result<Option<PreparationDisposal>, Error> {
        match &mut self.custody {
            Custody::Source(native) => native.poll_disposal_on_control(),
            Custody::Pair { reaper, native, .. } => {
                if native.is_none() {
                    // There was no spawned thread; this consumes only the same
                    // guard's actual terminal return. Protocol failure here is
                    // expected for an absent join, not native disposal evidence.
                    let _ = reaper.join_on_control();
                    let state = reaper
                        .worker
                        .as_mut()
                        .ok_or(Error::Protocol("native startup terminal missing"))?;
                    match state.processor.take() {
                        Some(Processor::Native(instance)) => {
                            *native = Some((*instance).cancel_retained_on_control());
                        }
                        other => {
                            state.processor = other;
                            return Err(Error::Protocol("native startup source missing"));
                        }
                    }
                }
                match native.as_mut() {
                    Some(native) => native.poll_disposal_on_control(),
                    None => Err(Error::Protocol("native startup disposal missing")),
                }
            }
        }
    }
    /// Legacy synchronous error path. Drop joins/reaps on this control lane;
    /// returning the original cause is not an observable disposal receipt.
    pub fn finish_on_control(self) -> Error {
        self.cause
    }
}

impl NativeWorker {
    /// Consume the original control and paired callback only after their Root
    /// carrier returned. Success drops old queues on control and returns the
    /// same quiescent child. Failure keeps the actual old pair and native lease
    /// in the existing refusal packet until independently observed disposal.
    /// An admitted live capture reaches its real terminal before SDK quiescence,
    /// including when transport revocation has already refused its caller token.
    /// Stopped graph renewal releases the retired instrument gates in that child.
    #[allow(clippy::result_large_err)]
    pub(crate) fn into_quiescent_on_control(
        mut self,
        callback: Callback,
    ) -> Result<hex_plugin_host::native::Quiescent, NativeWorkerStartFailure> {
        let mismatch = !self.owns_callback(&callback);
        let joined = self.reaper.join_on_control();
        let mut cause = if mismatch {
            Some(Error::Protocol("native quiescence callback differs"))
        } else if self.retirement_requested {
            Some(Error::Protocol("native worker retirement is terminal"))
        } else {
            joined.err().map(|fault| Error::Protocol(fault.message()))
        };
        let mut native = None;
        if let Some(state) = self.reaper.worker.as_mut() {
            if cause.is_none()
                && state.native_error.is_none()
                && let Err(error) = state.drain_capture_before_quiescence(&self.reaper.completed)
            {
                state.native_error = Some(error);
            }
            // Preserve the actual native error ahead of the transport summary.
            if let Some(error) = state.native_error.take() {
                cause = Some(error);
            }
            match state.processor.take() {
                Some(Processor::Native(instance)) => {
                    if cause.is_some() {
                        native = Some((*instance).cancel_retained_on_control());
                    } else {
                        match (*instance).quiesce_for_stopped_restore_on_control() {
                            Ok(source) => return Ok(source),
                            Err(mut failure) => {
                                cause = Some(failure.take_cause_on_control());
                                native = Some(failure);
                            }
                        }
                    }
                }
                other => state.processor = other,
            }
        }
        Err(NativeWorkerStartFailure {
            cause: cause.unwrap_or(Error::Protocol("native quiescence source missing")),
            binding: self.binding,
            custody: Custody::Pair {
                reaper: self.reaper,
                _callback: callback,
                native,
            },
        })
    }

    /// Retain the original native source and any constructed queue pair on
    /// every refusal, including std thread creation failure. Success transfers
    /// the exact live pair; failure cannot be used as a worker or callback.
    #[allow(clippy::result_large_err)] // Return the actual fixed endpoint packet into its pre-reserved Root cell.
    pub fn start_retained_on_control(
        instance: Prepared,
    ) -> Result<(Self, Callback), NativeWorkerStartFailure> {
        Self::start_retained_with(instance, spawn_transport)
    }
    #[allow(clippy::result_large_err)] // Same fixed custody return as the public constructor; no late allocation.
    pub(super) fn start_retained_with(
        mut instance: Prepared,
        spawn: impl FnOnce(
            crate::native_callback_owner::Guard<State>,
            Arc<Signal>,
        ) -> std::io::Result<SpawnedTransport>,
    ) -> Result<(Self, Callback), NativeWorkerStartFailure> {
        if let Err(cause) = instance.validate_worker_transfer_on_control() {
            return Err(NativeWorkerStartFailure::retain_source_on_control(
                cause, instance,
            ));
        }
        let Some(source_storage) = instance.retained_storage_on_control() else {
            return Err(NativeWorkerStartFailure::retain_source_on_control(
                Error::Protocol("native source backing is unavailable"),
                instance,
            ));
        };
        let binding = instance.configuration().binding;
        let quantum = instance.configuration().quantum;
        let prefix = instance.completed_blocks_on_control();
        let channel = if instance.configuration().input_channels == 0 {
            instrument_channel_from_completed_prefix(binding, quantum, prefix)
        } else {
            channel_from_completed_prefix(binding, quantum, prefix)
        };
        let (control, callback, worker) = match channel {
            Ok(pair) => pair,
            Err(fault) => {
                return Err(NativeWorkerStartFailure::retain_source_on_control(
                    Error::Protocol(fault.message()),
                    instance,
                ));
            }
        };
        let cancellation = instance.cancellation_on_control();
        match spawn_processor_with(
            control,
            worker,
            Processor::Native(Box::new(instance)),
            spawn,
        ) {
            Ok(reaper) => Ok((
                Self {
                    reaper,
                    binding,
                    quantum,
                    retirement_requested: false,
                    source_storage: Some(source_storage),
                },
                callback,
            )),
            Err(SpawnFailure { error, reaper }) => {
                cancellation.cancel_on_control();
                Err(NativeWorkerStartFailure {
                    cause: Error::IoAt("native transport worker", error),
                    binding,
                    custody: Custody::Pair {
                        reaper,
                        _callback: callback,
                        native: None,
                    },
                })
            }
        }
    }
}

#[cfg(test)]
mod tests;
