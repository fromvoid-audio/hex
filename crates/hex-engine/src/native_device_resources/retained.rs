//! Existing constructor-paired shared ownership around the actual immutable input bank.
//! No publication permission: host integration must inventory every live role and retire it.
use super::*;
use crate::{AudioRootStorageLedger, AudioStorageRole, owned_shared::OwnedShared};

struct ResourceRoot {
    bank: AudioInputStorageAndObligations,
    catalog: DeviceResourceCatalog,
}
#[cfg(test)]
thread_local! {
    pub(crate) static RETIRED_RESOURCE_ROOTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
#[cfg(test)]
impl Drop for ResourceRoot {
    fn drop(&mut self) {
        RETIRED_RESOURCE_ROOTS.with(|count| count.set(count.get() + 1));
    }
}
/// Control-side source custody. No Clone, raw Arc, PCM owner or weak handle escapes.
pub struct DeviceResourceBank {
    root: OwnedShared<ResourceRoot>,
    handle: u64,
}
/// The original actual bank and facts are returned intact on a reported refusal.
pub struct DeviceResourceBankRefusal {
    pub reason: ResourcePreparationError,
    pub bank: AudioInputStorageAndObligations,
    pub catalog: DeviceResourceCatalog,
}
/// Host-owned lease for ONE prepared configuration. Provider coefficients do not own it.
pub struct PreparedDeviceResourceLease {
    root: OwnedShared<ResourceRoot>,
    scope: DeviceResourceScope,
    handle: u64,
}
/// Closed original-source view for media/history/waveform readers. This is a
/// handle into the SAME resource root, with no raw Audio/Arc escape, new cache,
/// PCM allocation, authored device slot or provider-owned payload.
/// Owning clone/drop belongs on control or its existing retirement path.
pub struct OriginalSourceLease {
    root: OwnedShared<ResourceRoot>,
    source: usize,
    handle: u64,
}
impl std::fmt::Debug for OriginalSourceLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OriginalSourceLease")
            .field("source", &self.source_facts())
            .finish_non_exhaustive()
    }
}
impl DeviceResourceBank {
    pub fn borrow_original_source_on_control(
        &self,
        asset: AssetId,
        max_metadata_bytes: usize,
    ) -> Result<OriginalSourceLease, ResourcePreparationError> {
        if std::mem::size_of::<OriginalSourceLease>() > max_metadata_bytes {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let source = self
            .root
            .catalog
            .source_index(asset.value())
            .ok_or(ResourcePreparationError::InvalidSource)?;
        self.root
            .bank
            .source_view(&self.root.catalog.sources[source].token)?;
        let handle = crate::audio_only_inputs::handle()?;
        Ok(OriginalSourceLease {
            root: self.root.clone_on_control(),
            source,
            handle,
        })
    }
    pub fn input_storage_summary(&self) -> crate::AudioInputStorageSummary {
        self.root.bank.summary()
    }
    pub fn try_new_on_control(
        bank: AudioInputStorageAndObligations,
        catalog: DeviceResourceCatalog,
        max_retained_data_bytes: usize,
        max_construction_peak_bytes: usize,
        cancel: &dyn crate::CancellationQuery,
    ) -> Result<Self, Box<DeviceResourceBankRefusal>> {
        let result = (|| {
            cancellation(cancel)?;
            bank.validate_unresolved_on_control()?;
            for source in &catalog.sources {
                cancellation(cancel)?;
                bank.source_view(&source.token)?;
            }
            let (payload, overhead) = OwnedShared::<ResourceRoot>::declared_parts()
                .ok_or(AudioStorageError::UnsupportedConstructor)?;
            let bytes = bank
                .summary()
                .retained_data_bytes
                .checked_sub(std::mem::size_of::<AudioInputStorageAndObligations>())
                .and_then(|n| n.checked_add(payload))
                .and_then(|n| n.checked_add(overhead))
                .and_then(|n| n.checked_add(std::mem::size_of::<Self>()))
                .and_then(|n| {
                    n.checked_add(
                        catalog
                            .sources
                            .capacity()
                            .checked_mul(std::mem::size_of::<SourceFact>())?,
                    )
                })
                .ok_or(AudioStorageError::Overflow)?;
            let peak = bank
                .summary()
                .construction_peak_bytes
                .checked_add(
                    bytes
                        .checked_sub(bank.summary().retained_data_bytes)
                        .ok_or(AudioStorageError::Overflow)?,
                )
                .ok_or(AudioStorageError::Overflow)?;
            if bytes > max_retained_data_bytes || peak > max_construction_peak_bytes {
                return Err(AudioStorageError::ReservationExceeded.into());
            }
            cancellation(cancel)?;
            Ok(crate::audio_only_inputs::handle()?)
        })();
        match result {
            Ok(handle) => Ok(Self {
                root: OwnedShared::new_owned(ResourceRoot { bank, catalog }),
                handle,
            }),
            Err(reason) => Err(Box::new(DeviceResourceBankRefusal {
                reason,
                bank,
                catalog,
            })),
        }
    }
    /// Reserve scope storage and a fresh physical handle before cloning on control.
    /// This does not reserve a host's cross-revision data or retirement capacity.
    pub fn prepare_lease_on_control(
        &self,
        state: &NativeDeviceState,
        max_metadata_bytes: usize,
    ) -> Result<PreparedDeviceResourceLease, ResourcePreparationError> {
        let scope_budget = max_metadata_bytes
            .checked_sub(std::mem::size_of::<PreparedDeviceResourceLease>())
            .and_then(|n| n.checked_add(std::mem::size_of::<DeviceResourceScope>()))
            .ok_or(ResourcePreparationError::MetadataCapacity)?;
        let scope = self
            .root
            .catalog
            .prepare_scope_on_control(state, scope_budget)?;
        self.root.catalog.resources(&self.root.bank, &scope)?;
        let handle = crate::audio_only_inputs::handle()?;
        Ok(PreparedDeviceResourceLease {
            root: self.root.clone_on_control(),
            scope,
            handle,
        })
    }
}
impl DeviceResources for PreparedDeviceResourceLease {
    fn len(&self) -> usize {
        self.scope.bindings.len()
    }
    fn get(&self, index: usize) -> Option<DeviceResourceView<'_>> {
        // All members are private, immutable and were verified before the owning clone.
        // O(1) token checks, with no whole-scope validation on each sampled frame.
        ScopedDeviceResources {
            bank: &self.root.bank,
            catalog: &self.root.catalog,
            scope: &self.scope,
        }
        .view(index)
    }
}
fn visit<'a>(
    root: &'a OwnedShared<ResourceRoot>,
    field: crate::audio_storage::Owner<'a>,
    handle: u64,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    let visited = ledger.contains_shared_storage(root.arc());
    ledger.shared(root.arc(), root.parts(), field, handle, None)?;
    if !visited {
        let ResourceRoot { bank, catalog } = root.as_ref();
        bank.visit_resource_storage(ledger)?;
        let DeviceResourceCatalog { id: _, sources } = catalog;
        ledger.vec(sources)?;
    }
    Ok(())
}
impl<'a> AudioRootStorageLedger<'a> {
    /// Actual retained lease field embedded in an already charged metadata
    /// body. Repeated borrows of that same field do not create extra handles or
    /// charge its inline header twice. All existing root/PCM checks still apply.
    pub fn borrow_embedded_original_source_lease_on_control(
        &mut self,
        lease: &'a OriginalSourceLease,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| {
            if ledger.contains_shared_field(lease) {
                return Ok(());
            }
            visit(
                &lease.root,
                crate::audio_storage::Owner::exclusive(lease),
                lease.handle,
                ledger,
            )
        })
    }
    pub fn borrow_original_source_lease_on_control(
        &mut self,
        lease: &'a OriginalSourceLease,
        role: AudioStorageRole,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| {
            ledger.root(lease, role)?;
            visit(
                &lease.root,
                crate::audio_storage::Owner::exclusive(lease),
                lease.handle,
                ledger,
            )
        })
    }
    pub fn borrow_device_resource_bank_on_control(
        &mut self,
        bank: &'a DeviceResourceBank,
        role: AudioStorageRole,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| {
            ledger.root(bank, role)?;
            visit(
                &bank.root,
                crate::audio_storage::Owner::exclusive(bank),
                bank.handle,
                ledger,
            )
        })
    }
    pub fn borrow_device_resource_lease_on_control(
        &mut self,
        lease: &'a PreparedDeviceResourceLease,
        role: AudioStorageRole,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| {
            ledger.root(lease, role)?;
            lease.visit_storage(ledger)
        })
    }
}

