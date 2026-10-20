//! Finite Status field metadata. The actual Status/route storage stays in the
//! audited Audio Root anchor and is borrowed on EVERY traversal; no saved owner
//! address, erased owning pointer, caller bytes or scalar admission token.
use super::*;
use crate::{
    SharedMetadata,
    sampler_host_fields::{BodyRef, SharedMetadataRef, VecRef},
};
use std::sync::atomic::{AtomicU8, Ordering};
const VACANT: u8 = 0;
const LIVE: u8 = 1;
const DROP_PENDING: u8 = 2;
const DISPOSED: u8 = 3;
const FAILED: u8 = 4;
const ROLES: usize = 8;
struct Fields {
    phases: [AtomicU8; ROLES],
    lineages: [[u64; 2]; ROLES],
}
pub struct NativeStatusRegistration {
    fields: SharedMetadata<Fields>,
}
/// One actual actor's release obligation, moved rather than cloned.
/// ```compile_fail
/// let record = hex_engine::sampler_host_fields::NativeStatusRegistration::new_on_control().unwrap();
/// let field = record.reserve_field_on_control(0).unwrap();
/// let _duplicate = field.clone();
/// ```
pub struct NativeStatusFieldLease {
    fields: SharedMetadata<Fields>,
    role: usize,
}
/// Actual immutable owning Audio archive, not a body-only child certificate.
/// Its producer audits the private Status and every route entry as scalar-only
/// apart from this actual Vec. Engine drives the finite traversal.
pub struct NativeStatusFields<'a> {
    pub body: BodyRef<'a>,
    pub owner: SharedMetadataRef<'a>,
    pub routes: VecRef<'a>,
    pub registration: &'a NativeStatusRegistration,
}
impl<'a> NativeStatusFields<'a> {
    pub fn visit_storage_on_control(
        self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.mutate(|ledger| visit_status(self, ledger))
    }
}
impl NativeStatusRegistration {
    pub fn requested_bytes_on_control() -> Result<usize, AudioStorageError> {
        let (body, header) = crate::owned_shared::OwnedShared::<Fields>::declared_parts()
            .ok_or(AudioStorageError::UnknownStorage)?;
        body.checked_add(header).ok_or(AudioStorageError::Overflow)
    }
    /// Additional physical metadata beside the already-counted Status/routes.
    /// Pending or failed field destruction cannot be priced as reusable storage.
    pub fn retained_metadata_bytes_on_control(&self) -> Result<usize, AudioStorageError> {
        if self
            .fields
            .phases
            .iter()
            .any(|phase| !matches!(phase.load(Ordering::Acquire), VACANT | LIVE | DISPOSED))
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        Self::requested_bytes_on_control()
    }
    pub fn construction_scratch_bytes_on_control() -> Result<usize, AudioStorageError> {
        size_of::<Fields>()
            .checked_add(size_of::<Self>())
            .ok_or(AudioStorageError::Overflow)
    }
    /// Metadata only. Owning fields must be constructed/checked beside the real
    /// qualified Status anchor by the private audited Audio constructor.
    pub fn new_on_control() -> Result<Self, AudioStorageError> {
        Self::requested_bytes_on_control()?;
        let mut lineages = [[0; 2]; ROLES];
        for pair in &mut lineages {
            for id in pair {
                *id = crate::audio_only_inputs::handle()?;
            }
        }
        Ok(Self {
            fields: SharedMetadata::new_on_control(Fields {
                phases: std::array::from_fn(|_| AtomicU8::new(VACANT)),
                lineages,
            }),
        })
    }
    /// Actual Root exclusion remains held by the caller. There is exactly one
    /// reservation attempt and no fallible/allocation work after the live CAS
    /// before constructing the lease. Audio immediately clones the SAME owned
    /// Status anchor (nonallocating), without an intervening external callback.
    pub fn reserve_field_on_control(
        &self,
        role: usize,
    ) -> Result<NativeStatusFieldLease, AudioStorageError> {
        let phase = self
            .fields
            .phases
            .get(role)
            .ok_or(AudioStorageError::SourceMismatch)?;
        if self
            .fields
            .phases
            .iter()
            .any(|p| matches!(p.load(Ordering::Acquire), DROP_PENDING | FAILED))
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        phase
            .compare_exchange(VACANT, LIVE, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| AudioStorageError::SourceMismatch)?;
        Ok(NativeStatusFieldLease {
            fields: self.fields.clone(),
            role,
        })
    }
    pub fn matches_actual_lease_on_control(&self, lease: &NativeStatusFieldLease) -> bool {
        self.matches_actual_field_role_on_control(lease, lease.role)
    }
    /// Exact actual field role at the audited constructor boundary, not a
    /// permission to adopt a Status or infer its owning descendants absent.
    pub fn matches_actual_field_role_on_control(
        &self,
        lease: &NativeStatusFieldLease,
        role: usize,
    ) -> bool {
        role < ROLES
            && lease.role == role
            && self.fields.shares_owner_with(&lease.fields)
            && self.fields.phases[role].load(Ordering::Acquire) == LIVE
    }
    pub(crate) fn visit_fields<'a>(
        &'a self,
        owner: SharedMetadataRef<'a>,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let Fields { phases, lineages } = self.fields.as_ref();
        let phases = phases.each_ref().map(|p| p.load(Ordering::Acquire));
        if phases
            .iter()
            .any(|p| !matches!(*p, VACANT | LIVE | DISPOSED))
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        let paired = self.fields.paired_owner_on_control();
        if !ledger.skip_observed_shared_field(&self.fields) {
            ledger.observe_shared(paired.arc(), paired.parts(), Owner::exclusive(&self.fields))?;
        }
        let metadata = Owner::shared(paired.arc());
        let data = owner.constructor_owner()?;
        for (role, phase) in phases.into_iter().enumerate() {
            if phase != LIVE {
                continue;
            }
            for (field, owner) in [data, metadata].into_iter().enumerate() {
                if let Some(arithmetic) = &mut ledger.arithmetic {
                    arithmetic.existing(owner)?;
                }
                ledger.handle_at(
                    owner,
                    Owner::exclusive(&lineages[role][field]),
                    Location("native-status-field", role * 2 + field),
                    lineages[role][field],
                )?;
            }
        }
        Ok(())
    }
}
impl NativeStatusFieldLease {
    pub fn begin_actual_field_drop_on_control(&mut self) -> Result<(), AudioStorageError> {
        let phase = &self.fields.phases[self.role];
        if phase.swap(DROP_PENDING, Ordering::AcqRel) != LIVE {
            phase.store(FAILED, Ordering::Release);
            return Err(AudioStorageError::UnknownStorage);
        }
        Ok(())
    }
    pub fn complete_actual_field_drop_on_control(&mut self) -> Result<(), AudioStorageError> {
        let phase = &self.fields.phases[self.role];
        if phase.load(Ordering::Acquire) != DROP_PENDING {
            phase.store(FAILED, Ordering::Release);
            return Err(AudioStorageError::UnknownStorage);
        }
        phase.store(DISPOSED, Ordering::Release);
        Ok(())
    }
}
impl Drop for NativeStatusFieldLease {
    fn drop(&mut self) {
        if self.fields.phases[self.role].load(Ordering::Acquire) != DISPOSED {
            self.fields.phases[self.role].store(FAILED, Ordering::Release);
        }
    }
}
pub(crate) fn visit_status<'a>(
    fields: NativeStatusFields<'a>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    if !fields.body.contains(fields.owner.body())
        || !fields.body.contains(BodyRef::of(fields.registration))
        || !fields.owner.payload().contains(fields.routes.body())
    {
        return Err(AudioStorageError::SourceMismatch);
    }
    fields.owner.observe(ledger)?;
    fields.routes.observe(ledger)?;
    fields.registration.visit_fields(fields.owner, ledger)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_birth_is_exact_and_normal_role_release_has_no_hidden_allocation() {
        let mut record = None;
        let measured = allocation_counter::measure(|| {
            record = Some(NativeStatusRegistration::new_on_control().unwrap());
        });
        let bytes = NativeStatusRegistration::requested_bytes_on_control().unwrap();
        assert_eq!(
            (measured.count_total, measured.bytes_total),
            (1, bytes as u64)
        );
        let record = record.unwrap();
        let measured = allocation_counter::measure(|| {
            let mut role = record.reserve_field_on_control(0).unwrap();
            role.begin_actual_field_drop_on_control().unwrap();
            role.complete_actual_field_drop_on_control().unwrap();
            drop(role);
        });
        assert_eq!((measured.count_total, measured.count_current), (0, 0));
        let measured = allocation_counter::measure(|| drop(record));
        assert_eq!(
            (measured.count_current, measured.bytes_current),
            (-1, -(bytes as i64))
        );
    }
    #[test]
    fn interrupted_status_release_never_prices_or_reuses_pending_metadata() {
        let registration = NativeStatusRegistration::new_on_control().unwrap();
        let mut field = registration.reserve_field_on_control(0).unwrap();
        assert_eq!(
            registration.retained_metadata_bytes_on_control().unwrap(),
            NativeStatusRegistration::requested_bytes_on_control().unwrap()
        );
        field.begin_actual_field_drop_on_control().unwrap();
        assert!(matches!(
            registration.retained_metadata_bytes_on_control(),
            Err(AudioStorageError::UnknownStorage)
        ));
        assert!(registration.reserve_field_on_control(1).is_err());
        drop(field); // interrupted after pending must remain sticky failed
        assert!(registration.retained_metadata_bytes_on_control().is_err());
        assert!(registration.reserve_field_on_control(0).is_err());
    }
    #[test]
    fn completed_status_field_is_disposed_once_and_metadata_remains_physical() {
        let registration = NativeStatusRegistration::new_on_control().unwrap();
        let mut field = registration.reserve_field_on_control(6).unwrap();
        assert!(registration.reserve_field_on_control(6).is_err());
        field.begin_actual_field_drop_on_control().unwrap();
        field.complete_actual_field_drop_on_control().unwrap();
        drop(field);
        assert_eq!(
            registration.retained_metadata_bytes_on_control().unwrap(),
            NativeStatusRegistration::requested_bytes_on_control().unwrap()
        );
        assert!(registration.reserve_field_on_control(6).is_err());
        let foreign = NativeStatusRegistration::new_on_control().unwrap();
        let field = foreign.reserve_field_on_control(0).unwrap();
        assert!(!registration.matches_actual_lease_on_control(&field));
        drop(field); // only foreign registration fails
        assert!(registration.retained_metadata_bytes_on_control().is_ok());
    }
}
