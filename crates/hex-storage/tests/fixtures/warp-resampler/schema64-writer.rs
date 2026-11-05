// Run against e0b401f74, the schema64 writer (before schema65 warp laws).
use hex_project::{Command, Created, Editor, Project, WarpMode, default_audio_warp};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if hex_storage::NEWEST_SCHEMA_VERSION != 64 { return Err("requires schema64 writer".into()); }
    let path = std::env::args_os().nth(1).ok_or("output path missing")?;
    let mut e = Editor::new(Project::new(48_000)?, 64)?;
    // Sinc v2 media makes the document schema64.
    let Created::Imported { track, clip: pitched, asset, .. } = e.execute(Command::ImportPreparedAudioTrack { name: "pitched".into(), path: "Media/source-96000.wav".into(), source_sample_rate: 96_000, source_frames: 192_000, playback_frames: 96_000, conversion_version: 2 })? else { return Err("import".into()) };
    e.execute(Command::SetAudioClipPitch { clip: pitched, cents: 700 })?;
    // An alias shares the pitched content.
    let Created::Clips { first: alias, .. } = e.execute(Command::DuplicateAliasClips(vec![pitched]))? else { return Err("alias".into()) };
    e.execute(Command::SetClipFades { clip: alias, fade_in: 120, fade_out: 240 })?;
    // A warped clip.
    let Created::Clip(warped) = e.execute(Command::PlaceClip { track, asset, start: 480_000, duration: 96_000, source_offset: 0, source_length: 96_000, looping: false })? else { return Err("warped".into()) };
    let clip = e.project().clip(warped).ok_or("warped clip")?.clone();
    let settings = default_audio_warp(&clip, e.project().tempo_map(), WarpMode::RePitch)?;
    e.execute(Command::SetAudioClipWarp { clip: warped, settings: Some(settings) })?;
    // An unpitched, unwarped processed clip (fade only).
    let Created::Clip(plain) = e.execute(Command::PlaceClip { track, asset, start: 720_000, duration: 48_000, source_offset: 0, source_length: 48_000, looping: false })? else { return Err("plain".into()) };
    e.execute(Command::SetAudioClipSourceGain { clip: plain, points: vec![hex_project::SourceGainPoint { frame: 0, gain_q16: 32_768 }] })?;
    // A group retains two pitched roots.
    let mut roots = Vec::new();
    for start in [960_000, 1_056_000] {
        let Created::Clip(root) = e.execute(Command::PlaceClip { track, asset, start, duration: 48_000, source_offset: 0, source_length: 48_000, looping: false })? else { return Err("root".into()) };
        e.execute(Command::SetAudioClipPitch { clip: root, cents: -300 })?;
        roots.push(root);
    }
    e.execute(Command::GroupClips(roots))?;
    let encoded = hex_storage::encode(e.project())?;
    if hex_storage::schema_version_for(e.project()) != 64 { return Err("schema".into()); }
    if hex_storage::decode(&encoded)?.snapshot() != e.project().snapshot() { return Err("round trip".into()); }
    std::fs::write(path, encoded)?;
    Ok(())
}
