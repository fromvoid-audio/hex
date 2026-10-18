//! Constructor-bound payload and opaque-resource declaration for these exact
//! coupled carriers. Fresh carrier Box/cell lineage has separate pinned layout
//! qualification. Other shared/Status headers and children remain separate;
//! this is not a full DATA closure or a backend-stop claim.
use super::*;
/// Known payloads and bounded shared groups from one exact private channel
/// factory. Fixed rtrb requested Arc/backing bytes are priced separately;
/// other shared headers and element-owned children remain unqualified.
#[derive(Clone, Copy, Default)]
pub(crate) struct ChannelStorage {
    pub(crate) data_bytes: usize,
    pub(crate) shared_groups: usize,
    pub(crate) shared_handles: usize,
}
impl ChannelStorage {
    pub(crate) fn ring<T>(ring: &rtrb::RingBuffer<T>) -> Option<Self> {
        Some(Self {
            data_bytes: super::ring_storage::requested_bytes(ring)?,
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
    carriers: [super::allocations::CarrierAllocation; 2],
    endpoint_carriers: Option<[super::allocations::CarrierAllocation; 2]>,
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
        let carriers = [
            super::allocations::CarrierAllocation::capture(data)?,
            super::allocations::CarrierAllocation::capture(errors)?,
        ];
        let carrier_bytes = carriers[0]
            .requested_bytes()?
            .checked_add(carriers[1].requested_bytes()?)?;
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
        if !carriers[0].matches_body(data) || !carriers[1].matches_body(errors) {
            return None;
        }
        let data_bytes = carrier_bytes
            // Actual persistent retirement destinations and the captured guard
            // values are distinct from their separately allocated owner Boxes.
            .checked_add(std::mem::size_of::<NativeRetirement>())?
            .checked_add(std::mem::size_of::<Guard<NativeCallbackOwner>>())?
            .checked_add(std::mem::size_of::<Guard<ErrorCallbackOwner>>())?
            .checked_add(data.status.candidate_boundary_data_bytes()?)?
            .checked_add(channels.data_bytes)?;
        Some(Self {
            root: root.root_id(),
            carriers,
            endpoint_carriers: None,
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
        let endpoint_carriers = [
            super::allocations::CarrierAllocation::capture(client.guard())?,
            super::allocations::CarrierAllocation::capture(publication.guard())?,
        ];
        let control = publication.owner_ref()?;
        if control.native_candidate_root().root_id() != self.root
            || !client.owner_ref()?.matches(&control.status)
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
        self.spectrum_reader_bound = true;
        self.endpoint_carriers = Some(endpoint_carriers);
        self.data_bytes = self
            .data_bytes
            .checked_add(endpoint_carriers[0].requested_bytes()?)?
            .checked_add(endpoint_carriers[1].requested_bytes()?)?
            .checked_add(std::mem::size_of::<crate::prepared::Client>())?
            .checked_add(std::mem::size_of::<crate::prepared::WorkerControl>())?;
        // Two actual terminal cells each have a guard and receiver handle.
        // The client bundle adds one Status handle to the unsplit constructor.
        self.channels = self.channels.plus(ChannelStorage {
            data_bytes: 0,
            shared_groups: 2,
            shared_handles: 5,
        })?;
        Some(self)
    }
    pub(crate) fn anchor(&mut self, root: u64, terminals: &NativeTerminals) -> bool {
        let endpoint_match = match (self.endpoint_carriers, terminals.endpoints.as_ref()) {
            (None, None) => true,
            (Some(expected), Some(endpoints)) => {
                expected[0].matches_terminal(&endpoints.client)
                    && expected[1].matches_terminal(&endpoints.publication)
            }
            _ => false,
        };
        if !endpoint_match
            || self.anchored
            || self.root != root
            || !self.carriers[0].matches_terminal(&terminals.data)
            || !self.carriers[1].matches_terminal(&terminals.errors)
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
