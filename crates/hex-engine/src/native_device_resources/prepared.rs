//! Exact original-source configurations for ordinary registered instruments.
//!
//! Providers receive only borrowed inventories. Runtime leases are created on control and move
//! with their runtime, including into an outgoing tail; configuration equality never rebinds a
//! carried voice. The revision carries a nonowning set identity, not a source owner.
//! A retained source contributes one configuration at its explicit audio receiver;
//! contributor tracks borrow its provider and never create a second resource lease.
use super::{resident::NativeBankLoan, *};
use crate::{AudioRootStorageLedger, owned_shared::OwnedShared};
use hex_lattice::{DevicePreparationContext, NativeInstrumentRegistry, PreparedNativeInstrument};
use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
use hex_project::{DeviceId, Project, TrackId};
use std::cell::Cell;
mod pcm;
use pcm::{PcmPublication, PcmPublications};

struct Binding {
    slot: DeviceAssetSlot,
    source: NativeBankLoan,
    source_index: usize,
}
struct Configuration {
    track: TrackId,
    device: DeviceId,
    type_id: DeviceTypeId,
    version: u32,
    input: i32,
    output: i32,
    payload: Vec<u8>,
    bindings: Vec<Binding>,
    factory_inline_bound: usize,
}
impl Configuration {
    fn matches(&self, state: &NativeDeviceState) -> bool {
        self.type_id == state.type_id()
            && self.version == state.state_version()
            && self.input == state.input_gain_millidb()
            && self.output == state.output_gain_millidb()
            && self.payload == state.payload()
            && self.bindings.len() == state.asset_references().len()
            && self
                .bindings
                .iter()
                .zip(state.asset_references())
                .all(|(binding, reference)| {
                    binding.slot == reference.slot
                        && binding.source.catalog.sources[binding.source_index]
                            .identity
                            .asset
                            == reference.asset
                })
    }
    fn visit_backing<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<u64, AudioStorageError> {
        if !ledger.native_payload_once(self)? {
            return Ok(0);
        }
        ledger.vec(&self.payload)?;
        ledger.vec(&self.bindings)?;
        self.bindings.iter().try_fold(0u64, |frames, binding| {
            frames
                .checked_add(binding.source.visit(ledger)?)
                .ok_or(AudioStorageError::Overflow)
        })
    }
    fn scan_frames(&self) -> Result<u64, ResourcePreparationError> {
        self.bindings.iter().try_fold(0u64, |frames, binding| {
            frames
                .checked_add(
                    binding.source.catalog.sources[binding.source_index]
                        .expected
                        .frames,
                )
                .ok_or(AudioStorageError::Overflow.into())
        })
    }
    fn metadata_bytes(&self) -> Result<usize, ResourcePreparationError> {
        let (payload, overhead) = OwnedShared::<Self>::declared_parts()
            .ok_or(AudioStorageError::UnsupportedConstructor)?;
        quoted_bytes::<Binding>(self.bindings.capacity(), payload)?
            .checked_add(self.payload.capacity())
            .and_then(|n| n.checked_add(overhead))
            .ok_or(AudioStorageError::Overflow.into())
    }
}
impl DeviceResources for Configuration {
    fn len(&self) -> usize {
        self.bindings.len()
    }
    fn get(&self, index: usize) -> Option<DeviceResourceView<'_>> {
        let binding = self.bindings.get(index)?;
        let fact = binding.source.catalog.sources.get(binding.source_index)?;
        let source = binding.source.bank.source_view(&fact.token).ok()?;
        let pcm = source.resident_frames()?;
        if source.sample_rate() != fact.expected.sample_rate
            || u64::try_from(pcm.len()).ok()? != fact.expected.frames
        {
            return None;
        }
        Some(DeviceResourceView {
            slot: binding.slot,
            identity: fact.identity,
            sample_rate: fact.expected.sample_rate,
            pcm: DeviceResourcePcm::Stereo(pcm),
        })
    }
}

