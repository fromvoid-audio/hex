//! Actual joining-worker constructor backing. No thread/Arc allocator layout
//! is inferred, and no source/publication or physical deadline is admitted here.
use super::*;
use crate::native_callback_owner::{ChannelStorage, Guard};

pub(super) fn reserved_backing(
    guard: &Guard<State>,
    completed: &Arc<AtomicU64>,
) -> Option<ChannelStorage> {
    let state = guard.owner_ref()?;
    let processor_bytes = match state.processor.as_ref()? {
        Processor::Function(process) => std::mem::size_of_val(process.as_ref()),
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        Processor::Native(instance) => std::mem::size_of_val(instance.as_ref()),
    };
    Some(ChannelStorage {
        data_bytes: std::mem::size_of_val(state)
            .checked_add(processor_bytes)?
            .checked_add(std::mem::size_of_val(guard.shared_cell()?.as_ref()))?
            .checked_add(std::mem::size_of_val(completed.as_ref()))?,
        // Original terminal/guard cell and control/worker completion counter.
        shared_groups: 2,
        shared_handles: 4,
    })
}

/// Constructor DATA bound for the original native source, worker and queues.
/// Excludes the enclosing NativeWorker value, failure diagnostics and exchange/
/// preparation temporaries. Known closure/result payloads are listed separately
/// from opaque std joining-context and allocator/Arc overhead. The original
/// charge stays until disposal, even after thread exit/normal native retirement.
/// Source handoff invalidates this composite; it does not reclaim old queues.
/// This is not a complete Root resource witness, RSS bound or publication ACK.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeWorkerStorage {
    pub data_bytes: usize,
    pub shared_groups: usize,
    pub shared_handle_bound: usize,
    pub ipc_handle_bound: usize,
    pub native_child_lifetime_bound: usize,
    pub joining_thread_lifetime_bound: usize,
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl NativeWorker {
    /// Only the exact constructor-paired callback can inspect this inventory.
    /// State, source Box and terminal were observed before their original move
    /// to the thread, whose capture value was measured before std::thread spawn.
    pub fn constructor_storage_on_control(
        &self,
        callback: &Callback,
    ) -> Option<NativeWorkerStorage> {
        let transport = self.transport_storage_on_control(callback)?;
        let source = self.retained_source_storage_on_control(callback)?;
        let worker = self.reaper.storage?;
        Some(NativeWorkerStorage {
            data_bytes: transport
                .data_bytes
                .checked_add(source.data_bytes)?
                .checked_add(worker.data_bytes)?,
            shared_groups: transport
                .shared_groups
                .checked_add(source.shared_groups)?
                .checked_add(worker.shared_groups)?,
            shared_handle_bound: transport
                .shared_handle_bound
                .checked_add(source.shared_handle_bound)?
                .checked_add(worker.shared_handles)?,
            ipc_handle_bound: source.ipc_handle_bound,
            native_child_lifetime_bound: source.native_child_lifetime_bound,
            joining_thread_lifetime_bound: 1,
        })
    }
}
