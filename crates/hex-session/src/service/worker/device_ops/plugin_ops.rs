//! External plugin hosting, editor, parameter query and state restoration worker operations.
use super::super::*;
use std::ops::ControlFlow;

pub(super) fn handle(ctx: &mut WorkerDispatch<'_>, request: Request) -> ControlFlow<()> {
    match request {
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
    ControlFlow::Continue(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_ops_route_predicate_matches_plugin_requests() {
        assert!(matches!(
            Request::ContinuePluginEditor,
            Request::ContinuePluginEditor
        ));
        assert!(matches!(
            Request::ContinuePluginStateRestore,
            Request::ContinuePluginStateRestore
        ));
    }
}
