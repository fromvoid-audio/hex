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

pub(super) fn handle(ctx: &mut WorkerDispatch<'_>, request: Request) -> ControlFlow<()> {
    match request {
        Request::ReorderTracks { .. } | Request::ReparentTracks { .. } => {
            ctx.outcome.error = Some("track reorder target was not resolved".into());
        }

        Request::PaintMidiClip(_) => {
            ctx.outcome.error = Some("clip paint target was not resolved".into());
        }

        Request::MoveExistingClips { .. } => {
            ctx.outcome.error = Some("clip move target was not resolved".into());
        }

        Request::CopyArrangementClipToConsoleTrack { .. } => {
            ctx.outcome.error = Some("console clip target was not resolved".into());
        }

        Request::DuplicateClips { .. }
        | Request::DuplicateAliasClips { .. }
        | Request::DuplicateSessionCell { .. } => {
            ctx.outcome.error = Some("clip duplication target was not resolved".into());
        }

        Request::GroupClips { .. } | Request::GroupTimeRange { .. } => {
            ctx.outcome.error = Some("clip grouping target was not resolved".into());
        }

        Request::GroupAudioEvents { .. } => {
            ctx.outcome.error = Some("audio event grouping target was not resolved".into());
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
                Arc::clone(&ctx.worker.session.shutdown),
                Arc::clone(&ctx.worker.document_intent),
                ctx.outcome.request_document_intent,
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
            let mut imported_resident = None;
            let result = if ctx.worker.session.audio_mode() != DocumentAudioMode::Resident {
                Err(SessionError::Invalid(
                    "project sample placement requires resident audio",
                ))
            } else if let Some(sink) = ctx.worker.events.resident_audio() {
                ctx.worker.session.place_existing_asset_revision_using(
                    crate::import_batch::ProjectSampleImport {
                        asset,
                        destination,
                        new_track_gain,
                        warp,
                    },
                    ctx.worker
                        .compiled
                        .as_ref()
                        .map(|(_, artifact)| artifact.as_ref()),
                    &mut admit,
                    |input, admit| {
                        let snapshot = resident_audio::prepare_import_playback(
                            input,
                            hex_engine::AudioSourceContext {
                                project_generation: ctx.worker.project_generation,
                                editor_revision: ctx.worker.revision.saturating_add(1),
                            },
                            sink,
                            admission,
                            ctx.worker.resident_snapshot.as_ref(),
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
                admission
                    .check_legacy_stage(&ctx.worker.session, &[])
                    .and_then(|()| {
                        ctx.worker.session.place_existing_asset_revision_using(
                            crate::import_batch::ProjectSampleImport {
                                asset,
                                destination,
                                new_track_gain,
                                warp,
                            },
                            ctx.worker
                                .compiled
                                .as_ref()
                                .map(|(_, artifact)| artifact.as_ref()),
                            &mut admit,
                            crate::import_batch::prepare_asset_free,
                        )
                    })
            };
            match result {
                Ok((result, prepared)) => {
                    if let Some(snapshot) = imported_resident {
                        ctx.worker.resident_snapshot = Some(snapshot);
                    }
                    ctx.worker.unsaved = true;
                    ctx.outcome.created = Some(result);
                    ctx.outcome.replacement_revision = Some(prepared);
                    ctx.outcome.status = Some("project sample inserted".into());
                }
                Err(reason) => {
                    ctx.outcome.error = Some(hex_storage::text::lowercase_outside_verbatim(
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
            ctx.outcome.audio_batch_complete = Some(Arc::clone(&control));
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
                project_generation: ctx.worker.project_generation,
                revision: ctx.worker.revision,
                active_gesture: ctx.worker.device_gesture.snapshot().is_some(),
            };
            let mut imported_resident = None;
            let result = if context != actual {
                Err(crate::import_batch_decode::AudioBatchError::ContextChanged)
            } else {
                AudioBatchImport::capture_with_control(
                    paths,
                    destination,
                    &ctx.worker.session,
                    actual,
                    Arc::clone(&control),
                    options,
                )
                .and_then(|job| {
                    if ctx.worker.session.audio_mode() == DocumentAudioMode::Resident
                        && let Some(sink) = ctx.worker.events.resident_audio()
                    {
                        let admission = ctx.worker.export_lane.admission_with_resident_owners(ctx.worker.compiled.as_ref().map(|(_, prepared)| hex_engine::native_device_resources::ControlSamplerSelection { sources: ctx.worker.current_sources.as_ref(), prepared: Some(prepared) }), ctx.worker.resident_seek.as_ref(), ctx.worker.resident_preview.as_ref());
                        let allowance = resident_audio::import_frame_allowance(
                            &ctx.worker.session, sink, admission, ctx.worker.resident_snapshot.as_ref())?;
                        let job = job.with_pcm_frame_allowance(allowance);
                        return job.run_with_storage_admission_using(
                            &mut ctx.worker.session, ctx.worker.compiled.as_ref().map(|(_, prepared)| prepared.as_ref()), || actual,
                            |session, prepared| admission.check_legacy_stage(session, prepared),
                            |input, admit| {
                                let snapshot = resident_audio::prepare_import_playback(input,
                                    hex_engine::AudioSourceContext { project_generation: ctx.worker.project_generation, editor_revision: ctx.worker.revision.saturating_add(1) }, sink, admission, ctx.worker.resident_snapshot.as_ref(), audition.unwrap_or(resident_audio::ImportPlayback::Normal), admit)?;
                                let prepared = Arc::clone(&snapshot.prepared);
                                imported_resident = Some(snapshot);
                                Ok(prepared)
                            });
                    }
                    job.run_with_storage_admission(
                        &mut ctx.worker.session,
                        ctx.worker.compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                        || actual,
                        |session, prepared| ctx.worker.export_lane.admission_with_resident_owners(ctx.worker.compiled.as_ref().map(|(_, p)| hex_engine::native_device_resources::ControlSamplerSelection { sources: ctx.worker.current_sources.as_ref(), prepared: Some(p) }), ctx.worker.resident_seek.as_ref(), ctx.worker.resident_preview.as_ref()).check_legacy_stage(session, prepared),
                    )
                })
            };
            match result {
                Ok((_, prepared)) => {
                    if let Some(snapshot) = imported_resident {
                        ctx.worker.resident_snapshot = Some(snapshot);
                    }
                    ctx.worker.unsaved = true;
                    ctx.outcome.replacement_revision = Some(prepared);
                    ctx.outcome.status = Some("audio batch imported · one undo".into());
                }
                Err(reason) => {
                    control.refuse();
                    ctx.outcome.error = Some(reason.to_string());
                    if let crate::import_batch_decode::AudioBatchError::Member {
                        member,
                        path,
                        source,
                    } = reason
                    {
                        ctx.outcome.audio_batch_failure = Some(AudioBatchMemberFailure {
                            member,
                            path,
                            explanation: source.to_string(),
                        });
                    }
                }
            }
        }

        Request::Edit(_)
            if ctx.outcome.failed_plugin_adoption
                && (!ctx.worker.events.resident_publication_enabled()
                    || ctx.worker.session.audio_mode() != DocumentAudioMode::Resident) =>
        {
            ctx.outcome.error = Some("vst3 recovery lost its resident publication path".into());
        }

        Request::Edit(command) if ctx.outcome.device_insert_proposal.is_some() => {
            // Catalog additions and picker replacements are resolved through
            // their existing default/owner validation and gesture barrier.
            // Keep their Created result after the exact install.
            let result = resident_audio::device_insert::submit_browser_edit(
                &mut ctx.worker.session,
                command,
                &ctx.worker.state,
                ctx.worker.project_generation,
                ctx.worker
                    .compiled
                    .as_ref()
                    .map(|(_, prepared)| prepared.as_ref()),
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
                ctx.outcome.request_document_intent,
                ctx.outcome.device_insert_proposal.as_ref(),
            );
            match result {
                Ok(BrowserEdit::Pending(mut staged)) => {
                    staged.publish_created = !ctx.outcome.replacing_catalog_instrument;
                    staged.proposal_control = ctx.outcome.device_insert_proposal.take();
                    ctx.worker.pending_device_insert = Some(*staged);
                    ctx.outcome.request_complete = false;
                    ctx.outcome.status = Some("device edit pending · waiting for audio".into());
                }
                Ok(BrowserEdit::Applied(new, prepared)) => {
                    if !Arc::ptr_eq(&ctx.worker.session.project_arc(), &ctx.worker.state.project) {
                        if !ctx.outcome.replacing_catalog_instrument {
                            ctx.outcome.created = Some(new);
                        }
                        ctx.worker.unsaved = true;
                        ctx.outcome.replacement_revision = Some(prepared);
                    }
                    ctx.outcome.device_insert_proposal_accepted = true;
                    ctx.outcome.status = Some(
                        if ctx.outcome.replacing_catalog_instrument {
                            "device replaced"
                        } else {
                            "device inserted"
                        }
                        .into(),
                    );
                }
                Ok(BrowserEdit::Unchanged) => {
                    ctx.outcome.device_insert_proposal_accepted = ctx
                        .outcome
                        .device_insert_proposal
                        .as_ref()
                        .is_some_and(|control| control.begin_commit());
                    ctx.outcome.status = Some("no change".into());
                }
                Err(reason) => ctx.outcome.error = Some(reason.to_string()),
            }
        }

        Request::Edit(command)
            if ctx.worker.session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                && (matches!(&command, Command::PasteDevices { .. })
                    || resident_audio::device_insert::graph_edit_operation(&command).is_some()
                    || ctx.worker.session.should_stage_verified_group_route(
                        &command,
                        ctx.worker
                            .compiled
                            .as_ref()
                            .map(|(_, prepared)| prepared.as_ref()),
                    ))
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
                    ctx.worker.session.stage_verified_graph_edit(
                        command,
                        &ctx.worker.state,
                        ctx.worker
                            .compiled
                            .as_ref()
                            .map(|(_, prepared)| prepared.as_ref()),
                        sink,
                        ctx.outcome.request_document_intent,
                        ctx.worker.device_gesture.snapshot().as_ref(),
                        None,
                    )
                });
            match result {
                Ok(Some(mut staged)) => {
                    staged.cut_devices = ctx.outcome.cut_device_clipboard.take();
                    staged.cut_tracks = ctx.outcome.cut_clipboard.take();
                    ctx.outcome.status = Some(
                        if staged.operation.device() == Some(GraphEditOperation::MoveTrack) {
                            "track move pending · waiting for audio"
                        } else {
                            "device edit pending · waiting for audio"
                        }
                        .into(),
                    );
                    ctx.worker.pending_device_insert = Some(staged);
                    ctx.outcome.request_complete = false;
                }
                Ok(None) => ctx.outcome.status = Some("no change".into()),
                Err(reason) => ctx.outcome.error = Some(reason.to_string()),
            }
        }

        Request::Edit(command)
            if ctx.worker.events.resident_publication_enabled()
                && ctx.worker.session.audio_mode() == DocumentAudioMode::Resident =>
        {
            let recoverable_edit = ctx.outcome.time_range_copy
                || matches!(command, Command::ResizeAudioClipView { .. });
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
            let mut switch_phases = (ctx.track_switch_request
                && ctx.worker.events.observing_request())
            .then(super::super::diagnostics::SectionPhases::default);
            let result = ctx
                .worker
                .events
                .resident_audio()
                .ok_or(SessionError::Invalid("native worker unavailable"))
                .and_then(|sink| {
                    if resident_audio::device_insert::should_stage_resident_graph_edit(
                        ctx.worker.session.project(),
                        &command,
                        sink,
                    ) || resident_audio::device_insert::stages_main_power_off(
                        ctx.worker.session.project(),
                        &command,
                        sink,
                    ) || resident_audio::device_insert::running_return_power_target(
                        ctx.worker.session.project(),
                        &command,
                        sink,
                    )
                    .is_some()
                        || resident_audio::device_insert::running_main_power_on(
                            ctx.worker.session.project(),
                            &command,
                            hex_engine::AudioSourceContext {
                                project_generation: ctx.worker.project_generation,
                                editor_revision: ctx.worker.revision,
                            },
                            sink,
                        )
                        .is_some()
                        || resident_audio::device_insert::running_return_power_on(
                            ctx.worker.session.project(),
                            &command,
                            hex_engine::AudioSourceContext {
                                project_generation: ctx.worker.project_generation,
                                editor_revision: ctx.worker.revision,
                            },
                            sink,
                        )
                        .is_some()
                        || resident_audio::device_insert::stopped_hosted_power(
                            ctx.worker.session.project(),
                            &command,
                            hex_engine::AudioSourceContext {
                                project_generation: ctx.worker.project_generation,
                                editor_revision: ctx.worker.revision,
                            },
                            sink,
                        )?
                        .is_some()
                    {
                        let staged = ctx.worker.session.stage_resident_device_insert(
                            command,
                            hex_engine::AudioSourceContext {
                                project_generation: ctx.worker.project_generation,
                                editor_revision: ctx.worker.revision,
                            },
                            sink,
                            admission,
                            ctx.worker.resident_snapshot.as_ref(),
                            None,
                            None,
                            ctx.outcome.request_document_intent,
                        )?;
                        if let Some(mut staged) = staged {
                            staged.cut_devices = ctx.outcome.cut_device_clipboard.take();
                            staged.cut_tracks = ctx.outcome.cut_clipboard.take();
                            ctx.worker.pending_device_insert = Some(staged);
                        }
                        return Ok(None);
                    }
                    ctx.worker
                        .session
                        .command_resident_snapshot_timed(
                            command,
                            ctx.worker
                                .adopted_device_draft
                                .as_ref()
                                .map(|(_, target)| *target),
                            resident_audio::ResidentSinkContext {
                                context,
                                sink,
                                admission,
                            },
                            ctx.worker.resident_snapshot.as_ref(),
                            switch_phases.as_mut(),
                        )
                        .map(Some)
                });
            if matches!(result, Ok(Some(_)))
                && let Some(phases) = switch_phases
            {
                ctx.worker.events.record_track_switch_phases(phases);
            }
            match result {
                Ok(None) => {
                    if let Some(pending) = &ctx.worker.pending_device_insert {
                        ctx.outcome.request_complete = false;
                        ctx.outcome.status = Some(format!(
                            "{} pending · waiting for audio",
                            pending.operation.edit_name()
                        ));
                    } else {
                        ctx.outcome.status = Some("no change".into());
                    }
                }
                Ok(Some((new, snapshot))) => {
                    if ctx.outcome.failed_plugin_adoption {
                        // Device-take VST3 publication returns only after the exact
                        // output/revision callback confirmation.
                        ctx.outcome.failed_plugin_publication_confirmed = true;
                    }
                    ctx.outcome.created = Some(new);
                    if !Arc::ptr_eq(&ctx.worker.session.project_arc(), &ctx.worker.state.project) {
                        ctx.worker.unsaved = true;
                        ctx.outcome.replacement_revision = Some(Arc::clone(&snapshot.prepared));
                        ctx.outcome.status =
                            Some("resident edit accepted · waiting for audio".into());
                    } else {
                        ctx.outcome.status = Some("no change".into());
                    }
                    ctx.worker.resident_snapshot = Some(snapshot);
                }
                Err(reason) => {
                    if recoverable_edit
                        && matches!(reason, SessionError::NativeSourceUnavailable(_))
                    {
                        ctx.outcome.resident_edit_source_unavailable =
                            Some(hex_engine::AudioSourceContext {
                                project_generation: ctx.worker.project_generation,
                                editor_revision: ctx.worker.revision,
                            });
                    }
                    ctx.outcome.error = Some(reason.to_string());
                }
            }
        }

        Request::Edit(command)
            if ctx.outcome.private_sampler
                || matches!(&command,
                Command::BindTrackSource { source: Some(source), .. }
                if ctx.worker.session.project().native_source(*source).is_some_and(|definition|
                    hex_engine::native_device_resources::requires_private_sampler_state(definition.instrument().state()))) =>
        {
            let current = ctx.worker.compiled.as_ref().map(|(_, p)| {
                hex_engine::native_device_resources::ControlSamplerSelection {
                    sources: ctx.worker.current_sources.as_ref(),
                    prepared: Some(p.as_ref()),
                }
            });
            match ctx.worker.session.command_service_snapshot(
                command,
                ctx.worker.export_lane.admission_with_resident_owners(
                    current,
                    ctx.worker.resident_seek.as_ref(),
                    ctx.worker.resident_preview.as_ref(),
                ),
            ) {
                Ok((new, snapshot)) => {
                    ctx.outcome.created = Some(new);
                    ctx.worker.unsaved = true;
                    ctx.outcome.replacement_revision = Some(snapshot.prepared);
                    ctx.outcome.replacement_sources = Some(snapshot.sources);
                    ctx.outcome.status = Some(
                        if ctx.worker.events.native_audio_requested() {
                            "sampler assigned · preparing frame-zero native arrangement"
                        } else {
                            "sampler offline export supported · native playback unavailable"
                        }
                        .into(),
                    );
                }
                Err(reason) => ctx.outcome.error = Some(reason.to_string()),
            }
        }

        request @ (Request::Import(_)
        | Request::ImportToTrack { .. }
        | Request::ImportToTrackWithOverwrite { .. }
        | Request::ImportToNewTrack { .. }
        | Request::ImportToNewTrackWithGain { .. }
        | Request::ImportToSession { .. })
            if ctx.worker.events.resident_publication_enabled()
                && ctx.worker.session.audio_mode() == DocumentAudioMode::Resident =>
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
                let sink = ctx
                    .worker
                    .events
                    .resident_audio()
                    .ok_or(SessionError::Invalid("native worker unavailable"))?;
                resident_audio::import_file(
                    &mut ctx.worker.session,
                    path,
                    destination,
                    crate::import_batch::AudioImportOptions {
                        overwrite,
                        new_track_gain: gain,
                        ..Default::default()
                    },
                    resident_audio::ResidentSinkContext {
                        context: hex_engine::AudioSourceContext {
                            project_generation: ctx.worker.project_generation,
                            editor_revision: ctx.worker.revision.saturating_add(1),
                        },
                        sink,
                        admission: ctx.worker.export_lane.admission_with_resident_owners(
                            ctx.worker.compiled.as_ref().map(|(_, prepared)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: ctx.worker.current_sources.as_ref(),
                                    prepared: Some(prepared),
                                }
                            }),
                            ctx.worker.resident_seek.as_ref(),
                            ctx.worker.resident_preview.as_ref(),
                        ),
                    },
                    &mut ctx.worker.resident_snapshot,
                    &mut ctx.worker.import_decode,
                )
            })();
            match result {
                Ok(prepared) => {
                    ctx.worker.unsaved = true;
                    ctx.outcome.replacement_revision = Some(prepared);
                    ctx.outcome.status = Some("audio imported".into());
                }
                Err(reason) => ctx.outcome.error = Some(reason.to_string()),
            }
        }

        Request::Import(path) => match import(
            &mut ctx.worker.session,
            path,
            ctx.worker
                .compiled
                .as_ref()
                .map(|(_, artifact)| artifact.as_ref()),
            ctx.worker.export_lane.admission_with_resident_owners(
                ctx.worker.compiled.as_ref().map(|(_, p)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: ctx.worker.current_sources.as_ref(),
                        prepared: Some(p),
                    }
                }),
                ctx.worker.resident_seek.as_ref(),
                ctx.worker.resident_preview.as_ref(),
            ),
            &mut ctx.worker.import_decode,
        ) {
            Ok(prepared) => {
                ctx.worker.unsaved = true;
                ctx.outcome.replacement_revision = Some(prepared);
                ctx.outcome.status = Some("audio imported".into());
            }
            Err(reason) => ctx.outcome.error = Some(reason),
        },

        Request::ImportToTrack { path, track, start } => match import_to_track(
            &mut ctx.worker.session,
            path,
            track,
            start,
            ctx.worker
                .compiled
                .as_ref()
                .map(|(_, artifact)| artifact.as_ref()),
            false,
            ctx.worker.export_lane.admission_with_resident_owners(
                ctx.worker.compiled.as_ref().map(|(_, p)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: ctx.worker.current_sources.as_ref(),
                        prepared: Some(p),
                    }
                }),
                ctx.worker.resident_seek.as_ref(),
                ctx.worker.resident_preview.as_ref(),
            ),
            &mut ctx.worker.import_decode,
        ) {
            Ok(prepared) => {
                ctx.worker.unsaved = true;
                ctx.outcome.replacement_revision = Some(prepared);
                ctx.outcome.status = Some("audio imported to selected track".into());
            }
            Err(reason) => ctx.outcome.error = Some(reason),
        },

        Request::ImportToTrackWithOverwrite { path, track, start } => {
            match import_to_track(
                &mut ctx.worker.session,
                path,
                track,
                start,
                ctx.worker
                    .compiled
                    .as_ref()
                    .map(|(_, artifact)| artifact.as_ref()),
                true,
                ctx.worker.export_lane.admission_with_resident_owners(
                    ctx.worker.compiled.as_ref().map(|(_, p)| {
                        hex_engine::native_device_resources::ControlSamplerSelection {
                            sources: ctx.worker.current_sources.as_ref(),
                            prepared: Some(p),
                        }
                    }),
                    ctx.worker.resident_seek.as_ref(),
                    ctx.worker.resident_preview.as_ref(),
                ),
                &mut ctx.worker.import_decode,
            ) {
                Ok(prepared) => {
                    ctx.worker.unsaved = true;
                    ctx.outcome.replacement_revision = Some(prepared);
                    ctx.outcome.status = Some("audio imported, overlapping clips cut".into());
                }
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        Request::ImportToNewTrack {
            path,
            before_track,
            start,
        } => match import_to_new_track(
            &mut ctx.worker.session,
            path,
            before_track,
            start,
            ctx.worker
                .compiled
                .as_ref()
                .map(|(_, artifact)| artifact.as_ref()),
            ctx.worker.export_lane.admission_with_resident_owners(
                ctx.worker.compiled.as_ref().map(|(_, p)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: ctx.worker.current_sources.as_ref(),
                        prepared: Some(p),
                    }
                }),
                ctx.worker.resident_seek.as_ref(),
                ctx.worker.resident_preview.as_ref(),
            ),
            &mut ctx.worker.import_decode,
        ) {
            Ok(prepared) => {
                ctx.worker.unsaved = true;
                ctx.outcome.replacement_revision = Some(prepared);
                ctx.outcome.status = Some("audio imported to new track".into());
            }
            Err(reason) => ctx.outcome.error = Some(reason),
        },

        Request::ImportToNewTrackWithGain {
            path,
            before_track,
            start,
            gain,
        } => match import_to_new_track_with_gain(
            &mut ctx.worker.session,
            path,
            before_track,
            start,
            gain,
            ctx.worker
                .compiled
                .as_ref()
                .map(|(_, artifact)| artifact.as_ref()),
            ctx.worker.export_lane.admission_with_resident_owners(
                ctx.worker.compiled.as_ref().map(|(_, p)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: ctx.worker.current_sources.as_ref(),
                        prepared: Some(p),
                    }
                }),
                ctx.worker.resident_seek.as_ref(),
                ctx.worker.resident_preview.as_ref(),
            ),
            &mut ctx.worker.import_decode,
        ) {
            Ok(prepared) => {
                ctx.worker.unsaved = true;
                ctx.outcome.replacement_revision = Some(prepared);
                ctx.outcome.status = Some("audio imported to new track".into());
            }
            Err(reason) => ctx.outcome.error = Some(reason),
        },

        Request::ImportToSession { path, track, scene } => match import_to_session(
            &mut ctx.worker.session,
            path,
            track,
            scene,
            ctx.worker
                .compiled
                .as_ref()
                .map(|(_, artifact)| artifact.as_ref()),
            ctx.worker.export_lane.admission_with_resident_owners(
                ctx.worker.compiled.as_ref().map(|(_, p)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: ctx.worker.current_sources.as_ref(),
                        prepared: Some(p),
                    }
                }),
                ctx.worker.resident_seek.as_ref(),
                ctx.worker.resident_preview.as_ref(),
            ),
            &mut ctx.worker.import_decode,
        ) {
            Ok(prepared) => {
                ctx.worker.unsaved = true;
                ctx.outcome.replacement_revision = Some(prepared);
                ctx.outcome.status = Some("audio source imported to session".into());
            }
            Err(reason) => ctx.outcome.error = Some(reason),
        },

        Request::TrackClipboard {
            action: TrackClipboardAction::Copy(tracks),
            ..
        } => {
            let cancellation = RequestCancellation::new(
                Arc::clone(&ctx.worker.session.shutdown),
                Arc::clone(&ctx.worker.document_intent),
                ctx.outcome.request_document_intent,
            );
            match plugin_state_save::capture_tracks_for_clipboard(
                ctx.worker.session.project(),
                &tracks,
                false,
                hex_engine::AudioSourceContext {
                    project_generation: ctx.worker.project_generation,
                    editor_revision: ctx.worker.revision,
                },
                &cancellation,
                ctx.worker.events.sampler_edit_audio(),
                "track copy cancelled",
            ) {
                Ok(captured) => {
                    ctx.worker.track_clipboard = Some(Arc::new(captured));
                    ctx.outcome.status = Some("tracks copied".into());
                }
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        Request::TrackClipboard { .. } => {
            ctx.outcome.error = Some("track clipboard request was not resolved".into())
        }

        Request::CopyClips(ids) => match ctx.worker.session.project().capture_clips(&ids) {
            Ok(captured) => {
                ctx.worker.range_clipboard = Some(ArrangementClipboard::Clips(Arc::new(captured)));
                ctx.outcome.status = Some("clips copied".into());
            }
            Err(reason) => ctx.outcome.error = Some(reason.to_string()),
        },

        Request::CutClips(ids) => match ctx.worker.session.project().capture_clips(&ids) {
            Ok(captured) => {
                match resident_audio::execute_command(
                    &mut ctx.worker.session,
                    Command::EditClips {
                        clips: ids,
                        edit: hex_project::ClipEdit::Delete,
                    },
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
                ) {
                    Ok((_, prepared)) => {
                        ctx.worker.range_clipboard =
                            Some(ArrangementClipboard::Clips(Arc::new(captured)));
                        if !Arc::ptr_eq(
                            &ctx.worker.session.project_arc(),
                            &ctx.worker.state.project,
                        ) {
                            ctx.worker.unsaved = true;
                            ctx.outcome.replacement_revision = Some(prepared);
                        }
                        ctx.outcome.status = Some("clips cut".into());
                    }
                    Err(reason) => ctx.outcome.error = Some(reason.to_string()),
                }
            }
            Err(reason) => ctx.outcome.error = Some(reason.to_string()),
        },

        Request::CopyTimeRange(range) => {
            match ctx.worker.session.project().capture_time_range(&range) {
                Ok(captured) => {
                    ctx.worker.range_clipboard =
                        Some(ArrangementClipboard::Range(Arc::new(captured)));
                    ctx.outcome.status = Some("time selection copied".into());
                }
                Err(reason) => ctx.outcome.error = Some(reason.to_string()),
            }
        }

        Request::CutTimeRange(range) => {
            match ctx.worker.session.project().capture_time_range(&range) {
                Ok(captured) => {
                    match resident_audio::execute_command(
                        &mut ctx.worker.session,
                        Command::DeleteTimeRange { range },
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
                    ) {
                        Ok((_, prepared)) => {
                            ctx.worker.range_clipboard =
                                Some(ArrangementClipboard::Range(Arc::new(captured)));
                            if !Arc::ptr_eq(
                                &ctx.worker.session.project_arc(),
                                &ctx.worker.state.project,
                            ) {
                                ctx.worker.unsaved = true;
                                ctx.outcome.replacement_revision = Some(prepared);
                            }
                            ctx.outcome.status = Some("time selection cut".into());
                        }
                        Err(reason) => ctx.outcome.error = Some(reason.to_string()),
                    }
                }
                Err(reason) => ctx.outcome.error = Some(reason.to_string()),
            }
        }

        Request::PasteTimeRange { track, frame } => {
            if let Some(clipboard) = &ctx.worker.range_clipboard {
                match resident_audio::execute_command(
                    &mut ctx.worker.session,
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
                ) {
                    Ok((result, prepared)) => {
                        if !Arc::ptr_eq(
                            &ctx.worker.session.project_arc(),
                            &ctx.worker.state.project,
                        ) {
                            ctx.worker.unsaved = true;
                            ctx.outcome.replacement_revision = Some(prepared);
                        }
                        // Clip pastes report the clips they created.
                        if matches!(clipboard, ArrangementClipboard::Clips(_)) {
                            ctx.outcome.created = Some(result);
                        }
                        ctx.outcome.status = Some(
                            match clipboard {
                                ArrangementClipboard::Range(_) => "time selection pasted",
                                ArrangementClipboard::Clips(_) => "clips pasted",
                            }
                            .into(),
                        );
                    }
                    Err(reason) => ctx.outcome.error = Some(reason.to_string()),
                }
            } else {
                ctx.outcome.error = Some("time clipboard is empty · copy a selection first".into());
            }
        }

        Request::InsertTimeRange { track, frame } => {
            if let Some(clipboard) = &ctx.worker.range_clipboard {
                match resident_audio::execute_command(
                    &mut ctx.worker.session,
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
                ) {
                    Ok((result, prepared)) => {
                        if !Arc::ptr_eq(
                            &ctx.worker.session.project_arc(),
                            &ctx.worker.state.project,
                        ) {
                            ctx.worker.unsaved = true;
                            ctx.outcome.replacement_revision = Some(prepared);
                        }
                        if matches!(clipboard, ArrangementClipboard::Clips(_)) {
                            ctx.outcome.created = Some(result);
                        }
                        ctx.outcome.status = Some("time selection inserted".into());
                    }
                    Err(reason) => ctx.outcome.error = Some(reason.to_string()),
                }
            } else {
                ctx.outcome.error = Some("time clipboard is empty · copy a selection first".into());
            }
        }

        Request::CommitMidiTake {
            project_generation: expected_generation,
            revision: expected_revision,
            binding,
            notes,
            expressions,
            stop_frame,
            ..
        } => {
            if expected_generation != ctx.worker.project_generation
                || expected_revision != ctx.worker.revision
            {
                ctx.outcome.error =
                    Some("midi take refused · project changed; take retained".into());
            } else if ctx.worker.device_gesture.snapshot().is_some() {
                ctx.outcome.error =
                    Some("midi take refused · device edit is still applying".into());
            } else {
                match crate::midi_recording::finish_take_with_expression(
                    ctx.worker.session.project(),
                    &binding,
                    &notes,
                    &expressions,
                    stop_frame,
                ) {
                    Ok(Some(command)) => {
                        let before = ctx.worker.session.project().clone();
                        match resident_audio::execute_command(
                            &mut ctx.worker.session,
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
                        ) {
                            Ok((result, prepared)) if ctx.worker.session.project() != &before => {
                                ctx.worker.unsaved = true;
                                ctx.outcome.replacement_revision = Some(prepared);
                                ctx.outcome.created = Some(result);
                                ctx.outcome.status = Some("midi take committed".into());
                                if let Some(completion) = &mut ctx.outcome.midi_take_complete {
                                    completion.accepted = true;
                                }
                            }
                            Ok(_) => {
                                ctx.outcome.status = Some("empty midi take".into());
                                if let Some(completion) = &mut ctx.outcome.midi_take_complete {
                                    completion.accepted = true;
                                }
                            }
                            Err(reason) => {
                                ctx.outcome.error =
                                    Some(hex_storage::text::lowercase_outside_verbatim(
                                        &reason.to_string(),
                                    ))
                            }
                        }
                    }
                    Ok(None) => {
                        ctx.outcome.status = Some("empty midi take".into());
                        if let Some(completion) = &mut ctx.outcome.midi_take_complete {
                            completion.accepted = true;
                        }
                    }
                    Err(reason) => ctx.outcome.error = Some(reason.into()),
                }
            }
        }

        Request::ArmGainRecording {
            project_generation: expected_generation,
            revision: expected_revision,
            track,
        } => {
            if expected_generation != ctx.worker.project_generation
                || expected_revision != ctx.worker.revision
            {
                ctx.outcome.error = Some("gain recording arm refused · project changed".into());
            } else if ctx.worker.device_gesture.snapshot().is_some() {
                ctx.outcome.error =
                    Some("gain recording arm refused · device edit is still applying".into());
            } else if ctx.worker.events.resident_publication_enabled()
                && ctx.worker.session.audio_mode() == DocumentAudioMode::Resident
            {
                let result = (|| {
                    let current = ctx
                        .worker
                        .resident_snapshot
                        .as_ref()
                        .ok_or(SessionError::Invalid("native gain source unavailable"))?;
                    let sink = ctx
                        .worker
                        .events
                        .resident_audio()
                        .ok_or(SessionError::Invalid("native gain output unavailable"))?;
                    ctx.worker.session.prepare_resident_gain_recording(
                        track,
                        hex_engine::AudioSourceContext {
                            project_generation: ctx.worker.project_generation,
                            editor_revision: ctx.worker.revision,
                        },
                        sink,
                        ctx.worker.export_lane.admission_with_resident_owners(
                            ctx.worker.compiled.as_ref().map(|(_, p)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: ctx.worker.current_sources.as_ref(),
                                    prepared: Some(p),
                                }
                            }),
                            ctx.worker.resident_seek.as_ref(),
                            ctx.worker.resident_preview.as_ref(),
                        ),
                        current,
                    )
                })();
                match result {
                    Ok(snapshot) => {
                        ctx.worker.compiled =
                            Some((ctx.worker.revision, Arc::clone(&snapshot.prepared)));
                        ctx.worker.resident_snapshot = Some(snapshot);
                        ctx.outcome.status = Some("gain recording target prepared".into());
                    }
                    Err(reason) => ctx.outcome.error = Some(reason.to_string()),
                }
            } else {
                match ctx.worker.session.prepare_gain_recording(
                    track,
                    ctx.worker
                        .compiled
                        .as_ref()
                        .map(|(_, artifact)| artifact.as_ref()),
                ) {
                    Ok(prepared) => {
                        ctx.outcome.restore_prepared = Some(prepared);
                        ctx.outcome.status = Some("gain recording target prepared".into());
                    }
                    Err(reason) => {
                        ctx.outcome.error = Some(hex_storage::text::lowercase_outside_verbatim(
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
            if expected_generation != ctx.worker.project_generation
                || expected_revision != ctx.worker.revision
            {
                ctx.outcome.error = Some("gain draft refused · project changed".into());
            } else if ctx.worker.session.project().track(track).is_none() {
                ctx.outcome.error = Some("gain draft refused · track no longer exists".into());
            } else if ctx.worker.device_gesture.snapshot().is_some() {
                ctx.outcome.error =
                    Some("gain draft refused · device edit is still applying".into());
            } else {
                let before = ctx.worker.session.project().clone();
                match resident_audio::execute_command(
                    &mut ctx.worker.session,
                    edit.command(track),
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
                ) {
                    Ok((result, prepared)) if ctx.worker.session.project() != &before => {
                        ctx.worker.unsaved = true;
                        ctx.outcome.replacement_revision = Some(prepared);
                        ctx.outcome.created = Some(result);
                        ctx.outcome.status = Some("gain edit committed".into());
                    }
                    Ok(_) => ctx.outcome.status = Some("no change".into()),
                    Err(reason) => {
                        ctx.outcome.error = Some(hex_storage::text::lowercase_outside_verbatim(
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
            let result = if captured_generation != ctx.worker.project_generation
                || captured_revision != ctx.worker.revision
            {
                Err("bounce cancelled · project changed".into())
            } else if grouping
                && (ctx.worker.pending_sampler_parameter.is_some()
                    || ctx.worker.device_gesture.snapshot().is_some()
                    || ctx.worker.tuning_gesture.snapshot().is_some())
            {
                Err("finish or cancel the active gesture before grouping clips".into())
            } else if control.is_cancelled() {
                Err("bounce cancelled".into())
            } else {
                project_save::bounce_directory(
                    ctx.worker.document.as_ref(),
                    ctx.worker.recovery.as_deref(),
                )
                .and_then(|root| {
                    if let Some(sink) = ctx.worker.events.resident_audio() {
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
                        let cancellation = RequestCancellation::new(
                            Arc::clone(&ctx.worker.session.shutdown),
                            Arc::clone(&ctx.worker.document_intent),
                            ctx.outcome.request_document_intent,
                        )
                        .with_bounce(&control);
                        resident_audio::bounce_arrangement(
                            &mut ctx.worker.session,
                            target,
                            in_place,
                            adoption,
                            &root,
                            &control,
                            &cancellation,
                            context,
                            sink,
                            admission,
                            &mut ctx.worker.resident_snapshot,
                        )
                    } else {
                        ctx.worker.session.bounce_arrangement_adopting_with(
                            target,
                            in_place,
                            adoption,
                            &root,
                            &control,
                            ctx.worker.compiled.as_ref().map(|(_, p)| p.as_ref()),
                            ctx.worker.events.streamed_bouncer(),
                        )
                    }
                })
            };
            match result {
                Ok((result, prepared)) => {
                    ctx.worker.unsaved = true;
                    ctx.outcome.replacement_revision = Some(prepared);
                    ctx.outcome.created = Some(result);
                    ctx.outcome.status = Some(
                        if grouping {
                            "audio grouped into one clip"
                        } else {
                            "arrangement bounced to audio"
                        }
                        .into(),
                    );
                }
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        Request::ConvertClip {
            project_generation: expected_generation,
            revision: expected_revision,
            clip,
            to_audio,
        } => {
            let result = if expected_generation != ctx.worker.project_generation
                || super::reconcile_barrier_revision(
                    expected_revision,
                    ctx.outcome.revision_before_request,
                    ctx.worker.revision,
                ) != ctx.worker.revision
            {
                Err("clip conversion canceled · project changed".to_owned())
            } else if to_audio {
                project_save::bounce_directory(ctx.worker.document.as_ref(), ctx.worker.recovery.as_deref()).and_then(|root| {
                    if let Some(sink) = ctx.worker.events.resident_audio() {
                        return resident_audio::bounce_clip(
                            &mut ctx.worker.session,
                            clip,
                            &root,
                            hex_engine::AudioSourceContext {
                                project_generation: ctx.worker.project_generation,
                                editor_revision: ctx.worker.revision.saturating_add(1),
                            },
                            sink,
                            ctx.worker.export_lane.admission_with_resident_owners(
                                ctx.worker.compiled.as_ref().map(|(_, p)| hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: ctx.worker.current_sources.as_ref(), prepared: Some(p),
                                }),
                                ctx.worker.resident_seek.as_ref(), ctx.worker.resident_preview.as_ref(),
                            ),
                            &mut ctx.worker.resident_snapshot,
                        );
                    }
                    ctx.worker.session.bounce_midi_clip_with_admission(
                        clip,
                        &root,
                        ctx.worker.compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                        |session, prepared| ctx.worker.export_lane.admission_with_resident_owners(ctx.worker.compiled.as_ref().map(|(_, p)| hex_engine::native_device_resources::ControlSamplerSelection { sources: ctx.worker.current_sources.as_ref(), prepared: Some(p) }), ctx.worker.resident_seek.as_ref(), ctx.worker.resident_preview.as_ref()).check_legacy_stage(session, std::slice::from_ref(prepared)),
                    )
                })
            } else {
                Err(hex_project::AUDIO_TO_MIDI_UNSUPPORTED.to_owned())
            };
            match result {
                Ok((result, prepared)) => {
                    ctx.worker.unsaved = true;
                    ctx.outcome.replacement_revision = Some(prepared);
                    ctx.outcome.created = Some(result);
                    ctx.outcome.status = Some("clip bounced to audio".into());
                }
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        Request::EditArrangementSection {
            project_generation: captured_generation,
            revision: captured_revision,
            command,
        } => {
            if captured_generation != ctx.worker.project_generation
                || captured_revision != ctx.worker.revision
            {
                ctx.outcome.error = Some("arrangement section target changed".into());
            } else if !arrangement_sections::is_command(&command) {
                ctx.outcome.error =
                    Some("section request requires a structural marker command".into());
            } else {
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
                match arrangement_sections::command(
                    &mut ctx.worker.session,
                    command,
                    ctx.worker
                        .compiled
                        .as_ref()
                        .map(|(_, artifact)| artifact.as_ref()),
                    context,
                    &mut ctx.worker.events,
                    &mut ctx.worker.resident_snapshot,
                    admission,
                ) {
                    Ok((result, prepared)) => {
                        if !Arc::ptr_eq(
                            &ctx.worker.session.project_arc(),
                            &ctx.worker.state.project,
                        ) {
                            ctx.worker.unsaved = true;
                            ctx.outcome.replacement_revision = Some(prepared);
                        }
                        ctx.outcome.created = Some(result);
                        ctx.outcome.status = Some("arrangement section edited".into());
                    }
                    Err(reason) => ctx.outcome.error = Some(reason.to_string()),
                }
            }
        }

        Request::Edit(command)
            if resident_audio::device_insert::is_tempo_edit(&command)
                && ctx.worker.session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                && ctx.worker.compiled.as_ref().is_some_and(|(_, prepared)| {
                    prepared.arrangement().main_latency_frames() != 0
                })
                && ctx
                    .worker
                    .events
                    .resident_audio()
                    .is_some_and(|sink| sink.verified_tempo_playing()) =>
        {
            let result = ctx
                .worker
                .events
                .resident_audio()
                .ok_or(SessionError::Invalid("native worker unavailable"))
                .and_then(|sink| {
                    ctx.worker.session.stage_verified_graph_edit(
                        command,
                        &ctx.worker.state,
                        ctx.worker
                            .compiled
                            .as_ref()
                            .map(|(_, prepared)| prepared.as_ref()),
                        sink,
                        ctx.outcome.request_document_intent,
                        None,
                        None,
                    )
                });
            match result {
                Ok(Some(staged)) => {
                    ctx.worker.pending_device_insert = Some(staged);
                    ctx.outcome.request_complete = false;
                    ctx.outcome.status = Some("tempo edit pending · waiting for audio".into());
                }
                Ok(None) => ctx.outcome.status = Some("no change".into()),
                Err(reason) => ctx.outcome.error = Some(reason.to_string()),
            }
        }

        Request::Edit(command) => {
            ctx.outcome.duplicate_complete = matches!(&command, Command::DuplicateClip(_));
            let finish = |source: &Project, target: &Project, prepared: &Arc<PreparedRevision>| {
                let keyboard = prepare_tuning_input_publication(
                    source,
                    target,
                    prepared,
                    ctx.worker.project_generation,
                    ctx.worker.revision,
                    ctx.worker.keyboard_activation.as_ref(),
                    ctx.worker.keyboard_tuning_hint.as_ref(),
                    ctx.worker.audition_activation.as_ref(),
                    false,
                )?;
                Ok((source != target, keyboard))
            };
            let result = if ctx.worker.session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                && let Some(sink) = ctx.worker.events.resident_audio()
            {
                ctx.worker.session.command_verified_prepared_revision_with(
                    command,
                    ctx.worker
                        .compiled
                        .as_ref()
                        .map(|(_, artifact)| artifact.as_ref()),
                    hex_engine::AudioSourceContext {
                        project_generation: ctx.worker.project_generation,
                        editor_revision: ctx.worker.revision.saturating_add(1),
                    },
                    sink,
                    finish,
                )
            } else {
                ctx.worker.session.command_prepared_revision_with(
                    command,
                    ctx.worker
                        .compiled
                        .as_ref()
                        .map(|(_, artifact)| artifact.as_ref()),
                    finish,
                )
            };
            match result {
                Ok((result, (prepared, (true, keyboard)))) => {
                    ctx.worker.unsaved = true;
                    ctx.outcome.replacement_revision = Some(prepared);
                    ctx.outcome.tuning_input_publication = keyboard;
                    ctx.outcome.created = Some(result);
                    ctx.outcome.status = Some("edit committed".into());
                }
                Ok(_) => ctx.outcome.status = Some("no change".into()),
                Err(reason) => {
                    ctx.outcome.error = Some(hex_storage::text::lowercase_outside_verbatim(
                        &reason.to_string(),
                    ))
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
    fn clip_ops_rejects_non_clip_request_without_panicking() {
        assert_eq!(
            latest_timeline_content_end(&Project::new(48_000).unwrap()),
            0
        );
    }
}
