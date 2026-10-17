//! Control-side catalog resolution for neutral device asset references.
use crate::{AssetId, Device, Invalid, Project};

impl Project {
    fn asset_devices(&self) -> impl Iterator<Item = &Device> {
        self.tracks()
            .iter()
            .flat_map(|track| track.devices().iter().chain(track.instrument()))
            .chain(
                self.return_tracks()
                    .iter()
                    .flat_map(|track| track.devices()),
            )
    }

    /// Resolve identities only; this performs no file access or PCM admission.
    pub fn validate_device_asset_references(&self) -> Result<(), Invalid> {
        for device in self.asset_devices() {
            for reference in device.state().asset_references() {
                if self.asset(AssetId(reference.asset.value())).is_none() {
                    return Err(Invalid("device references missing project media"));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn device_uses_asset(&self, asset: AssetId) -> bool {
        self.asset_devices().any(|device| {
            device
                .state()
                .asset_references()
                .iter()
                .any(|reference| reference.asset.value() == asset.value())
        })
    }
}
