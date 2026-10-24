//! Stable backing retained by the actual completed native instance.
use super::*;

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
    /// Existing request-input footprint charged against a cell allowance. The
    /// borrowed helper metadata is charged prospectively for its copy, without
    /// predicting clone capacity. Configuration copies, capture and transient
    /// resources remain separate; this is not a complete preparation bound.
    pub fn preparation_input_data_bytes_on_control(
        &self,
        build: &InstanceBuild,
        restore: Option<&NativeDeviceState>,
    ) -> Option<usize> {
        if restore.is_some_and(|state| !state.asset_references().is_empty()) {
            return None;
        }
        self.metadata_backing()?
            .checked_add(build.retained_storage()?.data_bytes)?
            .checked_add(restore.map_or(0, |state| state.payload().len()))
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
                .checked_add(build.data_bytes)?,
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
    /// than concealing its extra retained String in an opaque process count.
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
            data_bytes,
            shared_groups: shared,
            shared_handle_bound: shared,
            ipc_handle_bound: 2,
            native_child_lifetime_bound: 1,
        })
    }
}
