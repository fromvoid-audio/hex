//! One capture operation in the original completion allocation. Worker and
//! joining control use try_lock only; no lock spans native socket/SDK I/O.
//! Two fixed payload buffers (worker staging and shared result) are charged by
//! actual State/Signal constructor sizes. No additional shared allocation or
//! native owner exists. Host protocol temporaries remain separately bounded.
use super::*;
use hex_lattice_contract::{MAX_NATIVE_DEVICE_STATE_BYTES, NativeDeviceState};
use hex_plugin_host::Error;
use hex_plugin_host::protocol::StateCaptureRefusal;
use std::sync::{TryLockError, Weak};
use std::time::Instant;
mod stopped;
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Live,
    Stopped,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Requested,
    Complete,
}

#[derive(Clone, Copy)]
struct Payload {
    bytes: [u8; MAX_NATIVE_DEVICE_STATE_BYTES],
    len: usize,
}
impl Default for Payload {
    fn default() -> Self {
        Self {
            bytes: [0; MAX_NATIVE_DEVICE_STATE_BYTES],
            len: 0,
        }
    }
}
pub(super) struct Slot {
    pub(super) controller: super::controller::Slot,
    pub(super) parameters: super::parameters::NativeParameterValues,
    pub(super) edits_pending: bool,
    pub(super) edits_result: Option<hex_plugin_host::protocol::ParameterEdits>,
    ordinal: u64,
    phase: Phase,
    mode: Mode,
    payload: Payload,
    /// Set with `Phase::Complete` when the helper refused this operation.
    refusal: Option<StateCaptureRefusal>,
}
impl Default for Slot {
    fn default() -> Self {
        Self {
            controller: super::controller::Slot::default(),
            parameters: super::parameters::NativeParameterValues::default(),
            edits_pending: false,
            edits_result: None,
            ordinal: 0,
            phase: Phase::Idle,
            mode: Mode::Live,
            payload: Payload::default(),
            refusal: None,
        }
    }
}
impl Slot {
    /// Publish the worker's terminal for the requested `ordinal`: captured
    /// bytes or a non-terminal helper refusal. Any other slot state is a
    /// protocol fault.
    fn complete(
        &mut self,
        ordinal: u64,
        outcome: Result<Payload, StateCaptureRefusal>,
    ) -> Result<(), Error> {
        if self.ordinal != ordinal || self.phase != Phase::Requested {
            return Err(Error::Protocol("native capture operation changed"));
        }
        match outcome {
            Ok(payload) => {
                self.payload = payload;
                self.refusal = None;
            }
            Err(refusal) => self.refusal = Some(refusal),
        }
        self.phase = Phase::Complete;
        Ok(())
    }

    /// Consume a completed terminal and free the mailbox, or `None` while the
    /// operation is still pending.
    fn take_terminal(&mut self) -> Option<Result<Payload, StateCaptureRefusal>> {
        if self.phase != Phase::Complete {
            return None;
        }
        self.phase = Phase::Idle;
        Some(match self.refusal.take() {
            Some(refusal) => Err(refusal),
            None => Ok(self.payload),
        })
    }
}
pub(super) struct Work {
    ordinal: Option<u64>,
    result: Option<Result<Payload, StateCaptureRefusal>>,
    poll_after: Instant,
}
impl Default for Work {
    fn default() -> Self {
        Self {
            ordinal: None,
            result: None,
            poll_after: Instant::now(),
        }
    }
}
impl Work {
    pub(super) fn between_polls(&self) -> bool {
        self.is_in_flight() && Instant::now() < self.poll_after
    }
    pub(super) fn is_in_flight(&self) -> bool {
        self.ordinal.is_some() && self.result.is_none()
    }
}

/// Correlation with this exact joining worker's original shared cell, binding
/// and operation. The weak anchor owns no processor, endpoint or native lease.
/// Caller-owned token storage/weak alias is separate from constructor handles;
/// retain at most the one current operation and retire it at its exact terminal.
pub struct NativeWorkerCapture {
    source: Weak<Signal>,
    binding: Binding,
    ordinal: u64,
    mode: Mode,
}
/// Data/progress only, never a canonical edit, save or graph acknowledgement.
#[derive(Debug)]
pub enum NativeWorkerCaptureProgress {
    Pending,
    Captured(NativeDeviceState),
    /// Original transport is revoked; its error/native lease remains in the
    /// same worker terminal for joining control and independent disposal.
    Failed,
    /// The helper refused only this capture (state too large, or edits kept
    /// arriving). The worker and its transport keep running; the mailbox is
    /// free and a later capture may begin.
    Refused(StateCaptureRefusal),
}

