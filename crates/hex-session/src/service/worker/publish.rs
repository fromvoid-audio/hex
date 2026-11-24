//! Post-dispatch state publication, gesture rebase, recovery checkpointing, and outcome delivery.
use super::*;

struct PendingDelivery {
    outcome: audio_sink::PendingOutcome,
    seek_during_locator_recovery: bool,
    stage_recovery: Option<DeferredRecoveryKind>,
    defer_recovery: Option<DeferredRecoveryKind>,
    export_job: Option<ExportJob>,
    export_completion: Option<ExportEvent>,
}

impl WorkerLoop {
    pub(super) fn publish_and_deliver(&mut self, exec: RequestExecution) -> bool {
        let Some(pending) = self.finalize_request_and_build_outcome(exec) else {
            return false;
        };
        self.deliver_outcome_and_exports(pending)
    }

    fn finalize_request_and_build_outcome(
        &mut self,
        exec: RequestExecution,
    ) -> Option<PendingDelivery> {
        let RequestMeta {
            seek_during_locator_recovery,
            save_correlation,
            defaults_directory: _,
            recording_barrier_completion: _,
            publication_wait_failure: _,
            retiring_editor_replacement_failure: _,
            repeated_device_insert: _,
            loading_track_catalog: _,
            track_switch_request,
            midi_note_update,
            locator_add,
            arrangement_paste,
            section_copy,
            arrangement_duplicate,
            seek_intent,
            plugin_parameter_observation,
            bounce_complete,
            import_decode,
            mut device_automation_take_complete,
            adopted_device_draft,
            failed_plugin_adoption,
            comparison_binding,
            sampler_load_complete,
        } = exec.meta;
        let RequestOutcome {
            request_document_intent: _,
            device_insert_proposal,
            device_insert_proposal_accepted,
            replacing_catalog_instrument: _,
            time_range_copy,
            seek_submitted,
            request_complete,
            mut plugin_effect_complete,
            device_insert_confirmation,
            device_insert_complete,
            plugin_parameters,
            plugin_parameter_conversion,
            sampler_edit_complete,
            midi_take_complete,
            computer_keyboard_complete,
            tuning_audition_complete,
            live_expression_complete,
            mut computer_keyboard_playback,
            tuning_input_publication,
            audio_batch_complete,
            audio_batch_failure,
            rate_transition: _,
            open_complete,
            save_complete,
            save_confirmation_required,
            project_replaced,
            replacement_revision,
            installed_verified_source,
            replacement_sources,
            comparison_changed,
            mut created,
            revision_before_request,
            duplicate_complete,
            restore_complete,
            mut stopped_playback_restore_complete,
            mut restore_prepared,
            mut status,
            mut error,
            resident_edit_source_unavailable,
            mut export_completion,
            mut export_job,
            mut gesture_ack,
            mut tuning_gesture_ack,
            tuning_gesture_request,
            tuning_library,
            gesture_protocol_ack,
            gesture_request,
            mut preview_prepared,
            mut preview_project,
            cut_clipboard,
            cut_device_clipboard,
            failed_plugin_adoption: _,
            failed_plugin_publication_confirmed,
            private_sampler: _,
        } = exec.outcome;

        let (
            canonical_changed,
            canonical_prepared,
            committed_arrangement_duplicate,
            committed_arrangement_paste,
            committed_track_switch,
            eligible,
            stage_recovery,
        ) = {
            let Self {
                events,
                track_switch_views,
                arrangement_edit_views,
                core,
                ..
            } = self;
            let WorkerLoopCore {
                session,
                export_lane,
                recovery,
                resident_seek,
                revision: revision_ref,
                unsaved: unsaved_ref,
                range_clipboard,
                track_clipboard,
                device_clipboard,
                media_base,
                document,
                midi_preview,
                project_generation: project_generation_ref,
                state,
                keyboard_activation,
                keyboard_tuning_hint,
                audition_activation,
                compiled,
                current_sources,
                resident_snapshot,
                resident_preview,
                device_gesture,
                tuning_gesture,
                native_automation_take,
                plugin_recording,
                pending_recovery_outcome,
                ..
            } = core;
            let mut revision = *revision_ref;
            let unsaved = *unsaved_ref;
            let mut project_generation = *project_generation_ref;
            // Only successful canonical removal installs a cut. Failed preparation,
            // queue/publication admission or stale input retains the previous copy.
            if created.is_some()
                && let Some(captured) = cut_clipboard
            {
                *track_clipboard = Some(captured);
            }
            if created.is_some()
                && let Some(captured) = cut_device_clipboard
            {
                *device_clipboard = Some(captured);
            }
            if project_replaced {
                *device_clipboard = None;
                *track_clipboard = None;
                if let Some(input) = audition_activation.take() {
                    input.cancel();
                }
                *range_clipboard = None;
                device_gesture.reset();
                tuning_gesture.reset();
                *resident_preview = None;
                project_generation = project_generation.saturating_add(1);
            }
            if comparison_changed {
                // The only producer is the comparison-classified Editor transaction.
                // Keep the actual prepared owner and its exact preparation snapshot.
                let next = revision.saturating_add(1);
                if let Some(binding) = comparison_binding {
                    revision = next;
                    *state = view_state_bound(
                        session,
                        Arc::clone(midi_preview),
                        revision,
                        project_generation,
                        unsaved,
                        document.as_ref().map(|handle| handle.path.as_path()),
                        Some((Arc::clone(&binding.source), Some(binding))),
                    );
                    if let Some((compiled_revision, _)) = compiled {
                        *compiled_revision = revision;
                    }
                    if let Some((generation, keyboard_revision, _, _)) = keyboard_activation
                        && *generation == project_generation
                        && *keyboard_revision == revision_before_request
                    {
                        *keyboard_revision = revision;
                    }
                }
            }
            if events.resident_publication_enabled()
                && !project_replaced
                && replacement_revision.is_none()
                && let Some(snapshot) = resident_snapshot.as_ref()
            {
                // A no-op can accept a fresh audio revision without changing authored history.
                // Keep the canonical compiler owner aligned with that actual admitted snapshot;
                // retaining the superseded factory here would escape its resource inventory.
                *compiled = Some((revision, Arc::clone(&snapshot.prepared)));
            }
            let canonical_changed = replacement_revision.is_some();
            let canonical_prepared = if let Some(prepared) = replacement_revision {
                if resident_preview
                    .as_ref()
                    .is_some_and(|overlay| overlay.clip_target().is_some())
                    && resident_snapshot
                        .as_ref()
                        .is_some_and(|snapshot| Arc::ptr_eq(&snapshot.prepared, &prepared))
                {
                    *resident_preview = None;
                }
                if let Some(sources) = replacement_sources {
                    *current_sources = sources;
                } else if !prepared.requires_control_sampler_sources() {
                    *current_sources = None;
                }
                revision = revision.saturating_add(1);
                *midi_preview = Arc::clone(prepared.midi_preview());
                // The verified receipt and committed edit refer to the same staged
                // snapshot. Keep that identity for the next native admission.
                *state = events.measure_edit_view(|| {
                    view_state_bound(
                        session,
                        Arc::clone(midi_preview),
                        revision,
                        project_generation,
                        unsaved,
                        document.as_ref().map(|handle| handle.path.as_path()),
                        installed_verified_source.map(|source| (source, None)),
                    )
                });
                *compiled = Some((revision, Arc::clone(&prepared)));
                Some(prepared)
            } else {
                None
            };
            if let Some((playback, hint)) = tuning_input_publication {
                *keyboard_tuning_hint = hint;
                if keyboard_tuning_hint.is_none() {
                    *keyboard_activation = None;
                }
                if let Some((generation, prepared_revision, _, _)) = keyboard_activation {
                    *generation = project_generation;
                    *prepared_revision = revision;
                }
                computer_keyboard_playback = Some(playback);
            } else if project_replaced
                || canonical_changed
                || (!comparison_changed && revision != revision_before_request)
            {
                // Invalidate control-side inheritance immediately, but let the
                // actual audio publication cancel the audible lease. A candidate
                // can wait or be refused without silencing the installed graph.
                *keyboard_activation = None;
                *keyboard_tuning_hint = None;
            }
            // The canonical switch is committed. Its display can advance while a held
            // preview is rebuilt; the request terminal still owns recovery and the rebase.
            let committed_track_switch =
                track_switch_request && canonical_changed && error.is_none();
            if committed_track_switch {
                let mut tracks = Vec::new();
                if tracks
                    .try_reserve_exact(state.project.tracks().len())
                    .is_ok()
                {
                    tracks.extend(state.project.tracks().iter().map(|track| TrackSwitchState {
                        track: track.id(),
                        muted: track.muted(),
                        soloed: track.soloed(),
                    }));
                    track_switch_views.send(TrackSwitchViewObservation {
                        project_generation,
                        first_previous_revision: revision_before_request,
                        previous_revision: revision_before_request,
                        revision,
                        tracks,
                    });
                }
            }
            if canonical_changed && !project_replaced && device_gesture.snapshot().is_some() {
                let outcome = if events.resident_publication_enabled()
                    && session.audio_mode() == DocumentAudioMode::Resident
                {
                    // This path is permitted to retire a failed rebase only after the actual
                    // canonical snapshot accepted publication, never merely after a model edit.
                    let accepted = resident_snapshot
                        .as_ref()
                        .zip(compiled.as_ref())
                        .is_some_and(|(snapshot, (_, prepared))| {
                            Arc::ptr_eq(&snapshot.prepared, prepared)
                        });
                    if accepted {
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
                        let outcome = device_gesture.rebase_prepared(
                            session,
                            compiled.as_ref().map(|(_, artifact)| artifact),
                            &mut |session, operation| {
                                let sink = events.resident_audio().ok_or(SessionError::Invalid(
                                    "native preview output unavailable",
                                ))?;
                                if let crate::device_gesture::GesturePreparation::Plugin(
                                    operation,
                                ) = operation
                                {
                                    let mut recording = plugin_recording::Preparation {
                                        state: plugin_recording,
                                        lane: native_automation_take,
                                        document_base: media_base.as_deref(),
                                        recovery: recovery.as_deref(),
                                        may_defer: false,
                                        deferred: false,
                                    };
                                    return resident_preview::plugin_parameter::operation_using(
                                        session,
                                        operation,
                                        hex_engine::AudioSourceContext {
                                            project_generation,
                                            editor_revision: revision,
                                        },
                                        resident_snapshot,
                                        resident_preview,
                                        sink,
                                        admission,
                                        Some(&mut recording),
                                    );
                                }
                                resident_preview::device_operation(
                                    session,
                                    operation,
                                    hex_engine::AudioSourceContext {
                                        project_generation,
                                        editor_revision: revision,
                                    },
                                    resident_snapshot,
                                    resident_preview,
                                    sink,
                                    admission,
                                )
                            },
                        );
                        if outcome
                            .as_ref()
                            .is_some_and(|outcome| !outcome.ack.accepted)
                        {
                            *resident_preview = None;
                        }
                        outcome
                    } else {
                        None
                    }
                } else if session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                    && audio_sink::requires_plugin_profile(session.project())
                    && events.resident_audio().is_some()
                {
                    let previous = compiled.as_ref().map(|(_, artifact)| artifact);
                    device_gesture.rebase_prepared(session, previous, &mut |session, operation| {
                        let sink = events
                            .resident_audio()
                            .ok_or(SessionError::Invalid("native preview output unavailable"))?;
                        session.verified_device_gesture_operation(
                            operation,
                            previous,
                            hex_engine::AudioSourceContext {
                                project_generation,
                                editor_revision: revision,
                            },
                            sink,
                        )
                    })
                } else {
                    device_gesture.rebase(
                        session,
                        compiled.as_ref().map(|(_, artifact)| artifact),
                        project_generation,
                    )
                };
                if let Some(outcome) = outcome {
                    gesture_ack = Some(outcome.ack);
                    preview_prepared = outcome.playback;
                    preview_project = outcome.preview_project;
                    restore_prepared = preview_prepared
                        .is_none()
                        .then_some(restore_prepared)
                        .flatten();
                }
            } else if canonical_changed && !project_replaced && tuning_gesture.snapshot().is_some()
            {
                let outcome = if events.resident_publication_enabled()
                    && session.audio_mode() == DocumentAudioMode::Resident
                {
                    // This path is permitted to retire a failed rebase only after the actual
                    // canonical snapshot accepted publication, never merely after a model edit.
                    let accepted = resident_snapshot
                        .as_ref()
                        .zip(compiled.as_ref())
                        .is_some_and(|(snapshot, (_, prepared))| {
                            Arc::ptr_eq(&snapshot.prepared, prepared)
                        });
                    if accepted {
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
                        let outcome = tuning_gesture.rebase_prepared(
                            session,
                            compiled.as_ref().map(|(_, artifact)| artifact),
                            &mut |session, operation| {
                                let sink = events.resident_audio().ok_or(SessionError::Invalid(
                                    "native preview output unavailable",
                                ))?;
                                resident_preview::tuning_operation(
                                    session,
                                    operation,
                                    hex_engine::AudioSourceContext {
                                        project_generation,
                                        editor_revision: revision,
                                    },
                                    resident_snapshot,
                                    resident_preview,
                                    sink,
                                    admission,
                                )
                            },
                        );
                        if outcome
                            .as_ref()
                            .is_some_and(|outcome| !outcome.ack.accepted)
                        {
                            *resident_preview = None;
                        }
                        outcome
                    } else {
                        None
                    }
                } else {
                    tuning_gesture.rebase(session, compiled.as_ref().map(|(_, artifact)| artifact))
                };
                if let Some(outcome) = outcome {
                    tuning_gesture_ack = Some(outcome.ack);
                    preview_prepared = outcome.playback;
                    preview_project = outcome.preview_project;
                    restore_prepared = preview_prepared
                        .is_none()
                        .then_some(restore_prepared)
                        .flatten();
                }
            }
            let committed_arrangement_duplicate = created.filter(|created| {
                arrangement_duplicate
                    && canonical_changed
                    && error.is_none()
                    && matches!(
                        created,
                        Created::Clips { .. } | Created::ArrangementSection(_)
                    )
            });
            let committed_arrangement_paste =
                arrangement_paste && canonical_changed && error.is_none();
            let arrangement_edit_view = if committed_arrangement_paste {
                // Single-clip pastes use the same ordered selection receipt as a
                // batch. A generic clip creation would reselect after newer input.
                if let Some(Created::Clip(clip)) = created {
                    created = Some(Created::Clips {
                        first: clip,
                        last: clip,
                    });
                }
                Some(ArrangementEditViewKind::Paste)
            } else if time_range_copy && canonical_changed && error.is_none() {
                Some(ArrangementEditViewKind::TimeRangeCopy)
            } else if locator_add
                && canonical_changed
                && error.is_none()
                && let Some(Created::ArrangementLocator(id)) = created
            {
                Some(ArrangementEditViewKind::LocatorAdd(id))
            } else {
                committed_arrangement_duplicate.map(ArrangementEditViewKind::Duplicate)
            };
            if let Some(kind) = arrangement_edit_view {
                arrangement_edit_views.send(ArrangementEditViewObservation {
                    project_generation,
                    first_previous_revision: revision_before_request,
                    previous_revision: revision_before_request,
                    kind,
                    state: Arc::downgrade(state),
                });
            }
            let committed_recovery_kind = if committed_arrangement_paste {
                Some(DeferredRecoveryKind::Paste)
            } else if committed_track_switch {
                Some(DeferredRecoveryKind::TrackSwitch)
            } else if time_range_copy && canonical_changed && error.is_none() {
                Some(DeferredRecoveryKind::TimeRangeCopy)
            } else if section_copy && committed_arrangement_duplicate.is_some() {
                Some(DeferredRecoveryKind::SectionCopy)
            } else if locator_add
                && canonical_changed
                && error.is_none()
                && matches!(created, Some(Created::ArrangementLocator(_)))
            {
                Some(DeferredRecoveryKind::LocatorAdd)
            } else {
                None
            };
            let eligible =
                committed_recovery_kind.filter(|_| unsaved && revision != revision_before_request);
            let stage_recovery = eligible.filter(|kind| {
                pending_recovery_outcome
                    .as_ref()
                    .is_some_and(|(pending, _)| pending == kind)
            });
            *revision_ref = revision;
            *project_generation_ref = project_generation;
            (
                canonical_changed,
                canonical_prepared,
                committed_arrangement_duplicate,
                committed_arrangement_paste,
                committed_track_switch,
                eligible,
                stage_recovery,
            )
        };
        if self.core.pending_recovery_outcome.is_some()
            && stage_recovery.is_none()
            && !seek_during_locator_recovery
            && !self.finish_pending_recovery()
        {
            return None;
        }
        let Self { events, core, .. } = self;
        let WorkerLoopCore {
            session,
            document_intent,
            export_lane,
            recovery,
            resident_seek,
            revision,
            unsaved,
            media_base,
            document,
            journal,
            recovery_tracker,
            project_generation,
            state,
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
            pending_stopped_restore,
            native_automation_take,
            plugin_recording,
            recording_barrier,
            ..
        } = core;
        let revision = *revision;
        let unsaved = *unsaved;
        let project_generation = *project_generation;
        let mut defer_recovery = None;
        if stage_recovery.is_none()
            && !seek_during_locator_recovery
            && let Some(untitled_path) = recovery.as_deref()
        {
            let journal_result = if let Some(kind) = eligible {
                events.measure_recovery(kind.metric(), || {
                    recovery_tracker
                        .start_async(recovery::RecoveryContext {
                            session,
                            base: media_base.as_deref(),
                            document: document.as_ref(),
                            untitled: untitled_path,
                            journal,
                            generation: project_generation,
                        })
                        .map(|started| {
                            if started {
                                defer_recovery = Some(kind);
                            }
                            None
                        })
                })
            } else {
                let mut maintain_recovery = || {
                    recovery_tracker.maintain(
                        recovery::RecoveryContext {
                            session,
                            base: media_base.as_deref(),
                            document: document.as_ref(),
                            untitled: untitled_path,
                            journal,
                            generation: project_generation,
                        },
                        unsaved,
                        revision != revision_before_request,
                    )
                };
                if committed_arrangement_duplicate.is_some()
                    || (time_range_copy && canonical_changed && error.is_none())
                {
                    events.measure_recovery("duplicate_recovery_us", maintain_recovery)
                } else if committed_arrangement_paste {
                    events.measure_recovery("paste_recovery_us", maintain_recovery)
                } else if committed_track_switch {
                    events.measure_recovery("track_switch_recovery_us", maintain_recovery)
                } else if locator_add && canonical_changed && error.is_none() {
                    events.measure_recovery("locator_recovery_us", maintain_recovery)
                } else if midi_note_update && canonical_changed && error.is_none() {
                    // Worker-side journal maintenance only; excludes audio publication
                    // and client receipt. Refused or unchanged edits have no sample.
                    events.measure_recovery("midi_note_recovery_us", maintain_recovery)
                } else {
                    maintain_recovery()
                }
            };
            match journal_result {
                Ok(Some(_)) => {
                    status =
                        Some("previous recovery journal preserved beside its original slot".into());
                }
                Ok(None) => {}
                Err(reason) => {
                    let message = format!("automatic recovery: {reason}");
                    error = Some(match error {
                        Some(existing) => format!("{existing}; {message}"),
                        None => message,
                    });
                }
            }
        }
        if let Some(preview) = resident_preview.as_ref() {
            preview_project = Some(Arc::clone(&preview.project));
            preview_prepared = Some(Arc::clone(&preview.snapshot.prepared));
        }
        let device_preview = match (
            device_gesture.snapshot(),
            preview_project.clone(),
            preview_prepared.as_ref(),
        ) {
            (Some(snapshot), Some(project), Some(prepared)) => Some(DevicePreview {
                identity: resident_preview.as_ref().map_or(
                    DevicePreviewIdentity {
                        project_generation,
                        key: snapshot.key,
                        sequence: snapshot.sequence,
                        canonical_revision: revision,
                    },
                    |preview| {
                        preview.device_identity().unwrap_or(DevicePreviewIdentity {
                            project_generation,
                            key: snapshot.key,
                            sequence: snapshot.sequence,
                            canonical_revision: revision,
                        })
                    },
                ),
                state: preview_view_state(state, project.clone(), Arc::clone(prepared)),
            }),
            _ => None,
        };
        let tuning_preview = match (
            tuning_gesture.snapshot(),
            preview_project.as_ref(),
            preview_prepared.as_ref(),
        ) {
            (Some(snapshot), Some(project), Some(prepared)) => Some(TuningPreview {
                identity: resident_preview
                    .as_ref()
                    .and_then(resident_preview::Preview::tuning_identity)
                    .unwrap_or(TuningPreviewIdentity {
                        project_generation,
                        key: snapshot.key,
                        sequence: snapshot.sequence,
                        canonical_revision: revision,
                    }),
                state: preview_view_state(state, project.clone(), Arc::clone(prepared)),
            }),
            _ => None,
        };
        let playback_uses_preview = preview_prepared.is_some();
        // A canonical rebase must replace its canonical audition candidate.
        // Ordinary gesture publications already carry the prepared live-keyboard
        // input and tuning hint, so keep that specialized preview intact.
        let prefer_rebased_preview = canonical_changed && playback_uses_preview;
        if let Some(intent) = seek_intent.as_ref().filter(|_| !seek_submitted)
            && let Some(seek) = resident_seek.as_mut()
        {
            seek.reject(
                intent,
                error
                    .clone()
                    .unwrap_or_else(|| "native seek unavailable".into()),
            );
        }
        let mut playback = if events.resident_publication_enabled()
            || hex_engine::native_device_resources::requires_private_sampler_sources(
                session.project(),
            )
            || session.audio_mode() == DocumentAudioMode::VerifiedStreaming
        {
            None
        } else if computer_keyboard_playback.is_some() && !prefer_rebased_preview {
            computer_keyboard_playback
        } else if let Some(prepared) = preview_prepared {
            Some(native_playback(prepared))
        } else if let Some(prepared) = canonical_prepared {
            Some(native_playback(prepared))
        } else {
            restore_prepared.map(native_playback)
        };
        if let (Some(candidate), Some(input)) = (
            playback.as_mut(),
            audition_activation
                .as_ref()
                .filter(|input| !input.is_cancelled()),
        ) {
            let project = if playback_uses_preview {
                tuning_preview
                    .as_ref()
                    .map(|preview| preview.state.project.as_ref())
                    .unwrap_or_else(|| session.project())
            } else {
                session.project()
            };
            if let Err(reason) = candidate.prepare_tuning_audition(project, Arc::clone(input)) {
                error = Some(reason.to_string());
                playback = None; // keep the previously installed runtime audible
            }
        }
        if let Some(input) = live_expression.as_ref() {
            if input.project_generation != project_generation {
                input.cancel();
            }
            if let Some(playback) = playback.as_mut() {
                let source_project = if playback_uses_preview {
                    device_preview
                        .as_ref()
                        .map(|preview| preview.state.project.as_ref())
                        .or_else(|| {
                            tuning_preview
                                .as_ref()
                                .map(|preview| preview.state.project.as_ref())
                        })
                        .unwrap_or_else(|| session.project())
                } else {
                    session.project()
                };
                if playback
                    .prepare_live_expression(source_project, Arc::clone(input))
                    .is_err()
                {
                    input.cancel();
                }
            }
        }
        if let Some(job) = export_job.as_mut() {
            let streamed = job.streamed.is_some();
            let verified_only = session.audio_mode() == DocumentAudioMode::VerifiedStreaming;
            let needs_private = audio_sink::requires_plugin_profile(session.project());
            let result = if needs_private {
                let preflight = export_lane.preflight_private_job(job);
                let control = job
                    .controlled
                    .as_ref()
                    .map(|c| c.control.clone())
                    .unwrap_or_default();
                let cancellation = RequestCancellation::new(
                    Arc::clone(&session.shutdown),
                    Arc::clone(document_intent),
                    document_intent.load(Ordering::Acquire),
                )
                .with_bounce(&control);
                let context = hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                };
                preflight.and_then(|()| match events.resident_audio() {
                    Some(sink) => {
                        let prepared = {
                            let admission = export_lane.admission_with_resident_owners(
                                compiled.as_ref().map(|(_, prepared)| {
                                    hex_engine::native_device_resources::ControlSamplerSelection {
                                        sources: current_sources.as_ref(),
                                        prepared: Some(prepared),
                                    }
                                }),
                                resident_seek.as_ref(),
                                resident_preview.as_ref(),
                            );
                            resident_audio::prepare_private_export(
                                session,
                                context,
                                sink,
                                admission,
                                resident_snapshot.as_ref(),
                                &cancellation,
                                &control,
                            )
                        };
                        prepared.and_then(|(snapshot, owner)| {
                            job.prepared = Arc::clone(&snapshot.prepared);
                            job.foreign = Some(owner);
                            export_lane.prepare_private_job(
                                job,
                                session,
                                &snapshot,
                                resident_snapshot.as_ref(),
                                sink,
                                resident_seek.as_ref(),
                                resident_preview.as_ref(),
                            )
                        })
                    }
                    None => Err("private vst3 export is unavailable for this output".into()),
                })
            } else {
                export_lane.prepare_job(
                    job,
                    session,
                    current_sources.as_ref().filter(|_| !streamed),
                    resident_snapshot.as_ref().filter(|_| !verified_only).zip(
                        events
                            .resident_audio()
                            .map(|sink| &*sink as &dyn WorkerAudioSink),
                    ),
                    resident_seek.as_ref(),
                    resident_preview.as_ref(),
                )
            };
            if let Err(reason) = result {
                let failed = export_job.take();
                if let Some(job) = failed {
                    export_completion = Some(ExportEvent {
                        revision: job.revision,
                        path: job.path.clone(),
                        result: Err(reason.into()),
                    });
                }
            }
        }
        // A lease resolved for this request but not consumed is released here.
        events.finish_stopped_fence();
        if pending_plugin_effect.as_ref().is_some_and(|pending| {
            !pending.matches_context(hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision,
            }) || pending.insertion.control.is_cancelled()
                || pending_sampler_parameter.is_some()
                || device_gesture.snapshot().is_some_and(|gesture| {
                    pending
                        .independent_eq_gesture(session.project(), gesture.target)
                        .is_none()
                })
                || tuning_gesture.snapshot().is_some()
        }) && let Some(pending) = pending_plugin_effect.take()
        {
            plugin_effect_complete.push(pending.finish(events.sampler_edit_audio(), false));
        }
        if pending_stopped_restore.is_some_and(|captured| {
            captured
                != hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                }
                || device_gesture.snapshot().is_some()
                || tuning_gesture.snapshot().is_some()
        }) {
            *pending_stopped_restore = None;
            if let Some(sink) = events.resident_audio() {
                sink.cancel_stopped_playback_restore();
            }
            stopped_playback_restore_complete = true;
        }
        if !plugin_recording.pending() {
            // Recorded SDK input retains its original writer until the actual
            // source observation has drained its last point and interruption.
            native_automation_take.interrupt_if_source_changed(hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision,
            });
        }
        if let Some(completion) = device_automation_take_complete.as_mut() {
            if let Some((path, _)) = adopted_device_draft {
                let recovery_finish = failed_plugin_adoption.as_ref().map(|adoption| {
                    let publication = plugin_recording::classify_recovery_publication(
                        error.is_some(),
                        revision_before_request,
                        revision,
                        failed_plugin_publication_confirmed,
                    );
                    let result = plugin_recording::finish_failed_adoption(
                        plugin_recording,
                        native_automation_take,
                        adoption,
                        publication,
                    );
                    (publication, result)
                });
                if failed_plugin_adoption.is_some()
                    && failed_plugin_publication_confirmed
                    && revision != revision_before_request
                    && error.is_some()
                {
                    let reason = error
                        .as_deref()
                        .unwrap_or("recovery acknowledgement failed");
                    error = Some(format!(
                        "device automation take committed · recovery custody retained · {reason}"
                    ));
                    completion.progress =
                        DeviceAutomationTakeProgress::CommittedWithRetainedCustody(path);
                } else if error.is_none() && revision != revision_before_request {
                    if recovery_finish.as_ref().is_some_and(|(publication, _)| {
                        *publication != plugin_recording::RecoveryPublication::Committed
                    }) {
                        error = Some("device automation take committed · recovery custody retained · publication identity changed".into());
                        completion.progress =
                            DeviceAutomationTakeProgress::CommittedWithRetainedCustody(path);
                    } else if let Some((_, Err(reason))) = recovery_finish {
                        error = Some(format!(
                            "device automation take committed · recovery custody retained · {reason}"
                        ));
                        completion.progress =
                            DeviceAutomationTakeProgress::CommittedWithRetainedCustody(path);
                    } else {
                        completion.progress = DeviceAutomationTakeProgress::Committed;
                        status = Some("device automation take committed".into());
                    }
                } else {
                    let reason = error
                        .as_deref()
                        .unwrap_or("device automation take did not change the lane");
                    error = Some(native_automation_recording::retained_error(&path, reason));
                    completion.progress = DeviceAutomationTakeProgress::Retained(path);
                }
            } else if error.is_some() {
                completion.progress = DeviceAutomationTakeProgress::Refused;
            }
        }
        let import_truncation = import_decode.refused().cloned();
        if let (Some(message), Some(path)) = (&mut error, import_decode.recovered_path()) {
            *message = format!(
                "{message} · recovered source retained at {}",
                hex_storage::text::verbatim(path.display())
            );
        }
        if error.is_none()
            && let Some(note) = import_decode.recovery_note()
        {
            status = Some(match status {
                Some(status) => format!("{status} · {note}"),
                None => note,
            });
        }
        if let Some(ack) = gesture_ack.as_ref() {
            recording_barrier.complete(
                ack,
                hex_engine::AudioSourceContext {
                    project_generation,
                    editor_revision: revision,
                },
            );
        }
        let outcome = events.capture_outcome(Event {
            state: Arc::clone(state),
            playback,
            status,
            error,
            resident_edit_source_unavailable,
            recovery_failure: recovery_tracker.failure(),
            request_complete,
            sampler_load_complete,
            sampler_edit_complete,
            plugin_effect_complete,
            device_insert_proposal_complete: device_insert_proposal.map(|control| {
                PluginEffectCompletion::finish(control, device_insert_proposal_accepted)
            }),
            device_insert_confirmation,
            device_insert_complete,
            plugin_parameter_observation,
            plugin_parameters,
            plugin_parameter_conversion,
            computer_keyboard_complete,
            midi_take_complete,
            device_automation_take_complete,
            tuning_audition_complete,
            live_expression_complete,
            bounce_complete,
            audio_batch_complete,
            audio_batch_failure,
            import_truncation,
            save_complete,
            save_correlation: if save_complete {
                save_correlation
            } else {
                None
            },
            save_confirmation_required,
            document_format_upgrade_required: document
                .as_ref()
                .is_some_and(|handle| handle.token.requires_format_upgrade()),
            open_complete,
            project_replaced,
            stopped_playback_restore_complete,
            created,
            duplicate_complete,
            restore_complete,
            device_preview,
            gesture_ack,
            gesture_protocol_ack,
            gesture_request,
            active_device_gesture: device_gesture.snapshot(),
            tuning_preview,
            tuning_gesture_ack,
            tuning_gesture_request,
            active_tuning_gesture: tuning_gesture.snapshot(),
            tuning_library,
        });
        Some(PendingDelivery {
            outcome,
            seek_during_locator_recovery,
            stage_recovery,
            defer_recovery,
            export_job,
            export_completion,
        })
    }

    fn deliver_outcome_and_exports(&mut self, pending: PendingDelivery) -> bool {
        let PendingDelivery {
            mut outcome,
            seek_during_locator_recovery,
            stage_recovery,
            defer_recovery,
            export_job,
            export_completion,
        } = pending;
        if seek_during_locator_recovery {
            self.core.pending_locator_seek_outcome = Some(outcome);
        } else if let Some(kind) = stage_recovery {
            // Acknowledge this committed source before waiting for the older
            // checkpoint, while retaining both ordered terminals on control.
            let previous =
                self.core
                    .pending_recovery_outcome
                    .take()
                    .map(|(previous_kind, mut previous)| {
                        // The callback may apply the successor while the older
                        // checkpoint owns the journal. Keep its control-side
                        // fence progressing without acknowledging either outcome.
                        while self.core.recovery_tracker.async_waiting() {
                            self.events.poll();
                            std::thread::sleep(stopped_arrangement::FENCE_POLL);
                        }
                        let (elapsed, result) = self
                            .core
                            .recovery_tracker
                            .finish_async(&mut self.core.journal);
                        previous.finish_recovery(
                            previous_kind.metric(),
                            elapsed,
                            result,
                            self.core.recovery_tracker.failure(),
                        );
                        previous
                    });
            let started = std::time::Instant::now();
            let result = if let Some(untitled_path) = self.core.recovery.as_deref() {
                self.core
                    .recovery_tracker
                    .start_async(recovery::RecoveryContext {
                        session: &self.core.session,
                        base: self.core.media_base.as_deref(),
                        document: self.core.document.as_ref(),
                        untitled: untitled_path,
                        journal: &mut self.core.journal,
                        generation: self.core.project_generation,
                    })
            } else {
                Err("automatic recovery path is unavailable".into())
            };
            // Admit this already committed edit before delivering the older
            // terminal. If its consumer has closed, dropping the tracker joins
            // the admitted checkpoint instead of abandoning this edit.
            if let Some(previous) = previous
                && !self.deliver_outcome(previous)
            {
                return false;
            }
            match result {
                Ok(true) => self.core.pending_recovery_outcome = Some((kind, outcome)),
                Ok(false) => {
                    outcome.finish_recovery(
                        kind.metric(),
                        started.elapsed(),
                        Ok(None),
                        self.core.recovery_tracker.failure(),
                    );
                    if !self.deliver_outcome(outcome) {
                        return false;
                    }
                }
                Err(reason) => {
                    outcome.finish_recovery(
                        kind.metric(),
                        started.elapsed(),
                        Err(reason),
                        self.core.recovery_tracker.failure(),
                    );
                    if !self.deliver_outcome(outcome) {
                        return false;
                    }
                }
            }
        } else if let Some(kind) = defer_recovery {
            self.core.pending_recovery_outcome = Some((kind, outcome));
        } else if !self.deliver_outcome(outcome) {
            return false;
        }
        if let Some(job) = export_job
            && let Err(event) = self.core.export_lane.submit(job)
        {
            self.core
                .pending_export_events
                .push_and_flush(event, &self.export_events);
        }
        if let Some(event) = export_completion {
            self.core
                .pending_export_events
                .push_and_flush(event, &self.export_events);
        }
        self.core.pending_export_events.flush(&self.export_events);
        true
    }
}
