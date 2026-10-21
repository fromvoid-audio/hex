//! Exact original-source configurations for ordinary registered instruments.
//!
//! Providers receive only borrowed inventories. Runtime leases are created on control and move
//! with their runtime, including into an outgoing tail; configuration equality never rebinds a
//! carried voice. The revision carries a nonowning set identity, not a source owner.
use super::{resident::NativeBankLoan, *};
use crate::{AudioRootStorageLedger, owned_shared::OwnedShared};
use hex_lattice::{DevicePreparationContext, NativeInstrumentRegistry, PreparedNativeInstrument};
use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
use hex_project::{DeviceId, Project, TrackId};
use std::cell::Cell;

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
            project.tracks().iter().filter_map(|track| {
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
        for (_, device) in devices() {
            if !registry
                .definition(device.state().type_id())
                .is_some_and(|definition| definition.supports_asset_resources())
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
        };
        result
            .configurations
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
                }));
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
            )?,
            |bytes, configuration| {
                bytes
                    .checked_add(configuration.metadata_bytes()?)
                    .ok_or(AudioStorageError::Overflow.into())
            },
        )
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
        for track in project.tracks() {
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
    ) -> Result<(), AudioStorageError> {
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
        if ledger.native_payload_once(self.configuration.as_ref())? {
            ledger.vec(&self.configuration.payload)?;
            ledger.vec(&self.configuration.bindings)?;
            for binding in &self.configuration.bindings {
                binding.source.visit(ledger)?;
            }
        }
        Ok(())
    }
}

/// Output-owned immutable limits and exact outstanding resource obligations.
/// This inventories only the resource subtree. Ordinary clip PCM remains a separately charged
/// conservative capacity allowance, without native constructor provenance; runtime/context/DSP
/// buffers retain their existing budgets. No whole-graph DATA or retirement proof is claimed.
/// Allocation, capture, source destruction and ledger traversal are control operations only.
pub struct DeviceResourcePublication {
    reservation: crate::AudioStorageLedgerReservation,
    obligations: Vec<DeviceResourceLeaseObligation>,
    max_obligations: usize,
}
/// Unpublished resource obligations, captured from the actual candidate on control. A refusal
/// destroys these anchors on control while returning the unchanged candidate owner to its caller.
struct DeviceResourcePublicationBatch {
    obligations: Vec<DeviceResourceLeaseObligation>,
}
impl DeviceResourcePublication {
    pub fn reserve_on_control(
        reservation: crate::AudioStorageLedgerReservation,
        max_obligations: usize,
    ) -> Result<Self, ResourcePreparationError> {
        let max_obligations = max_obligations.min(reservation.handle_rows / 2);
        let mut result = Self {
            reservation,
            obligations: Vec::new(),
            max_obligations,
        };
        result
            .obligations
            .try_reserve_exact(max_obligations)
            .map_err(|_| AudioStorageError::Allocation)?;
        result.inspect(
            &DeviceResourcePublicationBatch {
                obligations: Vec::new(),
            },
            0,
        )?;
        Ok(result)
    }
    pub fn outstanding_leases(&self) -> usize {
        self.obligations.len()
    }
    fn prepare_on_control(
        &self,
        playback: &crate::playback::Playback,
        legacy_pcm_frames: u64,
    ) -> Result<DeviceResourcePublicationBatch, ResourcePreparationError> {
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
        let mut batch = DeviceResourcePublicationBatch {
            obligations: Vec::new(),
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
        self.inspect(&batch, legacy_pcm_frames)?;
        Ok(batch)
    }
    fn inspect(
        &self,
        batch: &DeviceResourcePublicationBatch,
        legacy_pcm_frames: u64,
    ) -> Result<crate::AudioRootStorageSummary, ResourcePreparationError> {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(self.reservation)?;
        ledger.mutate(|ledger| {
            ledger.root(self, crate::AudioStorageRole::Current)?;
            ledger.vec(&self.obligations)?;
            ledger.root(batch, crate::AudioStorageRole::FailedConstruction)?;
            ledger.vec(&batch.obligations)?;
            for obligation in self.obligations.iter().chain(&batch.obligations) {
                obligation.visit(ledger)?;
            }
            let legacy_bytes = usize::try_from(legacy_pcm_frames)
                .ok()
                .and_then(|frames| frames.checked_mul(std::mem::size_of::<crate::Frame>()))
                .ok_or(AudioStorageError::Overflow)?;
            ledger.bytes(
                crate::audio_storage::Owner::exclusive(self),
                crate::audio_storage::Location("ordinary-clip-pcm-capacity-allowance", 0),
                legacy_bytes,
            )?;
            Ok(())
        })?;
        Ok(ledger.summary()?)
    }
    /// Reserve actual candidate obligations, then call the output's queue operation. Only a
    /// successful move commits them; refusal returns its actual candidate through the error.
    pub fn publish_on_control<T, E>(
        &mut self,
        playback: crate::playback::Playback,
        legacy_pcm_frames: u64,
        publish: impl FnOnce(crate::playback::Playback) -> Result<T, E>,
    ) -> Result<T, DeviceResourcePublishError<E>> {
        let mut batch = match self.prepare_on_control(&playback, legacy_pcm_frames) {
            Ok(batch) => batch,
            Err(reason) => return Err(DeviceResourcePublishError::Resources { reason, playback }),
        };
        let result = publish(playback).map_err(DeviceResourcePublishError::Publication)?;
        self.obligations.append(&mut batch.obligations);
        Ok(result)
    }

    /// Inspect and destroy an actual returned graph on control before closing its exact leases.
    /// Migrated leases absent from that graph retain their original source obligations.
    pub fn retire_on_control(&mut self, playback: Box<crate::playback::Playback>) {
        self.mark_returned_on_control(&playback);
        drop(playback);
        self.release_marked_after_drop_on_control();
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
