//! Control-verified resource facts and per-configuration NONOWNING scope maps.
//! The actual input storage must live in the retained host root. These values,
//! digests and tokens do not grant native publication or retirement admission.
use crate::{AudioInputStorageAndObligations, AudioSourceToken, AudioStorageError};
use hex_lattice::{DeviceResourceIdentity, DeviceResourcePcm, DeviceResourceView, DeviceResources};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetSlot, MAX_DEVICE_ASSET_REFERENCES, NativeDeviceState,
};
use hex_project::AssetId;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) mod retained;
pub use retained::{DeviceResourceBank, DeviceResourceBankRefusal, PreparedDeviceResourceLease};
mod ingress;
pub use ingress::ResidentAudioIngressRefusal;

/// Expected original decoded source facts, captured by the media owner on control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OriginalResourceLayout {
    Mono,
    Stereo,
}
#[derive(Clone, Copy, Debug)]
pub struct OriginalResourceSource {
    pub asset: AssetId,
    pub sample_rate: u32,
    pub frames: u64,
    pub layout: OriginalResourceLayout,
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
#[derive(Clone, Copy, PartialEq, Eq)]
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
    bank: &'a AudioInputStorageAndObligations,
    catalog: &'a DeviceResourceCatalog,
    scope: &'a DeviceResourceScope,
}
fn quoted_bytes<T>(capacity: usize, header: usize) -> Result<usize, ResourcePreparationError> {
    capacity
        .checked_mul(std::mem::size_of::<T>())
        .and_then(|n| n.checked_add(header))
        .ok_or(ResourcePreparationError::MetadataCapacity)
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
        cancellation(cancel)?;
        if expected.len() > hex_project::MAX_ASSETS
            || expected
                .windows(2)
                .any(|pair| pair[0].asset.value() >= pair[1].asset.value())
        {
            return Err(ResourcePreparationError::InvalidSource);
        }
        let quote = quoted_bytes::<SourceFact>(expected.len(), std::mem::size_of::<Self>())?;
        if quote > reservation.metadata_bytes {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let mut sources = Vec::new();
        sources
            .try_reserve_exact(expected.len())
            .map_err(|_| AudioStorageError::Allocation)?;
        if quoted_bytes::<SourceFact>(sources.capacity(), std::mem::size_of::<Self>())?
            > reservation.metadata_bytes
        {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let mut work = reservation.hash_frames;
        for expected in expected {
            cancellation(cancel)?;
            let token = bank.source_token(expected.asset)?;
            let source = bank.source_view(&token)?;
            let pcm = match expected.layout {
                OriginalResourceLayout::Mono => DeviceResourcePcm::Mono(
                    source
                        .resident_mono_samples()
                        .ok_or(ResourcePreparationError::InvalidSource)?,
                ),
                OriginalResourceLayout::Stereo => DeviceResourcePcm::Stereo(
                    source
                        .resident_frames()
                        .ok_or(ResourcePreparationError::InvalidSource)?,
                ),
            };
            if expected.sample_rate == 0
                || source.sample_rate() != expected.sample_rate
                || u64::try_from(pcm.frames()).ok() != Some(expected.frames)
                || pcm.frames() == 0
            {
                return Err(ResourcePreparationError::InvalidSource);
            }
            work = work
                .checked_sub(expected.frames)
                .ok_or(ResourcePreparationError::WorkCapacity)?;
            let mut hash = Sha256::new();
            let prefix: &[u8] = match expected.layout {
                OriginalResourceLayout::Mono => b"hex.device-resource.original-mono.v1\0",
                OriginalResourceLayout::Stereo => b"hex.device-resource.original-stereo.v1\0",
            };
            hash.update(prefix);
            hash.update(expected.sample_rate.to_le_bytes());
            hash.update(expected.frames.to_le_bytes());
            match pcm {
                DeviceResourcePcm::Stereo(pcm) => {
                    for chunk in pcm.chunks(256) {
                        cancellation(cancel)?;
                        for frame in chunk {
                            for sample in frame {
                                hash.update(sample.to_bits().to_le_bytes());
                            }
                        }
                    }
                }
                DeviceResourcePcm::Mono(pcm) => {
                    for chunk in pcm.chunks(256) {
                        cancellation(cancel)?;
                        for sample in chunk {
                            hash.update(sample.to_bits().to_le_bytes());
                        }
                    }
                }
            }
            let identity = DeviceResourceIdentity {
                asset: DeviceAssetKey::new(expected.asset.value())
                    .map_err(|_| ResourcePreparationError::InvalidSource)?,
                content: hash.finalize().into(),
                derivation_version: 1,
                derivation: Sha256::digest(prefix).into(),
            };
            sources.push(SourceFact {
                expected: *expected,
                identity,
                token,
            });
        }
        cancellation(cancel)?;
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
        self.view(index)
    }
}
impl<'a> ScopedDeviceResources<'a> {
    fn view(&self, index: usize) -> Option<DeviceResourceView<'a>> {
        let binding = self.scope.bindings.get(index)?;
        let fact = self.catalog.sources.get(binding.source)?;
        let bank: &'a AudioInputStorageAndObligations = self.bank;
        let source = bank.source_view(&fact.token).ok()?;
        let pcm = match fact.expected.layout {
            OriginalResourceLayout::Mono => {
                DeviceResourcePcm::Mono(source.resident_mono_samples()?)
            }
            OriginalResourceLayout::Stereo => DeviceResourcePcm::Stereo(source.resident_frames()?),
        };
        if source.sample_rate() != fact.expected.sample_rate
            || pcm.frames() as u64 != fact.expected.frames
        {
            return None;
        }
        Some(DeviceResourceView {
            slot: binding.slot,
            identity: fact.identity,
            sample_rate: fact.expected.sample_rate,
            pcm,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests;