impl OriginalSourceLease {
    /// Reuse original PCM in a new prepared input bank without a second buffer.
    /// The result stays in the existing closed source family. Caller must reserve
    /// the destination bank and all coexistence roles before publication; an
    /// unselected returned owner prevents closed-handle validation of this root.
    pub fn alias_for_resource_plan_on_control(
        &self,
        max_metadata_bytes: usize,
    ) -> Result<crate::OwnedAudioSource, ResourcePreparationError> {
        if std::mem::size_of::<crate::OwnedAudioSource>() > max_metadata_bytes {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let source = self
            .root
            .catalog
            .sources
            .get(self.source)
            .ok_or(ResourcePreparationError::InvalidSource)?;
        Ok(self
            .root
            .bank
            .alias_resource_source_on_control(&source.token)?)
    }
    /// O(1) immutable borrow, tied to this actual retained root. No owner clone,
    /// hash, allocation, decode, wait or temporary-owner lifetime is involved.
    pub fn view(&self) -> Result<crate::OwnedAudioSourceView<'_>, AudioStorageError> {
        let fact = self
            .root
            .catalog
            .sources
            .get(self.source)
            .ok_or(AudioStorageError::SourceMismatch)?;
        self.root.bank.source_view(&fact.token)
    }
    pub fn source_facts(&self) -> Result<OriginalResourceSource, AudioStorageError> {
        self.root
            .catalog
            .sources
            .get(self.source)
            .map(|fact| fact.expected)
            .ok_or(AudioStorageError::SourceMismatch)
    }
    /// Equality of actual selected root/source custody, for control reuse only.
    /// This is not an ownership or publication proof.
    pub fn shares_source_with(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(self.root.arc(), other.root.arc()) && self.source == other.source
    }
    /// Caller must reserve aggregate coexistence/retirement capacity separately;
    /// this quotes the exact handle header before the control-only owning clone.
    pub fn try_clone_on_control(
        &self,
        max_metadata_bytes: usize,
    ) -> Result<Self, ResourcePreparationError> {
        if std::mem::size_of::<Self>() > max_metadata_bytes {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let handle = crate::audio_only_inputs::handle()?;
        Ok(Self {
            root: self.root.clone_on_control(),
            source: self.source,
            handle,
        })
    }
}
impl PreparedDeviceResourceLease {
    pub(crate) fn same_configuration(&self, other: &Self) -> bool {
        self.root.catalog.id == other.root.catalog.id && self.scope.bindings == other.scope.bindings
    }
    pub fn declared_metadata_bytes(&self) -> Result<usize, ResourcePreparationError> {
        quoted_bytes::<ScopeBinding>(self.scope.bindings.capacity(), std::mem::size_of::<Self>())
    }
    pub(crate) fn source_scope_identity(&self) -> u64 {
        self.root.catalog.id
    }
    pub(crate) fn visit_storage<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        visit(
            &self.root,
            crate::audio_storage::Owner::exclusive(self),
            self.handle,
            ledger,
        )?;
        let DeviceResourceScope {
            catalog: _,
            bindings,
        } = &self.scope;
        ledger.vec(bindings)
    }
}

