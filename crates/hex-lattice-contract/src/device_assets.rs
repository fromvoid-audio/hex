//! Bounded control-side asset identities, independent of paths and PCM ownership.
use crate::DeviceContractError;

/// Initial host envelope bound, independent of the device's 4 KiB DSP payload.
/// This bounds reference metadata, not PCM or an admitted runtime resource pool.
pub const MAX_DEVICE_ASSET_REFERENCES: usize = 128;

/// Provider-defined stable slot; zero is reserved for an absent identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceAssetSlot(u32);
impl DeviceAssetSlot {
    pub const fn new(value: u32) -> Result<Self, DeviceContractError> {
        if value == 0 {
            Err(DeviceContractError::InvalidId)
        } else {
            Ok(Self(value))
        }
    }
    pub const fn value(self) -> u32 {
        self.0
    }
}

/// Project-scoped asset identity; it contains neither a path nor a resource owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceAssetKey(u64);
impl DeviceAssetKey {
    pub const fn new(value: u64) -> Result<Self, DeviceContractError> {
        if value == 0 {
            Err(DeviceContractError::InvalidId)
        } else {
            Ok(Self(value))
        }
    }
    pub const fn value(self) -> u64 {
        self.0
    }
}

/// Several slots may intentionally share one asset, but a slot binds only once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceAssetReference {
    pub slot: DeviceAssetSlot,
    pub asset: DeviceAssetKey,
}
