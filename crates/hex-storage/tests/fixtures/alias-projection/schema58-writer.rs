// Run against immutable source 7a40a9ec37c1d0de57afa6005f4660b3d8456d98.
// This uses the public schema58 writer; the source WAV is metadata only.
use hex_project::{AudioEventEdit, Command, Created, Editor, Project};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if hex_storage::CURRENT_SCHEMA_VERSION != 58 { return Err("requires the frozen schema58 source".into()); }
    let path = std::env::args_os().nth(1).ok_or("output path missing")?;
    let mut editor = Editor::new(Project::new(48_000)?, 8)?;
    let Created::Imported { track, clip, .. } = editor.execute(Command::ImportAudioTrack {
        name: "source".into(), path: "source.wav".into(), sample_rate: 48_000, frames: 100,
    })? else { return Err("import failed".into()) };
    editor.execute(Command::SetTrackColor { track, color: hex_project::ElementColor::Red })?;
    editor.execute(Command::EditAudioEvents { clip, operation: AudioEventEdit::SetActive {
        events: vec![1], active: false,
    }})?;
    let encoded = hex_storage::encode(editor.project())?;
    if hex_storage::decode(&encoded)?.snapshot() != editor.project().snapshot() {
        return Err("frozen fixture failed its own round trip".into());
    }
    std::fs::write(path, encoded)?;
    Ok(())
}
