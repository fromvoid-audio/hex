//! CLI adapter for the shared WAV renderer and create-new publisher.
use hex_engine::Project;
use std::{io, path::Path};

pub fn render(path: &Path, project: &Project, frames: u64) -> io::Result<()> {
    hex_export::render_new(path, project, 0, frames)
        .map(|_| ())
        .map_err(io::Error::other)
}
