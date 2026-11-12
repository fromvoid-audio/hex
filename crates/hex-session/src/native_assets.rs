//! Opt-in owned-decoded admission. Encoded file buffers, decoder scratch and
//! pre-admission decoding peaks retain their separate existing limits.
use crate::*;
use hex_engine::native_device_resources::ResourcePreparationError;
use hex_engine::{AudioRootStorageSummary, AudioStorageLedgerReservation};
use std::sync::atomic::{AtomicBool, Ordering};

/// Explicit caller limits. No default, unlimited fallback, or reuse of the
/// mutable-runtime/preroll budget. Future host/export custody must extend THIS
/// reservation domain before any owning handle is exposed to those consumers.
#[derive(Clone, Copy, Debug)]
pub struct ResidentNativeReservation {
    pub storage: AudioStorageLedgerReservation,
    pub max_scan_frames: u64,
}
impl From<ResourcePreparationError> for SessionError {
    fn from(reason: ResourcePreparationError) -> Self {
        Self::NativeResource(reason)
    }
}
fn pcm_bytes(frames: u64) -> Result<usize, SessionError> {
    usize::try_from(frames)
        .ok()
        .and_then(|n| n.checked_mul(mem::size_of::<hex_engine::Frame>()))
        .ok_or(SessionError::Invalid("resident pcm byte count overflow"))
}
pub(crate) enum NativeAssetDestination {
    Unassigned,
    NewInstrument {
        track: hex_project::TrackId,
        placement: hex_project::InstrumentLoadPlacement,
        state: hex_lattice_contract::NativeDeviceState,
        source_slot: hex_lattice_contract::DeviceAssetSlot,
    },
    ExistingInstrument {
        track: hex_project::TrackId,
        device: hex_project::DeviceId,
        expected_state: hex_lattice_contract::NativeDeviceState,
        state: hex_lattice_contract::NativeDeviceState,
        source_slot: hex_lattice_contract::DeviceAssetSlot,
    },
}
pub(crate) struct ReconstructedOriginalSources<'a> {
    pub assets: hex_engine::native_device_resources::ResidentDeviceAssets,
    pub overview_budget: crate::source_overview::OverviewBudget,
    pub cancel: &'a AtomicBool,
}
impl Session {
    pub fn with_native_reservation(
        mut self,
        reservation: ResidentNativeReservation,
    ) -> Result<Self, SessionError> {
        self.native_assets
            .storage_on_control(pcm_bytes(self.resident_frames)?, reservation.storage)
            .map_err(ResourcePreparationError::from)?;
        self.native_reservation = Some(reservation);
        Ok(self)
    }
    pub(crate) fn validate_native_legacy_budget(&self, frames: u64) -> Result<(), SessionError> {
        if let Some(reservation) = self.native_reservation {
            self.native_assets
                .storage_on_control(pcm_bytes(frames)?, reservation.storage)
                .map_err(ResourcePreparationError::from)?;
        }
        Ok(())
    }
    pub fn native_source(&self, asset: AssetId) -> Option<hex_engine::OwnedAudioSourceView<'_>> {
        self.native_assets.source(asset)
    }
    pub fn native_storage_on_control(&self) -> Result<AudioRootStorageSummary, SessionError> {
        let reservation = self.native_reservation.ok_or(SessionError::Invalid(
            "native source import requires an explicit resident reservation",
        ))?;
        self.native_assets
            .storage_on_control(pcm_bytes(self.resident_frames)?, reservation.storage)
            .map_err(ResourcePreparationError::from)
            .map_err(SessionError::from)
    }
    /// Real canonical transaction. Unassigned metadata and original native PCM
    /// are committed together; no clip derivative or resource alias is made.
    pub fn import_device_asset(
        &mut self,
        decoded: DecodedAudio,
        cancel: &AtomicBool,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), SessionError> {
        self.import_device_asset_with_admission(decoded, cancel, previous, || Ok(()))
    }
    // Same transaction seam as prepared batch import: a final caller admission
    // can refuse while both the editor and source container are still staged.
    fn import_device_asset_with_admission(
        &mut self,
        decoded: DecodedAudio,
        cancel: &AtomicBool,
        previous: Option<&hex_engine::PreparedRevision>,
        admit: impl FnOnce() -> Result<(), SessionError>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), SessionError> {
        if hex_engine::native_device_resources::requires_private_sampler_sources(self.project()) {
            return Err(SessionError::Invalid(
                "sampler prepared snapshots are private to offline export",
            ));
        }
        self.import_device_asset_prepared(
            decoded,
            cancel,
            NativeAssetDestination::Unassigned,
            |candidate, media, table, transforms, waveforms, _stage, _media_assets| {
                let prepared = prepare_with_waveforms(
                    candidate,
                    media,
                    table,
                    previous.map(|p| p.session().as_ref()),
                    transforms,
                    waveforms,
                )?;
                admit()?;
                Ok(Arc::new(prepared))
            },
            |_, _, _| Ok(()),
        )
    }
    /// Complete staged preparation and the final cancel check before invoking `finalize`.
    /// A worker publication may enqueue there as its last fallible operation: after success only
    /// already-reserved source/history/overview moves remain, so late cancellation cannot claim
    /// that an accepted publication rolled the canonical edit back.
    pub(crate) fn import_device_asset_prepared<T>(
        &mut self,
        decoded: DecodedAudio,
        cancel: &AtomicBool,
        destination: NativeAssetDestination,
        prepare: impl FnOnce(
            &Project,
            &MediaBank,
            Arc<hex_engine::SineTable>,
            &hex_engine::audio_transform::AudioTransformPreparation,
            &std::cell::RefCell<BTreeMap<hex_project::ClipId, service::AudioClipWaveform>>,
            &hex_engine::native_device_resources::StagedNativeAssetView<'_>,
            &std::collections::HashMap<hex_project::AssetId, crate::ResidentAsset>,
        ) -> Result<T, SessionError>,
        finalize: impl FnOnce(
            &Project,
            &mut T,
            &hex_engine::native_device_resources::StagedNativeAssetView<'_>,
        ) -> Result<(), SessionError>,
    ) -> Result<(Created, T), SessionError> {
        let reservation = self.native_reservation.ok_or(SessionError::Invalid(
            "native source import requires an explicit resident reservation",
        ))?;
        if self.audio_mode != DocumentAudioMode::Resident {
            return Err(SessionError::Invalid(
                "native source import requires resident document mode",
            ));
        }
        if cancel.load(Ordering::Relaxed) || self.shutdown.load(Ordering::Relaxed) {
            return Err(ResourcePreparationError::Cancelled.into());
        }
        let incoming = u64::try_from(decoded.audio().pcm_capacity_frames())
            .map_err(|_| SessionError::Invalid("resident pcm capacity exceeds frame range"))?;
        self.native_assets
            .capacity_frames()
            .ok()
            .and_then(|n| n.checked_add(self.resident_frames))
            .and_then(|n| n.checked_add(incoming))
            .filter(|n| *n <= self.max_resident_frames)
            .ok_or(SessionError::Invalid("resident audio frame limit reached"))?;
        // Obvious PCM-only refusal precedes bank/catalog construction. The full
        // ledger below measures actual metadata and simultaneous Vec storage.
        let pcm_floor = pcm_bytes(
            self.resident_frames
                .checked_add(incoming)
                .ok_or(SessionError::Invalid("resident frame count overflow"))?,
        )?;
        if pcm_floor > reservation.storage.max_retained_bytes
            || pcm_floor > reservation.storage.max_construction_peak_bytes
        {
            return Err(ResourcePreparationError::Storage(
                hex_engine::AudioStorageError::ReservationExceeded,
            )
            .into());
        }
        let source_rate = decoded.sample_rate();
        let frames = u64::try_from(decoded.frames())
            .map_err(|_| SessionError::Invalid("source frame count exceeds project range"))?;
        let native = decoded
            .into_native_source_on_control(reservation.max_scan_frames, cancel)
            .map_err(|refused| SessionError::from(refused.reason))?;
        let (path, source) = native.into_parts();
        let media = self.media_bank();
        let table = self.sine_table();
        let legacy = pcm_bytes(self.resident_frames)?;
        let overview_budget = &self.overview_budget;
        let old_overviews = &self.source_overviews;
        let media_assets = &self.media;
        let native_assets = &mut self.native_assets;
        let shutdown = &self.shutdown;
        let transforms = &self.audio_transforms;
        let waveforms = &self.audio_clip_waveforms;
        let command = match destination {
            NativeAssetDestination::Unassigned => Command::ImportDeviceAsset {
                path,
                sample_rate: source_rate,
                frames,
            },
            NativeAssetDestination::NewInstrument {
                track,
                placement,
                state,
                source_slot,
            } => Command::ImportDeviceAssetIntoNewInstrument {
                track,
                placement,
                path,
                sample_rate: source_rate,
                frames,
                state,
                source_slot,
            },
            NativeAssetDestination::ExistingInstrument {
                track,
                device,
                expected_state,
                state,
                source_slot,
            } => Command::ImportDeviceAssetReplacingInstrument {
                track,
                device,
                expected_state,
                path,
                sample_rate: source_rate,
                frames,
                state,
                source_slot,
            },
        };
        let (created, ((prepared, source_overviews), _summary)) = self
            .editor
            .execute_prepared_transition(command, |old, candidate| {
                let id = candidate
                    .assets()
                    .find(|a| old.asset(a.id()).is_none())
                    .map(|a| a.id())
                    .ok_or(SessionError::Invalid(
                        "native import candidate has no asset",
                    ))?;
                native_assets.insert_checked_on_control(
                    (id, source),
                    legacy,
                    reservation.storage,
                    reservation.max_scan_frames,
                    cancel,
                    |stage| {
                        let source = stage.source(id).ok_or(SessionError::Invalid(
                            "staged original source is unavailable",
                        ))?;
                        let frames = source.resident_frames().ok_or(SessionError::Invalid(
                            "staged original source is not resident",
                        ))?;
                        let facts = stage.original_source_facts_on_control(id)?;
                        let overview =
                            crate::source_overview::OriginalSourceOverview::prepare_cancellable(
                                overview_budget,
                                frames,
                                source.sample_rate(),
                                facts,
                                |_| {
                                    cancel.load(Ordering::Acquire)
                                        || shutdown.load(Ordering::Acquire)
                                },
                            )?;
                        let source_overviews =
                            old_overviews.inserted(overview_budget, id, overview)?;
                        let mut prepared = prepare(
                            candidate,
                            &media,
                            table,
                            transforms,
                            waveforms,
                            stage,
                            media_assets,
                        )?;
                        if cancel.load(Ordering::Relaxed) || shutdown.load(Ordering::Relaxed) {
                            return Err(ResourcePreparationError::Cancelled.into());
                        }
                        finalize(candidate, &mut prepared, stage)?;
                        Ok::<_, SessionError>((prepared, source_overviews))
                    },
                )
            })?;
        self.source_overviews = source_overviews;
        Ok((created, prepared))
    }
}

#[cfg(test)]
mod tests;
