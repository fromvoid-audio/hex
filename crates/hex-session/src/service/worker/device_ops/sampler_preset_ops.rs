//! Sampler loading, asset import, sampler editing and device preset worker operations.
use super::super::*;
use std::ops::ControlFlow;

pub(super) fn handle(ctx: &mut WorkerDispatch<'_>, request: Request) -> ControlFlow<()> {
    match request {
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
                if ctx.worker.session.native_reservation.is_none() {
                    return Err(SessionError::Invalid(
                        "sampler requires explicit native document admission",
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
                    && (ctx.worker.session.uses_ordinary_worker_publication()
                        || ctx.worker.resident_snapshot.is_some())
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
                && ctx.worker.session.native_reservation.is_some() =>
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

        Request::SamplerEdit(mut edit)
            if ctx.worker.events.resident_publication_enabled()
                && (ctx.worker.session.uses_ordinary_worker_publication()
                    || ctx.worker.resident_snapshot.is_some()) =>
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

        _ => {
            ctx.outcome.error = Some("unexpected worker dispatch route".into());
        }
    }
    ControlFlow::Continue(())
}

#[cfg(test)]
mod tests {
    use super::super::device_track_command;
    use super::*;

    #[test]
    fn device_track_command_selects_gain_variant_when_gain_provided() {
        let first =
            hex_engine::native_device_catalog_for_class(hex_engine::NativeDeviceClass::Effect)
                .unwrap()
                .into_iter()
                .next()
                .unwrap();
        let state = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            first.type_id,
        )
        .unwrap();
        let cmd = device_track_command(
            None,
            hex_project::TrackDeviceSlot::Effect,
            state,
            first.name,
            Some(0.5),
        );
        assert!(matches!(
            cmd,
            Command::InsertDefaultTrackWithDeviceAndGain { .. }
        ));
    }
}
