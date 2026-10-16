//! Explicit recovery and media relinking, prepared before canonical adoption.
use super::*;

/// Explicit source selection. Recovery never adopts an overwrite association.
#[derive(Clone, Debug)]
pub enum ProjectSource {
    Saved(PathBuf),
    Journal(PathBuf),
    SavedJournal(PathBuf),
    PreviousSave(PathBuf),
}

/// Native application singleton, shared by development and review bundles.
/// No fallback to another user's directory when HOME is unavailable.
pub fn untitled_recovery_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("home directory is unavailable")?;
    Ok(PathBuf::from(home).join("Library/Application Support/hex/recovery/untitled.hex-journal"))
}

type OpenCandidate = (Session, PathBuf, Option<DocumentHandle>, bool);

fn read_source(
    source: &ProjectSource,
) -> Result<(Project, PathBuf, Option<DocumentHandle>), String> {
    let (project, path, document) = match source {
        ProjectSource::Saved(path) => {
            let (project, token) = hex_storage::load_with_token(path).map_err(|e| e.to_string())?;
            (
                project,
                path,
                Some(DocumentHandle {
                    path: absolute_document_path(path)?,
                    token,
                }),
            )
        }
        ProjectSource::Journal(path) => (
            hex_storage::load_journal(path).map_err(|e| e.to_string())?,
            path,
            None,
        ),
        ProjectSource::SavedJournal(path) => {
            let journal = hex_storage::journal_path(path).map_err(|e| e.to_string())?;
            (
                hex_storage::load_journal(&journal).map_err(|e| e.to_string())?,
                path,
                None,
            )
        }
        ProjectSource::PreviousSave(path) => (
            hex_storage::load_recovery(path)
                .map_err(|e| e.to_string())?
                .project,
            path,
            None,
        ),
    };
    Ok((project, absolute_document_base(path)?, document))
}

/// Read-only dialog preparation, off the UI thread. Decode and complete candidate
/// validation still happen on Open; this is not an admission or a content hash.
pub fn missing_assets(
    source: &ProjectSource,
) -> Result<Vec<(hex_project::AssetId, PathBuf)>, String> {
    let (project, base, _) = read_source(source)?;
    let mut missing = Vec::new();
    for asset in project.assets() {
        let path = if asset.path().is_absolute() {
            asset.path().to_path_buf()
        } else {
            base.join(asset.path())
        };
        match std::fs::metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push((asset.id(), path))
            }
            Err(error) => return Err(error.to_string()),
            Ok(metadata) if !metadata.is_file() => {
                return Err(format!("media is not a regular file: {}", path.display()));
            }
            Ok(_) => {}
        }
    }
    Ok(missing)
}

