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
    /// The first original-only graph still owns the actual reserved EMPTY cache
    /// backing. Observe it, pending empty capacity and the actual cancellation
    /// field under the same guards. Nonempty domains/tempo/derived entries and
    /// test-local owning budgets remain Unknown, not inferred from PCM counts.
    /// The enclosing preparation body and old/new/temporary peak are separate.
    pub fn visit_original_only_control_storage_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
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
        ledger.borrow_metadata_vec_backing_on_control(entries)?;
        if let Some(pending) = self.pending.as_ref() {
            ledger.borrow_metadata_vec_backing_on_control(pending)?;
        }
        self.owner.visit_cancellation_storage_on_control(ledger)
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
