//! Borrowed, control-only DATA inventory and unresolved owning HANDLEs.
//! No owner is cloned or transferred; this is never publication/lifetime authority.
use crate::{AudioPreparationCustody, AudioStorageError, OwnedAudioInputs, StreamedSourceMetadata};
use std::{any::Any, sync::Arc};
mod arithmetic;
mod canonical_state;
pub use canonical_state::canonical_native_state_storage_layout_supported;
mod model_buffers;
mod models;
pub use models::canonical_midi_storage_layout_supported;
#[cfg(test)]
mod construction_tests;
mod sources;
mod values;
#[cfg(test)]
thread_local! {
    static BACKING_RESERVATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioStorageRole {
    Current,
    Predecessor,
    Archive,
    PendingReclaim,
    FailedConstruction,
    Caller,
}
#[derive(Clone, Copy, Debug)]
pub struct AudioStorageLedgerReservation {
    pub data_rows: usize,
    pub handle_rows: usize,
    pub root_rows: usize,
    pub max_retained_bytes: usize,
    pub max_construction_peak_bytes: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioStorageIncomplete {
    UnknownStorage,
    RetirementReceiptUnknown,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioRootStorageSummary {
    pub data_allocations: usize,
    pub unresolved_handles: usize,
    pub unresolved_retirements: usize,
    pub unknown_storage: usize,
    pub data_bytes: usize,
    pub ledger_bytes: usize,
    pub retained_bytes: usize,
    pub construction_peak_bytes: usize,
}
impl AudioRootStorageSummary {
    pub fn require_known_storage(self) -> Result<Self, AudioStorageIncomplete> {
        if self.unknown_storage != 0 {
            Err(AudioStorageIncomplete::UnknownStorage)
        } else {
            Ok(self)
        }
    }
    pub fn require_complete(self) -> Result<(), AudioStorageIncomplete> {
        self.require_known_storage()?;
        Err(AudioStorageIncomplete::RetirementReceiptUnknown)
    }
}
type SharedCounts = fn(&dyn Any) -> Option<(usize, usize)>;
#[derive(Clone, Copy, PartialEq, Eq)]
enum OwnerDomain {
    Exclusive,
    Shared,
    DevicePayload,
    DeviceAssetReferences,
    CanonicalMidiSource,
    CanonicalMidiField,
    ExclusiveVec,
}
// Actual borrowed Vec OBJECT identity only. This private trait permits element
// types containing non-static guards without widening Any or copying a header.
trait PrivateVecField {}
impl<T> PrivateVecField for Vec<T> {}
#[derive(Clone, Copy)]
pub(crate) struct Owner<'a> {
    domain: OwnerDomain,
    body_field: Option<&'a dyn crate::sampler_host_fields::BodyLayout>,
    shared_field: Option<&'a dyn crate::sampler_host_fields::SharedLayout>,
    vec_field: Option<&'a (dyn PrivateVecField + 'a)>,
    value: &'a dyn Any,
    compare: fn(&dyn Any, &dyn Any) -> bool,
    counts: Option<SharedCounts>,
}
impl<'a> Owner<'a> {
    pub(crate) fn exclusive<T: 'static>(value: &'a T) -> Self {
        Self {
            body_field: None,
            shared_field: None,
            vec_field: None,
            domain: OwnerDomain::Exclusive,
            value,
            counts: None,
            compare: |a, b| {
                a.downcast_ref::<T>()
                    .zip(b.downcast_ref::<T>())
                    .is_some_and(|(a, b)| std::ptr::eq(a, b))
            },
        }
    }
    // Actual lifetime-bound Vec field identity only, including borrowed-element
    // metadata vectors. This grants no source origin or retirement authority.
    pub(crate) fn exclusive_vec<T>(value: &'a Vec<T>) -> Self {
        Self {
            domain: OwnerDomain::ExclusiveVec,
            body_field: None,
            shared_field: None,
            vec_field: Some(value), // the REAL lifetime-bound Vec field
            value: &(),             // unused in this domain; never identity/provenance
            compare: |_, _| false,
            counts: None,
        }
    }
    pub(crate) fn shared<T: 'static + ?Sized>(value: &'a Arc<T>) -> Self {
        Self {
            body_field: None,
            shared_field: Some(value),
            vec_field: None,
            domain: OwnerDomain::Shared,
            value,
            counts: Some(|value| {
                value
                    .downcast_ref::<Arc<T>>()
                    .map(|owner| (Arc::strong_count(owner), Arc::weak_count(owner)))
            }),
            compare: |a, b| {
                a.downcast_ref::<Arc<T>>()
                    .zip(b.downcast_ref::<Arc<T>>())
                    .is_some_and(|(a, b)| Arc::ptr_eq(a, b))
            },
        }
    }
    pub(crate) fn device_payload(value: &'a hex_lattice_contract::NativeDeviceState) -> Self {
        Self {
            body_field: None,
            shared_field: None,
            vec_field: None,
            domain: OwnerDomain::DevicePayload,
            value,
            counts: Some(|value| {
                value
                    .downcast_ref::<hex_lattice_contract::NativeDeviceState>()
                    .map(|state| {
                        let (_, strong, weak) = state.payload_storage_observation();
                        (strong, weak)
                    })
            }),
            compare: |a, b| {
                a.downcast_ref::<hex_lattice_contract::NativeDeviceState>()
                    .zip(b.downcast_ref::<hex_lattice_contract::NativeDeviceState>())
                    .is_some_and(|(a, b)| a.shares_payload_storage_with(b))
            },
        }
    }
    pub(crate) fn device_asset_references(
        value: &'a hex_lattice_contract::NativeDeviceState,
    ) -> Self {
        Self {
            body_field: None,
            shared_field: None,
            vec_field: None,
            domain: OwnerDomain::DeviceAssetReferences,
            value,
            counts: Some(|value| {
                value
                    .downcast_ref::<hex_lattice_contract::NativeDeviceState>()
                    .and_then(|state| {
                        state
                            .asset_reference_storage_observation()
                            .map(|(_, strong, weak)| (strong, weak))
                    })
            }),
            compare: |a, b| {
                a.downcast_ref::<hex_lattice_contract::NativeDeviceState>()
                    .zip(b.downcast_ref::<hex_lattice_contract::NativeDeviceState>())
                    .is_some_and(|(a, b)| a.shares_asset_reference_storage_with(b))
            },
        }
    }
    pub(crate) fn borrowed_body<T: crate::sampler_host_fields::BodyLayout>(value: &'a T) -> Self {
        Self {
            domain: OwnerDomain::Exclusive,
            body_field: Some(value),
            shared_field: None,
            vec_field: None,
            value: &(),
            compare: |_, _| false,
            counts: None,
        }
    }
    pub(crate) fn borrowed_body_ref(value: &'a dyn crate::sampler_host_fields::BodyLayout) -> Self {
        Self {
            domain: OwnerDomain::Exclusive,
            body_field: Some(value),
            shared_field: None,
            vec_field: None,
            value: &(),
            compare: |_, _| false,
            counts: None,
        }
    }
    pub(crate) fn borrowed_shared(value: &'a dyn crate::sampler_host_fields::SharedLayout) -> Self {
        Self {
            domain: OwnerDomain::Shared,
            body_field: None,
            shared_field: Some(value),
            vec_field: None,
            value: &(),
            compare: |_, _| false,
            counts: None,
        }
    }
    fn actual_counts(self) -> Result<Option<(usize, usize)>, AudioStorageError> {
        if let Some(shared) = self.shared_field {
            return Ok(Some(shared.counts()));
        }
        self.counts
            .map(|counts| counts(self.value).ok_or(AudioStorageError::UnknownStorage))
            .transpose()
    }
    fn allocation_address(self) -> Option<*const ()> {
        self.shared_field.map(|field| field.allocation_address())
    }
    fn same(self, other: Self) -> bool {
        if self.domain != other.domain {
            return false;
        }
        if self.shared_field.is_some() || other.shared_field.is_some() {
            return self
                .allocation_address()
                .zip(other.allocation_address())
                .is_some_and(|(a, b)| a == b);
        }
        match (self.body_field, other.body_field) {
            (Some(a), Some(b)) => return std::ptr::addr_eq(a, b),
            (Some(a), None) => return std::ptr::addr_eq(a, other.value),
            (None, Some(b)) => return std::ptr::addr_eq(self.value, b),
            _ => {}
        }
        match (self.vec_field, other.vec_field) {
            (Some(a), Some(b)) => std::ptr::addr_eq(a, b),
            (None, None) => (self.compare)(self.value, other.value),
            _ => false,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Location(pub(crate) &'static str, pub(crate) usize);
#[derive(Clone, Copy, PartialEq, Eq)]
struct Declaration {
    bytes: usize,
    capacity: usize,
    element_bytes: usize,
    overhead: usize,
}
struct Data<'a> {
    owner: Owner<'a>,
    location: Location,
    declaration: Declaration,
    streamed: Option<&'a StreamedSourceMetadata>,
}
struct Handle<'a> {
    owner: Owner<'a>,
    field: Owner<'a>,
    location: Location,
    id: u64,
}
struct Root<'a> {
    owner: Owner<'a>,
    _role: AudioStorageRole,
}
/// One actual metadata allocation's unresolved source-domain relation. Equal
/// metadata in two rows is still two owners; every physical handle is conserved.
#[derive(Clone, Copy, Debug)]
pub struct StreamedSourceBindingObligation<'a> {
    pub metadata: &'a StreamedSourceMetadata,
    pub owning_handles: usize,
}
/// A lifetime-bound inventory of the selected actual custodies. Internal rows
/// cannot be manufactured, edited, or used to obtain an owning graph reference.
/// Mutation/cleanup is statically unavailable while this borrow remains alive.
pub struct AudioRootStorageLedger<'a> {
    data: Vec<Data<'a>>,
    handles: Vec<Handle<'a>>,
    roots: Vec<Root<'a>>,
    reservation: AudioStorageLedgerReservation,
    growth: usize,
    unknown: usize,
    valid: bool,
    arithmetic: Option<arithmetic::Arithmetic<'a>>,
}
impl<'a> AudioRootStorageLedger<'a> {
    pub fn try_new_on_control(
        reservation: AudioStorageLedgerReservation,
    ) -> Result<Self, AudioStorageError> {
        // This lower bound is sufficient for a negative decision on every
        // allocator: each Vec must hold at least its requested rows. In the
        // pinned native std Global/RawVec::grow_exact family it is the exact
        // requested backing. It is NOT an external input-union allowance.
        let requested = Self::requested_metadata_bytes(reservation)?;
        if requested > reservation.max_retained_bytes
            || requested > reservation.max_construction_peak_bytes
        {
            return Err(AudioStorageError::ReservationExceeded);
        }
        let mut result = Self {
            data: Vec::new(),
            handles: Vec::new(),
            roots: Vec::new(),
            reservation,
            growth: 0,
            unknown: 0,
            valid: false,
            arithmetic: None,
        };
        #[cfg(test)]
        BACKING_RESERVATIONS.with(|count| count.set(count.get() + 1));
        result
            .data
            .try_reserve_exact(reservation.data_rows)
            .map_err(|_| AudioStorageError::Allocation)?;
        result
            .handles
            .try_reserve_exact(reservation.handle_rows)
            .map_err(|_| AudioStorageError::Allocation)?;
        result
            .roots
            .try_reserve_exact(reservation.root_rows)
            .map_err(|_| AudioStorageError::Allocation)?;
        result.valid = true;
        result.summary()?;
        Ok(result)
    }
    /// No allocation or owning observation. Includes this ledger's inline body.
    /// A bootstrap must still bind/charge ALL old inputs and overlap separately;
    /// an unsupported allocator cannot use this requested size as an upper bound.
    pub(crate) fn requested_metadata_bytes(
        reservation: AudioStorageLedgerReservation,
    ) -> Result<usize, AudioStorageError> {
        [
            reservation.data_rows.checked_mul(size_of::<Data<'_>>()),
            reservation.handle_rows.checked_mul(size_of::<Handle<'_>>()),
            reservation.root_rows.checked_mul(size_of::<Root<'_>>()),
        ]
        .into_iter()
        .try_fold(size_of::<Self>(), |sum, bytes| {
            sum.checked_add(bytes.ok_or(AudioStorageError::Overflow)?)
                .ok_or(AudioStorageError::Overflow)
        })
    }
    pub fn borrow_custody_on_control(
        &mut self,
        custody: &'a AudioPreparationCustody,
        role: AudioStorageRole,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|this| {
            this.root(custody, role)?;
            custody.visit_audio_storage(this, role)
        })
    }
    pub fn borrow_caller_inputs_on_control(
        &mut self,
        inputs: &'a OwnedAudioInputs,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|this| {
            this.root(inputs, AudioStorageRole::Caller)?;
            this.inputs(inputs)
        })
    }
    pub(crate) fn mutate(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<(), AudioStorageError>,
    ) -> Result<(), AudioStorageError> {
        if !self.valid {
            return Err(AudioStorageError::UnknownStorage);
        }
        self.valid = false;
        f(self)?;
        self.valid = true;
        let completion = if self.arithmetic.is_some() {
            self.arithmetic_completion()
        } else {
            self.summary().map(|_| ())
        };
        if let Err(error) = completion {
            self.valid = false;
            return Err(error);
        }
        Ok(())
    }
    /// Check the concrete new Sampler context allowance against this still
    /// borrowed selected union BEFORE its constructor runs. This is a local
    /// reservation check, not closure of unvisited compiler/application roots,
    /// an observed construction peak, or publication authority.
    pub(crate) fn preflight_sampler_context_construction_on_control(
        &self,
        project: &hex_project::Project,
        resources: Option<crate::native_device_resources::DeviceResourcePreparation<'_>>,
    ) -> Result<crate::shared_analytic::SamplerContextConstructionLimits, AudioStorageError> {
        let current = self.summary()?;
        current
            .require_known_storage()
            .map_err(|_| AudioStorageError::UnknownStorage)?;
        self.validate_selected_shared_handles_on_control()?;
        let new =
            crate::shared_analytic::SharedAnalyticContextTable::sampler_construction_allowance(
                project, resources,
            )
            .map_err(|_| AudioStorageError::UnsupportedGraph)?;
        let retained = current
            .retained_bytes
            .checked_add(new.retained_bytes)
            .ok_or(AudioStorageError::Overflow)?;
        let peak = current
            .construction_peak_bytes
            .checked_add(new.construction_peak_bytes)
            .ok_or(AudioStorageError::Overflow)?;
        if retained > self.reservation.max_retained_bytes
            || peak > self.reservation.max_construction_peak_bytes
        {
            return Err(AudioStorageError::ReservationExceeded);
        }
        Ok(new)
    }

