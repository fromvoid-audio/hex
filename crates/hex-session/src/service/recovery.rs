//! Explicit recovery and media relinking, prepared before canonical adoption.
use super::*;
use std::mem;
mod checkpoint;
mod checkpoint_worker;
pub(super) use checkpoint::{RecoveryContext, RecoveryTracker};
pub use checkpoint::{RecoveryFailure, RecoveryRestart};

/// A test-only, bounded hold of one private recovery checkpoint.
#[cfg(feature = "recovery-checkpoint-test-support")]
pub struct HeldCheckpoint {
    path: PathBuf,
    entered: std::sync::mpsc::Receiver<()>,
    has_entered: bool,
    release: Option<std::sync::mpsc::SyncSender<()>>,
}

#[cfg(feature = "recovery-checkpoint-test-support")]
static HELD_CHECKPOINT_PATHS: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

#[cfg(feature = "recovery-checkpoint-test-support")]
impl HeldCheckpoint {
    pub fn try_entered(&mut self) -> Result<(), std::sync::mpsc::TryRecvError> {
        self.entered.try_recv()?;
        self.has_entered = true;
        Ok(())
    }

    /// A disconnected receiver means the bounded hold timed out or was removed.
    pub fn release(mut self) -> Result<(), std::sync::mpsc::TrySendError<()>> {
        if !self.has_entered {
            return Err(std::sync::mpsc::TrySendError::Disconnected(()));
        }
        match self.release.take() {
            Some(release) => release.try_send(()),
            None => Err(std::sync::mpsc::TrySendError::Disconnected(())),
        }
    }
}

#[cfg(feature = "recovery-checkpoint-test-support")]
impl Drop for HeldCheckpoint {
    fn drop(&mut self) {
        let mut paths = HELD_CHECKPOINT_PATHS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut gates = checkpoint::CHECKPOINT_GATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        gates.retain(|(path, _, _)| path != &self.path);
        paths.retain(|path| path != &self.path);
    }
}

/// Register one uniquely scoped checkpoint hold; no default build exposes it.
#[cfg(feature = "recovery-checkpoint-test-support")]
pub fn hold_next_checkpoint(path: PathBuf) -> Result<HeldCheckpoint, &'static str> {
    let mut paths = HELD_CHECKPOINT_PATHS
        .lock()
        .map_err(|_| "checkpoint test paths unavailable")?;
    let mut gates = checkpoint::CHECKPOINT_GATE
        .lock()
        .map_err(|_| "checkpoint test gates unavailable")?;
    if paths.len() >= 8
        || gates.len() >= 8
        || paths.contains(&path)
        || gates.iter().any(|(existing, _, _)| existing == &path)
    {
        return Err("checkpoint test gate capacity or path conflict");
    }
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    gates.push((path.clone(), entered_tx, release_rx));
    paths.push(path.clone());
    Ok(HeldCheckpoint {
        path,
        entered,
        has_entered: false,
        release: Some(release),
    })
}

#[cfg(test)]
pub(super) fn install_checkpoint_gate(
    path: PathBuf,
    entered: std::sync::mpsc::SyncSender<()>,
    release: std::sync::mpsc::Receiver<()>,
) {
    checkpoint::CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path, entered, release));
}

/// Explicit source selection. Recovery never adopts an overwrite association.
#[derive(Clone, Debug)]
pub enum ProjectSource {
    Saved(PathBuf),
    Journal(PathBuf),
    SavedJournal(PathBuf),
    PreviousSave(PathBuf),
    /// Recover the retained source checkpoint; recorded events remain an independent draft.
    /// This source never acquires the original saved document's overwrite association.
    AutomationDraft {
        path: PathBuf,
        limits: hex_storage::automation_draft::DraftReadLimits,
    },
}

/// Slot 0 of the native application's untitled recovery directory, shared by
/// development and review bundles; its parent anchors derived media folders.
/// Running instances each claim their own slot with
/// [`claim_untitled_recovery_slot`]. No fallback to another user's directory
/// when HOME is unavailable.
pub fn untitled_recovery_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("home directory is unavailable")?;
    Ok(PathBuf::from(home).join("Library/Application Support/hex/recovery/untitled.hex-journal"))
}

