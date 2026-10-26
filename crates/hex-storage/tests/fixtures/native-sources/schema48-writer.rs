//! Authentic schema48 fixture writer, run against main677a00f4f384c42c59e5811687a6fb69d50aa689.
//! Reads the recorded schema47 document and uses canonical loop editing + the
//! unchanged historical public encoder. No media is opened or rewritten.
use hex_project::{Command, Editor, SessionAudioLoop, SessionCellContent};
use std::{error::Error, io::Write};
fn main() -> Result<(), Box<dyn Error>> {
    let original = include_bytes!("schema47-console-one-shot.json");
    let mut editor = Editor::new(hex_storage::decode(original)?, 8)?;
    let clip = editor.project().session_cells().iter().find_map(|cell| match cell.content() { SessionCellContent::Audio(clip) => Some(clip.id()), _ => None }).ok_or("fixture audio cell missing")?;
    editor.execute(Command::SetSessionAudioClipLoop { clip, loop_range: Some(SessionAudioLoop::new(17,71)?) })?;
    let bytes = hex_storage::encode(editor.project())?;
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&bytes)?["version"],48);
    assert_eq!(hex_storage::decode(&bytes)?, *editor.project());
    let destination = std::env::args().nth(1).ok_or("output destination missing")?;
    let mut output = std::fs::OpenOptions::new().write(true).create_new(true).open(destination)?;
    output.write_all(&bytes)?;
    Ok(())
}
