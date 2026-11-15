//! One requested edit-receipt read in the original completion allocation.
//! No callback lock, shared allocation, native owner or socket reader is added.
use super::*;
use hex_plugin_host::{
    Error,
    protocol::{ParameterEdits, RecoveryEdits},
};
use std::sync::TryLockError;

/// Result of recovering an already-admitted read on the joined original worker.
#[allow(clippy::large_enum_variant)] // One bounded control-side receipt; avoid allocating to transfer it.
pub enum StoppedEditProgress {
    Pending,
    Completed(RecoveryEdits),
}

#[derive(Default)]
pub(super) struct Work {
    result: Option<RecoveryEdits>,
}
impl State {
    pub(super) fn advance_parameter_edits(&mut self, signal: &Signal) -> Result<(), Error> {
        if !signal.edits_requested.load(Acquire) {
            return Ok(());
        }
        if self.edits.result.is_none() {
            let slot = match signal.capture.try_lock() {
                Ok(slot) => slot,
                Err(TryLockError::WouldBlock) => return Ok(()),
                Err(TryLockError::Poisoned(_)) => {
                    return Err(Error::Protocol("native edit mailbox failed"));
                }
            };
            if !slot.edits_pending {
                return Err(Error::Protocol("native edit request changed"));
            }
            let recovery = slot.recovery_edits;
            let flush_idle = slot.flush_idle_edits;
            drop(slot);
            let Some(Processor::Native(instance)) = self.processor.as_mut() else {
                return Err(Error::Protocol("native edit source missing"));
            };
            self.edits.result = Some(if flush_idle {
                instance.service_idle_editor_edits_on_worker()?
            } else if recovery {
                instance.poll_recovery_edits_on_worker()?
            } else {
                RecoveryEdits {
                    edits: instance.poll_parameter_edits_on_worker()?,
                    pending_latency_restart: None,
                }
            });
        }
        let mut slot = match signal.capture.try_lock() {
            Ok(slot) => slot,
            Err(TryLockError::WouldBlock) => return Ok(()),
            Err(TryLockError::Poisoned(_)) => {
                return Err(Error::Protocol("native edit mailbox failed"));
            }
        };
        if !slot.edits_pending || slot.edits_result.is_some() {
            return Err(Error::Protocol("native edit result changed"));
        }
        slot.edits_result = self.edits.result.take();
        slot.edits_pending = false;
        signal.edits_requested.store(false, Release);
        Ok(())
    }
}
impl NativeWorker {
    /// Read one additional bounded batch only after this stopped original
    /// worker has observed its producer's seal. Earlier admitted reads must
    /// be consumed first; a failure remains attached to the worker.
    pub fn poll_sealed_edits_on_control(&mut self) -> Result<RecoveryEdits, &'static str> {
        if self.retirement_requested
            || self.transport_is_live_on_control()
            || !self.retains_original_source_on_control()
            || !self.editor_edits_sealed
        {
            return Err("native editor source is not sealed");
        }
        let slot = self
            .reaper
            .completed
            .capture
            .try_lock()
            .map_err(|error| match error {
                TryLockError::WouldBlock => "native edit observation is busy",
                TryLockError::Poisoned(_) => "native edit mailbox failed",
            })?;
        if slot.edits_pending || slot.edits_result.is_some() {
            return Err("native prior edit read remains");
        }
        if self.reaper.completed.capture_requested.load(Acquire)
            || self.reaper.completed.controller_requested.load(Acquire)
            || self
                .reaper
                .completed
                .parameter_change_requested
                .load(Acquire)
        {
            return Err("native stopped control operation is busy");
        }
        drop(slot);
        let state = self
            .reaper
            .worker
            .as_mut()
            .ok_or("native stopped edit owner missing")?;
        if state.native_error.is_some() || state.edits.result.is_some() {
            return Err("native stopped edit source failed");
        }
        if state.capture.is_in_flight() || state.controller.is_in_flight() {
            return Err("native stopped control operation is busy");
        }
        let Some(Processor::Native(instance)) = state.processor.as_mut() else {
            return Err("native stopped edit source failed");
        };
        match instance.service_idle_editor_edits_on_worker() {
            Ok(result) => Ok(result),
            Err(error) => {
                state.native_error = Some(error);
                Err("native stopped edit read failed")
            }
        }
    }

    /// Recover one previously admitted edit read after transport revocation.
    /// This never starts a fresh read, revives playback, or transfers source
    /// custody. A completed receipt takes precedence over a later worker fault.
    pub fn poll_stopped_edits_on_control(
        &mut self,
        recovery: bool,
    ) -> Result<StoppedEditProgress, &'static str> {
        if self.retirement_requested
            || self.transport_is_live_on_control()
            || !self.retains_original_source_on_control()
        {
            return Err("native stopped edit source differs");
        }
        {
            let slot = self
                .reaper
                .completed
                .capture
                .try_lock()
                .map_err(|error| match error {
                    TryLockError::WouldBlock => "native edit observation is busy",
                    TryLockError::Poisoned(_) => "native edit mailbox failed",
                })?;
            if (slot.edits_pending || slot.edits_result.is_some())
                && slot.recovery_edits != recovery
            {
                return Err("native edit observation mode differs");
            }
        }
        if self
            .reaper
            .join
            .as_ref()
            .is_some_and(|join| !join.is_finished())
        {
            return Err("native preparation is busy");
        }
        let joined = self.reaper.join_on_control();
        let mut slot = self
            .reaper
            .completed
            .capture
            .try_lock()
            .map_err(|error| match error {
                TryLockError::WouldBlock => "native edit observation is busy",
                TryLockError::Poisoned(_) => "native edit mailbox failed",
            })?;
        if (slot.edits_pending || slot.edits_result.is_some()) && slot.recovery_edits != recovery {
            return Err("native edit observation mode differs");
        }
        if let Some(result) = slot.edits_result.take() {
            return Ok(StoppedEditProgress::Completed(result));
        }
        if let Some(state) = self.reaper.worker.as_mut()
            && state.edits.result.is_some()
        {
            if !slot.edits_pending || !self.reaper.completed.edits_requested.load(Acquire) {
                return Err("native edit result custody differs");
            }
            if let Some(result) = state.edits.result.take() {
                slot.edits_pending = false;
                self.reaper.completed.edits_requested.store(false, Release);
                return Ok(StoppedEditProgress::Completed(result));
            }
        }
        if !slot.edits_pending {
            return Err("native stopped edit read was not admitted");
        }
        let failed = joined.is_err()
            || self.reaper.worker.as_ref().is_none_or(|state| {
                state.native_error.is_some()
                    || !matches!(state.processor, Some(Processor::Native(_)))
            });
        if failed {
            slot.edits_pending = false;
            self.reaper.completed.edits_requested.store(false, Release);
            return Err("native stopped edit worker failed");
        }
        drop(slot);
        let state = self
            .reaper
            .worker
            .as_mut()
            .ok_or("native stopped edit owner missing")?;
        if let Err(error) = state.advance_parameter_edits(&self.reaper.completed) {
            state.native_error = Some(error);
            let mut slot =
                self.reaper
                    .completed
                    .capture
                    .try_lock()
                    .map_err(|error| match error {
                        TryLockError::WouldBlock => "native edit observation is busy",
                        TryLockError::Poisoned(_) => "native edit mailbox failed",
                    })?;
            if slot.edits_result.is_some() || state.edits.result.is_some() {
                return Err("native edit observation is busy");
            }
            if !slot.edits_pending || !self.reaper.completed.edits_requested.load(Acquire) {
                return Err("native edit result custody differs");
            }
            slot.edits_pending = false;
            self.reaper.completed.edits_requested.store(false, Release);
            return Err("native stopped edit read failed");
        }
        let mut slot = match self.reaper.completed.capture.try_lock() {
            Ok(slot) => slot,
            Err(TryLockError::WouldBlock) => return Ok(StoppedEditProgress::Pending),
            Err(TryLockError::Poisoned(_)) => return Err("native edit mailbox failed"),
        };
        Ok(match slot.edits_result.take() {
            Some(result) => StoppedEditProgress::Completed(result),
            None => StoppedEditProgress::Pending,
        })
    }

    /// Request/read one bounded ordered callback batch through this exact
    /// original worker. None is pending, Some consumes the batch once. Busy
    /// leaves both source and prior request intact. Receipts are native facts,
    /// not canonical edits; Source revocation/close cancels pending observation.
    pub fn poll_parameter_edits_on_control(
        &mut self,
    ) -> Result<Option<ParameterEdits>, &'static str> {
        Ok(self
            .poll_edits_on_control(false, false)?
            .map(|result| result.edits))
    }
    pub(crate) fn poll_recovery_edits_on_control(
        &mut self,
        flush_idle: bool,
    ) -> Result<Option<RecoveryEdits>, &'static str> {
        self.poll_edits_on_control(true, flush_idle)
    }

    fn poll_edits_on_control(
        &mut self,
        recovery: bool,
        flush_idle: bool,
    ) -> Result<Option<RecoveryEdits>, &'static str> {
        if !self.transport_is_live_on_control() || !self.retains_original_source_on_control() {
            return Err("native edit source is not live");
        }
        let mut slot = self
            .reaper
            .completed
            .capture
            .try_lock()
            .map_err(|_| "native edit observation is busy")?;
        if (slot.edits_pending || slot.edits_result.is_some()) && slot.recovery_edits != recovery {
            return Err("native edit observation mode differs");
        }
        if let Some(result) = slot.edits_result.take() {
            return Ok(Some(result));
        }
        if !slot.edits_pending {
            slot.edits_pending = true;
            slot.recovery_edits = recovery;
            slot.flush_idle_edits = flush_idle;
            self.reaper.completed.edits_requested.store(true, Release);
        }
        Ok(None)
    }
}
