//! Bounded control-side inspection and pad-drop mutation helpers for the native Drum Rack.
//!
//! Never called from the audio callback. Decodes and validates canonical
//! [`hex_lattice::DrumRackSpec`] bytes, builds normal child native instrument or
//! Sampler states, and re-encodes outer [`NativeDeviceState`] preserving host
//! metadata and aggregated [`DeviceAssetReference`] bindings.

use crate::Invalid;
use hex_lattice::{
    DRUM_RACK_MAX_PAD_NAME_BYTES, DRUM_RACK_MAX_PADS, DRUM_RACK_TYPE_ID, DrumRackPadSpec,
    DrumRackSpec, INSTRUMENT_RACK_TYPE_ID,
};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceTypeId, NativeDeviceState,
};
use hex_project::AssetId;
use std::{
    num::{NonZeroU8, NonZeroU64},
    path::Path,
};

/// Default first MIDI receive note for grid pad 1 (C1 = MIDI 36).
pub const DRUM_RACK_BASE_NOTE: u8 = 36;
/// Default play note forwarded to a newly populated pad instrument (C3 = MIDI 60).
pub const DRUM_RACK_DEFAULT_PLAY_NOTE: u8 = 60;

/// Read-only control/UI summary of one bounded Drum Rack grid slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrumRackPadView {
    pub pad_id: NonZeroU64,
    pub receive_note: u8,
    pub play_note: u8,
    pub name: String,
    pub occupied: bool,
    pub child_type_id: Option<DeviceTypeId>,
    pub child_display_name: Option<String>,
    pub asset_key: Option<DeviceAssetKey>,
    pub muted: bool,
    pub choke_group: Option<NonZeroU8>,
}

/// Whether `type_id` is an admitted built-in leaf native instrument for a Drum Rack pad.
/// Nested racks and unknown types are refused.
pub fn drum_rack_child_instrument_admitted(type_id: DeviceTypeId) -> bool {
    if type_id == DRUM_RACK_TYPE_ID || type_id == INSTRUMENT_RACK_TYPE_ID {
        return false;
    }
    hex_devices::native_leaf_instrument_registry()
        .is_ok_and(|registry| registry.definition(type_id).is_some())
}

