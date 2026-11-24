//! Pre-dispatch refusal gates and domain routing across clip, device, document, and live-input handlers.
use super::*;
use std::ops::ControlFlow;

impl WorkerLoop {
    pub(super) fn dispatch_domain_request(
        self,
        request: Request,
        barrier_failure: Option<String>,
        mut exec: RequestExecution,
    ) -> (Self, RequestExecution, ControlFlow<()>) {
        let Self {
            mut events,
            mut requests,
            export_events,
            track_switch_views,
            arrangement_edit_views,
            clip_audition,
            worker_diagnostics,
            core,
        } = self;
        let WorkerLoopCore {
            mut session,
            history_capacity,
            mut document_intent,
            mut export_lane,
            mut recovery,
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
            clip_audition_lane,
            verified_clip_audition_lane,
            verified_gain_lane,
            verified_play_start_lane,
            mut device_gesture,
            mut tuning_gesture,
            mut pending_sampler_parameter,
            mut pending_plugin_effect,
            mut pending_device_insert,
            mut pending_stopped_restore,
            mut native_automation_take,
            mut plugin_recording,
            recording_barrier,
            pending_recovery_outcome,
            pending_locator_seek_outcome,
            pending_export_events,
        } = core;
        let save_correlation = exec.meta.save_correlation;
        let recording_barrier_completion = exec.meta.recording_barrier_completion;
        let repeated_device_insert = exec.meta.repeated_device_insert;
        let loading_track_catalog = exec.meta.loading_track_catalog;
        let defaults_root = exec
            .meta
            .defaults_directory
            .as_deref()
            .map(PathBuf::as_path);
        let mut import_decode = std::mem::take(&mut exec.meta.import_decode);
        let adopted_device_draft = exec.meta.adopted_device_draft.take();
        let failed_plugin_adoption = exec.meta.failed_plugin_adoption.take();
        let RequestOutcome {
            request_document_intent,
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
            mut rate_transition,
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
            failed_plugin_adoption: _,
            mut failed_plugin_publication_confirmed,
            private_sampler: _,
        } = exec.outcome;
        let mut dispatch_flow = ControlFlow::Continue(());
        let comparison_binding = match &request {
            Request::CompareDevice { .. } | Request::Undo | Request::Redo => {
                ComparisonAudioBinding::advance(
                    &state,
                    project_generation,
                    revision.saturating_add(1),
                    &session,
                )
            }
            _ => None,
        };
        let private_sampler = hex_engine::native_device_resources::requires_private_sampler_sources(
            session.project(),
        );
        let sampler_load_complete = match &request {
            Request::LoadFirstSampler {
                request_id,
                project_generation,
                target,
                ..
            } => Some(SamplerLoadCompletion {
                request_id: *request_id,
                project_generation: *project_generation,
                track: target.track(),
            }),
            _ => None,
        };
        let native_edit_refusal = !events.resident_publication_enabled()
            && export_lane
                .native_hold
                .current_context
                .is_some_and(|c| c.project_generation == project_generation)
            && matches!(
                &request,
                Request::AssignSampler { .. }
                    | Request::InsertProjectSample { .. }
                    | Request::LoadFirstSampler { .. }
                    | Request::Edit(_)
                    | Request::CommitMidiTake { .. }
                    | Request::EditArrangementSection { .. }
                    | Request::DeviceClipboard {
                        action: DeviceClipboardAction::Cut { .. }
                            | DeviceClipboardAction::Paste { .. }
                            | DeviceClipboardAction::Duplicate { .. },
                        ..
                    }
                    | Request::TrackClipboard {
                        action: TrackClipboardAction::Cut(_)
                            | TrackClipboardAction::Duplicate { .. }
                            | TrackClipboardAction::Paste { .. },
                        ..
                    }
                    | Request::ReorderTracks { .. }
                    | Request::ReparentTracks { .. }
                    | Request::MoveTrackDevices { .. }
                    | Request::ReorderDevices { .. }
                    | Request::MoveExistingClips { .. }
                    | Request::ImportDeviceAsset { .. }
            );
        let sampler_without_resident = private_sampler && !events.resident_publication_enabled();
        let sampler_refusal = sampler_without_resident
            && !matches!(
                &request,
                Request::RestartAudio { .. }
                    | Request::RestartRecovery(_)
                    | Request::SamplerEdit(_)
                    | Request::ContinueSamplerEdit
                    | Request::DeviceAutomationTake { .. }
                    | Request::QueryFailedPluginAutomation { .. }
                    | Request::InspectSealedAutomationRecovery { .. }
                    | Request::QueryDeviceAutomationCloseSafety { .. }
                    | Request::PreparePluginEditorClose { .. }
                    | Request::RecoverDeviceAutomationTake { .. }
                    | Request::InsertPluginEffect(_)
                    | Request::ContinuePluginEffect
                    | Request::ContinueDeviceInsert
                    | Request::TogglePluginEditor { .. }
                    | Request::ContinuePluginEditor
                    | Request::QueryPluginParameters(_)
                    | Request::ContinueRetiringEditorReplay
                    | Request::ContinuePluginParameters
                    | Request::QueryPluginParameterConversion(_)
                    | Request::ContinuePluginParameterConversion
                    | Request::ContinuePluginStateRestore
                    | Request::PrepareComputerKeyboard { .. }
                    | Request::AssignSampler { .. }
                    | Request::LoadFirstSampler { .. }
                    | Request::Edit(_)
                    | Request::CommitMidiTake { .. }
                    | Request::EditArrangementSection { .. }
                    | Request::TrackClipboard { .. }
                    | Request::DeviceClipboard { .. }
                    | Request::ReorderTracks { .. }
                    | Request::ReparentTracks { .. }
                    | Request::MoveTrackDevices { .. }
                    | Request::ReorderDevices { .. }
                    | Request::MoveExistingClips { .. }
                    | Request::ImportDeviceAsset { .. }
                    | Request::SaveNew(_)
                    | Request::SaveProjectAs { .. }
                    | Request::SaveCommittedParameterCopy { .. }
                    | Request::SaveCurrent
                    | Request::SaveCurrentWithFormatUpgrade { .. }
                    | Request::Export(_)
                    | Request::ExportRange { .. }
                    | Request::NewWithAudioMode { .. }
                    | Request::Open(_)
                    | Request::OpenWith { .. }
                    | Request::OpenWithAudioMode { .. }
            );
        if !recording_barrier::save_matches(
            save_correlation,
            recording_barrier_completion,
            hex_engine::AudioSourceContext {
                project_generation,
                editor_revision: revision,
            },
        ) {
            save_complete = true;
            error = Some("save target changed · save cancelled".into());
        } else if matches!(&request, Request::Edit(command) if arrangement_sections::is_command(command))
        {
            error = Some("section edits require a revision-bound section request".into());
        } else if repeated_device_insert {
            error = Some("device insertion request was already submitted".into());
        } else if device_insert_proposal
            .as_ref()
            .is_some_and(|control| control.is_cancelled())
        {
            status = Some("device insertion was cancelled".into());
        } else if native_edit_refusal {
            error = Some("sampler native playback requires a fresh output; close or open a project before changing its sources or configuration".into());
        } else if sampler_refusal || (loading_track_catalog && sampler_without_resident) {
            error=Some(if events.native_audio_requested() {"sampler supports computer keyboard, frame-zero arrangement play and stop; other history steps and runtime tools are unavailable"} else {"sampler supports private offline export; native playback, other history steps and runtime tools are unavailable"}.into());
            if let Request::PrepareComputerKeyboard { request_id, .. } = &request {
                computer_keyboard_complete = Some(*request_id);
            }
            if let Request::RestorePlayback { generation } = &request {
                restore_complete = Some(*generation);
            }
            if matches!(&request, Request::RestoreStoppedPlayback) {
                stopped_playback_restore_complete = true;
            }
            if matches!(
                &request,
                Request::SaveNew(_)
                    | Request::SaveProjectAs { .. }
                    | Request::SaveCurrent
                    | Request::SaveCurrentWithFormatUpgrade { .. }
            ) {
                save_complete = true;
            }
        } else if let Some(reason) = barrier_failure {
            if let Request::DeviceGesture(gesture) = &request {
                gesture_request = true;
                gesture_protocol_ack = true;
                gesture_ack = Some(GestureAck {
                    source_lost: false,
                    key: gesture.key(),
                    phase: gesture.phase(),
                    sequence: gesture.sequence(),
                    accepted: false,
                    preview_retained: matches!(
                        gesture.phase(),
                        crate::device_gesture::GesturePhase::Commit
                            | crate::device_gesture::GesturePhase::Cancel
                    ) && device_gesture
                        .snapshot()
                        .is_some_and(|active| active.key == gesture.key()),
                    reason: Some(reason.clone()),
                });
            }
            sampler_edit_complete = match &request {
                Request::SamplerEdit(edit) => Some(edit.request_id()),
                _ => None,
            };
            open_complete = request.is_project_open();
            if let Request::TuningLibrary { operation_id, .. } = &request {
                tuning_library = Some(TuningLibraryResult {
                    operation_id: *operation_id,
                    slots: tuning_library_directory()
                        .map(|directory| hex_storage::tuning_library::list(&directory))
                        .unwrap_or_default(),
                    selected_slot: None,
                    loaded_name: None,
                    error: Some(reason.clone()),
                });
            }
            if matches!(
                &request,
                Request::SaveNew(_)
                    | Request::SaveProjectAs { .. }
                    | Request::SaveCurrent
                    | Request::SaveCurrentWithFormatUpgrade { .. }
            ) {
                save_complete = true;
            }
            if let Request::Export(path) | Request::ExportRange { path, .. } = &request {
                export_completion = Some(ExportEvent {
                    revision,
                    path: path.clone(),
                    result: Err(reason.clone().into()),
                });
            }
            if let Request::SaveCommittedParameterCopy { reply, .. } = &request {
                let _ = reply.try_send(Err(reason.clone()));
            }
            if let Request::PreparePausedInputMonitor { reply, .. } = &request {
                let _ = reply.try_send(Err(reason.clone()));
            }
            error = Some(reason);
        } else {
            let mut ctx = WorkerDispatch {
                track_switch_request: exec.meta.track_switch_request,
                worker: WorkerState {
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
                    defaults_root,
                    import_decode,
                    adopted_device_draft: &adopted_device_draft,
                    comparison_binding: &comparison_binding,
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
                    failed_plugin_adoption: failed_plugin_adoption.is_some(),
                    failed_plugin_publication_confirmed,
                    private_sampler,
                },
            };
            let flow = match request {
                req @ (Request::ArmGainRecording { .. }
                | Request::BounceArrangement { .. }
                | Request::CommitMidiTake { .. }
                | Request::ConvertClip { .. }
                | Request::CopyArrangementClipToConsoleTrack { .. }
                | Request::CopyClips(..)
                | Request::CutClips(..)
                | Request::CopyTimeRange(..)
                | Request::CutTimeRange(..)
                | Request::DuplicateAliasClips { .. }
                | Request::DuplicateClips { .. }
                | Request::DuplicateSessionCell { .. }
                | Request::Edit(..)
                | Request::EditArrangementSection { .. }
                | Request::GainLane { .. }
                | Request::GroupAudioEvents { .. }
                | Request::GroupClips { .. }
                | Request::GroupTimeRange { .. }
                | Request::Import(..)
                | Request::ImportAudioBatch { .. }
                | Request::ImportToNewTrack { .. }
                | Request::ImportToNewTrackWithGain { .. }
                | Request::ImportToSession { .. }
                | Request::ImportToTrack { .. }
                | Request::ImportToTrackWithOverwrite { .. }
                | Request::InsertProjectSample { .. }
                | Request::InsertTimeRange { .. }
                | Request::MoveExistingClips { .. }
                | Request::PaintMidiClip(..)
                | Request::PasteTimeRange { .. }
                | Request::ReorderTracks { .. }
                | Request::ReparentTracks { .. }
                | Request::TrackClipboard { .. }) => clip_ops::handle(&mut ctx, req),
                req @ (Request::AddNativeDevice { .. }
                | Request::AppendSourceUtility { .. }
                | Request::ArmNativeParameterRecording { .. }
                | Request::AssignSampler { .. }
                | Request::BindUnassignedNativeSource { .. }
                | Request::ChooseNativeSourceReceiver { .. }
                | Request::CompareDevice { .. }
                | Request::ContinueDeviceInsert
                | Request::ContinuePluginEditor
                | Request::ContinuePluginEditorRecording { .. }
                | Request::ContinuePluginEffect
                | Request::ContinuePluginParameterConversion
                | Request::ContinuePluginParameters
                | Request::ContinuePluginStateRestore
                | Request::ContinueRetiringEditorReplay
                | Request::ContinueSamplerEdit
                | Request::CreateRetainedNativeSource { .. }
                | Request::DeleteRetainedSource { .. }
                | Request::DeviceAutomation { .. }
                | Request::DeviceAutomationControl { .. }
                | Request::DeviceAutomationTake { .. }
                | Request::DeviceClipboard { .. }
                | Request::DeviceGesture(..)
                | Request::DropBrowserDeviceOnArrangement { .. }
                | Request::DropNativeDevice { .. }
                | Request::ImportDeviceAsset { .. }
                | Request::InsertBrowserInstrumentTrack { .. }
                | Request::InsertEffectBusDevice { .. }
                | Request::InsertPluginEffect(..)
                | Request::InspectSealedAutomationRecovery { .. }
                | Request::LoadDevicePreset { .. }
                | Request::LoadFirstSampler { .. }
                | Request::LoadTrackCatalogDevice { .. }
                | Request::MoveSourceUtility { .. }
                | Request::MoveTrackDevices { .. }
                | Request::PreparePluginEditorClose { .. }
                | Request::QueryDeviceAutomationCloseSafety { .. }
                | Request::QueryFailedPluginAutomation { .. }
                | Request::QueryPluginParameterConversion(..)
                | Request::QueryPluginParameters(..)
                | Request::RebindNativeSource { .. }
                | Request::RecoverDeviceAutomationTake { .. }
                | Request::RemoveSourceUtility { .. }
                | Request::RenameRetainedSource { .. }
                | Request::ReorderDevices { .. }
                | Request::ReplaceDeviceModulation { .. }
                | Request::ReplaceNativeDevice { .. }
                | Request::SamplerEdit(..)
                | Request::SaveDeviceDefault { .. }
                | Request::SaveDevicePreset { .. }
                | Request::SaveDeviceUnitDefault { .. }
                | Request::SaveDeviceUnitPreset { .. }
                | Request::SetDeviceParameter { .. }
                | Request::StartPluginEditorRecording { .. }
                | Request::StopPluginEditorRecording { .. }
                | Request::TogglePluginEditor { .. }) => device_ops::handle(&mut ctx, req),
                req @ (Request::ChangeProjectSampleRate { .. }
                | Request::ContinueRecovery
                | Request::Export(..)
                | Request::ExportRange { .. }
                | Request::NewWithAudioMode { .. }
                | Request::Open(..)
                | Request::OpenWith { .. }
                | Request::OpenWithAudioMode { .. }
                | Request::RecoverTruncatedImport(..)
                | Request::Redo
                | Request::RelinkMedia(..)
                | Request::RestartRecovery(..)
                | Request::SaveCurrent
                | Request::SaveCurrentWithFormatUpgrade { .. }
                | Request::SaveCommittedParameterCopy { .. }
                | Request::SaveNew(..)
                | Request::SaveProjectAs { .. }
                | Request::TuningLibrary { .. }
                | Request::Undo) => document_ops::handle(&mut ctx, req),
                req @ (Request::ContinueStoppedPlaybackRestore
                | Request::PrepareComputerKeyboard { .. }
                | Request::PrepareLiveExpression { .. }
                | Request::PreparePausedInputMonitor { .. }
                | Request::PrepareTuningAudition { .. }
                | Request::RepublishPausedPlayback
                | Request::ResidentArrangementSeek(..)
                | Request::RestartAudio { .. }
                | Request::RestorePlayback { .. }
                | Request::RestoreStoppedPlayback
                | Request::TuningGesture(..)) => live_input::handle(&mut ctx, req),
            };
            session = ctx.worker.session;
            requests = ctx.worker.requests;
            document_intent = ctx.worker.document_intent;
            events = ctx.worker.events;
            export_lane = ctx.worker.export_lane;
            recovery = ctx.worker.recovery;
            resident_seek = ctx.worker.resident_seek;
            revision = ctx.worker.revision;
            unsaved = ctx.worker.unsaved;
            range_clipboard = ctx.worker.range_clipboard;
            track_clipboard = ctx.worker.track_clipboard;
            device_clipboard = ctx.worker.device_clipboard;
            media_base = ctx.worker.media_base;
            document = ctx.worker.document;
            journal = ctx.worker.journal;
            recovery_tracker = ctx.worker.recovery_tracker;
            midi_preview = ctx.worker.midi_preview;
            state = ctx.worker.state;
            keyboard_activation = ctx.worker.keyboard_activation;
            keyboard_tuning_hint = ctx.worker.keyboard_tuning_hint;
            audition_activation = ctx.worker.audition_activation;
            live_expression = ctx.worker.live_expression;
            compiled = ctx.worker.compiled;
            current_sources = ctx.worker.current_sources;
            resident_snapshot = ctx.worker.resident_snapshot;
            resident_preview = ctx.worker.resident_preview;
            device_gesture = ctx.worker.device_gesture;
            tuning_gesture = ctx.worker.tuning_gesture;
            pending_sampler_parameter = ctx.worker.pending_sampler_parameter;
            pending_plugin_effect = ctx.worker.pending_plugin_effect;
            pending_device_insert = ctx.worker.pending_device_insert;
            pending_stopped_restore = ctx.worker.pending_stopped_restore;
            native_automation_take = ctx.worker.native_automation_take;
            plugin_recording = ctx.worker.plugin_recording;
            import_decode = ctx.worker.import_decode;
            device_insert_proposal = ctx.outcome.device_insert_proposal;
            device_insert_proposal_accepted = ctx.outcome.device_insert_proposal_accepted;
            seek_submitted = ctx.outcome.seek_submitted;
            request_complete = ctx.outcome.request_complete;
            plugin_effect_complete = ctx.outcome.plugin_effect_complete;
            device_insert_confirmation = ctx.outcome.device_insert_confirmation;
            device_insert_complete = ctx.outcome.device_insert_complete;
            plugin_parameters = ctx.outcome.plugin_parameters;
            plugin_parameter_conversion = ctx.outcome.plugin_parameter_conversion;
            sampler_edit_complete = ctx.outcome.sampler_edit_complete;
            midi_take_complete = ctx.outcome.midi_take_complete;
            computer_keyboard_complete = ctx.outcome.computer_keyboard_complete;
            tuning_audition_complete = ctx.outcome.tuning_audition_complete;
            live_expression_complete = ctx.outcome.live_expression_complete;
            computer_keyboard_playback = ctx.outcome.computer_keyboard_playback;
            tuning_input_publication = ctx.outcome.tuning_input_publication;
            audio_batch_complete = ctx.outcome.audio_batch_complete;
            audio_batch_failure = ctx.outcome.audio_batch_failure;
            rate_transition = ctx.outcome.rate_transition;
            open_complete = ctx.outcome.open_complete;
            save_complete = ctx.outcome.save_complete;
            save_confirmation_required = ctx.outcome.save_confirmation_required;
            project_replaced = ctx.outcome.project_replaced;
            replacement_revision = ctx.outcome.replacement_revision;
            installed_verified_source = ctx.outcome.installed_verified_source;
            replacement_sources = ctx.outcome.replacement_sources;
            comparison_changed = ctx.outcome.comparison_changed;
            created = ctx.outcome.created;
            duplicate_complete = ctx.outcome.duplicate_complete;
            restore_complete = ctx.outcome.restore_complete;
            stopped_playback_restore_complete = ctx.outcome.stopped_playback_restore_complete;
            restore_prepared = ctx.outcome.restore_prepared;
            status = ctx.outcome.status;
            error = ctx.outcome.error;
            resident_edit_source_unavailable = ctx.outcome.resident_edit_source_unavailable;
            export_completion = ctx.outcome.export_completion;
            export_job = ctx.outcome.export_job;
            gesture_ack = ctx.outcome.gesture_ack;
            tuning_gesture_ack = ctx.outcome.tuning_gesture_ack;
            tuning_gesture_request = ctx.outcome.tuning_gesture_request;
            tuning_library = ctx.outcome.tuning_library;
            gesture_protocol_ack = ctx.outcome.gesture_protocol_ack;
            gesture_request = ctx.outcome.gesture_request;
            preview_prepared = ctx.outcome.preview_prepared;
            preview_project = ctx.outcome.preview_project;
            cut_clipboard = ctx.outcome.cut_clipboard;
            cut_device_clipboard = ctx.outcome.cut_device_clipboard;
            failed_plugin_publication_confirmed = ctx.outcome.failed_plugin_publication_confirmed;
            dispatch_flow = flow;
        }
        exec.meta.import_decode = import_decode;
        exec.meta.adopted_device_draft = adopted_device_draft;
        exec.meta.failed_plugin_adoption = failed_plugin_adoption;
        exec.meta.comparison_binding = comparison_binding;
        exec.meta.sampler_load_complete = sampler_load_complete;
        exec.outcome = RequestOutcome {
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
            failed_plugin_adoption: exec.meta.failed_plugin_adoption.is_some(),
            failed_plugin_publication_confirmed,
            private_sampler,
        };
        let worker = Self {
            events,
            requests,
            export_events,
            track_switch_views,
            arrangement_edit_views,
            clip_audition,
            worker_diagnostics,
            core: WorkerLoopCore {
                session,
                history_capacity,
                document_intent,
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
                clip_audition_lane,
                verified_clip_audition_lane,
                verified_gain_lane,
                verified_play_start_lane,
                device_gesture,
                tuning_gesture,
                pending_sampler_parameter,
                pending_plugin_effect,
                pending_device_insert,
                pending_stopped_restore,
                native_automation_take,
                plugin_recording,
                recording_barrier,
                pending_recovery_outcome,
                pending_locator_seek_outcome,
                pending_export_events,
            },
        };
        (worker, exec, dispatch_flow)
    }
}
