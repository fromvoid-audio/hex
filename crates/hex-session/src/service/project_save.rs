//! Desktop saves collect project-owned generated audio into the destination
//! folder. The saved image can use relative asset paths while the live session
//! keeps its original media owners, identities, and history.

use super::{DocumentHandle, RequestCancellation, SaveCurrentCommit, recovery, save_document_as};
use hex_project::Project;
use hex_storage::project_folder::{self, Destination, SAMPLES};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) fn save_as(
    path: PathBuf,
    extension_added: bool,
    project: Arc<Project>,
    recovery_path: Option<&Path>,
    cancellation: &RequestCancellation,
) -> Result<(DocumentHandle, SaveCurrentCommit), String> {
    let exists = match std::fs::symlink_metadata(&path) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(display(hex_storage::StorageError::Io(error))),
    };
    if extension_added && exists {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        return Err(format!(
            "{} already exists · choose it by name to replace it",
            hex_storage::text::verbatim(name)
        ));
    }
    publish_as(path, exists, project, recovery_path, cancellation)
}

/// Publish a desktop project copy without permission to replace any file.
pub(super) fn save_copy(
    path: PathBuf,
    project: Arc<Project>,
    recovery_path: Option<&Path>,
    cancellation: &RequestCancellation,
) -> Result<PathBuf, String> {
    if path
        .try_exists()
        .map_err(|error| display(hex_storage::StorageError::Io(error)))?
    {
        return Err("copy destination exists · choose a new filename".into());
    }
    let (handle, commit) = publish_as(path, false, project, recovery_path, cancellation)?;
    match commit.error {
        Some(error) => Err(error),
        None => Ok(handle.path),
    }
}

fn publish_as(
    path: PathBuf,
    replace_existing: bool,
    project: Arc<Project>,
    recovery_path: Option<&Path>,
    cancellation: &RequestCancellation,
) -> Result<(DocumentHandle, SaveCurrentCommit), String> {
    let destination = Destination::prepare(path, replace_existing).map_err(display)?;
    match destination {
        Destination::File(path) => {
            let image = Image::for_document(project, &path, recovery_path)?;
            image.collect(cancellation)?;
            if replace_existing {
                save_document_as(path, &image.project)
            } else {
                hex_storage::save_new_with_outcome(path, &image.project)
                    .map(super::bind_new_save)
                    .map_err(display)
            }
        }
        Destination::Folder(folder) => {
            let image = (|| {
                let image = Image::new(project, Some(folder.root()), recovery_path)?;
                image.collect(cancellation)?;
                Ok::<_, String>(image)
            })()
            .map_err(|error| {
                format!(
                    "{error} · staging path {} was not removed by hex · inspect it before using or removing it",
                    hex_storage::text::verbatim(folder.root().display())
                )
            })?;
            let outcome = folder
                .publish(&image.project, || cancellation.is_cancelled())
                .map_err(display)?;
            Ok(super::bind_new_save(outcome))
        }
    }
}

pub(super) struct Image {
    pub(super) project: Arc<Project>,
    root: Option<PathBuf>,
    samples: Vec<(PathBuf, PathBuf)>,
}

impl Image {
    pub(super) fn for_document(
        project: Arc<Project>,
        path: &Path,
        recovery_path: Option<&Path>,
    ) -> Result<Self, String> {
        let root = project_folder::root_for_document(path).map_err(display)?;
        Self::new(project, root, recovery_path)
    }

    fn new(
        project: Arc<Project>,
        root: Option<&Path>,
        recovery_path: Option<&Path>,
    ) -> Result<Self, String> {
        let mut samples = Vec::new();
        let project = if root.is_some() {
            let generated = GeneratedRoots::new(recovery_path);
            let mut has_generated = false;
            for asset in project.assets() {
                if generated.relative(asset.path())?.is_some() {
                    has_generated = true;
                    break;
                }
            }
            if has_generated {
                let mut snapshot = project.snapshot();
                for asset in &mut snapshot.assets {
                    if let Some(relative) = generated.relative(&asset.path)? {
                        let source =
                            std::mem::replace(&mut asset.path, Path::new(SAMPLES).join(&relative));
                        samples.push((source, relative));
                    }
                }
                Arc::new(Project::from_snapshot(snapshot).map_err(display)?)
            } else {
                project
            }
        } else {
            project
        };
        Ok(Self {
            project,
            root: root.map(Path::to_path_buf),
            samples,
        })
    }

