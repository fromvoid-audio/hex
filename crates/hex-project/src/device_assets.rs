//! Control-side catalog resolution for neutral device asset references.
use crate::{AssetId, Device, Invalid, Project};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, MAX_DEVICE_ASSET_REFERENCES};

pub(crate) fn apply(
    project: &mut Project,
    command: crate::Command,
) -> Result<crate::Created, Invalid> {
    let crate::Command::ImportDeviceAsset {
        device,
        expected_state,
        state,
        slot,
        path,
        source_sample_rate,
        source_frames,
    } = command
    else {
        return Err(Invalid("command is not a device asset import"));
    };
    if project
        .device(device)
        .ok_or(Invalid("device no longer exists"))?
        .state()
        != &expected_state
        || state.type_id() != expected_state.type_id()
    {
        return Err(Invalid(
            "device changed; retry sample loading from its current state",
        ));
    }
    // Loading changes the authored payload and this one binding. Provider-side
    // reconstruction must not erase other slots or host gain metadata.
    let state = state
        .with_host_metadata_from(&expected_state)
        .map_err(|_| Invalid("device host metadata is invalid"))?;
    let count = state
        .asset_references()
        .iter()
        .filter(|reference| reference.slot != slot)
        .count()
        .checked_add(1)
        .ok_or(Invalid("device asset reference count overflow"))?;
    if count > MAX_DEVICE_ASSET_REFERENCES {
        return Err(Invalid("device asset reference limit reached"));
    }
    let mut references = Vec::new();
    references
        .try_reserve_exact(count)
        .map_err(|_| Invalid("could not reserve device asset references"))?;
    references.extend(
        state
            .asset_references()
            .iter()
            .copied()
            .filter(|reference| reference.slot != slot),
    );
    let playback_frames =
        crate::converted_frame_count(source_frames, source_sample_rate, project.sample_rate())?;
    let version = if source_sample_rate == project.sample_rate() {
        crate::IDENTITY_CONVERSION
    } else {
        crate::SINC_CONVERSION_V1
    };
    let asset = project.add_prepared_asset(
        path,
        source_sample_rate,
        source_frames,
        playback_frames,
        version,
    )?;
    references.push(DeviceAssetReference {
        slot,
        asset: DeviceAssetKey::new(asset.value())
            .map_err(|_| Invalid("new media identity is invalid"))?,
    });
    let state = state
        .with_asset_references(references)
        .map_err(|_| Invalid("device asset binding is invalid"))?;
    project.device_mut(device)?.state = state;
    Ok(crate::Created::Asset(asset))
}

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
            .chain(self.main_devices())
    }

    /// Resolve identities only; this performs no file access or PCM admission.
    pub fn validate_device_asset_references(&self) -> Result<(), Invalid> {
        for device in self.asset_devices() {
            for reference in device
                .authored_states()
                .flat_map(|state| state.asset_references())
            {
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
                .authored_states()
                .flat_map(|state| state.asset_references())
                .any(|reference| reference.asset.value() == asset.value())
        })
    }
}

#[cfg(test)]
mod tests;
