//! Control-verified resource facts and per-configuration nonowning scope maps.
//!
//! [`DeviceResourceCatalog`] holds checked facts/tokens; [`DeviceResourceScope`] maps authored
//! slots within one exact configuration.
//! The actual immutable input bank remains in retained host custody. Equal content cannot
//! authorize a foreign catalog/bank token or replace an outgoing voice's original scope.
//! Original-rate device PCM and project-rate clip derivatives remain different roles, even for a
//! common asset ID. Control verifies original rate/extent and hashes under explicit frame-work and
//! metadata reservations, checking cancellation between sources and bounded chunks. The normalized
//! stereo content hash includes its domain prefix, rate, extent and f32 bit patterns, not the
//! asset ID or encoded WAV bytes. Hashing never runs in sample/process callbacks. These facts
//! alone authorize neither native publication nor retirement. Ordinary registered instruments use
//! [`PreparedDeviceResources`] and runtime-owned leases; the obsolete private constructor retains
//! its separately closed [`ControlSamplerSources`] root.
use crate::{AudioInputStorageAndObligations, AudioSourceToken, AudioStorageError};
use hex_lattice::{DeviceResourceIdentity, DeviceResourcePcm, DeviceResourceView, DeviceResources};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetSlot, MAX_DEVICE_ASSET_REFERENCES, NativeDeviceState,
};
use hex_project::AssetId;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};

const ORIGINAL_PCM_DOMAIN: &[u8] = b"hex.device-resource.original-stereo.v1\0";
const HASH_BLOCK_FRAMES: usize = 256;
const HASH_STEREO_FRAME_BYTES: usize = 8;

mod admission;
pub(crate) mod current_sampler;
mod ingress;
mod resident;
pub use resident::sampler_import_template_on_control;
mod prepared;
mod snapshot;
pub use admission::{
    ControlSamplerOutbound, ControlSamplerSelection, DeviceResourceInventory, DeviceResourcePhase,
    inspect_device_resource_union,
};
pub use ingress::ResidentAudioIngressRefusal;
pub(crate) use prepared::{CompilerDeviceResources, inline_resource_factory_bytes};
pub use prepared::{
    DeviceResourceAdmission, DeviceResourcePublication, DeviceResourcePublishError,
    MAX_PLAYBACK_DEVICE_RESOURCE_LEASES, PreparedDeviceResourceLease, PreparedDeviceResources,
};
pub use resident::{ResidentDeviceAssets, StagedNativeAssetView};
pub use snapshot::{ControlSamplerSources, DispatchedSourceObligation};
pub(crate) use snapshot::{InstrumentResourcePreparation, prepare_instrument};

