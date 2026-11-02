// Run against a4f63df864f16d643c042a9c12d36b26b53ad069, the schema63 writer.
use hex_project::{Command, Editor, Project};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if hex_storage::CURRENT_SCHEMA_VERSION != 63 { return Err("requires schema63 writer".into()); }
    let path = std::env::args_os().nth(1).ok_or("output path missing")?;
    let mut e = Editor::new(Project::new(48_000)?, 16)?;
    // One sinc-v1 converted asset and one identity-rate asset.
    e.execute(Command::ImportPreparedAudioTrack { name: "converted".into(), path: "Media/source-44100.wav".into(), source_sample_rate: 44_100, source_frames: 44_100, playback_frames: 48_000, conversion_version: 1 })?;
    e.execute(Command::ImportPreparedAudioTrack { name: "native".into(), path: "Media/source-48000.wav".into(), source_sample_rate: 48_000, source_frames: 4_800, playback_frames: 4_800, conversion_version: 0 })?;
    let encoded = hex_storage::encode(e.project())?;
    if hex_storage::decode(&encoded)?.snapshot() != e.project().snapshot() { return Err("round trip".into()); }
    std::fs::write(path, encoded)?;
    Ok(())
}
