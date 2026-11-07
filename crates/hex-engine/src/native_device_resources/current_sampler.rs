//! Exact native constructor metadata, bound to the actual new input bank.
//! No PCM owner or completed graph is accepted from an external compiler.
use super::*;
use crate::owned_shared::OwnedShared;
use crate::{AudioRootStorageLedger, OwnedAudioInputs};
use hex_project::{DeviceId, Project, TrackId};

/// Persistent construction/cleanup destinations. The original bank remains
/// rooted here while a current runtime holds its distinct shared handle.
#[derive(Default)]
pub(crate) struct CurrentSamplerConstruction {
    metadata: Option<CurrentSamplerMetadata>,
    owner: Option<OwnedShared<CurrentSamplerSources>>,
    restored: Option<CurrentSamplerSources>,
    handle: u64,
}
impl CurrentSamplerConstruction {
    pub(crate) fn prepare_metadata(
        &mut self,
        bank: &OwnedAudioInputs,
        project: &Project,
        work: u64,
        metadata_bytes: usize,
        cancel: &AtomicBool,
        effects: bool,
    ) -> Result<(), AudioStorageError> {
        if self.metadata.is_some()
            || self.owner.is_some()
            || self.restored.is_some()
            || self.handle != 0
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        OwnedShared::<CurrentSamplerSources>::declared_parts()
            .ok_or(AudioStorageError::UnsupportedConstructor)?;
        self.handle = crate::audio_only_inputs::handle()?;
        self.metadata = Some(
            CurrentSamplerMetadata::prepare(
                bank,
                project,
                ResourceCatalogReservation {
                    metadata_bytes,
                    hash_frames: work,
                },
                cancel,
                effects,
            )
            .map_err(|e| match e {
                ResourcePreparationError::Storage(e) => e,
                ResourcePreparationError::MetadataCapacity
                | ResourcePreparationError::WorkCapacity => AudioStorageError::ReservationExceeded,
                _ => AudioStorageError::UnsupportedGraph,
            })?,
        );
        Ok(())
    }
    pub(crate) fn metadata(&self) -> Option<&CurrentSamplerMetadata> {
        self.metadata
            .as_ref()
            .or_else(|| self.owner.as_ref().map(|o| &o.metadata))
            .or_else(|| self.restored.as_ref().map(|o| &o.metadata))
    }
    pub(crate) fn bank(&self) -> Option<&OwnedAudioInputs> {
        self.owner
            .as_ref()
            .map(|o| &o.bank)
            .or_else(|| self.restored.as_ref().map(|o| &o.bank))
    }
    pub(crate) fn wrap_bank(
        &mut self,
        bank: &mut Option<OwnedAudioInputs>,
    ) -> Result<(), AudioStorageError> {
        if self.owner.is_some()
            || self.restored.is_some()
            || self.metadata.is_none()
            || bank.is_none()
            || self.handle == 0
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        // Both destinations validated. Source ownership moves only through these
        // fixed constructor fields; no provider/fallible call follows either take.
        match (bank.take(), self.metadata.take()) {
            (Some(bank), Some(metadata)) => {
                self.owner = Some(OwnedShared::new_owned(CurrentSamplerSources {
                    bank,
                    metadata,
                }))
            }
            (original, metadata) => {
                *bank = original;
                self.metadata = metadata;
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        Ok(())
    }
    pub(crate) fn runtime_capture(
        &self,
    ) -> Result<Option<(OwnedShared<CurrentSamplerSources>, u64)>, AudioStorageError> {
        let Some(owner) = self.owner.as_ref() else {
            return if self.metadata.is_some() || self.restored.is_some() {
                Err(AudioStorageError::SourceMismatch)
            } else {
                Ok(None)
            };
        };
        let handle = crate::audio_only_inputs::handle()?;
        Ok(Some((owner.clone_on_control(), handle)))
    }
    pub(crate) fn restore_bank(
        &mut self,
        bank: &mut Option<OwnedAudioInputs>,
    ) -> Result<(), AudioStorageError> {
        if self.owner.is_none() && self.restored.is_none() {
            return Ok(());
        }
        if bank.is_some() || self.metadata.is_some() {
            return Err(AudioStorageError::SourceMismatch);
        }
        OwnedShared::restore_value_on_control(&mut self.owner, &mut self.restored)?;
        if let Some(CurrentSamplerSources {
            bank: original,
            metadata,
        }) = self.restored.take()
        {
            *bank = Some(original);
            self.metadata = Some(metadata);
        }
        Ok(())
    }
    pub(crate) fn clear_metadata(&mut self) -> Result<(), AudioStorageError> {
        if self.owner.is_some() || self.restored.is_some() {
            return Err(AudioStorageError::SourceMismatch);
        }
        drop(self.metadata.take());
        self.handle = 0;
        Ok(())
    }
    pub(crate) fn bytes(&self) -> Result<usize, AudioStorageError> {
        let Some(metadata) = self.metadata() else {
            return Ok(0);
        };
        let (payload, overhead) = OwnedShared::<CurrentSamplerSources>::declared_parts()
            .ok_or(AudioStorageError::UnknownStorage)?;
        metadata
            .backing_bytes()?
            .checked_add(payload)
            .and_then(|n| n.checked_add(overhead))
            .ok_or(AudioStorageError::Overflow)
    }
    pub(crate) fn visit<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        if let Some(owner) = self.owner.as_ref() {
            ledger.shared(
                owner.arc(),
                owner.parts(),
                crate::audio_storage::Owner::exclusive(owner),
                self.handle,
                None,
            )?;
            owner.visit(ledger)?;
        } else {
            if let Some(value) = self.restored.as_ref() {
                value.visit(ledger)?;
            }
            if let Some(metadata) = self.metadata.as_ref() {
                metadata.visit(ledger)?;
            }
        }
        Ok(())
    }
}

pub(crate) struct CurrentSamplerConfiguration {
    pub(crate) track: TrackId,
    pub(crate) device: DeviceId,
    pub(crate) input_gain: f32,
    pub(crate) output_gain: f32,
    pub(crate) prepared: hex_devices::sampler::PreparedSamplerRuntime,
    /// Any sampler-supported saved version; `prepared` was decoded from it.
    state_version: u32,
    payload: Vec<u8>,
    references: Vec<hex_lattice_contract::DeviceAssetReference>,
    scope: DeviceResourceScope,
}
impl CurrentSamplerConfiguration {
    pub(crate) fn matches_state(&self, state: &NativeDeviceState) -> bool {
        state.type_id() == hex_devices::sampler::state::TYPE_ID
            && state.state_version() == self.state_version
            && state.payload() == self.payload
            && state.asset_references() == self.references
            && state.input_gain_linear().to_bits() == self.input_gain.to_bits()
            && state.output_gain_linear().to_bits() == self.output_gain.to_bits()
    }
}
pub(crate) struct CurrentSamplerMetadata {
    pub(crate) id: u64,
    catalog: DeviceResourceCatalog,
    configurations: Vec<CurrentSamplerConfiguration>,
    pub(crate) tuning: hex_lattice::TuningTable,
}
/// Actual independent constructor bank and its configuration-scoped metadata.
/// Only sealed native custody and its current runtime retain this allocation.
pub(crate) struct CurrentSamplerSources {
    pub(crate) bank: OwnedAudioInputs,
    pub(crate) metadata: CurrentSamplerMetadata,
}
impl CurrentSamplerSources {
    pub(crate) fn resources(
        &self,
        track: TrackId,
    ) -> Result<ScopedDeviceResources<'_>, ResourcePreparationError> {
        self.metadata.resources(&self.bank, track)
    }
    pub(crate) fn visit<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        if ledger.native_payload_once(self)? {
            ledger.inputs(&self.bank)?;
            self.metadata.visit(ledger)?;
        }
        Ok(())
    }
}

