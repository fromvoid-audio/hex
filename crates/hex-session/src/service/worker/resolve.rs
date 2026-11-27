//! Source-bound UI edit resolution prior to gesture barriers and domain dispatch.
use super::*;

impl WorkerLoop {
    pub(super) fn resolve_source_bound_edit(
        &mut self,
        request: Request,
        exec: &mut RequestExecution,
    ) -> (Request, Option<String>) {
        let Self { events, core, .. } = self;
        let WorkerLoopCore {
            session,
            document_intent,
            recovery,
            revision,
            track_clipboard,
            device_clipboard,
            media_base,
            project_generation,
            device_gesture,
            tuning_gesture,
            pending_sampler_parameter,
            native_automation_take,
            plugin_recording,
            ..
        } = core;
        let revision = *revision;
        let project_generation = *project_generation;
        let defaults_root = exec
            .meta
            .defaults_directory
            .as_deref()
            .map(PathBuf::as_path);
        let automation_source_error = match &request {
            Request::DeviceAutomation {
                project_generation: generation,
                revision: source_revision,
                ..
            }
            | Request::DeviceAutomationControl {
                project_generation: generation,
                revision: source_revision,
                ..
            } => {
                if *generation != project_generation || *source_revision != revision {
                    Some(
                        "automation edit refused · project changed · retry from the current parameter",
                    )
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Some("finish or cancel the active parameter gesture before editing automation")
                } else {
                    None
                }
            }
            _ => None,
        };
        let mut device_automation_take_complete = None;
        let mut adopted_device_draft = None;
        let mut failed_plugin_adoption = None;
        let failed_plugin_publication_confirmed = false;
        let resolved_edit = match &request {
            Request::CreateRetainedNativeSource {
                project_generation: captured_generation,
                revision: captured_revision,
                type_id,
                name,
            } => Some((|| {
                if *captured_generation != project_generation || *captured_revision != revision {
                    return Err("source creation canceled · project changed".into());
                }
                if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    return Err(
                        "finish or cancel the active gesture before creating a source".into(),
                    );
                }
                let state = hex_engine::default_native_device_state_for_class(
                    hex_engine::NativeDeviceClass::Instrument,
                    **type_id,
                )
                .map_err(|reason| format!("native instrument unavailable · {reason}"))?;
                Ok(Command::CreateNativeSource {
                    name: name.clone(),
                    devices: vec![state],
                    instrument_index: 0,
                })
            })()),
            Request::RenameRetainedSource {
                project_generation: captured_generation,
                revision: captured_revision,
                source,
                ..
            }
            | Request::DeleteRetainedSource {
                project_generation: captured_generation,
                revision: captured_revision,
                source,
            } => Some((|| {
                if *captured_generation != project_generation || *captured_revision != revision {
                    return Err("source edit canceled · project changed".into());
                }
                if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    return Err(
                        "finish or cancel the active gesture before editing a source".into(),
                    );
                }
                if session.project().native_source(*source).is_none() {
                    return Err("source edit canceled · source no longer exists".into());
                }
                match &request {
                    Request::RenameRetainedSource { name, .. } => Ok(Command::RenameNativeSource {
                        source: *source,
                        name: name.clone(),
                    }),
                    Request::DeleteRetainedSource { .. } => {
                        Ok(Command::RemoveNativeSource(*source))
                    }
                    _ => Err("source edit was not resolved".into()),
                }
            })()),
            Request::ChooseNativeSourceReceiver {
                project_generation: captured_generation,
                revision: captured_revision,
                source,
                expected_receiver,
                track,
            } => Some((|| {
                if *captured_generation != project_generation || *captured_revision != revision {
                    return Err("source receiver selection canceled · project changed".into());
                }
                if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    return Err(
                        "finish or cancel the active gesture before choosing a source receiver"
                            .into(),
                    );
                }
                let project = session.project();
                let definition = project
                    .native_source(*source)
                    .ok_or("source receiver selection canceled · source no longer exists")?;
                if definition.receiver() != *expected_receiver {
                    return Err("source receiver selection canceled · receiver changed".into());
                }
                let target = project
                    .track(*track)
                    .ok_or("source receiver selection canceled · track no longer exists")?;
                if target.source_id() != Some(*source) {
                    return Err(
                        "source receiver selection canceled · track is not linked to source".into(),
                    );
                }
                if *expected_receiver == Some(*track) {
                    return Err("source receiver is unchanged".into());
                }
                Ok(Command::SetNativeSourceReceiver {
                    source: *source,
                    track: Some(*track),
                })
            })()),
            Request::AppendSourceUtility {
                project_generation: captured_generation,
                revision: captured_revision,
                track,
                source,
                instrument,
            } => Some((|| {
                if *captured_generation != project_generation || *captured_revision != revision {
                    return Err("source effect canceled · project changed".into());
                }
                if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    return Err(
                        "finish or cancel the active gesture before editing a source".into(),
                    );
                }
                let project = session.project();
                if project
                    .track(*track)
                    .is_none_or(|track| track.source_id() != Some(*source))
                {
                    return Err("source effect canceled · track binding changed".into());
                }
                let source = project
                    .native_source(*source)
                    .ok_or("source effect canceled · source no longer exists")?;
                let default_instrument = hex_project::default_instrument_state()
                    .map_err(|_| "source instrument unavailable")?;
                if source.receiver().is_none()
                    || source.instrument().id() != *instrument
                    || source.instrument().state().type_id() != default_instrument.type_id()
                    || source.devices().count() >= hex_project::MAX_SOURCE_DEVICES
                    || source.post_instrument_effects().iter().any(|effect| {
                        effect.state().type_id()
                            != hex_lattice_contract::DeviceTypeId::from_static("hex.utility")
                    })
                {
                    return Err(
                        "source effect canceled · source chain changed or unsupported".into(),
                    );
                }
                let state = hex_engine::default_native_device_state_for_class(
                    hex_engine::NativeDeviceClass::Effect,
                    hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                )
                .map_err(|_| "mix is unavailable")?;
                Ok(Command::InsertNestedDevice {
                    parent: *instrument,
                    state,
                })
            })()),
            Request::RemoveSourceUtility {
                project_generation: captured_generation,
                revision: captured_revision,
                track,
                source,
                instrument,
                effect,
            } => Some((|| {
                if *captured_generation != project_generation || *captured_revision != revision {
                    return Err("source effect canceled · project changed".into());
                }
                if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    return Err(
                        "finish or cancel the active gesture before editing a source".into(),
                    );
                }
                let project = session.project();
                if project
                    .track(*track)
                    .is_none_or(|track| track.source_id() != Some(*source))
                {
                    return Err("source effect canceled · track binding changed".into());
                }
                let source = project
                    .native_source(*source)
                    .ok_or("source effect canceled · source no longer exists")?;
                let default_instrument = hex_project::default_instrument_state()
                    .map_err(|_| "source instrument unavailable")?;
                let effects = source.post_instrument_effects();
                if source.receiver().is_none()
                    || source.instrument().id() != *instrument
                    || source.instrument().state().type_id() != default_instrument.type_id()
                    || !effects.iter().any(|item| item.id() == *effect)
                    || effects.iter().any(|item| {
                        item.state().type_id()
                            != hex_lattice_contract::DeviceTypeId::from_static("hex.utility")
                    })
                    || source.devices().any(|item| item.parent() == Some(*effect))
                {
                    return Err(
                        "source effect canceled · source chain changed or unsupported".into(),
                    );
                }
                Ok(Command::RemoveDevice(*effect))
            })()),
            Request::MoveSourceUtility {
                project_generation: captured_generation,
                revision: captured_revision,
                track,
                source,
                instrument,
                effect,
                neighbor,
                direction,
            } => Some((|| {
                if *captured_generation != project_generation || *captured_revision != revision {
                    return Err("source effect canceled · project changed".into());
                }
                if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    return Err(
                        "finish or cancel the active gesture before editing a source".into(),
                    );
                }
                let project = session.project();
                if project
                    .track(*track)
                    .is_none_or(|track| track.source_id() != Some(*source))
                {
                    return Err("source effect canceled · track binding changed".into());
                }
                let definition = project
                    .native_source(*source)
                    .ok_or("source effect canceled · source no longer exists")?;
                let default_instrument = hex_project::default_instrument_state()
                    .map_err(|_| "source instrument unavailable")?;
                let effects = definition.post_instrument_effects();
                if definition.receiver().is_none()
                    || definition.instrument().id() != *instrument
                    || definition.instrument().state().type_id() != default_instrument.type_id()
                    || effects.iter().any(|item| {
                        item.state().type_id()
                            != hex_lattice_contract::DeviceTypeId::from_static("hex.utility")
                    })
                {
                    return Err(
                        "source effect canceled · source chain changed or unsupported".into(),
                    );
                }
                let index = effects
                    .iter()
                    .position(|item| item.id() == *effect)
                    .ok_or("source effect canceled · source chain changed")?;
                let neighbor_index = match direction {
                    SourceUtilityMove::Earlier => index.checked_sub(1),
                    SourceUtilityMove::Later => index.checked_add(1),
                }
                .ok_or("source effect canceled · source chain changed")?;
                let target = effects
                    .get(index)
                    .ok_or("source effect canceled · source chain changed")?;
                let adjacent = effects
                    .get(neighbor_index)
                    .ok_or("source effect canceled · source chain changed")?;
                if adjacent.id() != *neighbor
                    || target.parent() != adjacent.parent()
                    || definition.devices().any(|item| {
                        item.parent() == Some(*effect) || item.parent() == Some(*neighbor)
                    })
                {
                    return Err(
                        "source effect canceled · source chain changed or contains children".into(),
                    );
                }
                let before = match direction {
                    SourceUtilityMove::Earlier => Some(*neighbor),
                    SourceUtilityMove::Later => index
                        .checked_add(2)
                        .and_then(|next| effects.get(next))
                        .map(hex_project::Device::id),
                };
                Ok(Command::MoveNativeSourceDevice {
                    source: *source,
                    device: *effect,
                    before,
                })
            })()),
            Request::RebindNativeSource {
                project_generation: captured_generation,
                revision: captured_revision,
                track,
                expected_source,
                source,
            } => Some((|| {
                if *captured_generation != project_generation || *captured_revision != revision {
                    return Err("source assignment canceled · project changed".into());
                }
                if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    return Err(
                        "finish or cancel the active gesture before assigning a source".into(),
                    );
                }
                let project = session.project();
                let target = project
                    .track(*track)
                    .ok_or("source assignment canceled · track no longer exists")?;
                if target.is_group() {
                    return Err("group tracks cannot have an instrument source".into());
                }
                if target.source_id() != *expected_source {
                    return Err("source assignment canceled · track binding changed".into());
                }
                if *expected_source == *source {
                    return Err("source assignment is unchanged".into());
                }
                if source.is_some_and(|id| project.native_source(id).is_none()) {
                    return Err("source assignment canceled · source no longer exists".into());
                }
                if expected_source.is_some_and(|id| {
                    project
                        .native_source(id)
                        .is_some_and(|old| old.receiver() == Some(*track))
                        && project
                            .source_contributors(id)
                            .any(|contributor| contributor.id() != *track)
                }) {
                    return Err("choose another receiver before unlinking a shared source".into());
                }
                Ok(Command::BindTrackSource {
                    track: *track,
                    source: *source,
                })
            })()),
            Request::BindUnassignedNativeSource {
                project_generation: captured_generation,
                revision: captured_revision,
                track,
                source,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("source assignment canceled · project changed".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before assigning a source".into())
                } else if session.project().track(*track).is_none() {
                    Err("source assignment canceled · track no longer exists".into())
                } else if session
                    .project()
                    .track(*track)
                    .is_some_and(|track| track.source_id().is_some())
                {
                    Err("source assignment canceled · track already has a source".into())
                } else if session.project().native_source(*source).is_none() {
                    Err("source assignment canceled · source no longer exists".into())
                } else {
                    Ok(Command::BindTrackSource {
                        track: *track,
                        source: Some(*source),
                    })
                },
            ),
            Request::StartPluginEditorRecording { request_id, input } => {
                let result = (|| {
                    let current = hex_engine::AudioSourceContext {
                        project_generation,
                        editor_revision: revision,
                    };
                    if input.context != current
                        || pending_sampler_parameter.is_some()
                        || device_gesture.snapshot().is_some()
                        || tuning_gesture.snapshot().is_some()
                        || session.audio_mode() != DocumentAudioMode::Resident
                        || !events.resident_publication_enabled()
                    {
                        return Err(
                            "vst3 editor recording requires the current resident source".into()
                        );
                    }
                    let device = session
                        .project()
                        .device(input.target.device())
                        .filter(|device| !device.bypassed())
                        .ok_or("vst3 editor recording device changed")?;
                    if vst3_automation_class(device.state()) != Some(input.target.class()) {
                        return Err("vst3 editor recording class changed".into());
                    }
                    let target = match input.target {
                        PluginEditorRecordingTarget::Single {
                            device: id,
                            class,
                            parameter,
                            ..
                        } => {
                            let reading = device
                                .automation_target(hex_project::DeviceAutomationTarget::Vst3 {
                                    parameter,
                                })
                                .is_some_and(hex_project::DeviceAutomation::is_reading);
                            PluginEditorRecordingTarget::Single {
                                device: id,
                                class,
                                parameter,
                                reading,
                            }
                        }
                        PluginEditorRecordingTarget::Group(group) => {
                            if group.ids().iter().any(|&parameter| {
                                device
                                    .automation_target(hex_project::DeviceAutomationTarget::Vst3 {
                                        parameter,
                                    })
                                    .is_none_or(|lane| lane.vst3_class != Some(group.class()))
                            }) {
                                return Err(
                                    "assign each vst3 group parameter to an automation lane first"
                                        .into(),
                                );
                            }
                            input.target
                        }
                    };
                    let sink = events
                        .resident_audio()
                        .ok_or("vst3 output is unavailable")?;
                    plugin_recording::Preparation {
                        state: plugin_recording,
                        lane: native_automation_take,
                        document_base: media_base.as_deref(),
                        recovery: recovery.as_deref(),
                        may_defer: false,
                        deferred: false,
                    }
                    .begin_editor(
                        session,
                        PluginEditorRecordingInput { target, ..*input },
                        *request_id,
                        sink,
                    )
                })();
                let mut completion = DeviceAutomationTakeCompletion {
                    request_id: *request_id,
                    progress: DeviceAutomationTakeProgress::Refused,
                };
                let resolved = match result {
                    Ok(progress) => {
                        completion.progress = progress;
                        None
                    }
                    Err(reason) => Some(Err(reason)),
                };
                device_automation_take_complete = Some(completion);
                resolved
            }
            Request::StopPluginEditorRecording { request_id, key } => {
                let stopped = events.resident_audio().is_some_and(|sink| {
                    plugin_recording::Preparation {
                        state: plugin_recording,
                        lane: native_automation_take,
                        document_base: media_base.as_deref(),
                        recovery: recovery.as_deref(),
                        may_defer: false,
                        deferred: false,
                    }
                    .stop_editor(*key, *request_id, sink)
                });
                device_automation_take_complete = Some(DeviceAutomationTakeCompletion {
                    request_id: *request_id,
                    progress: if stopped {
                        DeviceAutomationTakeProgress::Pending
                    } else {
                        DeviceAutomationTakeProgress::Refused
                    },
                });
                (!stopped).then(|| Err("vst3 editor take is no longer active".into()))
            }
            Request::ContinuePluginEditorRecording { request_id } => {
                use native_automation_recording::Step;
                let current = hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                };
                plugin_recording
                    .finish_editor(*request_id, native_automation_take)
                    .and_then(|outcome| {
                        let mut completion = DeviceAutomationTakeCompletion {
                            request_id: *request_id,
                            progress: DeviceAutomationTakeProgress::Refused,
                        };
                        let resolved = match outcome.result {
                            Ok(Step::Commit(command, path))
                                if outcome.scope.context() == current =>
                            {
                                adopted_device_draft = Some((path, outcome.scope.target.into()));
                                Some(Ok(*command))
                            }
                            Ok(Step::Commit(_, path)) => {
                                completion.progress = DeviceAutomationTakeProgress::Retained(path);
                                None
                            }
                            Ok(Step::Progress(progress)) => {
                                completion.progress = progress;
                                None
                            }
                            Err(reason) => Some(Err(reason)),
                        };
                        device_automation_take_complete = Some(completion);
                        resolved
                    })
            }
            Request::QueryDeviceAutomationCloseSafety { request_id }
            | Request::PreparePluginEditorClose { request_id, .. } => {
                let context = hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                };
                let attempt = match &request {
                    Request::PreparePluginEditorClose {
                        context: expected,
                        attempt,
                        ..
                    } => Some((*expected, attempt)),
                    _ => None,
                };
                let progress = if attempt.is_some_and(|(expected, attempt)| {
                    expected != context || !attempt.upgrade().is_some_and(|owner| owner.is_valid())
                }) {
                    DeviceAutomationTakeProgress::ClosePluginEditor(
                        PluginEditorCloseAdmission::Refused,
                    )
                } else if !native_automation_take.close_safe() || !plugin_recording.close_safe() {
                    DeviceAutomationTakeProgress::CloseBlocked
                } else if let Some(blocker) = events
                    .resident_audio()
                    .and_then(|sink| sink.retiring_editor_close_blocker())
                {
                    DeviceAutomationTakeProgress::CloseRetiringEditor(blocker)
                } else if let Some((_, attempt)) = attempt {
                    let admission = events.sampler_edit_audio().map_or(
                        PluginEditorCloseAdmission::Held {
                            admitted_edits: false,
                        },
                        |sink| sink.prepare_plugin_editor_close(context, attempt),
                    );
                    DeviceAutomationTakeProgress::ClosePluginEditor(admission)
                } else {
                    DeviceAutomationTakeProgress::CloseSafe
                };
                device_automation_take_complete = Some(DeviceAutomationTakeCompletion {
                    request_id: *request_id,
                    progress,
                });
                None
            }
            Request::QueryFailedPluginAutomation { request_id } => {
                let gesture_active = pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some();
                let result = plugin_recording::failed_custody(
                    plugin_recording,
                    native_automation_take,
                    gesture_active,
                );
                let mut completion = DeviceAutomationTakeCompletion {
                    request_id: *request_id,
                    progress: DeviceAutomationTakeProgress::Refused,
                };
                let resolved = match result {
                    Ok(progress) => {
                        completion.progress = progress;
                        None
                    }
                    Err(reason) => Some(Err(reason)),
                };
                device_automation_take_complete = Some(completion);
                resolved
            }
            Request::InspectSealedAutomationRecovery { request_id, path } => {
                let mut completion = DeviceAutomationTakeCompletion {
                    request_id: *request_id,
                    progress: DeviceAutomationTakeProgress::Refused,
                };
                let resolved = match native_automation_recording::inspect_sealed_file(path) {
                    Ok(file) => {
                        completion.progress =
                            DeviceAutomationTakeProgress::SealedFile(Box::new(file));
                        None
                    }
                    Err(reason) => Some(Err(reason)),
                };
                device_automation_take_complete = Some(completion);
                resolved
            }
            Request::RecoverDeviceAutomationTake {
                request_id,
                context,
                target,
                expected,
                path,
                identity,
            } => {
                let current = hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                };
                let result = if *context != current
                    || pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                    || !plugin_recording::generic_sealed_recovery_idle(
                        plugin_recording,
                        native_automation_take,
                    )
                    || session.audio_mode() != DocumentAudioMode::Resident
                    || !events.resident_publication_enabled()
                {
                    Err("sealed recovery requires the idle current resident source".into())
                } else {
                    native_automation_recording::recover_sealed(
                        session,
                        media_base.as_deref(),
                        *target,
                        *expected,
                        path,
                        *identity,
                    )
                };
                device_automation_take_complete = Some(DeviceAutomationTakeCompletion {
                    request_id: *request_id,
                    progress: DeviceAutomationTakeProgress::Refused,
                });
                Some(match result {
                    Ok((command, publication)) => {
                        adopted_device_draft = Some((path.clone(), publication));
                        Ok(*command)
                    }
                    Err(reason) => Err(reason),
                })
            }
            Request::DeviceAutomationTake {
                request_id,
                scope,
                action,
            } => {
                use native_automation_recording::Step;
                let current = hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                };
                let document_base = media_base.as_deref();
                let gesture_active = pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some();
                let result = if matches!(action, DeviceAutomationTakeAction::Begin) {
                    (|| {
                        if scope.context() != current
                            || gesture_active
                            || session.audio_mode() != DocumentAudioMode::Resident
                            || !events.resident_publication_enabled()
                        {
                            return Err(
                                "device automation take requires the current resident source"
                                    .into(),
                            );
                        }
                        let source = events
                            .resident_audio()
                            .ok_or("native automation output unavailable")?
                            .device_automation_source(current, scope.target)?
                            .ok_or("automation target is not available on the playing output")?;
                        let directory = match recovery.as_deref().and_then(Path::parent) {
                            Some(directory) => directory.join("automation-drafts"),
                            None => automation_draft::default_directory()?,
                        };
                        native_automation_take.begin(
                            session,
                            document_base,
                            *scope,
                            source,
                            &directory,
                        )
                    })()
                } else if let DeviceAutomationTakeAction::CopyFailedPlugin {
                    destination,
                    max_events,
                } = action
                {
                    plugin_recording::start_failed_copy(
                        plugin_recording,
                        native_automation_take,
                        *scope,
                        destination,
                        *max_events,
                        gesture_active,
                    )
                } else if matches!(action, DeviceAutomationTakeAction::InspectFailedPluginCopy) {
                    plugin_recording::inspect_failed_copy(
                        plugin_recording,
                        native_automation_take,
                        *scope,
                        gesture_active,
                    )
                } else if let DeviceAutomationTakeAction::KeepFailedPluginCopy {
                    path,
                    receipt,
                    max_events,
                } = action
                {
                    plugin_recording::keep_failed_copy(
                        plugin_recording,
                        native_automation_take,
                        *scope,
                        path,
                        *receipt,
                        *max_events,
                        gesture_active,
                    )
                } else if let DeviceAutomationTakeAction::AdoptFailedPluginCopy { max_events } =
                    action
                {
                    (|| {
                        if scope.context() != current
                            || gesture_active
                            || session.audio_mode() != DocumentAudioMode::Resident
                            || !events.resident_publication_enabled()
                        {
                            return Err(
                                "vst3 recovery requires the current idle resident source".into()
                            );
                        }
                        let source = events
                            .resident_audio()
                            .ok_or("vst3 recovery output unavailable")?
                            .device_automation_source(current, scope.target)?;
                        let (command, adoption) = plugin_recording::prepare_failed_adoption(
                            plugin_recording,
                            native_automation_take,
                            session,
                            document_base,
                            *scope,
                            source,
                            *max_events,
                        )?;
                        let path = adoption.path.clone();
                        failed_plugin_adoption = Some(adoption);
                        Ok(Step::Commit(command, path))
                    })()
                } else if gesture_active
                    && matches!(
                        action,
                        DeviceAutomationTakeAction::PrepareNativeRecovery
                            | DeviceAutomationTakeAction::CopyNativeRecovery { .. }
                            | DeviceAutomationTakeAction::KeepNativeRecoveryCopy { .. }
                    )
                {
                    Err("native recovery requires a quiescent gesture".into())
                } else if gesture_active && matches!(action, DeviceAutomationTakeAction::Poll) {
                    // A take must never finalize somebody else's active edit.
                    Ok(Step::Progress(DeviceAutomationTakeProgress::Pending))
                } else {
                    native_automation_take.receive(session, document_base, current, *scope, action)
                };
                let mut completion = DeviceAutomationTakeCompletion {
                    request_id: *request_id,
                    progress: DeviceAutomationTakeProgress::Refused,
                };
                let resolved = match result {
                    Ok(Step::Progress(progress)) => {
                        completion.progress = progress;
                        None
                    }
                    Ok(Step::Commit(command, path)) => {
                        adopted_device_draft = Some((path, scope.target.into()));
                        Some(Ok(*command))
                    }
                    Err(reason) => Some(Err(reason)),
                };
                device_automation_take_complete = Some(completion);
                resolved
            }
            Request::DeviceAutomation {
                project_generation: _,
                revision: _,
                device,
                target,
                vst3_class,
                points,
            } => Some(if let Some(reason) = automation_source_error {
                Err(reason.into())
            } else if !device_automation_control::accepts_lane_target(
                session,
                *device,
                *target,
                *vst3_class,
            ) {
                Err("automation edit refused · target no longer available".into())
            } else {
                Ok(Command::ReplaceDeviceAutomationTarget {
                    device: *device,
                    target: *target,
                    points: points.clone(),
                })
            }),
            Request::DeviceAutomationControl { edit, .. } => Some(match automation_source_error {
                Some(reason) => Err(reason.into()),
                None => device_automation_control::command(session, edit),
            }),
            Request::MoveTrackDevices {
                project_generation: generation,
                revision: source_revision,
                source,
                selected,
                destination,
            } => Some(
                if *generation != project_generation || *source_revision != revision {
                    Err("device move canceled · project changed".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before moving devices".into())
                } else {
                    Ok(Command::MoveTrackDevices {
                        source: *source,
                        selected: selected.clone(),
                        destination: *destination,
                    })
                },
            ),
            Request::ReorderDevices {
                project_generation: generation,
                revision: source_revision,
                owner,
                selected,
                before,
            } => Some(
                if *generation != project_generation || *source_revision != revision {
                    Err("device reorder canceled · project changed".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before reordering devices".into())
                } else {
                    Ok(Command::ReorderDevices {
                        owner: *owner,
                        selected: selected.clone(),
                        before: *before,
                    })
                },
            ),
            Request::DeviceClipboard {
                project_generation: generation,
                revision: source_revision,
                action,
            } => {
                if *generation != project_generation || *source_revision != revision {
                    Some(Err("device clipboard canceled · project changed".into()))
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Some(Err(
                        "finish or cancel the active gesture before copying devices".into(),
                    ))
                } else {
                    match action {
                        DeviceClipboardAction::Copy { .. } => None,
                        DeviceClipboardAction::Cut { owner, selected } => Some((|| {
                            let cancellation = RequestCancellation::new(
                                Arc::clone(&session.shutdown),
                                Arc::clone(document_intent),
                                exec.outcome.request_document_intent,
                            );
                            let captured = plugin_state_save::capture_devices_for_clipboard(
                                session.project(),
                                *owner,
                                selected,
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision,
                                },
                                &cancellation,
                                events.sampler_edit_audio(),
                                "device cut cancelled",
                            )?;
                            exec.outcome.cut_device_clipboard = Some(Arc::new(captured));
                            Ok(Command::RemoveDevices {
                                owner: *owner,
                                selected: selected.clone(),
                            })
                        })(
                        )),
                        DeviceClipboardAction::Paste { owner, before } => Some(
                            device_clipboard
                                .as_ref()
                                .ok_or_else(|| {
                                    "device clipboard is empty · copy devices first".to_owned()
                                })
                                .map(|clipboard| Command::PasteDevices {
                                    clipboard: Arc::clone(clipboard),
                                    owner: *owner,
                                    before: *before,
                                }),
                        ),
                        DeviceClipboardAction::Duplicate {
                            source,
                            selected,
                            destination,
                            before,
                        } => Some((|| {
                            let cancellation = RequestCancellation::new(
                                Arc::clone(&session.shutdown),
                                Arc::clone(document_intent),
                                exec.outcome.request_document_intent,
                            );
                            let clipboard = plugin_state_save::capture_devices_for_clipboard(
                                session.project(),
                                *source,
                                selected,
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision,
                                },
                                &cancellation,
                                events.sampler_edit_audio(),
                                "device duplication cancelled",
                            )?;
                            Ok(Command::PasteDevices {
                                clipboard: Arc::new(clipboard),
                                owner: *destination,
                                before: *before,
                            })
                        })()),
                    }
                }
            }
            Request::TrackClipboard {
                project_generation: generation,
                revision: source_revision,
                action,
            } => {
                if *generation != project_generation || *source_revision != revision {
                    Some(Err("track clipboard canceled · project changed".into()))
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Some(Err(
                        "finish or cancel the active gesture before copying tracks".into(),
                    ))
                } else {
                    match action {
                        TrackClipboardAction::Copy(_) => None,
                        TrackClipboardAction::Cut(tracks) => Some((|| {
                            let cancellation = RequestCancellation::new(
                                Arc::clone(&session.shutdown),
                                Arc::clone(document_intent),
                                exec.outcome.request_document_intent,
                            );
                            let captured = plugin_state_save::capture_tracks_for_clipboard(
                                session.project(),
                                tracks,
                                false,
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision,
                                },
                                &cancellation,
                                events.sampler_edit_audio(),
                                "track cut cancelled",
                            )?;
                            exec.outcome.cut_clipboard = Some(Arc::new(captured));
                            Ok(Command::RemoveTracks(tracks.ids().to_vec()))
                        })()),
                        TrackClipboardAction::Paste { before } => Some(
                            track_clipboard
                                .as_ref()
                                .ok_or_else(|| {
                                    "track clipboard is empty · copy tracks first".to_owned()
                                })
                                .and_then(|clipboard| {
                                    session
                                        .project()
                                        .track_destination_before(*before)
                                        .map(|destination| Command::PasteTracks {
                                            clipboard: Arc::clone(clipboard),
                                            destination,
                                        })
                                        .map_err(|error| error.to_string())
                                }),
                        ),
                        TrackClipboardAction::Duplicate {
                            tracks,
                            without_clips,
                        } => Some((|| {
                            let project = session.project();
                            let destination = project
                                .track_destination_after_selection(tracks)
                                .map_err(|error| error.to_string())?;
                            let cancellation = RequestCancellation::new(
                                Arc::clone(&session.shutdown),
                                Arc::clone(document_intent),
                                exec.outcome.request_document_intent,
                            );
                            let clipboard = plugin_state_save::capture_tracks_for_clipboard(
                                project,
                                tracks,
                                *without_clips,
                                hex_engine::AudioSourceContext {
                                    project_generation,
                                    editor_revision: revision,
                                },
                                &cancellation,
                                events.sampler_edit_audio(),
                                "track duplication cancelled",
                            )?;
                            Ok(Command::PasteTracks {
                                clipboard: Arc::new(clipboard),
                                destination,
                            })
                        })()),
                    }
                }
            }

            Request::ReorderTracks {
                project_generation: captured_generation,
                revision: captured_revision,
                tracks,
                before,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("track reorder canceled · project changed".to_owned())
                } else if tracks
                    .ids()
                    .iter()
                    .any(|id| session.project().track(*id).is_none())
                    || before.is_some_and(|anchor| session.project().track(anchor).is_none())
                {
                    Err("track reorder target no longer exists".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before reordering tracks".into())
                } else {
                    let project = session.project();
                    project
                        .track_destination_before(*before)
                        .map(|destination| {
                            // A gap can change routing as well as row order. Use
                            // the same destination and receipt as a Group drop.
                            if let [id] = tracks.ids()
                                && project.track(*id).is_some_and(|track| {
                                    track.parent_group() != destination.parent_group
                                })
                            {
                                Command::ReparentTracks {
                                    tracks: tracks.clone(),
                                    destination,
                                }
                            } else {
                                Command::MoveTracksBefore {
                                    tracks: tracks.clone(),
                                    before: *before,
                                }
                            }
                        })
                        .map_err(|error| error.to_string())
                },
            ),
            Request::ReparentTracks {
                project_generation: captured_generation,
                revision: captured_revision,
                tracks,
                destination,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("track reparent canceled · project changed".to_owned())
                } else if tracks
                    .ids()
                    .iter()
                    .any(|id| session.project().track(*id).is_none())
                    || destination
                        .before
                        .is_some_and(|id| session.project().track(id).is_none())
                    || destination
                        .parent_group
                        .is_some_and(|id| session.project().track(id).is_none())
                {
                    Err("track reparent target no longer exists".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before reparenting tracks".into())
                } else {
                    Ok(Command::ReparentTracks {
                        tracks: tracks.clone(),
                        destination: *destination,
                    })
                },
            ),
            Request::InsertBrowserInstrumentTrack {
                project_generation: expected_generation,
                revision: expected_revision,
                class,
                type_id,
                new_track_gain,
                ..
            } => Some(
                if *expected_generation != project_generation || *expected_revision != revision {
                    Err("instrument insertion canceled · project changed".to_owned())
                } else if device_gesture.snapshot().is_some() || tuning_gesture.snapshot().is_some()
                {
                    Err(
                        "finish or cancel the active device edit before inserting an instrument"
                            .into(),
                    )
                } else if *class != hex_engine::NativeDeviceClass::Instrument {
                    Err("browser selection is not an instrument".into())
                } else {
                    hex_engine::default_native_device_state_for_class(*class, *type_id)
                        .map_err(|reason| {
                            hex_storage::text::lowercase_outside_verbatim(&reason.to_string())
                        })
                        .and_then(|state| {
                            device_defaults::initialize(
                                session,
                                device_track_command(
                                    None,
                                    hex_project::TrackDeviceSlot::Instrument,
                                    state,
                                    native_device_name(*class, *type_id)?,
                                    *new_track_gain,
                                ),
                                *class,
                                defaults_root,
                            )
                        })
                        .map(|prepared| {
                            exec.outcome.pending_unit_originals = prepared.originals;
                            prepared.command
                        })
                },
            ),
            Request::InsertEffectBusDevice {
                project_generation: expected_generation,
                revision: expected_revision,
                destination,
                class,
                type_id,
                before,
                ..
            } => Some(
                if *expected_generation != project_generation || *expected_revision != revision {
                    Err("effect insertion canceled · project changed".to_owned())
                } else if device_gesture.snapshot().is_some() || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active device edit before inserting an effect".into())
                } else {
                    effect_bus::insertion_command(
                        session.project(),
                        *destination,
                        *class,
                        *type_id,
                        *before,
                    )
                    .and_then(|command| {
                        device_defaults::initialize(session, command, *class, defaults_root)
                    })
                    .map(|prepared| {
                        exec.outcome.pending_unit_originals = prepared.originals;
                        prepared.command
                    })
                },
            ),
            Request::InsertNestedCatalogDevice {
                project_generation: expected_generation,
                revision: expected_revision,
                parent,
                class,
                type_id,
                ..
            } => Some(
                if *expected_generation != project_generation || *expected_revision != revision {
                    Err("nested device insertion canceled · project changed".to_owned())
                } else if device_gesture.snapshot().is_some() || tuning_gesture.snapshot().is_some()
                {
                    Err(
                        "finish or cancel the active device edit before inserting a nested device"
                            .into(),
                    )
                } else if *class != hex_engine::NativeDeviceClass::Effect {
                    Err("nested device slots accept compatible effects only".into())
                } else {
                    hex_engine::native_effect_admitted_for_nested_parent(
                        session.project(),
                        *parent,
                        *type_id,
                    )
                    .map_err(str::to_owned)
                    .and_then(|()| {
                        hex_engine::default_native_device_state_for_class(*class, *type_id).map_err(
                            |error| {
                                hex_storage::text::lowercase_outside_verbatim(&error.to_string())
                            },
                        )
                    })
                    .and_then(|state| {
                        device_defaults::initialize(
                            session,
                            Command::InsertNestedDevice {
                                parent: *parent,
                                state,
                            },
                            *class,
                            defaults_root,
                        )
                    })
                    .map(|prepared| {
                        exec.outcome.pending_unit_originals = prepared.originals;
                        prepared.command
                    })
                },
            ),
            Request::LoadTrackCatalogDevice {
                project_generation: expected_generation,
                revision: expected_revision,
                track,
                class,
                type_id,
                placement,
                ..
            } => Some(
                if *expected_generation != project_generation || *expected_revision != revision {
                    Err("device picker load canceled · project changed".into())
                } else if device_gesture.snapshot().is_some() || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active device edit before loading a device".into())
                } else {
                    device_defaults::track_picker_command(
                        session,
                        *track,
                        *class,
                        *type_id,
                        *placement,
                        defaults_root,
                    )
                    .map(|prepared| {
                        exec.outcome.pending_unit_originals = prepared.originals;
                        prepared.command
                    })
                },
            ),
            Request::ReplaceDeviceModulation {
                project_generation: captured_generation,
                revision: captured_revision,
                device,
                modulation,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("modulation edit cancelled · project changed".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before editing modulation".into())
                } else {
                    Ok(Command::ReplaceDeviceModulation {
                        device: *device,
                        modulation: modulation.clone(),
                    })
                },
            ),
            Request::PaintMidiClip(intent) => Some(intent.resolve(
                hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                },
                pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some(),
            )),
            Request::MoveExistingClips {
                project_generation: captured_generation,
                revision: captured_revision,
                command,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("clip move canceled · project changed".to_owned())
                } else if !match command {
                    Command::MoveClipWithOverwrite { .. }
                    | Command::MoveMidiClipWithOverwrite { .. }
                    | Command::CopyClipTo { .. }
                    | Command::CopyMidiClipTo { .. }
                    | Command::MoveClipToNewTrack { .. }
                    | Command::MoveMidiClipToNewTrack { .. }
                    | Command::CopyClipToNewTrack { .. }
                    | Command::CopyMidiClipToNewTrack { .. }
                    | Command::DuplicateAliasClipToNewTrack { .. } => true,
                    Command::MoveClipsWithOverwrite { clips, .. }
                    | Command::DuplicateAliasClipsAt { clips, .. } => {
                        !clips.is_empty() && clips.len() <= hex_project::MAX_CLIPS
                    }
                    _ => false,
                } {
                    Err("clip move canceled · invalid move request".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before moving clips".into())
                } else {
                    Ok(command.clone())
                },
            ),
            Request::CopyArrangementClipToConsoleTrack {
                project_generation: captured_generation,
                revision: captured_revision,
                source,
                before_track,
                scene,
                initial_gain,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("console clip insertion canceled · project changed".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err(
                        "finish or cancel the active gesture before inserting a console clip"
                            .into(),
                    )
                } else if session.project().clip(*source).is_some() {
                    Ok(Command::CopyArrangementAudioClipToNewSessionTrack {
                        source: *source,
                        before_track: *before_track,
                        scene: *scene,
                        initial_gain: *initial_gain,
                    })
                } else if session.project().midi_clip(*source).is_some() {
                    Ok(Command::CopyArrangementMidiToNewSessionTrack {
                        source: *source,
                        before_track: *before_track,
                        scene: *scene,
                        initial_gain: *initial_gain,
                    })
                } else {
                    Err("arrangement source no longer exists".into())
                },
            ),
            Request::DuplicateClips {
                project_generation: captured_generation,
                revision: captured_revision,
                clips,
            }
            | Request::DuplicateAliasClips {
                project_generation: captured_generation,
                revision: captured_revision,
                clips,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("clip duplication canceled · project changed".to_owned())
                } else if clips.is_empty() || clips.len() > hex_project::MAX_CLIPS {
                    Err("clip duplication exceeds project limits".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before duplicating clips".into())
                } else {
                    Ok(if matches!(&request, Request::DuplicateAliasClips { .. }) {
                        Command::DuplicateAliasClips(clips.clone())
                    } else {
                        Command::DuplicateClips(clips.clone())
                    })
                },
            ),
            Request::DuplicateSessionCell {
                project_generation: captured_generation,
                revision: captured_revision,
                track,
                scene,
                alias,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("console duplication canceled · project changed".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before duplicating clips".into())
                } else {
                    Ok(Command::DuplicateSessionCell {
                        track: *track,
                        scene: *scene,
                        alias: *alias,
                    })
                },
            ),
            Request::GroupClips {
                project_generation: captured_generation,
                revision: captured_revision,
                clips,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("clip grouping canceled · project changed".into())
                } else if clips.is_empty() || clips.len() > hex_project::MAX_CLIPS {
                    Err("invalid clip grouping selection count".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before grouping clips".into())
                } else {
                    Ok(Command::GroupClips(clips.clone()))
                },
            ),
            Request::GroupTimeRange {
                project_generation: captured_generation,
                revision: captured_revision,
                range,
                create_if_empty,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("clip grouping canceled · project changed".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before grouping clips".into())
                } else if *create_if_empty {
                    Ok(Command::GroupTimeRangeOrCreate(range.clone()))
                } else {
                    Ok(Command::GroupTimeRange(range.clone()))
                },
            ),
            Request::GroupAudioEvents {
                project_generation: captured_generation,
                revision: captured_revision,
                clip,
                start,
                end,
            } => Some(
                if *captured_generation != project_generation || *captured_revision != revision {
                    Err("audio event grouping canceled · project changed".into())
                } else if start >= end {
                    Err("select audio events to group".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Err("finish or cancel the active gesture before grouping audio events".into())
                } else {
                    Ok(Command::GroupAudioEventRange {
                        clip: *clip,
                        start: *start,
                        end: *end,
                    })
                },
            ),
            _ => None,
        };
        let (request, resolution_failure) = match resolved_edit {
            Some(Ok(command)) => (Request::Edit(command), None),
            Some(Err(reason)) => (request, Some(reason)),
            None => (request, None),
        };
        exec.meta.device_automation_take_complete = device_automation_take_complete;
        exec.meta.adopted_device_draft = adopted_device_draft;
        exec.meta.failed_plugin_adoption = failed_plugin_adoption;
        exec.outcome.failed_plugin_publication_confirmed = failed_plugin_publication_confirmed;
        (request, resolution_failure)
    }
}
