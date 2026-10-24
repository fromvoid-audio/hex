//! Borrowed native-device resources; ownership and admission stay with the host.
//!
//! Each inventory belongs to one exact prepared configuration, including its original sample rate,
//! channel layout, content and derivation identity. Reusing an authored slot in a later
//! configuration does not rebind an outgoing voice. The host must retain that voice's actual
//! original inventory until retirement; an equal digest, asset key or borrowed view cannot
//! establish source custody. Control validates PCM and numeric bounds; callback queries must be
//! bounded and free of allocation, blocking, I/O and owning-handle mutation.
use crate::TuningTable;
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetSlot, DeviceContractError, MAX_DEVICE_ASSET_REFERENCES,
    NativeDeviceState,
};

/// Host-verified content and derivation identity. Values alone confer no lease.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceResourceIdentity {
    pub asset: DeviceAssetKey,
    pub content: [u8; 32],
    pub derivation_version: u32,
    pub derivation: [u8; 32],
}
/// Supported layouts are explicit; mono expansion never creates an owning buffer.
#[derive(Clone, Copy, Debug)]
pub enum DeviceResourcePcm<'a> {
    Mono(&'a [f32]),
    Stereo(&'a [[f32; 2]]),
}
impl DeviceResourcePcm<'_> {
    pub fn frames(self) -> usize {
        match self {
            Self::Mono(pcm) => pcm.len(),
            Self::Stereo(pcm) => pcm.len(),
        }
    }
    pub fn frame(self, index: usize) -> Option<[f32; 2]> {
        match self {
            Self::Mono(pcm) => pcm.get(index).map(|&sample| [sample; 2]),
            Self::Stereo(pcm) => pcm.get(index).copied(),
        }
    }
    fn finite(self) -> bool {
        match self {
            Self::Mono(pcm) => pcm.iter().all(|sample| sample.is_finite()),
            Self::Stereo(pcm) => pcm.iter().flatten().all(|sample| sample.is_finite()),
        }
    }
}
/// Immutable original-rate samples borrowed only while the corresponding host root lives.
#[derive(Clone, Copy, Debug)]
pub struct DeviceResourceView<'a> {
    pub slot: DeviceAssetSlot,
    pub identity: DeviceResourceIdentity,
    pub sample_rate: u32,
    pub pcm: DeviceResourcePcm<'a>,
}
/// Inventory for one prepared device configuration, not the entire graph's slot namespace.
/// Implementations return a stable ordered inventory for the lifetime of its retained root.
/// Carried voices must borrow their actual source configuration's inventory, even
/// when a target device reuses the same authored slot numbers.
/// Queries must be bounded, allocation-free and free of I/O, waits and control locks.
/// The host retains/charges exact immutable owners before exposing this interface;
/// it must never use an identity or a borrowed view as a substitute for that proof.
pub trait DeviceResources: Sync {
    fn len(&self) -> usize;
    fn get(&self, index: usize) -> Option<DeviceResourceView<'_>>;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn slot(&self, slot: DeviceAssetSlot) -> Option<DeviceResourceView<'_>> {
        // The fixed contract cap also bounds queries from direct provider callers.
        (0..self.len().min(MAX_DEVICE_ASSET_REFERENCES))
            .filter_map(|index| self.get(index))
            .find(|view| view.slot == slot)
    }
}
/// Direct borrowed inventory for control fixtures and already retained host scopes.
/// It neither owns PCM nor grants engine resource admission.
pub struct BorrowedDeviceResources<'a>(pub &'a [DeviceResourceView<'a>]);
impl DeviceResources for BorrowedDeviceResources<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }
    fn get(&self, index: usize) -> Option<DeviceResourceView<'_>> {
        self.0.get(index).copied()
    }
}
pub static EMPTY_DEVICE_RESOURCES: BorrowedDeviceResources<'static> = BorrowedDeviceResources(&[]);

/// One current preparation boundary. Never construct or validate this on callback.
/// Tuning is already validated; resource PCM verification occurs on control under
/// the host's existing work/cancellation/memory limits before root publication.
pub struct DevicePreparationContext<'a> {
    pub sample_rate: u32,
    pub tuning: &'a TuningTable,
    pub resources: &'a dyn DeviceResources,
}
impl DevicePreparationContext<'_> {
    /// Control-only complete slot/type/bounds/finite-PCM check. This is not a
    /// cryptographic identity verification, reservation or runtime admission.
    pub fn validate_for(&self, state: &NativeDeviceState) -> Result<(), DeviceContractError> {
        if self.sample_rate == 0 {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        if self.resources.len() > MAX_DEVICE_ASSET_REFERENCES
            || self.resources.len() != state.asset_references().len()
        {
            return Err(DeviceContractError::AssetResourcesUnavailable);
        }
        for (index, reference) in state.asset_references().iter().enumerate() {
            let view = self
                .resources
                .get(index)
                .ok_or(DeviceContractError::AssetResourcesUnavailable)?;
            if view.slot != reference.slot
                || view.identity.asset != reference.asset
                || view.sample_rate == 0
                || view.identity.derivation_version == 0
                || view.pcm.frames() == 0
                || !view.pcm.finite()
            {
                return Err(DeviceContractError::AssetResourcesUnavailable);
            }
        }
        Ok(())
    }
}