/// Opaque control-side set built only from actual retained or staged original PCM.
/// Empty sets are valid. No sampler type dispatch, decoded-PCM substitute, owning clone or raw
/// bank projection is exposed. Every current/staged/captured set and runtime lease must be charged
/// together before publication; constructing this set alone grants no publication authority.
pub struct PreparedDeviceResources {
    id: u64,
    sample_rate: u32,
    tuning: hex_lattice::TuningTable,
    configurations: Vec<OwnedShared<Configuration>>,
    configuration_handles: Vec<u64>,
}
impl PreparedDeviceResources {
    pub fn prepare_on_control(
        store: &ResidentDeviceAssets,
        project: &Project,
        metadata_bytes: usize,
        cancel: &AtomicBool,
    ) -> Result<Self, ResourcePreparationError> {
        Self::prepare_with_sources(project, metadata_bytes, cancel, |asset| {
            store
                .assets
                .iter()
                .find(|(id, _)| *id == asset)
                .map(|(_, source)| source)
        })
    }
    pub fn prepare_staged_on_control(
        stage: &StagedNativeAssetView<'_>,
        project: &Project,
        metadata_bytes: usize,
        cancel: &AtomicBool,
    ) -> Result<Self, ResourcePreparationError> {
        Self::prepare_with_sources(project, metadata_bytes, cancel, |asset| {
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
        })
    }
    fn prepare_with_sources<'a>(
        project: &Project,
        metadata_bytes: usize,
        cancel: &AtomicBool,
        source_for: impl Fn(AssetId) -> Option<&'a NativeBankLoan>,
    ) -> Result<Self, ResourcePreparationError> {
        cancellation(cancel)?;
        let registry = hex_devices::native_instrument_registry()
            .map_err(|_| ResourcePreparationError::InvalidScope)?;
        let devices = || {
            project
                .tracks()
                .iter()
                .filter(|track| track.receives_source_audio())
                .filter_map(|track| {
                    track
                        .instrument()
                        .filter(|device| !device.state().asset_references().is_empty())
                        .map(|device| (track.id(), device))
                })
        };
        let count = devices().count();
        let (configuration_bytes, overhead) = OwnedShared::<Configuration>::declared_parts()
            .ok_or(AudioStorageError::UnsupportedConstructor)?;
        let mut minimum =
            quoted_bytes::<OwnedShared<Configuration>>(count, std::mem::size_of::<Self>())?;
        minimum = minimum
            .checked_add(
                count
                    .checked_mul(std::mem::size_of::<u64>())
                    .ok_or(AudioStorageError::Overflow)?,
            )
            .ok_or(AudioStorageError::Overflow)?;
        for (_, device) in devices() {
            if !registry
                .definition(device.state().type_id())
                .is_some_and(|definition| {
                    definition.supports_asset_resources()
                        && definition
                            .resource_factory_inline_bound()
                            .is_some_and(|bytes| bytes != 0)
                })
            {
                return Err(ResourcePreparationError::InvalidScope);
            }
            minimum = minimum
                .checked_add(configuration_bytes)
                .and_then(|n| n.checked_add(overhead))
                .and_then(|n| n.checked_add(device.state().payload().len()))
                .and_then(|n| {
                    n.checked_add(
                        device
                            .state()
                            .asset_references()
                            .len()
                            .checked_mul(std::mem::size_of::<Binding>())?,
                    )
                })
                .ok_or(AudioStorageError::Overflow)?;
        }
        if minimum > metadata_bytes {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let mut result = Self {
            id: crate::audio_only_inputs::handle()?,
            sample_rate: project.sample_rate(),
            tuning: project
                .tuning()
                .table()
                .map_err(|_| ResourcePreparationError::InvalidScope)?,
            configurations: Vec::new(),
            configuration_handles: Vec::new(),
        };
        result
            .configurations
            .try_reserve_exact(count)
            .map_err(|_| AudioStorageError::Allocation)?;
        result
            .configuration_handles
            .try_reserve_exact(count)
            .map_err(|_| AudioStorageError::Allocation)?;
        for (track, device) in devices() {
            cancellation(cancel)?;
            let state = device.state();
            let mut payload = Vec::new();
            payload
                .try_reserve_exact(state.payload().len())
                .map_err(|_| AudioStorageError::Allocation)?;
            payload.extend_from_slice(state.payload());
            let mut bindings = Vec::new();
            bindings
                .try_reserve_exact(state.asset_references().len())
                .map_err(|_| AudioStorageError::Allocation)?;
            for reference in state.asset_references() {
                cancellation(cancel)?;
                let asset = project
                    .assets()
                    .find(|asset| asset.id().value() == reference.asset.value())
                    .ok_or(ResourcePreparationError::InvalidSource)?
                    .id();
                let source = source_for(asset).ok_or(ResourcePreparationError::InvalidSource)?;
                let source_index = source
                    .catalog
                    .source_index(asset.value())
                    .ok_or(ResourcePreparationError::InvalidSource)?;
                source
                    .bank
                    .source_view(&source.catalog.sources[source_index].token)?;
                bindings.push(Binding {
                    slot: reference.slot,
                    source: source.capture_on_control()?,
                    source_index,
                });
            }
            let factory_inline_bound = registry
                .definition(state.type_id())
                .and_then(|definition| definition.resource_factory_inline_bound())
                .filter(|bytes| *bytes != 0)
                .ok_or(AudioStorageError::UnknownStorage)?;
            let handle = crate::audio_only_inputs::handle()?;
            result
                .configurations
                .push(OwnedShared::new_owned(Configuration {
                    track,
                    device: device.id(),
                    type_id: state.type_id(),
                    version: state.state_version(),
                    input: state.input_gain_millidb(),
                    output: state.output_gain_millidb(),
                    payload,
                    bindings,
                    factory_inline_bound,
                }));
            result.configuration_handles.push(handle);
            if result.metadata_bytes()? > metadata_bytes {
                return Err(ResourcePreparationError::MetadataCapacity);
            }
        }
        cancellation(cancel)?;
        Ok(result)
    }
    pub fn metadata_bytes(&self) -> Result<usize, ResourcePreparationError> {
        self.configurations.iter().try_fold(
            quoted_bytes::<OwnedShared<Configuration>>(
                self.configurations.capacity(),
                std::mem::size_of::<Self>(),
            )?
            .checked_add(
                self.configuration_handles
                    .capacity()
                    .checked_mul(std::mem::size_of::<u64>())
                    .ok_or(AudioStorageError::Overflow)?,
            )
            .ok_or(AudioStorageError::Overflow)?,
            |bytes, configuration| {
                bytes
                    .checked_add(configuration.metadata_bytes()?)
                    .ok_or(AudioStorageError::Overflow.into())
            },
        )
    }
    pub(super) fn visit<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<u64, AudioStorageError> {
        ledger.root(self, crate::AudioStorageRole::Current)?;
        ledger.vec(&self.configurations)?;
        ledger.vec(&self.configuration_handles)?;
        let mut frames = 0u64;
        for (configuration, handle) in self.configurations.iter().zip(&self.configuration_handles) {
            ledger.shared(
                configuration.arc(),
                configuration.parts(),
                crate::audio_storage::Owner::exclusive(self),
                *handle,
                None,
            )?;
            frames = frames
                .checked_add(configuration.visit_backing(ledger)?)
                .ok_or(AudioStorageError::Overflow)?;
        }
        Ok(frames)
    }
    /// Capture the same immutable configuration set for an existing worker job. The scope identity
    /// stays exact; every new physical configuration handle is minted and inventoried separately.
    /// No PCM copy or raw owning Arc projection is exposed. All live captures must be selected in
    /// the caller's control admission union until their actual worker-side destruction completes.
    pub fn capture_on_control(
        &self,
        metadata_bytes: usize,
    ) -> Result<Self, ResourcePreparationError> {
        if self.metadata_bytes()? > metadata_bytes {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let mut captured = Self {
            id: self.id,
            sample_rate: self.sample_rate,
            tuning: self.tuning,
            configurations: Vec::new(),
            configuration_handles: Vec::new(),
        };
        captured
            .configurations
            .try_reserve_exact(self.configurations.len())
            .map_err(|_| AudioStorageError::Allocation)?;
        captured
            .configuration_handles
            .try_reserve_exact(self.configurations.len())
            .map_err(|_| AudioStorageError::Allocation)?;
        for configuration in &self.configurations {
            let handle = crate::audio_only_inputs::handle()?;
            captured
                .configurations
                .push(configuration.clone_on_control());
            captured.configuration_handles.push(handle);
        }
        if captured.metadata_bytes()? > metadata_bytes {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        Ok(captured)
    }
    pub(crate) fn factory_inline_bound(&self, track: TrackId) -> Option<usize> {
        self.configurations
            .iter()
            .find(|configuration| configuration.track == track)
            .map(|configuration| configuration.factory_inline_bound)
    }
    /// Finite compiler scratch overlap from each resource definition's inline-only factory bound.
    /// Registry preparation, its mutable-factory copy and the audited sampler
    /// proof may coexist. Retained owners are selected separately from the
    /// actual compiled revision in the same control union.
    pub fn compiler_construction_allowance_bytes(&self) -> Result<usize, ResourcePreparationError> {
        self.configurations
            .iter()
            .try_fold(0usize, |bytes, configuration| {
                bytes
                    .checked_add(
                        configuration
                            .factory_inline_bound
                            .checked_mul(2)
                            .and_then(|bytes| bytes.checked_add(
                                if configuration.type_id == hex_devices::sampler::state::TYPE_ID {
                                    hex_devices::sampler::PreparedSamplerRuntime::history_retarget_inline_bound()
                                } else { 0 }
                            ))
                            .ok_or(AudioStorageError::Overflow)?,
                    )
                    .ok_or(AudioStorageError::Overflow.into())
            })
    }
    /// Quote aggregate PCM-frame visits for the actual bounded compiler factory sites. This is
    /// work, not source extent/storage: enabled instruments currently prepare three times and
    /// bypassed instruments twice, with two source scans per call. No PCM scan occurs here.
    pub fn compiler_scan_work_on_control(
        &self,
        project: &Project,
    ) -> Result<u64, ResourcePreparationError> {
        self.validate_project(project)
            .map_err(|_| ResourcePreparationError::InvalidScope)?;
        self.configurations
            .iter()
            .try_fold(0u64, |frames, configuration| {
                let device = project
                    .track(configuration.track)
                    .and_then(|track| track.instrument())
                    .ok_or(ResourcePreparationError::InvalidScope)?;
                let calls = if device.bypassed() { 2 } else { 3 };
                frames
                    .checked_add(
                        configuration
                            .scan_frames()?
                            .checked_mul(2 * calls)
                            .ok_or(AudioStorageError::Overflow)?,
                    )
                    .ok_or(AudioStorageError::Overflow.into())
            })
    }
    pub(crate) fn id(&self) -> u64 {
        self.id
    }
    pub(crate) fn validate_project(&self, project: &Project) -> Result<(), crate::Invalid> {
        if project.sample_rate() != self.sample_rate
            || project.tuning().table().ok().as_ref() != Some(&self.tuning)
        {
            return Err(crate::Invalid(
                "device resource preparation context differs",
            ));
        }
        let mut count = 0usize;
        for track in project
            .tracks()
            .iter()
            .filter(|track| track.receives_source_audio())
        {
            let Some(device) = track
                .instrument()
                .filter(|device| !device.state().asset_references().is_empty())
            else {
                continue;
            };
            if !self.configurations.iter().any(|configuration| {
                configuration.track == track.id()
                    && configuration.device == device.id()
                    && configuration.matches(device.state())
            }) {
                return Err(crate::Invalid(
                    "device configuration differs from its exact resource set",
                ));
            }
            count += 1;
        }
        if count != self.configurations.len() {
            return Err(crate::Invalid("device resource configuration set differs"));
        }
        Ok(())
    }
    pub(crate) fn lease_on_control(
        &self,
        track: TrackId,
    ) -> Result<Option<PreparedDeviceResourceLease>, ResourcePreparationError> {
        self.configurations
            .iter()
            .find(|configuration| configuration.track == track)
            .map(|configuration| {
                Ok(PreparedDeviceResourceLease {
                    configuration: configuration.clone_on_control(),
                    id: crate::audio_only_inputs::handle()?,
                })
            })
            .transpose()
    }
}

/// One actual runtime's original-source owner. No Clone or public constructor is available.
/// Moving the runtime moves this lease unchanged; its destruction belongs on control.
pub struct PreparedDeviceResourceLease {
    configuration: OwnedShared<Configuration>,
    id: u64,
}
impl PreparedDeviceResourceLease {
    fn visit<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<u64, AudioStorageError> {
        ledger.shared(
            self.configuration.arc(),
            self.configuration.parts(),
            crate::audio_storage::Owner::exclusive(self),
            self.id,
            None,
        )?;
        self.configuration.visit_backing(ledger)
    }
    pub(crate) fn resources(&self) -> &dyn DeviceResources {
        self.configuration.as_ref()
    }
    pub(crate) fn capture_on_control(&self) -> Result<Self, ResourcePreparationError> {
        Ok(Self {
            configuration: self.configuration.clone_on_control(),
            id: crate::audio_only_inputs::handle()?,
        })
    }
    fn capture_obligation_on_control(
        &self,
    ) -> Result<DeviceResourceLeaseObligation, ResourcePreparationError> {
        Ok(DeviceResourceLeaseObligation {
            configuration: self.configuration.clone_on_control(),
            lease: self.id,
            anchor: crate::audio_only_inputs::handle()?,
            returning: false,
        })
    }
    fn matches_obligation(&self, obligation: &DeviceResourceLeaseObligation) -> bool {
        self.id == obligation.lease
            && std::ptr::eq(
                self.configuration.as_ref(),
                obligation.configuration.as_ref(),
            )
    }
    pub(crate) fn compatible(&self, other: &Self) -> bool {
        self.configuration.bindings.len() == other.configuration.bindings.len()
            && self
                .configuration
                .bindings
                .iter()
                .zip(&other.configuration.bindings)
                .all(|(a, b)| {
                    a.slot == b.slot
                        && a.source_index == b.source_index
                        && std::ptr::eq(a.source.owner.as_ref(), b.source.owner.as_ref())
                })
    }
}

/// One synchronous compiler transaction. Every registry invocation consumes its actual two finite
/// PCM scans before either scan starts. Cancellation is checked at the inventory query separating
/// registry validation from provider binding, and again after provider completion.
pub(crate) struct CompilerDeviceResources<'a> {
    pub(crate) resources: &'a PreparedDeviceResources,
    remaining_scan_frames: Cell<u64>,
    cancel: &'a AtomicBool,
}
impl<'a> CompilerDeviceResources<'a> {
    pub(crate) fn new(
        resources: &'a PreparedDeviceResources,
        max_scan_frames: u64,
        cancel: &'a AtomicBool,
    ) -> Self {
        Self {
            resources,
            remaining_scan_frames: Cell::new(max_scan_frames),
            cancel,
        }
    }
    pub(crate) fn prepare_instrument(
        &self,
        registry: &NativeInstrumentRegistry<'_>,
        device: DeviceId,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, crate::Invalid> {
        let configuration = self
            .resources
            .configurations
            .iter()
            .find(|configuration| configuration.device == device);
        if state.asset_references().is_empty() {
            if configuration.is_some() {
                return Err(crate::Invalid("unexpected device resource configuration"));
            }
            return registry
                .prepare_instrument_with_runtime_limits(
                    state,
                    context,
                    crate::mutable_instrument::runtime_limits(context.sample_rate)?,
                )
                .map_err(|_| crate::Invalid("track instrument state is unsupported"));
        }
        let configuration = configuration
            .filter(|configuration| configuration.matches(state))
            .ok_or(crate::Invalid(
                "device configuration differs from its exact resource set",
            ))?;
        let work = configuration
            .scan_frames()
            .ok()
            .and_then(|frames| frames.checked_mul(2))
            .and_then(|frames| self.remaining_scan_frames.get().checked_sub(frames))
            .ok_or(crate::Invalid(
                "device resource compiler scan budget exceeded",
            ))?;
        self.remaining_scan_frames.set(work);
        cancellation(self.cancel)
            .map_err(|_| crate::Invalid("device resource preparation cancelled"))?;
        let resources = CancellableResources {
            configuration,
            cancel: self.cancel,
        };
        let result = registry
            .prepare_instrument_with_runtime_limits(
                state,
                &DevicePreparationContext {
                    sample_rate: context.sample_rate,
                    tuning: context.tuning,
                    resources: &resources,
                },
                crate::mutable_instrument::runtime_limits(context.sample_rate)?,
            )
            .map_err(|_| crate::Invalid("track instrument state is unsupported"));
        cancellation(self.cancel)
            .map_err(|_| crate::Invalid("device resource preparation cancelled"))?;
        result
    }
}
struct CancellableResources<'a> {
    configuration: &'a Configuration,
    cancel: &'a AtomicBool,
}
impl DeviceResources for CancellableResources<'_> {
    fn len(&self) -> usize {
        self.configuration.len()
    }
    fn get(&self, index: usize) -> Option<DeviceResourceView<'_>> {
        if self.cancel.load(Ordering::Relaxed) {
            None
        } else {
            self.configuration.get(index)
        }
    }
}

