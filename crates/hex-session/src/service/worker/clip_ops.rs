//! Clip, media import, arrangement edit, time-range and track-clipboard worker operations.
use super::*;
use std::ops::ControlFlow;

/// Decode and convert `path` for a resident import, bounded by the remaining
/// resident frame budget. Callers first offer the file to
/// [`Session::unchanged_resident_import`], which needs no decode.
pub(crate) fn decode_resident_member(
    session: &Session,
    path: PathBuf,
    decode: &mut truncated_import::ImportDecode,
) -> Result<PreparedImport, String> {
    let remaining_frames = session
        .max_resident_frames()
        .saturating_sub(session.resident_frames());
    let output_limit = usize::try_from(remaining_frames)
        .map_err(|_| "resident frame limit exceeds addressable range".to_owned())?;
    if output_limit == 0 {
        return Err("resident audio frame limit reached".into());
    }
    let decoded = decode
        .decode(path, Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT))
        .map_err(|error| {
            hex_storage::text::lowercase_outside_verbatim(&truncated_import::decode_error_text(
                error,
            ))
        })?;
    PreparedImport::at_rate(
        decoded,
        session.project().sample_rate(),
        ResampleLimits {
            max_source_frames: IMPORT_FRAME_LIMIT,
            max_output_frames: output_limit,
        },
    )
    .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))
}

/// Import `path` onto a new track. An unchanged file already resident reuses
/// its asset without decoding; see `import_reuse`.
pub(crate) fn import(
    session: &mut Session,
    path: PathBuf,
    previous: Option<&PreparedRevision>,
    admission: sampler_export::Admission<'_>,
    decode: &mut truncated_import::ImportDecode,
) -> Result<Arc<PreparedRevision>, String> {
    let path = absolute_document_path(&path)?;
    let reused = session.unchanged_resident_import(&path);
    if reused.is_none()
        && let Some(source) = decode
            .plan(session, &path, None)
            .map_err(|e| e.to_string())?
    {
        return session
            .import_verified_revision(
                source,
                VerifiedImportDestination::NewTrack {
                    before: None,
                    start: 0,
                },
                previous,
            )
            .map(|(_, prepared)| prepared)
            .map_err(|e| e.to_string());
    }

    let prepared = match reused {
        Some(prepared) => prepared,
        None => decode_resident_member(session, path, decode)?,
    };
    admission
        .check_legacy_stage(session, std::slice::from_ref(&prepared))
        .map_err(|e| e.to_string())?;
    session
        .import_prepared_revision(prepared, previous)
        .map(|(_, revision)| revision)
        .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn import_to_track(
    session: &mut Session,
    path: PathBuf,
    track: hex_project::TrackId,
    start: u64,
    previous: Option<&PreparedRevision>,
    overwrite: bool,
    admission: sampler_export::Admission<'_>,
    decode: &mut truncated_import::ImportDecode,
) -> Result<Arc<PreparedRevision>, String> {
    let path = absolute_document_path(&path)?;
    let reused = session.unchanged_resident_import(&path);
    if reused.is_none()
        && let Some(source) = decode
            .plan(session, &path, None)
            .map_err(|e| e.to_string())?
    {
        return session
            .import_verified_revision(
                source,
                VerifiedImportDestination::Track {
                    track,
                    start,
                    overwrite,
                },
                previous,
            )
            .map(|(_, prepared)| prepared)
            .map_err(|e| e.to_string());
    }

    let prepared = match reused {
        Some(prepared) => prepared,
        None => decode_resident_member(session, path, decode)?,
    };
    admission
        .check_legacy_stage(session, std::slice::from_ref(&prepared))
        .map_err(|e| e.to_string())?;
    let imported = if overwrite {
        session.import_prepared_audio_clip_with_overwrite_revision(prepared, track, start, previous)
    } else {
        session.import_prepared_audio_clip_revision(prepared, track, start, previous)
    };
    imported
        .map(|(_, revision)| revision)
        .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))
}

