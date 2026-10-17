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
impl DeviceResourceBank {
    pub fn input_storage_summary(&self) -> crate::AudioInputStorageSummary {
        self.root.bank.summary()
    }
    pub fn try_new_on_control(
        bank: AudioInputStorageAndObligations,
        catalog: DeviceResourceCatalog,
        max_retained_data_bytes: usize,
        max_construction_peak_bytes: usize,
        cancel: &AtomicBool,
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
impl PreparedDeviceResourceLease {
    pub(crate) fn try_clone_on_control(
        &self,
        max_metadata_bytes: usize,
    ) -> Result<Self, ResourcePreparationError> {
        if quoted_bytes::<ScopeBinding>(self.scope.bindings.len(), std::mem::size_of::<Self>())?
            > max_metadata_bytes
        {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let mut bindings = Vec::new();
        bindings
            .try_reserve_exact(self.scope.bindings.len())
            .map_err(|_| AudioStorageError::Allocation)?;
        if quoted_bytes::<ScopeBinding>(bindings.capacity(), std::mem::size_of::<Self>())?
            > max_metadata_bytes
        {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        bindings.extend_from_slice(&self.scope.bindings);
        let handle = crate::audio_only_inputs::handle()?;
        Ok(Self {
            root: self.root.clone_on_control(),
            scope: DeviceResourceScope {
                catalog: self.scope.catalog,
                bindings,
            },
            handle,
        })
    }
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
