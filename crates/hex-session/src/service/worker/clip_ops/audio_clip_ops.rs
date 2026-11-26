//! Audio import, resident decoding, gain-lane, bounce and arrangement-section worker operations.
use super::super::*;
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
    fn decode_resident_member_refuses_when_resident_budget_exhausted() {
        let session = Session::new(48_000, 8, 0).unwrap();
        let mut decode = truncated_import::ImportDecode::default();
        match decode_resident_member(&session, PathBuf::from("sample.wav"), &mut decode) {
            Err(err) => assert_eq!(err, "resident audio frame limit reached"),
            Ok(_) => panic!("expected resident frame limit refusal"),
        }
    }
}
