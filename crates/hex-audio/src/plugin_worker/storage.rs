//! Actual joining-worker constructor backing. No thread/Arc allocator layout
//! is inferred, and no source/publication or physical deadline is admitted here.
use super::*;
use crate::native_callback_owner::{ChannelStorage, Guard};

pub(super) fn reserved_backing(
    guard: &Guard<State>,
    completed: &Arc<Signal>,
) -> Option<ChannelStorage> {
    let state = guard.owner_ref()?;
    let processor_bytes = match state.processor.as_ref()? {
        Processor::Function(process) => std::mem::size_of_val(process.as_ref()),
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        Processor::Native(instance) => std::mem::size_of_val(instance.as_ref())
            .checked_add(capture::MAX_RETAINED_PAYLOAD_BYTES)?,
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
    /// Known steady DATA requested by the exact native worker/queue factories,
    /// before allocating or spawning them. The original Prepared heap payloads
    /// are already present; their enclosing Box is additional here. Failure
    /// diagnostics, opaque std thread/allocator resources and transient exchange
    /// costs remain separate, so this is not complete Root eligibility.
    #[cfg(test)]
    pub(crate) fn preconstruction_data_bytes_on_control(
        instance: &hex_plugin_host::native::Prepared,
    ) -> Option<usize> {
        Self::preconstruction_data_bytes_for_source_on_control(
            instance.retained_storage_on_control()?,
        )
    }

    pub(crate) fn preconstruction_data_bytes_for_source_on_control(
        source: hex_plugin_host::native::PreparedStorage,
    ) -> Option<usize> {
        Self::preconstruction_with_channel(source, channel_storage_bound()?)
    }

    /// Known constructor DATA for the exact dense effect queue profile.
    pub fn preconstruction_dense_effect_data_bytes_on_control(
        instance: &hex_plugin_host::native::Prepared,
    ) -> Option<usize> {
        if instance.configuration().input_channels != 2 {
            return None;
        }
        Self::preconstruction_dense_effect_for_source_on_control(
            instance.retained_storage_on_control()?,
        )
    }

    pub(crate) fn preconstruction_dense_effect_for_source_on_control(
        source: hex_plugin_host::native::PreparedStorage,
    ) -> Option<usize> {
        Self::preconstruction_with_channel(source, dense_channel_storage_bound()?)
    }

    fn preconstruction_with_channel(
        source: hex_plugin_host::native::PreparedStorage,
        channel: ConstructorStorage,
    ) -> Option<usize> {
        fn capture_bytes<R>(_: fn(Guard<State>, Arc<Signal>) -> R) -> usize {
            std::mem::size_of::<R>()
        }
        source
            .data_bytes
            .checked_add(channel.data_bytes)?
            .checked_add(ChannelStorage::owner_reservation::<State>()?.data_bytes)?
            .checked_add(std::mem::size_of::<hex_plugin_host::native::Prepared>())?
            .checked_add(std::mem::size_of::<Signal>())?
            .checked_add(capture::MAX_RETAINED_PAYLOAD_BYTES)?
            .checked_add(capture_bytes(transport_runner))?
            .checked_add(std::mem::size_of::<Completion>())
    }

    /// Only the exact constructor-paired callback can inspect this inventory.
    /// State, source Box and terminal were observed before their original move
    /// to the thread, whose capture value was measured before std::thread spawn.
    pub fn constructor_storage_on_control(
        &self,
        callback: &Callback,
    ) -> Option<NativeWorkerStorage> {
        let transport = self.transport_storage_on_control(callback)?;
        let source = self.retained_source_storage_on_control(callback)?;
        self.constructor_storage_for(transport, source)
    }

    /// Constructor-owned backing of the exact reserved effect pair, including
    /// the independent dense ring and its control-owned staging storage.
    pub fn constructor_dense_effect_storage_on_control(
        &self,
        callback: &DenseEffectCallback,
    ) -> Option<NativeWorkerStorage> {
        let transport = callback.constructor_storage_on_control(&self.reaper.control)?;
        let source = self.source_storage?;
        self.constructor_storage_for(transport, source)
    }

    fn constructor_storage_for(
        &self,
        transport: ConstructorStorage,
        source: hex_plugin_host::native::PreparedStorage,
    ) -> Option<NativeWorkerStorage> {
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

/// Finite native constructor resources. Copies are informational; only the
/// private factory and its enclosing original Root/source cells retain owners.
/// Source resources, original copied channel, guard/terminal and completion
/// counter are counted separately from the opaque std joining context.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[derive(Clone, Copy, Debug)]
pub struct NativeWorkerResourceBound {
    pub source: hex_plugin_host::native::PreparationResourceBound,
    pub shared_groups: usize,
    pub shared_handle_bound: usize,
    pub joining_thread_lifetime_bound: usize,
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl NativeWorker {
    pub(crate) fn resource_bound_on_control() -> Option<NativeWorkerResourceBound> {
        let source = hex_plugin_host::native::PreparationResourceBound::ordinary_instance();
        let channel = channel_storage_bound()?;
        let terminal = ChannelStorage::owner_reservation::<State>()?;
        Some(NativeWorkerResourceBound {
            source,
            shared_groups: source
                .shared_groups
                .checked_add(channel.shared_groups)?
                .checked_add(terminal.shared_groups)?
                .checked_add(1)?, // Original completion counter.
            shared_handle_bound: source
                .shared_handle_bound
                .checked_add(channel.shared_handle_bound)?
                .checked_add(terminal.shared_handles)?
                .checked_add(2)?,
            joining_thread_lifetime_bound: 1,
        })
    }
}
