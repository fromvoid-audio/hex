//! Closed control-side media plans and borrowed actual PCM inventory.
//! No second source cache, publication authority or callback preparation.
use crate::{OriginalAudio, Session, service::AudioClipWaveform};
use hex_engine::{
    AudioRootStorageLedger, AudioStorageError, AudioStorageReservation, AudioStorageRole,
    native_device_resources::{
        DeviceResourceBank, ResourceCatalogReservation, ResourcePreparationError,
    },
};
use hex_project::{AssetId, ClipId, SortedEntries};
use std::mem::size_of;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
pub(crate) mod producer;
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug)]
pub struct ResidentResourcePlanReservation {
    pub inputs: AudioStorageReservation,
    pub catalog: ResourceCatalogReservation,
    pub max_preparation_metadata_bytes: usize,
    pub max_retained_data_bytes: usize,
    pub max_construction_peak_bytes: usize,
}

/// Holds the actual immutable media and waveform borrow while the ledger reads
/// their fields. It owns no PCM; no RefCell guard escapes into a callback.
pub struct ResidentPcmInventory<'a> {
    session: &'a Session,
    waveforms: &'a SortedEntries<ClipId, AudioClipWaveform>,
    transforms: &'a hex_engine::audio_transform::AudioTransformPcmInventory<'a>,
}
impl ResidentPcmInventory<'_> {
    /// Join the first original-only worker's actual model/media, cached sine,
    /// both cancellation fields and reserved empty transform metadata. This
    /// remains CHILD storage: enclosing worker/Event/native roots and complete
    /// construction/publication/retirement proof must join separately.
    pub fn visit_original_only_worker_children_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        self.visit_worker_model_media_storage_on_control(ledger)?;
        ledger.borrow_metadata_entries_backing_on_control(&self.session.verified_sources)?;
        self.session.sine_table.visit_storage_on_control(ledger)?;
        self.session.shutdown.visit(ledger)?;
        self.transforms
            .visit_original_only_control_storage_on_control(ledger)
    }
    /// Join the ACTUAL worker's current/Undo/Redo model fields and resident
    /// source/derivative/waveform fields under this same scoped Session/cache
    /// borrow. No snapshots or synthetic identities are constructed. Every
    /// retained history model remains visited, even if absent from current.
    ///
    /// This is selected model/media storage, NOT the whole Session: its inline
    /// enclosing body, sine/shutdown owners, transform metadata, verified-file
    /// facts, other worker locals, construction peak and native/root custody
    /// still require their own actual proof. Unknown model/source children
    /// refuse. The existing ledger can join this traversal with all guarded
    /// export families and real native current/candidate/retirement roots.
    pub fn visit_worker_model_media_storage_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        if !self.session.verified_sources.is_empty() {
            return Err(AudioStorageError::UnknownStorage);
        }
        ledger.borrow_editor_history_buffers_on_control(&self.session.editor)?;
        for project in self.session.editor.retained_projects_on_control() {
            crate::shared_project::visit_project_selected_storage_on_control(project, ledger)?;
        }
        self.visit_waveform_storage_on_control(ledger)?;
        for resident in self.session.media.values() {
            match &resident.source_audio {
                OriginalAudio::Stereo(audio) => {
                    ledger.borrow_embedded_native_audio_on_control(audio)?;
                }
                OriginalAudio::Mono(source) => {
                    ledger.borrow_embedded_native_source_on_control(source)?;
                }
            }
            if let Some(audio) = &resident.audio {
                ledger.borrow_embedded_native_audio_on_control(audio)?;
            }
        }
        // Committed/pending transformed PCM still uses its existing validated
        // source/root visitor; processed/unknown storage cannot be relabeled.
        self.transforms.visit_embedded_on_control(ledger)
    }
    /// Actual resident and retained-clip waveform map backing, paired summary
    /// handles, pyramids and embedded PCM. Other resident-source fields,
    /// transform/model metadata and enclosing Session storage remain separate.
    /// This selected traversal cannot grant whole-root admission by itself.
    pub fn visit_waveform_storage_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.borrow_metadata_entries_backing_on_control(&self.session.media)?;
        ledger.borrow_metadata_entries_backing_on_control(self.waveforms)?;
        for resident in self.session.media.values() {
            hex_media::waveform::borrow_shared_storage_on_control(&resident.waveform, ledger)?;
        }
        for waveform in self.waveforms.values() {
            hex_media::waveform::borrow_shared_storage_on_control(&waveform.waveform, ledger)?;
        }
        Ok(())
    }
    /// PCM fields only. Enclosing maps, transform caches, graph metadata and
    /// retirement/publication proof remain separate and must not be inferred.
    pub fn visit_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
        role: AudioStorageRole,
    ) -> Result<(), AudioStorageError> {
        for resident in self.session.media.values() {
            match &resident.source_audio {
                OriginalAudio::Stereo(audio) => {
                    ledger.borrow_native_audio_on_control(audio, role)?
                }
                OriginalAudio::Mono(source) => {
                    ledger.borrow_native_source_on_control(source, role)?
                }
            }
            if let Some(audio) = &resident.audio {
                ledger.borrow_native_audio_on_control(audio, role)?;
            }
            visit_waveform(ledger, &resident.waveform, role)?;
        }
        for waveform in self.waveforms.values() {
            visit_waveform(ledger, &waveform.waveform, role)?;
        }
        self.transforms.visit_on_control(ledger, role)?;
        Ok(())
    }
}
fn visit_waveform<'a>(
    ledger: &mut AudioRootStorageLedger<'a>,
    waveform: &'a hex_media::waveform::Waveform,
    role: AudioStorageRole,
) -> Result<(), AudioStorageError> {
    if let Some(audio) = waveform.audio_source() {
        ledger.borrow_native_audio_on_control(audio, role)
    } else if let Some(source) = waveform.original_source() {
        ledger.borrow_original_source_lease_on_control(source, role)
    } else {
        Err(AudioStorageError::UnknownStorage)
    }
}
fn check(cancel: &dyn hex_engine::CancellationQuery) -> Result<(), ResourcePreparationError> {
    if cancel.is_cancelled() {
        Err(ResourcePreparationError::Cancelled)
    } else {
        Ok(())
    }
}
impl Session {
    /// Actual cached DSP table field only; all external prepared/control aliases
    /// must join too. This neither observes the rest of Session nor grants native
    /// admission. Cloning a synthetic table cannot substitute for this field.
    pub fn visit_cached_sine_storage_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        self.sine_table.visit_storage_on_control(ledger)
    }
    /// The scoped inventory/ledger cannot escape transaction preparation or
    /// retain a borrow across cache commit, eviction or canonical mutation.
    pub fn with_resident_pcm_inventory_on_control<T>(
        &self,
        visit: impl for<'scope> FnOnce(
            &'scope ResidentPcmInventory<'scope>,
        ) -> Result<T, AudioStorageError>,
    ) -> Result<T, AudioStorageError> {
        let waveforms = self
            .audio_clip_waveforms
            .try_borrow()
            .map_err(|_| AudioStorageError::UnknownStorage)?;
        self.audio_transforms
            .with_pcm_inventory_on_control(|transforms| {
                let inventory = ResidentPcmInventory {
                    session: self,
                    waveforms: &waveforms,
                    transforms,
                };
                visit(&inventory)
            })
    }

    /// Prepare requested sorted source identities from ACTUAL retained media.
    /// History-only media may resolve here; freshness/publication still belongs
    /// to the canonical worker and actual graph/root admission.
    pub fn prepare_resident_resource_bank_on_control(
        &self,
        assets: &[AssetId],
        reservation: ResidentResourcePlanReservation,
        cancel: &dyn hex_engine::CancellationQuery,
    ) -> Result<DeviceResourceBank, ResourcePreparationError> {
        self.prepare_resident_resource_bank_with_pending_on_control(
            assets,
            None,
            reservation,
            cancel,
        )
    }

    /// Borrow one actual unpublished decoded original while preparing the
    /// candidate's catalog. The canonical asset must be fresh and requested;
    /// this never inserts media or publishes a model/audio revision.
    pub(crate) fn prepare_resident_resource_bank_with_pending_on_control(
        &self,
        assets: &[AssetId],
        pending: Option<(&hex_project::Asset, &OriginalAudio)>,
        reservation: ResidentResourcePlanReservation,
        cancel: &dyn hex_engine::CancellationQuery,
    ) -> Result<DeviceResourceBank, ResourcePreparationError> {
        producer::ResidentResourceSources::new(&self.media).prepare(
            assets,
            pending,
            reservation,
            cancel,
        )
    }
}

impl Session {
    /// Actual Session and transform owning cancellation fields, not all Session
    /// metadata or startup peak. The enclosing table/other worker aliases join
    /// separately; raw input refuses rather than gaining a paired declaration.
    pub fn visit_cancellation_storage_on_control<'a>(
        &'a self,
        ledger: &mut hex_engine::AudioRootStorageLedger<'a>,
    ) -> Result<(), hex_engine::AudioStorageError> {
        self.shutdown.visit(ledger)?;
        self.audio_transforms
            .visit_cancellation_storage_on_control(ledger)
    }
}
