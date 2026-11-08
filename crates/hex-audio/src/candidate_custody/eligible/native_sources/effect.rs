//! Actual unsplit effect construction stays in the reserved Root Work cell.
//! Callback loans/resource sealing remain separate from constructor success.
use super::*;
use crate::plugin_effects::{AuthoredSelection, NativeEffect};

impl Custody {
    #[cfg(test)]
    pub(in crate::candidate_custody) fn start_native_effect(
        &self,
        binding: Binding,
    ) -> Result<(), &'static str> {
        self.start_native_effect_with_selection(binding, None)
    }

    pub(in crate::candidate_custody) fn start_native_effect_with_selection(
        &self,
        binding: Binding,
        authored: Option<&AuthoredSelection>,
    ) -> Result<(), &'static str> {
        self.start_native_effect_between(binding, authored, || {})
    }
    pub(super) fn start_native_effect_between(
        &self,
        binding: Binding,
        authored: Option<&AuthoredSelection>,
        after_construction: impl FnOnce(),
    ) -> Result<(), &'static str> {
        let Ok(mut work) = self.work.try_lock() else {
            return Err("native preparation is busy");
        };
        let (index, identity, max_data_bytes) = {
            let mut bank = lock(&self.bank);
            let index = bank.sources.index(binding).ok_or("native source differs")?;
            let max_data_bytes = bank
                .sources
                .limits
                .ok_or("native source data is not reserved")?
                .max_retained_data_bytes;
            let slot = &mut bank.sources.slots[index];
            if slot.phase != Phase::Accepted
                || slot.retiring
                || slot.starting
                || slot.processor.is_some()
                || !matches!(work.sources.cells[index].owner, Some(Owner::Prepared(_)))
            {
                return Err("native source is not awaiting effect construction");
            }
            let identity = slot.identity.ok_or("native source identity missing")?;
            slot.starting = true;
            (index, identity, max_data_bytes)
        };
        let cell = &mut work.sources.cells[index];
        cell.pending = true;
        let result = catch_unwind(AssertUnwindSafe(|| match cell.owner.take() {
            Some(Owner::Prepared(owner)) => {
                match NativeEffect::prepare_with_renewal_on_control(
                    identity,
                    owner,
                    max_data_bytes,
                    authored.map(AuthoredSelection::parameters),
                    cell.renewal.take(),
                ) {
                    Ok(owner) => cell.owner = Some(Owner::Effect(owner)),
                    Err(owner) => {
                        cell.constructor_refused = true;
                        cell.owner = Some(Owner::WorkerRefused(owner));
                    }
                }
            }
            other => cell.owner = other,
        }));
        if let Err(payload) = result {
            cell.first_panic = Some(payload);
            self.fail_native_source(index);
            return Ok(());
        }
        after_construction();
        cell.pending = false;
        let prepared = match cell.owner.as_ref() {
            Some(Owner::Effect(owner)) => Some(owner.prepared()),
            _ => None,
        };
        let mut bank = lock(&self.bank);
        let closing = bank.closing;
        let slot = &mut bank.sources.slots[index];
        slot.starting = false;
        slot.processor = prepared;
        slot.retiring |= closing || prepared.is_none();
        if prepared.is_none() {
            slot.phase = Phase::Cleaning;
        }
        Ok(()) // Retained custody, including constructor refusal/late close.
    }
}

impl CandidateRoot {
    #[cfg(test)]
    pub(crate) fn start_native_effect(&self, binding: Binding) -> Result<(), &'static str> {
        self.start_native_effect_with_authored_selection(binding, None)
    }

    pub(crate) fn start_native_effect_with_authored_selection(
        &self,
        binding: Binding,
        authored: Option<&AuthoredSelection>,
    ) -> Result<(), &'static str> {
        self.require_native_source_control_open()?;
        let result = self
            .0
            .eligible
            .start_native_effect_with_selection(binding, authored);
        crate::lifecycle::request_candidate_cleanup(self);
        result
    }
}
