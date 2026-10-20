//! Constructor-derived individual Root Arc fields, not a payload-completeness
//! certificate. The three original actors move these affine leases with their
//! actual Root fields until normal release; no caller Arc is adopted.
use super::*;
use crate::{
    SharedMetadata,
    sampler_host_fields::{BodyRef, SharedMetadataRef},
};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
mod captured_main;
pub use captured_main::NativeCapturedMainRootLease;
const VACANT: u8 = 0;
const LIVE: u8 = 1;
const DROP_PENDING: u8 = 2;
const DISPOSED: u8 = 3;
const FAILED: u8 = 4;
const ROLES: usize = 3;
struct Fields {
    phases: [AtomicU8; ROLES],
    lineages: [[u64; 2]; ROLES],
    main_phase: AtomicU8,
    main_generation: AtomicU64,
    main_lineages: [AtomicU64; 2],
}
pub struct NativeRootFieldRegistration {
    fields: SharedMetadata<Fields>,
}
/// One actual actor's release obligation, moved rather than cloned.
/// ```compile_fail
/// let record = hex_engine::sampler_host_fields::NativeRootFieldRegistration::new_on_control().unwrap();
/// let field = record.reserve_field_on_control(0).unwrap();
/// let _duplicate = field.clone();
/// ```
pub struct NativeRootFieldLease {
    fields: SharedMetadata<Fields>,
    role: usize,
}
impl NativeRootFieldRegistration {
    pub fn requested_bytes_on_control() -> Result<usize, AudioStorageError> {
        let (body, header) = crate::owned_shared::OwnedShared::<Fields>::declared_parts()
            .ok_or(AudioStorageError::UnknownStorage)?;
        body.checked_add(header).ok_or(AudioStorageError::Overflow)
    }
    /// Additional physical metadata beside the actual Root fields.
    /// Pending or failed field destruction cannot be priced as reusable storage.
    pub fn retained_metadata_bytes_on_control(&self) -> Result<usize, AudioStorageError> {
        if self
            .fields
            .phases
            .iter()
            .any(|phase| !matches!(phase.load(Ordering::Acquire), VACANT | LIVE | DISPOSED))
            || !matches!(
                self.fields.main_phase.load(Ordering::Acquire),
                VACANT | LIVE | DISPOSED
            )
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
    /// qualified Root anchor by the private audited Audio constructor.
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
                main_phase: AtomicU8::new(VACANT),
                main_generation: AtomicU64::new(0),
                main_lineages: [AtomicU64::new(0), AtomicU64::new(0)],
            }),
        })
    }
    /// Actual Root exclusion remains held by the caller. There is exactly one
    /// reservation attempt and no fallible/allocation work after the live CAS
    /// before constructing the lease. Audio immediately clones the SAME owned
    /// Root anchor (nonallocating), without an intervening external callback.
    pub fn reserve_field_on_control(
        &self,
        role: usize,
    ) -> Result<NativeRootFieldLease, AudioStorageError> {
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
        Ok(NativeRootFieldLease {
            fields: self.fields.clone(),
            role,
        })
    }
    pub fn matches_actual_lease_on_control(&self, lease: &NativeRootFieldLease) -> bool {
        self.matches_actual_field_role_on_control(lease, lease.role)
    }
    /// Exact actual owning-field correlation, not descendant completeness or
    /// disposal authority. Any-member inspection retains its existing meaning.
    pub fn matches_actual_field_role_on_control(
        &self,
        lease: &NativeRootFieldLease,
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
        let Fields {
            phases,
            lineages,
            main_phase,
            main_generation: _,
            main_lineages,
        } = self.fields.as_ref();
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
                    Location("native-root-actor-field", role * 2 + field),
                    lineages[role][field],
                )?;
            }
        }
        match main_phase.load(Ordering::Acquire) {
            LIVE => {
                for (field, owner) in [data, metadata].into_iter().enumerate() {
                    if let Some(arithmetic) = &mut ledger.arithmetic {
                        arithmetic.existing(owner)?;
                    }
                    ledger.handle_at(
                        owner,
                        Owner::exclusive(&main_lineages[field]),
                        Location("captured-main-root-field", field),
                        main_lineages[field].load(Ordering::Relaxed),
                    )?;
                }
            }
            VACANT | DISPOSED => {}
            _ => return Err(AudioStorageError::UnknownStorage),
        }
        Ok(())
    }
}
impl NativeRootFieldLease {
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
impl Drop for NativeRootFieldLease {
    fn drop(&mut self) {
        if self.fields.phases[self.role].load(Ordering::Acquire) != DISPOSED {
            self.fields.phases[self.role].store(FAILED, Ordering::Release);
        }
    }
}

impl<'a> AudioRootStorageLedger<'a> {
    /// Header and individual field inventory ONLY. This operation cannot certify
    /// the Root's unseen children or produce complete/admission authority.
    pub fn borrow_sampler_native_root_fields_on_control(
        &mut self,
        root: crate::sampler_host_fields::SharedMetadataRef<'a>,
        fields: Option<&'a NativeRootFieldRegistration>,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| {
            root.observe(ledger)?;
            if let Some(fields) = fields {
                if !root.payload().contains(BodyRef::of(fields)) {
                    return Err(AudioStorageError::SourceMismatch);
                }
                fields.visit_fields(root, ledger)?;
            }
            Ok(())
        })
    }
}
