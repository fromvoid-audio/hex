// Run against 4ce0ecfb0, the schema65 writer (before schema66 view read laws).
use hex_project::{AudioDomainClock, BeatTime, Command, Created, Editor, Project, Tempo};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if hex_storage::NEWEST_SCHEMA_VERSION != 65 { return Err("requires schema65 writer".into()); }
    let path = std::env::args_os().nth(1).ok_or("output path missing")?;
    let mut e = Editor::new(Project::new(48_000)?, 64)?;
    let Created::Imported { track, clip: musical, asset, .. } = e.execute(Command::ImportAudioTrack { name: "directed".into(), path: "Media/witness.wav".into(), sample_rate: 48_000, frames: 30_000 })? else { return Err("import".into()) };
    e.execute(Command::SetClipFades { clip: musical, fade_in: 0, fade_out: 0 })?;
    for (beat, bpm) in [(BeatTime::ZERO, 240), (BeatTime::from_ratio(1, 2)?, 60)] {
        e.execute(Command::SetTempoPoint { beat, tempo: Tempo::new(bpm * 1_000_000)? })?;
    }
    // A musical-clock reversed view reads fractional base positions (v1).
    e.execute(Command::SetAudioClipMusical { clip: musical, enabled: true })?;
    e.execute(Command::ReverseClips(vec![musical]))?;
    // A frame-clock reversed view copies integer samples.
    let Created::Clip(frames) = e.execute(Command::PlaceClip { track, asset, start: 480_000, duration: 24_000, source_offset: 0, source_length: 30_000, looping: false })? else { return Err("frames".into()) };
    e.execute(Command::ReverseClips(vec![frames]))?;
    // New pitch takes the explicit v2 content law, making the document schema65.
    let Created::Clip(pitched) = e.execute(Command::PlaceClip { track, asset, start: 960_000, duration: 24_000, source_offset: 0, source_length: 30_000, looping: false })? else { return Err("pitched".into()) };
    e.execute(Command::SetAudioClipPitch { clip: pitched, cents: 700 })?;
    let clocks: Vec<_> = e.project().tracks()[0].clips().iter().filter_map(|c| c.audio_edit()).filter(|a| a.content_view.is_some()).map(|a| matches!(a.domain.clock, AudioDomainClock::Beats(_))).collect();
    if clocks != [true, false] { return Err(format!("views {clocks:?}").into()); }
    let encoded = hex_storage::encode(e.project())?;
    if hex_storage::schema_version_for(e.project()) != 65 { return Err("schema".into()); }
    if hex_storage::decode(&encoded)?.snapshot() != e.project().snapshot() { return Err("round trip".into()); }
    std::fs::write(path, encoded)?;
    Ok(())
}