/// Claim this instance's own untitled recovery slot so concurrent hex
/// instances never checkpoint over each other's untitled work (Pulse #3948).
pub fn claim_untitled_recovery_slot() -> Result<hex_storage::UntitledSlotClaim, String> {
    let slot = untitled_recovery_path()?;
    let directory = slot.parent().ok_or("recovery directory is unavailable")?;
    claim_untitled_slot_in(directory)
}

/// Create a missing recovery directory (refusing a symbolic link or file in its
/// place, as checkpointing does) and claim an unowned slot inside it.
fn claim_untitled_slot_in(directory: &Path) -> Result<hex_storage::UntitledSlotClaim, String> {
    match std::fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err("recovery directory must be a real directory".into());
        }
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            return Err(hex_storage::text::lowercase_outside_verbatim(
                &error.to_string(),
            ));
        }
        _ => {}
    }
    std::fs::create_dir_all(directory)
        .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))?;
    hex_storage::UntitledSlotClaim::claim(directory).map_err(|error| error.to_string())
}

/// A readable journal left by a session that ended without saving or discarding
/// it (Pulse #3990). Detection is read-only: nothing is adopted, written or
/// checkpointed, so declining leaves the slot to the copy-on-first-edit rule.
#[derive(Clone, Debug)]
pub struct LeftoverRecovery {
    /// Explicit recovery source for the existing open path.
    pub source: ProjectSource,
    pub journal: PathBuf,
    fingerprint: (u64, Option<std::time::SystemTime>),
    /// Held while another ended instance's slot is offered (Pulse #4575), so
    /// no other launching instance adopts or offers it meanwhile.
    foreign: Option<std::sync::Arc<hex_storage::UntitledSlotClaim>>,
}
impl LeftoverRecovery {
    /// False once anything rewrote the slot after detection, for example this
    /// session's own first checkpoint; recovering then would open the wrong work.
    pub fn unchanged(&self) -> bool {
        fingerprint(&self.journal).is_some_and(|current| current == self.fingerprint)
    }

    /// The source to open when the user chooses recover. This instance's own
    /// slot opens in place. A slot left by another ended instance is first set
    /// aside into kept copies (never deleted) and the kept copy is opened, so
    /// its work is not offered again; it stays reachable through
    /// `kept_journal` if opening fails.
    pub fn recover_source(&self) -> Result<ProjectSource, String> {
        if self.foreign.is_none() {
            return Ok(self.source.clone());
        }
        self.discard().map(ProjectSource::Journal)
    }

    /// Explicit, confirmed discard: the journal moves into a kept-copies folder
    /// beside its slot and stops being offered. Nothing recoverable is deleted.
    /// Returns the kept copy, which `kept_journal` can open later.
    pub fn discard(&self) -> Result<PathBuf, String> {
        if !self.unchanged() {
            return Err("recovery journal changed since it was found".into());
        }
        hex_storage::Journal::open(self.journal.clone())
            .and_then(|mut journal| journal.set_aside())
            .map_err(|error| error.to_string())
    }
}

/// Resolve a kept recovery copy picked by the user: a `.hex-journal` file, or a
/// kept-copies folder holding exactly one (its file may be a hidden dot-file).
pub fn kept_journal(picked: &Path) -> Result<ProjectSource, String> {
    let journal = if picked.is_dir() {
        let mut found = std::fs::read_dir(picked)
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && path.to_string_lossy().ends_with(".hex-journal"));
        match (found.next(), found.next()) {
            (Some(path), None) => path,
            _ => return Err("choose a kept copy folder holding one recovery journal".into()),
        }
    } else {
        picked.to_path_buf()
    };
    hex_storage::load_journal(&journal).map_err(|error| error.to_string())?;
    Ok(ProjectSource::Journal(journal))
}

fn fingerprint(path: &Path) -> Option<(u64, Option<std::time::SystemTime>)> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    metadata
        .is_file()
        .then(|| (metadata.len(), metadata.modified().ok()))
}

