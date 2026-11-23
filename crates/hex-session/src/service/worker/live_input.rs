//! Live input, computer keyboard, tuning audition, expression, resident seek and audio restart operations.
use super::*;
use std::ops::ControlFlow;

pub(crate) fn compile(
    session: &Session,
    previous: Option<&PreparedRevision>,
) -> Result<Arc<PreparedRevision>, String> {
    session
        .compile_prepared_revision(previous)
        .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))
}

pub(crate) fn project_source_for_runtime_input(
    session: &Session,
    tuning: Option<TuningGestureSnapshot>,
    canonical: &Arc<PreparedRevision>,
) -> Result<(Arc<Project>, Arc<PreparedRevision>, bool), String> {
    if let Some(snapshot) = tuning {
        session
            .preview_tuning(snapshot.tuning, Some(canonical))
            .map(|(project, prepared)| (project, prepared, true))
            .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))
    } else {
        Ok((
            Arc::new(session.project().clone()),
            Arc::clone(canonical),
            false,
        ))
    }
}

pub(crate) fn native_playback(prepared: Arc<PreparedRevision>) -> Playback {
    match Playback::try_from_prepared_with_session(Arc::clone(&prepared)) {
        Ok(playback) => playback,
        Err(error) => Playback::from_prepared_with_session_unavailable(prepared, error),
    }
}

pub(crate) type TuningInputPublication = (
    Playback,
    Option<hex_engine::computer_keyboard::ComputerKeyboardTuningHint>,
);

#[allow(clippy::too_many_arguments)] // One borrowed worker generation/revision/input tuple, inside history preparation.
pub(crate) fn prepare_tuning_input_publication(
    source: &Project,
    target: &Project,
    prepared: &Arc<PreparedRevision>,
    generation: u64,
    revision: u64,
    activation: Option<&(
        u64,
        u64,
        hex_project::TrackId,
        Arc<hex_engine::computer_keyboard::ComputerKeyboardInput>,
    )>,
    hint: Option<&hex_engine::computer_keyboard::ComputerKeyboardTuningHint>,
    audition: Option<&Arc<hex_engine::tuning_audition::TuningAuditionInput>>,
    allow_unchanged: bool,
) -> Result<Option<TuningInputPublication>, crate::SessionError> {
    let keyboard = activation.filter(|(old_generation, old_revision, _, input)| {
        !input.is_cancelled()
            && *old_generation == generation
            && *old_revision == revision
            && source.same_except_tuning(target)
            && (allow_unchanged || source.tuning() != target.tuning())
    });
    let audition =
        audition.filter(|input| input.project_generation == generation && !input.is_cancelled());
    if keyboard.is_none() && audition.is_none() {
        return Ok(None);
    }
    let mut playback = native_playback(Arc::clone(prepared));
    let next_hint = if let Some((_, _, track, input)) = keyboard {
        let hint = hint.ok_or(crate::SessionError::Invalid(
            "computer keyboard tuning hint is missing",
        ))?;
        playback.prepare_computer_keyboard_for_tuning(target, *track, Arc::clone(input), hint)?;
        Some(
            playback
                .computer_keyboard_tuning_hint()
                .ok_or(crate::SessionError::Invalid(
                    "computer keyboard tuning hint was not prepared",
                ))?,
        )
    } else {
        None
    };
    if let Some(input) = audition {
        playback.prepare_tuning_audition(target, Arc::clone(input))?;
    }
    Ok(Some((playback, next_hint)))
}

