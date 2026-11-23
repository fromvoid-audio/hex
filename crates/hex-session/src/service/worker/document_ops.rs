//! Document open/save/new, recovery, sample-rate, undo/redo, export and tuning-library worker operations.
use super::*;
use std::ops::ControlFlow;

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
    document.path == destination
        || matches!(
            (std::fs::canonicalize(&document.path), std::fs::canonicalize(destination)),
            (Ok(open), Ok(chosen)) if open == chosen
        )
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

pub(crate) fn tuning_library_directory() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| "home directory is unavailable for tuning presets".to_owned())?;
    Ok(PathBuf::from(home).join("Library/Application Support/PORTALSURFER/hex/scales"))
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
            run_tuning_library_operation_in_using(session, context, operation, &directory, apply)
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
pub(crate) fn run_tuning_library_operation_in_using(
    session: &mut Session,
    context: TuningLibraryContext,
    operation: TuningLibraryOperation,
    directory: &Path,
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
                match hex_storage::tuning_library::load(directory, slot) {
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
    result.slots = hex_storage::tuning_library::list(directory);
    (result, prepared)
}

#[allow(unused_mut, unused_variables, clippy::too_many_lines)]
pub(super) fn handle<'a>(
    ctx: WorkerDispatch<'a>,
    request: Request,
) -> (WorkerDispatch<'a>, ControlFlow<()>) {
    let WorkerDispatch {
        mut session,
        history_capacity,
        requests,
        document_intent,
        mut events,
        mut export_lane,
        recovery,
        mut resident_seek,
        mut revision,
        mut unsaved,
        mut range_clipboard,
        mut track_clipboard,
        mut device_clipboard,
        mut media_base,
        mut document,
        mut journal,
        mut recovery_tracker,
        mut midi_preview,
        project_generation,
        mut state,
        mut keyboard_activation,
        mut keyboard_tuning_hint,
        mut audition_activation,
        mut live_expression,
        mut compiled,
        mut current_sources,
        mut resident_snapshot,
        mut resident_preview,
        mut device_gesture,
        mut tuning_gesture,
        mut pending_sampler_parameter,
        mut pending_plugin_effect,
        mut pending_device_insert,
        mut pending_stopped_restore,
        mut native_automation_take,
        mut plugin_recording,
        request_document_intent,
        defaults_root,
        mut import_decode,
        mut device_insert_proposal,
        mut device_insert_proposal_accepted,
        replacing_catalog_instrument,
        time_range_copy,
        mut seek_submitted,
        mut request_complete,
        mut plugin_effect_complete,
        mut device_insert_confirmation,
        mut device_insert_complete,
        mut plugin_parameters,
        mut plugin_parameter_conversion,
        mut sampler_edit_complete,
        mut midi_take_complete,
        mut computer_keyboard_complete,
        mut tuning_audition_complete,
        mut live_expression_complete,
        mut computer_keyboard_playback,
        mut tuning_input_publication,
        mut audio_batch_complete,
        mut audio_batch_failure,
        mut rate_transition,
        mut open_complete,
        mut save_complete,
        mut save_confirmation_required,
        mut project_replaced,
        mut replacement_revision,
        mut installed_verified_source,
        mut replacement_sources,
        mut comparison_changed,
        mut created,
        revision_before_request,
        mut duplicate_complete,
        mut restore_complete,
        mut stopped_playback_restore_complete,
        mut restore_prepared,
        mut status,
        mut error,
        mut range_copy_source_unavailable,
        mut export_completion,
        mut export_job,
        mut gesture_ack,
        mut tuning_gesture_ack,
        mut tuning_gesture_request,
        mut tuning_library,
        mut gesture_protocol_ack,
        mut gesture_request,
        mut preview_prepared,
        mut preview_project,
        mut cut_clipboard,
        mut cut_device_clipboard,
        adopted_device_draft,
        failed_plugin_adoption,
        mut failed_plugin_publication_confirmed,
        barrier,
        comparison_binding,
        private_sampler,
    } = ctx;
    #[allow(unused_mut)]
    let mut flow = ControlFlow::Continue(());
    match request {
        Request::ContinueRecovery => {}

        Request::ChangeProjectSampleRate {
            project_generation: captured,
            revision: captured_revision,
            ..
        } if captured != project_generation || captured_revision != revision => {
            error = Some("project changed · sample rate request cancelled".into());
        }

        Request::ChangeProjectSampleRate { .. } | Request::Undo | Request::Redo
            if rate_transition.is_some() =>
        {
            let admission = export_lane.admission_with_resident_owners(
                compiled.as_ref().map(|(_, p)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: current_sources.as_ref(),
                        prepared: Some(p.as_ref()),
                    }
                }),
                resident_seek.as_ref(),
                resident_preview.as_ref(),
            );
            if let Some(transition) = rate_transition.take() {
                let cancellation = RequestCancellation::new(
                    Arc::clone(&session.shutdown),
                    Arc::clone(&document_intent),
                    document_intent.load(Ordering::Acquire),
                );
                match project_rate::replace(
                    &mut session,
                    transition,
                    history_capacity,
                    media_base.as_deref(),
                    hex_engine::AudioSourceContext {
                        project_generation,
                        editor_revision: revision,
                    },
                    &cancellation,
                    &mut events,
                    resident_snapshot.as_ref(),
                    admission,
                ) {
                    Ok(Some(prepared)) => {
                        resident_audio::adopt_document_snapshot(
                            &mut resident_snapshot,
                            prepared.resident,
                        );
                        unsaved = true;
                        project_replaced = true;
                        replacement_revision = Some(prepared.prepared);
                        replacement_sources = Some(prepared.sources);
                        status = Some("project sample rate updated".into());
                    }
                    Ok(None) => status = Some("project sample rate already selected".into()),
                    Err(reason) => error = Some(reason),
                }
            }
        }

        Request::ChangeProjectSampleRate { .. } => {
            error = Some("project sample rate choice could not be resolved".into());
        }
        // `truncated_import::admit` unwraps every recovery at receipt.
        Request::RecoverTruncatedImport(_) => {
            error = Some("truncated import recovery was not admitted".into());
        }

        Request::RestartRecovery(intent) => {
            let result =
                if device_gesture.snapshot().is_some() || tuning_gesture.snapshot().is_some() {
                    Err("finish the active value edit before restarting recovery".into())
                } else if let Some(untitled) = recovery.as_deref() {
                    recovery_tracker.restart(
                        recovery::RecoveryContext {
                            session: &session,
                            base: media_base.as_deref(),
                            document: document.as_ref(),
                            untitled,
                            journal: &mut journal,
                            generation: project_generation,
                        },
                        revision,
                        intent,
                    )
                } else {
                    Err("automatic recovery is not enabled".into())
                };
            match result {
                Ok(()) => {
                    status =
                        Some("automatic recovery restarted · previous journal preserved".into())
                }
                Err(reason) => error = Some(reason),
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
            let effective_revision = if expected_revision == revision_before_request {
                revision
            } else {
                expected_revision
            };
            let (result, prepared) = run_tuning_library_operation(
                &mut session,
                TuningLibraryContext {
                    request_generation: expected_generation,
                    request_revision: effective_revision,
                    operation_id,
                    current_generation: project_generation,
                    current_revision: revision,
                },
                operation,
                |session, command| {
                    resident_audio::execute_command(
                        session,
                        command,
                        compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                        hex_engine::AudioSourceContext {
                            project_generation,
                            editor_revision: revision.saturating_add(1),
                        },
                        &mut events,
                        &mut resident_snapshot,
                        export_lane.admission_with_resident_owners(
                            compiled.as_ref().map(|(_, prepared)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: current_sources.as_ref(),
                                    prepared: Some(prepared),
                                }
                            }),
                            resident_seek.as_ref(),
                            resident_preview.as_ref(),
                        ),
                    )
                    .map(|(_, prepared)| prepared)
                },
            );
            tuning_library = Some(result);
            if let Some(prepared) = prepared {
                unsaved = true;
                replacement_revision = Some(prepared);
            }
        }

        Request::RelinkMedia(relinks) => {
            let count = relinks.len();
            let result = if events.resident_publication_enabled() {
                events
                    .resident_audio()
                    .ok_or(SessionError::Invalid("native worker unavailable"))
                    .and_then(|sink| {
                        resident_audio::relink_files(
                            &mut session,
                            relinks,
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision.saturating_add(1),
                            },
                            sink,
                            export_lane.admission_with_resident_owners(
                                compiled.as_ref().map(|(_, prepared)| {
                                    hex_engine::native_device_resources::ControlSamplerSelection {
                                        sources: current_sources.as_ref(),
                                        prepared: Some(prepared),
                                    }
                                }),
                                resident_seek.as_ref(),
                                resident_preview.as_ref(),
                            ),
                            &mut resident_snapshot,
                        )
                    })
            } else {
                let allowance = session
                    .max_resident_frames()
                    .saturating_sub(session.resident_frames());
                resident_audio::decode_relinks(&session, relinks, allowance).and_then(|prepared| {
                    let expected = session.project().clone();
                    session.relink_offline_media_revision(
                        prepared,
                        &expected,
                        compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                    )
                })
            };
            match result {
                Ok(prepared) => {
                    unsaved = true;
                    replacement_revision = Some(prepared);
                    status = Some(format!("relinked {count} files"));
                }
                Err(reason) => error = Some(reason.to_string()),
            }
        }

        Request::NewWithAudioMode { mode, reservation } => {
            open_complete = true;
            let candidate = Session::new_with_audio_mode_in_overview_domain(
                events
                    .default_project_sample_rate()
                    .unwrap_or_else(|| session.project().sample_rate()),
                history_capacity,
                session.max_resident_frames(),
                mode,
                reservation.or_else(|| {
                    (mode == DocumentAudioMode::Resident)
                        .then(|| events.ordinary_source_reservation())
                        .flatten()
                }),
                session.overview_budget.clone(),
            )
            .and_then(|candidate| match session.native_reservation {
                Some(limits) => candidate.with_native_reservation(limits),
                None => Ok(candidate),
            })
            .and_then(|candidate| {
                project_rate::new_following_interface(candidate, history_capacity)
            })
            .map(|candidate| candidate.with_shutdown(Arc::clone(&session.shutdown)))
            .map_err(|reason| reason.to_string());
            let current = compiled.as_ref().map(|(_, p)| {
                hex_engine::native_device_resources::ControlSamplerSelection {
                    sources: current_sources.as_ref(),
                    prepared: Some(p.as_ref()),
                }
            });
            let admission = export_lane.admission_with_resident_owners(
                current,
                resident_seek.as_ref(),
                resident_preview.as_ref(),
            );
            match candidate.and_then(|candidate| {
                let prepared = resident_audio::prepare_document_replacement(
                    &candidate,
                    &session,
                    hex_engine::AudioSourceContext {
                        project_generation,
                        editor_revision: revision,
                    },
                    &mut events,
                    resident_snapshot.as_ref(),
                    admission,
                )?;
                Ok((candidate, prepared))
            }) {
                Ok((candidate, prepared)) => {
                    // All fallible construction/admission is complete. These fixed moves
                    // immediately adopt the exact pending candidate and trigger teardown.
                    session = candidate;
                    resident_audio::adopt_document_snapshot(
                        &mut resident_snapshot,
                        prepared.resident,
                    );
                    journal = None;
                    media_base = None;
                    document = None;
                    unsaved = false;
                    project_replaced = true;
                    replacement_revision = Some(prepared.prepared);
                    replacement_sources = Some(prepared.sources);
                    status = Some("new project".into());
                }
                Err(reason) => error = Some(reason),
            }
        }

        request @ (Request::Open(_)
        | Request::OpenWith { .. }
        | Request::OpenWithAudioMode { .. }) => {
            open_complete = true;
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
                    flow = ControlFlow::Break(());
                    return (
                        WorkerDispatch {
                            session,
                            history_capacity,
                            requests,
                            document_intent,
                            events,
                            export_lane,
                            recovery,
                            resident_seek,
                            revision,
                            unsaved,
                            range_clipboard,
                            track_clipboard,
                            device_clipboard,
                            media_base,
                            document,
                            journal,
                            recovery_tracker,
                            midi_preview,
                            project_generation,
                            state,
                            keyboard_activation,
                            keyboard_tuning_hint,
                            audition_activation,
                            live_expression,
                            compiled,
                            current_sources,
                            resident_snapshot,
                            resident_preview,
                            device_gesture,
                            tuning_gesture,
                            pending_sampler_parameter,
                            pending_plugin_effect,
                            pending_device_insert,
                            pending_stopped_restore,
                            native_automation_take,
                            plugin_recording,
                            request_document_intent,
                            defaults_root,
                            import_decode,
                            device_insert_proposal,
                            device_insert_proposal_accepted,
                            replacing_catalog_instrument,
                            time_range_copy,
                            seek_submitted,
                            request_complete,
                            plugin_effect_complete,
                            device_insert_confirmation,
                            device_insert_complete,
                            plugin_parameters,
                            plugin_parameter_conversion,
                            sampler_edit_complete,
                            midi_take_complete,
                            computer_keyboard_complete,
                            tuning_audition_complete,
                            live_expression_complete,
                            computer_keyboard_playback,
                            tuning_input_publication,
                            audio_batch_complete,
                            audio_batch_failure,
                            rate_transition,
                            open_complete,
                            save_complete,
                            save_confirmation_required,
                            project_replaced,
                            replacement_revision,
                            installed_verified_source,
                            replacement_sources,
                            comparison_changed,
                            created,
                            revision_before_request,
                            duplicate_complete,
                            restore_complete,
                            stopped_playback_restore_complete,
                            restore_prepared,
                            status,
                            error,
                            range_copy_source_unavailable,
                            export_completion,
                            export_job,
                            gesture_ack,
                            tuning_gesture_ack,
                            tuning_gesture_request,
                            tuning_library,
                            gesture_protocol_ack,
                            gesture_request,
                            preview_prepared,
                            preview_project,
                            cut_clipboard,
                            cut_device_clipboard,
                            adopted_device_draft,
                            failed_plugin_adoption,
                            failed_plugin_publication_confirmed,
                            barrier,
                            comparison_binding,
                            private_sampler,
                        },
                        flow,
                    );
                }
            };
            let current = compiled.as_ref().map(|(_, p)| {
                hex_engine::native_device_resources::ControlSamplerSelection {
                    sources: current_sources.as_ref(),
                    prepared: Some(p.as_ref()),
                }
            });
            let admission = export_lane.admission_with_resident_owners(
                current,
                resident_seek.as_ref(),
                resident_preview.as_ref(),
            );
            let candidate = resident_audio::open_document(
                &session,
                history_capacity,
                &source,
                &relinks,
                mode,
                reservation,
                &mut events,
                resident_snapshot.as_ref(),
                admission,
            );
            match candidate {
                Ok((candidate, candidate_base, candidate_document, dirty)) => {
                    let compiled_candidate = resident_audio::prepare_document_replacement(
                        &candidate,
                        &session,
                        hex_engine::AudioSourceContext {
                            project_generation,
                            editor_revision: revision,
                        },
                        &mut events,
                        resident_snapshot.as_ref(),
                        admission,
                    );
                    match compiled_candidate {
                        Ok(snapshot) => {
                            journal = None;
                            session = candidate;
                            resident_audio::adopt_document_snapshot(
                                &mut resident_snapshot,
                                snapshot.resident,
                            );
                            media_base = Some(candidate_base);
                            document = candidate_document;
                            unsaved = dirty;
                            project_replaced = true;
                            replacement_revision = Some(snapshot.prepared);
                            replacement_sources = Some(snapshot.sources);
                            status = Some(
                                if dirty {
                                    "project recovered or relinked · save to keep changes"
                                } else {
                                    "project opened"
                                }
                                .into(),
                            );
                        }
                        Err(reason) => error = Some(reason),
                    }
                }
                Err(reason) => error = Some(reason),
            }
        }

        request @ (Request::Undo | Request::Redo)
            if events.resident_publication_enabled()
                && session.audio_mode() == DocumentAudioMode::Resident
                && session
                    .editor
                    .next_history_requires_stopped_bounce(matches!(&request, Request::Redo)) =>
        {
            let context = hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision.saturating_add(1),
            };
            let admission = export_lane.admission_with_resident_owners(
                compiled.as_ref().map(|(_, p)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: current_sources.as_ref(),
                        prepared: Some(p),
                    }
                }),
                resident_seek.as_ref(),
                resident_preview.as_ref(),
            );
            let prefence = events.take_stopped_fence(hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision,
            });
            let result = events
                .resident_audio()
                .ok_or(SessionError::Invalid("native worker unavailable"))
                .and_then(|sink| {
                    resident_audio::bounce_history(
                        &mut session,
                        matches!(request, Request::Redo),
                        context,
                        sink,
                        admission,
                        &mut resident_snapshot,
                        prefence,
                    )
                });
            match result {
                Ok(Some(prepared)) => {
                    unsaved = true;
                    replacement_revision = Some(prepared);
                    status = Some("bounce history committed".into());
                }
                Ok(None) => status = Some("no history change".into()),
                Err(reason) => error = Some(reason.to_string()),
            }
        }

        request @ (Request::Undo | Request::Redo)
            if events.resident_publication_enabled()
                && session.audio_mode() == DocumentAudioMode::Resident
                && session
                    .section_history_requires_publication(matches!(&request, Request::Redo)) =>
        {
            let context = hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision.saturating_add(1),
            };
            let admission = export_lane.admission_with_resident_owners(
                compiled.as_ref().map(|(_, prepared)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: current_sources.as_ref(),
                        prepared: Some(prepared),
                    }
                }),
                resident_seek.as_ref(),
                resident_preview.as_ref(),
            );
            match arrangement_sections::history(
                &mut session,
                matches!(request, Request::Redo),
                context,
                &mut events,
                &mut resident_snapshot,
                admission,
            ) {
                Ok(Some(prepared)) => {
                    unsaved = true;
                    replacement_revision = Some(prepared);
                    status = Some("section history committed".into());
                }
                Ok(None) => status = Some("no history change".into()),
                Err(reason) => error = Some(reason.to_string()),
            }
        }

        request @ (Request::Undo | Request::Redo)
            if events.resident_publication_enabled()
                && session.audio_mode() == DocumentAudioMode::Resident
                && resident_snapshot.is_some()
                && session.section_history_marker_only(matches!(&request, Request::Redo)) =>
        {
            let admission = export_lane.admission_with_resident_owners(
                compiled.as_ref().map(|(_, prepared)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: current_sources.as_ref(),
                        prepared: Some(prepared),
                    }
                }),
                resident_seek.as_ref(),
                resident_preview.as_ref(),
            );
            match arrangement_sections::marker_history(
                &mut session,
                matches!(request, Request::Redo),
                hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision.saturating_add(1),
                },
                &mut events,
                &mut resident_snapshot,
                admission,
            ) {
                Ok(Some(prepared)) => {
                    unsaved = true;
                    replacement_revision = Some(prepared);
                    status = Some("section history committed".into());
                }
                Ok(None) => status = Some("no history change".into()),
                Err(reason) => error = Some(reason.to_string()),
            }
        }

        request @ (Request::Undo | Request::Redo)
            if events.resident_publication_enabled()
                && session.audio_mode() == DocumentAudioMode::Resident =>
        {
            let redo = matches!(request, Request::Redo);
            let context = hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision,
            };
            let admission = export_lane.admission_with_resident_owners(
                compiled.as_ref().map(|(_, prepared)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: current_sources.as_ref(),
                        prepared: Some(prepared),
                    }
                }),
                resident_seek.as_ref(),
                resident_preview.as_ref(),
            );
            let mut pending_history = None;
            let result = events
                .resident_audio()
                .ok_or(SessionError::Invalid("native worker unavailable"))
                .and_then(|sink| {
                    pending_history = session.stage_resident_graph_history(
                        redo,
                        context,
                        sink,
                        admission,
                        resident_snapshot.as_ref(),
                        request_document_intent,
                    )?;
                    if pending_history.is_some() {
                        return Ok(None);
                    }
                    session.history_resident_snapshot(
                        redo,
                        hex_engine::AudioSourceContext {
                            editor_revision: revision.saturating_add(1),
                            ..context
                        },
                        sink,
                        admission,
                        resident_snapshot.as_ref(),
                    )
                });
            match result {
                Ok(Some(snapshot)) => {
                    unsaved = true;
                    replacement_revision = Some(Arc::clone(&snapshot.prepared));
                    resident_snapshot = Some(snapshot);
                    status = Some("history committed".into());
                }
                Ok(None) if pending_history.is_some() => {
                    pending_device_insert = pending_history;
                    request_complete = false;
                    status = Some("history pending · waiting for audio".into());
                }
                Ok(None) => status = Some("no history change".into()),
                Err(reason) => error = Some(reason.to_string()),
            }
        }

        request @ (Request::Undo | Request::Redo)
            if session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                && session.has_verified_hosted_history(matches!(request, Request::Redo))
                && events
                    .resident_audio()
                    .is_some_and(|sink| sink.confirms_verified_device_inserts()) =>
        {
            let result = events
                .resident_audio()
                .ok_or(SessionError::Invalid("native worker unavailable"))
                .and_then(|sink| {
                    session.stage_verified_hosted_history(
                        matches!(request, Request::Redo),
                        &state,
                        compiled.as_ref().map(|(_, prepared)| prepared.as_ref()),
                        sink,
                        request_document_intent,
                    )
                });
            match result {
                Ok(staged) => {
                    pending_device_insert = Some(staged);
                    request_complete = false;
                    status = Some("history pending · waiting for audio".into());
                }
                Err(reason) => error = Some(reason.to_string()),
            }
        }

        request @ (Request::Undo | Request::Redo) => {
            let redo = matches!(request, Request::Redo);
            let group_history = if session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                && let Some(sink) = events.resident_audio()
                && sink.confirms_verified_device_inserts()
            {
                session.stage_verified_group_history(
                    redo,
                    &state,
                    compiled.as_ref().map(|(_, prepared)| prepared.as_ref()),
                    sink,
                    request_document_intent,
                )
            } else {
                Ok(None)
            };
            match group_history {
                Ok(Some(staged)) => {
                    pending_device_insert = Some(staged);
                    request_complete = false;
                    status = Some("history pending · waiting for audio".into());
                }
                Err(reason) => error = Some(reason.to_string()),
                Ok(None) => {
                    let previous = compiled
                        .as_ref()
                        .filter(|_| comparison_binding.is_some())
                        .map(|(_, artifact)| artifact);
                    let finish =
                        |source: &Project, target: &Project, prepared: &Arc<PreparedRevision>| {
                            prepare_tuning_input_publication(
                                source,
                                target,
                                prepared,
                                project_generation,
                                revision,
                                keyboard_activation.as_ref(),
                                keyboard_tuning_hint.as_ref(),
                                audition_activation.as_ref(),
                                false,
                            )
                        };
                    let history_result = if session.audio_mode()
                        == DocumentAudioMode::VerifiedStreaming
                        && let Some(sink) = events.resident_audio()
                    {
                        session.comparison_history_verified(
                            redo,
                            previous,
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision.saturating_add(1),
                            },
                            sink,
                            finish,
                        )
                    } else {
                        session.comparison_history(redo, previous, finish)
                    };
                    match history_result {
                        Ok(Some(outcome)) => {
                            unsaved = true;
                            if outcome.metadata_only {
                                comparison_changed = true;
                            } else {
                                replacement_revision = Some(outcome.prepared);
                            }
                            tuning_input_publication = outcome.completed.flatten();
                            status = Some(
                                if redo {
                                    "redo committed"
                                } else {
                                    "undo committed"
                                }
                                .into(),
                            );
                        }
                        Ok(None) => {
                            status = Some(
                                if redo {
                                    "nothing to redo"
                                } else {
                                    "nothing to undo"
                                }
                                .into(),
                            )
                        }
                        Err(reason) => {
                            error = Some(hex_storage::text::lowercase_outside_verbatim(
                                &reason.to_string(),
                            ))
                        }
                    }
                }
            }
        }

        request @ (Request::SaveNew(_) | Request::SaveProjectAs { .. }) => {
            save_complete = true;
            let (path, folder_layout) = match request {
                Request::SaveNew(path) => (path, None),
                Request::SaveProjectAs {
                    path,
                    extension_added,
                } => (path, Some(extension_added)),
                _ => {
                    flow = ControlFlow::Break(());
                    return (
                        WorkerDispatch {
                            session,
                            history_capacity,
                            requests,
                            document_intent,
                            events,
                            export_lane,
                            recovery,
                            resident_seek,
                            revision,
                            unsaved,
                            range_clipboard,
                            track_clipboard,
                            device_clipboard,
                            media_base,
                            document,
                            journal,
                            recovery_tracker,
                            midi_preview,
                            project_generation,
                            state,
                            keyboard_activation,
                            keyboard_tuning_hint,
                            audition_activation,
                            live_expression,
                            compiled,
                            current_sources,
                            resident_snapshot,
                            resident_preview,
                            device_gesture,
                            tuning_gesture,
                            pending_sampler_parameter,
                            pending_plugin_effect,
                            pending_device_insert,
                            pending_stopped_restore,
                            native_automation_take,
                            plugin_recording,
                            request_document_intent,
                            defaults_root,
                            import_decode,
                            device_insert_proposal,
                            device_insert_proposal_accepted,
                            replacing_catalog_instrument,
                            time_range_copy,
                            seek_submitted,
                            request_complete,
                            plugin_effect_complete,
                            device_insert_confirmation,
                            device_insert_complete,
                            plugin_parameters,
                            plugin_parameter_conversion,
                            sampler_edit_complete,
                            midi_take_complete,
                            computer_keyboard_complete,
                            tuning_audition_complete,
                            live_expression_complete,
                            computer_keyboard_playback,
                            tuning_input_publication,
                            audio_batch_complete,
                            audio_batch_failure,
                            rate_transition,
                            open_complete,
                            save_complete,
                            save_confirmation_required,
                            project_replaced,
                            replacement_revision,
                            installed_verified_source,
                            replacement_sources,
                            comparison_changed,
                            created,
                            revision_before_request,
                            duplicate_complete,
                            restore_complete,
                            stopped_playback_restore_complete,
                            restore_prepared,
                            status,
                            error,
                            range_copy_source_unavailable,
                            export_completion,
                            export_job,
                            gesture_ack,
                            tuning_gesture_ack,
                            tuning_gesture_request,
                            tuning_library,
                            gesture_protocol_ack,
                            gesture_request,
                            preview_prepared,
                            preview_project,
                            cut_clipboard,
                            cut_device_clipboard,
                            adopted_device_draft,
                            failed_plugin_adoption,
                            failed_plugin_publication_confirmed,
                            barrier,
                            comparison_binding,
                            private_sampler,
                        },
                        flow,
                    );
                }
            };
            // Earlier source intent may cancel the edit ahead of Save,
            // but FIFO still saves its resulting canonical project.
            // Only a new intent during this capture cancels this Save.
            let save_cancellation = RequestCancellation::new(
                Arc::clone(&session.shutdown),
                Arc::clone(&document_intent),
                document_intent.load(Ordering::Acquire),
            );
            match plugin_state_save::project_for_save_with_capture(
                &session,
                media_base.as_deref(),
                hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                },
                &save_cancellation,
                events.sampler_edit_audio(),
            ) {
                Ok(project) => match absolute_document_path(&path) {
                    Ok(path)
                        if document
                            .as_ref()
                            .is_some_and(|handle| is_open_document(handle, &path)) =>
                    {
                        error = Some(
                            "save as chose the open project · use save to update it, or choose a new name"
                                .into(),
                        );
                    }
                    Ok(path) => match if let Some(extension_added) = folder_layout {
                        project_save::save_as(
                            path,
                            extension_added,
                            project,
                            recovery.as_deref(),
                            &save_cancellation,
                        )
                    } else {
                        save_document_as(path, &project)
                    } {
                        Ok((handle, commit)) => {
                            document = Some(handle);
                            unsaved = commit.dirty;
                            error = commit.error;
                            state = view_state_bound(
                                &session,
                                Arc::clone(&midi_preview),
                                revision,
                                project_generation,
                                unsaved,
                                document.as_ref().map(|handle| handle.path.as_path()),
                                state.canonical_audio_source.as_ref().map(|source| {
                                    (Arc::clone(source), state.comparison_audio.clone())
                                }),
                            );
                            status = Some(commit.status);
                        }
                        Err(reason) => error = Some(reason),
                    },
                    Err(reason) => error = Some(reason),
                },
                Err(reason) => error = Some(reason),
            }
        }

        request @ (Request::SaveCurrent | Request::SaveCurrentWithFormatUpgrade { .. }) => {
            save_complete = true;
            // Earlier source intent may cancel the edit ahead of Save,
            // but FIFO still saves its resulting canonical project.
            // Only a new intent during this capture cancels this Save.
            let save_cancellation = RequestCancellation::new(
                Arc::clone(&session.shutdown),
                Arc::clone(&document_intent),
                document_intent.load(Ordering::Acquire),
            );
            let consent = match &request {
                Request::SaveCurrentWithFormatUpgrade {
                    project_generation: expected,
                    path,
                } => {
                    *expected == project_generation
                        && document.as_ref().is_some_and(|handle| handle.path == *path)
                }
                _ => false,
            };
            let stale_consent =
                matches!(request, Request::SaveCurrentWithFormatUpgrade { .. }) && !consent;
            let save_project = if stale_consent {
                Err("project changed · format upgrade cancelled".to_owned())
            } else if document.is_none() {
                Err("cannot save current project before it has a file path".to_owned())
            } else {
                plugin_state_save::project_for_save_with_capture(
                    &session,
                    media_base.as_deref(),
                    hex_engine::AudioSourceContext {
                        project_generation,
                        editor_revision: revision,
                    },
                    &save_cancellation,
                    events.sampler_edit_audio(),
                )
            };
            let image = save_project.and_then(|project| {
                let handle = document.as_ref().ok_or("project has no file path")?;
                project_save::Image::for_document(project, &handle.path, recovery.as_deref())
            });
            match (document.as_mut(), image) {
                (None, _) => {
                    error = Some("cannot save current project before it has a file path".into())
                }
                (_, Err(reason)) => error = Some(reason),
                (Some(_), Ok(_)) if stale_consent => {
                    error = Some("project changed · format upgrade cancelled".into());
                }
                (Some(handle), Ok(image))
                    if !consent && handle.token.requires_schema_upgrade_for(&image.project) =>
                {
                    save_confirmation_required = true;
                    error = Some("saving makes this file incompatible with older hex readers; use save as to preserve it, or explicitly confirm overwrite".into());
                }
                (Some(handle), Ok(image)) => {
                    match hex_storage::prepare_replacement_within(
                        &handle.path,
                        &handle.token,
                        &image.project,
                        events.legacy_reader_limit(),
                    ) {
                        Ok(prepared) if !consent && prepared.requires_confirmation() => {
                            save_confirmation_required = true;
                            error = Some("saving makes this file incompatible with older hex readers; use save as to preserve it, or explicitly confirm overwrite".into());
                        }
                        Ok(prepared) => {
                            match image
                                .collect(&save_cancellation)
                                .and_then(|()| prepared.publish().map_err(project_save::display))
                            {
                                Ok(outcome) => {
                                    let commit = apply_current_save_outcome(handle, outcome);
                                    unsaved = commit.dirty;
                                    status = Some(commit.status);
                                    error = commit.error;
                                    state = view_state_bound(
                                        &session,
                                        Arc::clone(&midi_preview),
                                        revision,
                                        project_generation,
                                        unsaved,
                                        document.as_ref().map(|current| current.path.as_path()),
                                        state.canonical_audio_source.as_ref().map(|source| {
                                            (Arc::clone(source), state.comparison_audio.clone())
                                        }),
                                    );
                                }
                                Err(reason) => error = Some(reason),
                            }
                        }
                        Err(reason) => {
                            error = Some(hex_storage::text::lowercase_outside_verbatim(
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
                let job = if audio_sink::requires_plugin_profile(session.project())
                    && export_destination_is_original_media(
                        session.project(),
                        &path,
                        media_base.as_deref(),
                    ) {
                    Err("export destination is original media; choose another path".to_owned())
                } else {
                    match compiled.as_ref() {
                        Some((compiled_revision, prepared)) if *compiled_revision == revision => {
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
                                revision,
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
                    if session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                        || job.prepared.arrangement().contains_streamed_sources()
                    {
                        job.streamed = Some(streamed_export::StreamedExportJob::capture(
                            &session,
                            &state.project,
                            project_generation,
                            revision,
                            events.streamed_exporter(),
                        )?);
                    }
                    Ok(job)
                });
                match job {
                    Ok(job) => {
                        status = Some("export queued".into());
                        export_job = Some(job);
                    }
                    Err(reason) => {
                        export_completion = Some(ExportEvent {
                            revision,
                            path,
                            result: Err(reason.into()),
                        })
                    }
                }
            }
        }

        _ => {
            error = Some("unexpected worker dispatch route".into());
        }
    }
    let ctx = WorkerDispatch {
        session,
        history_capacity,
        requests,
        document_intent,
        events,
        export_lane,
        recovery,
        resident_seek,
        revision,
        unsaved,
        range_clipboard,
        track_clipboard,
        device_clipboard,
        media_base,
        document,
        journal,
        recovery_tracker,
        midi_preview,
        project_generation,
        state,
        keyboard_activation,
        keyboard_tuning_hint,
        audition_activation,
        live_expression,
        compiled,
        current_sources,
        resident_snapshot,
        resident_preview,
        device_gesture,
        tuning_gesture,
        pending_sampler_parameter,
        pending_plugin_effect,
        pending_device_insert,
        pending_stopped_restore,
        native_automation_take,
        plugin_recording,
        request_document_intent,
        defaults_root,
        import_decode,
        device_insert_proposal,
        device_insert_proposal_accepted,
        replacing_catalog_instrument,
        time_range_copy,
        seek_submitted,
        request_complete,
        plugin_effect_complete,
        device_insert_confirmation,
        device_insert_complete,
        plugin_parameters,
        plugin_parameter_conversion,
        sampler_edit_complete,
        midi_take_complete,
        computer_keyboard_complete,
        tuning_audition_complete,
        live_expression_complete,
        computer_keyboard_playback,
        tuning_input_publication,
        audio_batch_complete,
        audio_batch_failure,
        rate_transition,
        open_complete,
        save_complete,
        save_confirmation_required,
        project_replaced,
        replacement_revision,
        installed_verified_source,
        replacement_sources,
        comparison_changed,
        created,
        revision_before_request,
        duplicate_complete,
        restore_complete,
        stopped_playback_restore_complete,
        restore_prepared,
        status,
        error,
        range_copy_source_unavailable,
        export_completion,
        export_job,
        gesture_ack,
        tuning_gesture_ack,
        tuning_gesture_request,
        tuning_library,
        gesture_protocol_ack,
        gesture_request,
        preview_prepared,
        preview_project,
        cut_clipboard,
        cut_device_clipboard,
        adopted_device_draft,
        failed_plugin_adoption,
        failed_plugin_publication_confirmed,
        barrier,
        comparison_binding,
        private_sampler,
    };
    (ctx, flow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tuning_library_directory_points_under_scales() {
        if let Ok(path) = tuning_library_directory() {
            assert!(path.ends_with("scales"));
        }
    }
}