impl hex_lattice::HostDeviceResourceScope for PreparedDeviceResourceLease {
    fn same_configuration(&self, other: &Self) -> bool {
        PreparedDeviceResourceLease::same_configuration(self, other)
    }
    fn validate_configuration_on_control(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<(), hex_lattice_contract::DeviceContractError> {
        use hex_lattice_contract::DeviceContractError;
        if sample_rate == 0 {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        if state.asset_references().len() != self.scope.bindings.len() {
            return Err(DeviceContractError::AssetResourcesUnavailable);
        }
        // This closed root owns immutable native originals. Catalog preparation
        // already validated every finite sample under explicit work/cancel bounds.
        // Recheck authored slots and actual retained tokens/rate/layout/window,
        // not every PCM frame again for each configured effect/checkpoint.
        for (index, reference) in state.asset_references().iter().enumerate() {
            let view = self
                .get(index)
                .ok_or(DeviceContractError::AssetResourcesUnavailable)?;
            if view.slot != reference.slot || view.identity.asset != reference.asset {
                return Err(DeviceContractError::AssetResourcesUnavailable);
            }
        }
        Ok(())
    }
}

/// Immutable configuration custody shared only by actual host owners. The
/// existing OwnedShared constructor retains one lease/binding vector; aliases
/// do not create another PCM store. Control keeps an anchor while the runtime
/// is active, so union inventory never borrows mutable callback state.
/// This does not grant publication or permit callback-side owning clone/drop.
pub struct DeviceResourceConfiguration {
    lease: OwnedShared<PreparedDeviceResourceLease>,
    handle: u64,
}
impl DeviceResourceBank {
    pub fn prepare_configuration_on_control(
        &self,
        state: &NativeDeviceState,
        max_metadata_bytes: usize,
    ) -> Result<DeviceResourceConfiguration, ResourcePreparationError> {
        let (_, overhead) = OwnedShared::<PreparedDeviceResourceLease>::declared_parts()
            .ok_or(AudioStorageError::UnknownStorage)?;
        let extra = overhead
            .checked_add(std::mem::size_of::<DeviceResourceConfiguration>())
            .ok_or(AudioStorageError::Overflow)?;
        let lease_budget = max_metadata_bytes
            .checked_sub(extra)
            .ok_or(ResourcePreparationError::MetadataCapacity)?;
        let lease = self.prepare_lease_on_control(state, lease_budget)?;
        lease
            .try_into_configuration_on_control(max_metadata_bytes)
            .map_err(|refusal| refusal.reason)
    }
}
impl std::fmt::Debug for DeviceResourceConfiguration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeviceResourceConfiguration")
            .field("bindings", &self.len())
            .finish_non_exhaustive()
    }
}
impl DeviceResourceConfiguration {
    /// Quote this physical alias header before taking another owning handle.
    /// Keep and retire the resulting handle through the host's existing paths.
    pub fn try_clone_on_control(
        &self,
        max_metadata_bytes: usize,
    ) -> Result<Self, ResourcePreparationError> {
        if std::mem::size_of::<Self>() > max_metadata_bytes {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let handle = crate::audio_only_inputs::handle()?;
        Ok(Self {
            lease: self.lease.clone_on_control(),
            handle,
        })
    }
    pub fn declared_metadata_bytes(&self) -> Result<usize, ResourcePreparationError> {
        let (_, overhead) = self
            .lease
            .parts()
            .ok_or(AudioStorageError::UnknownStorage)?;
        self.lease
            .declared_metadata_bytes()?
            .checked_add(overhead)
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Self>()))
            .ok_or(AudioStorageError::Overflow.into())
    }
    pub(crate) fn visit_storage<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let visited = ledger.contains_shared_storage(self.lease.arc());
        ledger.shared(
            self.lease.arc(),
            self.lease.parts(),
            crate::audio_storage::Owner::exclusive(self),
            self.handle,
            None,
        )?;
        if !visited {
            self.lease.visit_storage(ledger)?;
        }
        Ok(())
    }
}
impl DeviceResources for DeviceResourceConfiguration {
    fn len(&self) -> usize {
        self.lease.len()
    }
    fn get(&self, index: usize) -> Option<DeviceResourceView<'_>> {
        self.lease.get(index)
    }
}
impl hex_lattice::HostDeviceResourceScope for DeviceResourceConfiguration {
    fn same_configuration(&self, other: &Self) -> bool {
        self.lease.same_configuration(&other.lease)
    }
    fn validate_configuration_on_control(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<(), hex_lattice_contract::DeviceContractError> {
        hex_lattice::HostDeviceResourceScope::validate_configuration_on_control(
            self.lease.as_ref(),
            state,
            sample_rate,
        )
    }
}
impl<'a> AudioRootStorageLedger<'a> {
    pub fn borrow_device_resource_configuration_on_control(
        &mut self,
        configuration: &'a DeviceResourceConfiguration,
        role: AudioStorageRole,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| {
            ledger.root(configuration, role)?;
            configuration.visit_storage(ledger)
        })
    }
}