/// Find a readable leftover journal for the untitled slot (`document` None) or
/// beside a saved `document`. Missing, unreadable or unrecognized slots return
/// None: an unreadable journal stays with the existing failure/restart path.
pub fn leftover_recovery(untitled: &Path, document: Option<&Path>) -> Option<LeftoverRecovery> {
    let (journal, source) = match document {
        Some(document) => (
            hex_storage::journal_path(document).ok()?,
            ProjectSource::SavedJournal(document.to_path_buf()),
        ),
        None => (
            untitled.to_path_buf(),
            ProjectSource::Journal(untitled.to_path_buf()),
        ),
    };
    let before = fingerprint(&journal)?;
    hex_storage::load_journal(&journal).ok()?;
    (fingerprint(&journal)? == before).then_some(LeftoverRecovery {
        source,
        journal,
        fingerprint: before,
        foreign: None,
    })
}

/// Every readable untitled leftover a launch should offer (Pulse #4575): this
/// instance's own slot first, then each slot other ended instances left in the
/// same directory. Live-owned slots are never included; foreign leftovers stay
/// claimed by the returned offers until they are dropped. Unreadable foreign
/// journals are released untouched.
pub fn untitled_leftovers(own: &Path) -> Vec<LeftoverRecovery> {
    let mut found: Vec<_> = leftover_recovery(own, None).into_iter().collect();
    if let Some(directory) = own.parent() {
        found.extend(
            hex_storage::UntitledSlotClaim::claim_leftovers(directory, own)
                .into_iter()
                .filter_map(|claim| {
                    let mut leftover = leftover_recovery(claim.path(), None)?;
                    leftover.foreign = Some(std::sync::Arc::new(claim));
                    Some(leftover)
                }),
        );
    }
    found
}

pub(super) type OpenCandidate = (Session, PathBuf, Option<DocumentHandle>, bool);

pub(super) fn read_source(
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
        ProjectSource::AutomationDraft { path, limits } => {
            let reader = hex_storage::automation_draft::DraftReader::open(path, *limits)
                .map_err(|error| error.to_string())?;
            (reader.source().clone(), path, None)
        }
    };
    Ok((project, absolute_document_base(path)?, document))
}

fn validate_sampler_source_identity(project: &Project) -> Result<bool, String> {
    let private_sampler =
        hex_engine::native_device_resources::requires_private_sampler_sources(project);
    if private_sampler
        && project
            .tracks()
            .iter()
            .filter_map(|track| track.instrument())
            .filter(|device| device.state().type_id().as_str() == Ok("hex.instrument.sampler"))
            .flat_map(|device| device.state().asset_references())
            .any(|reference| {
                !project.assets().any(|asset| {
                    asset.id().value() == reference.asset.value()
                        && asset.original_source().is_some()
                })
            })
    {
        return Err("sampler reopening requires qualified original source reconstruction and saved content identity".into());
    }
    Ok(private_sampler)
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
    // Validate saved sampler identity before inspecting its media. The prepare
    // boundary repeats this check because other callers enter it directly.
    let private_sampler = validate_sampler_source_identity(&project)?;
    if mode == DocumentAudioMode::VerifiedStreaming
        && (private_sampler
            || project
                .assets()
                .any(|asset| asset.original_source().is_some()))
    {
        return Err("device-original verified opening is not yet available".into());
    }
    project = with_missing_media_offline(project, &project_dir)?;
    let session = prepare_project(
        current,
        history_capacity,
        project,
        &project_dir,
        mode,
        reservation,
        native,
        frame_allowance,
        admission,
    )?;
    Ok((session, project_dir, document, dirty))
}

