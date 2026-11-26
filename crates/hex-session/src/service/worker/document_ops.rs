//! Document open/save/new, recovery, sample-rate, undo/redo, export and tuning-library worker operations.
use super::*;
use std::ops::ControlFlow;

mod committed_copy;

pub(crate) struct DocumentHandle {
    pub(crate) path: PathBuf,
    pub(crate) token: FileToken,
}

pub(crate) struct SaveCurrentCommit {
    pub(crate) dirty: bool,
    pub(crate) status: String,
    pub(crate) error: Option<String>,
}

/// Save As onto the open document would skip Save's format-upgrade and
/// older-reader consent checks, so it is refused; Save owns that replacement.
pub(crate) fn is_open_document(document: &DocumentHandle, destination: &Path) -> bool {
    document.path == destination || same_existing_file(&document.path, destination)
}

/// Save As: create `path`, or replace it when it already exists. The UI only
/// sends an existing path after the OS replace prompt for that exact path.
/// Both outcomes bind the document, including a published file whose
/// durability could not be confirmed, so the next Save replaces it in place.
pub(crate) fn save_document_as(
    path: PathBuf,
    project: &hex_project::Project,
) -> Result<(DocumentHandle, SaveCurrentCommit), String> {
    let lower = |error: hex_storage::StorageError| {
        hex_storage::text::lowercase_outside_verbatim(&error.to_string())
    };
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {
            let outcome = hex_storage::replace_confirmed(&path, project).map_err(lower)?;
            let mut handle = DocumentHandle {
                path,
                token: outcome.token.clone(),
            };
            let commit = apply_current_save_outcome(&mut handle, outcome);
            Ok((handle, commit))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let outcome = hex_storage::save_new_with_outcome(&path, project).map_err(lower)?;
            Ok(bind_new_save(outcome))
        }
        Err(error) => Err(hex_storage::text::lowercase_outside_verbatim(
            &error.to_string(),
        )),
    }
}

pub(crate) fn bind_new_save(
    outcome: hex_storage::SaveNewOutcome,
) -> (DocumentHandle, SaveCurrentCommit) {
    let commit = match outcome.warning {
        Some(warning) => SaveCurrentCommit {
            dirty: true,
            status: "project was published, but durability could not be confirmed".into(),
            error: Some(format!(
                "complete project was published at {}, but could not {}: {}",
                hex_storage::text::verbatim(outcome.path.display()),
                warning.operation,
                warning.source
            )),
        },
        None => SaveCurrentCommit {
            dirty: false,
            status: save_status::SAVED.into(),
            error: None,
        },
    };
    (
        DocumentHandle {
            path: outcome.path,
            token: outcome.token,
        },
        commit,
    )
}

/// Binds the replaced document and reports the save. A durability warning
/// keeps the document dirty with an error; lost file attributes only extend the
/// success status, because the project itself was saved.
pub(crate) fn apply_current_save_outcome(
    document: &mut DocumentHandle,
    outcome: hex_storage::ReplaceOutcome,
) -> SaveCurrentCommit {
    document.path = outcome.path.clone();
    document.token = outcome.token;
    match outcome.warning {
        Some(warning) => SaveCurrentCommit {
            dirty: true,
            status: "project was published, but durability could not be confirmed".into(),
            error: Some(format!(
                "complete project was published at {}, but could not {}: {}; recovery copy is at {}",
                hex_storage::text::verbatim(outcome.path.display()),
                warning.operation,
                warning.source,
                hex_storage::text::verbatim(outcome.recovery_path.display())
            )),
        },
        // Lost file attributes are non-fatal: the project is saved and clean.
        None => SaveCurrentCommit {
            dirty: false,
            status: save_status::saved_status(&outcome.metadata_warnings),
            error: None,
        },
    }
}

#[cfg(test)]
pub(crate) fn tuning_library_directory_from(home: &Path) -> PathBuf {
    hex_preferences::app_support_dir_from(home).join("scales")
}

/// Legacy macOS tuning preset location used prior to portable app-support paths.
/// Retained as a read-only fallback so existing presets remain visible and
/// loadable without mutating or migrating files on disk.
#[cfg(target_os = "macos")]
fn legacy_macos_tuning_library_directory_from(home: &Path) -> PathBuf {
    home.join("Library/Application Support/PORTALSURFER/hex/scales")
}