/// A refused immutable-anchor construction returns the identical original lease.
pub struct ResourceConfigurationRefusal {
    pub reason: ResourcePreparationError,
    pub lease: PreparedDeviceResourceLease,
}
impl PreparedDeviceResourceLease {
    pub fn try_into_configuration_on_control(
        self,
        max_metadata_bytes: usize,
    ) -> Result<DeviceResourceConfiguration, ResourceConfigurationRefusal> {
        let result = (|| {
            let (_, overhead) = OwnedShared::<PreparedDeviceResourceLease>::declared_parts()
                .ok_or(AudioStorageError::UnknownStorage)?;
            let quote = self
                .declared_metadata_bytes()?
                .checked_add(overhead)
                .and_then(|bytes| {
                    bytes.checked_add(std::mem::size_of::<DeviceResourceConfiguration>())
                })
                .ok_or(AudioStorageError::Overflow)?;
            if quote > max_metadata_bytes {
                return Err(ResourcePreparationError::MetadataCapacity);
            }
            crate::audio_only_inputs::handle().map_err(ResourcePreparationError::from)
        })();
        match result {
            Ok(handle) => Ok(DeviceResourceConfiguration {
                lease: OwnedShared::new_owned(self),
                handle,
            }),
            Err(reason) => Err(ResourceConfigurationRefusal {
                reason,
                lease: self,
            }),
        }
    }
}
impl DeviceResourceConfiguration {
    pub(crate) fn source_scope_identity(&self) -> u64 {
        self.lease.source_scope_identity()
    }
    pub(crate) fn same_configuration(&self, other: &Self) -> bool {
        self.lease.same_configuration(&other.lease)
    }
}
