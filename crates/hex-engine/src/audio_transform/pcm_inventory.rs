//! Borrow actual committed/pending cache PCM without cloning owning fields.
use super::*;
use crate::{AudioRootStorageLedger, AudioStorageError, AudioStorageRole};
use std::cell::Ref;

pub struct AudioTransformPcmInventory<'a> {
    cache: Ref<'a, Cache>,
    pending: Ref<'a, Option<Vec<Entry>>>,
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
            cache: self
                .cache
                .try_borrow()
                .map_err(|_| AudioStorageError::UnknownStorage)?,
            pending: self
                .pending
                .try_borrow()
                .map_err(|_| AudioStorageError::UnknownStorage)?,
        };
        visit(&inventory)
    }
}
impl AudioTransformPcmInventory<'_> {
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
