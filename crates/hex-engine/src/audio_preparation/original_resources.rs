//! The original-resource bank belongs INSIDE the actual transferred custodian.
//! Installation and aggregate reservation remain sealed until their producer
//! route is complete. Legacy public constructors still refuse this nonempty path.
use super::*;

impl AudioPreparationCustody {
    pub(crate) fn original_resource_destination_vacant(&self) -> bool {
        self.phase == AudioPreparationPhase::Vacant
            && self.bank.is_none()
            && self.original_resources.is_none()
            && !self.original_resources_drop_pending
            && self.first_panic.is_none()
    }
    pub(crate) fn prepare_original_resources_on_control(
        &mut self,
        plan: &crate::native_device_resources::OriginalResourceBankPlan<'_>,
        allowance: crate::native_device_resources::OriginalResourcePlanReservation,
        cancel: &dyn crate::CancellationQuery,
    ) -> Result<(), crate::native_device_resources::ResourcePreparationError> {
        if !self.original_resource_destination_vacant() {
            return Err(AudioStorageError::SourceMismatch.into());
        }
        // Retain the producer's preflighted local allowance through failure.
        // This is not the missing aggregate current/history/native peak quote.
        self.capacity.original_resource_retained_allowance = allowance.max_retained_data_bytes;
        self.capacity.original_resource_construction_allowance =
            allowance.max_construction_peak_bytes;
        // Success moves directly into the actual owner. No cancellation query,
        // allocation, clone, callback or other fallible work follows the producer.
        self.original_resources = Some(plan.prepare_on_control(allowance, cancel)?);
        Ok(())
    }
    pub(super) fn dispose_original_resources_on_control(&mut self) -> bool {
        if self.original_resources_drop_pending {
            self.phase = AudioPreparationPhase::FailedIncomplete;
            return false;
        }
        if self.original_resources.is_none() {
            return true;
        }
        // Record unresolved destruction before removing the actual owning field.
        // Panic cannot turn a missing owner into successful/retryable cleanup.
        self.original_resources_drop_pending = true;
        match catch_unwind(AssertUnwindSafe(|| {
            drop(self.original_resources.take());
        })) {
            Ok(()) => {
                self.original_resources_drop_pending = false;
                true
            }
            Err(payload) => {
                self.first_panic = Some(payload);
                self.phase = AudioPreparationPhase::FailedIncomplete;
                false
            }
        }
    }
}

#[cfg(test)]
mod tests;