    pub(super) fn collect(&self, cancellation: &RequestCancellation) -> Result<(), String> {
        if let Some(root) = &self.root {
            for (source, relative) in &self.samples {
                project_folder::collect_sample(root, relative, source, || {
                    cancellation.is_cancelled()
                })
                .map_err(|error| {
                    format!(
                        "could not collect sample {} · {}",
                        hex_storage::text::verbatim(source.display()),
                        display(error)
                    )
                })?;
            }
        }
        if cancellation.is_cancelled() {
            return Err("project save cancelled".into());
        }
        Ok(())
    }
}

struct GeneratedRoots {
    bounce: Option<PathBuf>,
    default_bounce: Option<PathBuf>,
    takes: Option<PathBuf>,
}

impl GeneratedRoots {
    fn new(recovery_path: Option<&Path>) -> Self {
        let default = recovery::untitled_recovery_path().ok();
        let app = default.as_deref().and_then(app_root);
        let resolved = |root: PathBuf| root.canonicalize().unwrap_or(root);
        Self {
            bounce: recovery_path
                .and_then(app_root)
                .map(|root| resolved(root.join("media/bounces"))),
            default_bounce: app.map(|root| resolved(root.join("media/bounces"))),
            takes: app.map(|root| resolved(root.join("sample-takes"))),
        }
    }

    fn relative(&self, source: &Path) -> Result<Option<PathBuf>, String> {
        for directory in source.ancestors().skip(1) {
            if directory.file_name().is_some_and(|name| name == SAMPLES)
                && let Some(root) = directory.parent()
                && project_folder::is_root(root).map_err(display)?
                && let Ok(path) = source.strip_prefix(directory)
            {
                return Ok(Some(path.to_path_buf()));
            }
        }
        for (root, category) in [
            (self.bounce.as_deref(), "bounces"),
            (self.default_bounce.as_deref(), "bounces"),
            (self.takes.as_deref(), "recorded"),
        ] {
            if let Some(root) = root
                && let Ok(relative) = source.strip_prefix(root)
                && !relative.as_os_str().is_empty()
            {
                return Ok(Some(Path::new(category).join(relative)));
            }
        }
        Ok(None)
    }
}

fn app_root(recovery_path: &Path) -> Option<&Path> {
    recovery_path.parent().and_then(Path::parent)
}

/// Prepared bounces go directly to a saved project's samples. Untitled and
/// legacy projects keep the existing recovery-owned staging location.
pub(super) fn bounce_directory(
    document: Option<&DocumentHandle>,
    recovery_path: Option<&Path>,
) -> Result<PathBuf, String> {
    if let Some(document) = document
        && let Some(root) = project_folder::root_for_document(&document.path).map_err(display)?
    {
        return project_folder::sample_directory(root, Path::new("bounces")).map_err(display);
    }
    let default;
    let path = if let Some(path) = recovery_path {
        path
    } else {
        default = recovery::untitled_recovery_path()?;
        &default
    };
    app_root(path)
        .map(|root| root.join("media/bounces"))
        .ok_or("bounce media folder is unavailable".into())
}

/// Root for the recorder's immutable take directories. An unsaved or legacy
/// document uses the same app-owned storage as before.
pub fn sample_take_root(document: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(path) = document
        && let Some(root) = project_folder::root_for_document(path).map_err(display)?
    {
        return project_folder::sample_directory(root, Path::new("recorded")).map_err(display);
    }
    let recovery = recovery::untitled_recovery_path()?;
    app_root(&recovery)
        .map(|root| root.join("sample-takes"))
        .ok_or("sample take storage is unavailable".into())
}

pub(super) fn display(error: impl std::fmt::Display) -> String {
    hex_storage::text::lowercase_outside_verbatim(&error.to_string())
}

#[cfg(test)]
mod tests;
