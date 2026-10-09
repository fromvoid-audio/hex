//! Headless import -> canonical edit model -> compiled playback -> export.
use hex_engine::compile;
use hex_media::{Limits, decode_wav_file};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, error::Error, path::Path};

pub fn render(input: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    // Explicit resident-media ceiling; native streaming is a separate future path.
    let media = decode_wav_file(input, Limits::new(64 * 1024 * 1024, 8_000_000))?;
    let frames = u64::try_from(media.frames())?;
    let mut editor = Editor::new(Project::new(media.sample_rate())?, 32)?;
    let asset = match editor.execute(Command::ImportAudioTrack {
        name: "imported audio".into(),
        path: media.path().into(),
        sample_rate: media.sample_rate(),
        frames,
    })? {
        Created::Imported { asset, .. } => asset,
        _ => return Err("import did not return its asset id".into()),
    };
    let bank = BTreeMap::from([(asset, media.audio().clone())]);
    let project = compile::project(editor.project(), &bank)?;
    super::wav::render(output, &project, frames)?;
    println!(
        "rendered {} imported frames at {} hz",
        media.frames(),
        media.sample_rate()
    );
    Ok(())
}
