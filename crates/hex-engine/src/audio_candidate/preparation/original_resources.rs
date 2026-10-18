//! Nonextracting concrete producer route into the actual preparation custodian.
//! Numeric local allowances never certify global union, source freshness or native
//! publication. Those remain mandatory on the sealed same-worker host route.
use super::*;
use crate::native_device_resources::{
    OriginalResourceBankPlan, OriginalResourcePlanReservation, ResourcePreparationError,
};
impl AudioCandidatePreparation {
    pub(in crate::audio_candidate) fn install_original_resources_on_control(
        &mut self,
        plan: &OriginalResourceBankPlan<'_>,
        allowance: OriginalResourcePlanReservation,
        cancel: &dyn crate::CancellationQuery,
    ) -> Result<(), ResourcePreparationError> {
        if self.failed
            || self.cleaning
            || self.complete
            || self.original_resource_attempted
            || self.owner.is_some()
            || self.bank.is_some()
            || self.token.is_some()
            || self.resident_draft.capacity() != 0
            || self
                .custody
                .as_ref()
                .is_none_or(|custody| !custody.original_resource_destination_vacant())
        {
            return Err(AudioStorageError::SourceMismatch.into());
        }
        // These are genuine ceilings on this already-reserved cell, not a
        // scalar admission permit or a substitute for the enclosing prequote.
        let base = self.collection_bytes()?;
        let retained = base
            .checked_add(allowance.max_retained_data_bytes)
            .ok_or(AudioStorageError::Overflow)?;
        let peak = base
            .checked_add(allowance.max_construction_peak_bytes)
            .and_then(|bytes| bytes.checked_add(self.reservation.caller_construction_bytes))
            .ok_or(AudioStorageError::Overflow)?;
        if retained > self.reservation.max_prepared_data_bytes
            || peak > self.reservation.max_construction_data_bytes
            || allowance.inputs.max_prepared_data_bytes > allowance.max_retained_data_bytes
            || allowance.inputs.max_construction_data_bytes > allowance.max_construction_peak_bytes
            || allowance.max_preparation_metadata_bytes > allowance.max_construction_peak_bytes
            || allowance.catalog.metadata_bytes > allowance.max_construction_peak_bytes
        {
            return Err(AudioStorageError::ReservationExceeded.into());
        }
        self.original_resource_attempted = true;
        let result = catch_unwind(AssertUnwindSafe(|| {
            self.custody
                .as_mut()
                .ok_or(AudioStorageError::SourceMismatch)?
                .prepare_original_resources_on_control(plan, allowance, cancel)
        }));
        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                self.original_resource_refused = true;
                Err(error)
            }
            Err(payload) => {
                self.first_panic = Some(payload);
                self.failed = true;
                Err(AudioStorageError::UnknownStorage.into())
            }
        }
    }
}

#[cfg(test)]
mod tests;
