//! Stable backing retained by the actual completed native instance.
use super::*;
use hex_lattice_contract::DeviceAssetReference;

/// Constructor lifetime bounds for one ordinary-instance preparation and its
/// retained source. Cancellation/captured/restored state contribute at most
/// four groups; sticky lost custody adds one original I/O group. Handles cover
/// this owner's private operation token and three lost-error aliases. Caller
/// token/state aliases, copied PCM queues and joining threads are separate.
/// One reader/iterator/entry/canonicalization context is live per serialized
/// revalidation. The preflight probe is reaped before instance startup, so
/// only one directly owned child/command context can be live. Four application
/// IPC endpoints may coexist before spawn; two remain in the Prepared owner.
/// std I/O error internals (at most original + cleanup), map nodes, process,
/// thread and allocator/Arc implementation costs are opaque, never DATA bytes.
/// These finite bounds follow the existing constructors, not a public permit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparationResourceBound {
    pub shared_groups: usize,
    pub shared_handle_bound: usize,
    pub ipc_handle_bound: usize,
    pub native_child_lifetime_bound: usize,
    pub command_context_lifetime_bound: usize,
    pub inventory_node_lifetime_bound: usize,
    pub file_reader_lifetime_bound: usize,
    pub directory_iterator_lifetime_bound: usize,
    pub directory_entry_lifetime_bound: usize,
    pub canonicalization_lifetime_bound: usize,
    pub io_error_lifetime_bound: usize,
}
impl PreparationResourceBound {
    pub const fn ordinary_instance() -> Self {
        Self {
            shared_groups: 5,
            shared_handle_bound: 9,
            ipc_handle_bound: 4,
            native_child_lifetime_bound: 1,
            command_context_lifetime_bound: 1,
            inventory_node_lifetime_bound:
                hex_plugin_scan::macos::build_identity::INSTANCE_INVENTORY_NODE_BOUND,
            file_reader_lifetime_bound: 1,
            directory_iterator_lifetime_bound: 1,
            directory_entry_lifetime_bound: 1,
            canonicalization_lifetime_bound: 1,
            io_error_lifetime_bound: 2,
        }
    }
}

/// Listed immutable heap payloads and this owner's resource obligations.
/// `Prepared` itself (including its packet/channel/child values) belongs to its
/// enclosing cell or Box and is excluded. Arc headers, allocator/OS overhead,
/// preparation/exchange temporaries and later failure diagnostics are excluded.
/// Counts cover handles retained here, not arbitrary external state/token clones.
/// A snapshot is not a construction reservation or Root/publication witness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedStorage {
    pub data_bytes: usize,
    pub shared_groups: usize,
    pub shared_handle_bound: usize,
    pub ipc_handle_bound: usize,
    pub native_child_lifetime_bound: usize,
}

/// Actual retained facts-stage configuration and source/helper provenance.
/// Inline Pending storage, OS/allocator/map-node implementation overhead and
/// preparation/revalidation temporaries are excluded. A configured child is not
/// final preparation, source admission or a construction-peak reservation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingStorage {
    pub data_bytes: usize,
    pub shared_groups: usize,
    pub shared_handle_bound: usize,
    pub ipc_handle_bound: usize,
    pub native_child_lifetime_bound: usize,
    pub inventory_node_lifetime_bound: usize,
}

/// Known retained DATA for request input, Pending and final Prepared phases.
/// Helper/path copies use exact-length producers before native startup; final
/// captured state uses the shared codec's limit. No I/O or allocation occurs in
/// this query. `base_peak_data_bytes` adds file/protocol working components;
/// opaque resource lifetimes and unexpected unwind backing remain separate. This cannot seal Root/publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparationStorageBound {
    pub input_data_bytes: usize,
    pub max_pending_data_bytes: usize,
    pub max_prepared_data_bytes: usize,
    pub helper_working_data_bytes: usize,
    pub source_working_data_bytes: usize,
    /// Included in both phase maxima: original cause Boxes and lost-custody
    /// I/O payload. std I/O/allocator/Arc internals remain opaque obligations.
    pub diagnostic_data_bytes: usize,
}
impl PreparationStorageBound {
    pub fn max_retained_data_bytes(self) -> usize {
        self.input_data_bytes
            .max(self.max_pending_data_bytes)
            .max(self.max_prepared_data_bytes)
    }
    /// Retained phases, one bounded file snapshot, the fixed instance helper's
    /// and selected-module path scratch plus explicit protocol buffers. Helper
    /// and selected-module revalidation are serialized, so their workspaces do
    /// not overlap. Original diagnostic DATA is included; opaque resources
    /// remain before sealing.
    pub fn base_peak_data_bytes(self) -> Option<usize> {
        self.max_retained_data_bytes()
            .checked_add(usize::try_from(hex_plugin_scan::MAX_BINARY).ok()?)?
            .checked_add(
                self.helper_working_data_bytes
                    .max(self.source_working_data_bytes),
            )?
            .checked_add(protocol::MAX_WORKING_DATA_BYTES)
    }
}

pub(super) fn copy_configuration_path(source: &str) -> Result<String> {
    if source.is_empty() || source.len() > 4096 || source.contains('\0') {
        return Err(Error::Protocol("unsupported instance configuration"));
    }
    let mut target = String::new();
    target
        .try_reserve_exact(source.len())
        .map_err(|_| Error::Protocol("instance configuration allocation refused"))?;
    if target.capacity() != source.len() {
        return Err(Error::Protocol(
            "instance configuration allocation bound differs",
        ));
    }
    target.push_str(source);
    Ok(target)
}

