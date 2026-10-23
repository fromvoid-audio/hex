//! Final source ACK stays in the same reserved Root cell. Neither preparation
//! completion nor this read-only state copy seals endpoints for publication.
use super::*;
use hex_plugin_host::protocol::PresentationLatency;

impl Custody {
    pub(in crate::candidate_custody) fn complete_native_source(
        &self,
        binding: Binding,
        presentation: Option<PresentationLatency>,
    ) -> Result<(), &'static str> {
        self.complete_native_source_between(binding, presentation, || {}, || {})
    }

    pub(super) fn complete_native_source_between(
        &self,
        binding: Binding,
        presentation: Option<PresentationLatency>,
        after_reservation: impl FnOnce(),
        after_completion: impl FnOnce(),
    ) -> Result<(), &'static str> {
        let Ok(mut work) = self.work.try_lock() else {
            return Err("native preparation is busy");
        };
        let index = {
            let mut bank = lock(&self.bank);
            let index = bank.sources.index(binding).ok_or("native source differs")?;
            let slot = &mut bank.sources.slots[index];
            if slot.phase != Phase::Accepted
                || slot.retiring
                || slot.starting
                || slot.presentation.is_some()
            {
                return Err("native source is not awaiting completion");
            }
            if !matches!(work.sources.cells[index].owner, Some(Owner::Pending(_))) {
                return Err("native preparation owner differs");
            }
            slot.starting = true;
            index
        };
        let cell = &mut work.sources.cells[index];
        // Set the persistent pending marker before extracting the unique owner.
        // Only the dedicated Work lock spans native I/O; close can still cancel
        // the exact source through its separately retained token.
        cell.pending = true;
        after_reservation();
        let result = catch_unwind(AssertUnwindSafe(|| match cell.owner.take() {
            Some(Owner::Pending(owner)) => match owner.complete_retained_on_control(presentation) {
                Ok(owner) => cell.owner = Some(Owner::Prepared(owner)),
                Err(owner) => {
                    cell.error = Some(owner.to_string());
                    cell.owner = Some(Owner::Refused(owner));
                }
            },
            other => cell.owner = other,
        }));
        if let Err(payload) = result {
            cell.first_panic = Some(payload);
            self.fail_native_source(index);
            return Ok(());
        }
        after_completion();
        cell.pending = false;
        let completed = match cell.owner.as_ref() {
            Some(Owner::Prepared(owner)) => Some(owner.ready().presentation),
            _ => None,
        };
        let mut bank = lock(&self.bank);
        let closing = bank.closing;
        let slot = &mut bank.sources.slots[index];
        slot.starting = false;
        slot.presentation = completed;
        slot.retiring |= closing || completed.is_none();
        if completed.is_none() {
            slot.phase = Phase::Cleaning;
        }
        Ok(()) // Custody accepted, including native refusal or concurrent close.
    }

    pub(in crate::candidate_custody) fn native_source_prepared_state(
        &self,
        binding: Binding,
    ) -> Option<NativeDeviceState> {
        let work = self.work.try_lock().ok()?;
        let bank = self.bank.try_lock().ok()?;
        let index = bank.sources.index(binding)?;
        let slot = &bank.sources.slots[index];
        if slot.retiring || slot.starting || slot.phase != Phase::Accepted {
            return None;
        }
        let Some(Owner::Prepared(owner)) = work.sources.cells[index].owner.as_ref() else {
            return None;
        };
        // Preserve the original restore envelope, even if the plugin normalizes
        // its capture. A copy is a query, never a source/publication witness.
        Some(
            owner
                .configuration()
                .restore
                .as_ref()
                .unwrap_or(&owner.ready().initial_state)
                .clone(),
        )
    }
}

impl CandidateRoot {
    pub(crate) fn complete_native_source_preparation(
        &self,
        binding: Binding,
        presentation: Option<PresentationLatency>,
    ) -> Result<(), &'static str> {
        let result = self
            .0
            .eligible
            .complete_native_source(binding, presentation);
        crate::lifecycle::request_candidate_cleanup(self);
        result
    }

    pub(crate) fn native_source_prepared_state(
        &self,
        binding: Binding,
    ) -> Option<NativeDeviceState> {
        self.0.eligible.native_source_prepared_state(binding)
    }
}
