// Run against immutable source 379d7fbc247fa6ea3375bf86b96a5ba878597063.
// This uses the public schema60 writer; the source WAV is metadata only.
use hex_project::{AudioEventEdit, Command, Created, Editor, Project};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if hex_storage::CURRENT_SCHEMA_VERSION != 60 { return Err("requires the frozen schema60 source".into()); }
    let path = std::env::args_os().nth(1).ok_or("output path missing")?;
    let mut editor = Editor::new(Project::new(48_000)?, 8)?;
    let Created::Imported { track, clip, .. } = editor.execute(Command::ImportAudioTrack {
        name: "source".into(), path: "source.wav".into(), sample_rate: 48_000, frames: 100,
    })? else { return Err("import failed".into()) };
    editor.execute(Command::SetTrackColor { track, color: hex_project::ElementColor::Red })?;
    editor.execute(Command::EditAudioEvents { clip, operation: AudioEventEdit::SetActive {
        events: vec![1], active: false,
    }})?;
    editor.execute(Command::DuplicateAliasClips(vec![clip]))?;
    let encoded = hex_storage::encode(editor.project())?;
    if hex_storage::decode(&encoded)?.snapshot() != editor.project().snapshot() {
        return Err("frozen fixture failed its own round trip".into());
    }
    std::fs::write(path, encoded)?;
    Ok(())
}
