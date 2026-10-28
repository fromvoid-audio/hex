// Run against 091b3addf0107450d67b19219cddfdf75c572319, the schema61 writer.
use hex_project::{AudioEventEdit, BeatTime, ClipGain, Command, Created, Editor, MidiClipGain, MidiClipVolume, MidiClipVolumePoint, Project};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if hex_storage::CURRENT_SCHEMA_VERSION != 61 { return Err("requires schema61 writer".into()); }
    let path = std::env::args_os().nth(1).ok_or("output path missing")?;
    let mut e = Editor::new(Project::new(48_000)?, 16)?;
    let Created::Track(track) = e.execute(Command::AddTrack { name: "midi".into() })? else { return Err("track".into()) };
    let length = BeatTime::from_quarters(4)?;
    let Created::Clip(clip) = e.execute(Command::PlaceMidiClip { track, start: BeatTime::ZERO, length })? else { return Err("clip".into()) };
    e.execute(Command::AddMidiNote { clip, start: BeatTime::ZERO, duration: BeatTime::from_quarters(1)?, pitch: 60, channel: 0, velocity: 90 })?;
    e.execute(Command::ReplaceMidiClipVolume { clip, volume: MidiClipVolume::new(vec![MidiClipVolumePoint::linear(BeatTime::ZERO, MidiClipGain::new(0.5)?)], length)? })?;
    let Created::Scene(scene) = e.execute(Command::AddScene { name: "scene".into() })? else { return Err("scene".into()) };
    e.execute(Command::CopyArrangementMidiToSession { source: clip, scene })?;
    let Created::Imported { clip: audio, .. } = e.execute(Command::ImportAudioTrack { name: "audio".into(), path: "source.wav".into(), sample_rate: 48_000, frames: 100 })? else { return Err("audio".into()) };
    e.execute(Command::EditAudioEvents { clip: audio, operation: AudioEventEdit::SetGain { event: 1, gain: ClipGain::new(-6_000)? } })?;
    let encoded = hex_storage::encode(e.project())?;
    if hex_storage::decode(&encoded)?.snapshot() != e.project().snapshot() { return Err("round trip".into()); }
    std::fs::write(path, encoded)?;
    Ok(())
}
