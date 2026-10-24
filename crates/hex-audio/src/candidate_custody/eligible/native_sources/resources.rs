//! Private constructor-derived pool obligations. Fixed source cells bound all
//! native children/threads/IPC and preparation I/O contexts through physical
//! disposal and report recovery. The entire shared-handle reservation stays
//! charged while cells are vacant; it is not an allocation or a publication ACK.
use super::*;
use crate::plugin_worker::{NativeWorker, NativeWorkerResourceBound};

#[derive(Clone, Copy)]
pub(super) struct PoolResources {
    pub(super) per_source: NativeWorkerResourceBound,
    pub(super) capacity: usize,
    pub(super) shared_handles: usize,
}
impl PoolResources {
    pub(super) fn new(capacity: usize) -> Option<Self> {
        let mut per_source = NativeWorker::resource_bound_on_control()?;
        // Two retained Root cancellation aliases, two temporary startup aliases,
        // the one in-flight Root cancellation request (two handles), and the
        // callback's direct state alias, two endpoint-return signal handles,
        // and the retained control configuration alias. State payloads already belong
        // to the original source; the return signal adds one shared group.
        // No process-wide Arc count is consulted.
        per_source.shared_handle_bound = per_source.shared_handle_bound.checked_add(10)?;
        per_source.shared_groups = per_source.shared_groups.checked_add(1)?;
        Some(Self {
            per_source,
            capacity,
            shared_handles: per_source.shared_handle_bound.checked_mul(capacity)?,
        })
    }
}
impl Bank {
    pub(in crate::candidate_custody::eligible) fn reserved_shared_handles(&self) -> usize {
        self.resources
            .map_or(0, |resources| resources.shared_handles)
    }
    /// Actual stable unsplit constructor DATA, never a scalar allowance. Every
    /// occupied source must be the exact accepted effect in its original cell.
    /// Pending/refused/retiring/unwound work has no such declaration. Callback
    /// publication remains separately refused until its unique loan closes.
    pub(in crate::candidate_custody::eligible) fn actual_data_bytes(
        &self,
        work: &Work,
    ) -> Option<usize> {
        let Some(resources) = self.resources else {
            return self.slots.is_empty().then_some(0);
        };
        if self.slots.len() != resources.capacity || work.cells.len() != resources.capacity {
            return None;
        }
        let limits = self.limits?;
        self.slots
            .iter()
            .zip(&work.cells)
            .try_fold(0usize, |sum, (slot, cell)| {
                if slot.phase == Phase::Vacant {
                    return (cell.owner.is_none()
                        && !cell.pending
                        && cell.first_panic.is_none()
                        && cell.error.is_none()
                        && cell.cleanup_error.is_none()
                        && cell.disposal.is_none()
                        && !cell.constructor_refused)
                        .then_some(sum);
                }
                if slot.phase != Phase::Accepted
                    || slot.starting
                    || slot.retiring
                    || cell.pending
                    || cell.first_panic.is_some()
                    || cell.error.is_some()
                    || cell.cleanup_error.is_some()
                    || cell.constructor_refused
                    || slot.cancellation_requested
                    || slot.cancellation.is_none()
                {
                    return None;
                }
                let Some(Owner::Effect(effect)) = cell.owner.as_ref() else {
                    return None;
                };
                if slot.binding != Some(effect.binding())
                    || slot.identity != Some(effect.identity())
                    || slot.processor != Some(effect.prepared())
                {
                    return None;
                }
                let inventory = effect.constructor_storage_on_control()?;
                let data = inventory.data_bytes()?;
                let reserved = resources.per_source;
                if data.checked_add(hex_plugin_host::NATIVE_DIAGNOSTIC_DATA_BYTES)?
                    > limits.max_retained_data_bytes
                    || inventory.worker.shared_groups > reserved.shared_groups
                    || inventory.shared_handle_bound()?.checked_add(2)?
                        > reserved.shared_handle_bound
                    || inventory.worker.ipc_handle_bound > reserved.source.ipc_handle_bound
                    || inventory.worker.native_child_lifetime_bound
                        > reserved.source.native_child_lifetime_bound
                    || inventory.worker.joining_thread_lifetime_bound
                        > reserved.joining_thread_lifetime_bound
                {
                    return None;
                }
                sum.checked_add(data)
            })
    }
}
