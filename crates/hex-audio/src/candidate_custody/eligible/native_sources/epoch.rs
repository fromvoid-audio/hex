//! Renew the same actual source only after original graph/endpoint return.
//! The new scalar binding reserves custody, never a native resume or graph ACK.
use super::*;

impl Custody {
    pub(in crate::candidate_custody) fn quiesce_native_source(
        &self,
        previous: Binding,
        graph: u64,
    ) -> Result<Binding, &'static str> {
        self.quiesce_native_source_between(previous, graph, || {})
    }

    pub(super) fn quiesce_native_source_between(
        &self,
        previous: Binding,
        graph: u64,
        after_reservation: impl FnOnce(),
    ) -> Result<Binding, &'static str> {
        let mut work = self
            .work
            .try_lock()
            .map_err(|_| "native preparation is busy")?;
        let (index, binding) = {
            let mut bank = lock(&self.bank);
            if !self.enabled() || bank.closing || graph == 0 || graph < previous.graph {
                return Err("native epoch is closing or invalid");
            }
            let index = bank
                .sources
                .index(previous)
                .ok_or("native source differs")?;
            let slot = &bank.sources.slots[index];
            let cell = &work.sources.cells[index];
            if slot.phase != Phase::Accepted
                || slot.retiring
                || slot.starting
                || slot.endpoint_loaned
                || slot.endpoint_return.is_some()
                || slot.cancellation_requested
                || slot.cancellation.is_none()
                || bank.publication.references_native_source(index)
                || cell.pending
                || cell.first_panic.is_some()
                || cell.cause().is_some()
                || cell.cleanup_error.is_some()
                || cell.disposal.is_some()
            {
                return Err("native epoch requires original graph and endpoint return");
            }
            let Some(Owner::Effect(effect)) = cell.owner.as_ref() else {
                return Err("native epoch requires original effect custody");
            };
            if effect.binding() != previous
                || slot.identity != Some(effect.identity())
                || slot.processor != Some(effect.prepared())
                || effect.constructor_storage_on_control().is_none()
            {
                return Err("native epoch effect custody differs");
            }
            let binding = Binding {
                graph,
                epoch: bank.sources.next_epoch,
                ..previous
            };
            let next = bank
                .sources
                .next_epoch
                .checked_add(1)
                .ok_or("native preparation epoch exhausted")?;
            if binding.epoch <= previous.epoch {
                return Err("native epoch must advance");
            }
            let slot = &mut bank.sources.slots[index];
            slot.binding = Some(binding);
            slot.starting = true;
            slot.processor = None;
            slot.presentation = None;
            slot.facts = None;
            bank.sources.next_epoch = next;
            (index, binding)
        };
        let cell = &mut work.sources.cells[index];
        cell.pending = true;
        after_reservation();
        let result = catch_unwind(AssertUnwindSafe(|| match cell.owner.take() {
            Some(Owner::Effect(effect)) => match effect.into_quiescent_on_control() {
                Ok(owner) => cell.owner = Some(Owner::Quiescent(owner)),
                Err(owner) => {
                    cell.constructor_refused = true;
                    cell.owner = Some(Owner::WorkerRefused(owner));
                }
            },
            other => cell.owner = other,
        }));
        if let Err(payload) = result {
            cell.first_panic = Some(payload);
            self.fail_native_source(index);
            return Ok(binding);
        }
        cell.pending = false;
        let facts = match cell.owner.as_ref() {
            Some(Owner::Quiescent(owner)) if owner.retained_storage_on_control().is_some() => {
                Some(owner.facts())
            }
            _ => None,
        };
        if facts.is_none() && matches!(cell.owner, Some(Owner::Quiescent(_))) {
            cell.error = Some(hex_plugin_host::Error::Protocol(
                "native epoch source backing unavailable",
            ));
        }
        let mut bank = lock(&self.bank);
        let closing = bank.closing;
        let slot = &mut bank.sources.slots[index];
        slot.starting = false;
        slot.facts = facts;
        slot.retiring |= closing || facts.is_none();
        if facts.is_none() {
            slot.phase = Phase::Cleaning;
        }
        Ok(binding)
    }
}

impl CandidateRoot {
    pub(crate) fn quiesce_native_source_preparation(
        &self,
        previous: Binding,
        graph: u64,
    ) -> Result<Binding, &'static str> {
        self.require_native_source_control_open()?;
        let result = self.0.eligible.quiesce_native_source(previous, graph);
        crate::lifecycle::request_candidate_cleanup(self);
        result
    }
}