/// Expected original decoded source facts, captured by the media owner on control.
#[derive(Clone, Copy, Debug)]
pub struct OriginalResourceSource {
    pub asset: AssetId,
    pub sample_rate: u32,
    pub frames: u64,
}
/// Caller-reserved metadata and finite control work; no new global ceiling.
#[derive(Clone, Copy, Debug)]
pub struct ResourceCatalogReservation {
    pub metadata_bytes: usize,
    pub hash_frames: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourcePreparationError {
    Storage(AudioStorageError),
    MetadataCapacity,
    WorkCapacity,
    Cancelled,
    InvalidSource,
    InvalidScope,
}
impl From<AudioStorageError> for ResourcePreparationError {
    fn from(value: AudioStorageError) -> Self {
        Self::Storage(value)
    }
}
struct SourceFact {
    expected: OriginalResourceSource,
    identity: DeviceResourceIdentity,
    token: AudioSourceToken,
}
/// Plain verified facts and nonowning bank tokens, never an alternative PCM owner.
pub struct DeviceResourceCatalog {
    id: u64,
    sources: Vec<SourceFact>,
}
struct ScopeBinding {
    slot: DeviceAssetSlot,
    source: usize,
}
/// Authored slots map within ONE exact prepared device configuration.
pub struct DeviceResourceScope {
    catalog: u64,
    bindings: Vec<ScopeBinding>,
}
/// Callback queries borrow the actual bank, its verified facts and this scope.
pub struct ScopedDeviceResources<'a> {
    bank: BorrowedResourceBank<'a>,
    catalog: &'a DeviceResourceCatalog,
    scope: &'a DeviceResourceScope,
}
/// Both variants borrow actual constructor custody. This adapter neither owns
/// sources nor changes token validity, declarations or native admission.
#[derive(Clone, Copy)]
enum BorrowedResourceBank<'a> {
    Retained(&'a AudioInputStorageAndObligations),
    Inputs(&'a crate::OwnedAudioInputs),
}
impl<'a> BorrowedResourceBank<'a> {
    fn source_token(self, asset: AssetId) -> Result<AudioSourceToken, AudioStorageError> {
        match self {
            Self::Retained(bank) => bank.source_token(asset),
            Self::Inputs(bank) => bank.source_token(asset),
        }
    }
    fn source_view(
        self,
        token: &AudioSourceToken,
    ) -> Result<crate::OwnedAudioSourceView<'a>, AudioStorageError> {
        match self {
            Self::Retained(bank) => bank.source_view(token),
            Self::Inputs(bank) => bank.source_view(token),
        }
    }
}
fn quoted_bytes<T>(capacity: usize, header: usize) -> Result<usize, ResourcePreparationError> {
    capacity
        .checked_mul(std::mem::size_of::<T>())
        .and_then(|n| n.checked_add(header))
        .ok_or(ResourcePreparationError::MetadataCapacity)
}
/// Compute the original decoded stereo identity without granting bank, token or
/// provider custody. Control-side callers must bound the hash work explicitly.
pub fn original_pcm_facts_on_control(
    sample_rate: u32,
    pcm: &[crate::Frame],
    max_scan_frames: u64,
    cancel: &AtomicBool,
) -> Result<hex_project::OriginalSourceFacts, ResourcePreparationError> {
    cancellation(cancel)?;
    let frames = u64::try_from(pcm.len()).map_err(|_| ResourcePreparationError::InvalidSource)?;
    if sample_rate == 0 || frames == 0 {
        return Err(ResourcePreparationError::InvalidSource);
    }
    if frames > max_scan_frames {
        return Err(ResourcePreparationError::WorkCapacity);
    }
    let content = original_pcm_digest(sample_rate, frames, pcm, cancel)?;
    hex_project::OriginalSourceFacts::new(hex_project::OriginalSourceFacts::LAW_VERSION, content)
        .map_err(|_| ResourcePreparationError::InvalidSource)
}

fn original_pcm_digest(
    sample_rate: u32,
    frames: u64,
    pcm: &[crate::Frame],
    cancel: &AtomicBool,
) -> Result<[u8; 32], ResourcePreparationError> {
    // Fixed control-stack staging. Keep the existing little-endian protocol
    // and cancellation interval for both resource admission and file custody.
    let mut encoded = [0_u8; HASH_BLOCK_FRAMES * HASH_STEREO_FRAME_BYTES];
    let mut hash = Sha256::new();
    hash.update(ORIGINAL_PCM_DOMAIN);
    hash.update(sample_rate.to_le_bytes());
    hash.update(frames.to_le_bytes());
    for chunk in pcm.chunks(HASH_BLOCK_FRAMES) {
        cancellation(cancel)?;
        for (frame, bytes) in chunk
            .iter()
            .zip(encoded.chunks_exact_mut(HASH_STEREO_FRAME_BYTES))
        {
            bytes[..4].copy_from_slice(&frame[0].to_bits().to_le_bytes());
            bytes[4..].copy_from_slice(&frame[1].to_bits().to_le_bytes());
        }
        let used = chunk
            .len()
            .checked_mul(HASH_STEREO_FRAME_BYTES)
            .ok_or(ResourcePreparationError::InvalidSource)?;
        let bytes = encoded
            .get(..used)
            .ok_or(ResourcePreparationError::InvalidSource)?;
        hash.update(bytes);
    }
    Ok(hash.finalize().into())
}

