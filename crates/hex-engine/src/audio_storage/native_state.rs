//! Finite Audio State BOX footprint, not its descendants. The audited Audio
//! factory pairs the actual constructor body before escape. Root retains the
//! record and State moves the non-Clone lease through the existing transport.
use super::*;
use crate::{SharedMetadata, sampler_host_fields::BodyRef};
use std::sync::atomic::{AtomicU8, Ordering};
const LIVE: u8 = 0;
const DROP_PENDING: u8 = 1;
const DISPOSED: u8 = 2;
const FAILED: u8 = 3;
struct StateBody {
    bytes: usize,
    lease_lineage: u64,
    phase: AtomicU8,
}
/// Internal Audio integration only. This owns immutable physical metadata and
/// a retention obligation, never the mutable State or a Root clone/cycle.
/// Body footprint says NOTHING about owning children or native admission.
pub struct NativeStateStorageRegistration {
    state: SharedMetadata<StateBody>,
}
/// Exactly one actual State field; neither Clone nor an owner reconstruction.
pub struct NativeStateStorageLease {
    state: SharedMetadata<StateBody>,
}
impl NativeStateStorageRegistration {
    /// Concrete paired metadata body+header. The Audio constructor MUST add
    /// this to its actual old/new retained+peak union BEFORE either allocation.
    pub fn metadata_requested_bytes_on_control() -> Result<usize, AudioStorageError> {
        let (body, header) = crate::owned_shared::OwnedShared::<StateBody>::declared_parts()
            .ok_or(AudioStorageError::UnknownStorage)?;
        body.checked_add(header).ok_or(AudioStorageError::Overflow)
    }
    /// Actual new value/returned-pair inline overlap. Audio separately reserves
    /// its actual State initializer and existing host/family guard scratch.
    pub fn construction_scratch_bytes_on_control() -> Result<usize, AudioStorageError> {
        size_of::<StateBody>()
            .checked_add(size_of::<(Self, NativeStateStorageLease)>())
            .ok_or(AudioStorageError::Overflow)
    }
    /// Audited callsite: actual Audio State Box, before queue escape. No caller
    /// byte declaration, saved address, owner extraction or child certificate.
    pub fn pair_actual_state_on_control(
        body: BodyRef<'_>,
    ) -> Result<(Self, NativeStateStorageLease), AudioStorageError> {
        Self::metadata_requested_bytes_on_control()?;
        if body.layout_bytes() == 0 {
            return Err(AudioStorageError::SourceMismatch);
        }
        let lineage = crate::audio_only_inputs::handle()?;
        let state = SharedMetadata::new_on_control(StateBody {
            bytes: body.layout_bytes(),
            lease_lineage: lineage,
            phase: AtomicU8::new(LIVE),
        });
        let lease = NativeStateStorageLease {
            state: state.clone(),
        };
        Ok((Self { state }, lease))
    }
    /// Only the actual same returned State+lease may enter normal control
    /// destruction. The record stays charged while its body is taken/dropped.
    pub fn begin_actual_state_drop_on_control(
        &self,
        body: BodyRef<'_>,
        lease: &NativeStateStorageLease,
    ) -> Result<(), AudioStorageError> {
        if !self.state.shares_owner_with(&lease.state)
            || self.state.bytes != body.layout_bytes()
            || self.state.phase.load(Ordering::Acquire) != LIVE
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.state.phase.store(DROP_PENDING, Ordering::Release);
        Ok(())
    }
    pub fn is_normally_disposed_on_control(&self) -> bool {
        self.state.phase.load(Ordering::Acquire) == DISPOSED
    }
    /// Same actual persistent record drives arithmetic AND normal DATA/handle
    /// observation. Its State children require their own mandatory visitors.
    pub fn visit_storage_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.mutate(|ledger| self.visit_fields(ledger))
    }
    pub(crate) fn is_live_on_control(&self) -> bool {
        self.state.phase.load(Ordering::Acquire) == LIVE
    }
    pub(crate) fn visit_fields<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let phase = self.state.phase.load(Ordering::Acquire);
        if !matches!(phase, LIVE | DISPOSED) {
            return Err(AudioStorageError::UnknownStorage);
        }
        let paired = self.state.paired_owner_on_control();
        if !ledger.skip_observed_shared_field(&self.state) {
            ledger.observe_shared(paired.arc(), paired.parts(), Owner::exclusive(&self.state))?;
        }
        if phase == LIVE {
            let mut owner = Owner::exclusive(self.state.as_ref());
            owner.domain = OwnerDomain::RetainedNative;
            ledger.bytes(owner, Location("native-state-box", 0), self.state.bytes)?;
            let shared = Owner::shared(paired.arc());
            if let Some(arithmetic) = &mut ledger.arithmetic {
                arithmetic.existing(shared)?;
            }
            ledger.handle_at(
                shared,
                Owner::exclusive(&self.state.lease_lineage),
                Location("native-state-lease", 0),
                self.state.lease_lineage,
            )?;
        }
        Ok(())
    }
}
impl NativeStateStorageLease {
    pub(crate) fn visit_bound_body_fields<'a>(
        &'a self,
        body: BodyRef<'_>,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        if self.state.phase.load(Ordering::Acquire) != LIVE
            || self.state.bytes != body.layout_bytes()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        let shared = Owner::shared(self.state.paired_owner_on_control().arc());
        let mut physical = Owner::exclusive(self.state.as_ref());
        physical.domain = OwnerDomain::RetainedNative;
        for owner in [shared, physical] {
            if let Some(arithmetic) = &mut ledger.arithmetic {
                arithmetic.existing(owner)?;
            } else if !ledger.data.iter().any(|row| row.owner.same(owner)) {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        Ok(())
    }
    // Only the Engine affine fresh-Box holder uses this path. The private
    // holder keeps the actual Box and its non-Clone constructor lease together;
    // Audio's existing same-Root returned-State validation remains unchanged.
    pub(crate) fn begin_paired_body_drop_on_control(
        &mut self,
        body: BodyRef<'_>,
    ) -> Result<(), AudioStorageError> {
        if self.state.bytes != body.layout_bytes()
            || self.state.phase.load(Ordering::Acquire) != LIVE
        {
            self.state.phase.store(FAILED, Ordering::Release);
            return Err(AudioStorageError::SourceMismatch);
        }
        self.state.phase.store(DROP_PENDING, Ordering::Release);
        Ok(())
    }

    /// The actual non-Clone State lease has moved into its existing control
    /// Execution BEFORE State is taken. Call ONLY after that same Box normally
    /// finishes destruction. On unwind the parent retains this lease+Pending.
    pub fn complete_actual_state_drop_on_control(&mut self) -> Result<(), AudioStorageError> {
        let owner = self.state.paired_owner_on_control().arc();
        if self.state.phase.load(Ordering::Acquire) != DROP_PENDING
            || Arc::strong_count(owner) != 2
            || Arc::weak_count(owner) != 0
        {
            self.state.phase.store(FAILED, Ordering::Release);
            return Err(AudioStorageError::UnknownStorage);
        }
        self.state.phase.store(DISPOSED, Ordering::Release);
        Ok(())
    }
}
impl Drop for NativeStateStorageLease {
    fn drop(&mut self) {
        // Neither disappearance of a field nor an interrupted destructor proves
        // normal physical completion. The persistent Root record keeps failure.
        if self.state.phase.load(Ordering::Acquire) != DISPOSED {
            self.state.phase.store(FAILED, Ordering::Release);
        }
    }
}

/// Actual guarded birth slot and the TWO newly owning constructor fields.
/// Engine verifies containment and actual paired allocation equality while all
/// references exist. This is an audited Audio constructor schema, not a permit.
pub struct NativeClaimBirthFields<'a> {
    pub root: crate::sampler_host_fields::SharedMetadataRef<'a>,
    pub slots: crate::sampler_host_fields::VecRef<'a>,
    pub index: usize,
    pub life: crate::sampler_host_fields::SharedMetadataRef<'a>,
    pub owned_root: crate::sampler_host_fields::SharedMetadataRef<'a>,
    pub owned_life: crate::sampler_host_fields::SharedMetadataRef<'a>,
}
struct ClaimFields {
    root_lineage: u64,
    life_lineage: u64,
    lease_lineage: u64,
    phase: AtomicU8,
}
pub struct NativeClaimStorageRegistration {
    state: SharedMetadata<ClaimFields>,
}
/// Moves with the actual Root+Life payload. It owns neither Root nor Life and
/// therefore cannot introduce a strong reference cycle into its birth Root.
pub struct NativeClaimStorageLease {
    state: SharedMetadata<ClaimFields>,
}
impl NativeClaimStorageRegistration {
    pub fn metadata_requested_bytes_on_control() -> Result<usize, AudioStorageError> {
        let (body, header) = crate::owned_shared::OwnedShared::<ClaimFields>::declared_parts()
            .ok_or(AudioStorageError::UnknownStorage)?;
        body.checked_add(header).ok_or(AudioStorageError::Overflow)
    }
    pub fn construction_scratch_bytes_on_control() -> Result<usize, AudioStorageError> {
        size_of::<ClaimFields>()
            .checked_add(size_of::<(Self, NativeClaimStorageLease)>())
            .ok_or(AudioStorageError::Overflow)
    }
    pub fn pair_actual_claim_on_control(
        fields: NativeClaimBirthFields<'_>,
    ) -> Result<(Self, NativeClaimStorageLease), AudioStorageError> {
        Self::metadata_requested_bytes_on_control()?;
        validate_claim_parent(fields.root, fields.slots, fields.index, fields.life)?;
        if !fields
            .root
            .constructor_owner()?
            .same(fields.owned_root.constructor_owner()?)
            || !fields
                .life
                .constructor_owner()?
                .same(fields.owned_life.constructor_owner()?)
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        // Actual constructor fields, not identities guessed from population.
        // All IDs precede the new metadata allocation and lease creation.
        let root_lineage = crate::audio_only_inputs::handle()?;
        let life_lineage = crate::audio_only_inputs::handle()?;
        let lease_lineage = crate::audio_only_inputs::handle()?;
        let state = SharedMetadata::new_on_control(ClaimFields {
            root_lineage,
            life_lineage,
            lease_lineage,
            phase: AtomicU8::new(LIVE),
        });
        let lease = NativeClaimStorageLease {
            state: state.clone(),
        };
        Ok((Self { state }, lease))
    }
    pub fn is_normally_disposed_on_control(&self) -> bool {
        self.state.phase.load(Ordering::Acquire) == DISPOSED
    }
    /// Mandatory actual Root and BIRTH-slot Life must already be declared by
    /// the finite host traversal. This record adds owning HANDLE lineages only,
    /// never a second Root/Life DATA declaration or a replacement owner anchor.
    pub fn visit_storage_on_control<'a>(
        &'a self,
        root: crate::sampler_host_fields::SharedMetadataRef<'a>,
        slots: crate::sampler_host_fields::VecRef<'a>,
        index: usize,
        life: crate::sampler_host_fields::SharedMetadataRef<'a>,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.mutate(|ledger| self.visit_fields(root, slots, index, life, ledger))
    }
    pub(crate) fn visit_fields<'a>(
        &'a self,
        root: crate::sampler_host_fields::SharedMetadataRef<'a>,
        slots: crate::sampler_host_fields::VecRef<'a>,
        index: usize,
        life: crate::sampler_host_fields::SharedMetadataRef<'a>,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        validate_claim_parent(root, slots, index, life)?;
        // The actual counterpart remains in its actual guarded birth slot.
        if !slots.element(index)?.contains(BodyRef::of(self)) {
            return Err(AudioStorageError::SourceMismatch);
        }
        let phase = self.state.phase.load(Ordering::Acquire);
        if !matches!(phase, LIVE | DISPOSED) {
            return Err(AudioStorageError::UnknownStorage);
        }
        let paired = self.state.paired_owner_on_control();
        if !ledger.skip_observed_shared_field(&self.state) {
            ledger.observe_shared(paired.arc(), paired.parts(), Owner::exclusive(&self.state))?;
        }
        if phase == LIVE {
            for (owner, field, id) in [
                (
                    root.constructor_owner()?,
                    &self.state.root_lineage,
                    self.state.root_lineage,
                ),
                (
                    life.constructor_owner()?,
                    &self.state.life_lineage,
                    self.state.life_lineage,
                ),
                (
                    Owner::shared(paired.arc()),
                    &self.state.lease_lineage,
                    self.state.lease_lineage,
                ),
            ] {
                if let Some(arithmetic) = &mut ledger.arithmetic {
                    arithmetic.existing(owner)?;
                } else if !ledger.data.iter().any(|row| row.owner.same(owner)) {
                    return Err(AudioStorageError::SourceMismatch);
                }
                ledger.handle_at(
                    owner,
                    Owner::exclusive(field),
                    Location("native-claim-lineage", 0),
                    id,
                )?;
            }
        }
        Ok(())
    }
}
fn validate_claim_parent(
    root: crate::sampler_host_fields::SharedMetadataRef<'_>,
    slots: crate::sampler_host_fields::VecRef<'_>,
    index: usize,
    life: crate::sampler_host_fields::SharedMetadataRef<'_>,
) -> Result<(), AudioStorageError> {
    root.constructor_owner()?;
    life.constructor_owner()?;
    if !root.payload().contains(slots.body()) || !slots.element(index)?.contains(life.body()) {
        return Err(AudioStorageError::SourceMismatch);
    }
    Ok(())
}
impl NativeClaimStorageLease {
    /// Only the private immutable owning payload is about to be destroyed.
    /// Its non-Clone lease remains outside that payload through normal drop.
    pub fn begin_actual_claim_drop_on_control(&mut self) -> Result<(), AudioStorageError> {
        if self.state.phase.load(Ordering::Acquire) != LIVE {
            self.state.phase.store(FAILED, Ordering::Release);
            return Err(AudioStorageError::UnknownStorage);
        }
        self.state.phase.store(DROP_PENDING, Ordering::Release);
        Ok(())
    }
    pub fn complete_actual_claim_drop_on_control(&mut self) -> Result<(), AudioStorageError> {
        let owner = self.state.paired_owner_on_control().arc();
        if self.state.phase.load(Ordering::Acquire) != DROP_PENDING
            || Arc::strong_count(owner) != 2
            || Arc::weak_count(owner) != 0
        {
            self.state.phase.store(FAILED, Ordering::Release);
            return Err(AudioStorageError::UnknownStorage);
        }
        self.state.phase.store(DISPOSED, Ordering::Release);
        Ok(())
    }
}
impl Drop for NativeClaimStorageLease {
    fn drop(&mut self) {
        if self.state.phase.load(Ordering::Acquire) != DISPOSED {
            self.state.phase.store(FAILED, Ordering::Release);
        }
    }
}

// Fixed next constructor fields only, never exported totals/approval or a new
// source-observation mode. Normal mandatory visitors/closure run first.
impl AudioRootStorageLedger<'_> {
    pub(crate) fn require_future_claimed_wrapper_rows(&self) -> Result<(), AudioStorageError> {
        if self.arithmetic.is_some() {
            return Err(AudioStorageError::UnknownStorage);
        }
        if self.reservation.data_rows.saturating_sub(self.data.len()) < 2
            || self.data.capacity().saturating_sub(self.data.len()) < 2
            || self
                .reservation
                .handle_rows
                .saturating_sub(self.handles.len())
                < 2
            || self.handles.capacity().saturating_sub(self.handles.len()) < 2
        {
            return Err(AudioStorageError::Capacity);
        }
        Ok(())
    }
}
