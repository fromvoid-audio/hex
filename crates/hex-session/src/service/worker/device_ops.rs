//! Device, plugin, sampler, automation, preset and source-utility worker operations.
use super::*;
use std::ops::ControlFlow;

/// Registered presentation name, offered as the automatic track name on device
/// loads; `hex_project::track_name` owns whether the track adopts it.
pub(crate) fn native_device_name(
    class: hex_engine::NativeDeviceClass,
    type_id: hex_lattice_contract::DeviceTypeId,
) -> Result<String, String> {
    hex_engine::native_device_catalog_for_class(class)
        .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))?
        .into_iter()
        .find(|entry| entry.type_id == type_id)
        .map_or_else(
            // Unlisted definitions present their type identifier, as device views do.
            || {
                type_id
                    .as_str()
                    .map(str::to_owned)
                    .map_err(|_| "native device type identifier is invalid".to_owned())
            },
            |entry| Ok(entry.name),
        )
}

pub(crate) fn device_track_command(
    before_track: Option<hex_project::TrackId>,
    slot: hex_project::TrackDeviceSlot,
    state: hex_lattice_contract::NativeDeviceState,
    device_name: String,
    gain: Option<f32>,
) -> Command {
    match gain {
        Some(gain) => Command::InsertDefaultTrackWithDeviceAndGain {
            before_track,
            slot,
            state,
            device_name,
            gain,
        },
        None => Command::InsertDefaultTrackWithDevice {
            before_track,
            slot,
            state,
            device_name,
        },
    }
}

pub(crate) fn arrangement_device_command(
    project: &hex_project::Project,
    class: hex_engine::NativeDeviceClass,
    type_id: hex_lattice_contract::DeviceTypeId,
    destination: ArrangementDeviceDestination,
    new_track_gain: Option<f32>,
) -> Result<Command, String> {
    use hex_engine::NativeDeviceClass;
    let state = hex_engine::default_native_device_state_for_class(class, type_id)
        .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))?;
    match destination {
        ArrangementDeviceDestination::NewTrack { before_track } => Ok(device_track_command(
            before_track,
            match class {
                NativeDeviceClass::Instrument => hex_project::TrackDeviceSlot::Instrument,
                NativeDeviceClass::Effect => hex_project::TrackDeviceSlot::Effect,
            },
            state,
            native_device_name(class, type_id)?,
            new_track_gain,
        )),
        ArrangementDeviceDestination::ExistingTrack { track, instrument } => {
            let target = project
                .track(track)
                .ok_or("device drop track no longer exists")?;
            if target.instrument().map(hex_project::Device::id) != instrument {
                return Err("instrument replacement target changed".into());
            }
            Ok(match class {
                NativeDeviceClass::Effect => Command::InsertDevice {
                    track,
                    before: None,
                    state,
                },
                NativeDeviceClass::Instrument => Command::LoadInstrument {
                    track,
                    replacing: instrument,
                    state,
                    device_name: native_device_name(class, type_id)?,
                },
            })
        }
    }
}