pub(super) fn handle(ctx: &mut WorkerDispatch<'_>, request: Request) -> ControlFlow<()> {
    match request {
        Request::RestartAudio {
            project_generation: expected,
            request_id,
        } => {
            if expected != ctx.worker.project_generation || request_id == 0 {
                ctx.outcome.error = Some("audio restart request is stale".into());
            } else {
                match ctx
                    .worker
                    .events
                    .restart_audio(ctx.worker.project_generation, request_id)
                {
                    Ok(()) => {
                        if let Some(retired) = ctx.worker.device_gesture.retire_native_on_restart()
                        {
                            if ctx
                                .worker
                                .resident_preview
                                .as_ref()
                                .and_then(resident_preview::Preview::device_identity)
                                .is_some_and(|preview| preview.key == retired.key)
                            {
                                ctx.worker.resident_preview = None;
                            }
                            ctx.outcome.gesture_ack = Some(retired);
                        }
                        ctx.outcome.status = Some("restarting audio".into());
                        let context = hex_engine::AudioSourceContext {
                            project_generation: ctx.worker.project_generation,
                            editor_revision: ctx.worker.revision,
                        };
                        if ctx.worker.session.audio_mode() == DocumentAudioMode::Resident
                            && let Some(sink) = ctx.worker.events.resident_audio()
                            && !sink.plugin_state_restore_pending(context)
                        {
                            let admission = ctx.worker.export_lane.admission_with_resident_owners(
                                ctx.worker.compiled.as_ref().map(|(_, prepared)| {
                                    hex_engine::native_device_resources::ControlSamplerSelection {
                                        sources: ctx.worker.current_sources.as_ref(),
                                        prepared: Some(prepared),
                                    }
                                }),
                                ctx.worker.resident_seek.as_ref(),
                                None,
                            );
                            let metadata = ResidentPublicationMetadata {
                                source_origin: crate::service::ResidentSourceOrigin::Unknown,
                                clip_preview: None,
                                context,
                                device_preview: None,
                                tuning_preview: None,
                                seek_request: None,
                                stopped_restore: false,
                                computer_keyboard_request: None,
                                gain_recording_track: ctx
                                    .worker
                                    .resident_snapshot
                                    .as_ref()
                                    .and_then(|snapshot| snapshot.prepared.gain_recording_track()),
                                requires_preroll: ctx
                                    .worker
                                    .resident_snapshot
                                    .as_ref()
                                    .is_some_and(|snapshot| {
                                        snapshot.prepared.arrangement_seek_requires_preroll()
                                    }),
                            };
                            // Restart is an explicit output shutdown, independent of authoring. Replay
                            // the actual canonical/preview owner through offline custody while its old
                            // book retires. Old monitor clients belong to the relinquished Client.
                            if let Err(reason) = resident_preview::publish_inputs(
                                &ctx.worker.session,
                                &mut ctx.worker.resident_snapshot,
                                &mut ctx.worker.resident_preview,
                                sink,
                                admission,
                                metadata,
                                ResidentPublicationPolicy::InstallFreshRuntime,
                                resident_audio::RuntimeInputs::default(),
                                None,
                            ) {
                                ctx.outcome.error =
                                    Some(format!("audio restart graph unavailable: {reason}"));
                            }
                        }
                    }
                    Err(reason) => ctx.outcome.error = Some(reason),
                }
            }
        }

        Request::TuningGesture(gesture) => {
            ctx.outcome.tuning_gesture_request = true;
            ctx.outcome.gesture_protocol_ack = true;
            let outcome = if ctx.worker.device_gesture.snapshot().is_some()
                && matches!(gesture, TuningGestureRequest::Begin { .. })
            {
                crate::tuning_gesture::TuningGestureOutcome {
                    ack: TuningGestureAck {
                        preview_retained: false,
                        key: gesture.key(),
                        phase: gesture.phase(),
                        sequence: gesture.sequence(),
                        accepted: false,
                        reason: Some("device gesture is active".into()),
                    },
                    playback: None,
                    preview_project: None,
                    canonical_changed: false,
                }
            } else if ctx.worker.events.resident_publication_enabled()
                && ctx.worker.session.audio_mode() == DocumentAudioMode::Resident
            {
                let admission = ctx.worker.export_lane.admission_with_resident_owners(
                    ctx.worker.compiled.as_ref().map(|(_, prepared)| {
                        hex_engine::native_device_resources::ControlSamplerSelection {
                            sources: ctx.worker.current_sources.as_ref(),
                            prepared: Some(prepared),
                        }
                    }),
                    ctx.worker.resident_seek.as_ref(),
                    None,
                );
                ctx.worker.tuning_gesture.handle_prepared(
                    &mut ctx.worker.session,
                    gesture,
                    ctx.worker.compiled.as_ref().map(|(_, artifact)| artifact),
                    ctx.worker.project_generation,
                    |session, operation| {
                        let sink = ctx
                            .worker
                            .events
                            .resident_audio()
                            .ok_or(SessionError::Invalid("native tuning output unavailable"))?;
                        resident_preview::tuning_operation(
                            session,
                            operation,
                            hex_engine::AudioSourceContext {
                                project_generation: ctx.worker.project_generation,
                                editor_revision: ctx.worker.revision,
                            },
                            &mut ctx.worker.resident_snapshot,
                            &mut ctx.worker.resident_preview,
                            sink,
                            admission,
                        )
                    },
                )
            } else {
                ctx.worker.tuning_gesture.handle_with(
                    &mut ctx.worker.session,
                    gesture,
                    ctx.worker.compiled.as_ref().map(|(_, artifact)| artifact),
                    ctx.worker.project_generation,
                    |source, target, prepared| {
                        ctx.outcome.tuning_input_publication = prepare_tuning_input_publication(
                            source,
                            target,
                            prepared,
                            ctx.worker.project_generation,
                            ctx.worker.revision,
                            ctx.worker.keyboard_activation.as_ref(),
                            ctx.worker.keyboard_tuning_hint.as_ref(),
                            ctx.worker.audition_activation.as_ref(),
                            true,
                        )?;
                        Ok(())
                    },
                )
            };
            let native_terminal_accepted = ctx.worker.events.resident_publication_enabled()
                && ctx.worker.session.audio_mode() == DocumentAudioMode::Resident
                && outcome.ack.accepted
                && matches!(
                    gesture.phase(),
                    crate::tuning_gesture::TuningGesturePhase::Commit
                        | crate::tuning_gesture::TuningGesturePhase::Cancel
                );
            ctx.outcome.tuning_gesture_ack = Some(outcome.ack.clone());
            if let Some(prepared) = outcome.playback {
                if native_terminal_accepted && !outcome.canonical_changed {
                    ctx.worker.compiled = Some((ctx.worker.revision, Arc::clone(&prepared)));
                }
                if outcome.canonical_changed {
                    ctx.worker.unsaved = true;
                    ctx.outcome.replacement_revision = Some(prepared);
                } else if matches!(gesture, TuningGestureRequest::Change { .. }) {
                    ctx.outcome.preview_prepared = Some(prepared);
                    ctx.outcome.preview_project = outcome.preview_project;
                } else {
                    ctx.outcome.restore_prepared = Some(prepared);
                }
            }
        }

        Request::PreparePausedInputMonitor { request, reply } => {
            let result = (|| {
                if request.source.context
                    != (hex_engine::AudioSourceContext {
                        project_generation: ctx.worker.project_generation,
                        editor_revision: ctx.worker.revision,
                    })
                    || ctx.worker.session.audio_mode() != DocumentAudioMode::Resident
                {
                    return Err("paused input monitor source changed".into());
                }
                let sink = ctx
                    .worker
                    .events
                    .resident_audio()
                    .ok_or("paused input monitor output is unavailable")?;
                let snapshot = ctx
                    .worker
                    .resident_snapshot
                    .as_ref()
                    .ok_or("paused input monitor graph is unavailable")?;
                ctx.worker.session.prepare_resident_paused_input_monitor(
                    request,
                    snapshot,
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
                )
            })();
            let _ = reply.try_send(result);
        }

        Request::RestoreStoppedPlayback | Request::ContinueStoppedPlaybackRestore => {
            ctx.outcome.stopped_playback_restore_complete = true;
            if ctx.worker.device_gesture.snapshot().is_some() {
                ctx.outcome.error =
                    Some("finish the device gesture before restoring stopped playback".into());
            } else if ctx.worker.events.resident_publication_enabled()
                && ctx.worker.session.audio_mode() == DocumentAudioMode::Resident
            {
                let result = (|| {
                    let context = hex_engine::AudioSourceContext {
                        project_generation: ctx.worker.project_generation,
                        editor_revision: ctx.worker.revision,
                    };
                    if ctx
                        .worker
                        .pending_stopped_restore
                        .is_some_and(|old| old != context)
                    {
                        return Err(SessionError::Invalid("stopped restore source changed"));
                    }
                    let sink = ctx
                        .worker
                        .events
                        .resident_audio()
                        .ok_or(SessionError::Invalid("native restore output unavailable"))?;
                    match sink
                        .prepare_stopped_playback_restore(context)
                        .map_err(SessionError::NativeAudio)?
                    {
                        Some(false) => {
                            ctx.worker.pending_stopped_restore = Some(context);
                            ctx.outcome.stopped_playback_restore_complete = false;
                            return Ok(());
                        }
                        Some(true) => {
                            let snapshot = ctx.worker.session.restore_resident_snapshot(
                                context, sink,
                                ctx.worker.export_lane.admission_with_resident_owners(ctx.worker.compiled.as_ref().map(|(_, p)| hex_engine::native_device_resources::ControlSamplerSelection { sources: ctx.worker.current_sources.as_ref(), prepared: Some(p) }), ctx.worker.resident_seek.as_ref(), None),
                                ctx.worker.resident_snapshot.as_ref(),
                            )?;
                            ctx.worker.compiled =
                                Some((ctx.worker.revision, Arc::clone(&snapshot.prepared)));
                            ctx.worker.resident_snapshot = Some(snapshot);
                            ctx.worker.resident_preview = None;
                            ctx.worker.pending_stopped_restore = None;
                            return Ok(());
                        }
                        None => {}
                    }
                    let snapshot =
                        ctx.worker
                            .resident_snapshot
                            .as_mut()
                            .ok_or(SessionError::Invalid(
                                "current playback revision is unavailable",
                            ))?;
                    let selected = snapshot.inputs.clone();
                    let metadata = ResidentPublicationMetadata {
                        source_origin: crate::service::ResidentSourceOrigin::Unknown,
                        clip_preview: None,
                        context: hex_engine::AudioSourceContext {
                            project_generation: ctx.worker.project_generation,
                            editor_revision: ctx.worker.revision,
                        },
                        device_preview: None,
                        tuning_preview: None,
                        seek_request: None,
                        stopped_restore: true,
                        computer_keyboard_request: None,
                        gain_recording_track: snapshot.prepared.gain_recording_track(),
                        requires_preroll: snapshot.prepared.arrangement_seek_requires_preroll(),
                    };
                    resident_preview::publish_inputs(
                        &ctx.worker.session,
                        &mut ctx.worker.resident_snapshot,
                        &mut ctx.worker.resident_preview,
                        sink,
                        ctx.worker.export_lane.admission_with_resident_owners(
                            ctx.worker.compiled.as_ref().map(|(_, p)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: ctx.worker.current_sources.as_ref(),
                                    prepared: Some(p),
                                }
                            }),
                            ctx.worker.resident_seek.as_ref(),
                            None,
                        ),
                        metadata,
                        ResidentPublicationPolicy::InstallFreshRuntime,
                        selected,
                        None,
                    )?;
                    Ok(())
                })();
                match result {
                    Ok(_) => ctx.outcome.status = Some("restoring stopped playback".into()),
                    Err(reason) => {
                        ctx.worker.pending_stopped_restore = None;
                        if let Some(sink) = ctx.worker.events.resident_audio() {
                            sink.cancel_stopped_playback_restore();
                        }
                        ctx.outcome.error = Some(reason.to_string());
                    }
                }
            } else if let Some((compiled_revision, prepared)) = ctx.worker.compiled.as_ref()
                && *compiled_revision == ctx.worker.revision
            {
                ctx.outcome.restore_prepared = Some(Arc::clone(prepared));
                ctx.outcome.status = Some("restoring stopped playback".into());
            } else {
                ctx.outcome.error = Some("current playback revision is unavailable".into());
            }
        }

        Request::PrepareLiveExpression {
            input,
            revision: expected_revision,
        } => {
            if ctx.worker.session.audio_mode() == DocumentAudioMode::VerifiedStreaming {
                input.cancel();
                ctx.outcome.error =
                    Some("controller input is unavailable for verified streaming audio".into());
            } else if input.project_generation != ctx.worker.project_generation
                || expected_revision != ctx.worker.revision
                || input.is_cancelled()
            {
                if !ctx.worker.events.resident_publication_enabled()
                    || !ctx
                        .worker
                        .resident_snapshot
                        .as_ref()
                        .and_then(|s| s.inputs.expression.as_ref())
                        .is_some_and(|old| Arc::ptr_eq(old, &input))
                {
                    input.cancel();
                }
                ctx.outcome.error = Some("controller input preparation is stale".into());
            } else if ctx.worker.events.resident_publication_enabled() {
                let result = (|| {
                    let snapshot =
                        ctx.worker
                            .resident_snapshot
                            .as_mut()
                            .ok_or(SessionError::Invalid(
                                "native controller source unavailable",
                            ))?;
                    let mut selected = snapshot.inputs.clone();
                    selected.expression = Some(Arc::clone(&input));
                    let metadata = ResidentPublicationMetadata {
                        source_origin: crate::service::ResidentSourceOrigin::Unknown,
                        clip_preview: None,
                        context: hex_engine::AudioSourceContext {
                            project_generation: ctx.worker.project_generation,
                            editor_revision: ctx.worker.revision,
                        },
                        device_preview: None,
                        tuning_preview: None,
                        seek_request: None,
                        stopped_restore: false,
                        computer_keyboard_request: None,
                        gain_recording_track: snapshot.prepared.gain_recording_track(),
                        requires_preroll: snapshot.prepared.arrangement_seek_requires_preroll(),
                    };
                    let sink = ctx
                        .worker
                        .events
                        .resident_audio()
                        .ok_or(SessionError::Invalid(
                            "native controller output unavailable",
                        ))?;
                    resident_preview::publish_inputs(
                        &ctx.worker.session,
                        &mut ctx.worker.resident_snapshot,
                        &mut ctx.worker.resident_preview,
                        sink,
                        ctx.worker.export_lane.admission_with_resident_owners(
                            ctx.worker.compiled.as_ref().map(|(_, p)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: ctx.worker.current_sources.as_ref(),
                                    prepared: Some(p),
                                }
                            }),
                            ctx.worker.resident_seek.as_ref(),
                            None,
                        ),
                        metadata,
                        ResidentPublicationPolicy::InheritRuntime,
                        selected,
                        Some(resident_audio::RequiredInput::Expression),
                    )
                })();
                match result {
                    Ok(hint) => {
                        ctx.worker.keyboard_tuning_hint = hint;
                        ctx.outcome.live_expression_complete = Some(Arc::clone(&input));
                        ctx.worker.live_expression = Some(input);
                    }
                    Err(reason) => {
                        if !ctx
                            .worker
                            .resident_snapshot
                            .as_ref()
                            .and_then(|s| s.inputs.expression.as_ref())
                            .is_some_and(|old| Arc::ptr_eq(old, &input))
                        {
                            input.cancel();
                        }
                        ctx.outcome.error = Some(reason.to_string());
                    }
                }
            } else if let Some((_, prepared)) = ctx
                .worker
                .compiled
                .as_ref()
                .filter(|(compiled_revision, _)| *compiled_revision == ctx.worker.revision)
            {
                let source = project_source_for_runtime_input(
                    &ctx.worker.session,
                    ctx.worker.tuning_gesture.snapshot(),
                    prepared,
                );
                let prepared =
                    source.and_then(|(source_project, source_prepared, is_preview)| {
                        let mut playback = native_playback(Arc::clone(&source_prepared));
                        let result = playback
                        .prepare_live_expression(&source_project, Arc::clone(&input))
                    .and_then(|()| {
                        if let Some((_, _, track, keyboard)) = ctx.worker.keyboard_activation
                            .as_ref()
                            .filter(|(generation, prepared_revision, _, keyboard)| {
                                *generation == ctx.worker.project_generation
                                    && *prepared_revision == ctx.worker.revision
                                    && !keyboard.is_cancelled()
                            })
                        {
                            if let Some(hint) = ctx.worker.keyboard_tuning_hint.as_ref() {
                                playback.prepare_computer_keyboard_for_controller_activation_with_tuning_hint(&source_project, *track, Arc::clone(keyboard), hint)?;
                            } else {
                                playback.prepare_computer_keyboard_for_controller_activation(&source_project, *track, Arc::clone(keyboard))?;
                            }
                        }
                        Ok(())
                    });
                        if result.is_ok() && is_preview {
                            ctx.outcome.preview_prepared = Some(source_prepared);
                            ctx.outcome.preview_project = Some(source_project);
                        }
                        result.map(|()| playback).map_err(|error| error.to_string())
                    });
                match prepared {
                    Ok(playback) => {
                        ctx.outcome.live_expression_complete = Some(Arc::clone(&input));
                        if let Some(previous) =
                            ctx.worker.live_expression.replace(Arc::clone(&input))
                            && !Arc::ptr_eq(&previous, &input)
                        {
                            previous.cancel();
                        }
                        ctx.worker.keyboard_tuning_hint = playback.computer_keyboard_tuning_hint();
                        ctx.outcome.computer_keyboard_playback = Some(playback);
                    }
                    Err(reason) => {
                        input.cancel();
                        ctx.outcome.error = Some(reason.to_string());
                    }
                }
            } else {
                input.cancel();
                ctx.outcome.error =
                    Some("controller input requires a compiled project revision".into());
            }
        }

        Request::PrepareTuningAudition {
            revision: expected_revision,
            input,
        } => {
            ctx.outcome.tuning_audition_complete = Some(Arc::clone(&input));
            if ctx.worker.session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                || expected_revision != ctx.worker.revision
                || input.project_generation != ctx.worker.project_generation
                || input.is_cancelled()
            {
                if !ctx.worker.events.resident_publication_enabled()
                    || !ctx
                        .worker
                        .resident_snapshot
                        .as_ref()
                        .and_then(|s| s.inputs.audition.as_ref())
                        .is_some_and(|old| Arc::ptr_eq(old, &input))
                {
                    input.cancel();
                }
                ctx.outcome.error =
                    Some("tuning audition preparation is stale or unavailable".into());
            } else if ctx.worker.events.resident_publication_enabled() {
                let result = (|| {
                    let snapshot = ctx
                        .worker
                        .resident_snapshot
                        .as_mut()
                        .ok_or(SessionError::Invalid("native audition source unavailable"))?;
                    let mut selected = snapshot.inputs.clone();
                    selected.audition = Some(Arc::clone(&input));
                    let metadata = ResidentPublicationMetadata {
                        source_origin: crate::service::ResidentSourceOrigin::Unknown,
                        clip_preview: None,
                        context: hex_engine::AudioSourceContext {
                            project_generation: ctx.worker.project_generation,
                            editor_revision: ctx.worker.revision,
                        },
                        device_preview: None,
                        tuning_preview: None,
                        seek_request: None,
                        stopped_restore: false,
                        computer_keyboard_request: None,
                        gain_recording_track: snapshot.prepared.gain_recording_track(),
                        requires_preroll: snapshot.prepared.arrangement_seek_requires_preroll(),
                    };
                    let sink = ctx
                        .worker
                        .events
                        .resident_audio()
                        .ok_or(SessionError::Invalid("native audition output unavailable"))?;
                    resident_preview::publish_inputs(
                        &ctx.worker.session,
                        &mut ctx.worker.resident_snapshot,
                        &mut ctx.worker.resident_preview,
                        sink,
                        ctx.worker.export_lane.admission_with_resident_owners(
                            ctx.worker.compiled.as_ref().map(|(_, p)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: ctx.worker.current_sources.as_ref(),
                                    prepared: Some(p),
                                }
                            }),
                            ctx.worker.resident_seek.as_ref(),
                            None,
                        ),
                        metadata,
                        ResidentPublicationPolicy::InheritRuntime,
                        selected,
                        Some(resident_audio::RequiredInput::Audition),
                    )
                })();
                match result {
                    Ok(hint) => {
                        ctx.worker.keyboard_tuning_hint = hint;
                        ctx.worker.audition_activation = Some(input);
                        ctx.outcome.status = Some("tuning audition ready".into());
                    }
                    Err(reason) => {
                        if !ctx
                            .worker
                            .resident_snapshot
                            .as_ref()
                            .and_then(|s| s.inputs.audition.as_ref())
                            .is_some_and(|old| Arc::ptr_eq(old, &input))
                        {
                            input.cancel();
                        }
                        ctx.outcome.error = Some(reason.to_string());
                    }
                }
            } else if let Some((_, canonical)) = ctx
                .worker
                .compiled
                .as_ref()
                .filter(|(candidate, _)| *candidate == ctx.worker.revision)
            {
                match project_source_for_runtime_input(
                    &ctx.worker.session,
                    ctx.worker.tuning_gesture.snapshot(),
                    canonical,
                )
                .and_then(|(project, prepared, is_preview)| {
                    let mut playback = native_playback(Arc::clone(&prepared));
                    playback
                        .prepare_tuning_audition(&project, Arc::clone(&input))
                        .map_err(|error| error.to_string())?;
                    Ok((project, prepared, playback, is_preview))
                }) {
                    Ok((project, prepared, playback, is_preview)) => {
                        if let Some(previous) =
                            ctx.worker.audition_activation.replace(Arc::clone(&input))
                            && !Arc::ptr_eq(&previous, &input)
                        {
                            previous.cancel();
                        }
                        ctx.outcome.computer_keyboard_playback = Some(playback);
                        if is_preview {
                            ctx.outcome.preview_project = Some(project);
                            ctx.outcome.preview_prepared = Some(prepared);
                        }
                        ctx.outcome.status = Some("tuning audition ready".into());
                    }
                    Err(reason) => {
                        input.cancel();
                        ctx.outcome.error = Some(reason);
                    }
                }
            } else {
                input.cancel();
                ctx.outcome.error = Some("tuning audition requires a compiled project".into());
            }
        }

        Request::ResidentArrangementSeek(intent) => {
            let context = hex_engine::AudioSourceContext {
                project_generation: ctx.worker.project_generation,
                editor_revision: ctx.worker.revision,
            };
            let result = (|| -> Result<(), String> {
                let seek = ctx
                    .worker
                    .resident_seek
                    .as_mut()
                    .ok_or("native seek worker unavailable")?;
                let current = ctx
                    .worker
                    .resident_snapshot
                    .as_ref()
                    .ok_or("native seek source unavailable")?;
                let sink = ctx
                    .worker
                    .events
                    .resident_audio()
                    .ok_or("native seek output unavailable")?;
                seek.submit(
                    intent.clone(),
                    &ctx.worker.session,
                    current,
                    sink,
                    ctx.worker.export_lane.admission_with_resident_owners(
                        ctx.worker.compiled.as_ref().map(|(_, p)| {
                            hex_engine::native_device_resources::ControlSamplerSelection {
                                sources: ctx.worker.current_sources.as_ref(),
                                prepared: Some(p),
                            }
                        }),
                        None,
                        ctx.worker.resident_preview.as_ref(),
                    ),
                    context,
                )
            })();
            match result {
                Ok(()) => ctx.outcome.seek_submitted = true,
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        Request::PrepareComputerKeyboard {
            project_generation: generation,
            revision: expected_revision,
            request_id,
            track,
            input,
        } => {
            ctx.outcome.computer_keyboard_complete = Some(request_id);
            if generation != ctx.worker.project_generation
                || expected_revision != ctx.worker.revision
                || input.is_cancelled()
            {
                ctx.outcome.error = Some("computer keyboard preparation is stale".into());
            } else if ctx.worker.events.resident_publication_enabled()
                && ctx.worker.session.audio_mode() == DocumentAudioMode::Resident
            {
                let result = (|| {
                    let snapshot = ctx
                        .worker
                        .resident_snapshot
                        .as_mut()
                        .ok_or(SessionError::Invalid("native keyboard source unavailable"))?;
                    let sink = ctx
                        .worker
                        .events
                        .resident_audio()
                        .ok_or(SessionError::Invalid("native keyboard output unavailable"))?;
                    let mut selected = snapshot.inputs.clone();
                    selected.keyboard = Some((track, Arc::clone(&input)));
                    let metadata = ResidentPublicationMetadata {
                        source_origin: crate::service::ResidentSourceOrigin::Unknown,
                        clip_preview: None,
                        context: hex_engine::AudioSourceContext {
                            project_generation: ctx.worker.project_generation,
                            editor_revision: ctx.worker.revision,
                        },
                        device_preview: None,
                        tuning_preview: None,
                        seek_request: None,
                        stopped_restore: false,
                        computer_keyboard_request: Some(request_id),
                        gain_recording_track: snapshot.prepared.gain_recording_track(),
                        requires_preroll: snapshot.prepared.arrangement_seek_requires_preroll(),
                    };
                    resident_preview::publish_inputs(
                        &ctx.worker.session,
                        &mut ctx.worker.resident_snapshot,
                        &mut ctx.worker.resident_preview,
                        sink,
                        ctx.worker.export_lane.admission_with_resident_owners(
                            ctx.worker.compiled.as_ref().map(|(_, p)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: ctx.worker.current_sources.as_ref(),
                                    prepared: Some(p),
                                }
                            }),
                            ctx.worker.resident_seek.as_ref(),
                            None,
                        ),
                        metadata,
                        ResidentPublicationPolicy::InheritRuntime,
                        selected,
                        Some(resident_audio::RequiredInput::Keyboard),
                    )
                })();
                match result {
                    Ok(hint) => {
                        ctx.worker.keyboard_activation = Some((
                            ctx.worker.project_generation,
                            ctx.worker.revision,
                            track,
                            input,
                        ));
                        ctx.worker.keyboard_tuning_hint = hint;
                    }
                    Err(reason) => {
                        if !ctx
                            .worker
                            .resident_snapshot
                            .as_ref()
                            .and_then(|s| s.inputs.keyboard.as_ref())
                            .is_some_and(|(_, old)| Arc::ptr_eq(old, &input))
                        {
                            input.cancel();
                        }
                        ctx.outcome.error = Some(reason.to_string());
                    }
                }
            } else if ctx.outcome.private_sampler {
                let activation = (|| -> Result<(), String> {
                    let (output, epoch) = input
                        .native_sampler_activation()
                        .ok_or("sampler keyboard requires an output-bound activation")?;
                    let device = ctx
                        .worker
                        .session
                        .project()
                        .track(track)
                        .and_then(|t| t.instrument())
                        .filter(|d| {
                            !d.bypassed()
                                && d.state().type_id().as_str() == Ok("hex.instrument.sampler")
                        })
                        .ok_or("sampler keyboard track is unavailable")?
                        .id();
                    ctx.worker.events.prepare_sampler_keyboard(
                        hex_engine::AudioSourceContext {
                            project_generation: ctx.worker.project_generation,
                            editor_revision: ctx.worker.revision,
                        },
                        output,
                        epoch,
                        track,
                        device,
                    )
                })();
                if let Err(reason) = activation {
                    input.cancel();
                    ctx.outcome.error = Some(reason);
                }
            } else if let Some((_, prepared)) = ctx
                .worker
                .compiled
                .as_ref()
                .filter(|(compiled_revision, _)| *compiled_revision == ctx.worker.revision)
            {
                match project_source_for_runtime_input(
                    &ctx.worker.session,
                    ctx.worker.tuning_gesture.snapshot(),
                    prepared,
                ) {
                    Ok((source_project, source_prepared, is_preview)) => {
                        let mut playback = native_playback(source_prepared.clone());
                        match playback.prepare_computer_keyboard(
                            &source_project,
                            track,
                            Arc::clone(&input),
                        ) {
                            Ok(()) => {
                                ctx.worker.keyboard_activation = Some((
                                    ctx.worker.project_generation,
                                    ctx.worker.revision,
                                    track,
                                    input,
                                ));
                                ctx.worker.keyboard_tuning_hint =
                                    playback.computer_keyboard_tuning_hint();
                                ctx.outcome.computer_keyboard_playback = Some(playback);
                                if is_preview {
                                    ctx.outcome.preview_project = Some(source_project);
                                    ctx.outcome.preview_prepared = Some(source_prepared);
                                }
                            }
                            Err(reason) => ctx.outcome.error = Some(reason.to_string()),
                        }
                    }
                    Err(reason) => ctx.outcome.error = Some(reason),
                }
            } else {
                ctx.outcome.error =
                    Some("computer keyboard requires a compiled project revision".into());
            }
        }

        request @ (Request::RestorePlayback { .. } | Request::RepublishPausedPlayback) => {
            if let Request::RestorePlayback { generation } = &request {
                ctx.outcome.restore_complete = Some(*generation);
            }
            let republish = matches!(&request, Request::RepublishPausedPlayback);
            let context = hex_engine::AudioSourceContext {
                project_generation: ctx.worker.project_generation,
                editor_revision: ctx.worker.revision,
            };
            if !republish
                && ctx
                    .worker
                    .events
                    .resident_audio()
                    .is_some_and(|sink| sink.plugin_state_restore_pending(context))
            {
                // The exact fresh graph ACK gates the editor's cursor
                // restore and any held Play. Keep the existing owner.
                ctx.outcome.status = Some("restoring vst3 playback".into());
            } else if ctx.worker.events.resident_publication_enabled()
                && ctx.worker.session.audio_mode() == DocumentAudioMode::Resident
            {
                let result = (|| {
                    let snapshot = ctx
                        .worker
                        .resident_snapshot
                        .as_mut()
                        .ok_or(SessionError::Invalid("native restore source unavailable"))?;
                    let selected = snapshot.inputs.clone();
                    let metadata = ResidentPublicationMetadata {
                        source_origin: crate::service::ResidentSourceOrigin::Unknown,
                        clip_preview: None,
                        context,
                        device_preview: None,
                        tuning_preview: None,
                        seek_request: None,
                        stopped_restore: false,
                        computer_keyboard_request: None,
                        gain_recording_track: snapshot.prepared.gain_recording_track(),
                        requires_preroll: snapshot.prepared.arrangement_seek_requires_preroll(),
                    };
                    let sink = ctx
                        .worker
                        .events
                        .resident_audio()
                        .ok_or(SessionError::Invalid("native restore output unavailable"))?;
                    resident_preview::publish_inputs(
                        &ctx.worker.session,
                        &mut ctx.worker.resident_snapshot,
                        &mut ctx.worker.resident_preview,
                        sink,
                        ctx.worker.export_lane.admission_with_resident_owners(
                            ctx.worker.compiled.as_ref().map(|(_, p)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: ctx.worker.current_sources.as_ref(),
                                    prepared: Some(p),
                                }
                            }),
                            ctx.worker.resident_seek.as_ref(),
                            None,
                        ),
                        metadata,
                        ResidentPublicationPolicy::InheritRuntime,
                        selected,
                        None,
                    )
                })();
                match result {
                    Ok(hint) => {
                        ctx.worker.keyboard_tuning_hint = hint;
                        ctx.outcome.status = Some(
                            if republish {
                                "paused audio updated · effect tails reset"
                            } else {
                                "playback restored"
                            }
                            .into(),
                        );
                    }
                    Err(reason) => ctx.outcome.error = Some(reason.to_string()),
                }
            } else {
                match ctx.worker.compiled.as_ref() {
                    Some((compiled_revision, prepared))
                        if *compiled_revision == ctx.worker.revision =>
                    {
                        ctx.outcome.restore_prepared = Some(Arc::clone(prepared));
                    }
                    _ => match compile(
                        &ctx.worker.session,
                        ctx.worker
                            .compiled
                            .as_ref()
                            .map(|(_, artifact)| artifact.as_ref()),
                    ) {
                        Ok(prepared) => {
                            ctx.worker.compiled =
                                Some((ctx.worker.revision, Arc::clone(&prepared)));
                            ctx.worker.midi_preview = Arc::clone(prepared.midi_preview());
                            ctx.worker.state = view_state(
                                &ctx.worker.session,
                                Arc::clone(&ctx.worker.midi_preview),
                                ctx.worker.revision,
                                ctx.worker.project_generation,
                                ctx.worker.unsaved,
                                ctx.worker
                                    .document
                                    .as_ref()
                                    .map(|handle| handle.path.as_path()),
                            );
                            ctx.outcome.restore_prepared = Some(prepared);
                        }
                        Err(reason) => {
                            ctx.worker.compiled = None;
                            ctx.outcome.error = Some(reason);
                        }
                    },
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
    fn compile_produces_prepared_revision_for_fresh_session() {
        let session = Session::new(48_000, 4, 0).expect("session");
        let prepared = compile(&session, None).expect("compile");
        let playback = native_playback(prepared);
        assert!(playback.prepared_revision().is_some());
    }
}