fn cancellation(cancel: &AtomicBool) -> Result<(), ResourcePreparationError> {
    if cancel.load(Ordering::Relaxed) {
        Err(ResourcePreparationError::Cancelled)
    } else {
        Ok(())
    }
}
impl DeviceResourceCatalog {
    /// No owner is cloned or removed. Source records must be sorted and unique.
    /// Hashes describe normalized original-rate stereo f32 PCM, not encoded files.
    pub fn prepare_on_control(
        bank: &AudioInputStorageAndObligations,
        expected: &[OriginalResourceSource],
        reservation: ResourceCatalogReservation,
        cancel: &AtomicBool,
    ) -> Result<Self, ResourcePreparationError> {
        Self::prepare_bank_on_control(
            BorrowedResourceBank::Retained(bank),
            expected,
            reservation,
            cancel,
        )
    }
    #[cfg(test)]
    pub(crate) fn prepare_inputs_on_control(
        bank: &crate::OwnedAudioInputs,
        expected: &[OriginalResourceSource],
        reservation: ResourceCatalogReservation,
        cancel: &AtomicBool,
    ) -> Result<Self, ResourcePreparationError> {
        Self::prepare_bank_on_control(
            BorrowedResourceBank::Inputs(bank),
            expected,
            reservation,
            cancel,
        )
    }
    fn prepare_bank_on_control(
        bank: BorrowedResourceBank<'_>,
        expected: &[OriginalResourceSource],
        reservation: ResourceCatalogReservation,
        cancel: &AtomicBool,
    ) -> Result<Self, ResourcePreparationError> {
        if expected.len() > hex_project::MAX_ASSETS
            || expected
                .windows(2)
                .any(|pair| pair[0].asset.value() >= pair[1].asset.value())
        {
            return Err(ResourcePreparationError::InvalidSource);
        }
        Self::prepare_iterator_on_control(bank, || expected.iter().copied(), reservation, cancel)
    }
    /// Canonical project assets are unique but need not be ordered. Build the
    /// actual catalog directly; no temporary source-fact allocation overlaps it.
    pub(crate) fn prepare_inputs_iter_on_control<I: Iterator<Item = OriginalResourceSource>>(
        bank: &crate::OwnedAudioInputs,
        expected: impl Fn() -> I,
        reservation: ResourceCatalogReservation,
        cancel: &AtomicBool,
    ) -> Result<Self, ResourcePreparationError> {
        Self::prepare_iterator_on_control(
            BorrowedResourceBank::Inputs(bank),
            expected,
            reservation,
            cancel,
        )
    }
    fn prepare_iterator_on_control<I: Iterator<Item = OriginalResourceSource>>(
        bank: BorrowedResourceBank<'_>,
        expected: impl Fn() -> I,
        reservation: ResourceCatalogReservation,
        cancel: &AtomicBool,
    ) -> Result<Self, ResourcePreparationError> {
        cancellation(cancel)?;
        let count = expected().take(hex_project::MAX_ASSETS + 1).count();
        if count > hex_project::MAX_ASSETS {
            return Err(ResourcePreparationError::InvalidSource);
        }
        let quote = quoted_bytes::<SourceFact>(count, std::mem::size_of::<Self>())?;
        if quote > reservation.metadata_bytes {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let mut sources = Vec::new();
        sources
            .try_reserve_exact(count)
            .map_err(|_| AudioStorageError::Allocation)?;
        if quoted_bytes::<SourceFact>(sources.capacity(), std::mem::size_of::<Self>())?
            > reservation.metadata_bytes
        {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let mut work = reservation.hash_frames;
        for expected in expected() {
            cancellation(cancel)?;
            let token = bank.source_token(expected.asset)?;
            let source = bank.source_view(&token)?;
            let pcm = source
                .resident_frames()
                .ok_or(ResourcePreparationError::InvalidSource)?;
            if expected.sample_rate == 0
                || source.sample_rate() != expected.sample_rate
                || u64::try_from(pcm.len()).ok() != Some(expected.frames)
                || pcm.is_empty()
            {
                return Err(ResourcePreparationError::InvalidSource);
            }
            work = work
                .checked_sub(expected.frames)
                .ok_or(ResourcePreparationError::WorkCapacity)?;
            let content = original_pcm_digest(expected.sample_rate, expected.frames, pcm, cancel)?;
            let identity = DeviceResourceIdentity {
                asset: DeviceAssetKey::new(expected.asset.value())
                    .map_err(|_| ResourcePreparationError::InvalidSource)?,
                content,
                derivation_version: hex_project::OriginalSourceFacts::LAW_VERSION,
                derivation: Sha256::digest(ORIGINAL_PCM_DOMAIN).into(),
            };
            if sources.len() >= count || sources.len() >= sources.capacity() {
                return Err(ResourcePreparationError::InvalidSource);
            }
            sources.push(SourceFact {
                expected,
                identity,
                token,
            });
        }
        cancellation(cancel)?;
        if sources.len() != count {
            return Err(ResourcePreparationError::InvalidSource);
        }
        sources.sort_unstable_by_key(|s| s.expected.asset);
        if sources
            .windows(2)
            .any(|w| w[0].expected.asset == w[1].expected.asset)
        {
            return Err(ResourcePreparationError::InvalidSource);
        }
        Ok(Self {
            id: crate::audio_only_inputs::handle()?,
            sources,
        })
    }
    pub fn identity(&self, asset: AssetId) -> Option<DeviceResourceIdentity> {
        self.source_index(asset.value())
            .map(|index| self.sources[index].identity)
    }
    fn source_index(&self, asset: u64) -> Option<usize> {
        self.sources
            .binary_search_by_key(&asset, |source| source.expected.asset.value())
            .ok()
    }
    /// Value plus backing-buffer declaration, not allocator/RSS or admission proof.
    pub fn declared_metadata_bytes(&self) -> Result<usize, ResourcePreparationError> {
        quoted_bytes::<SourceFact>(self.sources.capacity(), std::mem::size_of::<Self>())
    }
    pub fn prepare_scope_on_control(
        &self,
        state: &NativeDeviceState,
        metadata_bytes: usize,
    ) -> Result<DeviceResourceScope, ResourcePreparationError> {
        let references = state.asset_references();
        if references.len() > MAX_DEVICE_ASSET_REFERENCES
            || quoted_bytes::<ScopeBinding>(
                references.len(),
                std::mem::size_of::<DeviceResourceScope>(),
            )? > metadata_bytes
        {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let mut bindings = Vec::new();
        bindings
            .try_reserve_exact(references.len())
            .map_err(|_| AudioStorageError::Allocation)?;
        if quoted_bytes::<ScopeBinding>(
            bindings.capacity(),
            std::mem::size_of::<DeviceResourceScope>(),
        )? > metadata_bytes
        {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        for reference in references {
            let source = self
                .source_index(reference.asset.value())
                .ok_or(ResourcePreparationError::InvalidScope)?;
            bindings.push(ScopeBinding {
                slot: reference.slot,
                source,
            });
        }
        Ok(DeviceResourceScope {
            catalog: self.id,
            bindings,
        })
    }
    /// Bounded immutable token checks only; no owner cloning or final drop.
    pub fn resources<'a>(
        &'a self,
        bank: &'a AudioInputStorageAndObligations,
        scope: &'a DeviceResourceScope,
    ) -> Result<ScopedDeviceResources<'a>, ResourcePreparationError> {
        self.resources_from_bank(BorrowedResourceBank::Retained(bank), scope)
    }
    pub(crate) fn resources_from_inputs<'a>(
        &'a self,
        bank: &'a crate::OwnedAudioInputs,
        scope: &'a DeviceResourceScope,
    ) -> Result<ScopedDeviceResources<'a>, ResourcePreparationError> {
        self.resources_from_bank(BorrowedResourceBank::Inputs(bank), scope)
    }
    fn resources_from_bank<'a>(
        &'a self,
        bank: BorrowedResourceBank<'a>,
        scope: &'a DeviceResourceScope,
    ) -> Result<ScopedDeviceResources<'a>, ResourcePreparationError> {
        if scope.catalog != self.id {
            return Err(ResourcePreparationError::InvalidScope);
        }
        for binding in &scope.bindings {
            bank.source_view(&self.sources[binding.source].token)?;
        }
        Ok(ScopedDeviceResources {
            bank,
            catalog: self,
            scope,
        })
    }
}
impl DeviceResourceScope {
    pub fn declared_metadata_bytes(&self) -> Result<usize, ResourcePreparationError> {
        quoted_bytes::<ScopeBinding>(self.bindings.capacity(), std::mem::size_of::<Self>())
    }
}
impl DeviceResources for ScopedDeviceResources<'_> {
    fn len(&self) -> usize {
        self.scope.bindings.len()
    }
    fn get(&self, index: usize) -> Option<DeviceResourceView<'_>> {
        let binding = self.scope.bindings.get(index)?;
        let fact = self.catalog.sources.get(binding.source)?;
        let source = self.bank.source_view(&fact.token).ok()?;
        let pcm = source.resident_frames()?;
        if source.sample_rate() != fact.expected.sample_rate
            || pcm.len() as u64 != fact.expected.frames
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