pub(super) fn handle(ctx: &mut WorkerDispatch<'_>, request: Request) -> ControlFlow<()> {
    let mut flow = ControlFlow::Continue(());
    match request {
        Request::DeviceGesture(gesture) => {
            let gesture = *gesture;
            let independent_plugin_eq = ctx
                .worker
                .device_gesture
                .snapshot()
                .filter(|active| active.key == gesture.key())
                .and_then(|active| {
                    ctx.worker
                        .pending_plugin_effect
                        .as_ref()?
                        .independent_eq_gesture(ctx.worker.session.project(), active.target)
                });
            ctx.outcome.gesture_request = true;
            ctx.outcome.gesture_protocol_ack = true;
            let mut deferred = false;
            let outcome = if ctx.worker.tuning_gesture.snapshot().is_some()
                && matches!(
                    gesture,
                    GestureRequest::Begin { .. }
                        | GestureRequest::BeginTrackGains { .. }
                        | GestureRequest::BeginPlugin { .. }
                        | GestureRequest::BeginSnapshot { .. }
                        | GestureRequest::BeginEqCreation { .. }
                ) {
                crate::device_gesture::GestureOutcome {
                    ack: GestureAck {
                        preview_retained: false,
                        source_lost: false,
                        key: gesture.key(),
                        phase: gesture.phase(),
                        sequence: gesture.sequence(),
                        accepted: false,
                        reason: Some("project tuning gesture is active".into()),
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
                let mut outcome = ctx.worker.device_gesture.handle_prepared(
                    &mut ctx.worker.session,
                    gesture,
                    ctx.worker.compiled.as_ref().map(|(_, artifact)| artifact),
                    ctx.worker.project_generation,
                    |session, operation| {
                        let sink = ctx
                            .worker
                            .events
                            .resident_audio()
                            .ok_or(SessionError::Invalid("native preview output unavailable"))?;
                        let mut recording = plugin_recording::Preparation {
                            state: &mut ctx.worker.plugin_recording,
                            lane: &mut ctx.worker.native_automation_take,
                            document_base: ctx.worker.media_base.as_deref(),
                            recovery: ctx.worker.recovery.as_deref(),
                            may_defer: matches!(
                                gesture.phase(),
                                crate::device_gesture::GesturePhase::Commit
                                    | crate::device_gesture::GesturePhase::Cancel
                            ),
                            deferred: false,
                        };
                        if let crate::device_gesture::GesturePreparation::Plugin(operation) =
                            operation
                        {
                            let result = resident_preview::plugin_parameter::operation_using(
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
                                Some(&mut recording),
                            );
                            deferred |= recording.deferred;
                            return result;
                        }
                        resident_preview::device_operation(
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
                );
                if !deferred
                    && ctx
                        .worker
                        .resident_preview
                        .as_ref()
                        .and_then(resident_preview::Preview::device_identity)
                        .is_none_or(|preview| preview.key != outcome.ack.key)
                {
                    ctx.worker
                        .device_gesture
                        .retire_unpublished_native_terminal(&mut outcome.ack);
                }
                outcome
            } else if ctx.worker.session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                && audio_sink::requires_plugin_profile(ctx.worker.session.project())
                && ctx.worker.events.resident_audio().is_some()
            {
                let previous = ctx.worker.compiled.as_ref().map(|(_, artifact)| artifact);
                ctx.worker.device_gesture.handle_verified(
                    &mut ctx.worker.session,
                    gesture,
                    previous,
                    ctx.worker.project_generation,
                    |session, operation| {
                        let sink = ctx
                            .worker
                            .events
                            .resident_audio()
                            .ok_or(SessionError::Invalid("native preview output unavailable"))?;
                        session.verified_device_gesture_operation(
                            operation,
                            previous,
                            hex_engine::AudioSourceContext {
                                project_generation: ctx.worker.project_generation,
                                editor_revision: ctx.worker.revision,
                            },
                            sink,
                        )
                    },
                )
            } else {
                ctx.worker.device_gesture.handle(
                    &mut ctx.worker.session,
                    gesture,
                    ctx.worker.compiled.as_ref().map(|(_, artifact)| artifact),
                    ctx.worker.project_generation,
                )
            };
            // Begin may renew a stopped native epoch before metadata
            // admission. Retain the new canonical resources even if
            // the subsequent target validation refuses this gesture.
            if matches!(gesture, GestureRequest::BeginPlugin { .. })
                && ctx.worker.resident_preview.is_none()
                && let Some(snapshot) = ctx.worker.resident_snapshot.as_ref()
            {
                ctx.worker.compiled = Some((ctx.worker.revision, Arc::clone(&snapshot.prepared)));
            }
            let native_terminal_accepted = ctx.worker.events.resident_publication_enabled()
                && ctx.worker.session.audio_mode() == DocumentAudioMode::Resident
                && outcome.ack.accepted
                && matches!(
                    gesture.phase(),
                    crate::device_gesture::GesturePhase::Commit
                        | crate::device_gesture::GesturePhase::Cancel
                );
            if outcome.ack.accepted
                && outcome.canonical_changed
                && let Some(device) = independent_plugin_eq
                && let Some(pending) = ctx.worker.pending_plugin_effect.as_mut()
            {
                pending.rebase_after_accepted_eq_write(
                    hex_engine::AudioSourceContext {
                        project_generation: ctx.worker.project_generation,
                        editor_revision: ctx.worker.revision,
                    },
                    ctx.worker.session.project(),
                    device,
                );
            }
            if deferred {
                ctx.outcome.request_complete = false;
                ctx.worker.plugin_recording.defer(gesture);
                ctx.outcome.gesture_protocol_ack = false;
                ctx.outcome.gesture_request = false;
            } else {
                ctx.worker.plugin_recording.attempt(gesture);
                ctx.outcome.gesture_ack = Some(outcome.ack);
            }
            if let Some(prepared) = outcome.playback {
                if native_terminal_accepted && !outcome.canonical_changed {
                    ctx.worker.compiled = Some((ctx.worker.revision, Arc::clone(&prepared)));
                }
                if outcome.canonical_changed {
                    ctx.worker.unsaved = true;
                    ctx.outcome.replacement_revision = Some(prepared);
                } else if matches!(
                    gesture,
                    GestureRequest::Change { .. }
                        | GestureRequest::ChangePlugin { .. }
                        | GestureRequest::ChangePair { .. }
                        | GestureRequest::ChangeSnapshot { .. }
                        | GestureRequest::ChangeEqCreation { .. }
                ) {
                    ctx.outcome.preview_prepared = Some(prepared);
                    ctx.outcome.preview_project = outcome.preview_project;
                } else {
                    ctx.outcome.restore_prepared = Some(prepared);
                }
            }
        }

        Request::MoveTrackDevices { .. } => {
            ctx.outcome.error = Some("device move was not resolved".into());
        }

        Request::ReorderDevices { .. } => {
            ctx.outcome.error = Some("device reorder was not resolved".into());
        }

        Request::AppendSourceUtility { .. }
        | Request::MoveSourceUtility { .. }
        | Request::RemoveSourceUtility { .. }
        | Request::BindUnassignedNativeSource { .. }
        | Request::RebindNativeSource { .. }
        | Request::ChooseNativeSourceReceiver { .. }
        | Request::RenameRetainedSource { .. }
        | Request::DeleteRetainedSource { .. }
        | Request::CreateRetainedNativeSource { .. } => {
            ctx.outcome.error = Some("source assignment was not resolved".into());
        }

        Request::InsertBrowserInstrumentTrack { .. } => {
            ctx.outcome.error =
                Some("instrument insertion request was not prepared; select it again".into());
        }

        Request::ReloadPlugin {
            project_generation: target_generation,
            device,
        } => {
            if ctx.worker.device_gesture.snapshot().is_some() {
                ctx.outcome.error =
                    Some("finish the device gesture before reloading a vst3 plugin".into());
            } else if target_generation != ctx.worker.project_generation
                || !ctx
                    .worker
                    .session
                    .project()
                    .device(device)
                    .is_some_and(|entry| {
                        entry.state().type_id().as_str() == Ok("hex.vst3") && !entry.bypassed()
                    })
            {
                ctx.outcome.error = Some("vst3 reload target changed or is powered off".into());
            } else if !ctx.worker.events.resident_publication_enabled()
                || ctx.worker.session.audio_mode() != DocumentAudioMode::Resident
            {
                ctx.outcome.error = Some("vst3 reload requires resident audio".into());
            } else {
                let context = hex_engine::AudioSourceContext {
                    project_generation: ctx.worker.project_generation,
                    editor_revision: ctx.worker.revision,
                };
                let result = (|| {
                    let sink = ctx
                        .worker
                        .events
                        .resident_audio()
                        .ok_or(SessionError::Invalid("native reload output unavailable"))?;
                    let snapshot = ctx.worker.session.reload_plugin_snapshot(
                        context,
                        device,
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
                        ctx.worker.resident_snapshot.as_ref(),
                    )?;
                    ctx.worker.compiled =
                        Some((ctx.worker.revision, Arc::clone(&snapshot.prepared)));
                    ctx.worker.resident_snapshot = Some(snapshot);
                    ctx.worker.resident_preview = None;
                    Ok::<_, SessionError>(())
                })();
                match result {
                    Ok(()) => ctx.outcome.status = Some("vst3 plugin reloaded".into()),
                    Err(error) => ctx.outcome.error = Some(error.to_string()),
                }
            }
        }

        Request::TogglePluginEditor {
            project_generation: target_generation,
            device,
        } => {
            if target_generation != ctx.worker.project_generation
                || !ctx
                    .worker
                    .session
                    .project()
                    .device(device)
                    .is_some_and(|device| device.state().type_id().as_str() == Ok("hex.vst3"))
            {
                ctx.outcome.error = Some("vst3 editor target changed".into());
            } else {
                let outcome = ctx
                    .worker
                    .events
                    .sampler_edit_audio()
                    .ok_or_else(|| "vst3 editor unavailable for this output".to_owned())
                    .and_then(|audio| {
                        audio.toggle_plugin_editor(ctx.worker.project_generation, device.value())
                    });
                if let Err(reason) = outcome {
                    ctx.outcome.error = Some(reason);
                }
            }
        }

        Request::ContinuePluginEditor => {
            match ctx
                .worker
                .events
                .sampler_edit_audio()
                .and_then(|audio| audio.take_plugin_editor_result(ctx.worker.project_generation))
            {
                Some(Ok(true)) => ctx.outcome.status = Some("vst3 editor opened".into()),
                Some(Ok(false)) => ctx.outcome.status = Some("vst3 editor closed".into()),
                Some(Err(reason)) => ctx.outcome.error = Some(reason),
                None => {}
            }
        }

        Request::QueryPluginParameters(query) => {
            let result = if query.context.project_generation != ctx.worker.project_generation
                || query.context.editor_revision != ctx.worker.revision
                || !ctx
                    .worker
                    .session
                    .project()
                    .device(query.device)
                    .is_some_and(|device| device.state().type_id().as_str() == Ok("hex.vst3"))
            {
                Err("vst3 parameter target changed".into())
            } else {
                ctx.worker
                    .events
                    .sampler_edit_audio()
                    .ok_or_else(|| "vst3 parameters unavailable for this output".to_owned())
                    .and_then(|audio| audio.query_plugin_parameters(query))
            };
            if let Err(reason) = result {
                ctx.outcome.plugin_parameters = Some(PluginParameterResult {
                    query,
                    source: None,
                    total: 0,
                    descriptor_signature: None,
                    rows: Err(reason),
                });
            }
        }

        Request::ContinueRetiringEditorReplay => {
            if ctx.worker.session.audio_mode() == DocumentAudioMode::Resident
                && ctx.worker.resident_preview.is_none()
                && ctx.worker.device_gesture.snapshot().is_none()
                && !ctx.worker.plugin_recording.editor_active()
                && ctx.worker.pending_stopped_restore.is_none()
                && let Some(sink) = ctx.worker.events.sampler_edit_audio()
            {
                let context = hex_engine::AudioSourceContext {
                    project_generation: ctx.worker.project_generation,
                    editor_revision: ctx.worker.revision,
                };
                let replay = match sink.prepare_retiring_editor_replay_restore(context) {
                    Ok(true) => {
                        ctx.worker.pending_stopped_restore = Some(context);
                        Ok(None)
                    }
                    Ok(false) => retiring_plugin_editor::apply(
                        &mut ctx.worker.session,
                        sink,
                        &mut ctx.worker.resident_snapshot,
                        context,
                        ctx.worker.export_lane.admission_with_resident_owners(
                            ctx.worker.compiled.as_ref().map(|(_, prepared)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: ctx.worker.current_sources.as_ref(),
                                    prepared: Some(prepared),
                                }
                            }),
                            ctx.worker.resident_seek.as_ref(),
                            None,
                        ),
                    ),
                    Err(reason) => Err(reason),
                };
                match replay {
                    Ok(Some(outcome)) => {
                        if let Some(prepared) = outcome.prepared {
                            ctx.worker.unsaved = true;
                            ctx.outcome.replacement_revision = Some(prepared);
                        }
                        ctx.outcome.error = outcome.receipt_failure;
                    }
                    Ok(None) => {}
                    Err(reason) => ctx.outcome.error = Some(reason),
                }
            }
        }

        Request::ContinuePluginParameters => {
            ctx.outcome.plugin_parameters = ctx
                .worker
                .events
                .sampler_edit_audio()
                .and_then(|audio| audio.take_plugin_parameters());
        }

        Request::QueryPluginParameterConversion(query) => {
            let result = if query.context.project_generation != ctx.worker.project_generation
                || query.context.editor_revision != ctx.worker.revision
                || !ctx
                    .worker
                    .session
                    .project()
                    .device(query.device)
                    .is_some_and(|device| device.state().type_id().as_str() == Ok("hex.vst3"))
            {
                Err("vst3 parameter target changed".into())
            } else {
                ctx.worker
                    .events
                    .sampler_edit_audio()
                    .ok_or_else(|| "vst3 conversion unavailable for this output".to_owned())
                    .and_then(|audio| audio.query_plugin_parameter_conversion(query.clone()))
            };
            if let Err(reason) = result {
                ctx.outcome.plugin_parameter_conversion = Some(PluginParameterConversionResult {
                    query,
                    value: Err(reason),
                });
            }
        }

        Request::ContinuePluginParameterConversion => {
            ctx.outcome.plugin_parameter_conversion = ctx
                .worker
                .events
                .sampler_edit_audio()
                .and_then(|audio| audio.take_plugin_parameter_conversion());
            if let Some(result) = &mut ctx.outcome.plugin_parameter_conversion
                && (result.query.context.project_generation != ctx.worker.project_generation
                    || result.query.context.editor_revision != ctx.worker.revision
                    || !ctx
                        .worker
                        .session
                        .project()
                        .device(result.query.device)
                        .is_some_and(|device| device.state().type_id().as_str() == Ok("hex.vst3")))
            {
                result.value = Err("vst3 parameter target changed".into());
            }
        }

        Request::ContinueDeviceInsert => 'device_insert: {
            if let Some(mut pending) = ctx.worker.pending_device_insert.take() {
                use resident_audio::device_insert::Progress;
                let cancel = ctx.worker.session.shutdown.load(Ordering::Acquire)
                    || pending.document_intent
                        != ctx.worker.document_intent.load(Ordering::Acquire);
                let Some(progress) = pending.poll(cancel) else {
                    // An ordinary request cannot force a pending transaction.
                    ctx.worker.pending_device_insert = Some(pending);
                    {
                        flow = ControlFlow::Break(());
                        break 'device_insert;
                    }
                };
                let terminal = match progress {
                    Progress::PreparationFailed => {
                        ctx.outcome.device_insert_proposal = pending.proposal_control.take();
                        ctx.outcome.error = pending.preparation_error();
                        if let Some(attempt) = pending.verified_attempt()
                            && let Some(sink) = ctx.worker.events.resident_audio()
                        {
                            sink.finish_verified_graph_edit(attempt, false);
                        }
                        ctx.outcome.request_complete = pending.plugin.is_none();
                        if let Some(plugin) = pending.plugin.take() {
                            ctx.outcome
                                .plugin_effect_complete
                                .push(plugin.finish(ctx.worker.events.sampler_edit_audio(), false));
                        }
                        break 'device_insert;
                    }
                    Progress::Proceed => {
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
                        match ctx
                            .worker
                            .events
                            .resident_audio()
                            .ok_or(SessionError::Invalid("native worker unavailable"))
                            .and_then(|sink| {
                                pending.proceed(
                                    &mut ctx.worker.session,
                                    sink,
                                    admission,
                                    ctx.worker.resident_snapshot.as_ref(),
                                )
                            }) {
                            Ok(()) => {
                                ctx.worker.pending_device_insert = Some(pending);
                                {
                                    flow = ControlFlow::Break(());
                                    break 'device_insert;
                                }
                            }
                            Err(reason) => {
                                ctx.outcome.error = Some(reason.to_string());
                                DeviceInsertAudioState::Refused(
                                    hex_engine::PlaybackHandoffRejection::RuntimeFenceChanged,
                                )
                            }
                        }
                    }
                    Progress::Terminal(DeviceInsertAudioState::Refused(
                        hex_engine::PlaybackHandoffRejection::DeviceInsertNeedsConfirmation,
                    )) if cancel => DeviceInsertAudioState::Cancelled,
                    Progress::Terminal(terminal) => terminal,
                };
                if !cancel
                    && terminal
                        == DeviceInsertAudioState::Refused(
                            hex_engine::PlaybackHandoffRejection::DeviceInsertNeedsConfirmation,
                        )
                    && let Some(offer) = ctx
                        .worker
                        .events
                        .resident_audio()
                        .and_then(|sink| pending.offer(sink))
                {
                    ctx.outcome.device_insert_confirmation = Some(offer);
                    ctx.worker.pending_device_insert = Some(pending);
                    break 'device_insert;
                }
                let operation = pending.operation;
                let edit_name = operation.edit_name();
                let Some(submission) = pending.submission() else {
                    ctx.outcome.device_insert_proposal = pending.proposal_control.take();
                    if let Some(attempt) = pending.verified_attempt()
                        && let Some(sink) = ctx.worker.events.resident_audio()
                    {
                        sink.finish_verified_graph_edit(attempt, false);
                    }
                    ctx.outcome.status = Some(format!("{edit_name} cancelled"));
                    ctx.outcome.request_complete = pending.plugin.is_none();
                    if let Some(plugin) = pending.plugin.take() {
                        ctx.outcome
                            .plugin_effect_complete
                            .push(plugin.finish(ctx.worker.events.sampler_edit_audio(), false));
                    }
                    break 'device_insert;
                };
                let verified = pending.verified_attempt().cloned();
                let was_offered = pending.awaiting_confirmation();
                let plugin = pending.plugin.take();
                let publish_created = pending.publish_created
                    && plugin.as_ref().is_none_or(|plugin| {
                        !matches!(
                            plugin.insertion.destination,
                            PluginEffectDestination::NewTrack { .. }
                        )
                    });
                ctx.outcome.request_complete = plugin.is_none();
                ctx.outcome.device_insert_proposal = pending.proposal_control.take();
                let outcome = match terminal {
                    DeviceInsertAudioState::Installed => {
                        let cut_devices = pending.cut_devices.take();
                        let cut_tracks = pending.cut_tracks.take();
                        match pending.commit(&mut ctx.worker.session, ctx.worker.resident_snapshot.as_ref()) {
                            Ok((new, prepared, snapshot)) => {
                                if publish_created {
                                    ctx.outcome.created = Some(new);
                                }
                                ctx.worker.unsaved = true;
                                ctx.outcome.replacement_revision = Some(prepared);
                                ctx.outcome.installed_verified_source = verified
                                    .as_ref()
                                    .map(|attempt| Arc::clone(attempt.source()));
                                if let Some(snapshot) = snapshot {
                                    ctx.worker.resident_snapshot = Some(snapshot);
                                }
                                ctx.outcome.cut_device_clipboard = cut_devices;
                                ctx.outcome.cut_clipboard = cut_tracks;
                                ctx.outcome.status = Some(operation.installed_status().into());
                                DeviceInsertOutcome::Installed
                            }
                            Err(reason) => {
                                ctx.outcome.error = Some(reason.to_string());
                                DeviceInsertOutcome::ProjectCommitFailed
                            }
                        }
                    }
                    DeviceInsertAudioState::Refused(reason)
                        if pending.is_live_return_power_off()
                            && reason == hex_engine::PlaybackHandoffRejection::DeviceInsertNeedsConfirmation =>
                    {
                        ctx.outcome.error = Some("running return power off cannot keep this route aligned".into());
                        DeviceInsertOutcome::Refused(reason)
                    }
                    DeviceInsertAudioState::Refused(reason) => {
                        if reason == hex_engine::PlaybackHandoffRejection::DeviceInsertNeedsConfirmation {
                            ctx.outcome.error = Some(match operation.device() {
                                Some(GraphEditOperation::InsertDevice) => "stop playback to insert this device",
                                Some(GraphEditOperation::ReplaceDevice) => "stop playback to replace this device",
                                Some(GraphEditOperation::RemoveDevice) => "stop playback to remove this device",
                                Some(GraphEditOperation::MoveDevice) => "stop playback to move devices",
                                Some(GraphEditOperation::MoveTrack) => "stop playback to move this track",
                                Some(GraphEditOperation::PowerDevice) => "stop playback to change device power",
                                Some(GraphEditOperation::RemoveTrack) => "stop playback to remove this track",
                                None => reason.message(),
                            }.into());
                            DeviceInsertOutcome::ConfirmationRequired
                        } else {
                            if ctx.outcome.error.is_none() {
                                ctx.outcome.error = Some(reason.message().into());
                            }
                            DeviceInsertOutcome::Refused(reason)
                        }
                    }
                    DeviceInsertAudioState::OutputClosed => {
                        ctx.outcome.error =
                            Some("audio output closed · restart audio to recover".into());
                        DeviceInsertOutcome::OutputClosed
                    }
                    DeviceInsertAudioState::Cancelled => {
                        if was_offered
                            || ctx.outcome.device_insert_proposal.as_ref().is_some_and(|control| control.is_cancelled())
                        {
                            ctx.outcome.status = Some(format!("{edit_name} cancelled"));
                        } else {
                            ctx.outcome.error = Some(
                                format!("{edit_name} cancelled before installation"),
                            );
                        }
                        DeviceInsertOutcome::Cancelled
                    }
                    DeviceInsertAudioState::Expired => {
                        ctx.outcome.error =
                            Some(format!("{edit_name} timed out before installation"));
                        DeviceInsertOutcome::Expired
                    }
                    DeviceInsertAudioState::Pending | DeviceInsertAudioState::Applying => {
                        pending.plugin = plugin;
                        pending.proposal_control = ctx.outcome.device_insert_proposal.take();
                        ctx.worker.pending_device_insert = Some(pending);
                        { flow = ControlFlow::Break(()); break 'device_insert; }
                    }
                };
                if operation.device().is_some() {
                    ctx.outcome.device_insert_complete = Some(DeviceInsertCompletion {
                        submission,
                        outcome,
                    });
                }
                let accepted = outcome == DeviceInsertOutcome::Installed;
                ctx.outcome.device_insert_proposal_accepted = accepted;
                if let Some(sink) = ctx.worker.events.resident_audio() {
                    if let Some(attempt) = verified.as_ref() {
                        sink.finish_verified_graph_edit(attempt, accepted);
                    } else {
                        sink.finish_resident_effect_edit(submission.context, accepted);
                    }
                }
                if let Some(plugin) = plugin {
                    ctx.outcome
                        .plugin_effect_complete
                        .push(plugin.finish(ctx.worker.events.sampler_edit_audio(), accepted));
                }
            }
        }

        request @ (Request::InsertPluginEffect(_) | Request::ContinuePluginEffect) => {
            if let Request::InsertPluginEffect(insertion) = request {
                match plugin_effect::Pending::new(insertion, ctx.worker.session.project()) {
                    Err(reason) => ctx.outcome.error = Some(reason),
                    Ok(pending) if ctx.worker.pending_plugin_effect.is_some() => {
                        ctx.outcome
                            .plugin_effect_complete
                            .push(pending.finish(ctx.worker.events.sampler_edit_audio(), false));
                        ctx.outcome.error = Some("another vst3 effect is still preparing".into());
                    }
                    Ok(pending)
                        if ctx.worker.pending_sampler_parameter.is_some()
                            || ctx.worker.device_gesture.snapshot().is_some()
                            || ctx.worker.tuning_gesture.snapshot().is_some() =>
                    {
                        ctx.outcome
                            .plugin_effect_complete
                            .push(pending.finish(ctx.worker.events.sampler_edit_audio(), false));
                        ctx.outcome.error = Some(
                            "finish or cancel the active gesture before inserting a vst3 effect"
                                .into(),
                        );
                    }
                    Ok(pending) => ctx.worker.pending_plugin_effect = Some(pending),
                }
            }
            // A refused second request must not drive or retire the first one.
            if ctx.outcome.error.is_none()
                && let Some(pending) = ctx.worker.pending_plugin_effect.as_mut()
            {
                let context = hex_engine::AudioSourceContext {
                    project_generation: ctx.worker.project_generation,
                    editor_revision: ctx.worker.revision,
                };
                let result = ctx
                    .worker
                    .events
                    .sampler_edit_audio()
                    .ok_or_else(|| "vst3 hosting unavailable for this output".to_string())
                    .and_then(|sink| {
                        pending.prepare(
                            &ctx.worker.session,
                            context,
                            ctx.worker.compiled.as_ref().map(|(_, p)| p.as_ref()),
                            sink,
                        )
                    });
                match result {
                    Ok(PluginEffectPreparationState::Preparing) => {
                        ctx.outcome.status = Some("preparing vst3".into())
                    }
                    result => {
                        if let Some(pending) = ctx.worker.pending_plugin_effect.take() {
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
                            let mut staged_insert = None;
                            let committed = result.and_then(|preparation| {
                                let PluginEffectPreparationState::Ready(hosted_state) = preparation
                                else {
                                    return Err("vst3 preparation did not finish".into());
                                };
                                // Recheck the captured target before preparing the canonical transaction.
                                pending.validate(context, ctx.worker.session.project())?;
                                let command = pending.insertion.command(hosted_state)?;
                                let sink = ctx
                                    .worker
                                    .events
                                    .resident_audio()
                                    .ok_or("native worker unavailable")?;
                                if ctx.worker.session.audio_mode()
                                    == DocumentAudioMode::VerifiedStreaming
                                {
                                    staged_insert = ctx
                                        .worker
                                        .session
                                        .stage_verified_graph_edit(
                                            command,
                                            &ctx.worker.state,
                                            ctx.worker.compiled.as_ref().map(|(_, p)| p.as_ref()),
                                            sink,
                                            ctx.outcome.request_document_intent,
                                            None,
                                            Some(Arc::clone(&pending.insertion.control)),
                                        )
                                        .map_err(|error| error.to_string())?;
                                    return Ok(None);
                                }
                                if sink.confirms_device_inserts() {
                                    staged_insert = ctx
                                        .worker
                                        .session
                                        .stage_resident_device_insert(
                                            command,
                                            context,
                                            sink,
                                            admission,
                                            ctx.worker.resident_snapshot.as_ref(),
                                            Some(&pending.insertion.control),
                                            None,
                                            ctx.outcome.request_document_intent,
                                        )
                                        .map_err(|e| e.to_string())?;
                                    return Ok(None);
                                }
                                ctx.worker
                                    .session
                                    .plugin_effect_resident_snapshot(
                                        command,
                                        Arc::clone(&pending.insertion.control),
                                        hex_engine::AudioSourceContext {
                                            editor_revision: ctx.worker.revision.saturating_add(1),
                                            ..context
                                        },
                                        sink,
                                        admission,
                                        ctx.worker.resident_snapshot.as_ref(),
                                    )
                                    .map(Some)
                                    .map_err(|e| e.to_string())
                            });
                            if let Some(mut staged) = staged_insert {
                                staged.plugin = Some(pending);
                                ctx.worker.pending_device_insert = Some(staged);
                                ctx.outcome.status =
                                    Some("vst3 pending · waiting for audio".into());
                            } else {
                                let accepted = matches!(committed, Ok(Some(_)));
                                match committed {
                                    Ok(Some((new, snapshot))) => {
                                        if !matches!(
                                            pending.insertion.destination,
                                            PluginEffectDestination::NewTrack { .. }
                                        ) {
                                            ctx.outcome.created = Some(new);
                                        }
                                        ctx.worker.unsaved = true;
                                        ctx.outcome.replacement_revision =
                                            Some(Arc::clone(&snapshot.prepared));
                                        ctx.worker.resident_snapshot = Some(snapshot);
                                        ctx.outcome.status =
                                            Some("vst3 inserted · waiting for audio".into());
                                    }
                                    Ok(None) => {}
                                    Err(reason) => ctx.outcome.error = Some(reason),
                                }
                                ctx.outcome.plugin_effect_complete.push(
                                    pending
                                        .finish(ctx.worker.events.sampler_edit_audio(), accepted),
                                );
                            }
                        }
                    }
                }
            }
        }

        Request::InsertEffectBusDevice { .. } => {
            ctx.outcome.error =
                Some("effect insertion request was not prepared; select it again".into());
        }

        Request::LoadTrackCatalogDevice { .. } => {
            ctx.outcome.error =
                Some("device picker request was not prepared; select it again".into());
        }

        Request::ReplaceDeviceModulation { .. } => {
            ctx.outcome.error = Some("modulation target was not resolved".into());
        }

        Request::DeviceAutomation { .. } | Request::DeviceAutomationControl { .. } => {
            ctx.outcome.error = Some("automation target was not resolved".into());
        }

        Request::DropBrowserDeviceOnArrangement {
            project_generation: expected_generation,
            revision: expected_revision,
            class,
            type_id,
            destination,
            new_track_gain,
            ..
        } => {
            let command = if ctx.worker.device_gesture.snapshot().is_some() {
                Err("device edit is still applying; drag again".to_owned())
            } else if expected_generation != ctx.worker.project_generation
                || expected_revision != ctx.worker.revision
            {
                Err("device drag canceled · project changed".to_owned())
            } else {
                arrangement_device_command(
                    ctx.worker.session.project(),
                    class,
                    type_id,
                    destination,
                    new_track_gain,
                )
                .and_then(|command| {
                    device_defaults::initialize(command, class, ctx.worker.defaults_root)
                })
            };
            match command.and_then(|command| {
                resident_audio::device_insert::submit_browser_edit(
                    &mut ctx.worker.session,
                    command,
                    &ctx.worker.state,
                    ctx.worker.project_generation,
                    ctx.worker
                        .compiled
                        .as_ref()
                        .map(|(_, artifact)| artifact.as_ref()),
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
                )
                .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))
            }) {
                Ok(BrowserEdit::Pending(mut staged)) => {
                    staged.publish_created = false;
                    staged.proposal_control = ctx.outcome.device_insert_proposal.take();
                    ctx.worker.pending_device_insert = Some(*staged);
                    ctx.outcome.request_complete = false;
                    ctx.outcome.status = Some("device edit pending · waiting for audio".into());
                }
                Ok(BrowserEdit::Applied(_, prepared)) => {
                    // No Created::Track focus response: this drop preserves prior selection.
                    ctx.worker.unsaved = true;
                    ctx.outcome.replacement_revision = Some(prepared);
                    ctx.outcome.device_insert_proposal_accepted = true;
                    ctx.outcome.status = Some("device added to arrangement".into());
                }
                Ok(BrowserEdit::Unchanged) => ctx.outcome.status = Some("no change".into()),
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        Request::DropNativeDevice {
            project_generation: expected_generation,
            revision: expected_revision,
            source,
            track,
            before,
            replace,
            ..
        } => {
            let command = if ctx.worker.device_gesture.snapshot().is_some() {
                Err("device edit is still applying; drag again".to_owned())
            } else if expected_generation != ctx.worker.project_generation
                || expected_revision != ctx.worker.revision
            {
                Err("device drag canceled · project changed".to_owned())
            } else if before.is_some_and(|anchor| {
                !ctx.worker
                    .session
                    .project()
                    .track(track)
                    .is_some_and(|target| {
                        target.ordered_device_chain().any(|node| match node {
                            hex_project::TrackDeviceRef::Instrument(device)
                            | hex_project::TrackDeviceRef::Effect(device) => device.id() == anchor,
                        })
                    })
            }) {
                Err("device insertion anchor no longer exists".to_owned())
            } else if replace.is_some() && before.is_some() {
                Err("device replacement cannot include an insertion anchor".to_owned())
            } else if replace.is_some()
                && !matches!(
                    source,
                    NativeDeviceDropSource::Browser {
                        class: hex_engine::NativeDeviceClass::Instrument,
                        ..
                    }
                )
            {
                Err("only a browser instrument drop can replace an instrument".to_owned())
            } else {
                match source {
                    NativeDeviceDropSource::Browser { class, type_id } => match class {
                        hex_engine::NativeDeviceClass::Effect => {
                            hex_engine::default_native_device_state_for_class(class, type_id)
                                .map(|state| Command::InsertDevice {
                                    track,
                                    before,
                                    state,
                                })
                                .map_err(|reason| {
                                    hex_storage::text::lowercase_outside_verbatim(
                                        &reason.to_string(),
                                    )
                                })
                        }
                        hex_engine::NativeDeviceClass::Instrument => ctx
                            .worker
                            .session
                            .project()
                            .track(track)
                            .ok_or_else(|| "track no longer exists".to_owned())
                            .and_then(|target| {
                                let state = hex_engine::default_native_device_state_for_class(
                                    class, type_id,
                                )
                                .map_err(|reason| {
                                    hex_storage::text::lowercase_outside_verbatim(
                                        &reason.to_string(),
                                    )
                                })?;
                                if let Some(replacement) = replace {
                                    let Some(instrument) = target.instrument() else {
                                        return Err(
                                            "instrument replacement target no longer exists".into(),
                                        );
                                    };
                                    if instrument.id() != replacement {
                                        return Err("instrument replacement target changed".into());
                                    }
                                } else if target.instrument().is_some() {
                                    return Err("instrument slot is already occupied".into());
                                }
                                Ok(Command::LoadInstrument {
                                    track,
                                    replacing: replace,
                                    state,
                                    device_name: native_device_name(class, type_id)?,
                                })
                            }),
                    }
                    .and_then(|command| {
                        device_defaults::initialize(command, class, ctx.worker.defaults_root)
                    }),
                    NativeDeviceDropSource::Existing {
                        device,
                        class,
                        type_id,
                    } => ctx
                        .worker
                        .session
                        .native_device_class(device)
                        .map_err(|reason| {
                            hex_storage::text::lowercase_outside_verbatim(&reason.to_string())
                        })
                        .and_then(|actual_class| {
                            if actual_class != class {
                                return Err("device drag class changed".to_owned());
                            }
                            let actual_type = ctx
                                .worker
                                .session
                                .project()
                                .device(device)
                                .map(|device| device.state().type_id())
                                .ok_or_else(|| "device no longer exists".to_owned())?;
                            if actual_type != type_id {
                                return Err("device drag type changed".to_owned());
                            }
                            Ok(Command::MoveTrackDevice {
                                device,
                                track,
                                before,
                            })
                        }),
                }
            };
            match command {
                Ok(command) => {
                    let before_project = ctx.worker.session.project().clone();
                    match resident_audio::device_insert::submit_browser_edit(
                        &mut ctx.worker.session,
                        command,
                        &ctx.worker.state,
                        ctx.worker.project_generation,
                        ctx.worker
                            .compiled
                            .as_ref()
                            .map(|(_, artifact)| artifact.as_ref()),
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
                    ) {
                        Ok(BrowserEdit::Pending(mut staged)) => {
                            staged.publish_created = false;
                            staged.proposal_control = ctx.outcome.device_insert_proposal.take();
                            ctx.worker.pending_device_insert = Some(*staged);
                            ctx.outcome.request_complete = false;
                            ctx.outcome.status =
                                Some("device edit pending · waiting for audio".into());
                        }
                        Ok(BrowserEdit::Applied(_, prepared))
                            if ctx.worker.session.project() != &before_project =>
                        {
                            ctx.worker.unsaved = true;
                            ctx.outcome.replacement_revision = Some(prepared);
                            ctx.outcome.device_insert_proposal_accepted = true;
                            ctx.outcome.status = Some(match source {
                                NativeDeviceDropSource::Browser { .. } if replace.is_some() => {
                                    "instrument replaced".into()
                                }
                                NativeDeviceDropSource::Browser { .. } => "device added".into(),
                                NativeDeviceDropSource::Existing { .. } => "device moved".into(),
                            });
                        }
                        Ok(BrowserEdit::Applied(..) | BrowserEdit::Unchanged) => {
                            ctx.outcome.status = Some("no change".into())
                        }
                        Err(reason) => {
                            ctx.outcome.error = Some(hex_storage::text::lowercase_outside_verbatim(
                                &reason.to_string(),
                            ))
                        }
                    }
                }
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        Request::LoadFirstSampler {
            request_id: _,
            project_generation: target_generation,
            revision: target_revision,
            target,
            path,
            cancel,
        } => {
            let mut loaded_resident = None;
            let result = (|| -> Result<_, SessionError> {
                if target_generation != ctx.worker.project_generation
                    || super::reconcile_barrier_revision(
                        target_revision,
                        ctx.outcome.revision_before_request,
                        ctx.worker.revision,
                    ) != ctx.worker.revision
                {
                    return Err(SessionError::Invalid("sampler load target changed"));
                }
                validate_sampler_load_target(ctx.worker.session.project(), &target)?;
                if ctx.outcome.private_sampler && !ctx.worker.events.resident_publication_enabled()
                {
                    return Err(SessionError::Invalid(
                        "first sampler load only; active sampler replacement is unavailable",
                    ));
                }
                if ctx.worker.session.audio_mode() != DocumentAudioMode::Resident
                    || ctx.worker.session.native_reservation.is_none()
                {
                    return Err(SessionError::Invalid(
                        "sampler requires explicit resident document admission",
                    ));
                }
                if cancel.load(Ordering::Acquire) {
                    return Err(SessionError::Invalid("sampler load cancelled"));
                }
                let decoded =
                    decode_wav_file(path, Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT))?;
                if cancel.load(Ordering::Acquire) {
                    return Err(SessionError::Invalid("sampler load cancelled"));
                }
                let current = ctx.worker.compiled.as_ref().map(|(_, p)| {
                    hex_engine::native_device_resources::ControlSamplerSelection {
                        sources: ctx.worker.current_sources.as_ref(),
                        prepared: Some(p),
                    }
                });
                let frames = decoded.audio().frames().len();
                let destination = match target {
                    SamplerLoadTarget::Track {
                        track,
                        slot: SamplerLoadSlot::New(placement),
                    } => new_sampler_load_destination(
                        frames,
                        hex_project::InstrumentLoadDestination::Track { track, placement },
                    )?,
                    SamplerLoadTarget::NewTrack { before_track } => new_sampler_load_destination(
                        frames,
                        hex_project::InstrumentLoadDestination::NewTrack { before_track },
                    )?,
                    SamplerLoadTarget::Track {
                        track,
                        slot:
                            SamplerLoadSlot::Existing {
                                device,
                                expected_state,
                            },
                    } => {
                        let (state, source_slot) =
                            hex_engine::sampler_replacement_template_on_control(
                                &expected_state,
                                frames,
                            )?;
                        crate::native_assets::NativeAssetDestination::ExistingInstrument {
                            owner: hex_project::InstrumentAssetOwner::Track(track),
                            device,
                            expected_state,
                            state,
                            source_slot,
                        }
                    }
                };
                if let Some(sink) = ctx.worker.events.resident_audio() {
                    let (created, snapshot) = ctx.worker.session.import_resident_sampler(
                        decoded,
                        &cancel,
                        destination,
                        resident_audio::ResidentSinkContext {
                            context: hex_engine::AudioSourceContext {
                                project_generation: ctx.worker.project_generation,
                                editor_revision: ctx.worker.revision.saturating_add(1),
                            },
                            sink,
                            admission: ctx.worker.export_lane.admission_with_resident_owners(
                                current,
                                ctx.worker.resident_seek.as_ref(),
                                ctx.worker.resident_preview.as_ref(),
                            ),
                        },
                        ctx.worker.resident_snapshot.as_ref(),
                    )?;
                    let prepared = Arc::clone(&snapshot.prepared);
                    loaded_resident = Some(snapshot);
                    return Ok((
                        created,
                        sampler_export::ServiceExportSnapshot {
                            prepared,
                            sources: None,
                        },
                    ));
                }
                ctx.worker.session.import_service_snapshot_destination(
                    decoded,
                    &cancel,
                    ctx.worker.export_lane.admission_with_resident_owners(
                        current,
                        ctx.worker.resident_seek.as_ref(),
                        ctx.worker.resident_preview.as_ref(),
                    ),
                    destination,
                    None,
                )
            })();
            match result {
                Ok((asset, snapshot)) => {
                    if let Some(snapshot) = loaded_resident {
                        ctx.worker.resident_snapshot = Some(snapshot);
                    }
                    ctx.outcome.created = Some(asset);
                    ctx.worker.unsaved = true;
                    ctx.outcome.replacement_revision = Some(snapshot.prepared);
                    ctx.outcome.replacement_sources = Some(snapshot.sources);
                    ctx.outcome.status = Some(if ctx.worker.events.native_audio_requested() {
                        "sample loaded into sampler · preparing audio"
                    } else { "sample loaded into sampler · offline export supported · native playback unavailable" }.into());
                }
                Err(reason) => {
                    ctx.outcome.error = Some(hex_storage::text::lowercase_outside_verbatim(
                        &reason.to_string(),
                    ))
                }
            }
        }

        Request::AssignSampler { track, asset } => {
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
            let assignment = if ctx.worker.session.native_reservation.is_none() {
                Err(SessionError::Invalid(
                    "sampler requires an explicit resident reservation",
                ))
            } else {
                ctx.worker
                    .session
                    .native_assets
                    .sampler_state_on_control(asset)
                    .map_err(SessionError::from)
            };
            let result = assignment.and_then(|state| {
                let target = ctx
                    .worker
                    .session
                    .project()
                    .track(track)
                    .ok_or(SessionError::Invalid("sampler track no longer exists"))?;
                let command = target.instrument().map_or(
                    Command::InsertInstrument {
                        track,
                        state: state.clone(),
                    },
                    |device| Command::ReplaceDeviceState {
                        device: device.id(),
                        state,
                    },
                );
                if ctx.worker.events.resident_publication_enabled()
                    && ctx.worker.session.audio_mode() == DocumentAudioMode::Resident
                {
                    resident_audio::execute_command(
                        &mut ctx.worker.session,
                        command,
                        ctx.worker
                            .compiled
                            .as_ref()
                            .map(|(_, prepared)| prepared.as_ref()),
                        hex_engine::AudioSourceContext {
                            project_generation: ctx.worker.project_generation,
                            editor_revision: ctx.worker.revision.saturating_add(1),
                        },
                        &mut ctx.worker.events,
                        &mut ctx.worker.resident_snapshot,
                        admission,
                    )
                    .map(|(new, prepared)| {
                        (
                            new,
                            sampler_export::ServiceExportSnapshot {
                                prepared,
                                sources: None,
                            },
                        )
                    })
                } else {
                    ctx.worker
                        .session
                        .command_service_snapshot(command, admission)
                }
            });
            match result {
                Ok((new, snapshot)) => {
                    ctx.outcome.created = Some(new);
                    if ctx.worker.session.project() != ctx.worker.state.project.as_ref() {
                        ctx.worker.unsaved = true;
                        ctx.outcome.replacement_revision = Some(snapshot.prepared);
                        ctx.outcome.replacement_sources = Some(snapshot.sources);
                    }
                    ctx.outcome.status = Some(
                        if ctx.worker.events.resident_publication_enabled() {
                            "sampler assigned · waiting for audio"
                        } else if ctx.worker.events.native_audio_requested() {
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

        Request::ImportDeviceAsset { path, cancel }
            if ctx.worker.events.resident_publication_enabled()
                && ctx.worker.session.audio_mode() == DocumentAudioMode::Resident =>
        {
            let result = (|| {
                if cancel.load(Ordering::Acquire) {
                    return Err(SessionError::Invalid("native source import canceled"));
                }
                let decoded =
                    decode_wav_file(path, Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT))?;
                let sink = ctx
                    .worker
                    .events
                    .resident_audio()
                    .ok_or(SessionError::Invalid("native worker unavailable"))?;
                ctx.worker.session.import_resident_sampler(
                    decoded,
                    &cancel,
                    crate::native_assets::NativeAssetDestination::Unassigned,
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
                    ctx.worker.resident_snapshot.as_ref(),
                )
            })();
            match result {
                Ok((asset, snapshot)) => {
                    ctx.outcome.created = Some(asset);
                    ctx.worker.unsaved = true;
                    ctx.outcome.replacement_revision = Some(Arc::clone(&snapshot.prepared));
                    ctx.outcome.replacement_sources = Some(None);
                    ctx.worker.resident_snapshot = Some(snapshot);
                    ctx.outcome.status = Some("native device source imported".into());
                }
                Err(reason) => ctx.outcome.error = Some(reason.to_string()),
            }
        }

        Request::ImportDeviceAsset { path, cancel } => {
            let result = if cancel.load(Ordering::Relaxed) {
                Err("native source import canceled".to_owned())
            } else if ctx.worker.session.native_reservation.is_none() {
                Err("native source import requires an explicit resident reservation".to_owned())
            } else {
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
                decode_resident_import(path)
                    .map_err(|reason| reason.to_string())
                    .and_then(|decoded| {
                        ctx.worker
                            .session
                            .import_service_snapshot(decoded, &cancel, admission)
                            .map_err(|reason| reason.to_string())
                    })
            };
            match result {
                Ok((asset, snapshot)) => {
                    ctx.outcome.created = Some(asset);
                    ctx.worker.unsaved = true;
                    ctx.outcome.replacement_revision = Some(snapshot.prepared);
                    ctx.outcome.replacement_sources = Some(snapshot.sources);
                    ctx.outcome.status = Some("native device source imported".into());
                }
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        Request::DeviceClipboard {
            action: DeviceClipboardAction::Copy { owner, selected },
            ..
        } => {
            let cancellation = RequestCancellation::new(
                Arc::clone(&ctx.worker.session.shutdown),
                Arc::clone(&ctx.worker.document_intent),
                ctx.outcome.request_document_intent,
            );
            match plugin_state_save::capture_devices_for_clipboard(
                ctx.worker.session.project(),
                owner,
                &selected,
                hex_engine::AudioSourceContext {
                    project_generation: ctx.worker.project_generation,
                    editor_revision: ctx.worker.revision,
                },
                &cancellation,
                ctx.worker.events.sampler_edit_audio(),
                "device copy cancelled",
            ) {
                Ok(captured) => {
                    ctx.worker.device_clipboard = Some(Arc::new(captured));
                    ctx.outcome.status = Some("devices copied".into());
                }
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        Request::DeviceClipboard { .. } => {
            ctx.outcome.error = Some("device clipboard request was not resolved".into())
        }

        Request::AddNativeDevice {
            track: track_id,
            class,
            type_id,
            before,
        } => {
            let command = match class {
                hex_engine::NativeDeviceClass::Effect => {
                    hex_engine::default_native_device_state_for_class(class, *type_id).map(
                        |state| Command::InsertDevice {
                            track: track_id,
                            before,
                            state,
                        },
                    )
                }
                hex_engine::NativeDeviceClass::Instrument if before.is_some() => Err(
                    hex_engine::Invalid("instruments cannot be inserted at an effect anchor"),
                ),
                hex_engine::NativeDeviceClass::Instrument => ctx
                    .worker
                    .session
                    .project()
                    .track(track_id)
                    .ok_or(hex_engine::Invalid("track no longer exists"))
                    .and_then(|track| {
                        if track.instrument().is_some() {
                            return Err(hex_engine::Invalid("instrument slot is already occupied"));
                        }
                        let device_name = native_device_name(class, *type_id)
                            .map_err(|_| hex_engine::Invalid("native device is not registered"))?;
                        hex_engine::default_native_device_state_for_class(class, *type_id).map(
                            |state| Command::LoadInstrument {
                                track: track_id,
                                replacing: None,
                                state,
                                device_name,
                            },
                        )
                    }),
            };
            let command = command
                .map_err(|error| error.to_string())
                .and_then(|command| {
                    device_defaults::initialize(command, class, ctx.worker.defaults_root)
                });
            match command {
                Ok(command) => {
                    let before_project = ctx.worker.session.project().clone();
                    match resident_audio::device_insert::submit_browser_edit(
                        &mut ctx.worker.session,
                        command,
                        &ctx.worker.state,
                        ctx.worker.project_generation,
                        ctx.worker
                            .compiled
                            .as_ref()
                            .map(|(_, artifact)| artifact.as_ref()),
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
                        None,
                    ) {
                        Ok(BrowserEdit::Pending(staged)) => {
                            ctx.worker.pending_device_insert = Some(*staged);
                            ctx.outcome.request_complete = false;
                            ctx.outcome.status =
                                Some("device edit pending · waiting for audio".into());
                        }
                        Ok(BrowserEdit::Applied(result, prepared))
                            if ctx.worker.session.project() != &before_project =>
                        {
                            ctx.worker.unsaved = true;
                            ctx.outcome.replacement_revision = Some(prepared);
                            ctx.outcome.created = Some(result);
                            ctx.outcome.status = Some("device added".into());
                        }
                        Ok(BrowserEdit::Applied(..) | BrowserEdit::Unchanged) => {
                            ctx.outcome.status = Some("no change".into())
                        }
                        Err(reason) => {
                            ctx.outcome.error = Some(hex_storage::text::lowercase_outside_verbatim(
                                &reason.to_string(),
                            ))
                        }
                    }
                }
                Err(reason) => {
                    ctx.outcome.error = Some(hex_storage::text::lowercase_outside_verbatim(
                        &reason.to_string(),
                    ))
                }
            }
        }

        Request::ReplaceNativeDevice {
            device,
            class,
            type_id,
        } => {
            let replacement = ctx
                .worker
                .session
                .native_device_class(device)
                .map_err(|reason| {
                    hex_storage::text::lowercase_outside_verbatim(&reason.to_string())
                })
                .and_then(|actual_class| {
                    if actual_class != class {
                        return Err("replacement class does not match device".to_owned());
                    }
                    hex_engine::default_native_device_state_for_class(class, *type_id).map_err(
                        |reason| hex_storage::text::lowercase_outside_verbatim(&reason.to_string()),
                    )
                })
                .and_then(|state| {
                    device_defaults::initialize(
                        Command::ReplaceDeviceState { device, state },
                        class,
                        ctx.worker.defaults_root,
                    )
                });
            match replacement {
                Ok(command) => {
                    let before_project = ctx.worker.session.project().clone();
                    let confirmed_output = ctx.worker.events.resident_audio().is_some_and(|sink| {
                        match ctx.worker.session.audio_mode() {
                            DocumentAudioMode::Resident => {
                                sink.confirms_device_inserts()
                                    && sink.resident_publication_readiness()
                                        != ResidentPublicationReadiness::NoOutput
                            }
                            DocumentAudioMode::VerifiedStreaming => {
                                sink.confirms_verified_device_inserts()
                            }
                        }
                    });
                    let proposal =
                        confirmed_output.then(|| Arc::new(PluginEffectControl::default()));
                    match resident_audio::device_insert::submit_browser_edit(
                        &mut ctx.worker.session,
                        command,
                        &ctx.worker.state,
                        ctx.worker.project_generation,
                        ctx.worker
                            .compiled
                            .as_ref()
                            .map(|(_, artifact)| artifact.as_ref()),
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
                        proposal.as_ref(),
                    ) {
                        Ok(BrowserEdit::Pending(mut staged)) => {
                            staged.operation = resident_audio::device_insert::Operation::Device(
                                GraphEditOperation::ReplaceDevice,
                            );
                            staged.publish_created = false;
                            staged.proposal_control = proposal;
                            ctx.worker.pending_device_insert = Some(*staged);
                            ctx.outcome.request_complete = false;
                            ctx.outcome.status =
                                Some("device replacement pending · waiting for audio".into());
                        }
                        Ok(BrowserEdit::Applied(_, prepared))
                            if ctx.worker.session.project() != &before_project =>
                        {
                            ctx.worker.unsaved = true;
                            ctx.outcome.replacement_revision = Some(prepared);
                            ctx.outcome.status = Some("device replaced".into());
                        }
                        Ok(BrowserEdit::Applied(..) | BrowserEdit::Unchanged) => {
                            ctx.outcome.status = Some("no change".into())
                        }
                        Err(reason) => {
                            ctx.outcome.error = Some(hex_storage::text::lowercase_outside_verbatim(
                                &reason.to_string(),
                            ))
                        }
                    }
                }
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        Request::CompareDevice {
            mut owner,
            expected,
            action,
        } => {
            owner.revision = super::reconcile_barrier_revision(
                owner.revision,
                ctx.outcome.revision_before_request,
                ctx.worker.revision,
            );
            let command = device_comparison::command(
                &ctx.worker.session,
                ctx.worker.project_generation,
                ctx.worker.revision,
                owner,
                expected,
                action,
            );
            match command {
                Ok(command) => {
                    let before = ctx.worker.session.project().clone();
                    let outcome = if ctx.worker.events.resident_publication_enabled()
                        && ctx.worker.session.audio_mode() == DocumentAudioMode::Resident
                    {
                        ctx.worker.events.resident_audio().ok_or(SessionError::Invalid("native worker unavailable"))
                            .and_then(|sink| ctx.worker.session.comparison_resident_snapshot(
                                command,
                                hex_engine::AudioSourceContext { project_generation: ctx.worker.project_generation, editor_revision: ctx.worker.revision.saturating_add(1) },
                                sink,
                                ctx.worker.export_lane.admission_with_resident_owners(
                                    ctx.worker.compiled.as_ref().map(|(_, prepared)| hex_engine::native_device_resources::ControlSamplerSelection { sources: ctx.worker.current_sources.as_ref(), prepared: Some(prepared) }),
                                    ctx.worker.resident_seek.as_ref(), ctx.worker.resident_preview.as_ref(),
                                ),
                                ctx.worker.resident_snapshot.as_ref(), ctx.worker.comparison_binding.is_some(),
                            )).map(|(created, outcome)| {
                                let completed = outcome.completed.map(|snapshot| {
                                    ctx.worker.resident_snapshot = Some(snapshot);
                                });
                                (created, crate::comparison_transition::ComparisonTransition {
                                    prepared: outcome.prepared, completed, metadata_only: outcome.metadata_only,
                                })
                            })
                    } else if ctx.worker.session.audio_mode()
                        == DocumentAudioMode::VerifiedStreaming
                        && audio_sink::requires_plugin_profile(ctx.worker.session.project())
                        && let Some(sink) = ctx.worker.events.resident_audio()
                    {
                        ctx.worker.session.comparison_command_verified(
                            command,
                            ctx.worker
                                .compiled
                                .as_ref()
                                .filter(|_| ctx.worker.comparison_binding.is_some())
                                .map(|(_, artifact)| artifact),
                            hex_engine::AudioSourceContext {
                                project_generation: ctx.worker.project_generation,
                                editor_revision: ctx.worker.revision.saturating_add(1),
                            },
                            sink,
                        )
                    } else {
                        ctx.worker.session.comparison_command(
                            command,
                            ctx.worker
                                .compiled
                                .as_ref()
                                .filter(|_| ctx.worker.comparison_binding.is_some())
                                .map(|(_, artifact)| artifact),
                        )
                    };
                    match outcome {
                        Ok((_, outcome)) if ctx.worker.session.project() != &before => {
                            ctx.worker.unsaved = true;
                            if outcome.metadata_only {
                                ctx.outcome.comparison_changed = true;
                            } else {
                                ctx.outcome.replacement_revision = Some(outcome.prepared);
                            }
                            ctx.outcome.status = Some(
                                match action {
                                    hex_project::DeviceComparisonAction::Switch => {
                                        "device comparison switched"
                                    }
                                    hex_project::DeviceComparisonAction::CopyToInactive => {
                                        "device comparison copied"
                                    }
                                }
                                .into(),
                            );
                        }
                        Ok(_) => ctx.outcome.status = Some("no change".into()),
                        Err(reason) => {
                            ctx.outcome.error = Some(hex_storage::text::lowercase_outside_verbatim(
                                &reason.to_string(),
                            ))
                        }
                    }
                }
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        Request::SamplerEdit(mut edit)
            if ctx.worker.events.resident_publication_enabled()
                && ctx.worker.session.audio_mode() == DocumentAudioMode::Resident =>
        {
            ctx.outcome.sampler_edit_complete = Some(edit.request_id());
            let result = (|| {
                if !edit
                    .validate_resident(
                        &ctx.worker.session,
                        ctx.worker.project_generation,
                        ctx.worker.revision,
                    )
                    .map_err(SessionError::NativeAudio)?
                {
                    return Ok(None);
                }
                let sink = ctx
                    .worker
                    .events
                    .resident_audio()
                    .ok_or(SessionError::Invalid("native worker unavailable"))?;
                ctx.worker.session.sampler_resident_edit(
                    edit,
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
                    ctx.worker.resident_snapshot.as_ref(),
                )
            })();
            match result {
                Ok(Some((new, snapshot))) => {
                    ctx.outcome.created = Some(new);
                    ctx.worker.unsaved = true;
                    ctx.outcome.replacement_revision = Some(Arc::clone(&snapshot.prepared));
                    ctx.worker.resident_snapshot = Some(snapshot);
                    ctx.outcome.status = Some("sampler edit committed".into());
                }
                Ok(None) => ctx.outcome.status = Some("no change".into()),
                Err(reason) => ctx.outcome.error = Some(reason.to_string()),
            }
        }

        Request::SamplerEdit(mut edit) => {
            let result = (|| -> Result<bool, String> {
                if ctx.worker.pending_sampler_parameter.is_some() {
                    return Err("sampler edit is already preparing".into());
                }
                if !edit.validate(
                    &ctx.worker.session,
                    ctx.worker.project_generation,
                    ctx.worker.revision,
                )? {
                    return Ok(false);
                }
                ctx.worker
                    .events
                    .sampler_edit_audio()
                    .ok_or("sampler edit requires native audio")?
                    .begin_sampler_edit(hex_engine::AudioSourceContext {
                        project_generation: ctx.worker.project_generation,
                        editor_revision: ctx.worker.revision,
                    })?;
                Ok(true)
            })();
            match result {
                Ok(true) => {
                    ctx.worker.pending_sampler_parameter = Some(edit);
                    ctx.outcome.status =
                        Some("preparing sampler edit · waiting for stopped audio".into());
                }
                Ok(false) => {
                    ctx.outcome.sampler_edit_complete = Some(edit.request_id());
                    ctx.outcome.status = Some("no change".into());
                }
                Err(reason) => {
                    ctx.outcome.sampler_edit_complete = Some(edit.request_id());
                    ctx.outcome.error = Some(reason);
                }
            }
        }

        Request::ContinueSamplerEdit => {
            if let Some(mut edit) = ctx.worker.pending_sampler_parameter.take() {
                ctx.outcome.sampler_edit_complete = Some(edit.request_id());
                let result =
                    (|| -> Result<(Created, sampler_export::ServiceExportSnapshot), String> {
                        if !edit.validate(
                            &ctx.worker.session,
                            ctx.worker.project_generation,
                            ctx.worker.revision,
                        )? {
                            return Err("sampler edit no longer changes this revision".into());
                        }
                        let next = ctx
                            .worker
                            .revision
                            .checked_add(1)
                            .ok_or("sampler edit revision exhausted")?;
                        let context = hex_engine::AudioSourceContext {
                            project_generation: ctx.worker.project_generation,
                            editor_revision: next,
                        };
                        let audio = ctx
                            .worker
                            .events
                            .sampler_edit_audio()
                            .ok_or("sampler edit output is unavailable")?;
                        audio
                            .sampler_edit_ready()
                            .ok_or("sampler edit fence is not ready")??;
                        let current = ctx.worker.compiled.as_ref().map(|(_, p)| {
                            hex_engine::native_device_resources::ControlSamplerSelection {
                                sources: ctx.worker.current_sources.as_ref(),
                                prepared: Some(p),
                            }
                        });
                        let prepared = edit
                            .prepare(
                                &mut ctx.worker.session,
                                ctx.worker.export_lane.admission_with_resident_owners(
                                    current,
                                    ctx.worker.resident_seek.as_ref(),
                                    ctx.worker.resident_preview.as_ref(),
                                ),
                                context,
                                audio,
                            )
                            .map_err(|e| e.to_string())?;
                        // Authored commit is complete. Audio failure from here is truthful
                        // unavailability, never an invented rollback of the exclusive output.
                        if let Err(reason) = audio.commit_sampler_edit(context) {
                            ctx.outcome.error = Some(reason);
                        }
                        Ok(prepared)
                    })();
                match result {
                    Ok((new, snapshot)) => {
                        ctx.outcome.created = Some(new);
                        ctx.worker.unsaved = true;
                        ctx.outcome.replacement_revision = Some(snapshot.prepared);
                        ctx.outcome.replacement_sources = Some(snapshot.sources);
                        ctx.outcome.status = Some("sampler edit changed · preparing audio".into());
                    }
                    Err(reason) => {
                        if let Some(audio) = ctx.worker.events.sampler_edit_audio() {
                            audio.cancel_sampler_edit();
                        }
                        ctx.outcome.error = Some(reason);
                    }
                }
            } else if let Some(audio) = ctx.worker.events.sampler_edit_audio() {
                audio.cancel_sampler_edit();
            }
        }

        Request::SetDeviceParameter {
            device,
            parameter,
            value,
        } => {
            let patched = ctx
                .worker
                .session
                .native_device_class(device)
                .map_err(|reason| {
                    hex_storage::text::lowercase_outside_verbatim(&reason.to_string())
                })
                .and_then(|class| {
                    let entry = ctx
                        .worker
                        .session
                        .project()
                        .device(device)
                        .ok_or_else(|| "device is missing".to_owned())?;
                    hex_engine::patch_native_device_parameter_for_class(
                        class,
                        entry.state(),
                        parameter,
                        value,
                    )
                    .map_err(|reason| {
                        hex_storage::text::lowercase_outside_verbatim(&reason.to_string())
                    })
                });
            match patched {
                Ok(state) => {
                    let before_project = ctx.worker.session.project().clone();
                    let command = crate::device_gesture::manual_device_edit(
                        &ctx.worker.session,
                        device,
                        state,
                        [parameter],
                        true,
                    );
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
                        Ok((_, prepared)) if ctx.worker.session.project() != &before_project => {
                            if let Some(pending) = ctx.worker.pending_plugin_effect.as_mut() {
                                pending.rebase_after_accepted_eq_write(
                                    hex_engine::AudioSourceContext {
                                        project_generation: ctx.worker.project_generation,
                                        editor_revision: ctx.worker.revision,
                                    },
                                    ctx.worker.session.project(),
                                    device,
                                );
                            }
                            ctx.worker.unsaved = true;
                            ctx.outcome.replacement_revision = Some(prepared);
                            ctx.outcome.status = Some(
                                if ctx.worker.events.resident_publication_enabled()
                                    && ctx.worker.session.audio_mode()
                                        == DocumentAudioMode::Resident
                                {
                                    "resident edit accepted · waiting for audio"
                                } else {
                                    "device parameter changed"
                                }
                                .into(),
                            );
                        }
                        Ok(_) => ctx.outcome.status = Some("no change".into()),
                        Err(reason) => {
                            ctx.outcome.error = Some(hex_storage::text::lowercase_outside_verbatim(
                                &reason.to_string(),
                            ))
                        }
                    }
                }
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        Request::DeviceAutomationTake { .. }
        | Request::StartPluginEditorRecording { .. }
        | Request::StopPluginEditorRecording { .. }
        | Request::ContinuePluginEditorRecording { .. }
        | Request::QueryFailedPluginAutomation { .. }
        | Request::InspectSealedAutomationRecovery { .. }
        | Request::QueryDeviceAutomationCloseSafety { .. }
        | Request::PreparePluginEditorClose { .. }
        | Request::RecoverDeviceAutomationTake { .. } => {}

        Request::ArmNativeParameterRecording {
            project_generation: expected_generation,
            revision: expected_revision,
            target,
        } => {
            let result = (|| {
                if expected_generation != ctx.worker.project_generation
                    || expected_revision != ctx.worker.revision
                {
                    return Err(SessionError::Invalid(
                        "automation arm refused · project changed",
                    ));
                }
                if ctx.worker.native_automation_take.busy() {
                    return Err(SessionError::Invalid(
                        "finish the current automation take before changing its arm",
                    ));
                }
                if ctx.worker.device_gesture.snapshot().is_some() {
                    return Err(SessionError::Invalid(
                        "automation arm refused · device edit is still applying",
                    ));
                }
                if !ctx.worker.events.resident_publication_enabled()
                    || ctx.worker.session.audio_mode() != DocumentAudioMode::Resident
                {
                    return Err(SessionError::Invalid(
                        "device automation recording requires ordinary resident playback",
                    ));
                }
                let current =
                    ctx.worker
                        .resident_snapshot
                        .as_ref()
                        .ok_or(SessionError::Invalid(
                            "native automation source unavailable",
                        ))?;
                let sink = ctx
                    .worker
                    .events
                    .resident_audio()
                    .ok_or(SessionError::Invalid(
                        "native automation output unavailable",
                    ))?;
                let input = target
                    .map(|(device, target)| {
                        hex_engine::native_parameter_input_target(
                            ctx.worker.session.project(),
                            device,
                            target,
                        )
                    })
                    .transpose()?;
                ctx.worker
                    .session
                    .prepare_resident_native_parameter_recording(
                        input,
                        hex_engine::AudioSourceContext {
                            project_generation: ctx.worker.project_generation,
                            editor_revision: ctx.worker.revision,
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
                        current,
                    )
            })();
            match result {
                Ok(snapshot) => {
                    ctx.worker.compiled =
                        Some((ctx.worker.revision, Arc::clone(&snapshot.prepared)));
                    ctx.worker.resident_snapshot = Some(snapshot);
                    ctx.outcome.status = Some(if target.is_some() {
                        "device automation target prepared".into()
                    } else {
                        "device automation recording disarmed".into()
                    });
                }
                Err(reason) => {
                    ctx.outcome.error = Some(hex_storage::text::lowercase_outside_verbatim(
                        &reason.to_string(),
                    ))
                }
            }
        }

        Request::SaveDevicePreset { mut owner, path } => {
            owner.revision = super::reconcile_barrier_revision(
                owner.revision,
                ctx.outcome.revision_before_request,
                ctx.worker.revision,
            );
            match preset::save(
                &ctx.worker.session,
                ctx.worker.project_generation,
                ctx.worker.revision,
                owner,
                &path,
            ) {
                Ok(()) => ctx.outcome.status = Some("device preset saved".into()),
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        Request::SaveDeviceDefault { mut owner, root } => {
            owner.revision = super::reconcile_barrier_revision(
                owner.revision,
                ctx.outcome.revision_before_request,
                ctx.worker.revision,
            );
            match preset::save_default(
                &ctx.worker.session,
                ctx.worker.project_generation,
                ctx.worker.revision,
                owner,
                &root,
            ) {
                Ok(()) => ctx.outcome.status = Some("device default saved".into()),
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        Request::SaveDeviceUnitPreset { mut owner, path } => {
            owner.revision = super::reconcile_barrier_revision(
                owner.revision,
                ctx.outcome.revision_before_request,
                ctx.worker.revision,
            );
            match preset::save_complete(
                &ctx.worker.session,
                ctx.worker.project_generation,
                ctx.worker.revision,
                owner,
                preset::CompleteDestination::Preset(&path),
            ) {
                Ok(()) => ctx.outcome.status = Some("device preset saved".into()),
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        Request::SaveDeviceUnitDefault { mut owner, root } => {
            owner.revision = super::reconcile_barrier_revision(
                owner.revision,
                ctx.outcome.revision_before_request,
                ctx.worker.revision,
            );
            match preset::save_complete(
                &ctx.worker.session,
                ctx.worker.project_generation,
                ctx.worker.revision,
                owner,
                preset::CompleteDestination::Default(&root),
            ) {
                Ok(()) => ctx.outcome.status = Some("device default saved".into()),
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        Request::LoadDevicePreset { mut owner, path } => {
            owner.revision = super::reconcile_barrier_revision(
                owner.revision,
                ctx.outcome.revision_before_request,
                ctx.worker.revision,
            );
            let candidate = preset::load_command(
                &ctx.worker.session,
                ctx.worker.project_generation,
                ctx.worker.revision,
                owner,
                &path,
            );
            match candidate {
                Ok(command) => {
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
                        Ok((_, prepared)) if ctx.worker.session.project() != &before => {
                            ctx.worker.unsaved = true;
                            ctx.outcome.replacement_revision = Some(prepared);
                            ctx.outcome.status =
                                Some("device preset loaded · undo restores previous state".into());
                        }
                        Ok(_) => ctx.outcome.status = Some("device preset already matches".into()),
                        Err(reason) => {
                            ctx.outcome.error = Some(format!("device preset refused · {reason}"))
                        }
                    }
                }
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        Request::ContinuePluginStateRestore => {
            if let Some(sink) = ctx.worker.events.resident_audio()
                && let Some(context) = sink.plugin_state_restore_ready()
            {
                // Metadata-only A/B edits retain this exact preparation source through
                // the sealed comparison binding. Keep its original native correlation;
                // a literal authored-revision match would discard a valid restore.
                let result = if context.project_generation != ctx.worker.project_generation
                    || ctx.worker.state.audio_authoring_revision(context)
                        != Some(ctx.worker.revision)
                {
                    Err(SessionError::Invalid(
                        "vst3 state restoration source changed",
                    ))
                } else if ctx.worker.device_gesture.snapshot().is_some()
                    || ctx.worker.tuning_gesture.snapshot().is_some()
                {
                    Err(SessionError::Invalid(
                        "finish the active gesture before restoring vst3 state",
                    ))
                } else {
                    ctx.worker.session.restore_plugin_state_snapshot(
                        context,
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
                        ctx.worker.resident_snapshot.as_ref(),
                    )
                };
                let result = result.map_err(|reason| reason.to_string());
                sink.finish_plugin_state_restore(
                    context,
                    result.as_ref().map(|_| ()).map_err(String::as_str),
                );
                match result {
                    Ok(snapshot) => {
                        ctx.worker.compiled =
                            Some((ctx.worker.revision, Arc::clone(&snapshot.prepared)));
                        ctx.worker.resident_snapshot = Some(snapshot);
                        ctx.outcome.status = Some("vst3 playback prepared".into());
                    }
                    Err(reason) => ctx.outcome.error = Some(reason),
                }
            }
        }

        _ => {
            ctx.outcome.error = Some("unexpected worker dispatch route".into());
        }
    }
    flow
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_device_name_falls_back_to_type_id_for_unlisted_device() {
        let unknown = hex_lattice_contract::DeviceTypeId::new("hex.test.unknown").unwrap();
        assert_eq!(
            native_device_name(hex_engine::NativeDeviceClass::Effect, unknown),
            Ok("hex.test.unknown".to_owned())
        );
    }
}