/// Rebuild a validated project from its original sources under the same
/// reservations as opening. The caller retains ownership of document identity.
#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_project(
    current: &Session,
    history_capacity: usize,
    project: Project,
    project_dir: &Path,
    mode: DocumentAudioMode,
    reservation: Option<SourcePlanReservation>,
    native: Option<crate::ResidentNativeReservation>,
    frame_allowance: u64,
    admission: sampler_export::Admission<'_>,
) -> Result<Session, String> {
    let private_sampler = validate_sampler_source_identity(&project)?;
    let has_originals = project.assets().any(|a| a.original_source().is_some());
    if (has_originals || private_sampler) && native.is_none() {
        return Err("device-original reopening requires resident native limits".into());
    }
    if current.shutdown.load(Ordering::Acquire) {
        return Err("project open cancelled".into());
    }

    let reservation = match mode {
        DocumentAudioMode::Resident => reservation.or(current.source_plan_reservation()),
        DocumentAudioMode::VerifiedStreaming => reservation,
    };
    let required_pcm = online_assets(&project).try_fold(0u64, |total, asset| {
        let frames = if asset.source_sample_rate() == project.sample_rate() {
            asset.playback_frames()
        } else {
            asset
                .source_frames()
                .checked_add(asset.playback_frames())
                .ok_or("project pcm frame count overflow")?
        };
        total
            .checked_add(frames)
            .ok_or("project pcm frame count overflow")
    })?;
    // The fresh output verifies the candidate's factual file inputs and
    // delivers the real page endpoint before queue submission. File facts
    // alone never promise page readiness or callback application.
    // A plain asset past the per-file resident decode caps was imported as a
    // streamed file; reopening must keep it file-backed even when the total
    // working set would otherwise fit (Pulse #3949).
    let oversized_plain_asset = online_assets(&project).any(|asset| {
        asset.original_source().is_none()
            && !asset_requires_resident_processing(&project, asset.id())
            && crate::verified_import::exceeds_resident_decode(
                &asset_media_path(project_dir, asset),
                asset.source_frames(),
            )
            .unwrap_or(false)
    });
    let ordinary_file_backed = mode == DocumentAudioMode::Resident
        && reservation.is_some()
        && (required_pcm > frame_allowance || oversized_plain_asset);
    if mode == DocumentAudioMode::VerifiedStreaming && !has_originals && !private_sampler {
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
        for asset in online_assets(&project) {
            let path = asset_media_path(project_dir, asset);
            let source = candidate
                .verify_asset(path, asset)
                .map_err(|e| e.to_string())?;
            if !source.matches_asset(asset, project.sample_rate()) {
                return Err("verified media differs from project metadata".into());
            }
            candidate.verified_sources.insert(asset.id(), source);
        }
        candidate.editor =
            hex_project::Editor::new(project, history_capacity).map_err(|e| e.to_string())?;
        candidate.sine_table = current.sine_table();
        return Ok(candidate);
    }
    let mut file_candidate = if ordinary_file_backed || mode == DocumentAudioMode::VerifiedStreaming
    {
        Some(
            Session::new_with_audio_mode_in_overview_domain(
                project.sample_rate(),
                history_capacity,
                current.max_resident_frames(),
                mode,
                reservation,
                current.overview_budget.clone(),
            )
            .map_err(|e| e.to_string())?
            .with_shutdown(Arc::clone(&current.shutdown)),
        )
    } else {
        None
    };
    let mut prepared_assets = Vec::new();
    prepared_assets
        .try_reserve_exact(project.assets().count())
        .map_err(|_| "could not reserve decoded media table".to_owned())?;
    let mut originals = hex_engine::native_device_resources::ResidentDeviceAssets::default();
    let mut remaining_frames = frame_allowance;
    // Load unreferenced assets too: the editor can place them later. Missing
    // media is either offline (recorded on the project) or rejects the candidate.
    for asset in online_assets(&project) {
        let media_path = asset_media_path(project_dir, asset);
        if current.shutdown.load(Ordering::Acquire) {
            return Err("project open cancelled".into());
        }
        if asset.original_source().is_none()
            && !asset_requires_resident_processing(&project, asset.id())
            && let Some(candidate) = file_candidate.as_mut()
        {
            let file = candidate
                .verify_asset(media_path, asset)
                .map_err(|e| e.to_string())?;
            if !file.matches_asset(asset, project.sample_rate()) {
                return Err("verified media differs from project metadata".into());
            }
            candidate.verified_sources.insert(asset.id(), file);
            continue;
        }
        if mode == DocumentAudioMode::VerifiedStreaming && asset.original_source().is_none() {
            return Err("verified recovery does not support resident clip processing".into());
        }
        if mode == DocumentAudioMode::VerifiedStreaming
            && (project.tracks().iter().any(|track| {
                track.clips().iter().any(|clip| {
                    clip.referenced_assets().any(|referenced| referenced == asset.id())
                })
            }) || project.session_cells().iter().any(|cell| {
                matches!(cell.content(), hex_project::SessionCellContent::Audio(clip) if clip.asset() == asset.id())
            }))
        {
            return Err("verified recovery cannot use device originals as clip sources".into());
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
            .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))?;
        if decoded.sample_rate() != asset.source_sample_rate()
            || u64::try_from(decoded.frames()).ok() != Some(asset.source_frames())
        {
            return Err(format!(
                "media {} rate or frame count differs from project metadata",
                hex_storage::text::verbatim(asset.path().display())
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
                .map_err(|e| {
                    format!(
                        "could not reopen {}: {e}",
                        hex_storage::text::verbatim(asset.path().display())
                    )
                })?;
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
        let prepared = PreparedImport::for_conversion(
            decoded,
            project.sample_rate(),
            ResampleLimits {
                max_source_frames: IMPORT_FRAME_LIMIT,
                max_output_frames: usize::try_from(output_allowance)
                    .map_err(|_| "resident frame limit exceeds addressable range".to_owned())?,
            },
            asset.conversion_version(),
        )
        .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))?;
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
    let files = if let Some(candidate) = file_candidate {
        Some(crate::audio_sources::ReconstructedFileSources {
            reservation: candidate
                .source_reservation
                .ok_or("file recovery reservation missing")?,
            files: candidate.verified_sources,
        })
    } else {
        None
    };
    let mut session = if has_originals || files.is_some() {
        Session::from_prepared_with_original_and_file_sources(
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
            files,
            mode,
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
    .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))?;
    session.source_reservation = reservation
        .map(SourcePlanReservation::validate)
        .transpose()
        .map_err(|e| e.to_string())?;
    if session.source_reservation.is_some() && session.semantic_source_epoch == 0 {
        session.semantic_source_epoch =
            crate::audio_sources::fresh_source_epoch().map_err(|e| e.to_string())?;
    }
    Ok(session.with_shutdown(Arc::clone(&current.shutdown)))
}

fn asset_media_path(project_dir: &Path, asset: &hex_project::Asset) -> PathBuf {
    if asset.path().is_absolute() {
        asset.path().to_path_buf()
    } else {
        project_dir.join(asset.path())
    }
}

fn online_assets(project: &Project) -> impl Iterator<Item = &hex_project::Asset> {
    project
        .assets()
        .filter(|asset| !project.asset_offline(asset.id()))
}

/// Mark ordinary assets whose files are absent as offline so the project opens
/// with their clips silent; nothing is dropped and paths are kept. Missing
/// device or Session-grid media is still refused (see
/// [`Project::with_offline_assets`]), and an existing path that cannot be
/// inspected is left for the loader to report.
fn with_missing_media_offline(project: Project, base: &Path) -> Result<Project, String> {
    let missing: Vec<_> = project
        .assets()
        .filter(|asset| {
            let path = if asset.path().is_absolute() {
                asset.path().to_path_buf()
            } else {
                base.join(asset.path())
            };
            matches!(std::fs::metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
        })
        .map(|asset| asset.id())
        .collect();
    if missing.is_empty() {
        return Ok(project);
    }
    project
        .with_offline_assets(missing)
        .map_err(|error| format!("missing media: {error}"))
}

/// Keep materialized processing dependencies resident, including retained
/// grouped roots. Complete plain-pitch captures can use verified worker pages;
/// other edits still require their existing resident preparation path.
fn asset_requires_resident_processing(project: &Project, asset: hex_project::AssetId) -> bool {
    let requires = |clip: &hex_project::Clip| {
        (clip.asset() == asset
            && crate::audio_sources::plain_pitch_requirement(project, clip).is_none()
            && hex_engine::audio_transform::AudioTransformPreparation::clip_requires_resident_source(clip))
            || clip.audio_edit().and_then(|edit| edit.domain.events.as_ref())
                .is_some_and(|events| events.sources.iter().any(|root| root.clip.asset() == asset))
    };
    // Session audio cells contain only finite source spans and gain; their
    // current compiler supports those directly with verified pages.
    project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .any(requires)
}

/// A checkpoint's exact project and destination, captured on the service worker.
/// Publication owns the image and journal exclusively; capture is not durability.
pub(super) struct PreparedCheckpoint {
    path: PathBuf,
    untitled: bool,
    image: SaveImage,
}

impl PreparedCheckpoint {
    pub(super) fn capture(context: &RecoveryContext<'_>) -> Result<Self, String> {
        Ok(Self {
            path: checkpoint_path(context.document, context.untitled)?,
            untitled: context.document.is_none(),
            image: SaveImage::capture(context.session, context.base)?,
        })
    }

    pub(super) fn publish(
        self,
        journal: &mut Option<hex_storage::Journal>,
    ) -> Result<Option<PathBuf>, String> {
        let Self {
            path,
            untitled,
            image,
        } = self;
        #[cfg(any(test, feature = "recovery-checkpoint-test-support"))]
        checkpoint::wait_at_checkpoint(&path);
        let project = image.materialize()?;
        if journal
            .as_ref()
            .is_none_or(|current| current.path() != path)
        {
            if untitled && let Some(parent) = path.parent() {
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
            if let Some(token) = hex_storage::JournalRestartToken::capture_readable(path.clone())
                .map_err(|e| e.to_string())?
            {
                let outcome = hex_storage::restart_journal(&token, &project);
                *journal = outcome.journal;
                outcome
                    .result
                    .map_err(|error| match outcome.preserved_path.as_ref() {
                        Some(_) => format!(
                            "{error}; previous recovery journal preserved beside its original slot"
                        ),
                        None => error.to_string(),
                    })?;
                return Ok(outcome.preserved_path);
            }
            *journal = Some(hex_storage::Journal::open_empty(path).map_err(|e| e.to_string())?);
        }
        if let Some(journal) = journal {
            journal.checkpoint(&project).map_err(|e| e.to_string())?;
        }
        Ok(None)
    }
}

pub(super) fn absolute_document_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))
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
            .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))
    }
}