/// Recognize only the intended finite, frame-zero native class. Existing
/// audio/Utility admission remains responsible for its own original class.
pub(crate) fn validate_project(project: &Project) -> Result<bool, AudioStorageError> {
    validate_project_with_effects(project, false)
}
pub(crate) fn validate_project_with_effects(
    project: &Project,
    effects: bool,
) -> Result<bool, AudioStorageError> {
    let present = requires_private_sampler_sources(project);
    if !present {
        return Ok(false);
    }
    // This closed constructor has one track-local generator/configuration per
    // row. Shared sources use ordinary PreparedDeviceResources publication,
    // whose receiver owns the single factory and source lease.
    if project
        .tracks()
        .iter()
        .any(|track| track.source_id().is_some() && !track.receives_source_audio())
    {
        return Err(AudioStorageError::UnsupportedGraph);
    }
    if project.loop_region().is_some_and(|r| r.enabled())
        || (!effects && !project.main_devices().is_empty())
        || !project.audio_feeds().is_empty()
        || !project.midi_routes().is_empty()
        || project.return_tracks().iter().any(|r| {
            r.devices()
                .iter()
                .any(|d| !d.state().asset_references().is_empty() || !effects)
        })
    {
        return Err(AudioStorageError::UnsupportedGraph);
    }
    // Finite Arrangement sources, including repeats and directed views, use
    // the same admitted occurrence compiler and exact constructor byte quote.
    crate::midi_compile::count_project_occurrences(project)
        .map_err(|_| AudioStorageError::UnsupportedGraph)?;
    for track in project.tracks() {
        if !track.gain_lane().spans().is_empty()
            || track.devices().iter().any(|d| {
                (!effects && d.state().type_id() != hex_devices::utility::UTILITY_TYPE_ID)
                    || !d.state().asset_references().is_empty()
            })
        {
            return Err(AudioStorageError::UnsupportedGraph);
        }
        let Some(device) = track.instrument() else {
            if !track.midi_clips().is_empty()
                || project.session_cells().iter().any(|c| {
                    c.track() == track.id()
                        && matches!(c.content(), hex_project::SessionCellContent::Midi(_))
                })
            {
                return Err(AudioStorageError::UnsupportedGraph);
            }
            continue;
        };
        let state = device.state();
        if state.type_id() != hex_devices::sampler::state::TYPE_ID
            || device.bypassed()
            || track.instrument_position() != Some(0)
            || state.asset_references().len() != 1
            || hex_devices::sampler::state::SamplerState::from_native(state).is_err()
        {
            return Err(AudioStorageError::UnsupportedGraph);
        }
        let reference = state.asset_references()[0];
        let asset = project
            .assets()
            .find(|a| a.id().value() == reference.asset.value())
            .ok_or(AudioStorageError::SourceMismatch)?;
        // One source namespace cannot simultaneously mean original sampler PCM
        // and a project-rate clip derivative in this closed constructor.
        if project.tracks().iter().any(|t| t.clips().iter().any(|c| c.referenced_assets().any(|id| id == asset.id())))
            || project.session_cells().iter().any(|c| matches!(c.content(), hex_project::SessionCellContent::Audio(a) if a.asset() == asset.id()))
        { return Err(AudioStorageError::UnsupportedGraph) }
    }
    Ok(true)
}

