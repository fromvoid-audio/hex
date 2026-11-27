//! Control-side Drum Rack pad drop builders & read-only pad summaries.
//!
//! Bridges `hex-lattice`'s `DrumRackSpec` and `hex-devices`'s built-in
//! instruments / `Sampler` state for `hex-session` without widening session
//! crate boundaries or allowing UI callers to synthesize fake pad state.

use crate::Invalid;
use hex_lattice::{
    DRUM_RACK_TYPE_ID, DrumRackPadSpec, DrumRackSpec, INSTRUMENT_RACK_TYPE_ID, NativeOutputPortId,
};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceTypeId, NativeDeviceState,
};
use hex_project::AssetId;
use std::num::NonZeroU64;

/// Read-only metadata projection of one Drum Rack pad for UI/session inspection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrumRackPadSummary {
    pub pad_id: NonZeroU64,
    pub name: String,
    pub receive_note: u8,
    pub play_note: u8,
    pub occupied_type: Option<DeviceTypeId>,
    pub has_sample_asset: bool,
}

/// Returns whether `type_id` is a supported leaf native instrument eligible for
/// dropping onto a Drum Rack pad (explicitly refusing nested racks and unknown types).
pub fn is_supported_pad_instrument(type_id: DeviceTypeId) -> bool {
    if type_id == DRUM_RACK_TYPE_ID || type_id == INSTRUMENT_RACK_TYPE_ID {
        return false;
    }
    hex_devices::native_leaf_instrument_registry()
        .ok()
        .is_some_and(|reg| reg.definition(type_id).is_some())
}

/// Build an initial Drum Rack `NativeDeviceState` pre-populated with `pad_count`
/// empty pads (`receive_note = base_note + i`, `play_note = 60`, `MAIN_STEREO`).
pub fn default_drum_rack_with_empty_pads(
    rack_id: NonZeroU64,
    base_note: u8,
    pad_count: usize,
) -> Result<NativeDeviceState, Invalid> {
    if pad_count == 0 || pad_count > hex_lattice::DRUM_RACK_MAX_PADS {
        return Err(Invalid("drum rack pad count out of bounds"));
    }
    let mut pads = Vec::with_capacity(pad_count);
    for idx in 0..pad_count {
        let receive_note = base_note
            .checked_add(idx as u8)
            .filter(|&n| n <= 127)
            .ok_or(Invalid("drum rack pad note out of midi range"))?;
        let pad_id = NonZeroU64::new((idx as u64) + 1).ok_or(Invalid("invalid pad id"))?;
        let device_id =
            NonZeroU64::new(100 + (idx as u64) + 1).ok_or(Invalid("invalid pad device id"))?;
        pads.push(DrumRackPadSpec {
            pad_id,
            device_id,
            name: format!("pad {}", idx + 1),
            receive_note,
            play_note: 60,
            choke_group: None,
            gain: 1.0,
            muted: false,
            output_port: NativeOutputPortId::MAIN_STEREO,
            state: None,
        });
    }
    DrumRackSpec {
        rack_id,
        gain: 1.0,
        pads,
    }
    .encode_state()
    .map_err(|_| Invalid("invalid drum rack state"))
}

/// Project read-only pad summaries from an existing Drum Rack `NativeDeviceState`.
pub fn pad_summaries(rack_state: &NativeDeviceState) -> Result<Vec<DrumRackPadSummary>, Invalid> {
    let spec = DrumRackSpec::decode_and_migrate_state(rack_state)
        .map_err(|_| Invalid("device is not a valid drum rack"))?;
    Ok(spec
        .pads
        .iter()
        .map(|pad| DrumRackPadSummary {
            pad_id: pad.pad_id,
            name: pad.name.clone(),
            receive_note: pad.receive_note,
            play_note: pad.play_note,
            occupied_type: pad.state.as_ref().map(NativeDeviceState::type_id),
            has_sample_asset: pad
                .state
                .as_ref()
                .is_some_and(|s| !s.asset_references().is_empty()),
        })
        .collect())
}