/// Format a concise lowercase pad label from a dropped sample path, bounded by
/// `DRUM_RACK_MAX_PAD_NAME_BYTES`.
pub fn drum_rack_pad_label_for_path(path: &Path) -> String {
    let raw = path
        .file_stem()
        .or_else(|| path.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sample".into());
    let lowered = raw.trim().to_lowercase();
    let trimmed = if lowered.is_empty() {
        "sample".to_owned()
    } else {
        lowered
    };
    truncate_utf8_bytes(&trimmed, DRUM_RACK_MAX_PAD_NAME_BYTES)
}

/// Format a MIDI note number as a concise lowercase pitch label (`c1`, `c#1`, ...).
pub fn drum_rack_note_label(note: u8) -> String {
    const NAMES: [&str; 12] = [
        "c", "c#", "d", "d#", "e", "f", "f#", "g", "g#", "a", "a#", "b",
    ];
    let pitch = usize::from(note % 12);
    let octave = i16::from(note / 12) - 2;
    format!("{}{octave}", NAMES[pitch])
}

/// Produce the bounded 16-slot pad view for a canonical Drum Rack state.
///
/// Every authored pad in `spec.pads` keeps its exact canonical `pad_id`,
/// `receive_note`, `name`, and child summary. Remaining empty grid slots up to
/// `DRUM_RACK_MAX_PADS` receive deterministic unused `pad_id`s in `1..=16` and
/// unused receive notes starting from `C1` (`36..=51`).
pub fn drum_rack_pad_views_on_control(
    state: &NativeDeviceState,
) -> Result<Vec<DrumRackPadView>, Invalid> {
    let spec = DrumRackSpec::decode_and_migrate_state(state)
        .map_err(|_| Invalid("drum rack state is invalid"))?;
    let leaf_registry = hex_devices::native_leaf_instrument_registry()
        .map_err(|_| Invalid("built-in instrument registry unavailable"))?;

    let mut views = Vec::with_capacity(DRUM_RACK_MAX_PADS);
    for pad in &spec.pads {
        let (occupied, child_type_id, child_display_name, asset_key) = match &pad.state {
            Some(child) => {
                let type_id = child.type_id();
                let display = leaf_registry
                    .definition(type_id)
                    .and_then(|def| def.display_name())
                    .map(|name| name.to_lowercase())
                    .or_else(|| type_id.as_str().ok().map(str::to_lowercase));
                let asset = child
                    .asset_references()
                    .first()
                    .map(|reference| reference.asset);
                (true, Some(type_id), display, asset)
            }
            None => (false, None, None, None),
        };
        views.push(DrumRackPadView {
            pad_id: pad.pad_id,
            receive_note: pad.receive_note,
            play_note: pad.play_note,
            name: pad.name.clone(),
            occupied,
            child_type_id,
            child_display_name,
            asset_key,
            muted: pad.muted,
            choke_group: pad.choke_group,
        });
    }

    for slot in 1..=(DRUM_RACK_MAX_PADS as u64) {
        if views.len() >= DRUM_RACK_MAX_PADS {
            break;
        }
        let Some(pad_id) = NonZeroU64::new(slot) else {
            continue;
        };
        if views.iter().any(|view| view.pad_id == pad_id) {
            continue;
        }
        let default_note = default_receive_note_for_slot(slot, &views);
        views.push(DrumRackPadView {
            pad_id,
            receive_note: default_note,
            play_note: DRUM_RACK_DEFAULT_PLAY_NOTE,
            name: format!("pad {slot}"),
            occupied: false,
            child_type_id: None,
            child_display_name: None,
            asset_key: None,
            muted: false,
            choke_group: None,
        });
    }

    views.sort_by_key(|view| (view.pad_id.get(), view.receive_note));
    Ok(views)
}

/// Patch one Drum Rack pad with a fresh default built-in leaf native instrument.
/// Refuses nested racks, unknown types, or invalid/unadmitted pad targets.
pub fn patch_drum_rack_pad_instrument_on_control(
    state: &NativeDeviceState,
    pad_id: NonZeroU64,
    child_type_id: DeviceTypeId,
) -> Result<NativeDeviceState, Invalid> {
    if child_type_id == DRUM_RACK_TYPE_ID || child_type_id == INSTRUMENT_RACK_TYPE_ID {
        return Err(Invalid(
            "nested racks cannot be dropped onto a drum rack pad",
        ));
    }
    let leaf_registry = hex_devices::native_leaf_instrument_registry()
        .map_err(|_| Invalid("built-in instrument registry unavailable"))?;
    let definition = leaf_registry
        .definition(child_type_id)
        .ok_or(Invalid("unsupported instrument for drum rack pad"))?;
    let child_state = definition
        .default_state()
        .map_err(|_| Invalid("instrument default state is unavailable"))?;
    let label = definition
        .display_name()
        .map(str::to_lowercase)
        .unwrap_or_else(|| {
            child_type_id
                .as_str()
                .unwrap_or("instrument")
                .to_lowercase()
        });
    apply_pad_child_state(state, pad_id, child_state, &label)
}

/// Patch one Drum Rack pad with a normal native Sampler bound to `asset` and `frames`.
pub fn patch_drum_rack_pad_sample_on_control(
    state: &NativeDeviceState,
    pad_id: NonZeroU64,
    label: &str,
    frames: usize,
    asset: AssetId,
) -> Result<NativeDeviceState, Invalid> {
    if frames == 0 {
        return Err(Invalid("drum rack sample must be nonempty"));
    }
    let slot = DeviceAssetSlot::new(hex_devices::sampler::state::SOURCE_SLOT)
        .map_err(|_| Invalid("invalid sampler source slot"))?;
    let key = DeviceAssetKey::new(asset.value())
        .map_err(|_| Invalid("invalid drum rack sample asset id"))?;
    let child_state = hex_devices::sampler::state::SamplerState::for_source_frames(frames)
        .and_then(|sampler| sampler.to_native())
        .and_then(|native| {
            native.with_asset_references(vec![DeviceAssetReference { slot, asset: key }])
        })
        .map_err(|_| Invalid("failed to build sampler state for drum rack pad"))?;
    let clean_label = if label.trim().is_empty() {
        "sample".to_owned()
    } else {
        truncate_utf8_bytes(&label.trim().to_lowercase(), DRUM_RACK_MAX_PAD_NAME_BYTES)
    };
    apply_pad_child_state(state, pad_id, child_state, &clean_label)
}

fn apply_pad_child_state(
    state: &NativeDeviceState,
    pad_id: NonZeroU64,
    child_state: NativeDeviceState,
    label: &str,
) -> Result<NativeDeviceState, Invalid> {
    let mut spec = DrumRackSpec::decode_and_migrate_state(state)
        .map_err(|_| Invalid("drum rack state is invalid"))?;
    let bounded_name = truncate_utf8_bytes(label, DRUM_RACK_MAX_PAD_NAME_BYTES);

    if let Some(existing) = spec.pads.iter_mut().find(|pad| pad.pad_id == pad_id) {
        existing.state = Some(child_state);
        existing.name = bounded_name;
    } else {
        if pad_id.get() > DRUM_RACK_MAX_PADS as u64 || spec.pads.len() >= DRUM_RACK_MAX_PADS {
            return Err(Invalid("drum rack pad target is unavailable"));
        }
        let device_id = next_pad_device_id(&spec)?;
        let receive_note = default_receive_note_for_spec_slot(pad_id.get(), &spec);
        spec.pads.push(DrumRackPadSpec {
            pad_id,
            device_id,
            name: bounded_name,
            receive_note,
            play_note: DRUM_RACK_DEFAULT_PLAY_NOTE,
            choke_group: None,
            gain: 1.0,
            muted: false,
            state: Some(child_state),
        });
    }

    spec.encode_state()
        .and_then(|encoded| encoded.with_input_gain_millidb(state.input_gain_millidb()))
        .and_then(|encoded| encoded.with_output_gain_millidb(state.output_gain_millidb()))
        .map_err(|_| Invalid("failed to encode drum rack pad state"))
}

fn next_pad_device_id(spec: &DrumRackSpec) -> Result<NonZeroU64, Invalid> {
    let max_existing = spec
        .pads
        .iter()
        .map(|pad| pad.device_id.get())
        .chain(std::iter::once(spec.rack_id.get()))
        .max()
        .unwrap_or(1);
    let candidate = max_existing
        .checked_add(1)
        .and_then(NonZeroU64::new)
        .ok_or(Invalid("drum rack pad device id exhausted"))?;
    Ok(candidate)
}

fn default_receive_note_for_slot(slot: u64, views: &[DrumRackPadView]) -> u8 {
    let preferred = DRUM_RACK_BASE_NOTE.saturating_add(u8::try_from(slot - 1).unwrap_or(0));
    if !views.iter().any(|view| view.receive_note == preferred) {
        return preferred;
    }
    for note in
        DRUM_RACK_BASE_NOTE..=DRUM_RACK_BASE_NOTE.saturating_add(DRUM_RACK_MAX_PADS as u8 - 1)
    {
        if !views.iter().any(|view| view.receive_note == note) {
            return note;
        }
    }
    (0..=127)
        .find(|&note| !views.iter().any(|view| view.receive_note == note))
        .unwrap_or(preferred)
}

fn default_receive_note_for_spec_slot(slot: u64, spec: &DrumRackSpec) -> u8 {
    let preferred = DRUM_RACK_BASE_NOTE.saturating_add(u8::try_from(slot - 1).unwrap_or(0));
    if !spec.pads.iter().any(|pad| pad.receive_note == preferred) {
        return preferred;
    }
    for note in
        DRUM_RACK_BASE_NOTE..=DRUM_RACK_BASE_NOTE.saturating_add(DRUM_RACK_MAX_PADS as u8 - 1)
    {
        if !spec.pads.iter().any(|pad| pad.receive_note == note) {
            return note;
        }
    }
    (0..=127)
        .find(|&note| !spec.pads.iter().any(|pad| pad.receive_note == note))
        .unwrap_or(preferred)
}

fn truncate_utf8_bytes(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    input[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NativeDeviceClass;

    fn default_drum_rack() -> NativeDeviceState {
        crate::default_native_device_state_for_class(
            NativeDeviceClass::Instrument,
            DRUM_RACK_TYPE_ID,
        )
        .expect("default drum rack state")
    }

    fn sample_asset_id() -> AssetId {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 4).unwrap();
        let hex_project::Created::Asset(asset) = editor
            .execute(hex_project::Command::ImportDeviceAsset {
                path: std::path::PathBuf::from("kick.wav"),
                sample_rate: 48_000,
                frames: 256,
            })
            .unwrap()
        else {
            panic!("expected asset");
        };
        asset
    }

    #[test]
    fn default_drum_rack_exposes_sixteen_empty_slots_and_accepts_sample_and_instrument() {
        let initial = default_drum_rack()
            .with_input_gain_millidb(-300)
            .unwrap()
            .with_output_gain_millidb(250)
            .unwrap();
        let views = drum_rack_pad_views_on_control(&initial).unwrap();
        assert_eq!(views.len(), DRUM_RACK_MAX_PADS);
        assert!(views.iter().all(|v| !v.occupied));
        assert_eq!(views[0].pad_id.get(), 1);
        assert_eq!(views[0].receive_note, 36);
        assert_eq!(views[15].pad_id.get(), 16);
        assert_eq!(views[15].receive_note, 51);

        // Drop a sample onto pad 1.
        let asset = sample_asset_id();
        let pad1 = NonZeroU64::new(1).unwrap();
        let with_sample =
            patch_drum_rack_pad_sample_on_control(&initial, pad1, "Kick_01", 256, asset).unwrap();
        assert_eq!(with_sample.input_gain_millidb(), -300);
        assert_eq!(with_sample.output_gain_millidb(), 250);
        assert_eq!(with_sample.asset_references().len(), 1);
        assert_eq!(
            with_sample.asset_references()[0].asset.value(),
            asset.value()
        );

        // Drop a leaf native instrument onto pad 2.
        let pad2 = NonZeroU64::new(2).unwrap();
        let with_both = patch_drum_rack_pad_instrument_on_control(
            &with_sample,
            pad2,
            hex_devices::subtractive::SUBTRACTIVE_TYPE_ID,
        )
        .unwrap();
        let views = drum_rack_pad_views_on_control(&with_both).unwrap();
        assert!(views[0].occupied);
        assert_eq!(views[0].name, "kick_01");
        assert_eq!(
            views[0].asset_key,
            Some(DeviceAssetKey::new(asset.value()).unwrap())
        );
        assert_eq!(
            views[0].child_type_id,
            Some(DeviceTypeId::from_static("hex.instrument.sampler"))
        );
        assert!(views[1].occupied);
        assert_eq!(
            views[1].child_type_id,
            Some(hex_devices::subtractive::SUBTRACTIVE_TYPE_ID)
        );
        assert_eq!(views[1].receive_note, 37);
    }

    #[test]
    fn refuses_nested_racks_unknown_instruments_zero_frame_sample_and_oob_pad() {
        let initial = default_drum_rack();
        let asset = sample_asset_id();
        let pad1 = NonZeroU64::new(1).unwrap();
        assert!(
            patch_drum_rack_pad_instrument_on_control(&initial, pad1, DRUM_RACK_TYPE_ID).is_err()
        );
        assert!(
            patch_drum_rack_pad_instrument_on_control(&initial, pad1, INSTRUMENT_RACK_TYPE_ID)
                .is_err()
        );
        assert!(
            patch_drum_rack_pad_instrument_on_control(
                &initial,
                pad1,
                DeviceTypeId::from_static("hex.instrument.unknown")
            )
            .is_err()
        );
        assert!(patch_drum_rack_pad_sample_on_control(&initial, pad1, "bad", 0, asset).is_err());
        let oob = NonZeroU64::new(17).unwrap();
        assert!(
            patch_drum_rack_pad_instrument_on_control(
                &initial,
                oob,
                hex_devices::SINE_INSTRUMENT_TYPE_ID
            )
            .is_err()
        );
    }

    #[test]
    fn replacing_existing_pad_preserves_custom_note_and_choke_group() {
        let pad_id = NonZeroU64::new(10).unwrap();
        let device_id = NonZeroU64::new(100).unwrap();
        let spec = DrumRackSpec {
            rack_id: NonZeroU64::new(1).unwrap(),
            gain: 1.0,
            pads: vec![DrumRackPadSpec {
                pad_id,
                device_id,
                name: "hat".into(),
                receive_note: 42,
                play_note: 64,
                choke_group: NonZeroU8::new(3),
                gain: 0.75,
                muted: false,
                state: None,
            }],
        };
        let encoded = spec.encode_state().unwrap();
        let patched = patch_drum_rack_pad_instrument_on_control(
            &encoded,
            pad_id,
            hex_devices::fm4::device::FM4_TYPE_ID,
        )
        .unwrap();
        let decoded = DrumRackSpec::decode_and_migrate_state(&patched).unwrap();
        assert_eq!(decoded.pads.len(), 1);
        let pad = &decoded.pads[0];
        assert_eq!(pad.pad_id, pad_id);
        assert_eq!(pad.device_id, device_id);
        assert_eq!(pad.receive_note, 42);
        assert_eq!(pad.play_note, 64);
        assert_eq!(pad.choke_group, NonZeroU8::new(3));
        assert_eq!(pad.gain, 0.75);
        assert_eq!(
            pad.state.as_ref().map(NativeDeviceState::type_id),
            Some(hex_devices::fm4::device::FM4_TYPE_ID)
        );
    }
}
