//! Device insertion, replacement, drag/drop, clipboard and A/B comparison worker operations.
use super::super::*;
use super::{arrangement_device_command, native_device_name};
use std::ops::ControlFlow;

pub(super) fn handle(ctx: &mut WorkerDispatch<'_>, request: Request) -> ControlFlow<()> {
    let mut flow = ControlFlow::Continue(());
    match request {
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
                if outcome == DeviceInsertOutcome::ProjectCommitFailed {
                    reconcile_audio_after_commit_failure(ctx, verified.is_some());
                }
                if let Some(plugin) = plugin {
                    ctx.outcome
                        .plugin_effect_complete
                        .push(plugin.finish(ctx.worker.events.sampler_edit_audio(), accepted));
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
                        && (ctx.worker.session.uses_ordinary_worker_publication()
                            || ctx.worker.resident_snapshot.is_some())
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

        _ => {
            ctx.outcome.error = Some("unexpected worker dispatch route".into());
        }
    }
    flow
}

/// Reconcile native audio when the callback installed a staged device edit but
/// the canonical project commit failed. Re-publishes the committed project
/// snapshot in Resident mode (or restores compiled playback in legacy mode),
/// and closes/fences the audio sink with an explicit recovery message whenever
/// resynchronization cannot be guaranteed.
fn reconcile_audio_after_commit_failure(ctx: &mut WorkerDispatch<'_>, was_verified: bool) {
    let base_error = ctx
        .outcome
        .error
        .take()
        .unwrap_or_else(|| "project commit failed".into());
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
    ctx.outcome.error = Some(reconcile_commit_failure_audio(
        &ctx.worker.session,
        &mut ctx.worker.events,
        &mut ctx.worker.resident_snapshot,
        ctx.worker.compiled.as_ref(),
        admission,
        context,
        was_verified,
        base_error,
        &mut ctx.outcome.restore_prepared,
    ));
}

#[allow(clippy::too_many_arguments)]
fn reconcile_commit_failure_audio(
    session: &Session,
    events: &mut WorkerEvents,
    resident_snapshot: &mut Option<resident_audio::Snapshot>,
    compiled: Option<&(u64, Arc<PreparedRevision>)>,
    admission: sampler_export::Admission<'_>,
    context: hex_engine::AudioSourceContext,
    was_verified: bool,
    base_error: String,
    restore_prepared: &mut Option<Arc<PreparedRevision>>,
) -> String {
    if !was_verified
        && events.resident_publication_enabled()
        && (session.uses_ordinary_worker_publication() || resident_snapshot.is_some())
    {
        let resync = events
            .resident_audio()
            .ok_or(SessionError::Invalid("native worker unavailable"))
            .and_then(|sink| {
                session.publish_resident_snapshot(
                    context,
                    sink,
                    admission,
                    resident_snapshot.as_ref(),
                )
            });
        match resync {
            Ok(restored) => {
                *resident_snapshot = Some(restored);
                format!("{base_error}; audio resynchronized to committed project")
            }
            Err(resync_error) => {
                if let Some(sink) = events.sampler_edit_audio() {
                    sink.close();
                }
                format!(
                    "{base_error}; audio output closed after resync failed ({resync_error}) · restart audio to recover"
                )
            }
        }
    } else if was_verified || session.audio_mode() == DocumentAudioMode::VerifiedStreaming {
        if let Some(sink) = events.sampler_edit_audio() {
            sink.close();
        }
        format!(
            "{base_error}; audio output closed to prevent model desync · restart audio to recover"
        )
    } else if let Some((_, prepared)) = compiled {
        *restore_prepared = Some(Arc::clone(prepared));
        format!("{base_error}; audio resynchronized to committed project")
    } else {
        if let Some(sink) = events.sampler_edit_audio() {
            sink.close();
        }
        format!(
            "{base_error}; audio output closed to prevent model desync · restart audio to recover"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::arrangement_device_command;
    use super::*;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    struct CommitFailureSink {
        resident: bool,
        allow_publish: bool,
        published_contexts: Arc<Mutex<Vec<hex_engine::AudioSourceContext>>>,
        close_calls: Arc<AtomicUsize>,
    }

    impl WorkerAudioSink for CommitFailureSink {
        fn initialize(&mut self, _shutdown: &AtomicBool) -> Result<u32, String> {
            Ok(48_000)
        }
        fn canonical_changed(&mut self, _source: Arc<CanonicalAudioSourceSnapshot>) {}
        fn poll(&mut self) {}
        fn close(&mut self) {
            self.close_calls.fetch_add(1, Ordering::AcqRel);
        }
        fn resident_publication_enabled(&self) -> bool {
            self.resident
        }
        fn publish_resident(
            &mut self,
            playback: Playback,
            _resources: Option<&hex_engine::native_device_resources::PreparedDeviceResources>,
            _observation: Arc<hex_engine::PreparedPlaybackObservation>,
            metadata: ResidentPublicationMetadata,
            _policy: ResidentPublicationPolicy,
            _metadata_limit: usize,
            _admit: &mut dyn FnMut(
                &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
            ) -> Result<(), String>,
        ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
            if self.allow_publish {
                self.published_contexts
                    .lock()
                    .unwrap()
                    .push(metadata.context);
                Ok(ResidentPublicationAcceptance::Submitted {
                    output_identity: 1,
                    audio_revision: 7,
                })
            } else {
                Err(ResidentPublicationRefusal {
                    kind: ResidentPublicationFailure::Other,
                    playback,
                    error: "native audio sink refused resync".into(),
                })
            }
        }
    }

    fn resident_session_with_limits() -> Session {
        Session::new(48_000, 8, 1024)
            .unwrap()
            .with_native_reservation(crate::ResidentNativeReservation {
                storage: hex_engine::AudioStorageLedgerReservation {
                    data_rows: 128,
                    handle_rows: 16,
                    root_rows: 8,
                    max_retained_bytes: 64 * 1024 * 1024,
                    max_construction_peak_bytes: 64 * 1024 * 1024,
                },
                max_scan_frames: 48_000 * 60,
            })
            .unwrap()
    }

    #[test]
    fn commit_failure_republishes_committed_project_in_resident_mode() {
        let session = resident_session_with_limits();
        let published = Arc::new(Mutex::new(Vec::new()));
        let closes = Arc::new(AtomicUsize::new(0));
        let (tx, _rx) = mpsc::sync_channel(4);
        let mut events = WorkerEvents::new(
            tx,
            Arc::new(AtomicBool::new(false)),
            Some(Box::new(CommitFailureSink {
                resident: true,
                allow_publish: true,
                published_contexts: Arc::clone(&published),
                close_calls: Arc::clone(&closes),
            })),
        );
        let mut snapshot = None;
        let mut restore = None;
        let context = hex_engine::AudioSourceContext {
            project_generation: 3,
            editor_revision: 11,
        };
        let message = reconcile_commit_failure_audio(
            &session,
            &mut events,
            &mut snapshot,
            None,
            sampler_export::Admission::default(),
            context,
            false,
            "staged edit changed".into(),
            &mut restore,
        );
        assert!(
            message.contains("audio resynchronized to committed project"),
            "unexpected message: {message}"
        );
        assert_eq!(*published.lock().unwrap(), vec![context]);
        assert_eq!(closes.load(Ordering::Acquire), 0);
        assert!(snapshot.is_some());
        assert!(restore.is_none());
    }

    #[test]
    fn commit_failure_fences_audio_sink_when_resident_resync_fails() {
        let session = resident_session_with_limits();
        let published = Arc::new(Mutex::new(Vec::new()));
        let closes = Arc::new(AtomicUsize::new(0));
        let (tx, _rx) = mpsc::sync_channel(4);
        let mut events = WorkerEvents::new(
            tx,
            Arc::new(AtomicBool::new(false)),
            Some(Box::new(CommitFailureSink {
                resident: true,
                allow_publish: false,
                published_contexts: Arc::clone(&published),
                close_calls: Arc::clone(&closes),
            })),
        );
        let mut snapshot = None;
        let mut restore = None;
        let context = hex_engine::AudioSourceContext {
            project_generation: 3,
            editor_revision: 11,
        };
        let message = reconcile_commit_failure_audio(
            &session,
            &mut events,
            &mut snapshot,
            None,
            sampler_export::Admission::default(),
            context,
            false,
            "staged edit changed".into(),
            &mut restore,
        );
        assert!(message.contains("restart audio to recover"));
        assert_eq!(closes.load(Ordering::Acquire), 1);
        assert!(snapshot.is_none());
    }

    #[test]
    fn commit_failure_fences_audio_sink_for_verified_graph_edits() {
        let session = resident_session_with_limits();
        let published = Arc::new(Mutex::new(Vec::new()));
        let closes = Arc::new(AtomicUsize::new(0));
        let (tx, _rx) = mpsc::sync_channel(4);
        let mut events = WorkerEvents::new(
            tx,
            Arc::new(AtomicBool::new(false)),
            Some(Box::new(CommitFailureSink {
                resident: true,
                allow_publish: true,
                published_contexts: Arc::clone(&published),
                close_calls: Arc::clone(&closes),
            })),
        );
        let mut snapshot = None;
        let mut restore = None;
        let context = hex_engine::AudioSourceContext {
            project_generation: 4,
            editor_revision: 2,
        };
        let message = reconcile_commit_failure_audio(
            &session,
            &mut events,
            &mut snapshot,
            None,
            sampler_export::Admission::default(),
            context,
            true,
            "staged edit changed".into(),
            &mut restore,
        );
        assert!(message.contains("restart audio to recover"));
        assert_eq!(closes.load(Ordering::Acquire), 1);
        assert!(published.lock().unwrap().is_empty());
    }

    #[test]
    fn commit_failure_restores_compiled_playback_in_legacy_mode() {
        let session = Session::new(48_000, 8, 1024).unwrap();
        let prepared = session.compile_prepared_revision(None).unwrap();
        let closes = Arc::new(AtomicUsize::new(0));
        let (tx, _rx) = mpsc::sync_channel(4);
        let mut events = WorkerEvents::new(
            tx,
            Arc::new(AtomicBool::new(false)),
            Some(Box::new(CommitFailureSink {
                resident: false,
                allow_publish: true,
                published_contexts: Arc::new(Mutex::new(Vec::new())),
                close_calls: Arc::clone(&closes),
            })),
        );
        let mut snapshot = None;
        let mut restore = None;
        let compiled = (1, Arc::clone(&prepared));
        let message = reconcile_commit_failure_audio(
            &session,
            &mut events,
            &mut snapshot,
            Some(&compiled),
            sampler_export::Admission::default(),
            hex_engine::AudioSourceContext {
                project_generation: 1,
                editor_revision: 1,
            },
            false,
            "staged edit changed".into(),
            &mut restore,
        );
        assert!(
            message.contains("audio resynchronized to committed project"),
            "unexpected message: {message}"
        );
        assert_eq!(closes.load(Ordering::Acquire), 0);
        assert!(restore.is_some());
    }

    #[test]
    fn arrangement_device_command_rejects_missing_target_track() {
        let mut session = Session::new(48_000, 8, 1024).unwrap();
        session
            .command(Command::AddTrack {
                name: "track".to_owned(),
            })
            .unwrap();
        let track = session.project().tracks()[0].id();
        session.command(Command::RemoveTrack(track)).unwrap();
        let first =
            hex_engine::native_device_catalog_for_class(hex_engine::NativeDeviceClass::Effect)
                .unwrap()
                .into_iter()
                .next()
                .unwrap();
        let err = arrangement_device_command(
            session.project(),
            hex_engine::NativeDeviceClass::Effect,
            first.type_id,
            ArrangementDeviceDestination::ExistingTrack {
                track,
                instrument: None,
            },
            None,
        )
        .unwrap_err();
        assert_eq!(err, "device drop track no longer exists");
    }
}