/// Produce the next validated Drum Rack `NativeDeviceState` after dropping a
/// built-in leaf native instrument onto `pad_id`. Sibling pads and their assets
/// remain completely untouched.
pub fn build_pad_instrument_state(
    rack_state: &NativeDeviceState,
    pad_id: NonZeroU64,
    type_id: DeviceTypeId,
    pad_name: Option<&str>,
) -> Result<NativeDeviceState, Invalid> {
    if !is_supported_pad_instrument(type_id) {
        return Err(Invalid("unsupported instrument for drum rack pad"));
    }
    let mut spec = DrumRackSpec::decode_and_migrate_state(rack_state)
        .map_err(|_| Invalid("target device is not a valid drum rack"))?;
    let leaf_reg = hex_devices::native_leaf_instrument_registry()
        .map_err(|_| Invalid("native instrument registry unavailable"))?;
    let child_state = leaf_reg
        .default_state(type_id)
        .map_err(|_| Invalid("could not construct default pad instrument state"))?;
    spec.set_pad_state(pad_id, Some(child_state))
        .map_err(|_| Invalid("drum rack pad target no longer exists"))?;
    if let Some(name) = pad_name
        && !name.trim().is_empty()
    {
        let trimmed = truncate_pad_name(name);
        let _ = spec.rename_pad(pad_id, trimmed);
    }
    spec.encode_state()
        .map_err(|_| Invalid("could not encode updated drum rack state"))
}

/// Produce the next validated Drum Rack `NativeDeviceState` after importing a
/// sample file (`asset_id`, `source_frames`) onto `pad_id` as a normal `Sampler`
/// bound via `DeviceAssetReference`. Sibling pads and their assets remain intact.
pub fn build_pad_sampler_state(
    rack_state: &NativeDeviceState,
    pad_id: NonZeroU64,
    asset_id: AssetId,
    source_frames: u64,
    pad_name: Option<&str>,
) -> Result<NativeDeviceState, Invalid> {
    if source_frames == 0 {
        return Err(Invalid("sample drop requires non-empty audio frames"));
    }
    let mut spec = DrumRackSpec::decode_and_migrate_state(rack_state)
        .map_err(|_| Invalid("target device is not a valid drum rack"))?;
    if spec.pad_by_id(pad_id).is_none() {
        return Err(Invalid("drum rack pad target no longer exists"));
    }
    let source_frames = usize::try_from(source_frames)
        .map_err(|_| Invalid("sample frame count exceeds host range"))?;
    let mut sampler = hex_devices::sampler::state::SamplerState::for_source_frames(source_frames)
        .map_err(|_| Invalid("sample frame count unsupported by sampler"))?;
    sampler.mode = hex_devices::sampler::state::AuthoredMode::OneShot {
        gate: hex_devices::sampler::GateMode::Trigger,
    };
    sampler.envelope = hex_devices::sampler::Envelope {
        attack_seconds: 0.0,
        decay_seconds: 0.0,
        sustain: 1.0,
        release_seconds: 0.005,
    };
    let slot = DeviceAssetSlot::new(1).map_err(|_| Invalid("invalid sampler slot"))?;
    let asset_key =
        DeviceAssetKey::new(asset_id.value()).map_err(|_| Invalid("invalid asset key"))?;
    let sampler_state = sampler
        .to_native()
        .map_err(|_| Invalid("could not encode sampler state"))?
        .with_asset_references(vec![DeviceAssetReference {
            slot,
            asset: asset_key,
        }])
        .map_err(|_| Invalid("could not bind sampler asset reference"))?;
    spec.set_pad_state(pad_id, Some(sampler_state))
        .map_err(|_| Invalid("could not assign sampler to drum rack pad"))?;
    if let Some(name) = pad_name
        && !name.trim().is_empty()
    {
        let trimmed = truncate_pad_name(name);
        let _ = spec.rename_pad(pad_id, trimmed);
    }
    spec.encode_state()
        .map_err(|_| Invalid("could not encode updated drum rack state"))
}

fn truncate_pad_name(raw: &str) -> String {
    let lower = raw.trim().to_ascii_lowercase();
    let mut out = String::new();
    for ch in lower.chars() {
        if out.len() + ch.len_utf8() > hex_lattice::DRUM_RACK_MAX_PAD_NAME_BYTES {
            break;
        }
        out.push(ch);
    }
    out
}