impl NativeWorker {
    /// Reserve one current-state capture while the original callback may be
    /// loaned. Live requests do no SDK/socket work here. The original worker
    /// starts/polls them between PCM exchanges, at most one control exchange per
    /// iteration and one status poll per millisecond. Full/contended admission
    /// refuses without disturbing an existing capture or callback. After normal
    /// transport revocation, joining control first observes the original thread's
    /// terminal and services that same retained State/channel. This does not
    /// revive transport, return a callback or grant graph publication.
    pub fn begin_state_capture_on_control(&mut self) -> Result<NativeWorkerCapture, &'static str> {
        if self.retirement_requested || !self.retains_original_source_on_control() {
            return Err("native capture source is not live");
        }
        let mode = if self.transport_is_live_on_control() {
            Mode::Live
        } else {
            self.prepare_stopped_capture()?;
            Mode::Stopped
        };
        let mut slot = self
            .reaper
            .completed
            .capture
            .try_lock()
            .map_err(|_| "native capture mailbox is busy")?;
        if slot.phase != Phase::Idle {
            return Err("native capture is already pending");
        }
        let ordinal = slot
            .ordinal
            .checked_add(1)
            .ok_or("native capture ordinal exhausted")?;
        slot.ordinal = ordinal;
        slot.phase = Phase::Requested;
        slot.mode = mode;
        self.reaper.completed.capture_requested.store(true, Release);
        Ok(NativeWorkerCapture {
            source: Arc::downgrade(&self.reaper.completed),
            binding: self.binding,
            ordinal,
            mode,
        })
    }

    /// Poll exactly the admitted operation. A stale/foreign/consumed token
    /// refuses without consuming the current result. Callback revocation wins
    /// over a live token's completed result. A separately admitted stopped token
    /// copies data only after the healthy original worker returns; neither mode
    /// resurrects transport or a returned source.
    /// Payload reconstruction runs on joining control; host gains/asset state
    /// are not captured and remain the canonical snapshot owner's responsibility.
    /// A completed matching result is consumed even if caller allocation fails;
    /// that refusal cannot occupy the original mailbox indefinitely.
    /// Pending also includes shared-slot contention before its exact state check.
    pub fn poll_state_capture_on_control(
        &mut self,
        request: &NativeWorkerCapture,
    ) -> Result<NativeWorkerCaptureProgress, &'static str> {
        self.poll_state_capture_using(request, |bytes| {
            let mut payload = Vec::new();
            payload
                .try_reserve_exact(bytes.len())
                .map_err(|_| "native capture state allocation failed")?;
            payload.extend_from_slice(bytes);
            NativeDeviceState::new(
                hex_plugin_state::TYPE_ID,
                hex_plugin_state::STATE_VERSION,
                payload,
            )
            .map_err(|_| "native capture state is invalid")
        })
    }

    fn poll_state_capture_using(
        &mut self,
        request: &NativeWorkerCapture,
        materialize: impl FnOnce(&[u8]) -> Result<NativeDeviceState, &'static str>,
    ) -> Result<NativeWorkerCaptureProgress, &'static str> {
        if request.binding != self.binding
            || request.source.as_ptr() != Arc::as_ptr(&self.reaper.completed)
        {
            return Err("native capture source differs");
        }
        if request.mode == Mode::Stopped {
            // Validate before servicing: an old/foreign token must not advance
            // another operation. The guard never spans native channel I/O.
            match self.reaper.completed.capture.try_lock() {
                Ok(slot)
                    if slot.ordinal == request.ordinal
                        && slot.phase != Phase::Idle
                        && slot.mode == request.mode => {}
                Ok(_) => return Err("native capture operation differs"),
                Err(TryLockError::WouldBlock) => return Ok(NativeWorkerCaptureProgress::Pending),
                Err(TryLockError::Poisoned(_)) => return Err("native capture mailbox failed"),
            }
            match self.service_stopped_capture() {
                Ok(()) => {}
                Err("native preparation is busy") => {
                    return Ok(NativeWorkerCaptureProgress::Pending);
                }
                Err(_) => return Ok(NativeWorkerCaptureProgress::Failed),
            }
        }
        let mut slot = match self.reaper.completed.capture.try_lock() {
            Ok(slot) => slot,
            Err(TryLockError::WouldBlock) => return Ok(NativeWorkerCaptureProgress::Pending),
            Err(TryLockError::Poisoned(_)) => return Err("native capture mailbox failed"),
        };
        if slot.ordinal != request.ordinal || slot.phase == Phase::Idle || slot.mode != request.mode
        {
            return Err("native capture operation differs");
        }
        if request.mode == Mode::Live && !self.transport_is_live_on_control() {
            return Ok(NativeWorkerCaptureProgress::Failed);
        }
        // The matching operation is terminal even if constructing the caller's
        // owned envelope fails. Release its fixed slot before fallible allocation
        // so a refused save cannot strand the original worker's only mailbox.
        let Some(terminal) = slot.take_terminal() else {
            return Ok(NativeWorkerCaptureProgress::Pending);
        };
        drop(slot);
        match terminal {
            Ok(completed) => materialize(&completed.bytes[..completed.len])
                .map(NativeWorkerCaptureProgress::Captured),
            Err(refusal) => Ok(NativeWorkerCaptureProgress::Refused(refusal)),
        }
    }
}

