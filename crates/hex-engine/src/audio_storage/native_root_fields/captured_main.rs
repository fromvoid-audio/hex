//! One finite captured-Main alias in the EXISTING Root-field registration.
//! It is not another callback/publication/retirement actor or a new registry.
use super::*;
pub struct NativeCapturedMainRootLease {
    fields: SharedMetadata<Fields>,
    generation: u64,
}
impl NativeRootFieldRegistration {
    /// The audited Audio constructor holds the actual selected Root's work
    /// exclusion. It immediately pairs this metadata field with that SAME
    /// Root field, before the captured parent/handle can escape.
    pub fn reserve_captured_main_field_on_control(
        &self,
    ) -> Result<NativeCapturedMainRootLease, AudioStorageError> {
        if self
            .fields
            .phases
            .iter()
            .any(|phase| matches!(phase.load(Ordering::Acquire), DROP_PENDING | FAILED))
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        let ids = [
            crate::audio_only_inputs::handle()?,
            crate::audio_only_inputs::handle()?,
        ];
        let previous = self.fields.main_phase.load(Ordering::Acquire);
        if !matches!(previous, VACANT | DISPOSED) {
            return Err(AudioStorageError::UnknownStorage);
        }
        self.fields
            .main_phase
            .compare_exchange(previous, DROP_PENDING, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| AudioStorageError::UnknownStorage)?;
        let Some(generation) = self
            .fields
            .main_generation
            .load(Ordering::Relaxed)
            .checked_add(1)
        else {
            self.fields.main_phase.store(FAILED, Ordering::Release);
            return Err(AudioStorageError::Overflow);
        };
        self.fields
            .main_generation
            .store(generation, Ordering::Relaxed);
        for (target, id) in self.fields.main_lineages.iter().zip(ids) {
            target.store(id, Ordering::Relaxed);
        }
        let lease = NativeCapturedMainRootLease {
            fields: self.fields.clone(),
            generation,
        };
        self.fields.main_phase.store(LIVE, Ordering::Release);
        Ok(lease)
    }
}
impl NativeCapturedMainRootLease {
    pub fn begin_actual_field_drop_on_control(&mut self) -> Result<(), AudioStorageError> {
        if self.fields.main_generation.load(Ordering::Acquire) != self.generation
            || self
                .fields
                .main_phase
                .compare_exchange(LIVE, DROP_PENDING, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            self.fields.main_phase.store(FAILED, Ordering::Release);
            return Err(AudioStorageError::UnknownStorage);
        }
        Ok(())
    }
    pub fn complete_actual_field_drop_on_control(&mut self) -> Result<(), AudioStorageError> {
        if self.fields.main_generation.load(Ordering::Acquire) != self.generation
            || self.fields.main_phase.load(Ordering::Acquire) != DROP_PENDING
        {
            self.fields.main_phase.store(FAILED, Ordering::Release);
            return Err(AudioStorageError::UnknownStorage);
        }
        self.fields.main_phase.store(DISPOSED, Ordering::Release);
        Ok(())
    }
}
impl Drop for NativeCapturedMainRootLease {
    fn drop(&mut self) {
        if self.fields.main_generation.load(Ordering::Acquire) == self.generation
            && self.fields.main_phase.load(Ordering::Acquire) != DISPOSED
        {
            self.fields.main_phase.store(FAILED, Ordering::Release);
        }
    }
}