pub(super) fn open_source(
    current: &Session,
    history_capacity: usize,
    source: &ProjectSource,
    relinks: &BTreeMap<hex_project::AssetId, PathBuf>,
) -> Result<OpenCandidate, String> {
    let (mut project, project_dir, document) = read_source(source)?;
    let dirty = document.is_none() || !relinks.is_empty();
    if !relinks.is_empty() {
        if relinks.keys().any(|id| project.asset(*id).is_none()) {
            return Err("relink refers to an unknown asset".into());
        }
        let paths: BTreeMap<_, _> = relinks
            .iter()
            .map(|(id, path)| (id.value(), path))
            .collect();
        let mut snapshot = project.snapshot();
        for asset in &mut snapshot.assets {
            if let Some(path) = paths.get(&asset.id) {
                asset.path = absolute_document_path(path)?;
            }
        }
        project = Project::from_snapshot(snapshot).map_err(|e| e.to_string())?;
    }
    if project.sample_rate() != current.project().sample_rate() {
        return Err("project sample rate differs from the active audio device".into());
    }
    let mut prepared_assets = Vec::new();
    prepared_assets
        .try_reserve_exact(project.assets().count())
        .map_err(|_| "could not reserve decoded media table".to_owned())?;
    let mut remaining_frames = current.max_resident_frames();
    // Load unreferenced assets too: the editor can place them later, so missing
    // media in any asset record rejects the candidate instead of dropping it.
    for asset in project.assets() {
        let media_path = if asset.path().is_absolute() {
            asset.path().to_path_buf()
        } else {
            project_dir.join(asset.path())
        };
        if asset.playback_frames() > remaining_frames {
            return Err("project media exceeds resident audio frame limit".into());
        }
        let decoded = decode_wav_file(
            media_path,
            Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT),
        )
        .map_err(|error| error.to_string().to_lowercase())?;
        if decoded.sample_rate() != asset.source_sample_rate()
            || u64::try_from(decoded.frames()).ok() != Some(asset.source_frames())
        {
            return Err(format!(
                "media {} rate or frame count differs from project metadata",
                asset.path().display()
            ));
        }
        let prepared = PreparedImport::at_rate(
            decoded,
            project.sample_rate(),
            ResampleLimits {
                max_source_frames: IMPORT_FRAME_LIMIT,
                max_output_frames: usize::try_from(remaining_frames)
                    .map_err(|_| "resident frame limit exceeds addressable range".to_owned())?,
            },
        )
        .map_err(|error| error.to_string().to_lowercase())?;
        remaining_frames = remaining_frames
            .checked_sub(asset.playback_frames())
            .ok_or_else(|| "project media exceeds resident audio frame limit".to_owned())?;
        prepared_assets.push((asset.id(), prepared));
    }
    let session = Session::from_prepared_with_sine_table(
        project,
        history_capacity,
        current.max_resident_frames(),
        prepared_assets,
        current.sine_table(),
    )
    .map_err(|error| error.to_string().to_lowercase())?;
    Ok((
        session.with_shutdown(Arc::clone(&current.shutdown)),
        project_dir,
        document,
        dirty,
    ))
}

pub(super) fn checkpoint_project(
    session: &Session,
    base: Option<&Path>,
    document: Option<&DocumentHandle>,
    untitled_path: &Path,
    journal: &mut Option<hex_storage::Journal>,
) -> Result<(), String> {
    let path = match document {
        Some(handle) => hex_storage::journal_path(&handle.path).map_err(|e| e.to_string())?,
        None => untitled_path.to_path_buf(),
    };
    if journal
        .as_ref()
        .is_none_or(|current| current.path() != path)
    {
        if document.is_none() {
            if let Some(parent) = path.parent() {
                match std::fs::symlink_metadata(parent) {
                    Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                        return Err("recovery directory must be a real directory".into());
                    }
                    Err(error) if error.kind() != io::ErrorKind::NotFound => {
                        return Err(error.to_string());
                    }
                    _ => {}
                }
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }
        *journal = Some(hex_storage::Journal::open(path).map_err(|e| e.to_string())?);
    }
    let project = project_for_save(session, base)?;
    if let Some(journal) = journal {
        let result = if document.is_none() {
            journal.checkpoint_latest(&project)
        } else {
            journal.checkpoint(&project)
        };
        result.map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub(super) fn absolute_document_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .map_err(|error| error.to_string().to_lowercase())
    }
}

fn absolute_document_base(path: &Path) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if parent.is_absolute() {
        Ok(parent.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(parent))
            .map_err(|error| error.to_string().to_lowercase())
    }
}

pub(super) fn project_for_save(
    session: &Session,
    document_base: Option<&Path>,
) -> Result<hex_project::Project, String> {
    let mut snapshot = session.project().snapshot();
    for asset in &mut snapshot.assets {
        if !asset.path.is_absolute() {
            let base = document_base.ok_or_else(|| {
                "relative media path has no project directory; refusing ambiguous save".to_owned()
            })?;
            asset.path = base.join(&asset.path);
        }
    }
    hex_project::Project::from_snapshot(snapshot).map_err(|error| error.to_string().to_lowercase())
}
