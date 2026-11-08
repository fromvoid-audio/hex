// Schema66 writer (before schema67 device-state gating, Pulse #4088). Every
// encoder this program reaches is unchanged from origin/main 2ed11f24a; see
// provenance.json. Sampler states are raw v2 bytes: the device codec is not a
// hex-storage dependency.
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceTypeId, NativeDeviceState};
use hex_project::{Command, Created, Editor};
const SAMPLER: DeviceTypeId = DeviceTypeId::from_static("hex.instrument.sampler");
/// v2 classic sampler: loop on over frames 1..4 of a 4-frame source.
fn v2_payload() -> Vec<u8> {
    let mut bytes = vec![1, 0, 0, 60, 16, 0, 0, 0];
    for n in [0_u32, 4, 1, 4] { bytes.extend_from_slice(&n.to_le_bytes()); }
    for v in [0.0_f64, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0] { bytes.extend_from_slice(&v.to_le_bytes()); }
    bytes
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if hex_storage::NEWEST_SCHEMA_VERSION < 66 { return Err("requires a schema66 writer".into()); }
    let mut args = std::env::args_os().skip(1);
    let input = args.next().ok_or("schema65 input path missing")?;
    let path = args.next().ok_or("output path missing")?;
    let mut e = Editor::new(hex_storage::decode(&std::fs::read(input)?)?, 64)?;
    // A new musical-clock reversed clip names an explicit v2 read law (schema66).
    let track = e.project().tracks()[0].id();
    let asset = e.project().tracks()[0].clips()[0].asset();
    let Created::Clip(clip) = e.execute(Command::PlaceClip { track, asset, start: 1_440_000, duration: 24_000, source_offset: 0, source_length: 30_000, looping: false })? else { return Err("place".into()) };
    e.execute(Command::SetAudioClipMusical { clip, enabled: true })?;
    e.execute(Command::ReverseClips(vec![clip]))?;
    // A loaded v2 sampler and an empty v2 sampler.
    let Created::Asset(source) = e.execute(Command::ImportDeviceAsset { path: "Media/sampler.wav".into(), sample_rate: 48_000, frames: 4 })? else { return Err("asset".into()) };
    let Created::Track(loaded) = e.execute(Command::AddMidiTrack { name: "sampler".into() })? else { return Err("track".into()) };
    let state = NativeDeviceState::new(SAMPLER, 2, v2_payload())?
        .with_asset_references(vec![DeviceAssetReference { slot: DeviceAssetSlot::new(1)?, asset: DeviceAssetKey::new(source.value())? }])?
        .with_output_gain_millidb(-1500)?;
    e.execute(Command::InsertInstrument { track: loaded, state })?;
    let Created::Track(empty) = e.execute(Command::AddMidiTrack { name: "empty".into() })? else { return Err("track".into()) };
    e.execute(Command::InsertInstrument { track: empty, state: NativeDeviceState::new(SAMPLER, 2, b"empty".to_vec())? })?;
    let encoded = hex_storage::encode(e.project())?;
    if hex_storage::schema_version_for(e.project()) != 66 { return Err("schema".into()); }
    if hex_storage::decode(&encoded)?.snapshot() != e.project().snapshot() { return Err("round trip".into()); }
    std::fs::write(path, encoded)?;
    Ok(())
}