impl super::InstrumentResourcePreparation for CompilerDeviceResources<'_> {
    fn device_scope(&self) -> Option<u64> {
        Some(self.resources.id())
    }
    fn prepare(
        &self,
        registry: &NativeInstrumentRegistry<'_>,
        device: DeviceId,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, crate::Invalid> {
        self.prepare_instrument(registry, device, state, context)
    }
}

#[cfg(test)]
mod tests;

/// Existing current-track, outgoing-tail and selected-keyboard capacities bound physical leases
/// in one ordinary Playback. No new voices or tail slots are admitted by this accounting bound.
pub const MAX_PLAYBACK_DEVICE_RESOURCE_LEASES: usize =
    hex_project::MAX_TRACKS + hex_lattice::MAX_STATEFUL_PROCESSORS + 1;

struct DeviceResourceLeaseObligation {
    configuration: OwnedShared<Configuration>,
    lease: u64,
    anchor: u64,
    returning: bool,
}
impl DeviceResourceLeaseObligation {
    fn visit<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<u64, AudioStorageError> {
        ledger.shared(
            self.configuration.arc(),
            self.configuration.parts(),
            crate::audio_storage::Owner::exclusive(self),
            self.anchor,
            None,
        )?;
        // This sealed row was captured from the actual runtime field before its move. It remains
        // outstanding across publications until that exact lease is physically returned.
        ledger.handle_at(
            crate::audio_storage::Owner::shared(self.configuration.arc()),
            crate::audio_storage::Owner::exclusive(self),
            crate::audio_storage::Location("outstanding-runtime-resource-lease", 0),
            self.lease,
        )?;
        self.configuration.visit_backing(ledger)
    }
}

