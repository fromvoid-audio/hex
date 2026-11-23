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
        mut resident_edit_source_unavailable,
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
        Request::RestartAudio {
            project_generation: expected,
            request_id,
        } => {
            if expected != project_generation || request_id == 0 {
                error = Some("audio restart request is stale".into());
            } else {
                match events.restart_audio(project_generation, request_id) {
                    Ok(()) => {
                        if let Some(retired) = device_gesture.retire_native_on_restart() {
                            if resident_preview
                                .as_ref()
                                .and_then(resident_preview::Preview::device_identity)
                                .is_some_and(|preview| preview.key == retired.key)
                            {
                                resident_preview = None;
                            }
                            gesture_ack = Some(retired);
                        }
                        status = Some("restarting audio".into());
                        let context = hex_engine::AudioSourceContext {
                            project_generation,
                            editor_revision: revision,
                        };
                        if session.audio_mode() == DocumentAudioMode::Resident
                            && let Some(sink) = events.resident_audio()
                            && !sink.plugin_state_restore_pending(context)
                        {
                            let admission = export_lane.admission_with_resident_owners(
                                compiled.as_ref().map(|(_, prepared)| {
                                    hex_engine::native_device_resources::ControlSamplerSelection {
                                        sources: current_sources.as_ref(),
                                        prepared: Some(prepared),
                                    }
                                }),
                                resident_seek.as_ref(),
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
                                gain_recording_track: resident_snapshot
                                    .as_ref()
                                    .and_then(|snapshot| snapshot.prepared.gain_recording_track()),
                                requires_preroll: resident_snapshot.as_ref().is_some_and(
                                    |snapshot| {
                                        snapshot.prepared.arrangement_seek_requires_preroll()
                                    },
                                ),
                            };
                            // Restart is an explicit output shutdown, independent of authoring. Replay
                            // the actual canonical/preview owner through offline custody while its old
                            // book retires. Old monitor clients belong to the relinquished Client.
                            if let Err(reason) = resident_preview::publish_inputs(
                                &session,
                                &mut resident_snapshot,
                                &mut resident_preview,
                                sink,
                                admission,
                                metadata,
                                ResidentPublicationPolicy::InstallFreshRuntime,
                                resident_audio::RuntimeInputs::default(),
                                None,
                            ) {
                                error = Some(format!("audio restart graph unavailable: {reason}"));
                            }
                        }
                    }
                    Err(reason) => error = Some(reason),
                }
            }
        }

        Request::TuningGesture(gesture) => {
            tuning_gesture_request = true;
            gesture_protocol_ack = true;
            let outcome = if device_gesture.snapshot().is_some()
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
            } else if events.resident_publication_enabled()
                && session.audio_mode() == DocumentAudioMode::Resident
            {
                let admission = export_lane.admission_with_resident_owners(
                    compiled.as_ref().map(|(_, prepared)| {
                        hex_engine::native_device_resources::ControlSamplerSelection {
                            sources: current_sources.as_ref(),
                            prepared: Some(prepared),
                        }
                    }),
                    resident_seek.as_ref(),
                    None,
                );
                tuning_gesture.handle_prepared(
                    &mut session,
                    gesture,
                    compiled.as_ref().map(|(_, artifact)| artifact),
                    project_generation,
                    |session, operation| {
                        let sink = events
                            .resident_audio()
                            .ok_or(SessionError::Invalid("native tuning output unavailable"))?;
                        resident_preview::tuning_operation(
                            session,
                            operation,
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision,
                            },
                            &mut resident_snapshot,
                            &mut resident_preview,
                            sink,
                            admission,
                        )
                    },
                )
            } else {
                tuning_gesture.handle_with(
                    &mut session,
                    gesture,
                    compiled.as_ref().map(|(_, artifact)| artifact),
                    project_generation,
                    |source, target, prepared| {
                        tuning_input_publication = prepare_tuning_input_publication(
                            source,
                            target,
                            prepared,
                            project_generation,
                            revision,
                            keyboard_activation.as_ref(),
                            keyboard_tuning_hint.as_ref(),
                            audition_activation.as_ref(),
                            true,
                        )?;
                        Ok(())
                    },
                )
            };
            let native_terminal_accepted = events.resident_publication_enabled()
                && session.audio_mode() == DocumentAudioMode::Resident
                && outcome.ack.accepted
                && matches!(
                    gesture.phase(),
                    crate::tuning_gesture::TuningGesturePhase::Commit
                        | crate::tuning_gesture::TuningGesturePhase::Cancel
                );
            tuning_gesture_ack = Some(outcome.ack.clone());
            if let Some(prepared) = outcome.playback {
                if native_terminal_accepted && !outcome.canonical_changed {
                    compiled = Some((revision, Arc::clone(&prepared)));
                }
                if outcome.canonical_changed {
                    unsaved = true;
                    replacement_revision = Some(prepared);
                } else if matches!(gesture, TuningGestureRequest::Change { .. }) {
                    preview_prepared = Some(prepared);
                    preview_project = outcome.preview_project;
                } else {
                    restore_prepared = Some(prepared);
                }
            }
        }

        Request::PreparePausedInputMonitor { request, reply } => {
            let result = (|| {
                if request.source.context
                    != (hex_engine::AudioSourceContext {
                        project_generation,
                        editor_revision: revision,
                    })
                    || session.audio_mode() != DocumentAudioMode::Resident
                {
                    return Err("paused input monitor source changed".into());
                }
                let sink = events
                    .resident_audio()
                    .ok_or("paused input monitor output is unavailable")?;
                let snapshot = resident_snapshot
                    .as_ref()
                    .ok_or("paused input monitor graph is unavailable")?;
                session.prepare_resident_paused_input_monitor(
                    request,
                    snapshot,
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
                )
            })();
            let _ = reply.try_send(result);
        }

        Request::RestoreStoppedPlayback | Request::ContinueStoppedPlaybackRestore => {
            stopped_playback_restore_complete = true;
            if device_gesture.snapshot().is_some() {
                error = Some("finish the device gesture before restoring stopped playback".into());
            } else if events.resident_publication_enabled()
                && session.audio_mode() == DocumentAudioMode::Resident
            {
                let result = (|| {
                    let context = hex_engine::AudioSourceContext {
                        project_generation,
                        editor_revision: revision,
                    };
                    if pending_stopped_restore.is_some_and(|old| old != context) {
                        return Err(SessionError::Invalid("stopped restore source changed"));
                    }
                    let sink = events
                        .resident_audio()
                        .ok_or(SessionError::Invalid("native restore output unavailable"))?;
                    match sink
                        .prepare_stopped_playback_restore(context)
                        .map_err(SessionError::NativeAudio)?
                    {
                        Some(false) => {
                            pending_stopped_restore = Some(context);
                            stopped_playback_restore_complete = false;
                            return Ok(());
                        }
                        Some(true) => {
                            let snapshot = session.restore_resident_snapshot(
                                context, sink,
                                export_lane.admission_with_resident_owners(compiled.as_ref().map(|(_, p)| hex_engine::native_device_resources::ControlSamplerSelection { sources: current_sources.as_ref(), prepared: Some(p) }), resident_seek.as_ref(), None),
                                resident_snapshot.as_ref(),
                            )?;
                            compiled = Some((revision, Arc::clone(&snapshot.prepared)));
                            resident_snapshot = Some(snapshot);
                            resident_preview = None;
                            pending_stopped_restore = None;
                            return Ok(());
                        }
                        None => {}
                    }
                    let snapshot = resident_snapshot.as_mut().ok_or(SessionError::Invalid(
                        "current playback revision is unavailable",
                    ))?;
                    let selected = snapshot.inputs.clone();
                    let metadata = ResidentPublicationMetadata {
                        source_origin: crate::service::ResidentSourceOrigin::Unknown,
                        clip_preview: None,
                        context: hex_engine::AudioSourceContext {
                            project_generation,
                            editor_revision: revision,
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
                        &session,
                        &mut resident_snapshot,
                        &mut resident_preview,
                        sink,
                        export_lane.admission_with_resident_owners(
                            compiled.as_ref().map(|(_, p)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: current_sources.as_ref(),
                                    prepared: Some(p),
                                }
                            }),
                            resident_seek.as_ref(),
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
                    Ok(_) => status = Some("restoring stopped playback".into()),
                    Err(reason) => {
                        pending_stopped_restore = None;
                        if let Some(sink) = events.resident_audio() {
                            sink.cancel_stopped_playback_restore();
                        }
                        error = Some(reason.to_string());
                    }
                }
            } else if let Some((compiled_revision, prepared)) = compiled.as_ref()
                && *compiled_revision == revision
            {
                restore_prepared = Some(Arc::clone(prepared));
                status = Some("restoring stopped playback".into());
            } else {
                error = Some("current playback revision is unavailable".into());
            }
        }

        Request::PrepareLiveExpression {
            input,
            revision: expected_revision,
        } => {
            if session.audio_mode() == DocumentAudioMode::VerifiedStreaming {
                input.cancel();
                error = Some("controller input is unavailable for verified streaming audio".into());
            } else if input.project_generation != project_generation
                || expected_revision != revision
                || input.is_cancelled()
            {
                if !events.resident_publication_enabled()
                    || !resident_snapshot
                        .as_ref()
                        .and_then(|s| s.inputs.expression.as_ref())
                        .is_some_and(|old| Arc::ptr_eq(old, &input))
                {
                    input.cancel();
                }
                error = Some("controller input preparation is stale".into());
            } else if events.resident_publication_enabled() {
                let result = (|| {
                    let snapshot = resident_snapshot.as_mut().ok_or(SessionError::Invalid(
                        "native controller source unavailable",
                    ))?;
                    let mut selected = snapshot.inputs.clone();
                    selected.expression = Some(Arc::clone(&input));
                    let metadata = ResidentPublicationMetadata {
                        source_origin: crate::service::ResidentSourceOrigin::Unknown,
                        clip_preview: None,
                        context: hex_engine::AudioSourceContext {
                            project_generation,
                            editor_revision: revision,
                        },
                        device_preview: None,
                        tuning_preview: None,
                        seek_request: None,
                        stopped_restore: false,
                        computer_keyboard_request: None,
                        gain_recording_track: snapshot.prepared.gain_recording_track(),
                        requires_preroll: snapshot.prepared.arrangement_seek_requires_preroll(),
                    };
                    let sink = events.resident_audio().ok_or(SessionError::Invalid(
                        "native controller output unavailable",
                    ))?;
                    resident_preview::publish_inputs(
                        &session,
                        &mut resident_snapshot,
                        &mut resident_preview,
                        sink,
                        export_lane.admission_with_resident_owners(
                            compiled.as_ref().map(|(_, p)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: current_sources.as_ref(),
                                    prepared: Some(p),
                                }
                            }),
                            resident_seek.as_ref(),
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
                        keyboard_tuning_hint = hint;
                        live_expression_complete = Some(Arc::clone(&input));
                        live_expression = Some(input);
                    }
                    Err(reason) => {
                        if !resident_snapshot
                            .as_ref()
                            .and_then(|s| s.inputs.expression.as_ref())
                            .is_some_and(|old| Arc::ptr_eq(old, &input))
                        {
                            input.cancel();
                        }
                        error = Some(reason.to_string());
                    }
                }
            } else if let Some((_, prepared)) = compiled
                .as_ref()
                .filter(|(compiled_revision, _)| *compiled_revision == revision)
            {
                let source =
                    project_source_for_runtime_input(&session, tuning_gesture.snapshot(), prepared);
                let prepared =
                    source.and_then(|(source_project, source_prepared, is_preview)| {
                        let mut playback = native_playback(Arc::clone(&source_prepared));
                        let result = playback
                        .prepare_live_expression(&source_project, Arc::clone(&input))
                    .and_then(|()| {
                        if let Some((_, _, track, keyboard)) = keyboard_activation
                            .as_ref()
                            .filter(|(generation, prepared_revision, _, keyboard)| {
                                *generation == project_generation
                                    && *prepared_revision == revision
                                    && !keyboard.is_cancelled()
                            })
                        {
                            if let Some(hint) = keyboard_tuning_hint.as_ref() {
                                playback.prepare_computer_keyboard_for_controller_activation_with_tuning_hint(&source_project, *track, Arc::clone(keyboard), hint)?;
                            } else {
                                playback.prepare_computer_keyboard_for_controller_activation(&source_project, *track, Arc::clone(keyboard))?;
                            }
                        }
                        Ok(())
                    });
                        if result.is_ok() && is_preview {
                            preview_prepared = Some(source_prepared);
                            preview_project = Some(source_project);
                        }
                        result.map(|()| playback).map_err(|error| error.to_string())
                    });
                match prepared {
                    Ok(playback) => {
                        live_expression_complete = Some(Arc::clone(&input));
                        if let Some(previous) = live_expression.replace(Arc::clone(&input))
                            && !Arc::ptr_eq(&previous, &input)
                        {
                            previous.cancel();
                        }
                        keyboard_tuning_hint = playback.computer_keyboard_tuning_hint();
                        computer_keyboard_playback = Some(playback);
                    }
                    Err(reason) => {
                        input.cancel();
                        error = Some(reason.to_string());
                    }
                }
            } else {
                input.cancel();
                error = Some("controller input requires a compiled project revision".into());
            }
        }

        Request::PrepareTuningAudition {
            revision: expected_revision,
            input,
        } => {
            tuning_audition_complete = Some(Arc::clone(&input));
            if session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                || expected_revision != revision
                || input.project_generation != project_generation
                || input.is_cancelled()
            {
                if !events.resident_publication_enabled()
                    || !resident_snapshot
                        .as_ref()
                        .and_then(|s| s.inputs.audition.as_ref())
                        .is_some_and(|old| Arc::ptr_eq(old, &input))
                {
                    input.cancel();
                }
                error = Some("tuning audition preparation is stale or unavailable".into());
            } else if events.resident_publication_enabled() {
                let result = (|| {
                    let snapshot = resident_snapshot
                        .as_mut()
                        .ok_or(SessionError::Invalid("native audition source unavailable"))?;
                    let mut selected = snapshot.inputs.clone();
                    selected.audition = Some(Arc::clone(&input));
                    let metadata = ResidentPublicationMetadata {
                        source_origin: crate::service::ResidentSourceOrigin::Unknown,
                        clip_preview: None,
                        context: hex_engine::AudioSourceContext {
                            project_generation,
                            editor_revision: revision,
                        },
                        device_preview: None,
                        tuning_preview: None,
                        seek_request: None,
                        stopped_restore: false,
                        computer_keyboard_request: None,
                        gain_recording_track: snapshot.prepared.gain_recording_track(),
                        requires_preroll: snapshot.prepared.arrangement_seek_requires_preroll(),
                    };
                    let sink = events
                        .resident_audio()
                        .ok_or(SessionError::Invalid("native audition output unavailable"))?;
                    resident_preview::publish_inputs(
                        &session,
                        &mut resident_snapshot,
                        &mut resident_preview,
                        sink,
                        export_lane.admission_with_resident_owners(
                            compiled.as_ref().map(|(_, p)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: current_sources.as_ref(),
                                    prepared: Some(p),
                                }
                            }),
                            resident_seek.as_ref(),
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
                        keyboard_tuning_hint = hint;
                        audition_activation = Some(input);
                        status = Some("tuning audition ready".into());
                    }
                    Err(reason) => {
                        if !resident_snapshot
                            .as_ref()
                            .and_then(|s| s.inputs.audition.as_ref())
                            .is_some_and(|old| Arc::ptr_eq(old, &input))
                        {
                            input.cancel();
                        }
                        error = Some(reason.to_string());
                    }
                }
            } else if let Some((_, canonical)) = compiled
                .as_ref()
                .filter(|(candidate, _)| *candidate == revision)
            {
                match project_source_for_runtime_input(
                    &session,
                    tuning_gesture.snapshot(),
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
                        if let Some(previous) = audition_activation.replace(Arc::clone(&input))
                            && !Arc::ptr_eq(&previous, &input)
                        {
                            previous.cancel();
                        }
                        computer_keyboard_playback = Some(playback);
                        if is_preview {
                            preview_project = Some(project);
                            preview_prepared = Some(prepared);
                        }
                        status = Some("tuning audition ready".into());
                    }
                    Err(reason) => {
                        input.cancel();
                        error = Some(reason);
                    }
                }
            } else {
                input.cancel();
                error = Some("tuning audition requires a compiled project".into());
            }
        }

        Request::ResidentArrangementSeek(intent) => {
            let context = hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision,
            };
            let result = (|| -> Result<(), String> {
                let seek = resident_seek
                    .as_mut()
                    .ok_or("native seek worker unavailable")?;
                let current = resident_snapshot
                    .as_ref()
                    .ok_or("native seek source unavailable")?;
                let sink = events
                    .resident_audio()
                    .ok_or("native seek output unavailable")?;
                seek.submit(
                    intent.clone(),
                    &session,
                    current,
                    sink,
                    export_lane.admission_with_resident_owners(
                        compiled.as_ref().map(|(_, p)| {
                            hex_engine::native_device_resources::ControlSamplerSelection {
                                sources: current_sources.as_ref(),
                                prepared: Some(p),
                            }
                        }),
                        None,
                        resident_preview.as_ref(),
                    ),
                    context,
                )
            })();
            match result {
                Ok(()) => seek_submitted = true,
                Err(reason) => error = Some(reason),
            }
        }

        Request::PrepareComputerKeyboard {
            project_generation: generation,
            revision: expected_revision,
            request_id,
            track,
            input,
        } => {
            computer_keyboard_complete = Some(request_id);
            if generation != project_generation
                || expected_revision != revision
                || input.is_cancelled()
            {
                error = Some("computer keyboard preparation is stale".into());
            } else if events.resident_publication_enabled()
                && session.audio_mode() == DocumentAudioMode::Resident
            {
                let result = (|| {
                    let snapshot = resident_snapshot
                        .as_mut()
                        .ok_or(SessionError::Invalid("native keyboard source unavailable"))?;
                    let sink = events
                        .resident_audio()
                        .ok_or(SessionError::Invalid("native keyboard output unavailable"))?;
                    let mut selected = snapshot.inputs.clone();
                    selected.keyboard = Some((track, Arc::clone(&input)));
                    let metadata = ResidentPublicationMetadata {
                        source_origin: crate::service::ResidentSourceOrigin::Unknown,
                        clip_preview: None,
                        context: hex_engine::AudioSourceContext {
                            project_generation,
                            editor_revision: revision,
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
                        &session,
                        &mut resident_snapshot,
                        &mut resident_preview,
                        sink,
                        export_lane.admission_with_resident_owners(
                            compiled.as_ref().map(|(_, p)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: current_sources.as_ref(),
                                    prepared: Some(p),
                                }
                            }),
                            resident_seek.as_ref(),
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
                        keyboard_activation = Some((project_generation, revision, track, input));
                        keyboard_tuning_hint = hint;
                    }
                    Err(reason) => {
                        if !resident_snapshot
                            .as_ref()
                            .and_then(|s| s.inputs.keyboard.as_ref())
                            .is_some_and(|(_, old)| Arc::ptr_eq(old, &input))
                        {
                            input.cancel();
                        }
                        error = Some(reason.to_string());
                    }
                }
            } else if private_sampler {
                let activation = (|| -> Result<(), String> {
                    let (output, epoch) = input
                        .native_sampler_activation()
                        .ok_or("sampler keyboard requires an output-bound activation")?;
                    let device = session
                        .project()
                        .track(track)
                        .and_then(|t| t.instrument())
                        .filter(|d| {
                            !d.bypassed()
                                && d.state().type_id().as_str() == Ok("hex.instrument.sampler")
                        })
                        .ok_or("sampler keyboard track is unavailable")?
                        .id();
                    events.prepare_sampler_keyboard(
                        hex_engine::AudioSourceContext {
                            project_generation,
                            editor_revision: revision,
                        },
                        output,
                        epoch,
                        track,
                        device,
                    )
                })();
                if let Err(reason) = activation {
                    input.cancel();
                    error = Some(reason);
                }
            } else if let Some((_, prepared)) = compiled
                .as_ref()
                .filter(|(compiled_revision, _)| *compiled_revision == revision)
            {
                match project_source_for_runtime_input(
                    &session,
                    tuning_gesture.snapshot(),
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
                                keyboard_activation =
                                    Some((project_generation, revision, track, input));
                                keyboard_tuning_hint = playback.computer_keyboard_tuning_hint();
                                computer_keyboard_playback = Some(playback);
                                if is_preview {
                                    preview_project = Some(source_project);
                                    preview_prepared = Some(source_prepared);
                                }
                            }
                            Err(reason) => error = Some(reason.to_string()),
                        }
                    }
                    Err(reason) => error = Some(reason),
                }
            } else {
                error = Some("computer keyboard requires a compiled project revision".into());
            }
        }

        request @ (Request::RestorePlayback { .. } | Request::RepublishPausedPlayback) => {
            if let Request::RestorePlayback { generation } = &request {
                restore_complete = Some(*generation);
            }
            let republish = matches!(&request, Request::RepublishPausedPlayback);
            let context = hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision,
            };
            if !republish
                && events
                    .resident_audio()
                    .is_some_and(|sink| sink.plugin_state_restore_pending(context))
            {
                // The exact fresh graph ACK gates the editor's cursor
                // restore and any held Play. Keep the existing owner.
                status = Some("restoring vst3 playback".into());
            } else if events.resident_publication_enabled()
                && session.audio_mode() == DocumentAudioMode::Resident
            {
                let result = (|| {
                    let snapshot = resident_snapshot
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
                    let sink = events
                        .resident_audio()
                        .ok_or(SessionError::Invalid("native restore output unavailable"))?;
                    resident_preview::publish_inputs(
                        &session,
                        &mut resident_snapshot,
                        &mut resident_preview,
                        sink,
                        export_lane.admission_with_resident_owners(
                            compiled.as_ref().map(|(_, p)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: current_sources.as_ref(),
                                    prepared: Some(p),
                                }
                            }),
                            resident_seek.as_ref(),
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
                        keyboard_tuning_hint = hint;
                        status = Some(
                            if republish {
                                "paused audio updated · effect tails reset"
                            } else {
                                "playback restored"
                            }
                            .into(),
                        );
                    }
                    Err(reason) => error = Some(reason.to_string()),
                }
            } else {
                match compiled.as_ref() {
                    Some((compiled_revision, prepared)) if *compiled_revision == revision => {
                        restore_prepared = Some(Arc::clone(prepared));
                    }
                    _ => match compile(
                        &session,
                        compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                    ) {
                        Ok(prepared) => {
                            compiled = Some((revision, Arc::clone(&prepared)));
                            midi_preview = Arc::clone(prepared.midi_preview());
                            state = view_state(
                                &session,
                                Arc::clone(&midi_preview),
                                revision,
                                project_generation,
                                unsaved,
                                document.as_ref().map(|handle| handle.path.as_path()),
                            );
                            restore_prepared = Some(prepared);
                        }
                        Err(reason) => {
                            compiled = None;
                            error = Some(reason);
                        }
                    },
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
        resident_edit_source_unavailable,
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
    fn compile_produces_prepared_revision_for_fresh_session() {
        let session = Session::new(48_000, 4, 0).expect("session");
        let prepared = compile(&session, None).expect("compile");
        let playback = native_playback(prepared);
        assert!(playback.prepared_revision().is_some());
    }
}