pub(crate) fn import_to_new_track(
    session: &mut Session,
    path: PathBuf,
    before_track: Option<hex_project::TrackId>,
    start: u64,
    previous: Option<&PreparedRevision>,
    admission: sampler_export::Admission<'_>,
    decode: &mut truncated_import::ImportDecode,
) -> Result<Arc<PreparedRevision>, String> {
    let path = absolute_document_path(&path)?;
    let reused = session.unchanged_resident_import(&path);
    if reused.is_none()
        && let Some(source) = decode
            .plan(session, &path, None)
            .map_err(|e| e.to_string())?
    {
        return session
            .import_verified_revision(
                source,
                VerifiedImportDestination::NewTrack {
                    before: before_track,
                    start,
                },
                previous,
            )
            .map(|(_, prepared)| prepared)
            .map_err(|e| e.to_string());
    }

    let project = session.project();
    if project.tracks().len() >= hex_project::MAX_TRACKS {
        return Err("track limit reached".into());
    }
    if before_track.is_some_and(|track| project.track(track).is_none()) {
        return Err("track insertion anchor no longer exists".into());
    }
    if reused.is_none() && project.assets().count() >= hex_project::MAX_ASSETS {
        return Err("asset limit reached".into());
    }
    let prepared = match reused {
        Some(prepared) => prepared,
        None => decode_resident_member(session, path, decode)?,
    };
    admission
        .check_legacy_stage(session, std::slice::from_ref(&prepared))
        .map_err(|e| e.to_string())?;
    session
        .import_prepared_audio_track_at_revision(prepared, before_track, start, previous)
        .map(|(_, revision)| revision)
        .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn import_to_new_track_with_gain(
    session: &mut Session,
    path: PathBuf,
    before_track: Option<hex_project::TrackId>,
    start: u64,
    gain: f32,
    previous: Option<&PreparedRevision>,
    admission: sampler_export::Admission<'_>,
    decode: &mut truncated_import::ImportDecode,
) -> Result<Arc<PreparedRevision>, String> {
    let options = crate::import_batch::AudioImportOptions {
        new_track_gain: Some(gain),
        overwrite: false,
        ..Default::default()
    };
    options.validate().map_err(|error| error.to_string())?;
    let path = absolute_document_path(&path)?;
    let destination = AudioBatchDestination::NewTracks {
        before_track,
        start,
    };
    let reused = session.unchanged_resident_import(&path);
    if reused.is_none()
        && let Some(source) = decode
            .plan(session, &path, None)
            .map_err(|e| e.to_string())?
    {
        let expected = session.project().clone();
        return session
            .import_verified_batch_revision(
                vec![source],
                destination,
                &expected,
                previous,
                options,
                || Ok(()),
                crate::import_batch::prepare_asset_free,
            )
            .map(|(_, prepared)| prepared)
            .map_err(|e| e.to_string());
    }
    let prepared = match reused {
        Some(prepared) => prepared,
        None => decode_resident_member(session, path, decode)?,
    };
    admission
        .check_legacy_stage(session, std::slice::from_ref(&prepared))
        .map_err(|e| e.to_string())?;
    let expected = session.project().clone();
    session
        .import_prepared_batch_revision_using(
            vec![prepared],
            destination,
            &expected,
            previous,
            options,
            || Ok(()),
            crate::import_batch::prepare_asset_free,
        )
        .map(|(_, prepared)| prepared)
        .map_err(|e| hex_storage::text::lowercase_outside_verbatim(&e.to_string()))
}

pub(crate) fn import_to_session(
    session: &mut Session,
    path: PathBuf,
    track: hex_project::TrackId,
    scene: hex_project::SceneId,
    previous: Option<&PreparedRevision>,
    admission: sampler_export::Admission<'_>,
    decode: &mut truncated_import::ImportDecode,
) -> Result<Arc<PreparedRevision>, String> {
    let path = absolute_document_path(&path)?;
    let reused = session.unchanged_resident_import(&path);
    if reused.is_none()
        && let Some(source) = decode
            .plan(session, &path, None)
            .map_err(|e| e.to_string())?
    {
        return session
            .import_verified_revision(
                source,
                VerifiedImportDestination::Session { track, scene },
                previous,
            )
            .map(|(_, prepared)| prepared)
            .map_err(|e| e.to_string());
    }

    let prepared = match reused {
        Some(prepared) => prepared,
        None => decode_resident_member(session, path, decode)?,
    };
    admission
        .check_legacy_stage(session, std::slice::from_ref(&prepared))
        .map_err(|e| e.to_string())?;
    session
        .import_session_audio_clip_revision(prepared, track, scene, previous)
        .map(|(_, revision)| revision)
        .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))
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
        rate_transition,
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
        Request::ReorderTracks { .. } | Request::ReparentTracks { .. } => {
            error = Some("track reorder target was not resolved".into());
        }

        Request::PaintMidiClip(_) => {
            error = Some("clip paint target was not resolved".into());
        }

        Request::MoveExistingClips { .. } => {
            error = Some("clip move target was not resolved".into());
        }

        Request::CopyArrangementClipToConsoleTrack { .. } => {
            error = Some("console clip target was not resolved".into());
        }

        Request::DuplicateClips { .. }
        | Request::DuplicateAliasClips { .. }
        | Request::DuplicateSessionCell { .. } => {
            error = Some("clip duplication target was not resolved".into());
        }

        Request::GroupClips { .. } | Request::GroupTimeRange { .. } => {
            error = Some("clip grouping target was not resolved".into());
        }

        Request::GroupAudioEvents { .. } => {
            error = Some("audio event grouping target was not resolved".into());
        }

        Request::InsertProjectSample {
            project_generation: _,
            revision: _,
            asset,
            destination,
            new_track_gain,
            warp,
            audition_timing,
        } => {
            let cancellation = RequestCancellation::new(
                Arc::clone(&session.shutdown),
                Arc::clone(&document_intent),
                request_document_intent,
            );
            let mut admit = || {
                if cancellation.is_cancelled() {
                    Err(SessionError::Invalid(
                        "sample insertion canceled · project changed",
                    ))
                } else {
                    Ok(())
                }
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
            let mut imported_resident = None;
            let result = if session.audio_mode() != DocumentAudioMode::Resident {
                Err(SessionError::Invalid(
                    "project sample placement requires resident audio",
                ))
            } else if let Some(sink) = events.resident_audio() {
                session.place_existing_asset_revision_using(
                    crate::import_batch::ProjectSampleImport {
                        asset,
                        destination,
                        new_track_gain,
                        warp,
                    },
                    compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                    &mut admit,
                    |input, admit| {
                        let snapshot = resident_audio::prepare_import_playback(
                            input,
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision.saturating_add(1),
                            },
                            sink,
                            admission,
                            resident_snapshot.as_ref(),
                            match (destination, audition_timing) {
                                (AudioBatchDestination::Track { track, start }, Some(timing)) => {
                                    resident_audio::ImportPlayback::Browser {
                                        track,
                                        start,
                                        timing,
                                    }
                                }
                                _ => resident_audio::ImportPlayback::Normal,
                            },
                            admit,
                        )?;
                        let prepared = Arc::clone(&snapshot.prepared);
                        imported_resident = Some(snapshot);
                        Ok(prepared)
                    },
                )
            } else {
                admission.check_legacy_stage(&session, &[]).and_then(|()| {
                    session.place_existing_asset_revision_using(
                        crate::import_batch::ProjectSampleImport {
                            asset,
                            destination,
                            new_track_gain,
                            warp,
                        },
                        compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                        &mut admit,
                        crate::import_batch::prepare_asset_free,
                    )
                })
            };
            match result {
                Ok((result, prepared)) => {
                    if let Some(snapshot) = imported_resident {
                        resident_snapshot = Some(snapshot);
                    }
                    unsaved = true;
                    created = Some(result);
                    replacement_revision = Some(prepared);
                    status = Some("project sample inserted".into());
                }
                Err(reason) => {
                    error = Some(hex_storage::text::lowercase_outside_verbatim(
                        &reason.to_string(),
                    ))
                }
            }
        }

        Request::ImportAudioBatch {
            paths,
            destination,
            context,
            control,
            options,
            audition_timing,
        } => {
            audio_batch_complete = Some(Arc::clone(&control));
            let audition = match destination {
                AudioBatchDestination::Track { track, start } if paths.len() == 1 => {
                    audition_timing.map(|timing| resident_audio::ImportPlayback::Browser {
                        track,
                        start,
                        timing,
                    })
                }
                _ => None,
            };
            let actual = AudioBatchContext {
                project_generation,
                revision,
                active_gesture: device_gesture.snapshot().is_some(),
            };
            let mut imported_resident = None;
            let result = if context != actual {
                Err(crate::import_batch_decode::AudioBatchError::ContextChanged)
            } else {
                AudioBatchImport::capture_with_control(
                    paths,
                    destination,
                    &session,
                    actual,
                    Arc::clone(&control),
                    options,
                )
                .and_then(|job| {
                    if session.audio_mode() == DocumentAudioMode::Resident
                        && let Some(sink) = events.resident_audio()
                    {
                        let admission = export_lane.admission_with_resident_owners(compiled.as_ref().map(|(_, prepared)| hex_engine::native_device_resources::ControlSamplerSelection { sources: current_sources.as_ref(), prepared: Some(prepared) }), resident_seek.as_ref(), resident_preview.as_ref());
                        let allowance = resident_audio::import_frame_allowance(
                            &session, sink, admission, resident_snapshot.as_ref())?;
                        let job = job.with_pcm_frame_allowance(allowance);
                        return job.run_with_storage_admission_using(
                            &mut session, compiled.as_ref().map(|(_, prepared)| prepared.as_ref()), || actual,
                            |session, prepared| admission.check_legacy_stage(session, prepared),
                            |input, admit| {
                                let snapshot = resident_audio::prepare_import_playback(input,
                                    hex_engine::AudioSourceContext { project_generation, editor_revision: revision.saturating_add(1) }, sink, admission, resident_snapshot.as_ref(), audition.unwrap_or(resident_audio::ImportPlayback::Normal), admit)?;
                                let prepared = Arc::clone(&snapshot.prepared);
                                imported_resident = Some(snapshot);
                                Ok(prepared)
                            });
                    }
                    job.run_with_storage_admission(
                        &mut session,
                        compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                        || actual,
                        |session, prepared| export_lane.admission_with_resident_owners(compiled.as_ref().map(|(_, p)| hex_engine::native_device_resources::ControlSamplerSelection { sources: current_sources.as_ref(), prepared: Some(p) }), resident_seek.as_ref(), resident_preview.as_ref()).check_legacy_stage(session, prepared),
                    )
                })
            };
            match result {
                Ok((_, prepared)) => {
                    if let Some(snapshot) = imported_resident {
                        resident_snapshot = Some(snapshot);
                    }
                    unsaved = true;
                    replacement_revision = Some(prepared);
                    status = Some("audio batch imported · one undo".into());
                }
                Err(reason) => {
                    control.refuse();
                    error = Some(reason.to_string());
                    if let crate::import_batch_decode::AudioBatchError::Member {
                        member,
                        path,
                        source,
                    } = reason
                    {
                        audio_batch_failure = Some(AudioBatchMemberFailure {
                            member,
                            path,
                            explanation: source.to_string(),
                        });
                    }
                }
            }
        }

        Request::Edit(_)
            if failed_plugin_adoption
                && (!events.resident_publication_enabled()
                    || session.audio_mode() != DocumentAudioMode::Resident) =>
        {
            error = Some("vst3 recovery lost its resident publication path".into());
        }

        Request::Edit(command) if device_insert_proposal.is_some() => {
            // Catalog additions and picker replacements are resolved through
            // their existing default/owner validation and gesture barrier.
            // Keep their Created result after the exact install.
            let result = resident_audio::device_insert::submit_browser_edit(
                &mut session,
                command,
                &state,
                project_generation,
                compiled.as_ref().map(|(_, prepared)| prepared.as_ref()),
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
                request_document_intent,
                device_insert_proposal.as_ref(),
            );
            match result {
                Ok(BrowserEdit::Pending(mut staged)) => {
                    staged.publish_created = !replacing_catalog_instrument;
                    staged.proposal_control = device_insert_proposal.take();
                    pending_device_insert = Some(*staged);
                    request_complete = false;
                    status = Some("device edit pending · waiting for audio".into());
                }
                Ok(BrowserEdit::Applied(new, prepared)) => {
                    if session.project() != state.project.as_ref() {
                        if !replacing_catalog_instrument {
                            created = Some(new);
                        }
                        unsaved = true;
                        replacement_revision = Some(prepared);
                    }
                    device_insert_proposal_accepted = true;
                    status = Some(
                        if replacing_catalog_instrument {
                            "device replaced"
                        } else {
                            "device inserted"
                        }
                        .into(),
                    );
                }
                Ok(BrowserEdit::Unchanged) => {
                    device_insert_proposal_accepted = device_insert_proposal
                        .as_ref()
                        .is_some_and(|control| control.begin_commit());
                    status = Some("no change".into());
                }
                Err(reason) => error = Some(reason.to_string()),
            }
        }

        Request::Edit(command)
            if session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                && (matches!(&command, Command::PasteDevices { .. })
                    || resident_audio::device_insert::graph_edit_operation(&command).is_some()
                    || session.should_stage_verified_group_route(
                        &command,
                        compiled.as_ref().map(|(_, prepared)| prepared.as_ref()),
                    ))
                && events
                    .resident_audio()
                    .is_some_and(|sink| sink.confirms_verified_device_inserts()) =>
        {
            let result = events
                .resident_audio()
                .ok_or(SessionError::Invalid("native worker unavailable"))
                .and_then(|sink| {
                    session.stage_verified_graph_edit(
                        command,
                        &state,
                        compiled.as_ref().map(|(_, prepared)| prepared.as_ref()),
                        sink,
                        request_document_intent,
                        device_gesture.snapshot().as_ref(),
                        None,
                    )
                });
            match result {
                Ok(Some(mut staged)) => {
                    staged.cut_devices = cut_device_clipboard.take();
                    staged.cut_tracks = cut_clipboard.take();
                    status = Some(
                        if staged.operation.device() == Some(GraphEditOperation::MoveTrack) {
                            "track move pending · waiting for audio"
                        } else {
                            "device edit pending · waiting for audio"
                        }
                        .into(),
                    );
                    pending_device_insert = Some(staged);
                    request_complete = false;
                }
                Ok(None) => status = Some("no change".into()),
                Err(reason) => error = Some(reason.to_string()),
            }
        }

        Request::Edit(command)
            if events.resident_publication_enabled()
                && session.audio_mode() == DocumentAudioMode::Resident =>
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
            let result = events
                .resident_audio()
                .ok_or(SessionError::Invalid("native worker unavailable"))
                .and_then(|sink| {
                    if resident_audio::device_insert::should_stage_resident_graph_edit(
                        &command, sink,
                    ) || resident_audio::device_insert::stages_main_power_off(
                        session.project(),
                        &command,
                        sink,
                    ) || resident_audio::device_insert::running_return_power_target(
                        session.project(),
                        &command,
                        sink,
                    )
                    .is_some()
                        || resident_audio::device_insert::running_main_power_on(
                            session.project(),
                            &command,
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision,
                            },
                            sink,
                        )
                        .is_some()
                        || resident_audio::device_insert::running_return_power_on(
                            session.project(),
                            &command,
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision,
                            },
                            sink,
                        )
                        .is_some()
                        || resident_audio::device_insert::stopped_hosted_power(
                            session.project(),
                            &command,
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision,
                            },
                            sink,
                        )?
                        .is_some()
                    {
                        let staged = session.stage_resident_device_insert(
                            command,
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision,
                            },
                            sink,
                            admission,
                            resident_snapshot.as_ref(),
                            None,
                            None,
                            request_document_intent,
                        )?;
                        if let Some(mut staged) = staged {
                            staged.cut_devices = cut_device_clipboard.take();
                            staged.cut_tracks = cut_clipboard.take();
                            pending_device_insert = Some(staged);
                        }
                        return Ok(None);
                    }
                    if let Some((_, target)) = adopted_device_draft.as_ref() {
                        session.device_take_resident_snapshot(
                            command,
                            *target,
                            context,
                            sink,
                            admission,
                            resident_snapshot.as_ref(),
                        )
                    } else {
                        session.command_resident_snapshot(
                            command,
                            context,
                            sink,
                            admission,
                            resident_snapshot.as_ref(),
                        )
                    }
                    .map(Some)
                });
            match result {
                Ok(None) if pending_device_insert.is_some() => {
                    request_complete = false;
                    status = Some(
                        if pending_device_insert.as_ref().is_some_and(|pending| {
                            pending.operation.device() == Some(GraphEditOperation::MoveTrack)
                        }) {
                            "track move pending · waiting for audio"
                        } else {
                            "device edit pending · waiting for audio"
                        }
                        .into(),
                    );
                }
                Ok(None) => status = Some("no change".into()),
                Ok(Some((new, snapshot))) => {
                    if failed_plugin_adoption {
                        // Device-take VST3 publication returns only after the exact
                        // output/revision callback confirmation.
                        failed_plugin_publication_confirmed = true;
                    }
                    created = Some(new);
                    if session.project() != state.project.as_ref() {
                        unsaved = true;
                        replacement_revision = Some(Arc::clone(&snapshot.prepared));
                        status = Some("resident edit accepted · waiting for audio".into());
                    } else {
                        status = Some("no change".into());
                    }
                    resident_snapshot = Some(snapshot);
                }
                Err(reason) => {
                    if time_range_copy && matches!(reason, SessionError::NativeSourceUnavailable(_))
                    {
                        range_copy_source_unavailable = Some(hex_engine::AudioSourceContext {
                            project_generation,
                            editor_revision: revision,
                        });
                    }
                    error = Some(reason.to_string());
                }
            }
        }

        Request::Edit(command)
            if private_sampler
                || matches!(&command,
                Command::BindTrackSource { source: Some(source), .. }
                if session.project().native_source(*source).is_some_and(|definition|
                    hex_engine::native_device_resources::requires_private_sampler_state(definition.instrument().state()))) =>
        {
            let current = compiled.as_ref().map(|(_, p)| {
                hex_engine::native_device_resources::ControlSamplerSelection {
                    sources: current_sources.as_ref(),
                    prepared: Some(p.as_ref()),
                }
            });
            match session.command_service_snapshot(
                command,
                export_lane.admission_with_resident_owners(
                    current,
                    resident_seek.as_ref(),
                    resident_preview.as_ref(),
                ),
            ) {
                Ok((new, snapshot)) => {
                    created = Some(new);
                    unsaved = true;
                    replacement_revision = Some(snapshot.prepared);
                    replacement_sources = Some(snapshot.sources);
                    status = Some(
                        if events.native_audio_requested() {
                            "sampler assigned · preparing frame-zero native arrangement"
                        } else {
                            "sampler offline export supported · native playback unavailable"
                        }
                        .into(),
                    );
                }
                Err(reason) => error = Some(reason.to_string()),
            }
        }

        request @ (Request::Import(_)
        | Request::ImportToTrack { .. }
        | Request::ImportToTrackWithOverwrite { .. }
        | Request::ImportToNewTrack { .. }
        | Request::ImportToNewTrackWithGain { .. }
        | Request::ImportToSession { .. })
            if events.resident_publication_enabled()
                && session.audio_mode() == DocumentAudioMode::Resident =>
        {
            let result = (|| {
                let (path, destination, overwrite, gain) = match request {
                    Request::Import(path) => (
                        path,
                        AudioBatchDestination::NewTracks {
                            before_track: None,
                            start: 0,
                        },
                        false,
                        None,
                    ),
                    Request::ImportToTrack { path, track, start } => (
                        path,
                        AudioBatchDestination::Track { track, start },
                        false,
                        None,
                    ),
                    Request::ImportToTrackWithOverwrite { path, track, start } => (
                        path,
                        AudioBatchDestination::Track { track, start },
                        true,
                        None,
                    ),
                    Request::ImportToNewTrack {
                        path,
                        before_track,
                        start,
                    } => (
                        path,
                        AudioBatchDestination::NewTracks {
                            before_track,
                            start,
                        },
                        false,
                        None,
                    ),
                    Request::ImportToNewTrackWithGain {
                        path,
                        before_track,
                        start,
                        gain,
                    } => (
                        path,
                        AudioBatchDestination::NewTracks {
                            before_track,
                            start,
                        },
                        false,
                        Some(gain),
                    ),
                    Request::ImportToSession { path, track, scene } => (
                        path,
                        AudioBatchDestination::Console(
                            crate::import_batch::ConsoleDestination::Cell { track, scene },
                        ),
                        false,
                        None,
                    ),
                    _ => return Err(SessionError::Invalid("audio import request changed")),
                };
                let sink = events
                    .resident_audio()
                    .ok_or(SessionError::Invalid("native worker unavailable"))?;
                resident_audio::import_file(
                    &mut session,
                    path,
                    destination,
                    crate::import_batch::AudioImportOptions {
                        overwrite,
                        new_track_gain: gain,
                        ..Default::default()
                    },
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
                    &mut import_decode,
                )
            })();
            match result {
                Ok(prepared) => {
                    unsaved = true;
                    replacement_revision = Some(prepared);
                    status = Some("audio imported".into());
                }
                Err(reason) => error = Some(reason.to_string()),
            }
        }

        Request::Import(path) => match import(
            &mut session,
            path,
            compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
            export_lane.admission_with_resident_owners(
                compiled.as_ref().map(|(_, p)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: current_sources.as_ref(),
                        prepared: Some(p),
                    }
                }),
                resident_seek.as_ref(),
                resident_preview.as_ref(),
            ),
            &mut import_decode,
        ) {
            Ok(prepared) => {
                unsaved = true;
                replacement_revision = Some(prepared);
                status = Some("audio imported".into());
            }
            Err(reason) => error = Some(reason),
        },

        Request::ImportToTrack { path, track, start } => match import_to_track(
            &mut session,
            path,
            track,
            start,
            compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
            false,
            export_lane.admission_with_resident_owners(
                compiled.as_ref().map(|(_, p)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: current_sources.as_ref(),
                        prepared: Some(p),
                    }
                }),
                resident_seek.as_ref(),
                resident_preview.as_ref(),
            ),
            &mut import_decode,
        ) {
            Ok(prepared) => {
                unsaved = true;
                replacement_revision = Some(prepared);
                status = Some("audio imported to selected track".into());
            }
            Err(reason) => error = Some(reason),
        },

        Request::ImportToTrackWithOverwrite { path, track, start } => {
            match import_to_track(
                &mut session,
                path,
                track,
                start,
                compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                true,
                export_lane.admission_with_resident_owners(
                    compiled.as_ref().map(|(_, p)| {
                        hex_engine::native_device_resources::ControlSamplerSelection {
                            sources: current_sources.as_ref(),
                            prepared: Some(p),
                        }
                    }),
                    resident_seek.as_ref(),
                    resident_preview.as_ref(),
                ),
                &mut import_decode,
            ) {
                Ok(prepared) => {
                    unsaved = true;
                    replacement_revision = Some(prepared);
                    status = Some("audio imported, overlapping clips cut".into());
                }
                Err(reason) => error = Some(reason),
            }
        }

        Request::ImportToNewTrack {
            path,
            before_track,
            start,
        } => match import_to_new_track(
            &mut session,
            path,
            before_track,
            start,
            compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
            export_lane.admission_with_resident_owners(
                compiled.as_ref().map(|(_, p)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: current_sources.as_ref(),
                        prepared: Some(p),
                    }
                }),
                resident_seek.as_ref(),
                resident_preview.as_ref(),
            ),
            &mut import_decode,
        ) {
            Ok(prepared) => {
                unsaved = true;
                replacement_revision = Some(prepared);
                status = Some("audio imported to new track".into());
            }
            Err(reason) => error = Some(reason),
        },

        Request::ImportToNewTrackWithGain {
            path,
            before_track,
            start,
            gain,
        } => match import_to_new_track_with_gain(
            &mut session,
            path,
            before_track,
            start,
            gain,
            compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
            export_lane.admission_with_resident_owners(
                compiled.as_ref().map(|(_, p)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: current_sources.as_ref(),
                        prepared: Some(p),
                    }
                }),
                resident_seek.as_ref(),
                resident_preview.as_ref(),
            ),
            &mut import_decode,
        ) {
            Ok(prepared) => {
                unsaved = true;
                replacement_revision = Some(prepared);
                status = Some("audio imported to new track".into());
            }
            Err(reason) => error = Some(reason),
        },

        Request::ImportToSession { path, track, scene } => match import_to_session(
            &mut session,
            path,
            track,
            scene,
            compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
            export_lane.admission_with_resident_owners(
                compiled.as_ref().map(|(_, p)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: current_sources.as_ref(),
                        prepared: Some(p),
                    }
                }),
                resident_seek.as_ref(),
                resident_preview.as_ref(),
            ),
            &mut import_decode,
        ) {
            Ok(prepared) => {
                unsaved = true;
                replacement_revision = Some(prepared);
                status = Some("audio source imported to session".into());
            }
            Err(reason) => error = Some(reason),
        },

        Request::TrackClipboard {
            action: TrackClipboardAction::Copy(tracks),
            ..
        } => {
            let cancellation = RequestCancellation::new(
                Arc::clone(&session.shutdown),
                Arc::clone(&document_intent),
                request_document_intent,
            );
            match plugin_state_save::capture_tracks_for_clipboard(
                session.project(),
                &tracks,
                false,
                hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                },
                &cancellation,
                events.sampler_edit_audio(),
                "track copy cancelled",
            ) {
                Ok(captured) => {
                    track_clipboard = Some(Arc::new(captured));
                    status = Some("tracks copied".into());
                }
                Err(reason) => error = Some(reason),
            }
        }

        Request::TrackClipboard { .. } => {
            error = Some("track clipboard request was not resolved".into())
        }

        Request::CopyClips(ids) => match session.project().capture_clips(&ids) {
            Ok(captured) => {
                range_clipboard = Some(ArrangementClipboard::Clips(Arc::new(captured)));
                status = Some("clips copied".into());
            }
            Err(reason) => error = Some(reason.to_string()),
        },

        Request::CopyTimeRange(range) => match session.project().capture_time_range(&range) {
            Ok(captured) => {
                range_clipboard = Some(ArrangementClipboard::Range(Arc::new(captured)));
                status = Some("time selection copied".into());
            }
            Err(reason) => error = Some(reason.to_string()),
        },

        Request::CutTimeRange(range) => match session.project().capture_time_range(&range) {
            Ok(captured) => {
                match resident_audio::execute_command(
                    &mut session,
                    Command::DeleteTimeRange { range },
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
                ) {
                    Ok((_, prepared)) => {
                        range_clipboard = Some(ArrangementClipboard::Range(Arc::new(captured)));
                        if session.project() != state.project.as_ref() {
                            unsaved = true;
                            replacement_revision = Some(prepared);
                        }
                        status = Some("time selection cut".into());
                    }
                    Err(reason) => error = Some(reason.to_string()),
                }
            }
            Err(reason) => error = Some(reason.to_string()),
        },

        Request::PasteTimeRange { track, frame } => {
            if let Some(clipboard) = &range_clipboard {
                match resident_audio::execute_command(
                    &mut session,
                    match clipboard {
                        ArrangementClipboard::Range(clipboard) => {
                            Command::PasteTimeRangeOnSourceTracks {
                                clipboard: Arc::clone(clipboard),
                                track,
                                frame,
                            }
                        }
                        ArrangementClipboard::Clips(clipboard) => Command::PasteClips {
                            clipboard: Arc::clone(clipboard),
                            track,
                            frame,
                            insert: false,
                        },
                    },
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
                ) {
                    Ok((result, prepared)) => {
                        if session.project() != state.project.as_ref() {
                            unsaved = true;
                            replacement_revision = Some(prepared);
                        }
                        // Clip pastes report the clips they created.
                        if matches!(clipboard, ArrangementClipboard::Clips(_)) {
                            created = Some(result);
                        }
                        status = Some(
                            match clipboard {
                                ArrangementClipboard::Range(_) => "time selection pasted",
                                ArrangementClipboard::Clips(_) => "clips pasted",
                            }
                            .into(),
                        );
                    }
                    Err(reason) => error = Some(reason.to_string()),
                }
            } else {
                error = Some("time clipboard is empty · copy a selection first".into());
            }
        }

        Request::InsertTimeRange { track, frame } => {
            if let Some(clipboard) = &range_clipboard {
                match resident_audio::execute_command(
                    &mut session,
                    match clipboard {
                        ArrangementClipboard::Range(clipboard) => {
                            Command::InsertTimeRangeOnSourceTracks {
                                clipboard: Arc::clone(clipboard),
                                track,
                                frame,
                            }
                        }
                        ArrangementClipboard::Clips(clipboard) => Command::PasteClips {
                            clipboard: Arc::clone(clipboard),
                            track,
                            frame,
                            insert: true,
                        },
                    },
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
                ) {
                    Ok((result, prepared)) => {
                        if session.project() != state.project.as_ref() {
                            unsaved = true;
                            replacement_revision = Some(prepared);
                        }
                        if matches!(clipboard, ArrangementClipboard::Clips(_)) {
                            created = Some(result);
                        }
                        status = Some("time selection inserted".into());
                    }
                    Err(reason) => error = Some(reason.to_string()),
                }
            } else {
                error = Some("time clipboard is empty · copy a selection first".into());
            }
        }

        Request::CommitMidiTake {
            project_generation: expected_generation,
            revision: expected_revision,
            binding,
            notes,
            stop_frame,
            ..
        } => {
            if expected_generation != project_generation || expected_revision != revision {
                error = Some("midi take refused · project changed; take retained".into());
            } else if device_gesture.snapshot().is_some() {
                error = Some("midi take refused · device edit is still applying".into());
            } else {
                match crate::midi_recording::finish_take(
                    session.project(),
                    &binding,
                    &notes,
                    stop_frame,
                ) {
                    Ok(Some(command)) => {
                        let before = session.project().clone();
                        match resident_audio::execute_command(
                            &mut session,
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
                        ) {
                            Ok((result, prepared)) if session.project() != &before => {
                                unsaved = true;
                                replacement_revision = Some(prepared);
                                created = Some(result);
                                status = Some("midi take committed".into());
                                if let Some(completion) = &mut midi_take_complete {
                                    completion.accepted = true;
                                }
                            }
                            Ok(_) => {
                                status = Some("empty midi take".into());
                                if let Some(completion) = &mut midi_take_complete {
                                    completion.accepted = true;
                                }
                            }
                            Err(reason) => {
                                error = Some(hex_storage::text::lowercase_outside_verbatim(
                                    &reason.to_string(),
                                ))
                            }
                        }
                    }
                    Ok(None) => {
                        status = Some("empty midi take".into());
                        if let Some(completion) = &mut midi_take_complete {
                            completion.accepted = true;
                        }
                    }
                    Err(reason) => error = Some(reason.into()),
                }
            }
        }

        Request::ArmGainRecording {
            project_generation: expected_generation,
            revision: expected_revision,
            track,
        } => {
            if expected_generation != project_generation || expected_revision != revision {
                error = Some("gain recording arm refused · project changed".into());
            } else if device_gesture.snapshot().is_some() {
                error = Some("gain recording arm refused · device edit is still applying".into());
            } else if events.resident_publication_enabled()
                && session.audio_mode() == DocumentAudioMode::Resident
            {
                let result = (|| {
                    let current = resident_snapshot
                        .as_ref()
                        .ok_or(SessionError::Invalid("native gain source unavailable"))?;
                    let sink = events
                        .resident_audio()
                        .ok_or(SessionError::Invalid("native gain output unavailable"))?;
                    session.prepare_resident_gain_recording(
                        track,
                        hex_engine::AudioSourceContext {
                            project_generation,
                            editor_revision: revision,
                        },
                        sink,
                        export_lane.admission_with_resident_owners(
                            compiled.as_ref().map(|(_, p)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: current_sources.as_ref(),
                                    prepared: Some(p),
                                }
                            }),
                            resident_seek.as_ref(),
                            resident_preview.as_ref(),
                        ),
                        current,
                    )
                })();
                match result {
                    Ok(snapshot) => {
                        compiled = Some((revision, Arc::clone(&snapshot.prepared)));
                        resident_snapshot = Some(snapshot);
                        status = Some("gain recording target prepared".into());
                    }
                    Err(reason) => error = Some(reason.to_string()),
                }
            } else {
                match session.prepare_gain_recording(
                    track,
                    compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                ) {
                    Ok(prepared) => {
                        restore_prepared = Some(prepared);
                        status = Some("gain recording target prepared".into());
                    }
                    Err(reason) => {
                        error = Some(hex_storage::text::lowercase_outside_verbatim(
                            &reason.to_string(),
                        ))
                    }
                }
            }
        }

        Request::GainLane {
            project_generation: expected_generation,
            revision: expected_revision,
            track,
            edit,
        } => {
            if expected_generation != project_generation || expected_revision != revision {
                error = Some("gain draft refused · project changed".into());
            } else if session.project().track(track).is_none() {
                error = Some("gain draft refused · track no longer exists".into());
            } else if device_gesture.snapshot().is_some() {
                error = Some("gain draft refused · device edit is still applying".into());
            } else {
                let before = session.project().clone();
                match resident_audio::execute_command(
                    &mut session,
                    edit.command(track),
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
                ) {
                    Ok((result, prepared)) if session.project() != &before => {
                        unsaved = true;
                        replacement_revision = Some(prepared);
                        created = Some(result);
                        status = Some("gain edit committed".into());
                    }
                    Ok(_) => status = Some("no change".into()),
                    Err(reason) => {
                        error = Some(hex_storage::text::lowercase_outside_verbatim(
                            &reason.to_string(),
                        ))
                    }
                }
            }
        }

        Request::BounceArrangement {
            project_generation: captured_generation,
            revision: captured_revision,
            target,
            in_place,
            adoption,
            control,
        } => {
            let grouping = target.is_audio_group();
            let result =
                if captured_generation != project_generation || captured_revision != revision {
                    Err("bounce cancelled · project changed".into())
                } else if grouping
                    && (pending_sampler_parameter.is_some()
                        || device_gesture.snapshot().is_some()
                        || tuning_gesture.snapshot().is_some())
                {
                    Err("finish or cancel the active gesture before grouping clips".into())
                } else if control.is_cancelled() {
                    Err("bounce cancelled".into())
                } else {
                    project_save::bounce_directory(document.as_ref(), recovery.as_deref()).and_then(
                        |root| {
                            if let Some(sink) = events.resident_audio() {
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
                                let cancellation = RequestCancellation::new(
                                    Arc::clone(&session.shutdown),
                                    Arc::clone(&document_intent),
                                    request_document_intent,
                                )
                                .with_bounce(&control);
                                resident_audio::bounce_arrangement(
                                    &mut session,
                                    target,
                                    in_place,
                                    adoption,
                                    &root,
                                    &control,
                                    &cancellation,
                                    context,
                                    sink,
                                    admission,
                                    &mut resident_snapshot,
                                )
                            } else {
                                session.bounce_arrangement_adopting_with(
                                    target,
                                    in_place,
                                    adoption,
                                    &root,
                                    &control,
                                    compiled.as_ref().map(|(_, p)| p.as_ref()),
                                    events.streamed_bouncer(),
                                )
                            }
                        },
                    )
                };
            match result {
                Ok((result, prepared)) => {
                    unsaved = true;
                    replacement_revision = Some(prepared);
                    created = Some(result);
                    status = Some(
                        if grouping {
                            "audio grouped into one clip"
                        } else {
                            "arrangement bounced to audio"
                        }
                        .into(),
                    );
                }
                Err(reason) => error = Some(reason),
            }
        }

        Request::ConvertClip {
            project_generation: expected_generation,
            revision: expected_revision,
            clip,
            to_audio,
        } => {
            let result = if expected_generation != project_generation
                || super::reconcile_barrier_revision(
                    expected_revision,
                    revision_before_request,
                    revision,
                ) != revision
            {
                Err("clip conversion canceled · project changed".to_owned())
            } else if to_audio {
                project_save::bounce_directory(document.as_ref(), recovery.as_deref()).and_then(|root| {
                    if let Some(sink) = events.resident_audio() {
                        return resident_audio::bounce_clip(
                            &mut session,
                            clip,
                            &root,
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision.saturating_add(1),
                            },
                            sink,
                            export_lane.admission_with_resident_owners(
                                compiled.as_ref().map(|(_, p)| hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: current_sources.as_ref(), prepared: Some(p),
                                }),
                                resident_seek.as_ref(), resident_preview.as_ref(),
                            ),
                            &mut resident_snapshot,
                        );
                    }
                    session.bounce_midi_clip_with_admission(
                        clip,
                        &root,
                        compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                        |session, prepared| export_lane.admission_with_resident_owners(compiled.as_ref().map(|(_, p)| hex_engine::native_device_resources::ControlSamplerSelection { sources: current_sources.as_ref(), prepared: Some(p) }), resident_seek.as_ref(), resident_preview.as_ref()).check_legacy_stage(session, std::slice::from_ref(prepared)),
                    )
                })
            } else {
                Err(hex_project::AUDIO_TO_MIDI_UNSUPPORTED.to_owned())
            };
            match result {
                Ok((result, prepared)) => {
                    unsaved = true;
                    replacement_revision = Some(prepared);
                    created = Some(result);
                    status = Some("clip bounced to audio".into());
                }
                Err(reason) => error = Some(reason),
            }
        }

        Request::EditArrangementSection {
            project_generation: captured_generation,
            revision: captured_revision,
            command,
        } => {
            if captured_generation != project_generation || captured_revision != revision {
                error = Some("arrangement section target changed".into());
            } else if !arrangement_sections::is_command(&command) {
                error = Some("section request requires a structural marker command".into());
            } else {
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
                match arrangement_sections::command(
                    &mut session,
                    command,
                    compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                    context,
                    &mut events,
                    &mut resident_snapshot,
                    admission,
                ) {
                    Ok((result, prepared)) => {
                        if session.project() != state.project.as_ref() {
                            unsaved = true;
                            replacement_revision = Some(prepared);
                        }
                        created = Some(result);
                        status = Some("arrangement section edited".into());
                    }
                    Err(reason) => error = Some(reason.to_string()),
                }
            }
        }

        Request::Edit(command)
            if resident_audio::device_insert::is_tempo_edit(&command)
                && session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                && compiled.as_ref().is_some_and(|(_, prepared)| {
                    prepared.arrangement().main_latency_frames() != 0
                })
                && events
                    .resident_audio()
                    .is_some_and(|sink| sink.verified_tempo_playing()) =>
        {
            let result = events
                .resident_audio()
                .ok_or(SessionError::Invalid("native worker unavailable"))
                .and_then(|sink| {
                    session.stage_verified_graph_edit(
                        command,
                        &state,
                        compiled.as_ref().map(|(_, prepared)| prepared.as_ref()),
                        sink,
                        request_document_intent,
                        None,
                        None,
                    )
                });
            match result {
                Ok(Some(staged)) => {
                    pending_device_insert = Some(staged);
                    request_complete = false;
                    status = Some("tempo edit pending · waiting for audio".into());
                }
                Ok(None) => status = Some("no change".into()),
                Err(reason) => error = Some(reason.to_string()),
            }
        }

        Request::Edit(command) => {
            duplicate_complete = matches!(&command, Command::DuplicateClip(_));
            let finish = |source: &Project, target: &Project, prepared: &Arc<PreparedRevision>| {
                let keyboard = prepare_tuning_input_publication(
                    source,
                    target,
                    prepared,
                    project_generation,
                    revision,
                    keyboard_activation.as_ref(),
                    keyboard_tuning_hint.as_ref(),
                    audition_activation.as_ref(),
                    false,
                )?;
                Ok((source != target, keyboard))
            };
            let result = if session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                && let Some(sink) = events.resident_audio()
            {
                session.command_verified_prepared_revision_with(
                    command,
                    compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                    hex_engine::AudioSourceContext {
                        project_generation,
                        editor_revision: revision.saturating_add(1),
                    },
                    sink,
                    finish,
                )
            } else {
                session.command_prepared_revision_with(
                    command,
                    compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                    finish,
                )
            };
            match result {
                Ok((result, (prepared, (true, keyboard)))) => {
                    unsaved = true;
                    replacement_revision = Some(prepared);
                    tuning_input_publication = keyboard;
                    created = Some(result);
                    status = Some("edit committed".into());
                }
                Ok(_) => status = Some("no change".into()),
                Err(reason) => {
                    error = Some(hex_storage::text::lowercase_outside_verbatim(
                        &reason.to_string(),
                    ))
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
    fn clip_ops_rejects_non_clip_request_without_panicking() {
        assert_eq!(
            latest_timeline_content_end(&Project::new(48_000).unwrap()),
            0
        );
    }
}
