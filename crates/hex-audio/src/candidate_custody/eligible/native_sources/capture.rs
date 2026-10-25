//! Current state stays inside original quiescent Root custody during SDK I/O.
//! A copied envelope grants no resume, endpoint or graph publication authority.
use super::*;

impl Custody {
    pub(in crate::candidate_custody) fn capture_native_source(
        &self,
        binding: Binding,
        is_cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<Option<NativeDeviceState>, &'static str> {
        let mut work = self
            .work
            .try_lock()
            .map_err(|_| "native preparation is busy")?;
        let index = {
            let mut bank = lock(&self.bank);
            if !self.enabled() || bank.closing {
                return Err("native capture is closing");
            }
            let index = bank.sources.index(binding).ok_or("native source differs")?;
            let slot = &bank.sources.slots[index];
            let cell = &work.sources.cells[index];
            if slot.phase != Phase::Accepted
                || slot.retiring
                || slot.starting
                || slot.endpoint_loaned
                || slot.endpoint_return.is_some()
                || slot.cancellation_requested
                || slot.cancellation.is_none()
                || slot.processor.is_some()
                || slot.presentation.is_some()
                || bank.publication.references_native_source(index)
                || cell.pending
                || cell.first_panic.is_some()
                || cell.cause().is_some()
                || cell.cleanup_error.is_some()
                || cell.disposal.is_some()
            {
                return Err("native capture requires original quiescent source custody");
            }
            let Some(Owner::Quiescent(owner)) = cell.owner.as_ref() else {
                return Err("native capture owner is not quiescent");
            };
            let previous = owner.configuration().binding;
            if previous.root != binding.root
                || previous.instance != binding.instance
                || previous.graph > binding.graph
                || previous.epoch >= binding.epoch
                || slot
                    .identity
                    .is_none_or(|identity| identity.device_id() != binding.instance)
                || slot.facts != Some(owner.facts())
                || owner.retained_storage_on_control().is_none()
            {
                return Err("native capture source reservation differs");
            }
            bank.sources.slots[index].starting = true;
            index
        };
        let cell = &mut work.sources.cells[index];
        cell.pending = true;
        // Only Work spans native control I/O. The ordinary bank is free so
        // close may cancel this exact source through its original token.
        let mut captured = None;
        let result = catch_unwind(AssertUnwindSafe(|| match cell.owner.take() {
            Some(Owner::Quiescent(owner)) => {
                match owner.capture_state_retained_while_on_control(is_cancelled) {
                    Ok((owner, state)) => {
                        cell.owner = Some(Owner::Quiescent(owner));
                        captured = Some(state);
                    }
                    Err(owner) => {
                        cell.constructor_refused = true;
                        cell.owner = Some(Owner::Refused(owner));
                    }
                }
            }
            other => cell.owner = other,
        }));
        if let Err(payload) = result {
            cell.first_panic = Some(payload);
            self.fail_native_source(index);
            return Ok(None);
        }
        cell.pending = false;
        let mut bank = lock(&self.bank);
        let closing = bank.closing;
        let slot = &mut bank.sources.slots[index];
        slot.starting = false;
        slot.retiring |= closing || captured.is_none();
        if captured.is_none() {
            slot.phase = Phase::Cleaning;
        }
        drop(bank);
        if closing {
            captured = None;
        }
        Ok(captured)
    }
}

impl CandidateRoot {
    pub(crate) fn capture_native_source_state(
        &self,
        binding: Binding,
        is_cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<Option<NativeDeviceState>, &'static str> {
        self.require_native_source_control_open()?;
        let result = self.0.eligible.capture_native_source(binding, is_cancelled);
        crate::lifecycle::request_candidate_cleanup(self);
        result
    }
}