pub(crate) fn inline_resource_factory_bytes(
    factory: &dyn hex_lattice::PreparedMutableInstrumentRuntime,
    bound: usize,
) -> Result<usize, AudioStorageError> {
    factory
        .resource_factory_inline_bytes()
        .filter(|bytes| *bytes != 0 && *bytes == std::mem::size_of_val(factory) && *bytes <= bound)
        .ok_or(AudioStorageError::UnknownStorage)
}

/// Borrowed actual registered-resource owners in one control admission phase.
/// Source banks deduplicate against Session stores and other selected owners in the same ledger.
/// This view grants neither publication authority nor a complete graph/retirement proof.
#[derive(Clone, Copy, Default)]
pub struct DeviceResourceAdmission<'a> {
    sets: &'a [&'a PreparedDeviceResources],
    revisions: &'a [&'a crate::PreparedRevision],
    set: Option<&'a PreparedDeviceResources>,
    revision: Option<&'a crate::PreparedRevision>,
    books: &'a [&'a DeviceResourcePublication],
    book: Option<&'a DeviceResourcePublication>,
    playback: Option<&'a crate::playback::Playback>,
    playback_resources: Option<&'a PreparedDeviceResources>,
    batch: Option<&'a DeviceResourcePublicationBatch>,
}
impl<'a> DeviceResourceAdmission<'a> {
    pub fn new(
        sets: &'a [&'a PreparedDeviceResources],
        books: &'a [&'a DeviceResourcePublication],
    ) -> Self {
        Self {
            sets,
            revisions: &[],
            set: None,
            revision: None,
            books,
            book: None,
            playback: None,
            playback_resources: None,
            batch: None,
        }
    }
    /// Borrow actual unpublished worker-owned runtime leases while an output is unavailable or
    /// a seek/preview result awaits publication/retirement. Cancellation does not release custody.
    pub fn unpublished_on_control(
        playback: &'a crate::playback::Playback,
        resources: Option<&'a PreparedDeviceResources>,
    ) -> Self {
        Self {
            playback: Some(playback),
            playback_resources: resources,
            ..Self::default()
        }
    }
    /// Select compiled resource-factory backing against one of this view's exact resource sets.
    /// Each actual resource set/revision must be selected once per phase, not repeated through
    /// both current/export aliases. Ordinary non-resource factory contracts remain unchanged.
    pub fn with_prepared_revisions(mut self, revisions: &'a [&'a crate::PreparedRevision]) -> Self {
        self.revisions = revisions;
        self
    }
    /// One actual captured set and its exact compiled revision, suitable for a bounded worker
    /// job inventory without allocating a separate array of borrowed selections.
    pub fn prepared_on_control(
        set: &'a PreparedDeviceResources,
        revision: &'a crate::PreparedRevision,
    ) -> Self {
        Self {
            set: Some(set),
            revision: Some(revision),
            ..Self::default()
        }
    }
    pub(super) fn legacy_sources(&self) -> impl Iterator<Item = &'a crate::Audio> + Clone {
        self.revisions
            .iter()
            .copied()
            .chain(self.revision)
            .flat_map(|revision| {
                revision
                    .arrangement()
                    .tracks()
                    .iter()
                    .flat_map(|t| t.clips.iter().filter_map(|c| c.audio.as_ref()))
                    .chain(
                        revision
                            .session()
                            .audio_clips()
                            .iter()
                            .filter_map(|c| c.resident_audio()),
                    )
            })
            .chain(
                self.books
                    .iter()
                    .copied()
                    .chain(self.book)
                    .flat_map(|book| book.pcm.sources()),
            )
            .chain(self.batch.into_iter().flat_map(|batch| batch.pcm.sources()))
            .chain(
                self.playback
                    .into_iter()
                    .flat_map(|playback| playback.resident_pcm_sources()),
            )
    }
    pub(super) fn visit(
        &self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<u64, AudioStorageError> {
        let mut frames = 0u64;
        for set in self.sets.iter().copied().chain(self.set) {
            frames = frames
                .checked_add(set.visit(ledger)?)
                .ok_or(AudioStorageError::Overflow)?;
        }
        for revision in self.revisions.iter().copied().chain(self.revision) {
            let set = self
                .sets
                .iter()
                .copied()
                .chain(self.set)
                .find(|set| Some(set.id()) == revision.device_resource_scope)
                .ok_or(AudioStorageError::SourceMismatch)?;
            revision
                .session()
                .visit_device_resource_factories(ledger, set)?;
            ledger.bytes(
                crate::audio_storage::Owner::exclusive(revision),
                crate::audio_storage::Location("ordinary-handoff-evidence-capacity", 0),
                revision.device_handoff_metadata_bytes()?,
            )?;
        }
        for book in self.books.iter().copied().chain(self.book) {
            ledger.root(book, crate::AudioStorageRole::Current)?;
            ledger.vec(&book.obligations)?;
            book.pcm.metadata(ledger)?;
            ledger.bytes(
                crate::audio_storage::Owner::exclusive(book),
                crate::audio_storage::Location("resource-graph-control-metadata-allowance", 0),
                book.graph_control_metadata_bytes,
            )?;
            for obligation in &book.obligations {
                frames = frames
                    .checked_add(obligation.visit(ledger)?)
                    .ok_or(AudioStorageError::Overflow)?;
            }
        }
        if let Some(playback) = self.playback {
            ledger.root(playback, crate::AudioStorageRole::Caller)?;
            if let Some(resources) = self.playback_resources {
                let revision = playback
                    .prepared_revision()
                    .ok_or(AudioStorageError::SourceMismatch)?;
                if revision.device_resource_scope != Some(resources.id()) {
                    return Err(AudioStorageError::SourceMismatch);
                }
                frames = frames
                    .checked_add(resources.visit(ledger)?)
                    .ok_or(AudioStorageError::Overflow)?;
                revision
                    .session()
                    .visit_device_resource_factories(ledger, resources)?;
                ledger.bytes(
                    crate::audio_storage::Owner::exclusive(revision),
                    crate::audio_storage::Location("ordinary-handoff-evidence-capacity", 0),
                    revision.device_handoff_metadata_bytes()?,
                )?;
            } else if playback
                .prepared_revision()
                .is_some_and(|revision| revision.device_resource_scope.is_some())
            {
                return Err(AudioStorageError::SourceMismatch);
            }
            for lease in playback.device_resource_leases() {
                frames = frames
                    .checked_add(lease.visit(ledger)?)
                    .ok_or(AudioStorageError::Overflow)?;
            }
        }
        if let Some(batch) = self.batch {
            ledger.root(batch, crate::AudioStorageRole::FailedConstruction)?;
            ledger.vec(&batch.obligations)?;
            batch.pcm.metadata(ledger)?;
            ledger.bytes(
                crate::audio_storage::Owner::exclusive(batch),
                crate::audio_storage::Location("resource-graph-control-metadata-allowance", 0),
                batch.graph_control_metadata_bytes,
            )?;
            for obligation in &batch.obligations {
                frames = frames
                    .checked_add(obligation.visit(ledger)?)
                    .ok_or(AudioStorageError::Overflow)?;
            }
        }
        Ok(frames)
    }
}

