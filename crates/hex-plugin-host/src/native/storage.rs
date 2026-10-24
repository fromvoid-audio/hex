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

impl Prepared {
    /// Inspect this source's actual string capacities and immutable state/Arc
    /// payloads before moving it to a joining worker. Optional restored state is
    /// charged independently of capture, even when a caller shares its payload.
    /// A lost child-custody diagnostic refuses this stable-only inventory rather
    /// than concealing its extra retained String in an opaque process count.
    pub fn retained_storage_on_control(&self) -> Option<PreparedStorage> {
        if matches!(self.lease.child, ChildCustody::Lost(_))
            || !self.ready.initial_state.asset_references().is_empty()
            || self
                .config
                .restore
                .as_ref()
                .is_some_and(|state| !state.asset_references().is_empty())
        {
            return None;
        }
        let restored = self.config.restore.as_ref();
        // These are the two actual cancellation Arc payloads. The control
        // socket is inline in Prepared; its OS storage is an IPC obligation.
        let data_bytes = self
            .config
            .bundle
            .capacity()
            .checked_add(self.config.executable.capacity())?
            .checked_add(self.ready.initial_state.payload().len())?
            .checked_add(restored.map_or(0, |state| state.payload().len()))?
            .checked_add(std::mem::size_of_val(
                self.lease.cancellation.stream.as_ref(),
            ))?
            .checked_add(std::mem::size_of_val(
                self.lease.cancellation.cancelled.as_ref(),
            ))?;
        let shared = 3 + usize::from(restored.is_some());
        Some(PreparedStorage {
            data_bytes,
            shared_groups: shared,
            shared_handle_bound: shared,
            ipc_handle_bound: 2,
            native_child_lifetime_bound: 1,
        })
    }
}
