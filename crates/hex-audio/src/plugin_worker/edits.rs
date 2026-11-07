//! One requested edit-receipt read in the original completion allocation.
//! No callback lock, shared allocation, native owner or socket reader is added.
use super::*;
use hex_plugin_host::{
    Error,
    protocol::{ParameterEdits, RecoveryEdits},
};
use std::sync::TryLockError;
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