fn legacy_tuning_library_directory() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = recovery::user_home_directory()?;
        let legacy = legacy_macos_tuning_library_directory_from(&home);
        match std::fs::symlink_metadata(&legacy) {
            Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => Some(legacy),
            _ => None,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

pub(crate) fn tuning_library_directory() -> Result<PathBuf, String> {
    hex_preferences::app_support_dir()
        .map(|base| base.join("scales"))
        .map_err(|_| "home directory is unavailable for tuning presets".to_owned())
}

pub(crate) struct TuningLibraryContext {
    pub(crate) request_generation: u64,
    pub(crate) request_revision: u64,
    pub(crate) operation_id: u64,
    pub(crate) current_generation: u64,
    pub(crate) current_revision: u64,
}

pub(crate) fn run_tuning_library_operation(
    session: &mut Session,
    context: TuningLibraryContext,
    operation: TuningLibraryOperation,
    apply: impl FnMut(&mut Session, Command) -> Result<Arc<PreparedRevision>, SessionError>,
) -> (TuningLibraryResult, Option<Arc<PreparedRevision>>) {
    match tuning_library_directory() {
        Ok(directory) => {
            let legacy = legacy_tuning_library_directory();
            run_tuning_library_operation_with_fallback(
                session,
                context,
                operation,
                &directory,
                legacy.as_deref(),
                apply,
            )
        }
        Err(reason) => (
            TuningLibraryResult {
                operation_id: context.operation_id,
                slots: Vec::new(),
                selected_slot: None,
                loaded_name: None,
                error: Some(reason),
            },
            None,
        ),
    }
}

#[cfg(test)]
pub(crate) fn run_tuning_library_operation_in(
    session: &mut Session,
    context: TuningLibraryContext,
    operation: TuningLibraryOperation,
    previous: Option<&Arc<PreparedRevision>>,
    directory: &Path,
) -> (TuningLibraryResult, Option<Arc<PreparedRevision>>) {
    run_tuning_library_operation_in_using(
        session,
        context,
        operation,
        directory,
        |session, command| {
            session
                .command_prepared_revision(command, previous.map(Arc::as_ref))
                .map(|(_, prepared)| prepared)
        },
    )
}

/// Library I/O selects a validated tuning; the caller owns its existing reserved graph transaction.
#[cfg(test)]
pub(crate) fn run_tuning_library_operation_in_using(
    session: &mut Session,
    context: TuningLibraryContext,
    operation: TuningLibraryOperation,
    directory: &Path,
    apply: impl FnMut(&mut Session, Command) -> Result<Arc<PreparedRevision>, SessionError>,
) -> (TuningLibraryResult, Option<Arc<PreparedRevision>>) {
    run_tuning_library_operation_with_fallback(session, context, operation, directory, None, apply)
}

fn load_tuning_slot_with_fallback(
    directory: &Path,
    legacy_directory: Option<&Path>,
    slot: usize,
) -> Result<hex_tuning::preset::Preset, String> {
    match hex_storage::tuning_library::load(directory, slot) {
        Ok(preset) => Ok(preset),
        Err(error) => {
            let canonical_slot = directory.join(format!("slot-{slot}.freqs"));
            let canonical_missing = matches!(
                std::fs::symlink_metadata(&canonical_slot),
                Err(io_error) if io_error.kind() == std::io::ErrorKind::NotFound
            );
            if canonical_missing
                && let Some(legacy) = legacy_directory
                && let Ok(preset) = hex_storage::tuning_library::load(legacy, slot)
            {
                return Ok(preset);
            }
            Err(error)
        }
    }
}

fn list_tuning_slots_with_fallback(
    directory: &Path,
    legacy_directory: Option<&Path>,
) -> Vec<hex_storage::tuning_library::Slot> {
    let mut slots = hex_storage::tuning_library::list(directory);
    if let Some(legacy) = legacy_directory {
        let legacy_slots = hex_storage::tuning_library::list(legacy);
        for (slot, legacy_slot) in slots.iter_mut().zip(legacy_slots) {
            if slot.name.is_none()
                && slot.error.is_none()
                && (legacy_slot.name.is_some() || legacy_slot.error.is_some())
            {
                *slot = legacy_slot;
            }
        }
    }
    slots
}

fn run_tuning_library_operation_with_fallback(
    session: &mut Session,
    context: TuningLibraryContext,
    operation: TuningLibraryOperation,
    directory: &Path,
    legacy_directory: Option<&Path>,
    mut apply: impl FnMut(&mut Session, Command) -> Result<Arc<PreparedRevision>, SessionError>,
) -> (TuningLibraryResult, Option<Arc<PreparedRevision>>) {
    let mut result = TuningLibraryResult {
        operation_id: context.operation_id,
        slots: Vec::new(),
        selected_slot: None,
        loaded_name: None,
        error: None,
    };
    let mut prepared = None;
    let operation_result = match operation {
        TuningLibraryOperation::Save { .. } | TuningLibraryOperation::Export { .. }
            if context.request_generation != context.current_generation
                || context.request_revision != context.current_revision =>
        {
            Err("tuning save or export belongs to a changed project revision".into())
        }
        TuningLibraryOperation::List => Ok(()),
        TuningLibraryOperation::Save { slot, name } => {
            result.selected_slot = Some(slot);
            hex_storage::tuning_library::save(
                directory,
                slot,
                &name,
                session.project().tuning().scale(),
            )
        }
        TuningLibraryOperation::Load { slot } => {
            result.selected_slot = Some(slot);
            if context.request_generation != context.current_generation
                || context.request_revision != context.current_revision
            {
                Err("tuning load belongs to a changed project revision".into())
            } else {
                match load_tuning_slot_with_fallback(directory, legacy_directory, slot) {
                    Ok(preset) => {
                        result.loaded_name = Some(preset.name);
                        match ProjectTuning::new(
                            session.project().tuning().enabled(),
                            preset.scale,
                            session.project().tuning().audition(),
                        ) {
                            Ok(tuning) if tuning == session.project().tuning() => Ok(()),
                            Ok(tuning) => match apply(session, Command::SetTuning(tuning)) {
                                Ok(artifact) => {
                                    prepared = Some(artifact);
                                    Ok(())
                                }
                                Err(error) => Err(hex_storage::text::lowercase_outside_verbatim(
                                    &error.to_string(),
                                )),
                            },
                            Err(error) => Err(error.to_string()),
                        }
                    }
                    Err(error) => Err(error),
                }
            }
        }
        TuningLibraryOperation::Rename { slot, name } => {
            result.selected_slot = Some(slot);
            hex_storage::tuning_library::rename(directory, slot, &name)
        }
        TuningLibraryOperation::Duplicate { slot } => {
            match hex_storage::tuning_library::duplicate(directory, slot) {
                Ok(new_slot) => {
                    result.selected_slot = Some(new_slot);
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }
        TuningLibraryOperation::Import { path } => {
            if context.request_generation != context.current_generation
                || context.request_revision != context.current_revision
            {
                Err("tuning import belongs to a changed project revision".into())
            } else {
                match hex_storage::tuning_library::read_file(&path) {
                    Ok(preset) => {
                        result.loaded_name = Some(preset.name);
                        match ProjectTuning::new(
                            session.project().tuning().enabled(),
                            preset.scale,
                            session.project().tuning().audition(),
                        ) {
                            Ok(tuning) if tuning == session.project().tuning() => Ok(()),
                            Ok(tuning) => match apply(session, Command::SetTuning(tuning)) {
                                Ok(artifact) => {
                                    prepared = Some(artifact);
                                    Ok(())
                                }
                                Err(error) => Err(hex_storage::text::lowercase_outside_verbatim(
                                    &error.to_string(),
                                )),
                            },
                            Err(error) => Err(error.to_string()),
                        }
                    }
                    Err(error) => Err(error),
                }
            }
        }
        TuningLibraryOperation::Export { path, name } => hex_storage::tuning_library::export_file(
            &path,
            &name,
            session.project().tuning().scale(),
        ),
    };
    result.error = operation_result.err();
    result.slots = list_tuning_slots_with_fallback(directory, legacy_directory);
    (result, prepared)
}

pub(super) fn handle(ctx: &mut WorkerDispatch<'_>, request: Request) -> ControlFlow<()> {
    match request {
        Request::ContinueRecovery => {}

        Request::ChangeProjectSampleRate {
            project_generation: captured,
            revision: captured_revision,
            ..
        } if captured != ctx.worker.project_generation
            || super::reconcile_barrier_revision(
                captured_revision,
                ctx.outcome.revision_before_request,
                ctx.worker.revision,
            ) != ctx.worker.revision =>
        {
            ctx.outcome.error = Some("project changed · sample rate request cancelled".into());
        }

        Request::ChangeProjectSampleRate { .. } | Request::Undo | Request::Redo
            if ctx.outcome.rate_transition.is_some() =>
        {
            let admission = ctx.worker.export_lane.admission_with_resident_owners(
                ctx.worker.compiled.as_ref().map(|(_, p)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: ctx.worker.current_sources.as_ref(),
                        prepared: Some(p.as_ref()),
                    }
                }),
                ctx.worker.resident_seek.as_ref(),
                ctx.worker.resident_preview.as_ref(),
            );
            if let Some(transition) = ctx.outcome.rate_transition.take() {
                let cancellation = RequestCancellation::new(
                    Arc::clone(&ctx.worker.session.shutdown),
                    Arc::clone(&ctx.worker.document_intent),
                    ctx.worker.document_intent.load(Ordering::Acquire),
                );
                match project_rate::replace(
                    &mut ctx.worker.session,
                    transition,
                    ctx.worker.history_capacity,
                    ctx.worker.media_base.as_deref(),
                    hex_engine::AudioSourceContext {
                        project_generation: ctx.worker.project_generation,
                        editor_revision: ctx.worker.revision,
                    },
                    &cancellation,
                    &mut ctx.worker.events,
                    ctx.worker.resident_snapshot.as_ref(),
                    admission,
                ) {
                    Ok(Some(prepared)) => {
                        resident_audio::adopt_document_snapshot(
                            &mut ctx.worker.resident_snapshot,
                            prepared.resident,
                        );
                        ctx.worker.unsaved = true;
                        ctx.outcome.project_replaced = true;
                        ctx.outcome.replacement_revision = Some(prepared.prepared);
                        ctx.outcome.replacement_sources = Some(prepared.sources);
                        ctx.outcome.status = Some("project sample rate updated".into());
                    }
                    Ok(None) => {
                        ctx.outcome.status = Some("project sample rate already selected".into())
                    }
                    Err(reason) => ctx.outcome.error = Some(reason),
                }
            }
        }

        Request::ChangeProjectSampleRate { .. } => {
            ctx.outcome.error = Some("project sample rate choice could not be resolved".into());
        }
        // `truncated_import::admit` unwraps every recovery at receipt.
        Request::RecoverTruncatedImport(_) => {
            ctx.outcome.error = Some("truncated import recovery was not admitted".into());
        }

        Request::RestartRecovery(intent) => {
            let result = if ctx.worker.device_gesture.snapshot().is_some()
                || ctx.worker.tuning_gesture.snapshot().is_some()
            {
                Err("finish the active value edit before restarting recovery".into())
            } else if let Some(untitled) = ctx.worker.recovery.as_deref() {
                ctx.worker.recovery_tracker.restart(
                    recovery::RecoveryContext {
                        session: &ctx.worker.session,
                        base: ctx.worker.media_base.as_deref(),
                        document: ctx.worker.document.as_ref(),
                        untitled,
                        journal: &mut ctx.worker.journal,
                        generation: ctx.worker.project_generation,
                    },
                    ctx.worker.revision,
                    intent,
                )
            } else {
                Err("automatic recovery is not enabled".into())
            };
            match result {
                Ok(()) => {
                    ctx.outcome.status =
                        Some("automatic recovery restarted · previous journal preserved".into())
                }
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        Request::TuningLibrary {
            project_generation: expected_generation,
            revision: expected_revision,
            operation_id,
            operation,
        } => {
            // A same-request gesture barrier may have committed the
            // tuning drag just before the captured library operation.
            // Accept its original revision only across that serialized
            // barrier commit; earlier queued project edits remain stale.
            let effective_revision = super::reconcile_barrier_revision(
                expected_revision,
                ctx.outcome.revision_before_request,
                ctx.worker.revision,
            );
            let (result, prepared) = run_tuning_library_operation(
                &mut ctx.worker.session,
                TuningLibraryContext {
                    request_generation: expected_generation,
                    request_revision: effective_revision,
                    operation_id,
                    current_generation: ctx.worker.project_generation,
                    current_revision: ctx.worker.revision,
                },
                operation,
                |session, command| {
                    resident_audio::execute_command(
                        session,
                        command,
                        ctx.worker
                            .compiled
                            .as_ref()
                            .map(|(_, artifact)| artifact.as_ref()),
                        hex_engine::AudioSourceContext {
                            project_generation: ctx.worker.project_generation,
                            editor_revision: ctx.worker.revision.saturating_add(1),
                        },
                        &mut ctx.worker.events,
                        &mut ctx.worker.resident_snapshot,
                        ctx.worker.export_lane.admission_with_resident_owners(
                            ctx.worker.compiled.as_ref().map(|(_, prepared)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: ctx.worker.current_sources.as_ref(),
                                    prepared: Some(prepared),
                                }
                            }),
                            ctx.worker.resident_seek.as_ref(),
                            ctx.worker.resident_preview.as_ref(),
                        ),
                    )
                    .map(|(_, prepared)| prepared)
                },
            );
            ctx.outcome.tuning_library = Some(result);
            if let Some(prepared) = prepared {
                ctx.worker.unsaved = true;
                ctx.outcome.replacement_revision = Some(prepared);
            }
        }

        Request::RelinkMedia(relinks) => {
            let count = relinks.len();
            let result = if ctx.worker.events.resident_publication_enabled() {
                ctx.worker
                    .events
                    .resident_audio()
                    .ok_or(SessionError::Invalid("native worker unavailable"))
                    .and_then(|sink| {
                        resident_audio::relink_files(
                            &mut ctx.worker.session,
                            relinks,
                            hex_engine::AudioSourceContext {
                                project_generation: ctx.worker.project_generation,
                                editor_revision: ctx.worker.revision.saturating_add(1),
                            },
                            sink,
                            ctx.worker.export_lane.admission_with_resident_owners(
                                ctx.worker.compiled.as_ref().map(|(_, prepared)| {
                                    hex_engine::native_device_resources::ControlSamplerSelection {
                                        sources: ctx.worker.current_sources.as_ref(),
                                        prepared: Some(prepared),
                                    }
                                }),
                                ctx.worker.resident_seek.as_ref(),
                                ctx.worker.resident_preview.as_ref(),
                            ),
                            &mut ctx.worker.resident_snapshot,
                        )
                    })
            } else {
                let allowance = ctx
                    .worker
                    .session
                    .max_resident_frames()
                    .saturating_sub(ctx.worker.session.resident_frames());
                resident_audio::decode_relinks(&ctx.worker.session, relinks, allowance).and_then(
                    |prepared| {
                        let expected = ctx.worker.session.project().clone();
                        ctx.worker.session.relink_offline_media_revision(
                            prepared,
                            &expected,
                            ctx.worker
                                .compiled
                                .as_ref()
                                .map(|(_, artifact)| artifact.as_ref()),
                        )
                    },
                )
            };
            match result {
                Ok(prepared) => {
                    ctx.worker.unsaved = true;
                    ctx.outcome.replacement_revision = Some(prepared);
                    ctx.outcome.status = Some(format!("relinked {count} files"));
                }
                Err(reason) => ctx.outcome.error = Some(reason.to_string()),
            }
        }

        Request::NewWithAudioMode { mode, reservation } => {
            ctx.outcome.open_complete = true;
            let candidate = Session::new_with_audio_mode_in_overview_domain(
                ctx.worker
                    .events
                    .default_project_sample_rate()
                    .unwrap_or_else(|| ctx.worker.session.project().sample_rate()),
                ctx.worker.history_capacity,
                ctx.worker.session.max_resident_frames(),
                mode,
                reservation.or_else(|| {
                    (mode == DocumentAudioMode::Resident)
                        .then(|| ctx.worker.events.ordinary_source_reservation())
                        .flatten()
                }),
                ctx.worker.session.overview_budget.clone(),
            )
            .and_then(|candidate| match ctx.worker.session.native_reservation {
                Some(limits) => candidate.with_native_reservation(limits),
                None => Ok(candidate),
            })
            .and_then(|candidate| {
                project_rate::new_following_interface(candidate, ctx.worker.history_capacity)
            })
            .map(|candidate| candidate.with_shutdown(Arc::clone(&ctx.worker.session.shutdown)))
            .map_err(|reason| reason.to_string());
            let current = ctx.worker.compiled.as_ref().map(|(_, p)| {
                hex_engine::native_device_resources::ControlSamplerSelection {
                    sources: ctx.worker.current_sources.as_ref(),
                    prepared: Some(p.as_ref()),
                }
            });
            let admission = ctx.worker.export_lane.admission_with_resident_owners(
                current,
                ctx.worker.resident_seek.as_ref(),
                ctx.worker.resident_preview.as_ref(),
            );
            match candidate.and_then(|candidate| {
                let prepared = resident_audio::prepare_document_replacement(
                    &candidate,
                    &ctx.worker.session,
                    hex_engine::AudioSourceContext {
                        project_generation: ctx.worker.project_generation,
                        editor_revision: ctx.worker.revision,
                    },
                    &mut ctx.worker.events,
                    ctx.worker.resident_snapshot.as_ref(),
                    admission,
                )?;
                Ok((candidate, prepared))
            }) {
                Ok((candidate, prepared)) => {
                    // All fallible construction/admission is complete. These fixed moves
                    // immediately adopt the exact pending candidate and trigger teardown.
                    ctx.worker.session = candidate;
                    resident_audio::adopt_document_snapshot(
                        &mut ctx.worker.resident_snapshot,
                        prepared.resident,
                    );
                    ctx.worker.journal = None;
                    ctx.worker.media_base = None;
                    ctx.worker.document = None;
                    ctx.worker.unsaved = false;
                    ctx.outcome.project_replaced = true;
                    ctx.outcome.replacement_revision = Some(prepared.prepared);
                    ctx.outcome.replacement_sources = Some(prepared.sources);
                    ctx.outcome.status = Some("new project".into());
                }
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        request @ (Request::Open(_)
        | Request::OpenWith { .. }
        | Request::OpenWithAudioMode { .. }) => {
            ctx.outcome.open_complete = true;
            let (source, relinks, mode, reservation) = match request {
                Request::Open(path) => (
                    ProjectSource::Saved(path),
                    BTreeMap::new(),
                    DocumentAudioMode::Resident,
                    None,
                ),
                Request::OpenWith { source, relinks } => {
                    (source, relinks, DocumentAudioMode::Resident, None)
                }
                Request::OpenWithAudioMode {
                    source,
                    relinks,
                    mode,
                    reservation,
                } => (source, relinks, mode, reservation),
                _ => {
                    return ControlFlow::Break(());
                }
            };
            let current = ctx.worker.compiled.as_ref().map(|(_, p)| {
                hex_engine::native_device_resources::ControlSamplerSelection {
                    sources: ctx.worker.current_sources.as_ref(),
                    prepared: Some(p.as_ref()),
                }
            });
            let admission = ctx.worker.export_lane.admission_with_resident_owners(
                current,
                ctx.worker.resident_seek.as_ref(),
                ctx.worker.resident_preview.as_ref(),
            );
            let candidate = resident_audio::open_document(
                &ctx.worker.session,
                resident_audio::OpenDocumentSpec {
                    history_capacity: ctx.worker.history_capacity,
                    source: &source,
                    relinks: &relinks,
                    mode,
                    reservation,
                },
                resident_audio::ResidentWorkerContext {
                    events: &mut ctx.worker.events,
                    admission,
                },
                ctx.worker.resident_snapshot.as_ref(),
            );
            match candidate {
                Ok((candidate, candidate_base, candidate_document, dirty)) => {
                    let compiled_candidate = resident_audio::prepare_document_replacement(
                        &candidate,
                        &ctx.worker.session,
                        hex_engine::AudioSourceContext {
                            project_generation: ctx.worker.project_generation,
                            editor_revision: ctx.worker.revision,
                        },
                        &mut ctx.worker.events,
                        ctx.worker.resident_snapshot.as_ref(),
                        admission,
                    );
                    match compiled_candidate {
                        Ok(snapshot) => {
                            ctx.worker.journal = None;
                            ctx.worker.session = candidate;
                            resident_audio::adopt_document_snapshot(
                                &mut ctx.worker.resident_snapshot,
                                snapshot.resident,
                            );
                            ctx.worker.media_base = Some(candidate_base);
                            ctx.worker.document = candidate_document;
                            ctx.worker.unsaved = dirty;
                            ctx.outcome.project_replaced = true;
                            ctx.outcome.replacement_revision = Some(snapshot.prepared);
                            ctx.outcome.replacement_sources = Some(snapshot.sources);
                            ctx.outcome.status = Some(
                                if dirty {
                                    "project recovered or relinked · save to keep changes"
                                } else {
                                    "project opened"
                                }
                                .into(),
                            );
                        }
                        Err(reason) => ctx.outcome.error = Some(reason),
                    }
                }
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        request @ (Request::Undo | Request::Redo)
            if ctx.worker.events.resident_publication_enabled()
                && ctx.worker.session.audio_mode() == DocumentAudioMode::Resident
                && ctx
                    .worker
                    .session
                    .editor
                    .next_history_requires_stopped_bounce(matches!(&request, Request::Redo)) =>
        {
            let context = hex_engine::AudioSourceContext {
                project_generation: ctx.worker.project_generation,
                editor_revision: ctx.worker.revision.saturating_add(1),
            };
            let admission = ctx.worker.export_lane.admission_with_resident_owners(
                ctx.worker.compiled.as_ref().map(|(_, p)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: ctx.worker.current_sources.as_ref(),
                        prepared: Some(p),
                    }
                }),
                ctx.worker.resident_seek.as_ref(),
                ctx.worker.resident_preview.as_ref(),
            );
            let prefence = ctx
                .worker
                .events
                .take_stopped_fence(hex_engine::AudioSourceContext {
                    project_generation: ctx.worker.project_generation,
                    editor_revision: ctx.worker.revision,
                });
            let result = ctx
                .worker
                .events
                .resident_audio()
                .ok_or(SessionError::Invalid("native worker unavailable"))
                .and_then(|sink| {
                    resident_audio::bounce_history(
                        &mut ctx.worker.session,
                        matches!(request, Request::Redo),
                        context,
                        sink,
                        admission,
                        &mut ctx.worker.resident_snapshot,
                        prefence,
                    )
                });
            match result {
                Ok(Some(prepared)) => {
                    ctx.worker.unsaved = true;
                    ctx.outcome.replacement_revision = Some(prepared);
                    ctx.outcome.status = Some("bounce history committed".into());
                }
                Ok(None) => ctx.outcome.status = Some("no history change".into()),
                Err(reason) => ctx.outcome.error = Some(reason.to_string()),
            }
        }

        request @ (Request::Undo | Request::Redo)
            if ctx.worker.events.resident_publication_enabled()
                && ctx.worker.session.audio_mode() == DocumentAudioMode::Resident
                && ctx
                    .worker
                    .session
                    .section_history_requires_publication(matches!(&request, Request::Redo)) =>
        {
            let context = hex_engine::AudioSourceContext {
                project_generation: ctx.worker.project_generation,
                editor_revision: ctx.worker.revision.saturating_add(1),
            };
            let admission = ctx.worker.export_lane.admission_with_resident_owners(
                ctx.worker.compiled.as_ref().map(|(_, prepared)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: ctx.worker.current_sources.as_ref(),
                        prepared: Some(prepared),
                    }
                }),
                ctx.worker.resident_seek.as_ref(),
                ctx.worker.resident_preview.as_ref(),
            );
            match arrangement_sections::history(
                &mut ctx.worker.session,
                matches!(request, Request::Redo),
                context,
                &mut ctx.worker.events,
                &mut ctx.worker.resident_snapshot,
                admission,
            ) {
                Ok(Some(prepared)) => {
                    ctx.worker.unsaved = true;
                    ctx.outcome.replacement_revision = Some(prepared);
                    ctx.outcome.status = Some("section history committed".into());
                }
                Ok(None) => ctx.outcome.status = Some("no history change".into()),
                Err(reason) => ctx.outcome.error = Some(reason.to_string()),
            }
        }

        request @ (Request::Undo | Request::Redo)
            if ctx.worker.events.resident_publication_enabled()
                && ctx.worker.session.audio_mode() == DocumentAudioMode::Resident
                && ctx.worker.resident_snapshot.is_some()
                && ctx
                    .worker
                    .session
                    .section_history_marker_only(matches!(&request, Request::Redo)) =>
        {
            let admission = ctx.worker.export_lane.admission_with_resident_owners(
                ctx.worker.compiled.as_ref().map(|(_, prepared)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: ctx.worker.current_sources.as_ref(),
                        prepared: Some(prepared),
                    }
                }),
                ctx.worker.resident_seek.as_ref(),
                ctx.worker.resident_preview.as_ref(),
            );
            match arrangement_sections::marker_history(
                &mut ctx.worker.session,
                matches!(request, Request::Redo),
                hex_engine::AudioSourceContext {
                    project_generation: ctx.worker.project_generation,
                    editor_revision: ctx.worker.revision.saturating_add(1),
                },
                &mut ctx.worker.events,
                &mut ctx.worker.resident_snapshot,
                admission,
            ) {
                Ok(Some(prepared)) => {
                    ctx.worker.unsaved = true;
                    ctx.outcome.replacement_revision = Some(prepared);
                    ctx.outcome.status = Some("section history committed".into());
                }
                Ok(None) => ctx.outcome.status = Some("no history change".into()),
                Err(reason) => ctx.outcome.error = Some(reason.to_string()),
            }
        }

        request @ (Request::Undo | Request::Redo)
            if ctx.worker.events.resident_publication_enabled()
                && ctx.worker.session.audio_mode() == DocumentAudioMode::Resident =>
        {
            let redo = matches!(request, Request::Redo);
            let context = hex_engine::AudioSourceContext {
                project_generation: ctx.worker.project_generation,
                editor_revision: ctx.worker.revision,
            };
            let admission = ctx.worker.export_lane.admission_with_resident_owners(
                ctx.worker.compiled.as_ref().map(|(_, prepared)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: ctx.worker.current_sources.as_ref(),
                        prepared: Some(prepared),
                    }
                }),
                ctx.worker.resident_seek.as_ref(),
                ctx.worker.resident_preview.as_ref(),
            );
            let mut pending_history = None;
            let result = ctx
                .worker
                .events
                .resident_audio()
                .ok_or(SessionError::Invalid("native worker unavailable"))
                .and_then(|sink| {
                    pending_history = ctx.worker.session.stage_resident_graph_history(
                        redo,
                        context,
                        sink,
                        admission,
                        ctx.worker.resident_snapshot.as_ref(),
                        ctx.outcome.request_document_intent,
                    )?;
                    if pending_history.is_some() {
                        return Ok(None);
                    }
                    ctx.worker.session.history_resident_snapshot(
                        redo,
                        hex_engine::AudioSourceContext {
                            editor_revision: ctx.worker.revision.saturating_add(1),
                            ..context
                        },
                        sink,
                        admission,
                        ctx.worker.resident_snapshot.as_ref(),
                    )
                });
            match result {
                Ok(Some(snapshot)) => {
                    ctx.worker.unsaved = true;
                    ctx.outcome.replacement_revision = Some(Arc::clone(&snapshot.prepared));
                    ctx.worker.resident_snapshot = Some(snapshot);
                    ctx.outcome.status = Some("history committed".into());
                }
                Ok(None) if pending_history.is_some() => {
                    ctx.worker.pending_device_insert = pending_history;
                    ctx.outcome.request_complete = false;
                    ctx.outcome.status = Some("history pending · waiting for audio".into());
                }
                Ok(None) => ctx.outcome.status = Some("no history change".into()),
                Err(reason) => ctx.outcome.error = Some(reason.to_string()),
            }
        }

        request @ (Request::Undo | Request::Redo)
            if ctx.worker.session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                && ctx
                    .worker
                    .session
                    .has_verified_hosted_history(matches!(request, Request::Redo))
                && ctx
                    .worker
                    .events
                    .resident_audio()
                    .is_some_and(|sink| sink.confirms_verified_device_inserts()) =>
        {
            let result = ctx
                .worker
                .events
                .resident_audio()
                .ok_or(SessionError::Invalid("native worker unavailable"))
                .and_then(|sink| {
                    ctx.worker.session.stage_verified_hosted_history(
                        matches!(request, Request::Redo),
                        &ctx.worker.state,
                        ctx.worker
                            .compiled
                            .as_ref()
                            .map(|(_, prepared)| prepared.as_ref()),
                        sink,
                        ctx.outcome.request_document_intent,
                    )
                });
            match result {
                Ok(staged) => {
                    ctx.worker.pending_device_insert = Some(staged);
                    ctx.outcome.request_complete = false;
                    ctx.outcome.status = Some("history pending · waiting for audio".into());
                }
                Err(reason) => ctx.outcome.error = Some(reason.to_string()),
            }
        }

        request @ (Request::Undo | Request::Redo) => {
            let redo = matches!(request, Request::Redo);
            let group_history = if ctx.worker.session.audio_mode()
                == DocumentAudioMode::VerifiedStreaming
                && let Some(sink) = ctx.worker.events.resident_audio()
                && sink.confirms_verified_device_inserts()
            {
                ctx.worker.session.stage_verified_group_history(
                    redo,
                    &ctx.worker.state,
                    ctx.worker
                        .compiled
                        .as_ref()
                        .map(|(_, prepared)| prepared.as_ref()),
                    sink,
                    ctx.outcome.request_document_intent,
                )
            } else {
                Ok(None)
            };
            match group_history {
                Ok(Some(staged)) => {
                    ctx.worker.pending_device_insert = Some(staged);
                    ctx.outcome.request_complete = false;
                    ctx.outcome.status = Some("history pending · waiting for audio".into());
                }
                Err(reason) => ctx.outcome.error = Some(reason.to_string()),
                Ok(None) => {
                    let previous = ctx
                        .worker
                        .compiled
                        .as_ref()
                        .filter(|_| ctx.worker.comparison_binding.is_some())
                        .map(|(_, artifact)| artifact);
                    let finish =
                        |source: &Project, target: &Project, prepared: &Arc<PreparedRevision>| {
                            prepare_tuning_input_publication(
                                source,
                                target,
                                prepared,
                                ctx.worker.project_generation,
                                ctx.worker.revision,
                                ctx.worker.keyboard_activation.as_ref(),
                                ctx.worker.keyboard_tuning_hint.as_ref(),
                                ctx.worker.audition_activation.as_ref(),
                                false,
                            )
                        };
                    let history_result = if ctx.worker.session.audio_mode()
                        == DocumentAudioMode::VerifiedStreaming
                        && let Some(sink) = ctx.worker.events.resident_audio()
                    {
                        ctx.worker.session.comparison_history_verified(
                            redo,
                            previous,
                            hex_engine::AudioSourceContext {
                                project_generation: ctx.worker.project_generation,
                                editor_revision: ctx.worker.revision.saturating_add(1),
                            },
                            sink,
                            finish,
                        )
                    } else {
                        ctx.worker
                            .session
                            .comparison_history(redo, previous, finish)
                    };
                    match history_result {
                        Ok(Some(outcome)) => {
                            ctx.worker.unsaved = true;
                            if outcome.metadata_only {
                                ctx.outcome.comparison_changed = true;
                            } else {
                                ctx.outcome.replacement_revision = Some(outcome.prepared);
                            }
                            ctx.outcome.tuning_input_publication = outcome.completed.flatten();
                            ctx.outcome.status = Some(
                                if redo {
                                    "redo committed"
                                } else {
                                    "undo committed"
                                }
                                .into(),
                            );
                        }
                        Ok(None) => {
                            ctx.outcome.status = Some(
                                if redo {
                                    "nothing to redo"
                                } else {
                                    "nothing to undo"
                                }
                                .into(),
                            )
                        }
                        Err(reason) => {
                            ctx.outcome.error = Some(hex_storage::text::lowercase_outside_verbatim(
                                &reason.to_string(),
                            ))
                        }
                    }
                }
            }
        }

        Request::SaveCommittedParameterCopy {
            path,
            revision,
            key,
            sequence,
            reply,
        } => {
            let result = committed_copy::save(ctx, path, revision, key, sequence);
            if let Err(reason) = &result {
                ctx.outcome.error = Some(reason.clone());
            }
            let _ = reply.try_send(result);
        }
        request @ (Request::SaveNew(_) | Request::SaveProjectAs { .. }) => {
            ctx.outcome.save_complete = true;
            let (path, folder_layout) = match request {
                Request::SaveNew(path) => (path, None),
                Request::SaveProjectAs {
                    path,
                    extension_added,
                } => (path, Some(extension_added)),
                _ => {
                    return ControlFlow::Break(());
                }
            };
            // Earlier source intent may cancel the edit ahead of Save,
            // but FIFO still saves its resulting canonical project.
            // Only a new intent during this capture cancels this Save.
            let save_cancellation = RequestCancellation::new(
                Arc::clone(&ctx.worker.session.shutdown),
                Arc::clone(&ctx.worker.document_intent),
                ctx.worker.document_intent.load(Ordering::Acquire),
            );
            match plugin_state_save::project_for_save_with_capture(
                &ctx.worker.session,
                ctx.worker.media_base.as_deref(),
                hex_engine::AudioSourceContext {
                    project_generation: ctx.worker.project_generation,
                    editor_revision: ctx.worker.revision,
                },
                &save_cancellation,
                ctx.worker.events.sampler_edit_audio(),
            ) {
                Ok(project) => match absolute_document_path(&path) {
                    Ok(path)
                        if ctx
                            .worker
                            .document
                            .as_ref()
                            .is_some_and(|handle| is_open_document(handle, &path)) =>
                    {
                        ctx.outcome.error = Some(
                            "save as chose the open project · use save to update it, or choose a new name"
                                .into(),
                        );
                    }
                    Ok(path) => match if let Some(extension_added) = folder_layout {
                        project_save::save_as(
                            path,
                            extension_added,
                            project,
                            ctx.worker.recovery.as_deref(),
                            &save_cancellation,
                        )
                    } else {
                        save_document_as(path, &project)
                    } {
                        Ok((handle, commit)) => {
                            ctx.worker.document = Some(handle);
                            ctx.worker.unsaved = commit.dirty;
                            ctx.outcome.error = commit.error;
                            ctx.worker.state =
                                view_state_bound(
                                    &ctx.worker.session,
                                    Arc::clone(&ctx.worker.midi_preview),
                                    ctx.worker.revision,
                                    ctx.worker.project_generation,
                                    ctx.worker.unsaved,
                                    ctx.worker
                                        .document
                                        .as_ref()
                                        .map(|handle| handle.path.as_path()),
                                    ctx.worker.state.canonical_audio_source.as_ref().map(
                                        |source| {
                                            (
                                                Arc::clone(source),
                                                ctx.worker.state.comparison_audio.clone(),
                                            )
                                        },
                                    ),
                                );
                            ctx.outcome.status = Some(commit.status);
                        }
                        Err(reason) => ctx.outcome.error = Some(reason),
                    },
                    Err(reason) => ctx.outcome.error = Some(reason),
                },
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        request @ (Request::SaveCurrent | Request::SaveCurrentWithFormatUpgrade { .. }) => {
            ctx.outcome.save_complete = true;
            // Earlier source intent may cancel the edit ahead of Save,
            // but FIFO still saves its resulting canonical project.
            // Only a new intent during this capture cancels this Save.
            let save_cancellation = RequestCancellation::new(
                Arc::clone(&ctx.worker.session.shutdown),
                Arc::clone(&ctx.worker.document_intent),
                ctx.worker.document_intent.load(Ordering::Acquire),
            );
            let consent = match &request {
                Request::SaveCurrentWithFormatUpgrade {
                    project_generation: expected,
                    path,
                } => {
                    *expected == ctx.worker.project_generation
                        && ctx
                            .worker
                            .document
                            .as_ref()
                            .is_some_and(|handle| handle.path == *path)
                }
                _ => false,
            };
            let stale_consent =
                matches!(request, Request::SaveCurrentWithFormatUpgrade { .. }) && !consent;
            let save_project = if stale_consent {
                Err("project changed · format upgrade cancelled".to_owned())
            } else if ctx.worker.document.is_none() {
                Err("cannot save current project before it has a file path".to_owned())
            } else {
                plugin_state_save::project_for_save_with_capture(
                    &ctx.worker.session,
                    ctx.worker.media_base.as_deref(),
                    hex_engine::AudioSourceContext {
                        project_generation: ctx.worker.project_generation,
                        editor_revision: ctx.worker.revision,
                    },
                    &save_cancellation,
                    ctx.worker.events.sampler_edit_audio(),
                )
            };
            let image = save_project.and_then(|project| {
                let handle = ctx
                    .worker
                    .document
                    .as_ref()
                    .ok_or("project has no file path")?;
                project_save::Image::for_document(
                    project,
                    &handle.path,
                    ctx.worker.recovery.as_deref(),
                )
            });
            match (ctx.worker.document.as_mut(), image) {
                (None, _) => {
                    ctx.outcome.error =
                        Some("cannot save current project before it has a file path".into())
                }
                (_, Err(reason)) => ctx.outcome.error = Some(reason),
                (Some(_), Ok(_)) if stale_consent => {
                    ctx.outcome.error = Some("project changed · format upgrade cancelled".into());
                }
                (Some(handle), Ok(image))
                    if !consent && handle.token.requires_schema_upgrade_for(&image.project) =>
                {
                    ctx.outcome.save_confirmation_required = true;
                    ctx.outcome.error = Some("saving makes this file incompatible with older hex readers; use save as to preserve it, or explicitly confirm overwrite".into());
                }
                (Some(handle), Ok(image)) => {
                    match hex_storage::prepare_replacement_within(
                        &handle.path,
                        &handle.token,
                        &image.project,
                        ctx.worker.events.legacy_reader_limit(),
                    ) {
                        Ok(prepared) if !consent && prepared.requires_confirmation() => {
                            ctx.outcome.save_confirmation_required = true;
                            ctx.outcome.error = Some("saving makes this file incompatible with older hex readers; use save as to preserve it, or explicitly confirm overwrite".into());
                        }
                        Ok(prepared) => {
                            match image
                                .collect(&save_cancellation)
                                .and_then(|()| prepared.publish().map_err(project_save::display))
                            {
                                Ok(outcome) => {
                                    let commit = apply_current_save_outcome(handle, outcome);
                                    ctx.worker.unsaved = commit.dirty;
                                    ctx.outcome.status = Some(commit.status);
                                    ctx.outcome.error = commit.error;
                                    ctx.worker.state = view_state_bound(
                                        &ctx.worker.session,
                                        Arc::clone(&ctx.worker.midi_preview),
                                        ctx.worker.revision,
                                        ctx.worker.project_generation,
                                        ctx.worker.unsaved,
                                        ctx.worker
                                            .document
                                            .as_ref()
                                            .map(|current| current.path.as_path()),
                                        ctx.worker.state.canonical_audio_source.as_ref().map(
                                            |source| {
                                                (
                                                    Arc::clone(source),
                                                    ctx.worker.state.comparison_audio.clone(),
                                                )
                                            },
                                        ),
                                    );
                                }
                                Err(reason) => ctx.outcome.error = Some(reason),
                            }
                        }
                        Err(reason) => {
                            ctx.outcome.error = Some(hex_storage::text::lowercase_outside_verbatim(
                                &reason.to_string(),
                            ))
                        }
                    }
                }
            }
        }

        request @ (Request::Export(_) | Request::ExportRange { .. }) => {
            // Both spellings are export requests, so their parts always exist.
            if let Some((path, frames, controlled)) = request.into_export_range() {
                let job = if ctx
                    .worker
                    .document
                    .as_ref()
                    .is_some_and(|document| is_open_document(document, &path))
                {
                    Err("export destination is the open project; choose another path".to_owned())
                } else if export_destination_is_original_media(
                    ctx.worker.session.project(),
                    &path,
                    ctx.worker.media_base.as_deref(),
                ) {
                    Err("export destination is original media; choose another path".to_owned())
                } else {
                    match ctx.worker.compiled.as_ref() {
                        Some((compiled_revision, prepared))
                            if *compiled_revision == ctx.worker.revision =>
                        {
                            hex_export::controlled_frame_count(
                                prepared.arrangement(),
                                controlled.start,
                                frames,
                                &controlled.signal,
                            )
                            .map_err(|error| error.to_string())
                            .map(|frames| ExportJob {
                                job_id: 0,
                                streamed: None,
                                foreign: None,
                                sources: None,
                                resources: None,
                                revision: ctx.worker.revision,
                                path: path.clone(),
                                prepared: Arc::clone(prepared),
                                frames,
                                controlled: Some(controlled),
                            })
                        }
                        _ => Err("latest project revision has not compiled".to_owned()),
                    }
                }
                .and_then(|mut job| {
                    if ctx.worker.session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                        || job.prepared.arrangement().contains_streamed_sources()
                    {
                        job.streamed = Some(streamed_export::StreamedExportJob::capture(
                            &ctx.worker.session,
                            &ctx.worker.state.project,
                            ctx.worker.project_generation,
                            ctx.worker.revision,
                            ctx.worker.events.streamed_exporter(),
                        )?);
                    }
                    Ok(job)
                });
                match job {
                    Ok(job) => {
                        ctx.outcome.status = Some("export queued".into());
                        ctx.outcome.export_job = Some(job);
                    }
                    Err(reason) => {
                        ctx.outcome.export_completion = Some(ExportEvent {
                            revision: ctx.worker.revision,
                            path,
                            result: Err(reason.into()),
                        })
                    }
                }
            }
        }

        _ => {
            ctx.outcome.error = Some("unexpected worker dispatch route".into());
        }
    }
    ControlFlow::Continue(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tuning_library_directory_uses_portable_app_support_dir_without_portalsurfer() {
        let home = Path::new("/tmp/hex-home");
        let path = tuning_library_directory_from(home);
        assert_eq!(
            path,
            hex_preferences::app_support_dir_from(home).join("scales")
        );
        assert!(
            !path.to_string_lossy().contains("PORTALSURFER"),
            "canonical tuning directory must not embed personal PORTALSURFER segment: {path:?}"
        );
        if cfg!(windows) && std::env::var_os("USERPROFILE").is_some_and(|v| !v.is_empty()) {
            let resolved = tuning_library_directory()
                .expect("windows user profile resolves tuning library directory");
            assert!(resolved.ends_with("scales"));
        } else if let Ok(resolved) = tuning_library_directory() {
            assert!(resolved.ends_with("scales"));
        }
    }

    #[test]
    fn tuning_library_reads_legacy_fallback_without_writing_to_legacy_directory() {
        let root = std::env::temp_dir().join(format!(
            "hex-tuning-fallback-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let canonical = root.join("canonical");
        let legacy = root.join("legacy");
        std::fs::create_dir_all(&legacy).unwrap();

        let legacy_scale = hex_tuning::Scale::equal(12);
        hex_storage::tuning_library::save(&legacy, 1, "legacy-preset", legacy_scale).unwrap();

        let mut session = Session::new(48_000, 8, 1024).unwrap();
        let ctx = TuningLibraryContext {
            request_generation: 1,
            request_revision: 0,
            operation_id: 1,
            current_generation: 1,
            current_revision: 0,
        };
        let (listed, _) = run_tuning_library_operation_with_fallback(
            &mut session,
            ctx,
            TuningLibraryOperation::List,
            &canonical,
            Some(&legacy),
            |session, cmd| session.command_prepared_revision(cmd, None).map(|(_, p)| p),
        );
        assert!(listed.error.is_none());
        assert_eq!(listed.slots[0].name.as_deref(), Some("legacy-preset"));

        let (loaded, _) = run_tuning_library_operation_with_fallback(
            &mut session,
            TuningLibraryContext {
                request_generation: 1,
                request_revision: 0,
                operation_id: 2,
                current_generation: 1,
                current_revision: 0,
            },
            TuningLibraryOperation::Load { slot: 1 },
            &canonical,
            Some(&legacy),
            |session, cmd| session.command_prepared_revision(cmd, None).map(|(_, p)| p),
        );
        assert!(loaded.error.is_none());
        assert_eq!(loaded.loaded_name.as_deref(), Some("legacy-preset"));

        let (saved, _) = run_tuning_library_operation_with_fallback(
            &mut session,
            TuningLibraryContext {
                request_generation: 1,
                request_revision: 0,
                operation_id: 3,
                current_generation: 1,
                current_revision: 0,
            },
            TuningLibraryOperation::Save {
                slot: 2,
                name: "canonical-preset".to_owned(),
            },
            &canonical,
            Some(&legacy),
            |session, cmd| session.command_prepared_revision(cmd, None).map(|(_, p)| p),
        );
        assert!(saved.error.is_none());
        assert!(canonical.join("slot-2.freqs").is_file());
        assert!(!legacy.join("slot-2.freqs").exists());
        assert_eq!(saved.slots[0].name.as_deref(), Some("legacy-preset"));
        assert_eq!(saved.slots[1].name.as_deref(), Some("canonical-preset"));

        let _ = std::fs::remove_dir_all(&root);
    }
}