    /// Known requested DATA plus explicit uncertainty. A partial invalidated
    /// inventory has no usable totals. This is neither allocator/RSS nor a
    /// lifetime proof, and excludes unselected external application objects.
    pub fn summary(&self) -> Result<AudioRootStorageSummary, AudioStorageError> {
        if !self.valid || self.arithmetic.is_some() {
            return Err(AudioStorageError::UnknownStorage);
        }
        for handle in &self.handles {
            if !self.data.iter().any(|row| row.owner.same(handle.owner)) {
                return Err(AudioStorageError::UnknownStorage);
            }
        }
        let data_bytes = self.data.iter().try_fold(0usize, |n, row| {
            n.checked_add(row.declaration.bytes)
                .ok_or(AudioStorageError::Overflow)
        })?;
        let ledger_bytes = [
            self.data
                .capacity()
                .checked_mul(std::mem::size_of::<Data<'_>>()),
            self.handles
                .capacity()
                .checked_mul(std::mem::size_of::<Handle<'_>>()),
            self.roots
                .capacity()
                .checked_mul(std::mem::size_of::<Root<'_>>()),
            Some(std::mem::size_of::<Self>()),
        ]
        .into_iter()
        .try_fold(0usize, |n, v| {
            n.checked_add(v.ok_or(AudioStorageError::Overflow)?)
                .ok_or(AudioStorageError::Overflow)
        })?;
        let retained_bytes = data_bytes
            .checked_add(ledger_bytes)
            .ok_or(AudioStorageError::Overflow)?;
        let construction_peak_bytes = retained_bytes
            .checked_add(self.growth)
            .ok_or(AudioStorageError::Overflow)?;
        if retained_bytes > self.reservation.max_retained_bytes
            || construction_peak_bytes > self.reservation.max_construction_peak_bytes
        {
            return Err(AudioStorageError::ReservationExceeded);
        }
        Ok(AudioRootStorageSummary {
            data_allocations: self
                .data
                .iter()
                .filter(|row| {
                    row.declaration.bytes != 0 && row.location != Location("root-inline", 0)
                })
                .count(),
            unresolved_handles: self.handles.len(),
            unresolved_retirements: self.roots.len(),
            unknown_storage: self.unknown,
            data_bytes,
            ledger_bytes,
            retained_bytes,
            construction_peak_bytes,
        })
    }
    /// Metadata facts only: no source owner, identity mint, lease or Arc escapes.
    /// Equal values from distinct allocations deliberately remain distinct rows.
    pub fn streamed_sources(&self) -> impl Iterator<Item = &'a StreamedSourceMetadata> + '_ {
        self.data.iter().filter_map(|row| row.streamed)
    }
    pub fn source_domain_obligations(
        &self,
    ) -> impl Iterator<Item = StreamedSourceBindingObligation<'a>> + '_ {
        self.data.iter().filter_map(|row| {
            row.streamed
                .map(|metadata| StreamedSourceBindingObligation {
                    metadata,
                    owning_handles: self
                        .handles
                        .iter()
                        .filter(|handle| handle.owner.same(row.owner))
                        .count(),
                })
        })
    }
    /// Every independent streamed-metadata owning handle remains a separate
    /// source/domain lifetime-binding obligation, even when DATA deduplicates.
    pub fn streamed_handle_obligations(&self) -> usize {
        self.handles
            .iter()
            .filter(|handle| {
                self.data
                    .iter()
                    .any(|row| row.streamed.is_some() && row.owner.same(handle.owner))
            })
            .count()
    }
    /// Actual ledger buffers remain observable even after a refused partial
    /// addition. These are metadata capacities, never a valid graph total.
    pub fn metadata_capacity_bytes(&self) -> Option<usize> {
        self.data
            .capacity()
            .checked_mul(std::mem::size_of::<Data<'_>>())?
            .checked_add(
                self.handles
                    .capacity()
                    .checked_mul(std::mem::size_of::<Handle<'_>>())?,
            )?
            .checked_add(
                self.roots
                    .capacity()
                    .checked_mul(std::mem::size_of::<Root<'_>>())?,
            )?
            .checked_add(std::mem::size_of::<Self>())
    }
    /// Selected actual field identity, used when immutable source templates are
    /// shared across revisions. This cannot declare an unseen allocation.
    pub(crate) fn skip_observed_root<T: 'static>(&self, value: &'a T) -> bool {
        // Arithmetic traversals must inspect descendants on EVERY visit. This
        // is a traversal policy, never a negative membership/provenance fact.
        if self.arithmetic.is_some() {
            return false;
        }
        let owner = Owner::exclusive(value);
        self.roots.iter().any(|row| row.owner.same(owner))
    }
    pub(crate) fn root<T: 'static>(
        &mut self,
        value: &'a T,
        role: AudioStorageRole,
    ) -> Result<(), AudioStorageError> {
        let owner = Owner::exclusive(value);
        if let Some(arithmetic) = &mut self.arithmetic {
            arithmetic.root()?;
            return self.bytes(owner, Location("root-inline", 0), size_of::<T>());
        }
        if self.roots.iter().any(|row| row.owner.same(owner)) {
            return Err(AudioStorageError::SourceMismatch);
        }
        if self.roots.len() == self.reservation.root_rows {
            return Err(AudioStorageError::Capacity);
        }
        self.roots.push(Root { owner, _role: role });
        self.bytes(owner, Location("root-inline", 0), std::mem::size_of::<T>())
    }
    fn data(
        &mut self,
        owner: Owner<'a>,
        location: Location,
        declaration: Declaration,
        streamed: Option<&'a StreamedSourceMetadata>,
    ) -> Result<(), AudioStorageError> {
        if let Some(arithmetic) = &mut self.arithmetic {
            return arithmetic.data(owner, location, declaration);
        }
        if let Some(old) = self
            .data
            .iter()
            .find(|row| row.owner.same(owner) && row.location == location)
        {
            if old.declaration != declaration {
                return Err(AudioStorageError::UnknownStorage);
            }
            return Ok(());
        }
        if self.data.len() == self.reservation.data_rows {
            return Err(AudioStorageError::Capacity);
        }
        self.data.push(Data {
            owner,
            location,
            declaration,
            streamed,
        });
        Ok(())
    }
    pub(crate) fn allocation(
        &mut self,
        owner: Owner<'a>,
        location: Location,
        capacity: usize,
        element_bytes: usize,
    ) -> Result<(), AudioStorageError> {
        let bytes = capacity
            .checked_mul(element_bytes)
            .ok_or(AudioStorageError::Overflow)?;
        self.data(
            owner,
            location,
            Declaration {
                bytes,
                capacity,
                element_bytes,
                overhead: 0,
            },
            None,
        )
    }
    pub(crate) fn bytes(
        &mut self,
        owner: Owner<'a>,
        location: Location,
        bytes: usize,
    ) -> Result<(), AudioStorageError> {
        self.data(
            owner,
            location,
            Declaration {
                bytes,
                capacity: bytes,
                element_bytes: 1,
                overhead: 0,
            },
            None,
        )
    }
    pub(crate) fn vec<T>(&mut self, value: &'a Vec<T>) -> Result<(), AudioStorageError> {
        let bytes = value
            .capacity()
            .checked_mul(std::mem::size_of::<T>())
            .ok_or(AudioStorageError::Overflow)?;
        self.data(
            Owner::exclusive_vec(value),
            Location("vec-buffer", 0),
            Declaration {
                bytes,
                capacity: value.capacity(),
                element_bytes: std::mem::size_of::<T>(),
                overhead: 0,
            },
            None,
        )
    }
    pub(crate) fn entries<K: 'static, V: 'static>(
        &mut self,
        value: &'a hex_project::SortedEntries<K, V>,
    ) -> Result<(), AudioStorageError> {
        self.allocation(
            Owner::exclusive(value),
            Location("sorted-entry-buffer", 0),
            value.capacity(),
            std::mem::size_of::<(K, V)>(),
        )
    }
    pub(crate) fn string(&mut self, value: &'a String) -> Result<(), AudioStorageError> {
        self.bytes(
            Owner::exclusive(value),
            Location("string-buffer", 0),
            value.capacity(),
        )
    }
    pub(crate) fn shared<T: 'static + ?Sized>(
        &mut self,
        owner: &'a Arc<T>,
        parts: Option<(usize, usize)>,
        field: Owner<'a>,
        handle: u64,
        streamed: Option<&'a StreamedSourceMetadata>,
    ) -> Result<(), AudioStorageError> {
        let (payload, overhead) = parts.ok_or(AudioStorageError::UnknownStorage)?;
        if Arc::weak_count(owner) != 0 {
            return Err(AudioStorageError::UnknownStorage);
        }
        let anchor = Owner::shared(owner);
        self.data(
            anchor,
            Location("shared-owner", 0),
            Declaration {
                bytes: payload
                    .checked_add(overhead)
                    .ok_or(AudioStorageError::Overflow)?,
                capacity: 1,
                element_bytes: payload,
                overhead,
            },
            streamed,
        )?;
        self.handle(anchor, field, handle)
    }
    pub(crate) fn skip_observed_shared_contents<T: 'static + ?Sized>(
        &self,
        owner: &Arc<T>,
    ) -> bool {
        if self.arithmetic.is_some() {
            return false; // never prune shared descendants in arithmetic mode
        }
        let owner = Owner::shared(owner);
        self.data.iter().any(|row| row.owner.same(owner))
    }
    pub(crate) fn skip_observed_shared_field<T: 'static>(&self, value: &'a T) -> bool {
        if self.arithmetic.is_some() {
            return false; // visit and validate embedded fields on every pass
        }
        let field = Owner::exclusive(value);
        self.handles
            .iter()
            .any(|row| row.field.same(field) && row.location == Location("handle-field", 0))
    }
    /// An additional physical field may only refer to DATA already declared by
    /// the actual constructor custody. Equal contents never supply that link.
    pub(crate) fn existing_shared<T: 'static + ?Sized>(
        &mut self,
        owner: &'a Arc<T>,
        field: Owner<'a>,
        location: Location,
    ) -> Result<(), AudioStorageError> {
        let owner = Owner::shared(owner);
        if let Some(arithmetic) = &mut self.arithmetic {
            arithmetic.existing(owner)?;
        } else if !self.data.iter().any(|row| row.owner.same(owner)) {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.observe_handle_at(owner, field, location)
    }
    /// Count observation detects excess caller handles only. It cannot create
    /// a declaration, prove provenance or grant retirement/publication rights.
    pub(crate) fn validate_closed_shared_handles(&self) -> Result<(), AudioStorageError> {
        self.summary()?;
        for row in &self.data {
            if let Some((strong, weak)) = row.owner.actual_counts()? {
                let declared = self
                    .handles
                    .iter()
                    .filter(|h| h.owner.same(row.owner))
                    .count();
                if strong != declared || weak != 0 {
                    return Err(AudioStorageError::UnknownStorage);
                }
            }
        }
        Ok(())
    }
    /// Control-side closure of selected actual constructor-paired handles.
    /// Counter equality cannot create provenance, prove retirement, cover
    /// unselected allocations or grant publication/runtime admission rights.
    pub fn validate_selected_shared_handles_on_control(&self) -> Result<(), AudioStorageError> {
        self.validate_closed_shared_handles()
    }
    /// Observed distinct shared allocations and selected physical handles.
    /// Includes root/header owners as well as PCM; not allocator RSS, complete
    /// host storage, retirement proof or permission to publish/process.
    pub fn selected_shared_counts_on_control(&self) -> (usize, usize) {
        self.shared_resource_counts()
    }
    pub(crate) fn shared_resource_counts(&self) -> (usize, usize) {
        let allocations = self
            .data
            .iter()
            .enumerate()
            .filter(|(index, row)| {
                (row.owner.counts.is_some() || row.owner.shared_field.is_some())
                    && !self.data[..*index]
                        .iter()
                        .any(|old| old.owner.same(row.owner))
            })
            .count();
        (allocations, self.handles.len())
    }
    pub(crate) fn handle(
        &mut self,
        owner: Owner<'a>,
        field: Owner<'a>,
        id: u64,
    ) -> Result<(), AudioStorageError> {
        self.handle_at(owner, field, Location("handle-field", 0), id)
    }
    pub(crate) fn handle_at(
        &mut self,
        owner: Owner<'a>,
        field: Owner<'a>,
        location: Location,
        id: u64,
    ) -> Result<(), AudioStorageError> {
        if let Some(arithmetic) = &mut self.arithmetic {
            // This is an existing constructor-paired identity, not a minted
            // observation. Preserve its nonzero provenance check in bootstrap.
            if id == 0 {
                return Err(AudioStorageError::SourceMismatch);
            }
            return arithmetic.handle();
        }
        if id == 0
            || self
                .handles
                .iter()
                .any(|row| row.id == id || (row.field.same(field) && row.location == location))
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        if self.handles.len() == self.reservation.handle_rows {
            return Err(AudioStorageError::Capacity);
        }
        self.handles.push(Handle {
            owner,
            field,
            location,
            id,
        });
        Ok(())
    }
    pub(crate) fn observe_handle_at(
        &mut self,
        owner: Owner<'a>,
        field: Owner<'a>,
        location: Location,
    ) -> Result<(), AudioStorageError> {
        if let Some(arithmetic) = &mut self.arithmetic {
            return arithmetic.handle();
        }
        self.handle_at(owner, field, location, crate::audio_only_inputs::handle()?)
    }
    pub(crate) fn observe_shared<T: 'static + ?Sized>(
        &mut self,
        owner: &'a Arc<T>,
        parts: Option<(usize, usize)>,
        field: Owner<'a>,
    ) -> Result<(), AudioStorageError> {
        let (payload, overhead) = parts.ok_or(AudioStorageError::UnknownStorage)?;
        if Arc::weak_count(owner) != 0 {
            return Err(AudioStorageError::UnknownStorage);
        }
        let anchor = Owner::shared(owner);
        self.data(
            anchor,
            Location("shared-owner", 0),
            Declaration {
                bytes: payload
                    .checked_add(overhead)
                    .ok_or(AudioStorageError::Overflow)?,
                capacity: 1,
                element_bytes: payload,
                overhead,
            },
            None,
        )?;
        self.observe_handle_at(anchor, field, Location("handle-field", 0))
    }
    // Actual non-static paired metadata projection. Same normal ledger DATA
    // and physical handle fields as the existing static visitor; no adoption.
    pub(crate) fn sampler_shared_field(
        &mut self,
        allocation: &'a dyn crate::sampler_host_fields::SharedLayout,
        parts: Option<(usize, usize)>,
        field: Owner<'a>,
    ) -> Result<(), AudioStorageError> {
        let (payload, overhead) = parts.ok_or(AudioStorageError::UnknownStorage)?;
        if allocation.counts().1 != 0 {
            return Err(AudioStorageError::UnknownStorage);
        }
        let owner = Owner::borrowed_shared(allocation);
        self.data(
            owner,
            Location("shared-owner", 0),
            Declaration {
                bytes: payload
                    .checked_add(overhead)
                    .ok_or(AudioStorageError::Overflow)?,
                capacity: 1,
                element_bytes: payload,
                overhead,
            },
            None,
        )?;
        if self.arithmetic.is_none()
            && self
                .handles
                .iter()
                .any(|row| row.field.same(field) && row.location == Location("handle-field", 0))
        {
            return Ok(());
        }
        self.observe_handle_at(owner, field, Location("handle-field", 0))
    }
    pub(crate) fn growth(&mut self, bytes: usize) -> Result<(), AudioStorageError> {
        self.growth = self
            .growth
            .checked_add(bytes)
            .ok_or(AudioStorageError::Overflow)?;
        Ok(())
    }
    pub(crate) fn unknown(&mut self) -> Result<(), AudioStorageError> {
        self.unknown = self
            .unknown
            .checked_add(1)
            .ok_or(AudioStorageError::Overflow)?;
        Ok(())
    }
}

/// Read-only compiler/stdlib declaration gate. Numeric layout evidence is not
/// an owner receipt: callers cannot insert rows or upgrade raw/provider owners.
pub fn audio_storage_constructor_layout_supported() -> bool {
    crate::pcm_arc::native_layout_known()
}

#[cfg(test)]
#[path = "audio_storage/state_metadata_tests.rs"]
mod state_metadata_tests;

#[cfg(test)]
mod vec_backing_tests;