fn immutable_backing(
    config: &Configuration,
    captured: Option<&NativeDeviceState>,
    cancellation: &Cancellation,
) -> Option<(usize, usize)> {
    let restored = config.restore.as_ref();
    if captured
        .into_iter()
        .chain(restored)
        .any(|state| !state.asset_references().is_empty())
    {
        return None;
    }
    let data = config
        .bundle
        .capacity()
        .checked_add(config.executable.capacity())?
        .checked_add(captured.map_or(0, |state| state.payload().len()))?
        .checked_add(restored.map_or(0, |state| state.payload().len()))?
        .checked_add(std::mem::size_of_val(cancellation.stream.as_ref()))?
        .checked_add(std::mem::size_of_val(cancellation.cancelled.as_ref()))?;
    Some((
        data,
        2 + usize::from(captured.is_some()) + usize::from(restored.is_some()),
    ))
}

impl Source {
    /// Bound the actual input and both retained native preparation phases.
    /// Borrowed helper capacity conservatively covers its exact-length copy;
    /// configuration copies are length-controlled and final capture is bounded
    /// by the codec. These DATA components do not authorize native admission.
    pub fn preparation_storage_bound_on_control(
        &self,
        build: &InstanceBuild,
        restore: Option<&NativeDeviceState>,
    ) -> Option<PreparationStorageBound> {
        let asset_bytes = restore.map_or(Some(0), |state| {
            state
                .asset_references()
                .len()
                .checked_mul(std::mem::size_of::<DeviceAssetReference>())
        })?;
        let restore_bytes = restore
            .map_or(0, |state| state.payload().len())
            .checked_add(asset_bytes)?;
        let input_data_bytes = self
            .metadata_backing()?
            .checked_add(build.retained_storage()?.data_bytes)?
            .checked_add(restore_bytes)?;
        let config_paths = self
            .bundle
            .canonical_path
            .as_os_str()
            .len()
            .checked_add(self.executable.relative_path.len())?;
        let config_data = config_paths
            .checked_add(std::mem::size_of::<UnixStream>())?
            .checked_add(std::mem::size_of::<AtomicBool>())?;
        Some(PreparationStorageBound {
            input_data_bytes,
            max_pending_data_bytes: input_data_bytes
                .checked_add(config_data)?
                .checked_add(protocol::PACKET_LIMIT)?
                .checked_add(crate::NATIVE_DIAGNOSTIC_DATA_BYTES)?,
            max_prepared_data_bytes: config_data
                .checked_add(restore_bytes)?
                .checked_add(protocol::PACKET_LIMIT)?
                .checked_add(hex_plugin_state::MAX_ENVELOPE_BYTES)?
                .checked_add(crate::NATIVE_DIAGNOSTIC_DATA_BYTES)?,
            diagnostic_data_bytes: crate::NATIVE_DIAGNOSTIC_DATA_BYTES,
            helper_working_data_bytes: build.revalidation_storage_bound()?.data_bytes,
            source_working_data_bytes: self
                .bundle
                .revalidation_storage_bound(&self.executable)?
                .data_bytes,
        })
    }
    pub(super) fn metadata_backing(&self) -> Option<usize> {
        self.bundle
            .supplied_path
            .capacity()
            .checked_add(self.bundle.canonical_path.capacity())?
            .checked_add(self.bundle.info_sha256.capacity())?
            .checked_add(self.executable.relative_path.capacity())?
            .checked_add(self.executable.sha256.capacity())
    }
}

impl Pending {
    pub fn retained_storage_on_control(&self) -> Option<PendingStorage> {
        if matches!(self.lease.child, ChildCustody::Lost(_)) {
            return None;
        }
        let (data, shared) = immutable_backing(&self.config, None, &self.lease.cancellation)?;
        let build = self.build.retained_storage()?;
        Some(PendingStorage {
            data_bytes: data
                .checked_add(self.source.metadata_backing()?)?
                .checked_add(build.data_bytes)?
                .checked_add(self.packet.storage_bytes())?,
            shared_groups: shared,
            shared_handle_bound: shared,
            ipc_handle_bound: 2,
            native_child_lifetime_bound: 1,
            inventory_node_lifetime_bound: build.inventory_node_lifetime_bound,
        })
    }
}

impl Prepared {
    /// Inspect this source's actual string capacities and immutable state/Arc
    /// payloads before moving it to a joining worker. Optional restored state is
    /// charged independently of capture, even when a caller shares its payload.
    /// A lost child-custody diagnostic refuses this stable-only inventory rather
    /// than concealing its extra shared I/O payload in an opaque process count.
    pub fn retained_storage_on_control(&self) -> Option<PreparedStorage> {
        if matches!(self.lease.child, ChildCustody::Lost(_)) {
            return None;
        }
        let (data_bytes, shared) = immutable_backing(
            &self.config,
            Some(&self.ready.initial_state),
            &self.lease.cancellation,
        )?;
        Some(PreparedStorage {
            data_bytes: data_bytes.checked_add(self.packet.storage_bytes())?,
            shared_groups: shared,
            shared_handle_bound: shared,
            ipc_handle_bound: 2,
            native_child_lifetime_bound: 1,
        })
    }
}
