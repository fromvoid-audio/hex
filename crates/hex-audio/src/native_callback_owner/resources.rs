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
    /// The fixed owner Box and terminal payload requested by `reserve::<T>`.
    /// The enclosing Guard/Terminal values and Arc/allocator internals are not
    /// additional DATA here. Only that private factory establishes custody.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(crate) fn owner_reservation<T>() -> Option<Self> {
        Some(Self {
            data_bytes: std::mem::size_of::<T>()
                .checked_add(std::mem::size_of::<OnceLock<Mutex<Box<T>>>>())?,
            shared_groups: 1,
            shared_handles: 2,
        })
    }
    pub(crate) fn ring_bytes<T>(capacity: usize) -> Option<usize> {
        capacity
            .checked_mul(std::mem::size_of::<T>())?
            .checked_add(std::mem::size_of::<rtrb::RingBuffer<T>>())
    }
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
    endpoint_cells: Option<[usize; 2]>,
    receipt_mailbox: Option<usize>,
    action_ledger: Option<usize>,
    data_bytes: usize,
    anchored: bool,
    channels: ChannelStorage,
    spectrum: Option<crate::spectrum_worker::NativeSpectrumReceipt>,
    spectrum_reader_bound: bool,
    spectrum_reaper_bound: bool,
}
impl NativeCandidateBoundary {
    pub(super) fn from_reserved(
        root: &crate::candidate_custody::CandidateRoot,
        data: &mut Guard<NativeCallbackOwner>,
        errors: &mut Guard<ErrorCallbackOwner>,
    ) -> Option<Self> {
        let data_cell = Arc::as_ptr(data.shared_cell()?) as usize;
        let error_cell = Arc::as_ptr(errors.shared_cell()?) as usize;
        let cell_data = std::mem::size_of_val(data.shared_cell()?.as_ref())
            .checked_add(std::mem::size_of_val(errors.shared_cell()?.as_ref()))?;
        let data = data.owner_ref()?;
        let errors = errors.owner_ref()?;
        if !data.callback.belongs_to_candidate_root(root)
            || !data.callback.matches_native_status(&data.status)
            || !Arc::ptr_eq(&data.status, &errors.status)
        {
            return None;
        }
        let spectrum = data.callback.native_spectrum_receipt()?;
        let channels = data.callback.constructor_channel_storage()?;
        let channels = if let Some(receipt) = spectrum {
            channels.plus(receipt.storage)?
        } else {
            channels
        };
        let data_bytes = std::mem::size_of_val(data)
            .checked_add(std::mem::size_of_val(errors))?
            .checked_add(cell_data)?
            // Actual persistent retirement destinations and the captured guard
            // values are distinct from their separately allocated owner Boxes.
            .checked_add(std::mem::size_of::<NativeRetirement>())?
            .checked_add(std::mem::size_of::<Guard<NativeCallbackOwner>>())?
            .checked_add(std::mem::size_of::<Guard<ErrorCallbackOwner>>())?
            .checked_add(data.status.candidate_boundary_data_bytes()?)?
            .checked_add(channels.data_bytes)?
            .checked_add(data.callback.native_endpoint_bank_data_bytes()?)?;
        Some(Self {
            root: root.root_id(),
            data_cell,
            error_cell,
            endpoint_cells: None,
            receipt_mailbox: None,
            action_ledger: data.callback.action_writer_identity(),
            data_bytes,
            anchored: false,
            channels,
            spectrum,
            spectrum_reader_bound: spectrum.is_none(),
            spectrum_reaper_bound: spectrum.is_none(),
        })
    }
    pub(super) fn with_worker_endpoints(
        mut self,
        client: &crate::prepared::Client,
        publication: &crate::prepared::WorkerControl,
    ) -> Option<Self> {
        let control = publication.owner_ref()?;
        if control.native_candidate_root().root_id() != self.root
            || !client.owner_ref()?.matches(&control.status)
            || client.owner_ref()?.action_admission_identity() != self.action_ledger
        {
            return None;
        }
        if client.owner_ref()?.native_spectrum_identity()?
            != self.spectrum.map(|receipt| receipt.identity)
        {
            return None;
        }
        let forwarding = client.owner_ref()?.resident_forwarding_storage(control)?;
        self.channels = self.channels.plus(forwarding)?;
        self.data_bytes = self.data_bytes.checked_add(forwarding.data_bytes)?;
        self.receipt_mailbox = Some(client.receipt_mailbox_identity()?);
        let receipt_bytes = crate::prepared::retired_scalar_receipts::storage_bytes();
        self.channels = self.channels.plus(ChannelStorage {
            data_bytes: receipt_bytes,
            shared_groups: 1,
            shared_handles: 2,
        })?;
        self.data_bytes = self.data_bytes.checked_add(receipt_bytes)?;
        self.spectrum_reader_bound = true;
        let client_cell = client.guard().shared_cell()?;
        let publication_cell = publication.guard().shared_cell()?;
        self.endpoint_cells = Some([
            Arc::as_ptr(client_cell) as usize,
            Arc::as_ptr(publication_cell) as usize,
        ]);
        self.data_bytes = self
            .data_bytes
            .checked_add(std::mem::size_of_val(client.owner_ref()?))?
            .checked_add(std::mem::size_of_val(control))?
            .checked_add(std::mem::size_of_val(client_cell.as_ref()))?
            .checked_add(std::mem::size_of_val(publication_cell.as_ref()))?
            .checked_add(std::mem::size_of::<crate::prepared::Client>())?
            .checked_add(std::mem::size_of::<crate::prepared::WorkerControl>())?;
        // Two actual terminal cells each have a guard and receiver handle.
        // The client bundle adds one Status handle to the unsplit constructor.
        self.channels = self.channels.plus(ChannelStorage {
            data_bytes: 0,
            shared_groups: 2,
            shared_handles: 5 + usize::from(self.action_ledger.is_some()),
        })?;
        Some(self)
    }
    pub(crate) fn anchor(&mut self, root: u64, terminals: &NativeTerminals) -> bool {
        let Some(data) = terminals.data.cell.as_ref() else {
            return false;
        };
        let Some(errors) = terminals.errors.cell.as_ref() else {
            return false;
        };
        let endpoint_match = match (self.endpoint_cells, terminals.endpoints.as_ref()) {
            (None, None) => true,
            (Some(expected), Some(endpoints)) => endpoints
                .client
                .cell
                .as_ref()
                .zip(endpoints.publication.cell.as_ref())
                .is_some_and(|(client, publication)| {
                    self.receipt_mailbox == Some(endpoints.receipts.identity())
                        && self.action_ledger
                            == endpoints
                                .action_failure
                                .as_ref()
                                .map(|signal| signal.identity())
                        && expected
                            == [
                                Arc::as_ptr(client) as usize,
                                Arc::as_ptr(publication) as usize,
                            ]
                }),
            _ => false,
        };
        if !endpoint_match
            || self.anchored
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
    pub(crate) fn anchor_spectrum_reaper(
        &mut self,
        identity: Option<crate::spectrum_worker::NativeSpectrumIdentity>,
    ) {
        self.spectrum_reaper_bound =
            self.spectrum_reader_bound && self.spectrum.map(|receipt| receipt.identity) == identity;
    }
    pub(crate) fn analysis_workers(&self) -> usize {
        usize::from(self.spectrum.is_some())
    }
    pub(crate) fn data_bytes(&self) -> Option<usize> {
        (self.anchored && self.spectrum_reader_bound && self.spectrum_reaper_bound)
            .then_some(self.data_bytes)
    }
    pub(crate) fn matches(&self, root: u64) -> bool {
        self.anchored
            && self.spectrum_reader_bound
            && self.spectrum_reaper_bound
            && self.root == root
    }
}

/// Normal worker factory's fixed payloads; both carriers and all endpoints are
/// quoted before opening. A live constructor still validates actual capacities.
pub(crate) fn worker_boundary_storage_quote() -> Option<usize> {
    std::mem::size_of::<NativeCallbackOwner>()
        .checked_add(std::mem::size_of::<ErrorCallbackOwner>())?
        .checked_add(std::mem::size_of::<OnceLock<Mutex<Box<NativeCallbackOwner>>>>())?
        .checked_add(std::mem::size_of::<OnceLock<Mutex<Box<ErrorCallbackOwner>>>>())?
        .checked_add(std::mem::size_of::<NativeRetirement>())?
        .checked_add(std::mem::size_of::<Guard<NativeCallbackOwner>>())?
        .checked_add(std::mem::size_of::<Guard<ErrorCallbackOwner>>())?
        .checked_add(crate::prepared::worker_boundary_storage_quote()?)?
        .checked_add(crate::prepared::native_endpoint_bank_storage_quote()?)?
        .checked_add(crate::spectrum_worker::constructor_storage_quote()?)
}
