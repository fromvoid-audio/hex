//! Sealed control/export sidecar. Source ownership never enters public prepared
//! graph accessors. This type has no Clone, Deref or raw Arc projection.
use super::resident::{NativeBankLoan, ResidentDeviceAssets};
use super::*;
use crate::{AudioRootStorageLedger, AudioStorageError, owned_shared::OwnedShared};
use hex_lattice_contract::{DeviceAssetReference, NativeDeviceState};
use hex_project::{DeviceId, TrackId};

struct Configuration {
    track: TrackId,
    device: DeviceId,
    /// Any sampler-supported saved version; the runtime was prepared from it.
    state_version: u32,
    payload: Vec<u8>,
    references: Vec<DeviceAssetReference>,
    input: u32,
    output: u32,
    source: NativeBankLoan,
    scope: DeviceResourceScope,
    prepared: hex_devices::sampler::PreparedSamplerRuntime,
}
struct SnapshotData {
    id: u64,
    tuning: hex_lattice::TuningTable,
    sample_rate: u32,
    configurations: Vec<Configuration>,
}
/// Opaque source sidecar for a private Service-controlled offline snapshot.
/// Capturing is explicit; callers must inventory every current/staged/captured
/// selection together before committing or dispatching it. No RT admission.
pub struct ControlSamplerSources {
    data: OwnedShared<SnapshotData>,
    handle: u64,
}
impl ControlSamplerSources {
    pub fn prepare_on_control(
        store: &ResidentDeviceAssets,
        project: &hex_project::Project,
        metadata_bytes: usize,
        max_scan_frames: u64,
        cancel: &AtomicBool,
    ) -> Result<Self, ResourcePreparationError> {
        Self::prepare_with_source_on_control(
            project,
            metadata_bytes,
            max_scan_frames,
            cancel,
            |asset| {
                store
                    .assets
                    .iter()
                    .find(|(id, _)| *id == asset)
                    .map(|(_, source)| source)
            },
        )
    }
    pub fn prepare_staged_on_control(
        stage: &super::StagedNativeAssetView<'_>,
        project: &hex_project::Project,
        metadata_bytes: usize,
        max_scan_frames: u64,
        cancel: &AtomicBool,
    ) -> Result<Self, ResourcePreparationError> {
        Self::prepare_with_source_on_control(
            project,
            metadata_bytes,
            max_scan_frames,
            cancel,
            |asset| {
                if stage.staged.asset.0 == asset {
                    Some(&stage.staged.asset.1)
                } else {
                    stage
                        .store
                        .assets
                        .iter()
                        .find(|(id, _)| *id == asset)
                        .map(|(_, source)| source)
                }
            },
        )
    }
    fn prepare_with_source_on_control<'a>(
        project: &hex_project::Project,
        metadata_bytes: usize,
        max_scan_frames: u64,
        cancel: &AtomicBool,
        source_for: impl Fn(AssetId) -> Option<&'a NativeBankLoan>,
    ) -> Result<Self, ResourcePreparationError> {
        cancellation(cancel)?;
        let tuning = project
            .tuning()
            .table()
            .map_err(|_| ResourcePreparationError::InvalidScope)?;
        let count = project
            .tracks()
            .iter()
            .filter(|track| track.receives_source_audio())
            .filter(|t| {
                t.instrument()
                    .is_some_and(|d| hex_devices::sampler::state::requires_source(d.state()))
            })
            .count();
        if count == 0 {
            return Err(ResourcePreparationError::InvalidScope);
        }
        // Reject a known-insufficient metadata reservation before allocating
        // configuration backing or copying current-format state keys.
        let mut minimum =
            quoted_bytes::<Configuration>(count, std::mem::size_of::<SnapshotData>())?;
        for device in project
            .tracks()
            .iter()
            .filter(|track| track.receives_source_audio())
            .filter_map(|t| t.instrument())
            .filter(|d| hex_devices::sampler::state::requires_source(d.state()))
        {
            let state = device.state();
            minimum = minimum
                .checked_add(state.payload().len())
                .and_then(|n| {
                    n.checked_add(
                        state
                            .asset_references()
                            .len()
                            .checked_mul(std::mem::size_of::<DeviceAssetReference>())?,
                    )
                })
                .and_then(|n| {
                    n.checked_add(
                        state
                            .asset_references()
                            .len()
                            .checked_mul(std::mem::size_of::<ScopeBinding>())?,
                    )
                })
                .ok_or(AudioStorageError::Overflow)?;
        }
        if minimum > metadata_bytes {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let mut configurations = Vec::new();
        configurations
            .try_reserve_exact(count)
            .map_err(|_| AudioStorageError::Allocation)?;
        if quoted_bytes::<Configuration>(
            configurations.capacity(),
            std::mem::size_of::<SnapshotData>(),
        )? > metadata_bytes
        {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let mut scan = max_scan_frames;
        for track in project
            .tracks()
            .iter()
            .filter(|track| track.receives_source_audio())
        {
            let Some(device) = track
                .instrument()
                .filter(|d| hex_devices::sampler::state::requires_source(d.state()))
            else {
                continue;
            };
            let state = device.state();
            let references = state.asset_references();
            if references.len() != 1 {
                return Err(ResourcePreparationError::InvalidScope);
            }
            let asset = project
                .assets()
                .find(|a| a.id().value() == references[0].asset.value())
                .map(|a| a.id())
                .ok_or(ResourcePreparationError::InvalidSource)?;
            let source = source_for(asset).ok_or(ResourcePreparationError::InvalidSource)?;
            let frames = source
                .bank
                .source_view(&source.bank.source_token(asset)?)?
                .resident_frames()
                .ok_or(ResourcePreparationError::InvalidSource)?
                .len();
            scan = scan
                .checked_sub(u64::try_from(frames).map_err(|_| AudioStorageError::Overflow)?)
                .ok_or(ResourcePreparationError::WorkCapacity)?;
            let scope = source
                .catalog
                .prepare_scope_on_control(state, metadata_bytes)?;
            let prepared = hex_devices::sampler::state::prepare_runtime_on_control(
                state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: project.sample_rate(),
                    tuning: &tuning,
                    resources: &source.catalog.resources(&source.bank, &scope)?,
                },
                crate::mutable_instrument::MAX_MUTABLE_EVENTS_PER_QUERY,
            )
            .map_err(|_| ResourcePreparationError::InvalidScope)?;
            let mut payload = Vec::new();
            payload
                .try_reserve_exact(state.payload().len())
                .map_err(|_| AudioStorageError::Allocation)?;
            payload.extend_from_slice(state.payload());
            let mut copied_references = Vec::new();
            copied_references
                .try_reserve_exact(references.len())
                .map_err(|_| AudioStorageError::Allocation)?;
            copied_references.extend_from_slice(references);
            configurations.push(Configuration {
                track: track.id(),
                device: device.id(),
                state_version: state.state_version(),
                payload,
                references: copied_references,
                input: state.input_gain_db().to_bits(),
                output: state.output_gain_db().to_bits(),
                source: source.capture_on_control()?,
                scope,
                prepared,
            });
            cancellation(cancel)?;
        }
        let data = OwnedShared::new_owned(SnapshotData {
            id: crate::audio_only_inputs::handle()?,
            tuning,
            sample_rate: project.sample_rate(),
            configurations,
        });
        if data.parts().is_none() {
            return Err(AudioStorageError::UnsupportedConstructor.into());
        }
        Ok(Self {
            data,
            handle: crate::audio_only_inputs::handle()?,
        })
    }
    pub(crate) fn id(&self) -> u64 {
        self.data.id
    }
    pub(crate) fn validate_project(
        &self,
        project: &hex_project::Project,
    ) -> Result<(), crate::Invalid> {
        let mut count = 0usize;
        for track in project
            .tracks()
            .iter()
            .filter(|track| track.receives_source_audio())
        {
            let Some(device) = track
                .instrument()
                .filter(|d| hex_devices::sampler::state::requires_source(d.state()))
            else {
                continue;
            };
            if !self
                .data
                .configurations
                .iter()
                .any(|c| c.device == device.id() && c.track == track.id())
            {
                return Err(crate::Invalid(
                    "sampler track/device binding differs from its source scope",
                ));
            }
            count += 1;
        }
        if count != self.data.configurations.len() {
            return Err(crate::Invalid(
                "sampler source scope has a different configuration set",
            ));
        }
        Ok(())
    }
    pub(crate) fn prepare_instrument(
        &self,
        device: DeviceId,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Box<dyn hex_lattice::PreparedNativeInstrument>, crate::Invalid> {
        let c = self
            .data
            .configurations
            .iter()
            .find(|c| c.device == device)
            .ok_or(crate::Invalid("sampler configuration scope is unavailable"))?;
        if state.type_id() != hex_devices::sampler::state::TYPE_ID
            || state.state_version() != c.state_version
            || state.payload() != c.payload
            || state.asset_references() != c.references
            || state.input_gain_db().to_bits() != c.input
            || state.output_gain_db().to_bits() != c.output
            || context.sample_rate != self.data.sample_rate
            || *context.tuning != self.data.tuning
        {
            return Err(crate::Invalid(
                "sampler configuration differs from its exact source scope",
            ));
        }
        Ok(Box::new(
            hex_devices::sampler::catalog::PreparedSamplerInstrument::from_runtime_on_control(
                c.prepared.clone(),
            ),
        ))
    }
    pub(crate) fn has_track(&self, track: TrackId) -> bool {
        self.data.configurations.iter().any(|c| c.track == track)
    }
    pub(crate) fn resources(
        &self,
        track: TrackId,
    ) -> Result<ScopedDeviceResources<'_>, ResourcePreparationError> {
        let c = self
            .data
            .configurations
            .iter()
            .find(|c| c.track == track)
            .ok_or(ResourcePreparationError::InvalidScope)?;
        c.source.catalog.resources(&c.source.bank, &c.scope)
    }
    pub(crate) fn visit<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<u64, AudioStorageError> {
        if !ledger.native_payload_once(self)? {
            return Ok(0);
        }
        ledger.bytes(
            crate::audio_storage::Owner::exclusive(self),
            crate::audio_storage::Location("control-source-owner", 0),
            std::mem::size_of::<Self>(),
        )?;
        ledger.shared(
            self.data.arc(),
            self.data.parts(),
            crate::audio_storage::Owner::exclusive(self),
            self.handle,
            None,
        )?;
        let mut frames = 0_u64;
        if ledger.native_payload_once(self.data.as_ref())? {
            ledger.vec(&self.data.configurations)?;
            for c in &self.data.configurations {
                frames = frames
                    .checked_add(c.source.visit(ledger)?)
                    .ok_or(AudioStorageError::Overflow)?;
                ledger.vec(&c.scope.bindings)?;
                ledger.vec(&c.payload)?;
                ledger.vec(&c.references)?;
            }
        }
        Ok(frames)
    }
    pub fn compiler_factory_allowance(&self) -> Result<usize, ResourcePreparationError> {
        // A future generic factory also owns its exact-target scalar proof;
        // the closed constructor may conservatively omit that capability.
        let per_factory = std::mem::size_of::<hex_devices::sampler::PreparedSamplerRuntime>()
            .checked_add(
                hex_devices::sampler::PreparedSamplerRuntime::history_retarget_inline_bound(),
            )
            .ok_or(AudioStorageError::Overflow)?;
        self.data
            .configurations
            .len()
            .checked_mul(per_factory)
            .ok_or(AudioStorageError::Overflow.into())
    }
    /// A sealed outer handle, not a source owner projection. The complete union
    /// must be checked before publishing this private control-side capture.
    pub fn capture_on_control(&self) -> Result<Self, ResourcePreparationError> {
        Ok(Self {
            data: self.data.clone_on_control(),
            handle: crate::audio_only_inputs::handle()?,
        })
    }
}
/// Private compiler adapter shared by the sealed legacy sidecar and ordinary resource sets.
/// The ordinary implementation always resolves the definition through its registry.
pub(crate) trait InstrumentResourcePreparation {
    fn private_scope(&self) -> Option<u64> {
        None
    }
    fn device_scope(&self) -> Option<u64> {
        None
    }
    fn prepare(
        &self,
        registry: &hex_lattice::NativeInstrumentRegistry<'_>,
        device: DeviceId,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Box<dyn hex_lattice::PreparedNativeInstrument>, crate::Invalid>;
}
impl InstrumentResourcePreparation for ControlSamplerSources {
    fn private_scope(&self) -> Option<u64> {
        Some(self.id())
    }
    fn prepare(
        &self,
        registry: &hex_lattice::NativeInstrumentRegistry<'_>,
        device: DeviceId,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Box<dyn hex_lattice::PreparedNativeInstrument>, crate::Invalid> {
        // This obsolete private constructor retains its closed sampler class.
        if hex_devices::sampler::state::requires_source(state) {
            self.prepare_instrument(device, state, context)
        } else {
            registry
                .prepare_instrument_with_runtime_limits(
                    state,
                    context,
                    crate::mutable_instrument::runtime_limits(context.sample_rate)?,
                )
                .map_err(|_| crate::Invalid("track instrument state is unsupported"))
        }
    }
}
pub(crate) fn prepare_instrument(
    registry: &hex_lattice::NativeInstrumentRegistry<'_>,
    device: DeviceId,
    state: &NativeDeviceState,
    context: &hex_lattice::DevicePreparationContext<'_>,
    resources: Option<&dyn InstrumentResourcePreparation>,
) -> Result<Box<dyn hex_lattice::PreparedNativeInstrument>, crate::Invalid> {
    match resources {
        Some(resources) => resources.prepare(registry, device, state, context),
        None => registry
            .prepare_instrument_with_runtime_limits(
                state,
                context,
                crate::mutable_instrument::runtime_limits(context.sample_rate)?,
            )
            .map_err(|_| crate::Invalid("track instrument state is unsupported")),
    }
}

#[cfg(test)]
mod tests;

/// Nonowning obligation for exactly one actual captured source wrapper moved
/// into one private export job. No Clone/Copy, public fields or numeric ctor.
/// The live control anchor supplies referent provenance; this conservatively
/// reserves the outbound field until proven post-drop terminal/join.
pub struct DispatchedSourceObligation {
    snapshot: u64,
    job: u64,
    handle: u64,
}
impl ControlSamplerSources {
    pub fn capture_for_dispatch_on_control(
        &self,
        job: u64,
    ) -> Result<(Self, DispatchedSourceObligation), ResourcePreparationError> {
        if job == 0 {
            return Err(ResourcePreparationError::InvalidScope);
        }
        let owner = self.capture_on_control()?;
        let obligation = DispatchedSourceObligation {
            snapshot: self.data.id,
            job,
            handle: owner.handle,
        };
        Ok((owner, obligation))
    }
    pub(crate) fn visit_dispatched<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
        obligation: &'a DispatchedSourceObligation,
        job: u64,
    ) -> Result<u64, AudioStorageError> {
        if job != obligation.job || self.data.id != obligation.snapshot {
            return Err(AudioStorageError::SourceMismatch);
        }
        let frames = self.visit(ledger)?;
        // The retained token's container is charged separately by the private
        // lane's compile-time fixed-storage allowance. This is ONLY the missing
        // outbound owner wrapper, not another traversal of its shared contents.
        ledger.bytes(
            crate::audio_storage::Owner::exclusive(obligation),
            crate::audio_storage::Location("dispatched-source-owner-storage", 0),
            std::mem::size_of::<Self>(),
        )?;
        ledger.handle_at(
            crate::audio_storage::Owner::shared(self.data.arc()),
            crate::audio_storage::Owner::exclusive(obligation),
            crate::audio_storage::Location(
                "dispatched-source-owner",
                usize::try_from(job).map_err(|_| AudioStorageError::Overflow)?,
            ),
            obligation.handle,
        )?;
        Ok(frames)
    }
}
