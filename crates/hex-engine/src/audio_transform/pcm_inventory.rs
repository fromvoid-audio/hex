//! Borrow actual committed/pending cache PCM without cloning owning fields.
use super::*;
use crate::{AudioRootStorageLedger, AudioStorageError, AudioStorageRole};
use std::cell::Ref;

pub struct AudioTransformPcmInventory<'a> {
    owner: &'a AudioTransformPreparation,
    cache: Ref<'a, Cache>,
    pending: Ref<'a, Option<Vec<Entry>>>,
    tempo: Ref<'a, Option<Arc<hex_project::TempoMap>>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OriginalOnlyStorageCost {
    pub(crate) existing_backing_bytes: usize,
    pub(crate) transaction_new_backing_bytes: usize,
}
impl AudioTransformPreparation {
    /// Control-only scoped borrow. The callback cannot return or retain its
    /// cache guards/borrowed ledger. Busy storage refuses before mutation.
    ///
    /// ```compile_fail
    /// let context = hex_engine::audio_transform::AudioTransformPreparation::default();
    /// let escaped = context.with_pcm_inventory_on_control(|inventory| Ok(inventory));
    /// ```
    pub fn with_pcm_inventory_on_control<T>(
        &self,
        visit: impl for<'scope> FnOnce(
            &'scope AudioTransformPcmInventory<'scope>,
        ) -> Result<T, AudioStorageError>,
    ) -> Result<T, AudioStorageError> {
        let inventory = AudioTransformPcmInventory {
            owner: self,
            cache: self
                .cache
                .try_borrow()
                .map_err(|_| AudioStorageError::UnknownStorage)?,
            pending: self
                .pending
                .try_borrow()
                .map_err(|_| AudioStorageError::UnknownStorage)?,
            tempo: self
                .tempo
                .try_borrow()
                .map_err(|_| AudioStorageError::UnknownStorage)?,
        };
        visit(&inventory)
    }
}
impl AudioTransformPcmInventory<'_> {
    /// Allocation-free facts from the SAME guarded fields subsequently visited.
    /// Cache-growth size follows the pinned std RawVec::grow_amortized policy
    /// used by transaction(), not an estimate derived from entry count alone.
    /// Original backing is already in the old union; replacement backing is
    /// additional and may coexist with it. No approval/constructor token escapes.
    pub(crate) fn original_only_storage_cost(
        &self,
    ) -> Result<OriginalOnlyStorageCost, AudioStorageError> {
        self.require_original_only()?;
        if !crate::audio_storage_constructor_layout_supported() {
            return Err(AudioStorageError::UnsupportedConstructor);
        }
        // Raw cancellation ingress remains unqualified before any allocation.
        if !matches!(self.owner.shutdown, TransformCancellation::Retained(_)) {
            return Err(AudioStorageError::UnknownStorage);
        }
        let bytes = |capacity: usize| {
            capacity
                .checked_mul(size_of::<Entry>())
                .ok_or(AudioStorageError::Overflow)
        };
        let existing = bytes(self.cache.entries.capacity())?
            .checked_add(bytes(self.pending.as_ref().map_or(0, Vec::capacity))?)
            .ok_or(AudioStorageError::Overflow)?;
        let required = self.owner.entry_limit;
        let capacity = self.cache.entries.capacity();
        let growth = if capacity < required {
            let minimum = if size_of::<Entry>() == 1 {
                8
            } else if size_of::<Entry>() <= 1024 {
                4
            } else {
                1
            };
            let target = capacity
                .checked_mul(2)
                .ok_or(AudioStorageError::Overflow)?
                .max(required)
                .max(minimum);
            std::alloc::Layout::array::<Entry>(target).map_err(|_| AudioStorageError::Overflow)?;
            bytes(target)?
        } else {
            0
        };
        let cost = OriginalOnlyStorageCost {
            existing_backing_bytes: existing,
            transaction_new_backing_bytes: growth,
        };
        cost.existing_backing_bytes
            .checked_add(cost.transaction_new_backing_bytes)
            .ok_or(AudioStorageError::Overflow)?;
        Ok(cost)
    }
    /// The first original-only graph still owns the actual reserved EMPTY cache
    /// backing. Observe it, pending empty capacity and the actual cancellation
    /// field under the same guards. Nonempty domains/tempo/derived entries and
    /// test-local owning budgets remain Unknown, not inferred from PCM counts.
    /// The enclosing preparation body and old/new/temporary peak are separate.
    pub fn visit_original_only_control_storage_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        // Validate the same guarded original-only class and checked metadata
        // sizing before recording its existing fields. The future transaction
        // overlap is NOT inserted into an observed current DATA summary.
        self.original_only_storage_cost()?;
        ledger.borrow_metadata_vec_backing_on_control(&self.cache.entries)?;
        if let Some(pending) = self.pending.as_ref() {
            ledger.borrow_metadata_vec_backing_on_control(pending)?;
        }
        self.owner.visit_cancellation_storage_on_control(ledger)
    }
    fn require_original_only(&self) -> Result<(), AudioStorageError> {
        let AudioTransformPreparation {
            cache: _,
            pending: _,
            tempo: _,
            shutdown: _,
            budget,
            cache_limit,
            source_limit,
            entry_limit,
            hits,
            failed,
            #[cfg(test)]
            cancel_after_polls,
        } = self.owner;
        let _: (&usize, &usize, &usize, &Cell<u64>, &Cell<bool>) =
            (cache_limit, source_limit, entry_limit, hits, failed);
        #[cfg(test)]
        let _: &Cell<Option<usize>> = cancel_after_polls;
        let Cache {
            entries,
            bytes,
            source_bytes,
        } = &*self.cache;
        if failed.get()
            || !entries.is_empty()
            || *bytes != 0
            || *source_bytes != 0
            || self
                .pending
                .as_ref()
                .is_some_and(|entries| !entries.is_empty())
            || self.tempo.is_some()
            || !budget.uses_static_production_counter()
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        Ok(())
    }
    /// Actual embedded fields when composing with other root/model visitors.
    /// Repeated visits of the SAME field deduplicate; another physical handle
    /// must still join closure. Derived/unknown PCM remains refused. Cache
    /// metadata, construction peak and publication are separate obligations.
    pub fn visit_embedded_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        for entry in self.cache.entries.iter().chain(
            self.pending
                .as_ref()
                .iter()
                .flat_map(|entries| entries.iter()),
        ) {
            ledger.borrow_embedded_native_audio_on_control(&entry.source)?;
            ledger.borrow_embedded_native_audio_on_control(&entry.prepared)?;
        }
        Ok(())
    }
    /// PCM fields only: source and prepared buffers in every actual pending and
    /// committed entry. Cache/domain/tempo metadata and publication remain separate.
    pub fn visit_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
        role: AudioStorageRole,
    ) -> Result<(), AudioStorageError> {
        for entry in self.cache.entries.iter().chain(
            self.pending
                .as_ref()
                .iter()
                .flat_map(|entries| entries.iter()),
        ) {
            ledger.borrow_native_audio_on_control(&entry.source, role)?;
            ledger.borrow_native_audio_on_control(&entry.prepared, role)?;
        }
        Ok(())
    }
}