#[cfg(test)]
mod tests;

/// Metadata classification for containment/refusal only; type identity alone
/// never grants source preparation, native admission or a musical runtime.
/// Conservative control-side dispatch hint for an incoming retained source.
/// This grants no resource/admission authority; the existing constructor still
/// validates state, original-bank identity and all work/storage reservations.
pub fn requires_private_sampler_state(state: &hex_lattice_contract::NativeDeviceState) -> bool {
    hex_devices::sampler::state::requires_source(state)
}
pub fn requires_private_sampler_sources(project: &hex_project::Project) -> bool {
    project.tracks().iter().any(|track| {
        track.receives_source_audio()
            && track
                .instrument()
                .is_some_and(|device| requires_private_sampler_state(device.state()))
    })
}
#[cfg(feature = "instrument-test-support")]
pub mod control_drop_probe;
#[cfg(any(test, feature = "instrument-test-support"))]
pub mod current_sampler_fixture;

/// A synchronous resource borrow into one sealed owner. Both rendering modes
/// use the same scope lookup and mutable sampler kernel; neither retains it.
#[derive(Clone, Copy)]
pub(crate) enum SamplerSourcesRef<'a> {
    Control(&'a ControlSamplerSources),
    Current(&'a current_sampler::CurrentSamplerSources),
}
impl<'a> SamplerSourcesRef<'a> {
    pub(crate) fn id(self) -> u64 {
        match self {
            Self::Control(s) => s.id(),
            Self::Current(s) => s.metadata.id,
        }
    }
    pub(crate) fn has_track(self, track: hex_project::TrackId) -> bool {
        match self {
            Self::Control(s) => s.has_track(track),
            Self::Current(s) => s.metadata.configuration(track).is_some(),
        }
    }
    pub(crate) fn resources(
        self,
        track: hex_project::TrackId,
    ) -> Result<ScopedDeviceResources<'a>, ResourcePreparationError> {
        match self {
            Self::Control(s) => s.resources(track),
            Self::Current(s) => s.resources(track),
        }
    }
}
