//! Track ordering, clip grouping and clip/time-range clipboard worker operations.
use super::super::*;
use std::ops::ControlFlow;

pub(super) fn handle(ctx: &mut WorkerDispatch<'_>, request: Request) -> ControlFlow<()> {
    match request {
        Request::ReorderTracks { .. } | Request::ReparentTracks { .. } => {
            ctx.outcome.error = Some("track reorder target was not resolved".into());
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
    fn capture_clips_rejects_empty_selection_cleanly() {
        let project = Project::new(48_000).unwrap();
        assert!(project.capture_clips(&[]).is_err());
    }
}