impl State {
    pub(super) fn advance_capture(&mut self, signal: &Signal) -> Result<(), Error> {
        let Some(Processor::Native(instance)) = self.processor.as_mut() else {
            return Ok(());
        };
        if self.capture.ordinal.is_none() {
            if !signal.capture_requested.load(Acquire) {
                return Ok(());
            }
            let slot = match signal.capture.try_lock() {
                Ok(slot) => slot,
                Err(TryLockError::WouldBlock) => return Ok(()),
                Err(TryLockError::Poisoned(_)) => {
                    return Err(Error::Protocol("native capture mailbox failed"));
                }
            };
            if slot.phase != Phase::Requested {
                return Ok(());
            }
            let ordinal = slot.ordinal;
            drop(slot);
            // A refusal retires only this operation; the lease keeps running.
            match instance.begin_state_capture_on_worker() {
                Ok(()) => {}
                Err(Error::CaptureRefused(refusal)) => self.capture.result = Some(Err(refusal)),
                Err(error) => return Err(error),
            }
            self.capture.ordinal = Some(ordinal);
        } else if self.capture.result.is_none() {
            if Instant::now() < self.capture.poll_after {
                return Ok(());
            }
            match instance.poll_state_capture_on_worker() {
                Ok(Some(state)) => {
                    let mut payload = Payload::default();
                    if state.type_id() != hex_plugin_state::TYPE_ID
                        || state.state_version() != hex_plugin_state::STATE_VERSION
                        || state.payload().len() > payload.bytes.len()
                    {
                        return Err(Error::Protocol("native captured state profile differs"));
                    }
                    payload.len = state.payload().len();
                    payload.bytes[..payload.len].copy_from_slice(state.payload());
                    self.capture.result = Some(Ok(payload));
                }
                Ok(None) => {}
                Err(Error::CaptureRefused(refusal)) => self.capture.result = Some(Err(refusal)),
                Err(error) => return Err(error),
            }
        }
        self.capture.poll_after = Instant::now()
            .checked_add(Duration::from_millis(1))
            .ok_or(Error::Protocol("native capture poll time exhausted"))?;
        if let (Some(outcome), Some(ordinal)) = (self.capture.result, self.capture.ordinal) {
            let mut slot = match signal.capture.try_lock() {
                Ok(slot) => slot,
                Err(TryLockError::WouldBlock) => return Ok(()),
                Err(TryLockError::Poisoned(_)) => {
                    return Err(Error::Protocol("native capture mailbox failed"));
                }
            };
            slot.complete(ordinal, outcome)?;
            signal.capture_requested.store(false, Release);
            self.capture.result = None;
            self.capture.ordinal = None;
        }
        Ok(())
    }
}
