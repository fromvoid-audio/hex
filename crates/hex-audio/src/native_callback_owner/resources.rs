//! Constructor-bound payload and opaque-resource declaration for these exact
//! coupled carriers. No Arc-header/OS byte estimate, arbitrary Arc adoption or
//! backend-stop claim is made here.
use super::*;
/// Known payloads and bounded shared groups from one exact private channel
/// factory. Arc control blocks/allocator overhead stay opaque RESOURCE costs.
#[derive(Clone, Copy, Default)]
pub(crate) struct ChannelStorage {
    pub(crate) data_bytes: usize,
    pub(crate) shared_groups: usize,
    pub(crate) shared_handles: usize,
}
impl ChannelStorage {
    pub(crate) fn ring<T>(ring: &rtrb::RingBuffer<T>) -> Option<Self> {
        Some(Self {
            data_bytes: ring
                .capacity()
                .checked_mul(std::mem::size_of::<T>())?
                .checked_add(std::mem::size_of_val(ring))?,
            shared_groups: 1,
            shared_handles: 2,
        })
    }
    // Caller owns the exact constructor's two private Arc handles; observing a
    // generic Arc's count is never used as provenance or a byte-layout proof.
    pub(crate) fn paired_payload<T>(payload: &T) -> Self {
        Self {
            data_bytes: std::mem::size_of_val(payload),
            shared_groups: 1,
            shared_handles: 2,
        }
    }
    pub(crate) fn plus(self, other: Self) -> Option<Self> {
        Some(Self {
            data_bytes: self.data_bytes.checked_add(other.data_bytes)?,
            shared_groups: self.shared_groups.checked_add(other.shared_groups)?,
            shared_handles: self.shared_handles.checked_add(other.shared_handles)?,
        })
    }
}
#[derive(Clone, Copy)]
pub(crate) struct NativeCandidateBoundary {
    root: u64,
    data_cell: usize,
    error_cell: usize,
    data_bytes: usize,
    anchored: bool,
    channels: ChannelStorage,
}
impl NativeCandidateBoundary {
    pub(super) fn from_reserved(
        root: &crate::candidate_custody::CandidateRoot,
        data: &mut Guard<NativeCallbackOwner>,
        errors: &mut Guard<ErrorCallbackOwner>,
    ) -> Option<Self> {
        let data_cell = Arc::as_ptr(&data.cell) as usize;
        let error_cell = Arc::as_ptr(&errors.cell) as usize;
        let cell_data = std::mem::size_of_val(data.cell.as_ref())
            .checked_add(std::mem::size_of_val(errors.cell.as_ref()))?;
        let data = data.owner_mut()?;
        let errors = errors.owner_mut()?;
        if !data.callback.belongs_to_candidate_root(root)
            || !data.callback.matches_native_status(&data.status)
            || !Arc::ptr_eq(&data.status, &errors.status)
        {
            return None;
        }
        let channels = data.callback.constructor_channel_storage()?;
        let data_bytes = std::mem::size_of_val(data)
            .checked_add(std::mem::size_of_val(errors))?
            .checked_add(cell_data)?
            // Separate fixed terminal destinations remain reserved through both
            // carrier destructors, including a failed destructor's quarantine.
            .checked_add(std::mem::size_of::<
                [Option<crate::candidate_custody::DomainEnvelope>; 2],
            >())?
            .checked_add(data.status.candidate_boundary_data_bytes()?)?
            .checked_add(channels.data_bytes)?;
        Some(Self {
            root: root.root_id(),
            data_cell,
            error_cell,
            data_bytes,
            anchored: false,
            channels,
        })
    }
    pub(crate) fn anchor(&mut self, root: u64, terminals: &NativeTerminals) -> bool {
        let Some(data) = terminals.data.cell.as_ref() else {
            return false;
        };
        let Some(errors) = terminals.errors.cell.as_ref() else {
            return false;
        };
        if self.anchored
            || self.root != root
            || self.data_cell != Arc::as_ptr(data) as usize
            || self.error_cell != Arc::as_ptr(errors) as usize
        {
            return false;
        }
        self.anchored = true;
        true
    }
    pub(crate) fn channel_resources(&self) -> (usize, usize) {
        (self.channels.shared_groups, self.channels.shared_handles)
    }
    pub(crate) fn data_bytes(&self) -> Option<usize> {
        self.anchored.then_some(self.data_bytes)
    }
    pub(crate) fn matches(&self, root: u64) -> bool {
        self.anchored && self.root == root
    }
}
