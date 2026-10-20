//! Exclusive control-side native source custody. No owning clone or callback
//! publication is available here. A ledger admission is not a retirement proof.
use super::*;
use crate::audio_storage::{Location, Owner};
use crate::{
    AudioRootStorageLedger, AudioRootStorageSummary, AudioStorageLedgerReservation,
    AudioStorageReservation, AudioStorageRole, OwnedAudioInputs, OwnedAudioSource,
};

struct ResidentDeviceAsset {
    bank: AudioInputStorageAndObligations,
    catalog: DeviceResourceCatalog,
    capacity_frames: u64,
}
/// One source namespace's native arm. IDs must remain disjoint from clip media.
/// Boxes keep root addresses stable; their payloads and actual Vec capacity are
/// included in the ledger, without duplicate inline-header charges.
#[derive(Default)]
pub struct ResidentDeviceAssets {
    assets: Vec<(AssetId, Box<ResidentDeviceAsset>)>,
}
struct StagedDeviceAsset {
    destination: Vec<(AssetId, Box<ResidentDeviceAsset>)>,
    asset: (AssetId, Box<ResidentDeviceAsset>),
    summary: AudioRootStorageSummary,
}
impl ResidentDeviceAssets {
    pub fn contains(&self, asset: AssetId) -> bool {
        self.assets.iter().any(|(id, _)| *id == asset)
    }
    pub fn ids(&self) -> impl Iterator<Item = AssetId> + '_ {
        self.assets.iter().map(|(id, _)| *id)
    }
    pub fn capacity_frames(&self) -> Result<u64, AudioStorageError> {
        self.assets.iter().try_fold(0_u64, |n, (_, a)| {
            n.checked_add(a.capacity_frames)
                .ok_or(AudioStorageError::Overflow)
        })
    }
    /// Borrow actual PCM; no source owner or alias escapes.
    pub fn source(&self, asset: AssetId) -> Option<crate::OwnedAudioSourceView<'_>> {
        let (_, a) = self.assets.iter().find(|(id, _)| *id == asset)?;
        a.bank.source_view(&a.bank.source_token(asset).ok()?).ok()
    }
    fn inventory<'a>(
        &'a self,
        candidate: Option<&'a ResidentDeviceAsset>,
        destination: Option<&'a Vec<(AssetId, Box<ResidentDeviceAsset>)>>,
        legacy_pcm_bytes: usize,
        reservation: AudioStorageLedgerReservation,
    ) -> Result<AudioRootStorageSummary, AudioStorageError> {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation)?;
        ledger.mutate(|ledger| {
            ledger.root(self, AudioStorageRole::Current)?;
            if let Some(destination) = destination {
                // Future retained backing plus simultaneous old backing during
                // the infallible move. A staging Vec header lives on control.
                ledger.vec(destination)?;
                ledger.growth(
                    self.assets
                        .capacity()
                        .checked_mul(std::mem::size_of::<(AssetId, Box<ResidentDeviceAsset>)>())
                        .and_then(|n| n.checked_add(std::mem::size_of_val(destination)))
                        .ok_or(AudioStorageError::Overflow)?,
                )?;
            } else {
                ledger.vec(&self.assets)?;
            }
            // Legacy PCM keeps its existing capacity accounting. This explicit
            // allowance never upgrades it to constructor-paired native custody.
            ledger.bytes(
                Owner::exclusive(self),
                Location("legacy-pcm-allowance", 0),
                legacy_pcm_bytes,
            )?;
            for asset in self.assets.iter().map(|(_, a)| a.as_ref()).chain(candidate) {
                ledger.root(asset, AudioStorageRole::Current)?;
                asset.bank.visit_owned_storage(ledger)?;
                ledger.vec(&asset.catalog.sources)?;
            }
            Ok(())
        })?;
        ledger.summary()
    }
    pub fn storage_on_control(
        &self,
        legacy_pcm_bytes: usize,
        reservation: AudioStorageLedgerReservation,
    ) -> Result<AudioRootStorageSummary, AudioStorageError> {
        self.inventory(None, None, legacy_pcm_bytes, reservation)
    }
    /// Admit the owned decoded source and construction overlap before canonical
    /// commit. No old container capacity or owner changes on any refusal.
    fn stage_on_control(
        &self,
        asset: AssetId,
        source: OwnedAudioSource,
        legacy_pcm_bytes: usize,
        reservation: AudioStorageLedgerReservation,
        max_scan_frames: u64,
        cancel: &AtomicBool,
    ) -> Result<StagedDeviceAsset, ResourcePreparationError> {
        cancellation(cancel)?;
        if self.contains(asset) || self.assets.len() >= hex_project::MAX_ASSETS {
            return Err(ResourcePreparationError::InvalidSource);
        }
        // Row limits bound both the scan and actual metadata allocation.
        let rows = self
            .assets
            .len()
            .checked_add(2)
            .ok_or(AudioStorageError::Overflow)?;
        if rows > reservation.root_rows {
            return Err(AudioStorageError::Capacity.into());
        }
        let capacity_frames = match &source.kind {
            crate::audio_only_inputs::SourceKind::Resident(audio) => {
                u64::try_from(audio.pcm_capacity_frames())
                    .map_err(|_| AudioStorageError::Overflow)?
            }
            _ => return Err(ResourcePreparationError::InvalidSource),
        };
        let expected = OriginalResourceSource {
            asset,
            sample_rate: source.sample_rate(),
            frames: source.frames(),
        };
        let mut sources = Vec::new();
        sources
            .try_reserve_exact(1)
            .map_err(|_| AudioStorageError::Allocation)?;
        sources.push((asset, source));
        let inputs = OwnedAudioInputs::try_new(
            sources,
            AudioStorageReservation {
                allocation_items: 2,
                handle_obligations: 1,
                max_prepared_data_bytes: reservation.max_retained_bytes,
                max_construction_data_bytes: reservation.max_construction_peak_bytes,
                caller_retained_bytes: 0,
                caller_construction_bytes: 0,
            },
        )
        .map_err(|r| ResourcePreparationError::Storage(r.reason))?;
        let bank = inputs
            .prepare_input_storage_on_control()
            .map_err(|r| ResourcePreparationError::Storage(r.reason))?;
        let catalog = DeviceResourceCatalog::prepare_on_control(
            &bank,
            &[expected],
            ResourceCatalogReservation {
                metadata_bytes: reservation.max_retained_bytes,
                hash_frames: max_scan_frames,
            },
            cancel,
        )?;
        let asset = (
            asset,
            Box::new(ResidentDeviceAsset {
                bank,
                catalog,
                capacity_frames,
            }),
        );
        let mut destination = Vec::new();
        destination
            .try_reserve_exact(
                self.assets
                    .len()
                    .checked_add(1)
                    .ok_or(AudioStorageError::Overflow)?,
            )
            .map_err(|_| AudioStorageError::Allocation)?;
        let summary = self.inventory(
            Some(&asset.1),
            Some(&destination),
            legacy_pcm_bytes,
            reservation,
        )?;
        cancellation(cancel)?;
        Ok(StagedDeviceAsset {
            destination,
            asset,
            summary,
        })
    }
    /// Validate the staged source with the caller's canonical preparation, then
    /// perform fixed moves. The staging value cannot escape or be replayed into
    /// another store. The callback must leave its own state unchanged on refusal.
    pub fn insert_on_control<T, E: From<ResourcePreparationError>>(
        &mut self,
        source: (AssetId, OwnedAudioSource),
        legacy_pcm_bytes: usize,
        reservation: AudioStorageLedgerReservation,
        max_scan_frames: u64,
        cancel: &AtomicBool,
        finish: impl FnOnce() -> Result<T, E>,
    ) -> Result<(T, AudioRootStorageSummary), E> {
        let mut staged = self
            .stage_on_control(
                source.0,
                source.1,
                legacy_pcm_bytes,
                reservation,
                max_scan_frames,
                cancel,
            )
            .map_err(E::from)?;
        cancellation(cancel).map_err(E::from)?;
        let result = finish()?;
        staged.destination.append(&mut self.assets);
        staged.destination.push(staged.asset);
        self.assets = staged.destination;
        Ok((result, staged.summary))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source() -> OwnedAudioSource {
        OwnedAudioSource::try_resident(8000, vec![[0.25, -0.5]; 4])
            .ok()
            .unwrap()
    }
    fn id(n: u64) -> AssetId {
        let mut p = hex_project::Editor::new(hex_project::Project::new(8000).unwrap(), 8).unwrap();
        let mut result = None;
        for _ in 0..n {
            if let hex_project::Created::Asset(id) = p
                .execute(hex_project::Command::AddAsset {
                    path: "fixture.wav".into(),
                    sample_rate: 8000,
                    frames: 4,
                })
                .unwrap()
            {
                result = Some(id);
            }
        }
        result.unwrap()
    }
    #[test]
    fn native_asset_commit_boundary_refusal_and_cancel_preserve_old_container() {
        let r = AudioStorageLedgerReservation {
            data_rows: 128,
            handle_rows: 16,
            root_rows: 8,
            max_retained_bytes: 100000,
            max_construction_peak_bytes: 100000,
        };
        let mut assets = ResidentDeviceAssets::default();
        let cancel = AtomicBool::new(false);
        assets
            .insert_on_control((id(1), source()), 0, r, 4, &cancel, || {
                Ok::<_, ResourcePreparationError>(())
            })
            .unwrap();
        let before = assets.storage_on_control(0, r).unwrap();
        let pointer = assets
            .source(id(1))
            .unwrap()
            .resident_frames()
            .unwrap()
            .as_ptr();
        let refused = assets.insert_on_control((id(2), source()), 0, r, 4, &cancel, || {
            cancel.store(true, Ordering::Relaxed);
            Err::<(), _>(ResourcePreparationError::Cancelled)
        });
        assert_eq!(refused.err(), Some(ResourcePreparationError::Cancelled));
        assert_eq!(assets.storage_on_control(0, r).unwrap(), before);
        assert_eq!(
            assets
                .source(id(1))
                .unwrap()
                .resident_frames()
                .unwrap()
                .as_ptr(),
            pointer
        );
        assert!(!assets.contains(id(2)));
        cancel.store(false, Ordering::Relaxed);
        // Once preparation succeeds, cancellation cannot cause a reported
        // refusal after a committing callback. Remaining moves are infallible.
        assets
            .insert_on_control((id(2), source()), 0, r, 4, &cancel, || {
                cancel.store(true, Ordering::Relaxed);
                Ok::<_, ResourcePreparationError>(())
            })
            .unwrap();
        assert!(assets.contains(id(2)));
        // Measure the real second admission, including old/new container overlap.
        // A fresh equivalent store must refuse one byte below that observed peak
        // without changing its existing source, rows or Vec capacity.
        cancel.store(false, Ordering::Relaxed);
        let mut measured = ResidentDeviceAssets::default();
        measured
            .insert_on_control((id(1), source()), 0, r, 4, &cancel, || {
                Ok::<_, ResourcePreparationError>(())
            })
            .unwrap();
        let (_, observed) = measured
            .insert_on_control((id(2), source()), 0, r, 4, &cancel, || {
                Ok::<_, ResourcePreparationError>(())
            })
            .unwrap();
        let mut bounded = ResidentDeviceAssets::default();
        bounded
            .insert_on_control((id(1), source()), 0, r, 4, &cancel, || {
                Ok::<_, ResourcePreparationError>(())
            })
            .unwrap();
        let before = bounded.storage_on_control(0, r).unwrap();
        let mut limited = r;
        limited.max_retained_bytes = observed.retained_bytes;
        limited.max_construction_peak_bytes = observed.construction_peak_bytes - 1;
        assert_eq!(
            bounded
                .insert_on_control((id(2), source()), 0, limited, 4, &cancel, || Ok::<
                    _,
                    ResourcePreparationError,
                >(
                    ()
                ))
                .err(),
            Some(ResourcePreparationError::Storage(
                AudioStorageError::ReservationExceeded
            ))
        );
        assert_eq!(bounded.storage_on_control(0, r).unwrap(), before);
        limited.max_construction_peak_bytes = observed.construction_peak_bytes;
        bounded
            .insert_on_control((id(2), source()), 0, limited, 4, &cancel, || {
                Ok::<_, ResourcePreparationError>(())
            })
            .unwrap();
    }
}
