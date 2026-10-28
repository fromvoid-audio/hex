use hex_project::{BeatTime, ClipGain, Command, Created, Editor, MidiClipGain, MidiClipVolume, MidiClipVolumePoint, Project};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if hex_storage::CURRENT_SCHEMA_VERSION != 62 { return Err("requires schema62 writer".into()); }
    let path = std::env::args_os().nth(1).ok_or("missing output")?;
    let mut editor = Editor::new(Project::new(48_000)?, 16)?;
    let Created::Track(track) = editor.execute(Command::AddMidiTrack { name: "midi".into() })? else { return Err("track".into()) };
    let length = BeatTime::from_quarters(4)?;
    let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip { track, start: BeatTime::ZERO, length })? else { return Err("clip".into()) };
    editor.execute(Command::AddMidiNote { clip, start: BeatTime::ZERO, duration: BeatTime::ONE, pitch: 60, channel: 0, velocity: 90 })?;
    editor.execute(Command::ReplaceMidiClipVolume { clip, volume: MidiClipVolume::new(vec![MidiClipVolumePoint::linear(BeatTime::ZERO, MidiClipGain::new(0.5)?)], length)? })?;
    editor.execute(Command::SetMidiClipGain { clip, gain: ClipGain::new(-6_000)? })?;
    let Created::Scene(scene) = editor.execute(Command::AddScene { name: "scene".into() })? else { return Err("scene".into()) };
    editor.execute(Command::CopyArrangementMidiToSession { source: clip, scene })?;
    let encoded = hex_storage::encode(editor.project())?;
    if hex_storage::decode(&encoded)?.snapshot() != editor.project().snapshot() { return Err("round trip".into()); }
    std::fs::write(path, encoded)?;
    Ok(())
}
