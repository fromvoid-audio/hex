//! Finite Resident endpoint fields. The actual guarded Slot stays in the
//! audited Audio Root anchor and is borrowed on EVERY traversal; no saved owner
//! address, erased owning pointer, caller bytes or scalar admission token.
use super::*;
use crate::{
    SharedMetadata,
    sampler_host_fields::{BodyRef, SharedMetadataRef},
};
use std::sync::atomic::{AtomicU8, Ordering};
const VACANT: u8 = 0;
const LIVE: u8 = 1;
const DROP_PENDING: u8 = 2;
const DISPOSED: u8 = 3;
const FAILED: u8 = 4;
const ROLES: usize = 2;
struct Fields {
    phases: [AtomicU8; ROLES],
    lineages: [[u64; 2]; ROLES],
}
pub struct NativeResidentRegistration {
    fields: SharedMetadata<Fields>,
}
/// One actual actor's release obligation, moved rather than cloned.
/// ```compile_fail
/// let record = hex_engine::sampler_host_fields::NativeResidentRegistration::new_on_control().unwrap();
/// let field = record.reserve_field_on_control(0).unwrap();
/// let _duplicate = field.clone();
/// ```
pub struct NativeResidentFieldLease {
    fields: SharedMetadata<Fields>,
    role: usize,
}
/// Actual immutable owning Audio archive, not a body-only child certificate.
/// Its producer borrows the real Slot under exclusion. Engine visits BOTH
/// possible runtime descendants; unqualified cancellation explicitly refuses.
pub struct NativeResidentFields<'a> {
    pub body: BodyRef<'a>,
    pub owner: SharedMetadataRef<'a>,
    pub slot: BodyRef<'a>,
    pub request: Option<&'a crate::playback::Playback>,
    pub failed_response: Option<&'a crate::playback::Playback>,
    /// Actual owning field remains unsupported until its paired birth is wired.
    /// Presence is never silently converted to an empty slot.
    pub cancellation: Option<BodyRef<'a>>,
    pub registration: &'a NativeResidentRegistration,
}
impl<'a> NativeResidentFields<'a> {
    pub fn visit_storage_on_control(
        self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.mutate(|ledger| visit_resident(self, ledger))
    }
}
impl NativeResidentRegistration {
    pub fn requested_bytes_on_control() -> Result<usize, AudioStorageError> {
        let (body, header) = crate::owned_shared::OwnedShared::<Fields>::declared_parts()
            .ok_or(AudioStorageError::UnknownStorage)?;
        body.checked_add(header).ok_or(AudioStorageError::Overflow)
    }
    /// Additional physical metadata beside the actual Resident payload.
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
    /// qualified Resident anchor by the private audited Audio constructor.
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
    /// Resident anchor (nonallocating), without an intervening external callback.
    pub fn reserve_field_on_control(
        &self,
        role: usize,
    ) -> Result<NativeResidentFieldLease, AudioStorageError> {
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
        Ok(NativeResidentFieldLease {
            fields: self.fields.clone(),
            role,
        })
    }
    pub fn matches_actual_lease_on_control(&self, lease: &NativeResidentFieldLease) -> bool {
        self.matches_actual_field_role_on_control(lease, lease.role)
    }
    /// Constructor field correlation only; Slot contents still require their
    /// actual guarded traversal and never follow from role/identity matching.
    pub fn matches_actual_field_role_on_control(
        &self,
        lease: &NativeResidentFieldLease,
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
                    Location("native-resident-field", role * 2 + field),
                    lineages[role][field],
                )?;
            }
        }
        Ok(())
    }
}
impl NativeResidentFieldLease {
    /// Correlation of the two private constructor fields, not admission.
    pub fn same_constructor_as_on_control(&self, other: &Self) -> bool {
        self.role != other.role && self.fields.shares_owner_with(&other.fields)
    }
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
impl Drop for NativeResidentFieldLease {
    fn drop(&mut self) {
        if self.fields.phases[self.role].load(Ordering::Acquire) != DISPOSED {
            self.fields.phases[self.role].store(FAILED, Ordering::Release);
        }
    }
}
pub(crate) fn visit_resident<'a>(
    fields: NativeResidentFields<'a>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    if !fields.body.contains(fields.owner.body())
        || !fields.body.contains(BodyRef::of(fields.registration))
        || !fields.owner.payload().contains(fields.slot)
    {
        return Err(AudioStorageError::SourceMismatch);
    }
    for playback in [fields.request, fields.failed_response]
        .into_iter()
        .flatten()
    {
        if !fields.slot.contains(BodyRef::of(playback)) {
            return Err(AudioStorageError::SourceMismatch);
        }
    }
    if fields
        .cancellation
        .is_some_and(|field| !fields.slot.contains(field))
    {
        return Err(AudioStorageError::SourceMismatch);
    }
    fields.owner.observe(ledger)?;
    fields.registration.visit_fields(fields.owner, ledger)?;
    if fields.cancellation.is_some() {
        return Err(AudioStorageError::UnknownStorage);
    }
    if let Some(playback) = fields.request {
        playback.visit_fresh_candidate_storage(ledger)?;
    }
    if let Some(playback) = fields.failed_response {
        playback.visit_fresh_candidate_storage(ledger)?;
    }
    Ok(())
}
