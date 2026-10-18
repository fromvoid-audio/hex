//! The original-resource bank belongs INSIDE the actual transferred custodian.
//! Installation and aggregate reservation remain sealed until their producer
//! route is complete. Legacy public constructors still refuse this nonempty path.
use super::*;

impl AudioPreparationCustody {
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
