//! Fixed-size control/worker observations; no process-memory/admission policy.
use crate::{CatalogAllocationFootprint, ConstructionAllocationFootprint};
use hex_media::KernelBuildCapacity;
use std::sync::{Arc, Mutex};

/// Partial observed component capacities, preserved before failed storage is
/// reclaimed. `finalized` means an API result was produced, not successful
/// cleanup. A panic can leave only the last observation, with finalized=false.
/// Source slots include inline reader scratch. Definition paths move from the
/// input into Sources and are charged once; independent reader paths are extra.
/// Unique shared kernels are charged once across cache/plan handles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkerConstructionObservation {
    pub catalog: CatalogAllocationFootprint,
    pub construction: ConstructionAllocationFootprint,
    pub failed_kernel: KernelBuildCapacity,
    pub kernel_transient_unobserved: bool,
    pub finalized: bool,
    pub reservation_attempts: usize,
}
impl WorkerConstructionObservation {
    /// Conservative component-capacity/construction overlap bound, not a
    /// sampled simultaneous peak. Excludes allocator/Arc/OS/thread/stack costs.
    pub fn data_bound_bytes(self) -> Option<usize> {
        if self.kernel_transient_unobserved {
            return None;
        }
        let weights = self
            .catalog
            .kernel_weight_bytes
            .checked_add(self.failed_kernel.weight_bytes()?)?;
        let weights = weights.max(self.construction.kernel_table_peak_bound_bytes);
        self.catalog
            .bytes()?
            .checked_sub(self.catalog.kernel_weight_bytes)?
            .checked_add(weights)?
            .checked_add(self.failed_kernel.shared_object_bytes)?
            .checked_add(self.construction.definition_slots.bytes()?)?
            .checked_add(
                self.construction
                    .kernel_cache_slots
                    .bytes()?
                    .checked_mul(2)?,
            )
    }
}

#[derive(Clone, Default)]
pub(crate) struct WorkerFaults {
    #[cfg(test)]
    pub(crate) exit_hold: Option<Arc<std::sync::atomic::AtomicBool>>,
    #[cfg(test)]
    pub(crate) on_cancel: Option<Arc<dyn Fn(usize) + Send + Sync>>,
    #[cfg(test)]
    pub(crate) fail_reservation: Option<usize>,
    #[cfg(test)]
    pub(crate) hold: Option<Arc<std::sync::atomic::AtomicBool>>,
    #[cfg(test)]
    pub(crate) panic_before_catalog: bool,
    #[cfg(test)]
    pub(crate) panic_after_publish: bool,
    #[cfg(test)]
    pub(crate) lose_page: bool,
}

pub(crate) struct WorkerObserver {
    pub(crate) shared: Option<Arc<Mutex<WorkerConstructionObservation>>>,
    pub(crate) data: WorkerConstructionObservation,
    pub(crate) faults: WorkerFaults,
}
impl WorkerObserver {
    pub(crate) fn legacy() -> Self {
        Self {
            shared: None,
            data: WorkerConstructionObservation::default(),
            faults: WorkerFaults::default(),
        }
    }
    pub(crate) fn publish(&self) {
        if let Some(shared) = &self.shared {
            *shared.lock().unwrap_or_else(|p| p.into_inner()) = self.data;
        }
    }
    pub(crate) fn reserve(&mut self) -> Result<(), crate::StreamFailureKind> {
        if self.shared.is_none() {
            return Ok(());
        }
        #[cfg(test)]
        let attempt = self.data.reservation_attempts;
        self.data.reservation_attempts += 1;
        self.publish();
        #[cfg(test)]
        if self.faults.fail_reservation == Some(attempt) {
            return Err(crate::StreamFailureKind::CapacityExhausted);
        }
        Ok(())
    }
    pub(crate) fn observed(&self) -> bool {
        self.shared.is_some()
    }
}
