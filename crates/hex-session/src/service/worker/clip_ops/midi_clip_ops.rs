//! MIDI clip painting, take commit and canonical project edit worker operations.
use super::super::*;
use std::ops::ControlFlow;

pub(super) fn handle(ctx: &mut WorkerDispatch<'_>, request: Request) -> ControlFlow<()> {
    match request {
        Request::PaintMidiClip(_) => {
            ctx.outcome.error = Some("clip paint target was not resolved".into());
        }

        Request::Edit(_)
            if ctx.outcome.failed_plugin_adoption
                && (!ctx.worker.events.resident_publication_enabled()
                    || (!ctx.worker.session.uses_ordinary_worker_publication()
                        && ctx.worker.resident_snapshot.is_none())) =>
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
                && !ctx.worker.session.uses_ordinary_worker_publication()
                && ctx.worker.resident_snapshot.is_none()
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
                && (ctx.worker.session.uses_ordinary_worker_publication()
                    || ctx.worker.resident_snapshot.is_some()) =>
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
    fn edit_dispatch_recognizes_duplicate_clip_command() {
        let project = Project::new(48_000).unwrap();
        assert_eq!(latest_timeline_content_end(&project), 0);
    }
}