/// Output-owned immutable limits and exact outstanding resource obligations.
/// Ordinary PCM is retained by bounded control-owned records captured before enqueue.
/// Admission borrows their actual backing allocations, shared across publications and
/// selected originals. Records release only after the matching graph is destroyed
/// on control. This capacity inventory does not upgrade legacy Arc provenance. Validated fixed inline
/// resource factories and complete MIDI handoff evidence have a separate conservative
/// control-metadata allowance per accepted/unreturned graph; source-tail anchors never retain a
/// whole revision. Runtime/context/DSP buffers retain their existing budgets. No whole-graph DATA
/// or retirement proof is claimed.
/// Allocation, capture, source destruction and ledger traversal are control operations only.
pub struct DeviceResourcePublication {
    reservation: crate::AudioStorageLedgerReservation,
    obligations: Vec<DeviceResourceLeaseObligation>,
    max_obligations: usize,
    graph_control_metadata_bytes: usize,
    unreturned_graphs: usize,
    pcm: PcmPublications,
}
/// Unpublished resource obligations, captured from the actual candidate on control. A refusal
/// destroys these anchors on control while returning the unchanged candidate owner to its caller.
struct DeviceResourcePublicationBatch {
    pcm: PcmPublication,
    obligations: Vec<DeviceResourceLeaseObligation>,
    graph_control_metadata_bytes: usize,
}
impl DeviceResourcePublication {
    pub fn reserve_on_control(
        reservation: crate::AudioStorageLedgerReservation,
        max_obligations: usize,
        max_graphs: usize,
    ) -> Result<Self, ResourcePreparationError> {
        let max_obligations = max_obligations.min(reservation.handle_rows / 2);
        Self::preflight_metadata(reservation, max_obligations, 0, max_graphs, 0)?;
        let mut result = Self {
            reservation,
            obligations: Vec::new(),
            max_obligations,
            graph_control_metadata_bytes: 0,
            unreturned_graphs: 0,
            pcm: PcmPublications::reserve(max_graphs)?,
        };
        result
            .obligations
            .try_reserve_exact(max_obligations)
            .map_err(|_| AudioStorageError::Allocation)?;
        result.inspect(None)?;
        Ok(result)
    }
    /// Borrow outstanding resource leases and actual PCM records. This grants
    /// neither callback quiescence nor retirement authority.
    pub fn admission_on_control(&self) -> DeviceResourceAdmission<'_> {
        DeviceResourceAdmission {
            book: Some(self),
            ..DeviceResourceAdmission::default()
        }
    }
    /// Accepted ordinary graph owners not yet physically destroyed on control, including graphs
    /// with no device resources. Callers retain their existing bounded per-runtime buffer allowance
    /// for each owner; this counter does not attest whole-graph allocation identity.
    pub fn unreturned_graphs_on_control(&self) -> usize {
        self.unreturned_graphs
    }
    pub fn outstanding_leases(&self) -> usize {
        self.obligations.len()
    }
    fn prepare_on_control(
        &self,
        playback: &crate::playback::Playback,
    ) -> Result<DeviceResourcePublicationBatch, ResourcePreparationError> {
        if !self.pcm.available() {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        self.unreturned_graphs
            .checked_add(1)
            .ok_or(AudioStorageError::Overflow)?;
        let count = playback.device_resource_leases().count();
        if count > MAX_PLAYBACK_DEVICE_RESOURCE_LEASES
            || self
                .obligations
                .len()
                .checked_add(count)
                .is_none_or(|count| count > self.max_obligations)
        {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let graph_control_metadata_bytes = playback.resource_control_metadata_bytes_on_control()?;
        let total_control_metadata_bytes = self
            .graph_control_metadata_bytes
            .checked_add(graph_control_metadata_bytes)
            .ok_or(AudioStorageError::Overflow)?;
        Self::preflight_metadata(
            self.reservation,
            self.obligations.capacity(),
            count,
            0,
            total_control_metadata_bytes,
        )?;
        let mut batch = DeviceResourcePublicationBatch {
            pcm: PcmPublication::capture(playback)?,
            obligations: Vec::new(),
            graph_control_metadata_bytes,
        };
        batch
            .obligations
            .try_reserve_exact(count)
            .map_err(|_| AudioStorageError::Allocation)?;
        for lease in playback.device_resource_leases() {
            if self.obligations.iter().any(|old| old.lease == lease.id)
                || batch.obligations.iter().any(|old| old.lease == lease.id)
            {
                return Err(ResourcePreparationError::InvalidScope);
            }
            batch
                .obligations
                .push(lease.capture_obligation_on_control()?);
        }
        self.inspect(Some(&batch))?;
        Ok(batch)
    }
    fn preflight_metadata(
        reservation: crate::AudioStorageLedgerReservation,
        retained_capacity: usize,
        batch_count: usize,
        graph_capacity: usize,
        graph_control_metadata_bytes: usize,
    ) -> Result<(), ResourcePreparationError> {
        let minimum = std::mem::size_of::<Self>()
            .checked_add(std::mem::size_of::<DeviceResourcePublicationBatch>())
            .and_then(|bytes| {
                bytes.checked_add(
                    retained_capacity
                        .checked_add(batch_count)?
                        .checked_mul(std::mem::size_of::<DeviceResourceLeaseObligation>())?,
                )
            })
            .and_then(|bytes| {
                bytes.checked_add(AudioRootStorageLedger::reserved_metadata_minimum(
                    reservation,
                )?)
            })
            .and_then(|bytes| {
                bytes.checked_add(PcmPublications::reserved_bytes(graph_capacity).ok()?)
            })
            .and_then(|bytes| bytes.checked_add(graph_control_metadata_bytes))
            .ok_or(AudioStorageError::Overflow)?;
        if minimum > reservation.max_retained_bytes
            || minimum > reservation.max_construction_peak_bytes
        {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        Ok(())
    }
    fn inspect(
        &self,
        batch: Option<&DeviceResourcePublicationBatch>,
    ) -> Result<crate::AudioRootStorageSummary, ResourcePreparationError> {
        let registered = [DeviceResourceAdmission {
            batch,
            ..self.admission_on_control()
        }];
        let phase = super::DeviceResourcePhase {
            stores: &[],
            registered: &registered,
            selections: &[],
            outbound: &[],
            legacy: &[],
            checked_runtime_bytes: 0,
            construction_bytes: 0,
        };
        Ok(super::inspect_device_resource_union(&phase, &phase, self.reservation)?.retained)
    }
    /// Reserve actual candidate obligations, then call the output's queue operation. Only a
    /// successful move commits them; refusal returns its actual candidate through the error.
    pub fn publish_on_control<T, E>(
        &mut self,
        playback: crate::playback::Playback,
        publish: impl FnOnce(crate::playback::Playback) -> Result<T, E>,
    ) -> Result<T, DeviceResourcePublishError<E>> {
        self.publish_admitted_on_control(playback, |playback, _| publish(playback))
    }

    /// Prepare actual prospective anchors, then synchronously lend the current book and candidate
    /// batch to the caller's full control union before its final enqueue. No owner/proof escapes
    /// through the borrowed view. Refusal drops only unpublished anchors on control and returns
    /// the unchanged candidate through the caller's error; queue success commits the exact batch.
    pub fn publish_admitted_on_control<T, E>(
        &mut self,
        mut playback: crate::playback::Playback,
        publish: impl FnOnce(crate::playback::Playback, DeviceResourceAdmission<'_>) -> Result<T, E>,
    ) -> Result<T, DeviceResourcePublishError<E>> {
        let mut batch = match self.prepare_on_control(&playback) {
            Ok(batch) => batch,
            Err(reason) => return Err(DeviceResourcePublishError::Resources { reason, playback }),
        };
        let view = DeviceResourceAdmission {
            batch: Some(&batch),
            ..self.admission_on_control()
        };
        playback.resident_pcm_publication = Some(batch.pcm.id);
        let result = publish(playback, view).map_err(DeviceResourcePublishError::Publication)?;
        // The identical sum was checked before the queue move; no fallible commit remains.
        self.unreturned_graphs += 1;
        self.graph_control_metadata_bytes += batch.graph_control_metadata_bytes;
        self.obligations.append(&mut batch.obligations);
        self.pcm.commit(batch.pcm);
        Ok(result)
    }

    /// Inspect and destroy an actual returned graph on control before closing its exact leases.
    /// Migrated leases absent from that graph retain their original source obligations.
    pub fn retire_on_control(
        &mut self,
        playback: Box<crate::playback::Playback>,
    ) -> Result<(), Box<crate::playback::Playback>> {
        let Some(index) = self.pcm.find(playback.resident_pcm_publication) else {
            return Err(playback);
        };
        let Some(graphs) = self.unreturned_graphs.checked_sub(1) else {
            return Err(playback);
        };
        let Some(metadata) = playback
            .resource_control_metadata_bytes_on_control()
            .ok()
            .and_then(|bytes| self.graph_control_metadata_bytes.checked_sub(bytes))
        else {
            return Err(playback);
        };
        self.mark_returned_on_control(&playback);
        drop(playback);
        self.pcm.release(index);
        self.unreturned_graphs = graphs;
        self.graph_control_metadata_bytes = metadata;
        self.release_marked_after_drop_on_control();
        Ok(())
    }

    /// Mark only leases physically present in this returned Playback. An original publication's
    /// return does not close a lease which moved into a successor's outgoing runtime tail.
    fn mark_returned_on_control(&mut self, playback: &crate::playback::Playback) {
        for obligation in &mut self.obligations {
            obligation.returning = playback
                .device_resource_leases()
                .any(|lease| lease.matches_obligation(obligation));
        }
    }
    /// The caller must destroy the actual marked Playback before invoking this method. Anchors
    /// then close the exact obligations, possibly releasing their last original source on control.
    fn release_marked_after_drop_on_control(&mut self) {
        self.obligations.retain(|obligation| !obligation.returning);
    }
}

/// A resource refusal retains the caller's exact graph; a queue refusal retains its original error.
pub enum DeviceResourcePublishError<E> {
    Resources {
        reason: ResourcePreparationError,
        playback: crate::playback::Playback,
    },
    Publication(E),
}
