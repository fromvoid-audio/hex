//! Explicit recovery and media relinking, prepared before canonical adoption.
use super::*;
use std::mem;
mod checkpoint;
pub(super) use checkpoint::{RecoveryContext, RecoveryTracker};
pub use checkpoint::{RecoveryFailure, RecoveryRestart};

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

pub(super) type OpenCandidate = (Session, PathBuf, Option<DocumentHandle>, bool);

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

#[allow(clippy::too_many_arguments)]
pub(super) fn open_source_with_limits(
    current: &Session,
    history_capacity: usize,
    source: &ProjectSource,
    relinks: &BTreeMap<hex_project::AssetId, PathBuf>,
    mode: DocumentAudioMode,
    reservation: Option<SourcePlanReservation>,
    native: Option<crate::ResidentNativeReservation>,
    frame_allowance: u64,
    admission: sampler_export::Admission<'_>,
) -> Result<OpenCandidate, String> {
    let (mut project, project_dir, document) = read_source(source)?;
    let private_sampler =
        hex_engine::native_device_resources::requires_private_sampler_sources(&project);
    if private_sampler
        && project
            .tracks()
            .iter()
            .filter_map(|t| t.instrument())
            .filter(|d| d.state().type_id().as_str() == Ok("hex.instrument.sampler"))
            .flat_map(|d| d.state().asset_references())
            .any(|reference| {
                !project.assets().any(|a| {
                    a.id().value() == reference.asset.value() && a.original_source().is_some()
                })
            })
    {
        return Err("sampler reopening requires qualified original source reconstruction and saved content identity".into());
    }
    let has_originals = project.assets().any(|a| a.original_source().is_some());
    if (has_originals || private_sampler)
        && (mode != DocumentAudioMode::Resident || native.is_none())
    {
        return Err("device-original reopening requires resident native limits".into());
    }
    if current.shutdown.load(Ordering::Acquire) {
        return Err("project open cancelled".into());
    }
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
    if mode == DocumentAudioMode::Resident
        && project.sample_rate() != current.project().sample_rate()
    {
        return Err("project sample rate differs from the active audio device".into());
    }
    if mode == DocumentAudioMode::VerifiedStreaming {
        let reservation = reservation
            .ok_or("verified source reservation is required")?
            .validate()
            .map_err(|e| e.to_string())?;
        let mut candidate = Session::new_with_audio_mode_in_overview_domain(
            project.sample_rate(),
            history_capacity,
            current.max_resident_frames(),
            mode,
            Some(reservation),
            current.overview_budget.clone(),
        )
        .and_then(|candidate| match native {
            Some(limits) => candidate.with_native_reservation(limits),
            None => Ok(candidate),
        })
        .map_err(|e| e.to_string())?
        .with_shutdown(Arc::clone(&current.shutdown));
        // Verify every retained asset, including unused records, without decoding PCM.
        for asset in project.assets() {
            let path = if asset.path().is_absolute() {
                asset.path().to_path_buf()
            } else {
                project_dir.join(asset.path())
            };
            let source = candidate.verify_import(path).map_err(|e| e.to_string())?;
            if !source.matches_asset(asset, project.sample_rate()) {
                return Err("verified media differs from project metadata".into());
            }
            candidate.verified_sources.insert(asset.id(), source);
        }
        candidate.editor =
            hex_project::Editor::new(project, history_capacity).map_err(|e| e.to_string())?;
        candidate.sine_table = current.sine_table();
        return Ok((candidate, project_dir, document, dirty));
    }
    let mut prepared_assets = Vec::new();
    prepared_assets
        .try_reserve_exact(project.assets().count())
        .map_err(|_| "could not reserve decoded media table".to_owned())?;
    let mut originals = hex_engine::native_device_resources::ResidentDeviceAssets::default();
    let mut remaining_frames = frame_allowance;
    // Load unreferenced assets too: the editor can place them later, so missing
    // media in any asset record rejects the candidate instead of dropping it.
    for asset in project.assets() {
        let media_path = if asset.path().is_absolute() {
            asset.path().to_path_buf()
        } else {
            project_dir.join(asset.path())
        };
        if current.shutdown.load(Ordering::Acquire) {
            return Err("project open cancelled".into());
        }
        let required_frames = if asset.original_source().is_some() {
            asset.source_frames()
        } else {
            asset.playback_frames()
        };
        if required_frames > remaining_frames {
            return Err("project media exceeds resident audio frame limit".into());
        }
        if has_originals {
            admission
                .check_recovery_sources(
                    current,
                    sampler_export::RecoverySourceStage {
                        originals: Some(&originals),
                        staged: None,
                        prepared: &prepared_assets,
                        decoded: None,
                        additional_frames: asset.source_frames(),
                    },
                )
                .map_err(|e| e.to_string())?;
        }
        let source_limit = if has_originals {
            usize::try_from(asset.source_frames().min(remaining_frames))
                .map_err(|_| "saved source frame count exceeds platform limit")?
                .min(IMPORT_FRAME_LIMIT)
        } else {
            IMPORT_FRAME_LIMIT
        };
        let decoded = decode_wav_file(media_path, Limits::new(IMPORT_FILE_LIMIT, source_limit))
            .map_err(|error| error.to_string().to_lowercase())?;
        if decoded.sample_rate() != asset.source_sample_rate()
            || u64::try_from(decoded.frames()).ok() != Some(asset.source_frames())
        {
            return Err(format!(
                "media {} rate or frame count differs from project metadata",
                asset.path().display()
            ));
        }
        if let Some(expected) = asset.original_source() {
            let limits =
                native.ok_or("device-original reopening requires resident native limits")?;
            let frames = u64::try_from(decoded.audio().pcm_capacity_frames())
                .map_err(|_| "original source capacity exceeds frame range")?;
            if frames > remaining_frames {
                return Err("project original media exceeds resident frame limit".into());
            }
            let source = hex_engine::OwnedAudioSource::try_from_decoded_audio_on_control(
                decoded.into_audio(),
                limits.max_scan_frames,
                &current.shutdown,
            )
            .map_err(|r| SessionError::from(r.reason).to_string())?;
            let legacy_frames = prepared_assets
                .iter()
                .try_fold(0_u64, |n, (_, m)| {
                    n.checked_add(m.retained_pcm_frames()?)
                        .ok_or(SessionError::Invalid("recovery legacy frame overflow"))
                })
                .map_err(|e| e.to_string())?;
            let legacy_bytes = usize::try_from(legacy_frames)
                .ok()
                .and_then(|n| n.checked_mul(mem::size_of::<hex_engine::Frame>()))
                .ok_or("recovery legacy byte count overflow")?;
            originals
                .insert_checked_on_control(
                    (asset.id(), source),
                    legacy_bytes,
                    limits.storage,
                    limits.max_scan_frames,
                    &current.shutdown,
                    |stage| -> Result<(), SessionError> {
                        if stage.original_source_facts_on_control(asset.id())? != expected {
                            return Err(SessionError::Invalid(
                                "original media content differs from saved source identity",
                            ));
                        }
                        admission.check_recovery_sources(
                            current,
                            sampler_export::RecoverySourceStage {
                                originals: None,
                                staged: Some(stage),
                                prepared: &prepared_assets,
                                decoded: None,
                                additional_frames: 0,
                            },
                        )
                    },
                )
                .map_err(|e| format!("could not reopen {}: {e}", asset.path().display()))?;
            remaining_frames = remaining_frames
                .checked_sub(frames)
                .ok_or("resident original frame allowance overflow")?;
            continue;
        }
        if has_originals {
            admission
                .check_recovery_sources(
                    current,
                    sampler_export::RecoverySourceStage {
                        originals: Some(&originals),
                        staged: None,
                        prepared: &prepared_assets,
                        decoded: Some(decoded.audio()),
                        additional_frames: if decoded.sample_rate() != project.sample_rate() {
                            asset.playback_frames()
                        } else {
                            0
                        },
                    },
                )
                .map_err(|e| e.to_string())?;
        }
        let output_allowance = if native.is_some() && decoded.sample_rate() != project.sample_rate()
        {
            let original = u64::try_from(decoded.audio().pcm_capacity_frames())
                .map_err(|_| "source capacity exceeds frame range")?;
            remaining_frames
                .checked_sub(original)
                .filter(|n| *n >= asset.playback_frames())
                .ok_or("project source and derivative exceed resident reservation")?
        } else {
            remaining_frames
        };
        let prepared = PreparedImport::at_rate(
            decoded,
            project.sample_rate(),
            ResampleLimits {
                max_source_frames: IMPORT_FRAME_LIMIT,
                max_output_frames: usize::try_from(output_allowance)
                    .map_err(|_| "resident frame limit exceeds addressable range".to_owned())?,
            },
        )
        .map_err(|error| error.to_string().to_lowercase())?;
        remaining_frames = remaining_frames
            .checked_sub(
                prepared
                    .retained_pcm_frames()
                    .map_err(|error| error.to_string())?,
            )
            .ok_or_else(|| "project media exceeds resident audio frame limit".to_owned())?;
        prepared_assets.push((asset.id(), prepared));
        if has_originals {
            admission
                .check_recovery_sources(
                    current,
                    sampler_export::RecoverySourceStage {
                        originals: Some(&originals),
                        staged: None,
                        prepared: &prepared_assets,
                        decoded: None,
                        additional_frames: 0,
                    },
                )
                .map_err(|e| e.to_string())?;
        }
    }
    if current.shutdown.load(Ordering::Acquire) {
        return Err("project open cancelled".into());
    }
    let session = if has_originals {
        Session::from_prepared_with_original_sources(
            project,
            history_capacity,
            current.max_resident_frames(),
            prepared_assets,
            current.sine_table(),
            native,
            crate::native_assets::ReconstructedOriginalSources {
                assets: originals,
                overview_budget: current.overview_budget.clone(),
                cancel: &current.shutdown,
            },
        )
    } else {
        Session::from_prepared_with_native_limits_in_overview_domain(
            project,
            history_capacity,
            current.max_resident_frames(),
            prepared_assets,
            current.sine_table(),
            native,
            current.overview_budget.clone(),
        )
    }
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
    let path = checkpoint_path(document, untitled_path)?;
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
    for (metadata, asset) in session.project().assets().zip(&mut snapshot.assets) {
        if metadata.id().value() != asset.id {
            return Err("save source inventory differs from canonical project".into());
        }
        if session.native_assets.contains(metadata.id()) {
            let source = session
                .native_source(metadata.id())
                .ok_or("native original source is unavailable for saving")?;
            if source.sample_rate() != asset.source_sample_rate
                || source
                    .resident_frames()
                    .and_then(|pcm| u64::try_from(pcm.len()).ok())
                    != Some(asset.source_frames)
            {
                return Err("native original source differs from canonical media metadata".into());
            }
            let facts = session
                .native_assets
                .original_source_facts_on_control(metadata.id())
                .map_err(|e| SessionError::from(e).to_string())?;
            if asset
                .original_source
                .is_some_and(|expected| expected != facts)
            {
                return Err("native original content differs from saved source identity".into());
            }
            asset.original_source = Some(facts);
        } else if asset.original_source.is_some() {
            return Err("saved device-original media has no admitted native source".into());
        }
        if !asset.path.is_absolute() {
            let base = document_base.ok_or_else(|| {
                "relative media path has no project directory; refusing ambiguous save".to_owned()
            })?;
            asset.path = base.join(&asset.path);
        }
    }
    hex_project::Project::from_snapshot(snapshot).map_err(|error| error.to_string().to_lowercase())
}

fn checkpoint_path(document: Option<&DocumentHandle>, untitled: &Path) -> Result<PathBuf, String> {
    match document {
        Some(handle) => hex_storage::journal_path(&handle.path).map_err(|e| e.to_string()),
        None => Ok(untitled.to_path_buf()),
    }
}
