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
#[derive(Default)]
pub(in crate::candidate_custody::eligible) struct Inventory {
    pub data: usize,
    pub groups: usize,
    pub workers: usize,
    pub children: usize,
    pub ipc: usize,
}
impl Bank {
    pub(in crate::candidate_custody::eligible) fn reserved_shared_handles(&self) -> usize {
        self.resources
            .map_or(0, |resources| resources.shared_handles)
    }
    /// Actual constructor DATA from each retained original source/control.
    /// Unique callback loans retain their constructor backing while Root keeps
    /// the paired worker. Pending/refused/unwound work has no declaration.
    /// Graph references and endpoint availability are validated separately.
    pub(in crate::candidate_custody::eligible) fn actual_data_bytes(
        &self,
        work: &Work,
    ) -> Option<usize> {
        self.inventory(work).map(|inventory| inventory.data)
    }
    pub(in crate::candidate_custody::eligible) fn inventory(
        &self,
        work: &Work,
    ) -> Option<Inventory> {
        let Some(resources) = self.resources else {
            return self.slots.is_empty().then_some(Inventory::default());
        };
        if self.slots.len() != resources.capacity || work.cells.len() != resources.capacity {
            return None;
        }
        let limits = self.limits?;
        self.slots.iter().zip(&work.cells).try_fold(
            Inventory::default(),
            |mut sum, (slot, cell)| {
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
                let (binding, identity, prepared, inventory) = match cell.owner.as_ref()? {
                    Owner::Effect(effect)
                        if !slot.endpoint_loaned && slot.endpoint_return.is_none() =>
                    {
                        (
                            effect.binding(),
                            effect.identity(),
                            effect.prepared(),
                            effect.constructor_storage_on_control()?,
                        )
                    }
                    Owner::Loaned(control)
                        if slot.endpoint_loaned && slot.endpoint_return.is_some() =>
                    {
                        (
                            control.binding(),
                            control.identity(),
                            control.prepared(),
                            control.constructor_storage_on_control()?,
                        )
                    }
                    _ => return None,
                };
                if slot.binding != Some(binding)
                    || slot.identity != Some(identity)
                    || slot.processor != Some(prepared)
                {
                    return None;
                }
                let loan_data = if slot.endpoint_loaned {
                    NativeEndpointEnvelope::loan_data_bytes()?
                } else {
                    0
                };
                let data = inventory.data_bytes()?.checked_add(loan_data)?;
                let reserved = resources.per_source;
                if data.checked_add(hex_plugin_host::NATIVE_DIAGNOSTIC_DATA_BYTES)?
                    > limits.max_retained_data_bytes
                    || inventory.worker.shared_groups > reserved.shared_groups
                    || inventory
                        .shared_handle_bound()?
                        .checked_add(if slot.endpoint_loaned { 5 } else { 2 })?
                        > reserved.shared_handle_bound
                    || inventory.worker.ipc_handle_bound > reserved.source.ipc_handle_bound
                    || inventory.worker.native_child_lifetime_bound
                        > reserved.source.native_child_lifetime_bound
                    || inventory.worker.joining_thread_lifetime_bound
                        > reserved.joining_thread_lifetime_bound
                {
                    return None;
                }
                sum.data = sum.data.checked_add(data)?;
                sum.groups = sum
                    .groups
                    .checked_add(inventory.worker.shared_groups)?
                    .checked_add(usize::from(slot.endpoint_loaned))?;
                sum.workers = sum
                    .workers
                    .checked_add(inventory.worker.joining_thread_lifetime_bound)?;
                sum.children = sum
                    .children
                    .checked_add(inventory.worker.native_child_lifetime_bound)?;
                sum.ipc = sum.ipc.checked_add(inventory.worker.ipc_handle_bound)?;
                Some(sum)
            },
        )
    }
}
