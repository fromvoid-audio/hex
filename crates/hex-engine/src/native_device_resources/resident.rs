//! Exclusive control-side native source custody. No owning clone or callback
//! publication is available here. A ledger admission is not a retirement proof.
use super::*;
use crate::audio_storage::{Location, Owner};
use crate::{
    AudioRootStorageLedger, AudioRootStorageSummary, AudioStorageLedgerReservation,
    AudioStorageReservation, AudioStorageRole, OwnedAudioInputs, OwnedAudioSource,
};

pub(super) struct ResidentDeviceAsset {
    pub(super) bank: AudioInputStorageAndObligations,
    pub(super) catalog: DeviceResourceCatalog,
    capacity_frames: u64,
    #[cfg(feature = "instrument-test-support")]
    _dropped: super::control_drop_probe::SourceDropped,
}
pub(super) struct NativeBankLoan {
    pub(super) owner: crate::owned_shared::OwnedShared<ResidentDeviceAsset>,
    handle: u64,
}
impl std::ops::Deref for NativeBankLoan {
    type Target = ResidentDeviceAsset;
    fn deref(&self) -> &Self::Target {
        &self.owner
    }
}
impl NativeBankLoan {
    pub(super) fn capture_on_control(&self) -> Result<Self, AudioStorageError> {
        Ok(Self {
            owner: self.owner.clone_on_control(),
            handle: crate::audio_only_inputs::handle()?,
        })
    }
    pub(super) fn visit<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<u64, AudioStorageError> {
        ledger.shared(
            self.owner.arc(),
            self.owner.parts(),
            Owner::exclusive(self),
            self.handle,
            None,
        )?;
        if ledger.native_payload_once(self.owner.as_ref())? {
            self.bank.visit_owned_storage(ledger)?;
            ledger.vec(&self.catalog.sources)?;
            return Ok(self.capacity_frames);
        }
        Ok(0)
    }
}
/// One source namespace's native arm. IDs must remain disjoint from clip media.
/// Boxes keep root addresses stable; their payloads and actual Vec capacity are
/// included in the ledger, without duplicate inline-header charges.
#[derive(Default)]
pub struct ResidentDeviceAssets {
    pub(super) assets: Vec<(AssetId, NativeBankLoan)>,
}
pub(super) struct StagedDeviceAssets {
    pub(super) destination: Vec<(AssetId, NativeBankLoan)>,
    summary: AudioRootStorageSummary,
}
/// Lifetime-bound view of the actual uncommitted owner/container. No owner
/// projection or clone is available, and it cannot outlive the transaction.
pub struct StagedNativeAssetView<'a> {
    pub(super) store: &'a ResidentDeviceAssets,
    pub(super) staged: &'a StagedDeviceAssets,
}
impl StagedNativeAssetView<'_> {
    pub(super) fn loan(&self, asset: AssetId) -> Option<&NativeBankLoan> {
        self.staged
            .destination
            .iter()
            .chain(&self.store.assets)
            .find(|(id, _)| *id == asset)
            .map(|(_, loan)| loan)
    }

    /// Borrow original PCM from this exact staged transaction; no owning handle
    /// escapes and the view cannot outlive either uncommitted source or store.
    pub fn source(&self, asset: AssetId) -> Option<crate::OwnedAudioSourceView<'_>> {
        let bank = &self.loan(asset)?.bank;
        bank.source_view(&bank.source_token(asset).ok()?).ok()
    }

    /// Expected content from this actual immutable source catalog.
    pub fn original_source_facts_on_control(
        &self,
        asset: AssetId,
    ) -> Result<hex_project::OriginalSourceFacts, ResourcePreparationError> {
        let identity = self
            .loan(asset)
            .and_then(|loan| loan.catalog.identity(asset))
            .ok_or(ResourcePreparationError::InvalidSource)?;
        hex_project::OriginalSourceFacts::new(identity.derivation_version, identity.content)
            .map_err(|_| ResourcePreparationError::InvalidSource)
    }
    pub fn inspect(
        &self,
        retained: &super::DeviceResourcePhase<'_>,
        construction: &super::DeviceResourcePhase<'_>,
        reservation: AudioStorageLedgerReservation,
    ) -> Result<super::DeviceResourceInventory, ResourcePreparationError> {
        super::admission::inspect_staged_union(retained, construction, reservation, self)
    }
}
/// Pure authored template for a prepared source import. No source identity or
/// processing capability is granted; the candidate binds its freshly allocated
/// AssetId and resource preparation still validates the actual staged bank.
pub fn sampler_import_template_on_control(
    frames: usize,
) -> Result<(NativeDeviceState, hex_lattice_contract::DeviceAssetSlot), ResourcePreparationError> {
    let state = hex_devices::sampler::state::SamplerState::for_source_frames(frames)
        .and_then(|s| s.to_native())
        .map_err(|_| ResourcePreparationError::InvalidScope)?;
    let slot = hex_lattice_contract::DeviceAssetSlot::new(hex_devices::sampler::state::SOURCE_SLOT)
        .map_err(|_| ResourcePreparationError::InvalidScope)?;
    Ok((state, slot))
}
impl ResidentDeviceAssets {
    pub fn contains(&self, asset: AssetId) -> bool {
        self.assets.iter().any(|(id, _)| *id == asset)
    }
    pub fn ids(&self) -> impl Iterator<Item = AssetId> + '_ {
        self.assets.iter().map(|(id, _)| *id)
    }
    /// Copy expected original content from the actual immutable source catalog.
    /// No bank token, PCM owner or native admission authority escapes.
    pub fn original_source_facts_on_control(
        &self,
        asset: AssetId,
    ) -> Result<hex_project::OriginalSourceFacts, ResourcePreparationError> {
        let source = self
            .assets
            .iter()
            .find(|(id, _)| *id == asset)
            .map(|(_, source)| source)
            .ok_or(ResourcePreparationError::InvalidSource)?;
        let identity = source
            .catalog
            .identity(asset)
            .ok_or(ResourcePreparationError::InvalidSource)?;
        hex_project::OriginalSourceFacts::new(identity.derivation_version, identity.content)
            .map_err(|_| ResourcePreparationError::InvalidSource)
    }
    pub fn capacity_frames(&self) -> Result<u64, AudioStorageError> {
        self.assets.iter().try_fold(0_u64, |n, (_, a)| {
            n.checked_add(a.capacity_frames)
                .ok_or(AudioStorageError::Overflow)
        })
    }
    /// Authored settings and exact canonical source reference only. This does
    /// not prepare a runtime or grant publication/ownership capability.
    pub fn sampler_state_on_control(
        &self,
        asset: AssetId,
    ) -> Result<NativeDeviceState, ResourcePreparationError> {
        let source = self
            .source(asset)
            .ok_or(ResourcePreparationError::InvalidSource)?;
        let frames = source
            .resident_frames()
            .ok_or(ResourcePreparationError::InvalidSource)?
            .len();
        hex_devices::sampler::state::SamplerState::for_source_frames(frames)
            .and_then(|s| s.to_native())
            .and_then(|s| {
                s.with_asset_references(vec![hex_lattice_contract::DeviceAssetReference {
                    slot: hex_lattice_contract::DeviceAssetSlot::new(
                        hex_devices::sampler::state::SOURCE_SLOT,
                    )?,
                    asset: hex_lattice_contract::DeviceAssetKey::new(asset.value())?,
                }])
            })
            .map_err(|_| ResourcePreparationError::InvalidScope)
    }
    /// Borrow actual PCM; no source owner or alias escapes.
    pub fn source(&self, asset: AssetId) -> Option<crate::OwnedAudioSourceView<'_>> {
        let (_, a) = self.assets.iter().find(|(id, _)| *id == asset)?;
        a.bank.source_view(&a.bank.source_token(asset).ok()?).ok()
    }
    fn inventory<'a>(
        &'a self,
        destination: Option<&'a Vec<(AssetId, NativeBankLoan)>>,
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
                        .checked_mul(std::mem::size_of::<(AssetId, NativeBankLoan)>())
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
            for (_, asset) in self.assets.iter().chain(destination.into_iter().flatten()) {
                asset.visit(ledger)?;
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
        self.inventory(None, legacy_pcm_bytes, reservation)
    }
    fn stage_bank_on_control(
        asset: AssetId,
        source: OwnedAudioSource,
        capacity_frames: u64,
        reservation: AudioStorageLedgerReservation,
        max_scan_frames: u64,
        cancel: &AtomicBool,
    ) -> Result<NativeBankLoan, ResourcePreparationError> {
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
        let owner = crate::owned_shared::OwnedShared::new_owned(ResidentDeviceAsset {
            bank,
            catalog,
            capacity_frames,
            #[cfg(feature = "instrument-test-support")]
            _dropped: super::control_drop_probe::SourceDropped::new(),
        });
        if owner.parts().is_none() {
            return Err(AudioStorageError::UnsupportedConstructor.into());
        }
        Ok(NativeBankLoan {
            owner,
            handle: crate::audio_only_inputs::handle()?,
        })
    }

    /// Admit the exact set without changing any current owner or capacity.
    fn stage_on_control(
        &self,
        sources: impl ExactSizeIterator<Item = (AssetId, OwnedAudioSource)>,
        legacy_pcm_bytes: usize,
        reservation: AudioStorageLedgerReservation,
        max_scan_frames: u64,
        cancel: &AtomicBool,
    ) -> Result<StagedDeviceAssets, ResourcePreparationError> {
        cancellation(cancel)?;
        let count = sources.len();
        let total = self
            .assets
            .len()
            .checked_add(count)
            .ok_or(AudioStorageError::Overflow)?;
        if count == 0 || total > hex_project::MAX_ASSETS {
            return Err(ResourcePreparationError::InvalidSource);
        }
        if total.checked_add(1).ok_or(AudioStorageError::Overflow)? > reservation.root_rows {
            return Err(AudioStorageError::Capacity.into());
        }

        let mut destination = Vec::new();
        destination
            .try_reserve_exact(total)
            .map_err(|_| AudioStorageError::Allocation)?;
        let frame_bytes = std::mem::size_of::<crate::Frame>();
        let mut pcm_floor = usize::try_from(self.capacity_frames()?)
            .ok()
            .and_then(|n| n.checked_mul(frame_bytes))
            .and_then(|n| n.checked_add(legacy_pcm_bytes))
            .ok_or(AudioStorageError::Overflow)?;
        let mut work = max_scan_frames;
        for (asset, source) in sources {
            cancellation(cancel)?;
            if destination.len() >= count
                || self.contains(asset)
                || destination.iter().any(|(id, _)| *id == asset)
            {
                return Err(ResourcePreparationError::InvalidSource);
            }
            let frames = match &source.kind {
                crate::audio_only_inputs::SourceKind::Resident(audio) => {
                    audio.pcm_capacity_frames()
                }
                _ => return Err(ResourcePreparationError::InvalidSource),
            };
            pcm_floor = frames
                .checked_mul(frame_bytes)
                .and_then(|bytes| pcm_floor.checked_add(bytes))
                .ok_or(AudioStorageError::Overflow)?;
            if pcm_floor > reservation.max_retained_bytes
                || pcm_floor > reservation.max_construction_peak_bytes
            {
                return Err(AudioStorageError::ReservationExceeded.into());
            }
            let source_frames = source.frames();
            work = work
                .checked_sub(source_frames)
                .ok_or(ResourcePreparationError::WorkCapacity)?;
            let capacity = u64::try_from(frames).map_err(|_| AudioStorageError::Overflow)?;
            let loan = Self::stage_bank_on_control(
                asset,
                source,
                capacity,
                reservation,
                source_frames,
                cancel,
            )?;
            destination.push((asset, loan));
        }
        if destination.len() != count {
            return Err(ResourcePreparationError::InvalidSource);
        }
        let summary = self.inventory(Some(&destination), legacy_pcm_bytes, reservation)?;
        cancellation(cancel)?;
        Ok(StagedDeviceAssets {
            destination,
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
        self.insert_checked_on_control(
            source,
            legacy_pcm_bytes,
            reservation,
            max_scan_frames,
            cancel,
            |_| finish(),
        )
    }
    pub fn insert_checked_on_control<T, E: From<ResourcePreparationError>>(
        &mut self,
        source: (AssetId, OwnedAudioSource),
        legacy_pcm_bytes: usize,
        reservation: AudioStorageLedgerReservation,
        max_scan_frames: u64,
        cancel: &AtomicBool,
        finish: impl FnOnce(&StagedNativeAssetView<'_>) -> Result<T, E>,
    ) -> Result<(T, AudioRootStorageSummary), E> {
        self.insert_many_checked_on_control(
            std::iter::once(source),
            legacy_pcm_bytes,
            reservation,
            max_scan_frames,
            cancel,
            finish,
        )
    }

    /// Prepare one exact original set, then commit only reserved moves after the
    /// caller's canonical preparation and publication succeed.
    pub fn insert_many_checked_on_control<T, E: From<ResourcePreparationError>>(
        &mut self,
        sources: impl ExactSizeIterator<Item = (AssetId, OwnedAudioSource)>,
        legacy_pcm_bytes: usize,
        reservation: AudioStorageLedgerReservation,
        max_scan_frames: u64,
        cancel: &AtomicBool,
        finish: impl FnOnce(&StagedNativeAssetView<'_>) -> Result<T, E>,
    ) -> Result<(T, AudioRootStorageSummary), E> {
        let mut staged = self
            .stage_on_control(
                sources,
                legacy_pcm_bytes,
                reservation,
                max_scan_frames,
                cancel,
            )
            .map_err(E::from)?;
        cancellation(cancel).map_err(E::from)?;
        let result = finish(&StagedNativeAssetView {
            store: self,
            staged: &staged,
        })?;
        let additions = staged.destination.len();
        staged.destination.append(&mut self.assets);
        staged.destination.rotate_left(additions);
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

    #[test]
    fn original_batch_exposes_exact_union_and_commits_only_after_preparation() {
        let reservation = AudioStorageLedgerReservation {
            data_rows: 128,
            handle_rows: 16,
            root_rows: 8,
            max_retained_bytes: 100_000,
            max_construction_peak_bytes: 100_000,
        };
        let cancel = AtomicBool::new(false);
        let mut assets = ResidentDeviceAssets::default();
        let [old, left, right] = [id(1), id(2), id(3)];
        assets
            .insert_on_control((old, source()), 0, reservation, 4, &cancel, || {
                Ok::<_, ResourcePreparationError>(())
            })
            .unwrap();
        let before = assets.storage_on_control(0, reservation).unwrap();
        let old_pointer = assets
            .source(old)
            .unwrap()
            .resident_frames()
            .unwrap()
            .as_ptr();
        let other_pcm = [[-0.5, 0.75]; 4];
        let sources = || {
            [
                (left, source()),
                (
                    right,
                    OwnedAudioSource::try_resident(11025, other_pcm.to_vec())
                        .ok()
                        .unwrap(),
                ),
            ]
            .into_iter()
        };
        let refusal =
            assets.insert_many_checked_on_control(sources(), 0, reservation, 7, &cancel, |_| {
                panic_result("aggregate work budget must refuse before preparation")
            });
        assert_eq!(refusal.err(), Some(ResourcePreparationError::WorkCapacity));

        let inspect = |stage: &StagedNativeAssetView<'_>| {
            assert_eq!(
                stage
                    .source(old)
                    .unwrap()
                    .resident_frames()
                    .unwrap()
                    .as_ptr(),
                old_pointer
            );
            assert_eq!(stage.source(left).unwrap().sample_rate(), 8000);
            assert_eq!(
                stage.source(right).unwrap().resident_frames().unwrap(),
                other_pcm
            );
            assert_eq!(
                stage.original_source_facts_on_control(right).unwrap(),
                super::super::original_pcm_facts_on_control(11025, &other_pcm, 4, &cancel).unwrap(),
            );
            let phase = super::super::DeviceResourcePhase {
                stores: &[stage.store],
                registered: &[],
                selections: &[],
                outbound: &[],
                legacy: &[],
                checked_runtime_bytes: 0,
                construction_bytes: 0,
            };
            let union = stage.inspect(&phase, &phase, reservation).unwrap();
            assert_eq!(union.retained_frames, 12);
        };
        let refused =
            assets.insert_many_checked_on_control(sources(), 0, reservation, 8, &cancel, |stage| {
                inspect(stage);
                Err::<(), _>(ResourcePreparationError::Cancelled)
            });
        assert_eq!(refused.err(), Some(ResourcePreparationError::Cancelled));
        assert_eq!(assets.storage_on_control(0, reservation).unwrap(), before);
        assert_eq!(assets.ids().collect::<Vec<_>>(), [old]);

        let duplicate = assets.insert_many_checked_on_control(
            [(left, source()), (left, source())].into_iter(),
            0,
            reservation,
            8,
            &cancel,
            |_| panic_result("duplicate identity must refuse before preparation"),
        );
        assert_eq!(
            duplicate.err(),
            Some(ResourcePreparationError::InvalidSource)
        );
        assert_eq!(assets.ids().collect::<Vec<_>>(), [old]);

        assets
            .insert_many_checked_on_control(sources(), 0, reservation, 8, &cancel, |stage| {
                inspect(stage);
                cancel.store(true, Ordering::Relaxed);
                Ok::<_, ResourcePreparationError>(())
            })
            .unwrap();
        assert_eq!(assets.ids().collect::<Vec<_>>(), [old, left, right]);
        assert_eq!(
            assets
                .source(old)
                .unwrap()
                .resident_frames()
                .unwrap()
                .as_ptr(),
            old_pointer
        );
    }

    fn panic_result(message: &'static str) -> Result<(), ResourcePreparationError> {
        panic!("{message}")
    }

    #[test]
    fn refused_original_set_does_not_publish_project_or_history() {
        use hex_project::{Command, Created, Editor, Invalid, Project};
        fn import(
            editor: &mut Editor,
            assets: &mut ResidentDeviceAssets,
            accept: bool,
        ) -> Result<(), Invalid> {
            let commands = ["a.wav", "b.wav"]
                .into_iter()
                .map(|path| Command::ImportDeviceAsset {
                    path: path.into(),
                    sample_rate: 8000,
                    frames: 4,
                })
                .collect();
            let reservation = AudioStorageLedgerReservation {
                data_rows: 128,
                handle_rows: 16,
                root_rows: 8,
                max_retained_bytes: 100_000,
                max_construction_peak_bytes: 100_000,
            };
            editor.execute_many_prepared(commands, |candidate, created| {
                let ids: Vec<_> = created
                    .iter()
                    .map(|result| match *result {
                        Created::Asset(id) => id,
                        _ => panic!("original asset result"),
                    })
                    .collect();
                assert_eq!(ids.len(), 2);
                assert_eq!(candidate.assets().count(), 2);
                assets
                    .insert_many_checked_on_control(
                        ids.iter().map(|&id| (id, source())),
                        0,
                        reservation,
                        8,
                        &AtomicBool::new(false),
                        |stage| {
                            assert!(ids.iter().all(|&id| stage.source(id).is_some()));
                            if accept {
                                Ok(())
                            } else {
                                Err(ResourcePreparationError::InvalidScope)
                            }
                        },
                    )
                    .map_err(|_| Invalid("original set not prepared"))
            })?;
            Ok(())
        }
        let mut editor = Editor::new(Project::new(8000).unwrap(), 8).unwrap();
        let mut assets = ResidentDeviceAssets::default();
        let before = editor.project().snapshot();
        assert_eq!(
            import(&mut editor, &mut assets, false),
            Err(Invalid("original set not prepared"))
        );
        assert_eq!(editor.project().snapshot(), before);
        assert!(!editor.can_undo());
        assert_eq!(assets.ids().count(), 0);
        import(&mut editor, &mut assets, true).unwrap();
        assert_eq!(
            editor
                .project()
                .assets()
                .map(|a| a.id())
                .collect::<Vec<_>>(),
            assets.ids().collect::<Vec<_>>()
        );
        assert!(editor.undo());
        assert_eq!(editor.project().assets().count(), 0);
        assert!(!editor.can_undo());
    }
}