pub(super) fn project_for_save(
    session: &Session,
    document_base: Option<&Path>,
) -> Result<hex_project::Project, String> {
    SaveImage::capture(session, document_base)?.materialize()
}

/// Owned save input after control-side media custody and identity checks.
/// It contains no Session, native resource handle, or borrowed path and can be
/// transferred to a worker before validating/materializing the project.
pub(super) struct SaveImage {
    snapshot: hex_project::Snapshot,
}

impl SaveImage {
    pub(super) fn capture(session: &Session, document_base: Option<&Path>) -> Result<Self, String> {
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
                    return Err(
                        "native original source differs from canonical media metadata".into(),
                    );
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
                    "relative media path has no project directory; refusing ambiguous save"
                        .to_owned()
                })?;
                asset.path = base.join(&asset.path);
            }
        }
        Ok(Self { snapshot })
    }

    pub(super) fn materialize(self) -> Result<hex_project::Project, String> {
        hex_project::Project::from_snapshot(self.snapshot)
            .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))
    }
}

fn checkpoint_path(document: Option<&DocumentHandle>, untitled: &Path) -> Result<PathBuf, String> {
    match document {
        Some(handle) => hex_storage::journal_path(&handle.path).map_err(|e| e.to_string()),
        None => Ok(untitled.to_path_buf()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn ordinary_file_recovery_only_exempts_complete_plain_pitch() {
        let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let hex_project::Created::Imported { clip, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "clip".into(),
                path: "clip.wav".into(),
                sample_rate: 48_000,
                frames: 32,
            })
            .unwrap()
        else {
            panic!("fixture")
        };
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 700 })
            .unwrap();
        assert!(!asset_requires_resident_processing(editor.project(), asset));
        let mut cropped = hex_project::Editor::new(editor.project().clone(), 8).unwrap();
        let start = cropped.project().clip(clip).unwrap().start();
        cropped
            .execute(Command::TrimClipEdge {
                clip,
                edge: hex_project::TrimEdge::Right,
                frame: start + 16,
            })
            .unwrap();
        assert!(asset_requires_resident_processing(cropped.project(), asset));
        editor
            .execute(Command::SetAudioClipMusical {
                clip,
                enabled: true,
            })
            .unwrap();
        assert!(asset_requires_resident_processing(editor.project(), asset));
    }
    #[cfg(feature = "recovery-checkpoint-test-support")]
    #[test]
    fn test_checkpoint_gate_releases_registration_and_rejects_unentered_release() {
        let path = PathBuf::from("hex-test-checkpoint-gate-registration");
        let first = hold_next_checkpoint(path.clone()).unwrap();
        assert!(hold_next_checkpoint(path.clone()).is_err());
        drop(first);
        let second = hold_next_checkpoint(path.clone()).unwrap();
        assert!(second.release().is_err());
        assert!(hold_next_checkpoint(path).is_ok());
    }

    #[test]
    fn prepared_checkpoint_keeps_snapshot_and_journal_custody_when_moved() {
        let directory = scratch();
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("untitled.hex-journal");
        let mut session = Session::new(48_000, 8, 1024).unwrap();
        session
            .command(Command::AddTrack {
                name: "first".into(),
            })
            .unwrap();
        let expected = project_for_save(&session, None).unwrap();
        let mut journal = None;
        let checkpoint = PreparedCheckpoint::capture(&RecoveryContext {
            session: &session,
            base: None,
            document: None,
            untitled: &path,
            journal: &mut journal,
            generation: 1,
        })
        .unwrap();
        session
            .command(Command::AddTrack {
                name: "later".into(),
            })
            .unwrap();
        let mut journal = std::thread::spawn(move || {
            checkpoint.publish(&mut journal).unwrap();
            journal
        })
        .join()
        .unwrap();
        assert_eq!(hex_storage::load_journal(&path).unwrap(), expected);
        let checkpoint = PreparedCheckpoint::capture(&RecoveryContext {
            session: &session,
            base: None,
            document: None,
            untitled: &path,
            journal: &mut journal,
            generation: 1,
        })
        .unwrap();
        let external = b"external journal contents";
        std::fs::write(&path, external).unwrap();
        assert!(
            std::thread::spawn(move || checkpoint.publish(&mut journal))
                .join()
                .unwrap()
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), external);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn captured_save_image_moves_to_worker_and_keeps_its_original_revision() {
        let mut session = Session::new(48_000, 8, 1024).unwrap();
        session
            .command(Command::AddTrack {
                name: "first".into(),
            })
            .unwrap();
        let expected = project_for_save(&session, None).unwrap();
        let image = SaveImage::capture(&session, None).unwrap();
        session
            .command(Command::AddTrack {
                name: "later".into(),
            })
            .unwrap();
        let materialized = std::thread::spawn(move || image.materialize())
            .join()
            .unwrap()
            .unwrap();
        assert_eq!(materialized, expected);
        assert_eq!(
            hex_storage::encode(&materialized).unwrap(),
            hex_storage::encode(&expected).unwrap()
        );
        assert_ne!(materialized, *session.project());
    }

    #[test]
    fn captured_save_image_rebases_relative_offline_media_without_changing_session() {
        let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "samples/source.wav".into(),
                sample_rate: 48_000,
                frames: 8,
            })
            .unwrap()
        else {
            panic!("audio fixture");
        };
        let project = editor
            .project()
            .clone()
            .with_offline_assets([asset])
            .unwrap();
        let session = Session::from_loaded(project, 8, 1024, Vec::new()).unwrap();
        assert!(SaveImage::capture(&session, None).is_err());
        let base = std::env::temp_dir().join("hex-save-image-test");
        let image = SaveImage::capture(&session, Some(&base)).unwrap();
        let mut snapshot = session.project().snapshot();
        snapshot.assets[0].path = base.join("samples/source.wav");
        let expected = Project::from_snapshot(snapshot).unwrap();
        let materialized = std::thread::spawn(move || image.materialize())
            .join()
            .unwrap()
            .unwrap();
        assert_eq!(materialized, expected);
        assert_eq!(
            hex_storage::encode(&materialized).unwrap(),
            hex_storage::encode(&expected).unwrap()
        );
        assert_eq!(
            session.project().asset(asset).unwrap().path(),
            Path::new("samples/source.wav")
        );
        assert_eq!(session.project().offline_assets().count(), 1);
    }

    fn scratch() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "hex-untitled-claim-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// Regression for #3948: each instance gets its own untitled slot in a
    /// freshly created recovery directory, starting with the historical name.
    #[test]
    fn concurrent_claims_create_the_directory_and_never_share_a_slot() {
        let root = scratch();
        let directory = root.join("recovery");
        let first = claim_untitled_slot_in(&directory).unwrap();
        let second = claim_untitled_slot_in(&directory).unwrap();
        assert_eq!(first.path(), directory.join("untitled.hex-journal"));
        assert_eq!(second.path(), directory.join("untitled-2.hex-journal"));
        drop((first, second));
        let _ = std::fs::remove_dir_all(root);
    }

    fn leave(path: &Path, rate: u32) {
        hex_storage::Journal::open_empty(path.to_path_buf())
            .unwrap()
            .checkpoint(&Project::new(rate).unwrap())
            .unwrap();
    }

    /// Regression for #4575: one launch offers every unowned untitled
    /// leftover, never a live instance's slot, and recovering a foreign slot
    /// sets it aside so it is not offered again.
    #[test]
    fn launch_offers_every_unowned_leftover_and_sets_recovered_foreign_slots_aside() {
        let root = scratch();
        let directory = root.join("recovery");
        let live = claim_untitled_slot_in(&directory).unwrap();
        leave(live.path(), 48_000);
        let slot = |index| hex_storage::untitled_slot_path(&directory, index);
        leave(&slot(1), 44_100);
        leave(&slot(2), 96_000);
        std::fs::write(slot(3), b"not a journal").unwrap();
        let own = claim_untitled_slot_in(&directory).unwrap();
        assert_eq!(own.path(), slot(1), "own slot adopts the first leftover");

        let found = untitled_leftovers(own.path());
        let journals: Vec<_> = found.iter().map(|found| found.journal.clone()).collect();
        assert_eq!(journals, [slot(1), slot(2)]);
        assert!(found[0].foreign.is_none());
        assert!(
            matches!(found[0].recover_source(), Ok(ProjectSource::Journal(path)) if path == slot(1))
        );
        let Ok(ProjectSource::Journal(kept)) = found[1].recover_source() else {
            panic!("foreign leftover opens its kept copy");
        };
        assert!(!slot(2).exists(), "no longer offered");
        assert_eq!(
            hex_storage::load_journal(&kept).unwrap(),
            Project::new(96_000).unwrap()
        );
        assert_eq!(std::fs::read(slot(3)).unwrap(), b"not a journal");
        assert_eq!(
            hex_storage::load_journal(live.path()).unwrap(),
            Project::new(48_000).unwrap(),
            "live slot untouched"
        );
        drop(found);
        let again: Vec<_> = untitled_leftovers(own.path())
            .into_iter()
            .map(|found| found.journal)
            .collect();
        assert_eq!(again, [slot(1)]);
        drop((live, own));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_recovery_directory_is_refused_without_following_it() {
        let root = scratch();
        std::fs::create_dir(&root).unwrap();
        let target = root.join("elsewhere");
        std::fs::create_dir(&target).unwrap();
        let directory = root.join("recovery");
        std::os::unix::fs::symlink(&target, &directory).unwrap();
        let error = claim_untitled_slot_in(&directory).unwrap_err();
        assert_eq!(error, "recovery directory must be a real directory");
        assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(root);
    }
}
