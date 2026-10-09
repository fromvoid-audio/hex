//! Explicit prior-version recovery to a new path, preserving both original files.
use hex_project::Project;
use std::{error::Error, path::Path};

pub fn recover_project(target: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    let recovered = hex_storage::load_recovery(target)?;
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let origin = if parent.is_absolute() {
        parent.to_path_buf()
    } else {
        std::env::current_dir()?.join(parent)
    };
    // A prior generation may still use relative media paths. Bind those to its
    // original directory, never to the new recovery destination or sidecar name.
    let mut snapshot = recovered.project.snapshot();
    for asset in &mut snapshot.assets {
        if !asset.path.is_absolute() {
            asset.path = origin.join(&asset.path);
        }
    }
    let project = Project::from_snapshot(snapshot)?;
    hex_storage::save_new(output, &project)?;
    println!("restored previous project to {}", output.display());
    Ok(())
}
