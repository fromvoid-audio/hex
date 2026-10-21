//! Nondefault production-Sampler fixture shared by native boundary probes.
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, NativeDeviceState,
};
/// Original deterministic zero-envelope trigger state, built with the owning
/// device's real codec. This helper exposes neither PCM nor native custody.
pub fn one_shot_state(
    frames: usize,
    asset: hex_project::AssetId,
) -> Result<NativeDeviceState, crate::Invalid> {
    let mut state = hex_devices::sampler::state::SamplerState::for_source_frames(frames)
        .map_err(|_| crate::Invalid("fixture source frames invalid"))?;
    state.mode = hex_devices::sampler::state::AuthoredMode::OneShot {
        gate: hex_devices::sampler::GateMode::Trigger,
    };
    state.envelope = hex_devices::sampler::Envelope {
        attack_seconds: 0.0,
        decay_seconds: 0.0,
        sustain: 1.0,
        release_seconds: 0.0,
    };
    state
        .to_native()
        .map_err(|_| crate::Invalid("fixture sampler state invalid"))?
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).map_err(|_| crate::Invalid("fixture slot invalid"))?,
            asset: DeviceAssetKey::new(asset.value())
                .map_err(|_| crate::Invalid("fixture asset invalid"))?,
        }])
        .map_err(|_| crate::Invalid("fixture binding invalid"))
}
