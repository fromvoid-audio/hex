//! CLI adapter for the shared WAV renderer and create-new publisher.
use hex_engine::Project;
use std::{io, path::Path};

pub fn render(path: &Path, project: &Project, frames: u64) -> io::Result<()> {
    let report = hex_export::render_new(path, project, 0, frames).map_err(io::Error::other)?;
    if let Some(stage) = report.retained_stage {
        eprintln!(
            "staging path {} was not removed; inspect it before removing anything",
            stage.display()
        );
    }
    Ok(())
}