impl CurrentSamplerMetadata {
    /// Bank stays in its persistent preparation slot through every refusal.
    /// Catalog tokens are created after the bank's constructor custodian commit.
    pub(crate) fn prepare(
        bank: &OwnedAudioInputs,
        project: &Project,
        reservation: ResourceCatalogReservation,
        cancel: &AtomicBool,
        effects: bool,
    ) -> Result<Self, ResourcePreparationError> {
        if !validate_project_with_effects(project, effects)? {
            return Err(ResourcePreparationError::InvalidScope);
        }
        cancellation(cancel)?;
        let count = project
            .tracks()
            .iter()
            .filter(|t| t.instrument().is_some())
            .count();
        let minimum =
            quoted_bytes::<CurrentSamplerConfiguration>(count, std::mem::size_of::<Self>())?;
        if minimum > reservation.metadata_bytes {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let expected = || {
            project
                .assets()
                .filter(|asset| {
                    project
                        .tracks()
                        .iter()
                        .filter_map(|t| t.instrument())
                        .any(|d| {
                            d.state()
                                .asset_references()
                                .iter()
                                .any(|r| r.asset.value() == asset.id().value())
                        })
                })
                .map(|asset| OriginalResourceSource {
                    asset: asset.id(),
                    sample_rate: asset.source_sample_rate(),
                    frames: asset.source_frames(),
                })
        };
        let catalog = DeviceResourceCatalog::prepare_inputs_iter_on_control(
            bank,
            expected,
            reservation,
            cancel,
        )?;
        let tuning = project
            .tuning()
            .table()
            .map_err(|_| ResourcePreparationError::InvalidScope)?;
        let mut configurations = Vec::new();
        configurations
            .try_reserve_exact(count)
            .map_err(|_| AudioStorageError::Allocation)?;
        let mut metadata_bytes = quoted_bytes::<CurrentSamplerConfiguration>(
            configurations.capacity(),
            std::mem::size_of::<Self>(),
        )?
        .checked_add(catalog.declared_metadata_bytes()?)
        .ok_or(AudioStorageError::Overflow)?;
        for track in project.tracks() {
            let Some(device) = track.instrument() else {
                continue;
            };
            cancellation(cancel)?;
            let state = device.state();
            let scope = catalog.prepare_scope_on_control(state, reservation.metadata_bytes)?;
            let prepared = hex_devices::sampler::state::prepare_runtime_on_control(
                state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: project.sample_rate(),
                    tuning: &tuning,
                    resources: &catalog.resources_from_inputs(bank, &scope)?,
                },
                crate::mutable_instrument::MAX_MUTABLE_EVENTS_PER_QUERY,
            )
            .map_err(|_| ResourcePreparationError::InvalidScope)?;
            let mut payload = Vec::new();
            payload
                .try_reserve_exact(state.payload().len())
                .map_err(|_| AudioStorageError::Allocation)?;
            payload.extend_from_slice(state.payload());
            let mut references = Vec::new();
            references
                .try_reserve_exact(state.asset_references().len())
                .map_err(|_| AudioStorageError::Allocation)?;
            references.extend_from_slice(state.asset_references());
            metadata_bytes = metadata_bytes
                .checked_add(payload.capacity())
                .and_then(|n| {
                    n.checked_add(references.capacity().checked_mul(std::mem::size_of::<
                        hex_lattice_contract::DeviceAssetReference,
                    >())?)
                })
                .and_then(|n| n.checked_add(scope.declared_metadata_bytes().ok()?))
                .ok_or(AudioStorageError::Overflow)?;
            if metadata_bytes > reservation.metadata_bytes {
                return Err(ResourcePreparationError::MetadataCapacity);
            }
            configurations.push(CurrentSamplerConfiguration {
                track: track.id(),
                device: device.id(),
                input_gain: state.input_gain_linear(),
                output_gain: state.output_gain_linear(),
                prepared,
                state_version: state.state_version(),
                payload,
                references,
                scope,
            });
        }
        cancellation(cancel)?;
        Ok(Self {
            id: crate::audio_only_inputs::handle()?,
            catalog,
            configurations,
            tuning,
        })
    }
    pub(crate) fn backing_bytes(&self) -> Result<usize, AudioStorageError> {
        self.configurations.iter().try_fold(
            self.catalog
                .sources
                .capacity()
                .checked_mul(std::mem::size_of::<SourceFact>())
                .and_then(|n| {
                    n.checked_add(
                        self.configurations
                            .capacity()
                            .checked_mul(std::mem::size_of::<CurrentSamplerConfiguration>())?,
                    )
                })
                .ok_or(AudioStorageError::Overflow)?,
            |bytes, config| {
                bytes
                    .checked_add(config.payload.capacity())
                    .and_then(|n| {
                        n.checked_add(config.references.capacity().checked_mul(
                            std::mem::size_of::<hex_lattice_contract::DeviceAssetReference>(),
                        )?)
                    })
                    .and_then(|n| {
                        n.checked_add(
                            config
                                .scope
                                .bindings
                                .capacity()
                                .checked_mul(std::mem::size_of::<ScopeBinding>())?,
                        )
                    })
                    .ok_or(AudioStorageError::Overflow)
            },
        )
    }
    pub(crate) fn configurations(&self) -> &[CurrentSamplerConfiguration] {
        &self.configurations
    }
    pub(crate) fn configuration(&self, track: TrackId) -> Option<&CurrentSamplerConfiguration> {
        self.configurations.iter().find(|c| c.track == track)
    }
    pub(crate) fn resources<'a>(
        &'a self,
        bank: &'a OwnedAudioInputs,
        track: TrackId,
    ) -> Result<ScopedDeviceResources<'a>, ResourcePreparationError> {
        let config = self
            .configuration(track)
            .ok_or(ResourcePreparationError::InvalidScope)?;
        self.catalog.resources_from_inputs(bank, &config.scope)
    }
    pub(crate) fn visit<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.vec(&self.catalog.sources)?;
        ledger.vec(&self.configurations)?;
        for config in &self.configurations {
            ledger.vec(&config.scope.bindings)?;
            ledger.vec(&config.payload)?;
            ledger.vec(&config.references)?;
        }
        Ok(())
    }
}
