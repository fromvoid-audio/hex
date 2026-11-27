//! Request admission, recovery/publication/stopped fences, and gesture-barrier finalization.
use super::*;

pub(super) enum AdmissionStep {
    Admitted(Box<(Request, RequestExecution)>),
    Skip,
    Shutdown,
}

fn requires_recovery_flush(request: &Request, session: &Session) -> bool {
    match request {
        Request::SaveNew(_)
        | Request::SaveProjectAs { .. }
        | Request::SaveCurrent
        | Request::SaveCurrentWithFormatUpgrade { .. }
        | Request::RestartRecovery(_)
        | Request::ChangeProjectSampleRate { .. } => true,
        Request::Undo => project_rate::history_needs_replacement(session, false),
        Request::Redo => project_rate::history_needs_replacement(session, true),
        other => other.is_project_open(),
    }
}

impl WorkerLoop {
    pub(super) fn admit_and_fence_request(&mut self, queued: QueuedRequest) -> AdmissionStep {
        if matches!(&queued.request, Request::ContinueRecovery) {
            if !self.harvest_ready_recovery() {
                return AdmissionStep::Shutdown;
            }
            return AdmissionStep::Skip;
        }
        if self.core.recovery_tracker.async_ready() && !self.harvest_ready_recovery() {
            return AdmissionStep::Shutdown;
        }
        if requires_recovery_flush(&queued.request, &self.core.session)
            && self.core.recovery_tracker.has_pending()
            && !self.flush_pending_recovery()
        {
            return AdmissionStep::Shutdown;
        }
        let Self { events, core, .. } = self;
        let WorkerLoopCore {
            session,
            document_intent,
            revision,
            project_generation,
            resident_snapshot,
            resident_preview,
            device_gesture,
            pending_sampler_parameter,
            pending_plugin_effect,
            plugin_recording,
            recording_barrier,
            ..
        } = core;
        let revision = *revision;
        let project_generation = *project_generation;
        let mut queued = queued;
        if !plugin_recording.close_safe() && queued.request.is_project_open() {
            queued.recording_barrier = Some(recording_barrier::Completion::Failed(
                "finish or recover the vst3 automation take before replacing the project",
            ));
        }
        if plugin_recording.editor_active()
            && matches!(
                &queued.request,
                Request::SaveNew(_)
                    | Request::SaveProjectAs { .. }
                    | Request::SaveCurrent
                    | Request::SaveCurrentWithFormatUpgrade { .. }
                    | Request::Undo
                    | Request::Redo
                    | Request::Export(_)
                    | Request::ExportRange { .. }
                    | Request::DeviceGesture(_)
            )
        {
            queued.recording_barrier = Some(recording_barrier::Completion::Failed(
                "finish the plug-in editor automation take first",
            ));
        }
        let queued = recording_barrier.admit(
            queued,
            plugin_recording.terminal_for(device_gesture.snapshot()),
            hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision,
            },
            document_intent.load(Ordering::Acquire),
        );
        let clip_audition_release = resident_preview
            .as_ref()
            .and_then(resident_preview::Preview::clip_target)
            .is_some_and(|target| target.matches_release(&queued.request));
        // A move can arrive before its cancelled audition reaches the
        // callback. A scope or device-graph edit can also arrive before an
        // earlier graph is applied. Keep their FIFO place and bound the wait
        // before native preparation tries to borrow the preceding graph's sources.
        let queued = if events.resident_publication_enabled()
            && (session.uses_ordinary_worker_publication() || resident_snapshot.is_some())
            && (clip_audition_release
                || (matches!(&queued.request, Request::ContinuePluginEffect)
                    && pending_plugin_effect.as_ref().is_some_and(|pending| {
                        pending.matches_context(hex_engine::AudioSourceContext {
                            project_generation,
                            editor_revision: revision,
                        }) && !pending.insertion.control.is_cancelled()
                    }))
                || defers_behind_resident_publication(&queued.request))
        {
            match events.defer_publication(queued) {
                Some(queued) => queued,
                None => return AdmissionStep::Skip,
            }
        } else {
            queued
        };
        let publication_wait_failure = queued.publication_wait_failure;
        // Requests whose handler takes the stopped Arrangement lease first begin
        // it here; a live callback that has not answered parks the request and
        // returns this worker to its loop (see `stopped_arrangement`, #4544).
        let fence_recovery = if events.resident_publication_enabled()
            && session.audio_mode() == DocumentAudioMode::Resident
        {
            match &queued.request {
                Request::EditArrangementSection {
                    project_generation: captured_generation,
                    revision: captured_revision,
                    command,
                } if *captured_generation == project_generation
                    && *captured_revision == revision
                    && arrangement_sections::is_command(command)
                    && command.requires_section_publication() =>
                {
                    Some(true)
                }
                Request::Undo | Request::Redo => {
                    let redo = matches!(&queued.request, Request::Redo);
                    if project_rate::history_needs_replacement(session, redo) {
                        None
                    } else if session.editor.next_history_requires_stopped_bounce(redo) {
                        Some(false)
                    } else if session.section_history_requires_publication(redo) {
                        Some(true)
                    } else {
                        None
                    }
                }
                _ => None,
            }
        } else {
            None
        };
        let queued = if let Some(recover) = fence_recovery {
            let source = hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision,
            };
            match events.defer_stopped_fence(queued, source, recover) {
                Some(queued) => queued,
                None => return AdmissionStep::Skip,
            }
        } else {
            queued
        };
        events.begin_diagnostics(&queued, project_generation, revision);
        events.begin_request(
            document_intent,
            queued.document_intent,
            queued.publication_waited,
        );
        let request_document_intent = queued.document_intent;
        let save_correlation = queued.save_correlation;
        let defaults_directory = queued.native_device_defaults;
        let defaults_root = defaults_directory.as_deref().map(PathBuf::as_path);
        let recording_barrier_completion = queued.recording_barrier;
        let (request, import_decode) = truncated_import::admit(queued.request);
        let opening_project = request.is_project_open();
        let retiring_editor_replacement_failure = if opening_project {
            events
                .sampler_edit_audio()
                .and_then(|sink| sink.retiring_editor_close_blocker())
                .map(|blocker| match blocker {
                    RetiringEditorCloseBlocker::Transferring => {
                        "vst3 editor changes are still transferring · retry opening the project"
                    }
                    RetiringEditorCloseBlocker::Unresolved => {
                        "resolve the retiring vst3 editor changes before replacing the project"
                    }
                })
        } else {
            None
        };
        let mut device_insert_proposal = None;
        let mut repeated_device_insert = false;
        let device_insert_proposal_accepted = false;
        let loading_track_catalog = matches!(&request, Request::LoadTrackCatalogDevice { .. });
        let replacing_catalog_instrument = matches!(
            &request,
            Request::LoadTrackCatalogDevice {
                placement: TrackCatalogPlacement::ReplaceInstrument(_),
                ..
            }
        );
        if let Some(control) = request.builtin_device_insert_control() {
            if control.claim() {
                device_insert_proposal = Some(Arc::clone(control));
            } else {
                repeated_device_insert = true;
            }
        }
        let track_switch_request =
            matches!(&request, Request::Edit(command) if is_track_switch(command));
        let midi_note_update = matches!(
            &request,
            Request::Edit(
                Command::UpdateMidiNote { .. }
                    | Command::UpdateMidiNotes { .. }
                    | Command::UpdateMidiNotesWithSourceExtent { .. }
                    | Command::UpdateMidiNotesWithSourcePrefix { .. }
            )
        );
        let time_range_copy = matches!(&request, Request::Edit(Command::CopyTimeRangeTo { .. }));
        let locator_add = matches!(
            &request,
            Request::Edit(Command::AddArrangementLocator { .. })
        );
        let arrangement_paste = matches!(
            &request,
            Request::PasteTimeRange { .. } | Request::InsertTimeRange { .. }
        );
        let section_copy = matches!(
            &request,
            Request::EditArrangementSection { command, .. } if is_section_copy(command)
        );
        let arrangement_duplicate = section_copy
            || matches!(
                &request,
                Request::DuplicateClips { .. } | Request::DuplicateAliasClips { .. }
            );
        let seek_intent = match &request {
            Request::ResidentArrangementSeek(intent) => Some(intent.clone()),
            _ => None,
        };
        let seek_submitted = false;
        let request_complete = !matches!(
            &request,
            Request::ContinueSamplerEdit
                | Request::ContinuePluginEffect
                | Request::ContinueDeviceInsert
                | Request::ContinuePluginEditor
                | Request::ContinuePluginEditorRecording { .. }
                | Request::ContinueRetiringEditorReplay
                | Request::ContinuePluginParameters
                | Request::ContinuePluginParameterConversion
                | Request::ContinuePluginStateRestore
                | Request::ContinueStoppedPlaybackRestore
        );
        let mut plugin_effect_complete = Vec::new();
        if publication_wait_failure.is_some()
            && matches!(&request, Request::ContinuePluginEffect)
            && let Some(pending) = pending_plugin_effect.take()
        {
            plugin_effect_complete.push(pending.finish(events.sampler_edit_audio(), false));
        }
        let device_insert_confirmation = None;
        let device_insert_complete = None;
        let plugin_parameter_observation = request.is_read_only_observation();
        let plugin_parameters = None;
        let plugin_parameter_conversion = None;
        let mut sampler_edit_complete = None;
        if opening_project
            && retiring_editor_replacement_failure.is_none()
            && recording_barrier::failure(recording_barrier_completion).is_none()
            && let Some(gain) = pending_sampler_parameter.take()
        {
            gain.cancel().store(true, Ordering::Release);
            sampler_edit_complete = Some(gain.request_id());
            if let Some(audio) = events.sampler_edit_audio() {
                audio.cancel_sampler_edit();
            }
        }
        let midi_take_complete = match &request {
            Request::CommitMidiTake { request_id, .. } => Some(MidiTakeCompletion {
                request_id: *request_id,
                accepted: false,
            }),
            _ => None,
        };
        let computer_keyboard_complete = None;
        let tuning_audition_complete = None;
        let live_expression_complete = None;
        let computer_keyboard_playback = None;
        let tuning_input_publication = None;
        let bounce_complete = match &request {
            Request::BounceArrangement { control, .. } => Some(control.clone()),
            _ => None,
        };
        let audio_batch_complete = None;
        let audio_batch_failure = None;
        let rate_transition = match &request {
            Request::ChangeProjectSampleRate { choice, .. } => {
                Some(project_rate::Transition::Apply(*choice))
            }
            Request::Undo | Request::Redo => {
                let redo = matches!(&request, Request::Redo);
                project_rate::history_needs_replacement(session, redo)
                    .then_some(project_rate::Transition::History { redo })
            }
            _ => None,
        };
        let open_complete = false;
        let save_complete = false;
        let save_confirmation_required = false;
        let project_replaced = false;
        let replacement_revision = None;
        let installed_verified_source = None;
        let replacement_sources: Option<
            Option<hex_engine::native_device_resources::ControlSamplerSources>,
        > = None;
        let comparison_changed = false;
        let created = None;
        let revision_before_request = revision;
        let duplicate_complete = false;
        let restore_complete = None;
        let stopped_playback_restore_complete = false;
        let restore_prepared = None;
        let status = None;
        let error = None;
        let resident_edit_source_unavailable = None;
        let export_completion = None;
        let export_job = None;
        let gesture_ack = None;
        let tuning_gesture_ack = None;
        let tuning_gesture_request = false;
        let tuning_library = None;
        let gesture_protocol_ack = false;
        let gesture_request = false;
        let preview_prepared = None;
        let preview_project = None;
        // Resolve source-bound UI edits on the worker before any gesture barrier. Invalid identities
        // and requests during a live draft cannot commit unrelated gesture history.
        // A valid request then uses the exact generic Edit publication path below,
        // including Created selection and keyboard/tuning preparation.
        let cut_clipboard = None;
        let cut_device_clipboard = None;
        let _ = defaults_root;
        AdmissionStep::Admitted(Box::new((
            request,
            RequestExecution {
                meta: RequestMeta {
                    save_correlation,
                    defaults_directory,
                    recording_barrier_completion,
                    publication_wait_failure,
                    retiring_editor_replacement_failure,
                    repeated_device_insert,
                    loading_track_catalog,
                    track_switch_request,
                    midi_note_update,
                    locator_add,
                    arrangement_paste,
                    arrangement_duplicate,
                    seek_intent,
                    plugin_parameter_observation,
                    bounce_complete,
                    import_decode,
                    device_automation_take_complete: None,
                    adopted_device_draft: None,
                    failed_plugin_adoption: None,
                    comparison_binding: None,
                    sampler_load_complete: None,
                },
                outcome: RequestOutcome {
                    request_document_intent,
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
                    failed_plugin_adoption: false,
                    failed_plugin_publication_confirmed: false,
                    private_sampler: false,
                    pending_unit_originals: Vec::new(),
                },
            },
        )))
    }

    pub(super) fn preflight_and_finalize_gesture_barrier(
        &mut self,
        request: &Request,
        resolution_failure: Option<String>,
        exec: &mut RequestExecution,
    ) -> Option<String> {
        let Self { events, core, .. } = self;
        let WorkerLoopCore {
            session,
            export_lane,
            resident_seek,
            revision,
            unsaved,
            document,
            midi_preview,
            project_generation,
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
            pending_sampler_parameter,
            pending_device_insert,
            plugin_recording,
            ..
        } = core;
        let project_generation = *project_generation;
        let mut revision = *revision;
        let mut unsaved = *unsaved;
        let save_correlation = exec.meta.save_correlation;
        let recording_barrier_completion = exec.meta.recording_barrier_completion;
        let publication_wait_failure = exec.meta.publication_wait_failure;
        let retiring_editor_replacement_failure = exec.meta.retiring_editor_replacement_failure;
        let mut gesture_ack = exec.outcome.gesture_ack.take();
        let mut tuning_gesture_ack = exec.outcome.tuning_gesture_ack.take();
        let mut gesture_protocol_ack = exec.outcome.gesture_protocol_ack;
        let mut restore_prepared = exec.outcome.restore_prepared.take();
        let mut tuning_input_publication = exec.outcome.tuning_input_publication.take();
        let barrier = request.finalizes_parameter_gesture();
        // Reject stale comparison ownership before a barrier could commit an
        // unrelated live draft. Valid requests still use the existing barrier.
        let mut barrier_failure = resolution_failure.or_else(|| match &request {
            Request::InsertProjectSample {
                project_generation: captured,
                revision: captured_revision,
                ..
            } => {
                if *captured != project_generation || *captured_revision != revision {
                    Some("sample insertion canceled · project changed".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Some("finish or cancel the active gesture before inserting a sample".into())
                } else {
                    None
                }
            }
            Request::BounceArrangement {
                project_generation: captured,
                revision: captured_revision,
                control,
                ..
            } => {
                if *captured != project_generation
                    || *captured_revision != revision
                    || control.is_cancelled()
                {
                    Some("bounce cancelled · target changed".into())
                } else if compiled
                    .as_ref()
                    .is_some_and(|(_, p)| p.gain_recording_track().is_some())
                {
                    Some("finish gain recording before bouncing".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Some("finish or cancel the active gesture before bouncing".into())
                } else {
                    None
                }
            }
            Request::EditArrangementSection {
                project_generation: captured,
                revision: captured_revision,
                command,
            } => {
                if *captured != project_generation || *captured_revision != revision {
                    Some("arrangement section target changed".into())
                } else if !arrangement_sections::is_command(command) {
                    Some("section request requires a structural marker command".into())
                } else if pending_sampler_parameter.is_some()
                    || device_gesture.snapshot().is_some()
                    || tuning_gesture.snapshot().is_some()
                {
                    Some("finish or cancel the active gesture before editing sections".into())
                } else {
                    None
                }
            }
            Request::SamplerEdit(SamplerEdit::Midi(edit)) => {
                if pending_sampler_parameter.is_some() {
                    Some("sampler edit is already preparing".into())
                } else if device_gesture.snapshot().is_some() || tuning_gesture.snapshot().is_some()
                {
                    Some("finish or cancel the active gesture before sampler midi editing".into())
                } else if edit.request_id == 0
                    || edit.project_generation != project_generation
                    || edit.revision != revision
                    || edit.cancel.load(Ordering::Acquire)
                {
                    Some("sampler midi target changed or edit was cancelled".into())
                } else {
                    None
                }
            }
            Request::SamplerEdit(SamplerEdit::History(edit)) => {
                if pending_sampler_parameter.is_some() {
                    Some("sampler edit is already preparing".into())
                } else if device_gesture.snapshot().is_some() || tuning_gesture.snapshot().is_some()
                {
                    Some("finish or cancel the active gesture before sampler history".into())
                } else {
                    let mut edit = SamplerEdit::History(edit.clone());
                    if events.resident_publication_enabled()
                        && session.audio_mode() == DocumentAudioMode::Resident
                    {
                        edit.validate_resident(session, project_generation, revision)
                            .err()
                    } else {
                        edit.validate(session, project_generation, revision).err()
                    }
                }
            }
            Request::CompareDevice {
                owner, expected, ..
            } => device_comparison::preflight(
                session,
                project_generation,
                revision,
                *owner,
                *expected,
            )
            .err(),
            Request::SaveDevicePreset { owner, .. }
            | Request::LoadDevicePreset { owner, .. }
            | Request::SaveDeviceDefault { owner, .. }
            | Request::SaveDeviceUnitPreset { owner, .. }
            | Request::SaveDeviceUnitDefault { owner, .. } => {
                preset::validate_owner(session, project_generation, revision, *owner).err()
            }
            Request::LoadFirstSampler {
                project_generation: target_generation,
                revision: target_revision,
                ..
            } if *target_generation != project_generation || *target_revision != revision => {
                Some("sampler load target changed".into())
            }
            Request::ConvertClip {
                project_generation: expected_generation,
                revision: expected_revision,
                ..
            } if *expected_generation != project_generation || *expected_revision != revision => {
                Some("clip conversion canceled · project changed".into())
            }
            Request::ChangeProjectSampleRate {
                project_generation: captured,
                revision: captured_revision,
                ..
            } if *captured != project_generation || *captured_revision != revision => {
                Some("project changed · sample rate request cancelled".into())
            }
            _ => None,
        });
        if barrier_failure.is_none() {
            barrier_failure = publication_wait_failure
                .or_else(|| recording_barrier::failure(recording_barrier_completion))
                .or(retiring_editor_replacement_failure)
                .map(str::to_owned);
        }
        if !recording_barrier::save_matches(
            save_correlation,
            recording_barrier_completion,
            hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision,
            },
        ) {
            barrier_failure = Some("save target changed · save cancelled".to_owned());
        }
        if barrier && barrier_failure.is_none() && plugin_recording.owns(device_gesture.snapshot())
        {
            // A recorded terminal has its own asynchronous durable transaction.
            // Do not send it through ordinary opaque/manual gesture finalization.
            barrier_failure = Some("finish or cancel the vst3 recording before this action".into());
        }
        if barrier
            && barrier_failure.is_none()
            && pending_device_insert.is_some()
            && (device_gesture.snapshot().is_some() || tuning_gesture.snapshot().is_some())
        {
            // Requests served while a staged graph edit is pending must never
            // run mutating gesture barriers underneath the staged edit.
            barrier_failure = Some("finish the pending device edit before saving".into());
        }
        if barrier && barrier_failure.is_none() {
            let device_finished = if events.resident_publication_enabled()
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
                device_gesture.finish_prepared(
                    session,
                    compiled.as_ref().map(|(_, artifact)| artifact),
                    project_generation,
                    |session, operation| {
                        let sink = events
                            .resident_audio()
                            .ok_or(SessionError::Invalid("native preview output unavailable"))?;
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
                )
            } else if session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                && audio_sink::requires_plugin_profile(session.project())
                && events.resident_audio().is_some()
            {
                let previous = compiled.as_ref().map(|(_, artifact)| artifact);
                device_gesture.finish_verified(
                    session,
                    previous,
                    project_generation,
                    |session, operation| {
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
                    },
                )
            } else {
                device_gesture.finish(
                    session,
                    compiled.as_ref().map(|(_, artifact)| artifact),
                    project_generation,
                )
            };
            if let Some(outcome) = device_finished {
                gesture_ack = Some(outcome.ack.clone());
                gesture_protocol_ack = true;
                if outcome.ack.accepted {
                    if let Some(prepared) = outcome.playback {
                        if events.resident_publication_enabled()
                            && session.audio_mode() == DocumentAudioMode::Resident
                            && !outcome.canonical_changed
                        {
                            *compiled = Some((revision, Arc::clone(&prepared)));
                        }
                        if outcome.canonical_changed {
                            revision = revision.saturating_add(1);
                            unsaved = true;
                            *midi_preview = Arc::clone(prepared.midi_preview());
                            *compiled = Some((revision, Arc::clone(&prepared)));
                            *state = view_state(
                                session,
                                Arc::clone(midi_preview),
                                revision,
                                project_generation,
                                unsaved,
                                document.as_ref().map(|handle| handle.path.as_path()),
                            );
                        }
                        restore_prepared = Some(prepared);
                    }
                } else {
                    barrier_failure = outcome.ack.reason;
                    restore_prepared = outcome.playback;
                }
            } else if let Some(outcome) = if events.resident_publication_enabled()
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
                tuning_gesture.finish_prepared(
                    session,
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
                            resident_snapshot,
                            resident_preview,
                            sink,
                            admission,
                        )
                    },
                )
            } else {
                tuning_gesture.finish_with(
                    session,
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
            } {
                tuning_gesture_ack = Some(outcome.ack.clone());
                gesture_protocol_ack = true;
                if outcome.ack.accepted {
                    if let Some(prepared) = outcome.playback {
                        if events.resident_publication_enabled()
                            && session.audio_mode() == DocumentAudioMode::Resident
                            && !outcome.canonical_changed
                        {
                            *compiled = Some((revision, Arc::clone(&prepared)));
                        }
                        if outcome.canonical_changed {
                            revision = revision.saturating_add(1);
                            unsaved = true;
                            *midi_preview = Arc::clone(prepared.midi_preview());
                            *compiled = Some((revision, Arc::clone(&prepared)));
                            *state = view_state(
                                session,
                                Arc::clone(midi_preview),
                                revision,
                                project_generation,
                                unsaved,
                                document.as_ref().map(|handle| handle.path.as_path()),
                            );
                        }
                        restore_prepared = Some(prepared);
                    }
                } else {
                    barrier_failure = outcome.ack.reason;
                    restore_prepared = outcome.playback;
                }
            }
        }
        self.core.revision = revision;
        self.core.unsaved = unsaved;
        exec.outcome.gesture_ack = gesture_ack;
        exec.outcome.tuning_gesture_ack = tuning_gesture_ack;
        exec.outcome.gesture_protocol_ack = gesture_protocol_ack;
        exec.outcome.restore_prepared = restore_prepared;
        exec.outcome.tuning_input_publication = tuning_input_publication;
        barrier_failure
    }
}
