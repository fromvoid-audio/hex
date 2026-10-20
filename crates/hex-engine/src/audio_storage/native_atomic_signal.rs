//! Exactly one fresh AtomicU64 signal, its persistent immutable allocation
//! anchor and two affine fields. No raw Arc adoption or mutable owner capture.
use super::*;
use crate::SharedMetadata;
use std::{
    ops::Deref,
    sync::atomic::{AtomicU8, AtomicU64, Ordering},
};
const LIVE: u8 = 0;
const DROP_PENDING: u8 = 1;
const DISPOSED: u8 = 2;
const FAILED: u8 = 3;
// Exhaustive scalar payload: this allocation has no owning descendants.
struct Core {
    value: AtomicU64,
    phases: [AtomicU8; 2],
    lineages: [[u64; 2]; 2],
}
/// Retained in the existing Audio Root, never the callback or another registry.
/// The actual core allocation stays charged even after both fields retire.
pub struct NativeAtomicSignalRegistration {
    core: SharedMetadata<Core>,
}
/// One constructor field plus its release obligation. Neither Clone nor a raw
/// owning getter exists. The only exposed borrow is the actual atomic value.
///
/// ```compile_fail
/// let (_, [field, _]) = hex_engine::sampler_host_fields::NativeAtomicSignalRegistration::fresh_pair_on_control(0).unwrap();
/// let _duplicate = field.clone();
/// ```
pub struct NativeAtomicSignal {
    value: Option<SharedMetadata<Core>>,
    lease: SharedMetadata<Core>,
    endpoint: usize,
}
impl NativeAtomicSignalRegistration {
    /// Existing channel footprint counted only AtomicU64 payload. The actual
    /// new constructor includes its header and inline lineage/phase fields.
    /// This cost difference is not a source/admission certificate.
    pub fn legacy_payload_budget_increment_on_control(&self) -> Result<usize, AudioStorageError> {
        if self
            .core
            .phases
            .iter()
            .any(|phase| !matches!(phase.load(Ordering::Acquire), LIVE | DISPOSED))
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        Self::requested_bytes_on_control()?
            .checked_sub(size_of::<AtomicU64>())
            .ok_or(AudioStorageError::Overflow)
    }
    /// Correlate one actual owning field and its lease with the original
    /// private constructor. Scalar equality/counts never establish this link.
    pub fn matches_actual_field_on_control(
        &self,
        field: &NativeAtomicSignal,
        endpoint: usize,
    ) -> bool {
        endpoint < 2
            && field.endpoint == endpoint
            && self.core.shares_owner_with(&field.lease)
            && field
                .value
                .as_ref()
                .is_some_and(|value| self.core.shares_owner_with(value))
            && self.core.phases[endpoint].load(Ordering::Acquire) == LIVE
    }
    pub fn matches_actual_fields_on_control(&self, fields: [&NativeAtomicSignal; 2]) -> bool {
        fields
            .into_iter()
            .enumerate()
            .all(|(index, field)| self.matches_actual_field_on_control(field, index))
    }
    pub fn requested_bytes_on_control() -> Result<usize, AudioStorageError> {
        let (body, header) = crate::owned_shared::OwnedShared::<Core>::declared_parts()
            .ok_or(AudioStorageError::UnknownStorage)?;
        body.checked_add(header).ok_or(AudioStorageError::Overflow)
    }
    pub fn construction_scratch_bytes_on_control() -> Result<usize, AudioStorageError> {
        size_of::<Core>()
            .checked_add(size_of::<(Self, [NativeAtomicSignal; 2])>())
            .ok_or(AudioStorageError::Overflow)
    }
    /// Only the fresh factory calls this after its full applicable prebirth
    /// checks. All individual identities are reserved before birth/clones.
    pub fn fresh_pair_on_control(
        value: u64,
    ) -> Result<(Self, [NativeAtomicSignal; 2]), AudioStorageError> {
        Self::requested_bytes_on_control()?;
        let lineages = [
            [
                crate::audio_only_inputs::handle()?,
                crate::audio_only_inputs::handle()?,
            ],
            [
                crate::audio_only_inputs::handle()?,
                crate::audio_only_inputs::handle()?,
            ],
        ];
        let core = SharedMetadata::new_on_control(Core {
            value: AtomicU64::new(value),
            phases: std::array::from_fn(|_| AtomicU8::new(LIVE)),
            lineages,
        });
        let fields = std::array::from_fn(|endpoint| NativeAtomicSignal {
            value: Some(core.clone()),
            lease: core.clone(),
            endpoint,
        });
        Ok((Self { core }, fields))
    }
    pub fn visit_storage_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.mutate(|ledger| self.visit_fields(ledger))
    }
    pub(crate) fn visit_fields<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let Core {
            value: _,
            phases,
            lineages,
        } = self.core.as_ref();
        let phases = phases.each_ref().map(|p| p.load(Ordering::Acquire));
        if phases.iter().any(|p| !matches!(*p, LIVE | DISPOSED)) {
            return Err(AudioStorageError::UnknownStorage);
        }
        // DATA comes from the actual persistent owned anchor, not a saved
        // callback body/address or a reconstructed physical declaration. Only
        // outside-field HANDLE observations use the constructor-frozen keys.
        let paired = self.core.paired_owner_on_control();
        if !ledger.skip_observed_shared_field(&self.core) {
            ledger.observe_shared(paired.arc(), paired.parts(), Owner::exclusive(&self.core))?;
        }
        let owner = Owner::shared(paired.arc());
        // Actual constructor fields and their affine leases have DISTINCT
        // immutable lineages. Their moves never create another identity.
        for (endpoint, phase) in phases.into_iter().enumerate() {
            if phase == LIVE {
                for (field, lineage) in lineages[endpoint].iter().enumerate() {
                    if let Some(arithmetic) = &mut ledger.arithmetic {
                        arithmetic.existing(owner)?;
                    }
                    ledger.handle_at(
                        owner,
                        Owner::exclusive(lineage),
                        Location("native-atomic-signal-field", endpoint * 2 + field),
                        *lineage,
                    )?;
                }
            }
        }
        Ok(())
    }
}
impl NativeAtomicSignal {
    fn live_value(&self) -> Result<&AtomicU64, AudioStorageError> {
        let Some(value) = self.value.as_ref() else {
            self.lease.phases[self.endpoint].store(FAILED, Ordering::Release);
            return Err(AudioStorageError::UnknownStorage);
        };
        if !value.shares_owner_with(&self.lease)
            || self.lease.phases[self.endpoint].load(Ordering::Acquire) != LIVE
        {
            self.lease.phases[self.endpoint].store(FAILED, Ordering::Release);
            return Err(AudioStorageError::UnknownStorage);
        }
        Ok(&value.value)
    }
    /// Audited stop-field access without a new fallible runtime panic. Failure
    /// keeps the SAME allocation/field charged and cannot complete retirement.
    pub fn try_load_stop_on_control(&self) -> Result<bool, AudioStorageError> {
        match self.live_value()?.load(Ordering::Acquire) {
            0 => Ok(false),
            1 => Ok(true),
            _ => {
                self.lease.phases[self.endpoint].store(FAILED, Ordering::Release);
                Err(AudioStorageError::UnknownStorage)
            }
        }
    }
    pub fn try_request_stop_on_control(&self) -> Result<(), AudioStorageError> {
        let value = self.live_value()?;
        if !matches!(value.load(Ordering::Acquire), 0 | 1) {
            self.lease.phases[self.endpoint].store(FAILED, Ordering::Release);
            return Err(AudioStorageError::UnknownStorage);
        }
        value.store(1, Ordering::Release);
        Ok(())
    }
    /// Correlation observation only, never constructor/retirement authority.
    pub fn try_value_identity_on_control(&self) -> Result<usize, AudioStorageError> {
        Ok(self.live_value()? as *const AtomicU64 as usize)
    }
}
impl Deref for NativeAtomicSignal {
    type Target = AtomicU64;
    fn deref(&self) -> &Self::Target {
        &self
            .value
            .as_ref()
            .expect("signal field already disposed")
            .value
    }
}
impl Drop for NativeAtomicSignal {
    fn drop(&mut self) {
        let phase = &self.lease.phases[self.endpoint];
        if self
            .value
            .as_ref()
            .is_none_or(|value| !value.shares_owner_with(&self.lease))
        {
            phase.store(FAILED, Ordering::Release);
            return;
        }
        if phase.swap(DROP_PENDING, Ordering::AcqRel) != LIVE {
            phase.store(FAILED, Ordering::Release);
            return;
        }
        // The lease keeps this actual core alive across the field release.
        // Its own release follows completion. Root's record keeps allocation
        // DATA rooted until the existing custodian actually drops the anchor.
        drop(self.value.take());
        phase.store(DISPOSED, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;
