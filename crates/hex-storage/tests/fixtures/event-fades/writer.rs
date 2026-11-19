//! Run only against the frozen schema41 source recorded in provenance.json.
use hex_project::{Command, Editor};
use std::io::Write;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if hex_storage::CURRENT_SCHEMA_VERSION != 41 {
        return Err("expected the genuine schema41 writer".into());
    }
    let mut args = std::env::args_os().skip(1);
    let input = args.next().ok_or("missing predecessor input")?;
    let output = args.next().ok_or("missing create-new output")?;
    let project = hex_storage::decode(&std::fs::read(input)?)?;
    let ids = project
        .tracks()
        .iter()
        .flat_map(|track| {
            track
                .clips()
                .iter()
                .map(|c| c.id())
                .chain(track.midi_clips().iter().map(|c| c.id()))
        })
        .collect::<Vec<_>>();
    let mut editor = Editor::new(project, 32)?;
    if !ids.is_empty() {
        editor.execute(Command::ReverseClips(ids))?;
    }
    let bytes = hex_storage::encode(editor.project())?;
    if hex_storage::decode(&bytes)?.snapshot() != editor.project().snapshot() {
        return Err("predecessor writer roundtrip mismatch".into());
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?
        .write_all(&bytes)?;
    Ok(())
}
