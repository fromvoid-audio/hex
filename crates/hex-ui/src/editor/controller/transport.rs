//! Request enqueueing, seek/zoom/pan, session launch, metronome, paused republish, latency rebuild and stop transport.
use super::*;
use crate::editor::TogglePlayTarget;

impl EditorView {
    pub(in crate::editor) fn enqueue(&mut self, request: Request) -> bool {
        if matches!(request, Request::Undo | Request::Redo) {
            self.cancel_midi_audition();
        }
        // Freeze import policy once, before this request can enter either queue.
        let request = match self.capture_audio_import_policy(request) {
            Ok(request) => request,
            Err(error) => {
                self.error = Some(error.into());
                return false;
            }
        };
        self.enqueue_with_clip_scope(request, false)
    }

    /// Submit an import whose producer already owns its warp policy.
    pub(in crate::editor) fn enqueue_captured_audio_import(&mut self, request: Request) -> bool {
        self.enqueue_with_clip_scope(request, false)
    }

    /// Source indices/coordinates deliberately belong to the displayed source.
    /// Whole-clip property controls must use ordinary enqueue instead.
    pub(in crate::editor) fn enqueue_focused_source_edit(
        &mut self,
        command: hex_project::Command,
    ) -> bool {
        self.enqueue_with_clip_scope(Request::Edit(command), true)
    }

    pub(super) fn enqueue_with_clip_scope(
        &mut self,
        request: Request,
        focused_source: bool,
    ) -> bool {
        let request = match self.resolve_clip_action_scope(request, focused_source) {
            Ok(request) => request,
            Err(error) => {
                self.error = Some(error.0.into());
                return false;
            }
        };
        let selection = self.created_note_selection_intent(&request);
        let pending_before = self.worker_requests_pending;
        let resume = self.sampler_edit_resume_frame(&request);
        if resume.is_some() {
            self.stop();
        }
        let accepted = self.enqueue_request(request, false);
        if let Some(frame) = resume {
            // A refused submission resumes at once; otherwise once it settles.
            self.sampler_edit_resume = Some(frame);
            if accepted {
                replace_status(
                    &mut self.status,
                    "applying sampler edit · playback will resume",
                );
            }
        }
        if accepted && let Some(mut selection) = selection {
            self.bind_created_selection_terminal(&mut selection);
            self.pending_created_note_selection = Some(selection);
            // Immediate submission happens inside enqueue_request, before this
            // intent is installed. Queued/tuning-barrier requests are stamped
            // only when their actual Service submission succeeds in tick.
            let queued = self
                .pending_requests
                .iter()
                .chain(self.device_gesture_barriers.iter())
                .any(|request| self.pending_time_copy_matches(request));
            if !queued && self.worker_requests_pending > pending_before {
                self.time_copy_submitted();
            }
        }
        accepted
    }

    pub(in crate::editor) fn enqueue_duplicate_continuation(&mut self, request: Request) -> bool {
        self.enqueue_request(request, true)
    }

    pub(super) fn mutates_clipboard_or_project(request: &Request) -> bool {
        // Copy only reads the project. Cut, paste, insert and duplicate can
        // change it and invalidate both local drafts and prepared seeks.
        matches!(
            request,
            Request::DeviceClipboard {
                action: DeviceClipboardAction::Cut { .. }
                    | DeviceClipboardAction::Paste { .. }
                    | DeviceClipboardAction::Duplicate { .. },
                ..
            } | Request::TrackClipboard {
                action: TrackClipboardAction::Cut(_)
                    | TrackClipboardAction::Duplicate { .. }
                    | TrackClipboardAction::Paste { .. },
                ..
            } | Request::CutClips(_)
                | Request::CutTimeRange(_)
                | Request::PasteTimeRange { .. }
                | Request::InsertTimeRange { .. }
        )
    }

    /// Apply the same gate before FIFO and device-barrier custody. Owned
    /// commits, document replacement, and read-only requests keep their
    /// separate admission and terminal contracts.
    pub(in crate::editor) fn admit_independent_edit_with_local_draft(
        &mut self,
        request: &Request,
    ) -> bool {
        // Independent edits can advance the canonical revision even when they
        // carry source stamps. Refuse them before taking worker custody.
        // Duplicate repeats submit a new edit after the prior terminal.
        let independent_edit = matches!(
            request,
            Request::Edit(_)
                | Request::BindUnassignedNativeSource { .. }
                | Request::RebindNativeSource { .. }
                | Request::ChooseNativeSourceReceiver { .. }
                | Request::AppendSourceUtility { .. }
                | Request::RemoveSourceUtility { .. }
                | Request::MoveSourceUtility { .. }
                | Request::DeleteRetainedSource { .. }
                | Request::CreateRetainedNativeSource { .. }
                | Request::RenameRetainedSource { .. }
                | Request::ChangeProjectSampleRate { .. }
                | Request::Import(_)
                | Request::ImportAudioBatch { .. }
                | Request::ImportToTrack { .. }
                | Request::ImportToTrackWithOverwrite { .. }
                | Request::ImportToNewTrack { .. }
                | Request::ImportToNewTrackWithGain { .. }
                | Request::ImportToSession { .. }
                | Request::ImportDeviceAsset { .. }
                | Request::LoadFirstSampler { .. }
                | Request::AssignSampler { .. }
                | Request::RecoverTruncatedImport(_)
                | Request::RelinkMedia(_)
                | Request::EditArrangementSection { .. }
                | Request::MoveExistingClips { .. }
                | Request::DuplicateClips { .. }
                | Request::DuplicateAliasClips { .. }
                | Request::DuplicateSessionCell { .. }
                | Request::GroupClips { .. }
                | Request::GroupTimeRange { .. }
                | Request::GroupAudioEvents { .. }
                | Request::CopyArrangementClipToConsoleTrack { .. }
                | Request::ReorderTracks { .. }
                | Request::ReparentTracks { .. }
                | Request::DropNativeDevice { .. }
                | Request::MoveTrackDevices { .. }
                | Request::ReorderDevices { .. }
                | Request::AddNativeDevice { .. }
                | Request::ReplaceNativeDevice { .. }
                | Request::DropBrowserDeviceOnArrangement { .. }
                | Request::InsertBrowserInstrumentTrack { .. }
                | Request::InsertEffectBusDevice { .. }
                | Request::LoadTrackCatalogDevice { .. }
                | Request::InsertPluginEffect(_)
                | Request::CompareDevice { .. }
                | Request::LoadDevicePreset { .. }
                | Request::SetDeviceParameter { .. }
                | Request::BounceArrangement { .. }
                | Request::ConvertClip { .. }
                | Request::Undo
                | Request::Redo
                | Request::TuningLibrary {
                    operation: TuningLibraryOperation::Load { .. }
                        | TuningLibraryOperation::Import { .. },
                    ..
                }
        ) || Self::mutates_clipboard_or_project(request);
        if independent_edit && self.gain_lane.draft.is_some() {
            self.error = Some("finish or cancel the gain point gesture before editing".into());
            return false;
        }
        if independent_edit && self.automation_view.devices.point_draft_active() {
            self.error =
                Some("finish or cancel the automation point gesture before editing".into());
            return false;
        }
        if independent_edit
            && (self.gain_lane.numeric_draft_active()
                || self.sampler_parameter.numeric_draft_active())
        {
            self.error = Some("finish or cancel the numeric draft before editing".into());
            return false;
        }
        true
    }

    /// Enqueue a request with any import policy already captured.
    pub(in crate::editor) fn enqueue_request(
        &mut self,
        request: Request,
        duplicate_continuation: bool,
    ) -> bool {
        self.wake_poll();
        if matches!(
            &request,
            Request::QueryPluginParameters(_) | Request::QueryPluginParameterConversion(_)
        ) {
            // The panel owns retryable read intent. A full/busy service must
            // not move observations into the edit queue or interrupt a gesture.
            return match self.submit_to_service(request) {
                Ok(()) => true,
                Err(submission::Refused {
                    error: SubmitError::Full(_) | SubmitError::Busy(_, _),
                    ..
                }) => false,
                Err(submission::Refused {
                    error: SubmitError::Closed(request),
                    kinds,
                }) => {
                    self.on_request_refused(kinds, &request, submission::Refusal::Closed);
                    false
                }
            };
        }
        if self.update_preparing() {
            self.error = Some("cancel update before editing".into());
            return false;
        }
        if matches!(
            &request,
            Request::Open(_)
                | Request::OpenWith { .. }
                | Request::NewWithAudioMode { .. }
                | Request::OpenWithAudioMode { .. }
        ) && let Some(control) = &self.bounce_control
        {
            control.cancel();
        }
        let open_save = self.project_open.permits_save()
            && matches!(
                &request,
                Request::SaveCurrent
                    | Request::SaveNew(_)
                    | Request::SaveProjectAs { .. }
                    | Request::SaveCurrentWithFormatUpgrade { .. }
            );
        // An already-started recovery must finish while a picker/confirmation
        // pauses user edits. Only the currently owned recovery token qualifies.
        let recovery_restore = matches!(&request, Request::RestorePlayback { generation }
            if self.pending_recovery_generation == Some(*generation)
                && self.recovery.generation() == Some(*generation));
        if (self.project_open.busy() || self.pending_open_path.is_some())
            && !open_save
            && !recovery_restore
        {
            self.error = Some("finish the current open action before editing".into());
            return false;
        }
        if self.native_automation_recorder.busy()
            && !self
                .native_automation_recorder
                .permits_original_editor_window(&request)
            && !matches!(
                &request,
                Request::DeviceAutomationTake { .. }
                    | Request::StartPluginEditorRecording { .. }
                    | Request::StopPluginEditorRecording { .. }
                    | Request::RestartAudio { .. }
                    | Request::RestartRecovery(_)
            )
        {
            self.error = Some("finish the automation touch before another project action".into());
            return false;
        }
        let own_midi_request = match &request {
            Request::CommitMidiTake { request_id, .. } => {
                self.midi_recorder.accepts_commit(*request_id)
            }
            Request::PrepareComputerKeyboard { .. } | Request::PrepareLiveExpression { .. } => {
                self.midi_recorder.preparing()
            }
            _ => false,
        };
        if self.midi_recorder.blocks_edits() && !own_midi_request {
            self.error = Some(
                "finish or explicitly discard the midi take before another project action".into(),
            );
            return false;
        }
        if self.audio_recorder.blocks_edits() {
            self.error = Some("finish the audio recording before another project action".into());
            return false;
        }
        let own_gain_take = self.pending_gain_take_matches(&request);
        if self.gain_recorder.blocks_edits() && !own_gain_take {
            self.error = Some(if self.gain_recorder.has_retained_take() {
                "retry or explicitly discard the retained gain take before another project action"
                    .into()
            } else {
                "finish or cancel gain recording before another project action".into()
            });
            return false;
        }
        if !self.admit_independent_edit_with_local_draft(&request) {
            return false;
        }
        // The exact sealed recording already owns its terminal. It shares
        // GainLane's request type but must finish through its existing receipt.
        if !own_gain_take
            && matches!(
                &request,
                Request::GainLane { .. }
                    | Request::DeviceAutomation { .. }
                    | Request::DeviceAutomationControl { .. }
                    | Request::SamplerEdit(_)
            )
        {
            let gain_conflict = (self.gain_lane.draft.is_some()
                || self.gain_lane.numeric_draft_active())
                && !self.pending_gain_matches(&request);
            let automation_conflict = self.automation_view.devices.point_draft_active()
                && !self.pending_device_automation_matches(&request);
            let sampler_conflict = self.sampler_parameter.numeric_draft_active();
            if gain_conflict || automation_conflict || sampler_conflict {
                self.error = Some("finish or cancel the local draft before editing".into());
                return false;
            }
        }
        #[cfg(test)]
        match &request {
            Request::CopyTimeRange(_) => self.last_time_request = Some(("copy", None)),
            Request::CutClips(_) | Request::CutTimeRange(_) => {
                self.last_time_request = Some(("cut", None))
            }
            Request::PasteTimeRange { track, frame } => {
                self.last_time_request = Some(("paste", Some((*track, *frame))))
            }
            Request::InsertTimeRange { track, frame } => {
                self.last_time_request = Some(("insert", Some((*track, *frame))))
            }
            _ => {}
        }
        #[cfg(test)]
        if let Request::DropBrowserDeviceOnArrangement {
            destination,
            new_track_gain,
            ..
        } = &request
        {
            self.last_enqueued_arrangement_device_drop = Some(*destination);
            self.last_enqueued_device_track_gain = Some(*new_track_gain);
        }
        #[cfg(test)]
        if let Request::ConvertClip {
            project_generation,
            revision,
            clip,
            to_audio,
        } = &request
        {
            self.last_enqueued_clip_conversion =
                Some((*project_generation, *revision, *clip, *to_audio));
        }
        #[cfg(test)]
        if let Request::Edit(command)
        | Request::EditArrangementSection { command, .. }
        | Request::MoveExistingClips { command, .. } = &request
        {
            self.last_enqueued_edit = Some(command.clone());
        }
        #[cfg(test)]
        if let Request::PaintMidiClip(intent) = &request {
            self.last_enqueued_edit = Some(hex_project::Command::PlaceMidiClipWithOverwrite {
                track: intent.track,
                start: intent.start,
                length: intent.length,
            });
        }
        #[cfg(test)]
        if let Request::AddNativeDevice {
            track,
            class,
            type_id,
            ..
        } = &request
        {
            self.last_enqueued_native_device_add = Some((*track, *class, **type_id));
        }
        #[cfg(test)]
        if let Request::ReplaceNativeDevice {
            device,
            class,
            type_id,
        } = &request
        {
            self.last_enqueued_native_device_replace = Some((*device, *class, **type_id));
        }
        #[cfg(test)]
        if let Request::LoadTrackCatalogDevice {
            track,
            class,
            type_id,
            placement,
            ..
        } = &request
        {
            match placement {
                hex_session::service::TrackCatalogPlacement::Insert { .. } => {
                    self.last_enqueued_native_device_add = Some((*track, *class, *type_id));
                }
                hex_session::service::TrackCatalogPlacement::ReplaceInstrument(device) => {
                    self.last_enqueued_native_device_replace = Some((*device, *class, *type_id));
                }
            }
        }
        #[cfg(test)]
        if let Request::DropNativeDevice {
            project_generation,
            revision,
            source,
            track,
            before,
            replace,
            ..
        } = &request
        {
            self.last_enqueued_native_device_drop = Some(EnqueuedNativeDeviceDrop {
                project_generation: *project_generation,
                revision: *revision,
                source: *source,
                track: *track,
                before: *before,
                replace: *replace,
            });
        }
        #[cfg(test)]
        if let Request::ImportAudioBatch {
            paths,
            destination,
            options,
            ..
        } = &request
            && let [path] = paths.as_slice()
        {
            match destination {
                hex_session::import_batch::AudioBatchDestination::Console(_) => {}
                hex_session::import_batch::AudioBatchDestination::Track { track, start } => {
                    self.last_enqueued_import_to_track =
                        Some((path.clone(), *track, *start, options.overwrite))
                }
                hex_session::import_batch::AudioBatchDestination::NewTracks {
                    before_track,
                    start,
                } => {
                    self.last_enqueued_import_to_new_track =
                        Some((path.clone(), *before_track, *start))
                }
            }
        }
        #[cfg(test)]
        match &request {
            Request::ImportToTrack { path, track, start } => {
                self.last_enqueued_import_to_track = Some((path.clone(), *track, *start, false));
            }
            Request::ImportToTrackWithOverwrite { path, track, start } => {
                self.last_enqueued_import_to_track = Some((path.clone(), *track, *start, true));
            }
            _ => {}
        }
        #[cfg(test)]
        if let Request::ImportToNewTrack {
            path,
            before_track,
            start,
        }
        | Request::ImportToNewTrackWithGain {
            path,
            before_track,
            start,
            ..
        } = &request
        {
            self.last_enqueued_import_to_new_track = Some((path.clone(), *before_track, *start));
        }
        self.midi_creation = None;
        if self.midi_stroke.is_some() {
            self.cancel_midi_stroke();
        }
        if request.is_project_open() {
            self.cancel_plugin_effect();
            self.retire_sampler_parameter();
            self.cancel_first_sampler_load(None);
            if let Some(control) = &self.audio_batch {
                control.request_cancel();
            }
        }
        if !request.is_project_open()
            && !allows_non_open_request(
                self.close_prompt_open,
                self.opening_project,
                duplicate_continuation,
            )
        {
            self.error = Some(if self.opening_project {
                "opening project · editing is paused".into()
            } else {
                "close confirmation open · editing is paused".into()
            });
            return false;
        }
        // An owned recovery continuation is not a new user intent: a selected
        // picker path may still be awaiting the next render's Open reconciliation.
        if !request.is_project_open() && !recovery_restore {
            self.pending_open_path = None;
        }
        // A released play-start marker can join its in-progress recovery.
        // It hasn't edited the project yet and must leave the recovered seek alive.
        if self.recovering_clip_play_start_request(&request) {
            return self.recover_before_graph_edit(
                crate::editor::recovery_state::DeferredEditKind::ClipPlayStart,
                request,
            );
        }
        if self.recovering_mixer_edit_request(&request) {
            return self.recover_before_graph_edit(
                crate::editor::recovery_state::DeferredEditKind::MixerControl,
                request,
            );
        }
        if matches!(
            &request,
            Request::DeviceGesture(_)
                | Request::BindUnassignedNativeSource { .. }
                | Request::RebindNativeSource { .. }
                | Request::ChooseNativeSourceReceiver { .. }
                | Request::AppendSourceUtility { .. }
                | Request::RemoveSourceUtility { .. }
                | Request::MoveSourceUtility { .. }
                | Request::DeleteRetainedSource { .. }
                | Request::CreateRetainedNativeSource { .. }
                | Request::RenameRetainedSource { .. }
                | Request::DropNativeDevice { .. }
                | Request::MoveTrackDevices { .. }
                | Request::ReorderDevices { .. }
                | Request::DropBrowserDeviceOnArrangement { .. }
                | Request::InsertBrowserInstrumentTrack { .. }
                | Request::InsertEffectBusDevice { .. }
                | Request::LoadTrackCatalogDevice { .. }
                | Request::InsertPluginEffect(_)
                | Request::AddNativeDevice { .. }
                | Request::ReplaceNativeDevice { .. }
                | Request::CompareDevice { .. }
                | Request::LoadDevicePreset { .. }
                | Request::SetDeviceParameter { .. }
                | Request::Import(_)
                | Request::LoadFirstSampler { .. }
                | Request::ImportAudioBatch { .. }
                | Request::ImportToTrack { .. }
                | Request::ImportToTrackWithOverwrite { .. }
                | Request::ImportToNewTrack { .. }
                | Request::ImportToNewTrackWithGain { .. }
                | Request::ImportToSession { .. }
                | Request::RecoverTruncatedImport(_)
                | Request::RelinkMedia(_)
                | Request::Open(_)
                | Request::OpenWith { .. }
                | Request::NewWithAudioMode { .. }
                | Request::OpenWithAudioMode { .. }
                | Request::GainLane { .. }
                | Request::DeviceAutomation { .. }
                | Request::DeviceAutomationControl { .. }
                | Request::EditArrangementSection { .. }
                | Request::ReorderTracks { .. }
                | Request::ReparentTracks { .. }
                | Request::MoveExistingClips { .. }
                | Request::DuplicateClips { .. }
                | Request::DuplicateAliasClips { .. }
                | Request::DuplicateSessionCell { .. }
                | Request::GroupClips { .. }
                | Request::GroupTimeRange { .. }
                | Request::GroupAudioEvents { .. }
                | Request::Edit(_)
                | Request::ChangeProjectSampleRate { .. }
                | Request::BounceArrangement { .. }
                | Request::ConvertClip { .. }
                | Request::Undo
                | Request::SamplerEdit(hex_session::service::SamplerEdit::History(_))
                | Request::SamplerEdit(hex_session::service::SamplerEdit::Midi(_))
                | Request::Redo
                | Request::RestorePlayback { .. }
                | Request::RestoreStoppedPlayback
        ) || Self::mutates_clipboard_or_project(&request)
        {
            self.invalidate_prepared_seek();
        }
        if request.finalizes_parameter_gesture() && self.device_gesture.snapshot().is_some() {
            return self.enqueue_device_barrier(request);
        }
        let mut request = Some(request);
        if let Some(accepted) = self.enqueue_after_device_terminal(&mut request) {
            return accepted;
        }
        let Some(request) = request else {
            return false;
        };
        if self.tuning.client.snapshot().is_some() && request.finalizes_parameter_gesture() {
            if self.pending_requests.len() + self.device_gesture_barriers.len()
                >= UI_REQUEST_CAPACITY
            {
                self.error = Some("request queue full · edit was not applied".into());
                return false;
            }
            if let Err(reason) = self.finalize_tuning_gesture() {
                self.tuning.error = Some(reason.into());
                return false;
            }
        }
        if self.tuning.client.has_pending_requests() {
            if self.pending_requests.len() + self.device_gesture_barriers.len()
                >= UI_REQUEST_CAPACITY
            {
                self.error = Some("request queue full · edit was not applied".into());
                return false;
            }
            self.pending_requests.push_back(request);
            return true;
        }
        if let Some(kind) = self.recoverable_graph_edit(&request) {
            return self.recover_before_graph_edit(kind, request);
        }
        if self.holds_stopped_vst3_requests() || !self.pending_requests.is_empty() {
            if self.pending_requests.len() + self.device_gesture_barriers.len()
                < UI_REQUEST_CAPACITY
            {
                self.pending_requests.push_back(request);
                return true;
            } else {
                self.error = Some("request queue full · edit was not applied".into());
                return false;
            }
        }
        let Err(refused) = self.submit_to_service(request) else {
            return true;
        };
        let (request, refusal) = match refused.error {
            SubmitError::Full(request)
                if self.pending_requests.len() + self.device_gesture_barriers.len()
                    < UI_REQUEST_CAPACITY =>
            {
                self.pending_requests.push_back(*request);
                return true;
            }
            SubmitError::Full(request) => (*request, submission::Refusal::QueueFull),
            SubmitError::Busy(request, reason) => (*request, submission::Refusal::Busy(reason)),
            SubmitError::Closed(request) => (*request, submission::Refusal::Closed),
        };
        self.on_request_refused(refused.kinds, &request, refusal);
        false
    }

    pub(in crate::editor) fn request_seek(&mut self, frame: u64) {
        self.playback_settings.pending = None;
        self.deferred_toggle_play = None;
        self.deferred_metadata_seek = None;
        self.wake_poll();
        self.latency_rebuild = None;
        if self.section_install_pending() {
            self.error = Some("applying edit · wait for audio".into());
            return;
        }
        if self.recovery.generation().is_some() {
            self.recovery.retarget(frame);
            if let Some((
                PlaybackAction::Transport(
                    PlaybackCommand::PlayFrom(position)
                    | PlaybackCommand::PlayFromWithNoteChase {
                        frame: position, ..
                    },
                ),
                _,
                _,
            )) = self.deferred_play.as_mut()
            {
                *position = frame;
            }
            self.offline_cursor = frame;
            self.status = "restarting audio · playback position updated".into();
            self.error = None;
            return;
        }
        if self.stopped_playback_restore.is_some() {
            self.error = Some("restoring stopped playback · wait before seeking".into());
            return;
        }
        if self.opening_project || self.close_prompt_open {
            self.error = Some(if self.opening_project {
                "opening project · seeking is paused".into()
            } else {
                "close confirmation open · seeking is paused".into()
            });
            return;
        }
        match self.current_seek_requires_preroll() {
            Some(true) => {
                let start_playing = self.audio.as_ref().is_some_and(|audio| {
                    self.desired_playing(
                        audio
                            .control
                            .status()
                            .is_some_and(|status| status.playing.load(Acquire)),
                    )
                });
                self.request_prepared_seek(frame, start_playing);
                return;
            }
            Some(false) => {}
            None if self.audio.is_none() => {}
            None if self.awaiting_applied_playback_metadata() => {
                self.deferred_metadata_seek = Some((frame, self.project_generation));
                self.status = "waiting for playback metadata · seek queued".into();
                clear_stale_playback_unavailable_error(&mut self.error);
                self.wake_poll();
                return;
            }
            None => {
                self.error = Some("playback source is unavailable · cannot seek safely".into());
                return;
            }
        }
        if self.seek_pending() {
            self.error = Some("seek pending · wait for the cursor".into());
            return;
        }
        let Some(audio) = self.audio.as_mut() else {
            self.error = Some("audio output is unavailable · cannot seek".into());
            return;
        };

        let command = PlaybackAction::Transport(PlaybackCommand::Seek(frame));
        if self.pending_audio_commands.is_empty() {
            match if let Some(trace) = &self.transport_trace {
                trace.attempt(audio, command)
            } else {
                audio.control.action_with_ticket(command)
            } {
                Ok(ticket) => {
                    self.pending_seek_ticket = Some(ticket);
                    clear_stale_playback_unavailable_error(&mut self.error);
                }
                Err(rejected) if self.pending_audio_commands.len() < UI_REQUEST_CAPACITY => {
                    self.pending_audio_commands
                        .push_back(PendingPlaybackAction {
                            action: rejected,
                            generation: self.project_generation,
                            revision: None,
                            intent_token: None,
                        });
                    self.status = "seek queued".into();
                }
                Err(_) => {
                    self.error = Some("playback command queue full · seek was not applied".into())
                }
            }
        } else if self.pending_audio_commands.len() < UI_REQUEST_CAPACITY {
            self.pending_audio_commands
                .push_back(PendingPlaybackAction {
                    action: command,
                    generation: self.project_generation,
                    revision: None,
                    intent_token: None,
                });
            self.status = "seek queued".into();
        } else {
            self.error = Some("playback command queue full · seek was not applied".into());
        }
    }

    pub(in crate::editor) fn seek_pending(&self) -> bool {
        if self.recovery.generation().is_some()
            || self.section_launch_waiting()
            || self.prepared_seek.is_some()
            || self.latched_seek_intent.is_some()
            || self.retired_seek_result.is_some()
        {
            return true;
        }
        let queued = self.pending_audio_commands.iter().any(|pending| {
            matches!(
                pending.action,
                PlaybackAction::Transport(
                    PlaybackCommand::Seek(_)
                        | PlaybackCommand::PlayFrom(_)
                        | PlaybackCommand::PlayFromWithNoteChase { .. }
                )
            )
        });
        let consumed = self.audio_status().map_or(0, |status| {
            audio_command_prefix(status, self.document_audio_mode)
        });
        if self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.control.has_action_outcomes())
        {
            queued || self.pending_seek_ticket.is_some()
        } else {
            seek_is_pending(self.pending_seek_ticket, consumed, queued)
        }
    }

    pub(in crate::editor) fn cancel_duplicate_chain(&mut self) {
        self.duplicate_intent.cancel_chain();
        self.cancel_time_duplicate_repeats();
    }

    pub(in crate::editor) fn zoom_timeline(&mut self, inward: bool) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let horizon = self.viewport_horizon(state.timeline_frames);
        let playhead = self.cursor_position();
        let viewport = self
            .visible_viewport(horizon)
            .zoom(horizon, playhead, inward);
        self.install_arranger_navigation(viewport);
    }

    pub(in crate::editor) fn zoom_timeline_wheel_at(
        &mut self,
        pointer_x: gpui::Pixels,
        timeline_left: f32,
        timeline_width: f32,
        delta_y: f32,
    ) -> bool {
        const PIXELS_PER_STEP: f32 = 80.0;
        const MAX_ACCUMULATED_PIXELS: f32 = PIXELS_PER_STEP * 4.0;
        if !delta_y.is_finite() || delta_y == 0.0 {
            return false;
        }
        self.zoom_wheel_accum = (self.zoom_wheel_accum + delta_y)
            .clamp(-MAX_ACCUMULATED_PIXELS, MAX_ACCUMULATED_PIXELS);
        let steps = (self.zoom_wheel_accum / PIXELS_PER_STEP).trunc() as i8;
        if steps == 0 {
            return false;
        }
        self.zoom_wheel_accum -= f32::from(steps) * PIXELS_PER_STEP;

        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let horizon = self.viewport_horizon(state.timeline_frames);
        let current = self.visible_viewport(horizon);
        let Some(anchor) = crate::editor::seek::pointer_to_frame(
            f32::from(pointer_x),
            timeline_left,
            timeline_width,
            current,
        ) else {
            return false;
        };
        let mut next = current;
        for _ in 0..steps.unsigned_abs() {
            next = next.zoom_at(horizon, anchor, steps < 0);
        }
        if next == current {
            return false;
        }
        self.install_arranger_navigation(next);
        true
    }

    pub(in crate::editor) fn fit_timeline(&mut self) {
        self.clear_retained_navigation();
        self.viewport = None;
    }

    pub(in crate::editor) fn pan_timeline(&mut self, right: bool) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let horizon = self.viewport_horizon(state.timeline_frames);
        let current = self.visible_viewport(horizon);
        let panned = current.pan_half(horizon, right);
        self.install_arranger_navigation(panned);
    }

    pub(in crate::editor) fn queue_audio_command(&mut self, command: PlaybackCommand) {
        self.queue_playback_action(PlaybackAction::Transport(command), None);
    }

    pub(in crate::editor) fn queue_session_command(
        &mut self,
        command: hex_engine::SessionPlaybackCommand,
        revision: u64,
    ) {
        self.playback_settings.pending = None;
        let Some(audio) = self.audio.as_ref() else {
            self.error = Some("session playback requires an audio output".into());
            return;
        };
        if self.recovery.generation().is_some()
            || self.stopped_playback_restore.is_some()
            || self.opening_project
            || self.close_prompt_open
            || self.edit_pending()
        {
            self.error = Some("session playback is paused while the project is changing".into());
            return;
        }
        if self.applied_project_revision() != Some(revision) {
            self.error =
                Some("session action paused · wait for the current project audio revision".into());
            return;
        }
        if audio
            .control
            .status()
            .and_then(|status| status.session_capability())
            != Some(hex_engine::SessionCapabilityStatus::Available)
        {
            self.error = Some(
                audio
                    .control
                    .status()
                    .and_then(|status| status.session_capability())
                    .and_then(hex_engine::SessionCapabilityStatus::message)
                    .unwrap_or("session playback is unavailable for this project")
                    .to_lowercase()
                    .into(),
            );
            return;
        }
        self.queue_playback_action(PlaybackAction::Session(command), Some(revision));
    }

    pub(in crate::editor) fn queue_session_launch_batch(
        &mut self,
        command: hex_engine::SessionPlaybackCommand,
        revision: u64,
    ) {
        self.queue_normal_start(crate::editor::playback_settings::Start::Session {
            command,
            revision,
        });
    }

    pub(in crate::editor) fn queue_session_launch_with_note_chase(
        &mut self,
        command: hex_engine::SessionPlaybackCommand,
        revision: u64,
        note_chase: hex_engine::NoteChase,
    ) {
        let command = match command {
            hex_engine::SessionPlaybackCommand::LaunchClip { clip, quantization } => {
                hex_engine::SessionPlaybackCommand::LaunchClipWithNoteChase {
                    clip,
                    quantization,
                    note_chase,
                }
            }
            hex_engine::SessionPlaybackCommand::LaunchScene {
                scene,
                quantization,
            } => hex_engine::SessionPlaybackCommand::LaunchSceneWithNoteChase {
                scene,
                quantization,
                note_chase,
            },
            command => command,
        };
        self.wake_poll();
        self.invalidate_prepared_seek();
        if !matches!(
            command,
            hex_engine::SessionPlaybackCommand::LaunchClip { .. }
                | hex_engine::SessionPlaybackCommand::LaunchClipWithNoteChase { .. }
                | hex_engine::SessionPlaybackCommand::LaunchScene { .. }
                | hex_engine::SessionPlaybackCommand::LaunchSceneWithNoteChase { .. }
        ) {
            self.error = Some("only session clips and scenes can start playback".into());
            return;
        }
        let Some(audio) = self.audio.as_ref() else {
            self.error = Some("session playback requires an audio output".into());
            return;
        };
        if self.recovery.generation().is_some()
            || self.stopped_playback_restore.is_some()
            || self.opening_project
            || self.close_prompt_open
            || self.edit_pending()
        {
            self.error = Some("session playback is paused while the project is changing".into());
            return;
        }
        if self.applied_project_revision() != Some(revision) {
            self.error =
                Some("session launch paused · wait for the current project audio revision".into());
            return;
        }
        if audio
            .control
            .status()
            .and_then(|status| status.session_capability())
            != Some(hex_engine::SessionCapabilityStatus::Available)
        {
            let message = audio
                .control
                .status()
                .and_then(|status| status.session_capability())
                .and_then(hex_engine::SessionCapabilityStatus::message)
                .unwrap_or("session playback is unavailable for this project")
                .to_lowercase();
            self.error = Some(message.into());
            return;
        }
        let token = match crate::editor::play_intent::prepare_admission(
            &mut self.play_intent,
            Some(true),
            self.pending_audio_commands.len(),
            UI_REQUEST_CAPACITY,
        ) {
            Ok(Some(token)) => token,
            Ok(None) => {
                self.error = Some("session launch intent could not be prepared".into());
                return;
            }
            Err(AdmissionError::QueueFull) => {
                self.error =
                    Some("playback command queue full · session launch was not queued".into());
                return;
            }
            Err(AdmissionError::TokenExhausted) => {
                self.report_intent_exhausted();
                return;
            }
        };
        let mut pending =
            crate::editor::session_grid::launch_action(command, revision, self.project_generation);
        pending.intent_token = Some(token);
        self.pending_audio_commands.push_back(pending);
        self.pending_session_ticket = None;
        self.play_intent.admit(token, true);
        self.status = "session launch requested · waiting for audio callback".into();
    }

    pub(in crate::editor) fn metronome_enabled(&self) -> bool {
        self.audio_status()
            .is_some_and(|status| status.metronome_enabled.load(Acquire))
    }

    pub(in crate::editor) fn metronome_pending(&self) -> bool {
        self.pending_metronome_ticket.is_some_and(|ticket| {
            if self
                .audio
                .as_ref()
                .is_some_and(|audio| audio.control.has_action_outcomes())
            {
                return true;
            }
            self.audio_status().is_some_and(|status| {
                audio_command_prefix(status, self.document_audio_mode) < ticket
            })
        }) || self
            .pending_audio_commands
            .iter()
            .any(|pending| matches!(pending.action, PlaybackAction::SetMetronome(_)))
    }

    pub(in crate::editor) fn toggle_metronome(&mut self) {
        if !self.metronome_pending() {
            self.queue_playback_action(
                PlaybackAction::SetMetronome(!self.metronome_enabled()),
                None,
            );
        }
    }

    /// Starts only on the source that actually accepted this committed copy.
    /// A full UI or audio queue refuses visibly rather than dropping the fence.
    pub(in crate::editor) fn queue_section_copy_play(
        &mut self,
        frame: u64,
        source: hex_audio::prepared::AppliedAudioSource,
    ) {
        self.queue_applied_source_play(frame, source, false);
    }

    pub(in crate::editor) fn queue_applied_source_play(
        &mut self,
        frame: u64,
        source: hex_audio::prepared::AppliedAudioSource,
        allow_playing: bool,
    ) {
        self.queue_normal_start(crate::editor::playback_settings::Start::Source {
            frame,
            source,
            allow_playing,
        });
    }

    pub(in crate::editor) fn queue_applied_source_play_with_note_chase(
        &mut self,
        frame: u64,
        source: hex_audio::prepared::AppliedAudioSource,
        allow_playing: bool,
        note_chase: hex_engine::NoteChase,
    ) {
        self.wake_poll();
        let playing = self
            .audio
            .as_ref()
            .and_then(|audio| audio.control.status())
            .is_some_and(|status| status.playing.load(Acquire));
        if (!allow_playing && self.desired_playing(playing))
            || (allow_playing && self.desired_playing(playing) != playing)
            || !self.pending_audio_commands.is_empty()
            || self.stopped_playback_restore.is_some()
        {
            self.error = Some("applying edit · wait for audio".into());
            return;
        }
        let token = match crate::editor::play_intent::prepare_admission(
            &mut self.play_intent,
            Some(true),
            0,
            UI_REQUEST_CAPACITY,
        ) {
            Ok(token) => token,
            Err(AdmissionError::QueueFull) => {
                self.error = Some("playback command queue full".into());
                return;
            }
            Err(AdmissionError::TokenExhausted) => {
                self.report_intent_exhausted();
                return;
            }
        };
        self.latency_rebuild = None;
        self.sampler_edit_resume = None;
        self.deferred_play = None;
        self.deferred_toggle_play = None;
        self.deferred_metadata_seek = None;
        self.cancel_section_launch_wait();
        self.invalidate_prepared_seek();
        let Some(ticket) = self.audio.as_mut().and_then(|audio| {
            audio
                .control
                .play_from_with_note_chase_source_with_ticket(frame, note_chase, source)
                .ok()
        }) else {
            self.error = Some("playback unavailable · retry".into());
            return;
        };
        let command = PlaybackCommand::PlayFromWithNoteChase { frame, note_chase };
        self.admit_play_intent(
            PendingPlaybackAction {
                action: PlaybackAction::Transport(command),
                generation: self.project_generation,
                revision: None,
                intent_token: token,
            },
            Some(true),
            Some(ticket),
        );
        self.pending_seek_ticket = Some(ticket);
        clear_stale_playback_unavailable_error(&mut self.error);
        #[cfg(test)]
        {
            self.last_transport_command = Some(command);
        }
    }

    /// Seek only if the callback still owns the observed source. Never place a
    /// rejected fenced action on the ordinary, generation-only UI queue.
    pub(in crate::editor) fn queue_applied_source_seek(
        &mut self,
        frame: u64,
        source: hex_audio::prepared::AppliedAudioSource,
    ) {
        self.playback_settings.pending = None;
        self.wake_poll();
        if !self.pending_audio_commands.is_empty()
            || self.stopped_playback_restore.is_some()
            || self.seek_pending()
        {
            self.error = Some("applying edit · wait for audio".into());
            return;
        }
        self.latency_rebuild = None;
        self.sampler_edit_resume = None;
        self.deferred_play = None;
        self.deferred_toggle_play = None;
        self.deferred_metadata_seek = None;
        self.cancel_section_launch_wait();
        self.invalidate_prepared_seek();
        let Some(ticket) = self.audio.as_mut().and_then(|audio| {
            audio
                .control
                .seek_from_source_with_ticket(frame, source)
                .ok()
        }) else {
            self.error = Some("playback unavailable · retry".into());
            return;
        };
        self.pending_seek_ticket = Some(ticket);
        clear_stale_playback_unavailable_error(&mut self.error);
        #[cfg(test)]
        {
            self.last_transport_command = Some(PlaybackCommand::Seek(frame));
        }
    }

    pub(super) fn queue_playback_action(&mut self, action: PlaybackAction, revision: Option<u64>) {
        if !matches!(action, PlaybackAction::SetMetronome(_)) {
            self.playback_settings.pending = None;
        }
        self.wake_poll();
        if !matches!(action, PlaybackAction::SetMetronome(_)) {
            self.latency_rebuild = None;
            self.sampler_edit_resume = None;
            self.deferred_play = None;
        }
        if self.section_install_pending()
            && (intent_desired_playing(action) == Some(true)
                || matches!(action, PlaybackAction::Transport(PlaybackCommand::Seek(_))))
        {
            self.error = Some("applying edit · wait for audio".into());
            return;
        }
        if intent_desired_playing(action) == Some(true)
            && (self.stopped_playback_restore.is_some()
                || self.stopped_plugin_parameter_terminal_pending())
        {
            self.deferred_play = Some((action, revision, self.project_generation));
            let message = if self.stopped_playback_restore.is_some() {
                "restoring stopped playback · playback starts when ready"
            } else {
                "applying vst3 parameter · playback starts when ready"
            };
            replace_status(&mut self.status, message);
            return;
        }
        if let PlaybackAction::Transport(
            PlaybackCommand::PlayFrom(frame) | PlaybackCommand::PlayFromWithNoteChase { frame, .. },
        ) = action
            && self.recover_degraded_play_at(frame)
        {
            if matches!(
                action,
                PlaybackAction::Transport(PlaybackCommand::PlayFromWithNoteChase { .. })
            ) && self.recovery.generation().is_some()
            {
                // Recovery already owns this start. Keep its explicit policy
                // in the existing generation-bound deferred action slot.
                self.deferred_play = Some((action, revision, self.project_generation));
            }
            return;
        }
        if !matches!(action, PlaybackAction::SetMetronome(_)) {
            self.cancel_section_launch_wait();
            self.invalidate_prepared_seek();
        }
        if self.audio.is_none() {
            self.error = Some("audio output is unavailable".into());
            return;
        }
        let desired_playing = intent_desired_playing(action);
        let intent_token = match crate::editor::play_intent::prepare_admission(
            &mut self.play_intent,
            desired_playing,
            self.pending_audio_commands.len(),
            UI_REQUEST_CAPACITY,
        ) {
            Ok(token) => token,
            Err(AdmissionError::QueueFull) => {
                self.error = Some("playback command queue full".into());
                return;
            }
            Err(AdmissionError::TokenExhausted) => {
                self.report_intent_exhausted();
                return;
            }
        };
        let pending = PendingPlaybackAction {
            action,
            generation: self.project_generation,
            revision,
            intent_token,
        };
        if self.pending_audio_commands.is_empty() {
            let result = self.audio.as_mut().map(|audio| {
                if let Some(trace) = &self.transport_trace {
                    trace.attempt(audio, action)
                } else {
                    audio.control.action_with_ticket(action)
                }
            });
            match result {
                Some(Ok(ticket)) => {
                    if matches!(action, PlaybackAction::SetMetronome(_)) {
                        self.pending_metronome_ticket = Some(ticket);
                    }
                    self.admit_play_intent(pending, desired_playing, Some(ticket));
                    if matches!(
                        action,
                        PlaybackAction::Transport(
                            PlaybackCommand::PlayFrom(_)
                                | PlaybackCommand::PlayFromWithNoteChase { .. }
                        )
                    ) {
                        self.pending_seek_ticket = Some(ticket);
                    }
                    if matches!(
                        action,
                        PlaybackAction::Session(_) | PlaybackAction::LaunchAndPlay(_)
                    ) {
                        self.pending_session_ticket = Some((ticket, self.project_generation));
                        replace_status(
                            &mut self.status,
                            "session request queued · waiting for audio callback",
                        );
                    }
                }
                Some(Err(rejected)) => {
                    if self.pending_audio_commands.len() < UI_REQUEST_CAPACITY {
                        self.pending_audio_commands
                            .push_back(PendingPlaybackAction {
                                action: rejected,
                                ..pending
                            });
                        if matches!(
                            rejected,
                            PlaybackAction::Session(_) | PlaybackAction::LaunchAndPlay(_)
                        ) {
                            self.pending_session_ticket = None;
                        }
                        self.admit_play_intent(pending, desired_playing, None);
                        if matches!(
                            rejected,
                            PlaybackAction::Transport(
                                PlaybackCommand::PlayFrom(_)
                                    | PlaybackCommand::PlayFromWithNoteChase { .. }
                            )
                        ) {
                            self.pending_seek_ticket = None;
                        }
                        if matches!(
                            rejected,
                            PlaybackAction::Session(_) | PlaybackAction::LaunchAndPlay(_)
                        ) {
                            replace_status(
                                &mut self.status,
                                "session request queued · waiting for audio callback",
                            );
                        }
                    } else {
                        self.error = Some("playback command queue full".into());
                    }
                }
                None => {
                    self.error = Some("audio output is unavailable".into());
                }
            }
        } else {
            self.pending_audio_commands.push_back(pending);
            if matches!(
                action,
                PlaybackAction::Transport(
                    PlaybackCommand::PlayFrom(_) | PlaybackCommand::PlayFromWithNoteChase { .. }
                )
            ) {
                self.pending_seek_ticket = None;
            }
            if matches!(
                action,
                PlaybackAction::Session(_) | PlaybackAction::LaunchAndPlay(_)
            ) {
                self.pending_session_ticket = None;
            }
            self.admit_play_intent(pending, desired_playing, None);
            if matches!(
                action,
                PlaybackAction::Session(_) | PlaybackAction::LaunchAndPlay(_)
            ) {
                replace_status(
                    &mut self.status,
                    "session request queued · waiting for audio callback",
                );
            }
        }
    }

    pub(super) fn admit_play_intent(
        &mut self,
        pending: PendingPlaybackAction,
        desired_playing: Option<bool>,
        ticket: Option<u64>,
    ) {
        if let (Some(token), Some(desired)) = (pending.intent_token, desired_playing) {
            self.play_intent.admit(token, desired);
            if let Some(ticket) = ticket {
                self.play_intent.attach_ticket(token, ticket);
            }
        }
    }

    pub(super) fn report_intent_exhausted(&mut self) {
        self.error = Some("playback intent tokens exhausted · restart hex to continue".into());
    }

    /// Rebuild only a current, rejected canonical graph. A fresh graph drops
    /// effect history, so retain the user's transport intent and never restart
    /// a superseded project, output or explicit transport action.
    /// A streamed document's sampler edit replaces its output only while
    /// stopped, so a playing edit stops first and resumes here at the same
    /// frame (audio restarts at the cursor). Resident documents apply live.
    pub(super) fn sampler_edit_resume_frame(&self, request: &Request) -> Option<u64> {
        if !matches!(request, Request::SamplerEdit(_))
            || self.document_audio_mode != hex_session::DocumentAudioMode::VerifiedStreaming
        {
            return None;
        }
        let status = self.audio.as_ref()?.control.status()?;
        status
            .playing
            .load(Acquire)
            .then(|| status.position.load(Acquire))
    }

    pub(in crate::editor) fn tick_sampler_edit_resume(&mut self) -> bool {
        let Some(frame) = self.sampler_edit_resume else {
            return false;
        };
        if self.edit_pending()
            || !self.pending_audio_commands.is_empty()
            || self.stopped_playback_restore.is_some()
            || self.audio.is_none()
            || self
                .state
                .as_ref()
                .is_none_or(|state| self.applied_project_revision != Some(state.revision))
        {
            return false;
        }
        self.sampler_edit_resume = None;
        self.queue_audio_command(PlaybackCommand::PlayFrom(frame));
        true
    }

    /// Starting the callback with a stopped VST3 terminal in flight can
    /// invalidate its transport fence before the original source commits.
    pub(super) fn stopped_plugin_parameter_terminal_pending(&self) -> bool {
        self.device_gesture.has_pending_terminal()
            && self.device_gesture.snapshot().is_some_and(|gesture| {
                matches!(
                    gesture.target,
                    hex_session::device_gesture::GestureTarget::PluginParameter { .. }
                )
            })
            && self
                .audio_status()
                .is_some_and(|status| !status.playing.load(Acquire))
    }

    /// Start an explicit Play once the stopped restore, VST3 edit or audio
    /// recovery settles. A replaced project drops the intent.
    pub(in crate::editor) fn tick_deferred_play(&mut self) -> bool {
        if self.stopped_playback_restore.is_some()
            || self.recovery.generation().is_some()
            || self.pending_loop_region().is_some()
            || self.track_gain_controls.has_committed_recovery()
            || self.stopped_plugin_parameter_terminal_pending()
            || self.paused_republish_blocks_play()
        {
            return false;
        }
        let Some((action, revision, generation)) = self.deferred_play.take() else {
            return false;
        };
        if generation == self.project_generation {
            self.queue_playback_action(action, revision);
        }
        true
    }

    pub(in crate::editor) fn awaiting_applied_playback_metadata(&self) -> bool {
        self.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && self.audible_playback.as_ref().is_some_and(|candidate| {
                candidate.project_generation == self.project_generation
                    && candidate.worker_requires_preroll.is_none()
                    && candidate.prepared_revision.is_none()
            })
            && self.normal_start_source().is_some()
    }

    pub(in crate::editor) fn can_defer_toggle_play_for_inflight_publication(&self) -> bool {
        if self.opening_project || self.close_prompt_open || self.state.is_none() {
            return false;
        }
        let Some(audio) = self.audio.as_ref() else {
            return false;
        };
        let Some(status) = audio.control.status() else {
            return false;
        };
        if status.stopped_edit_pending()
            || status.device_fault.load(Acquire)
            || status.render_fault.load(Acquire)
            || status.shutdown_complete.load(Acquire)
        {
            return false;
        }
        self.edit_pending()
            || self.pending_playback.is_some()
            || self.pending_resident_publication.is_some()
            || !self.playback_candidates.is_empty()
            || self.awaiting_applied_playback_metadata()
            || self.audio_admission.current().is_some_and(|admission| {
                admission.project.generation == self.project_generation
                    && admission.state == crate::editor::audio_admission::AdmissionState::Pending
            })
    }

    pub(in crate::editor) fn tick_deferred_toggle_play_and_metadata_seek(&mut self) -> bool {
        let mut changed = false;
        if let Some((frame, generation)) = self.deferred_metadata_seek {
            if generation != self.project_generation
                || self.opening_project
                || self.close_prompt_open
            {
                self.deferred_metadata_seek = None;
                changed = true;
            } else if self.current_seek_requires_preroll().is_some() {
                self.deferred_metadata_seek = None;
                self.request_seek(frame);
                changed = true;
            } else if !self.awaiting_applied_playback_metadata() {
                self.deferred_metadata_seek = None;
                if self.error.is_none() {
                    self.error = Some("playback source is unavailable · cannot seek safely".into());
                }
                changed = true;
            }
        }
        self.tick_deferred_toggle_play() || changed
    }

    fn tick_deferred_toggle_play(&mut self) -> bool {
        let Some(intent) = self.deferred_toggle_play else {
            return false;
        };
        if intent.generation != self.project_generation
            || self.opening_project
            || self.close_prompt_open
        {
            self.deferred_toggle_play = None;
            return true;
        }
        let output = self
            .audio
            .as_ref()
            .and_then(|audio| audio.control.output_identity());
        let mut source = self.normal_start_source();
        match intent.target {
            TogglePlayTarget::DeviceProposal { output: expected } => {
                if output == Some(expected) && self.pending_plugin_effect.is_some() {
                    return false;
                }
                self.deferred_toggle_play = None;
                return true;
            }
            TogglePlayTarget::CommittedDevice {
                output: expected,
                revision,
                audio_revision,
            } => {
                if output != Some(expected)
                    || self
                        .state
                        .as_ref()
                        .is_none_or(|state| state.revision != revision)
                {
                    self.deferred_toggle_play = None;
                    return true;
                }
                source = source.filter(|source| {
                    source.context.editor_revision == revision
                        && audio_revision.is_none_or(|revision| source.audio_revision == revision)
                });
            }
            TogglePlayTarget::Publication => {}
        }
        if let Some(source) = source
            && (self.document_audio_mode == hex_session::DocumentAudioMode::Resident
                || self.current_seek_requires_preroll().is_some())
        {
            if matches!(intent.target, TogglePlayTarget::Publication) {
                self.queue_normal_play(intent.frame);
            } else {
                self.queue_normal_start(crate::editor::playback_settings::Start::Source {
                    frame: intent.frame,
                    source,
                    allow_playing: false,
                });
            }
            return true;
        }
        if self.can_defer_toggle_play_for_inflight_publication() {
            return false;
        }
        self.deferred_toggle_play = None;
        if self.error.is_none() {
            self.error = Some("playback source is unavailable · cannot seek safely".into());
        }
        true
    }

    pub(in crate::editor) fn paused_republish_waiting_for_current(&self) -> bool {
        matches!(
            self.paused_republish,
            Some(crate::editor::audio_admission::PausedRepublish::WaitingForPause { project, output, .. })
                if project.generation == self.project_generation
                    && self
                        .state
                        .as_ref()
                        .is_some_and(|state| state.revision == project.revision)
                    && self
                        .audio
                        .as_ref()
                        .and_then(|audio| audio.control.output_identity())
                        == Some(output)
        )
    }

    pub(in crate::editor) fn paused_republish_blocks_play(&self) -> bool {
        let current = |project: crate::editor::audio_admission::ProjectRevision, output: u64| {
            project.generation == self.project_generation
                && self
                    .state
                    .as_ref()
                    .is_some_and(|state| state.revision == project.revision)
                && self
                    .audio
                    .as_ref()
                    .and_then(|audio| audio.control.output_identity())
                    == Some(output)
        };
        match self.paused_republish {
            Some(crate::editor::audio_admission::PausedRepublish::Submitted {
                project,
                output,
                ..
            }) => current(project, output),
            Some(crate::editor::audio_admission::PausedRepublish::WaitingForPause {
                project,
                output,
                ..
            }) => {
                let actual_playing = self
                    .audio_status()
                    .is_some_and(|status| status.playing.load(Acquire));
                current(project, output) && !self.play_intent.desired_playing(actual_playing)
            }
            _ => false,
        }
    }

    pub(in crate::editor) fn paused_republish_exhausted_for_current(&self) -> bool {
        matches!(
            self.paused_republish,
            Some(crate::editor::audio_admission::PausedRepublish::Exhausted { project, output })
                if project.generation == self.project_generation
                    && self
                        .state
                        .as_ref()
                        .is_some_and(|state| state.revision == project.revision)
                    && self
                        .audio
                        .as_ref()
                        .and_then(|audio| audio.control.output_identity())
                        == Some(output)
        )
    }

    pub(in crate::editor) fn tick_paused_republish(&mut self) -> bool {
        use crate::editor::audio_admission::{AdmissionState, PausedRepublish};
        let Some(republish) = self.paused_republish else {
            return false;
        };
        if let PausedRepublish::Unresolved {
            generation,
            output,
            audio_revision,
            reason,
        } = republish
        {
            if generation != self.project_generation
                || self
                    .audio
                    .as_ref()
                    .and_then(|audio| audio.control.output_identity())
                    != Some(output)
            {
                self.paused_republish = None;
                return false;
            }
            let Some(candidate) = self
                .playback_candidates
                .iter()
                .find(|candidate| candidate.audio_revision == audio_revision)
            else {
                return false;
            };
            if candidate.project_generation != generation
                || candidate.seek_request_id.is_some()
                || candidate.device_preview_identity.is_some()
                || candidate.tuning_preview_identity.is_some()
                || candidate.clip_preview_identity.is_some()
                || self
                    .state
                    .as_ref()
                    .is_none_or(|state| state.revision != candidate.state.revision)
            {
                self.paused_republish = None;
                return false;
            }
            let project = crate::editor::audio_admission::ProjectRevision {
                generation,
                revision: candidate.state.revision,
            };
            self.audio_admission
                .rejected(project, audio_revision, reason.message());
            return record_paused_republish_rejection(
                &mut self.paused_republish,
                &mut self.deferred_play,
                &mut self.status,
                &mut self.error,
                self.project_generation,
                self.state.as_deref(),
                &self.audio_admission,
                project,
                Some(output),
                audio_revision,
                reason,
            );
        }
        let (project, output) = match republish {
            PausedRepublish::WaitingForPause {
                project, output, ..
            }
            | PausedRepublish::Submitted {
                project, output, ..
            }
            | PausedRepublish::Exhausted { project, output } => (project, output),
            PausedRepublish::Unresolved { .. } => return false,
        };
        if self.project_generation != project.generation
            || self
                .state
                .as_ref()
                .is_none_or(|state| state.revision != project.revision)
            || self
                .audio
                .as_ref()
                .and_then(|audio| audio.control.output_identity())
                != Some(output)
        {
            if matches!(republish, PausedRepublish::Submitted { .. }) {
                self.deferred_play = None;
            }
            self.paused_republish = None;
            return false;
        }
        if self.applied_project_revision == Some(project.revision) {
            self.paused_republish = None;
            return true;
        }
        match republish {
            PausedRepublish::Unresolved { .. } | PausedRepublish::Exhausted { .. } => false,
            PausedRepublish::WaitingForPause { audio_revision, .. } => {
                if self.edit_pending()
                    || self.opening_project
                    || self.close_prompt_open
                    || self.save_dialog_open
                    || self.device_gesture.snapshot().is_some()
                    || self.tuning.client.snapshot().is_some()
                    || self.midi_recorder.blocks_edits()
                    || self.gain_recorder.blocks_edits()
                    || self.computer_keyboard.input.is_some()
                    || !self.pending_audio_commands.is_empty()
                    || self.pending_session_ticket.is_some()
                    || self.pending_seek_ticket.is_some()
                    || self.recovery.generation().is_some()
                    || self.stopped_playback_restore.is_some()
                    || self.section_launch_waiting()
                    || self.prepared_seek.is_some()
                    || self.latency_rebuild.is_some()
                {
                    return false;
                }
                let Some(playing) = self
                    .audio
                    .as_ref()
                    .and_then(|audio| audio.control.status())
                    .map(|status| status.playing.load(Acquire))
                else {
                    return false;
                };
                if playing {
                    return false;
                }
                if self.enqueue(Request::RepublishPausedPlayback) {
                    self.paused_republish = Some(PausedRepublish::Submitted {
                        project,
                        output,
                        rejected_audio_revision: audio_revision,
                    });
                    self.status = "applying audio update at the paused cursor".into();
                    return true;
                }
                false
            }
            PausedRepublish::Submitted {
                rejected_audio_revision,
                ..
            } => {
                if self.edit_pending()
                    || !self.playback_candidates.is_empty()
                    || self
                        .audio
                        .as_ref()
                        .and_then(|audio| audio.control.status())
                        .is_some_and(|status| status.publication_in_flight())
                {
                    return false;
                }
                if self.audio_admission.current().is_some_and(|current| {
                    current.project == project
                        && current.audio_revision == Some(rejected_audio_revision)
                        && matches!(current.state, AdmissionState::Rejected(_))
                }) {
                    self.paused_republish = Some(PausedRepublish::Exhausted { project, output });
                    self.deferred_play = None;
                    return replace_status(&mut self.status, "stop playback to apply edit");
                }
                false
            }
        }
    }

    pub(in crate::editor) fn tick_latency_rebuild(&mut self) -> bool {
        use crate::editor::audio_admission::LatencyRebuild;
        let Some(rebuild) = self.latency_rebuild else {
            return false;
        };
        if let LatencyRebuild::Unresolved {
            generation,
            output,
            audio_revision,
        } = rebuild
        {
            if generation != self.project_generation
                || self
                    .audio
                    .as_ref()
                    .and_then(|audio| audio.control.output_identity())
                    != Some(output)
            {
                self.latency_rebuild = None;
                return false;
            }
            let Some(candidate) = self
                .playback_candidates
                .iter()
                .find(|candidate| candidate.audio_revision == audio_revision)
            else {
                return false;
            };
            if candidate.project_generation != generation
                || candidate.seek_request_id.is_some()
                || candidate.device_preview_identity.is_some()
                || candidate.tuning_preview_identity.is_some()
                || self
                    .state
                    .as_ref()
                    .is_none_or(|state| state.revision != candidate.state.revision)
            {
                self.latency_rebuild = None;
                return false;
            }
            let project = crate::editor::audio_admission::ProjectRevision {
                generation,
                revision: candidate.state.revision,
            };
            self.audio_admission.rejected(
                project,
                audio_revision,
                hex_engine::PlaybackHandoffRejection::LatencyLayoutChanged.message(),
            );
            self.latency_rebuild = Some(LatencyRebuild::Waiting {
                project,
                output,
                runtime_audio: None,
            });
            return true;
        }
        let (project, output) = match rebuild {
            LatencyRebuild::Waiting {
                project, output, ..
            }
            | LatencyRebuild::Fading {
                project, output, ..
            }
            | LatencyRebuild::Restoring {
                project, output, ..
            } => (project, output),
            LatencyRebuild::Unresolved { .. } => return false,
        };
        if self.project_generation != project.generation
            || self
                .state
                .as_ref()
                .is_none_or(|state| state.revision != project.revision)
            || self
                .audio
                .as_ref()
                .and_then(|audio| audio.control.output_identity())
                != Some(output)
        {
            // A newer revision or output publishes on its own; its admission
            // reports any further refusal.
            self.latency_rebuild = None;
            return self.clear_latency_waiting();
        }
        match rebuild {
            LatencyRebuild::Unresolved { .. } => false,
            LatencyRebuild::Waiting { runtime_audio, .. } => {
                if self.edit_pending()
                    || self.opening_project
                    || self.close_prompt_open
                    || self.save_dialog_open
                    || self.device_gesture.snapshot().is_some()
                    || self.tuning.client.snapshot().is_some()
                    || self.midi_recorder.blocks_edits()
                    || self.gain_recorder.blocks_edits()
                    || self.computer_keyboard.input.is_some()
                    || !self.pending_audio_commands.is_empty()
                    || self.pending_session_ticket.is_some()
                    || self.pending_seek_ticket.is_some()
                    || self.recovery.generation().is_some()
                    || self.stopped_playback_restore.is_some()
                {
                    return false;
                }
                let Some(playing) = self
                    .audio
                    .as_ref()
                    .and_then(|audio| audio.control.status())
                    .map(|status| status.playing.load(Acquire))
                else {
                    return false;
                };
                if runtime_audio.is_none()
                    && self.applied_project_revision == Some(project.revision)
                {
                    self.latency_rebuild = None;
                    return self.clear_latency_waiting();
                }
                if !playing && runtime_audio.is_none() {
                    // Paused: the callback applies the new latency layout at
                    // the kept cursor. Republish the refused current revision
                    // once rather than leaving audio on the old graph.
                    self.latency_rebuild = None;
                    self.clear_latency_waiting();
                    if self.enqueue(Request::RepublishPausedPlayback) {
                        self.status = "applying latency change at the paused cursor".into();
                    }
                    return true;
                }
                // A requested pause has not reached the callback yet; republish
                // once it reports paused.
                if playing && !self.play_intent.desired_playing(true) {
                    return false;
                }
                // A fresh runtime cuts mutable voices. Until their live-input
                // custody can be restored explicitly, MIDI projects wait for
                // Pause (republish above) or Stop (`stop` restores the current
                // revision) instead of dropping notes. Say so: the committed
                // edit is not audible yet (Pulse #3793).
                if self.state.as_ref().is_some_and(|state| {
                    state
                        .project
                        .tracks()
                        .iter()
                        .any(|track| track.instrument().is_some() || !track.midi_clips().is_empty())
                }) {
                    return replace_error(&mut self.error, LATENCY_EDIT_WAITING);
                }
                let Some(status) = self.audio.as_ref().and_then(|audio| audio.control.status())
                else {
                    return false;
                };
                if runtime_audio.is_some_and(|revision| {
                    status
                        .native_effect_source()
                        .is_none_or(|source| source.audio_revision != revision)
                }) {
                    self.latency_rebuild = None;
                    return self.clear_latency_waiting();
                }
                // Stop would discard Session launches. Only rebuild when a
                // fresh route snapshot proves that every track is in Arrangement.
                let mut routes = [hex_audio::prepared::SessionRouteEntry::default();
                    hex_audio::prepared::SESSION_ROUTE_CAPACITY];
                let Some(snapshot) = status.session_routes(&mut routes) else {
                    return false;
                };
                if snapshot.audio_revision != status.applied_revision.load(Acquire)
                    || self
                        .state
                        .as_ref()
                        .is_none_or(|state| snapshot.count != state.project.tracks().len())
                    || routes[..snapshot.count].iter().any(|route| {
                        route.owner != hex_audio::prepared::SessionRouteOwner::Arrangement
                            || route.pending.is_some()
                    })
                {
                    self.latency_rebuild = None;
                    return false;
                }
                let frame = status.position.load(Acquire);
                if !playing {
                    return self.request_latency_restore(
                        project,
                        output,
                        frame,
                        false,
                        runtime_audio,
                    );
                }
                let Some(audio) = self.audio.as_mut() else {
                    return false;
                };
                let action = PlaybackAction::Transport(PlaybackCommand::Stop);
                let admitted = if let Some(trace) = &self.transport_trace {
                    trace.attempt(audio, action)
                } else {
                    audio.control.action_with_ticket(action)
                };
                let Ok(ticket) = admitted else {
                    // Backpressure leaves the exact rebuild waiting.
                    return false;
                };
                self.invalidate_prepared_seek();
                self.latency_rebuild = Some(LatencyRebuild::Fading {
                    project,
                    output,
                    frame,
                    ticket,
                    runtime_audio,
                });
                self.status = "fading playback before rebuilding audio".into();
                true
            }
            LatencyRebuild::Fading {
                frame,
                ticket,
                runtime_audio,
                ..
            } => {
                let Some(status) = self.audio.as_ref().and_then(|audio| audio.status()) else {
                    self.latency_rebuild = None;
                    return false;
                };
                if status.device_fault.load(Acquire)
                    || status.render_fault.load(Acquire)
                    || status
                        .rejected_action()
                        .is_some_and(|(rejected, _)| rejected == ticket)
                {
                    self.latency_rebuild = None;
                    return replace_status(
                        &mut self.status,
                        "latency rebuild stop was interrupted",
                    );
                }
                // Consumption only starts the fade. The callback keeps
                // reporting playing until the queued Stop actually settles.
                if audio_command_prefix(status, self.document_audio_mode) < ticket
                    || status.playing.load(Acquire)
                    || self.edit_pending()
                    || self.update_preparing()
                    || self.stopped_playback_restore.is_some()
                {
                    return false;
                }
                if runtime_audio.is_none()
                    && self.applied_project_revision == Some(project.revision)
                {
                    self.clear_latency_waiting();
                    self.queue_audio_command(PlaybackCommand::PlayFrom(frame));
                    true
                } else {
                    self.request_latency_restore(project, output, frame, true, runtime_audio)
                }
            }
            LatencyRebuild::Restoring {
                frame,
                resume,
                runtime_audio,
                installed,
                ..
            } => {
                if self.stopped_playback_restore.is_some() {
                    return false;
                }
                if self.applied_project_revision != Some(project.revision)
                    || (runtime_audio.is_some() && !installed)
                {
                    self.latency_rebuild = None;
                    return false;
                }
                // The project event may arrive after the matching audio ACK.
                // Keep the resume intent until its edit/command queues drain.
                if self.edit_pending() || !self.pending_audio_commands.is_empty() {
                    return false;
                }
                self.latency_rebuild = None;
                if resume {
                    self.queue_audio_command(PlaybackCommand::PlayFrom(frame));
                } else {
                    self.queue_audio_command(PlaybackCommand::Seek(frame));
                }
                true
            }
        }
    }

    pub(super) fn request_latency_restore(
        &mut self,
        project: crate::editor::audio_admission::ProjectRevision,
        output: u64,
        frame: u64,
        resume: bool,
        runtime_audio: Option<u64>,
    ) -> bool {
        if !self.enqueue(Request::RestoreStoppedPlayback) {
            return false;
        }
        self.clear_latency_waiting();
        self.stopped_playback_restore =
            Some(crate::editor::audio_admission::StoppedPlaybackRestore::Preparing);
        self.latency_rebuild = Some(crate::editor::audio_admission::LatencyRebuild::Restoring {
            project,
            output,
            frame,
            resume,
            runtime_audio,
            installed: false,
        });
        self.status = if resume {
            "rebuilding audio · effect tails will reset · playback will resume"
        } else {
            "rebuilding audio at the paused cursor"
        }
        .into();
        self.error = None;
        true
    }

    /// Retire the waiting notice once its refused revision is applied,
    /// superseded or handed to a paused republish.
    pub(super) fn clear_latency_waiting(&mut self) -> bool {
        if self.error.as_deref() == Some(LATENCY_EDIT_WAITING) {
            self.error = None;
            true
        } else {
            false
        }
    }

    pub(in crate::editor) fn stop(&mut self) {
        self.playback_settings.pending = None;
        self.escape_stop_armed = false;
        if self.finish_audio_recording_before_stop() {
            return;
        }
        self.stop_transport(true);
    }

    pub(in crate::editor) fn stop_at_beginning(&mut self) {
        self.play_start_at_beginning = true;
        self.stop();
    }

    /// Only an Escape that reaches the transport arms the next keyboard stop.
    /// Use desired state so the second stop can reset before callback feedback.
    pub(in crate::editor) fn stop_from_escape(&mut self) {
        let playing = self
            .audio_status()
            .is_some_and(|status| status.playing.load(Acquire));
        if self.escape_stop_armed && !self.desired_playing(playing) {
            self.stop_at_beginning();
        } else {
            self.stop();
        }
        self.escape_stop_armed = true;
    }

    pub(super) fn tick_stopped_vst3_restore(&mut self) -> bool {
        use crate::editor::audio_admission::StoppedPlaybackRestore;
        let Some(StoppedPlaybackRestore::WaitingForEdits { generation, output }) =
            self.stopped_playback_restore
        else {
            return false;
        };
        if generation != self.project_generation
            || self
                .audio
                .as_ref()
                .and_then(|audio| audio.control.output_identity())
                != Some(output)
            || !self.has_resident_vst3()
        {
            self.stopped_playback_restore = None;
            return true;
        }
        if self.edit_pending() || self.tuning.client.snapshot().is_some() || self.update_preparing()
        {
            return false;
        }
        self.stopped_playback_restore = None;
        if self.has_degraded_vst3_output() {
            self.recover_audio_at(0);
        } else {
            self.queue_stopped_playback_restore();
        }
        true
    }

    pub(super) fn queue_stopped_playback_restore(&mut self) {
        if self.enqueue(Request::RestoreStoppedPlayback) {
            self.stopped_playback_restore =
                Some(crate::editor::audio_admission::StoppedPlaybackRestore::Preparing);
            crate::editor::device_panel::vst3_parameters::refresh(self);
            self.status = "stopped · restoring current sound".into();
            self.error = None;
        }
    }

    pub(super) fn stop_transport(&mut self, stop_output: bool) {
        self.playback_settings.pending = None;
        self.paused_republish = None;
        self.cancel_native_automation_touch();
        use crate::editor::audio_admission::LatencyRebuild;
        let native_restore = matches!(
            self.latency_rebuild.take(),
            Some(
                LatencyRebuild::Waiting {
                    runtime_audio: Some(_),
                    ..
                } | LatencyRebuild::Fading {
                    runtime_audio: Some(_),
                    ..
                }
            )
        );
        // Stop restores the refused revision below, or a pending edit
        // publishes it while stopped.
        self.clear_latency_waiting();
        self.midi_step.cancel_pending();
        // Stop explicitly discards voice histories. A previously refused edit
        // or restoration can now install the canonical sound without inheriting
        // an incompatible preview's history. Keep construction on the worker.
        let can_restore = stop_output
            && self.stopped_playback_restore.is_none()
            && !self.edit_pending()
            && self.device_gesture.snapshot().is_none()
            && self.tuning.client.snapshot().is_none()
            && self.recovery.generation().is_none();
        let restart_degraded = can_restore && self.has_degraded_vst3_output();
        let restore = can_restore
            && !restart_degraded
            && self.audio.as_ref().is_some_and(|audio| {
                self.state.as_ref().is_some_and(|state| {
                    native_restore
                        // Priority Stop ends the transport epochs even when
                        // the same project revision is already audible.
                        || self.has_resident_vst3()
                        || self.applied_project_revision != Some(state.revision)
                        || self.audible_playback.as_ref().is_none_or(|audible| {
                            audible.audio_revision
                                != audio
                                    .control
                                    .status()
                                    .map_or(0, |status| status.applied_revision.load(Acquire))
                                || (audible.device_preview_identity.is_some()
                                    || audible.tuning_preview_identity.is_some())
                        })
                })
            });
        self.finish_midi_recording();
        // A healthy stopped output can be renewing its VST3 epochs after
        // Stop or staging its first graph after restart. In either case a
        // priority Stop would revoke the fresh endpoints. Keep the transport
        // intent below and let the graph install before accepting Play.
        use crate::editor::audio_admission::StoppedPlaybackRestore;
        let restore_owns_stopped_output = self.audio_status().is_some_and(|status| {
            if status.playing.load(Acquire)
                || status.device_fault.load(Acquire)
                || status.render_fault.load(Acquire)
            {
                return false;
            }
            if self.recovery.generation().is_some() {
                return self.has_resident_vst3() && status.applied_revision.load(Acquire) == 0;
            }
            matches!(
                self.stopped_playback_restore,
                Some(
                    StoppedPlaybackRestore::Preparing
                        | StoppedPlaybackRestore::Ready
                        | StoppedPlaybackRestore::Published(_)
                )
            )
        });
        // Publish the MIDI finish intent before any input prefix is revoked.
        if stop_output
            && !restore_owns_stopped_output
            && let Some(audio) = &self.audio
        {
            if let Some(trace) = &self.transport_trace {
                trace.stop(audio);
            }
            audio.control.stop();
        }
        self.cancel_tuning_audition();
        self.stop_computer_keyboard();
        self.cancel_scrub();
        self.cancel_section_launch_wait();
        self.invalidate_prepared_seek();
        self.offline_cursor = 0;
        self.recovery.stop();
        self.pending_audio_commands.clear();
        self.pending_metronome_ticket = None;
        self.pending_seek_ticket = None;
        self.pending_session_ticket = None;
        self.play_intent.priority_stop();
        self.deferred_play = None;
        self.deferred_toggle_play = None;
        self.deferred_metadata_seek = None;
        self.status = if self.play_start_at_beginning {
            "stopped · play start at beginning"
        } else {
            "stopped"
        }
        .into();
        if restart_degraded && !self.update_preparing() {
            // A revoked worker cannot be renewed on the applied graph. Start a
            // new output from the current project, at the explicit Stop cursor.
            self.recover_audio_at(0);
        } else if restore && !self.update_preparing() {
            self.queue_stopped_playback_restore();
        } else if stop_output
            && self.stopped_playback_restore.is_none()
            && self.recovery.generation().is_none()
            && self.has_resident_vst3()
            && let Some(output) = self
                .audio
                .as_ref()
                .and_then(|audio| audio.control.output_identity())
        {
            // Read-only inventory and in-flight edit terminals can overlap Stop.
            // Let them drain before renewing the exact stopped document/output.
            self.stopped_playback_restore = Some(
                crate::editor::audio_admission::StoppedPlaybackRestore::WaitingForEdits {
                    generation: self.project_generation,
                    output,
                },
            );
        }
    }
}

// Called only after exact current context/output identity has been reconciled.
pub(super) const fn is_paused_republish_rejection(
    reason: hex_engine::PlaybackHandoffRejection,
) -> bool {
    matches!(
        reason,
        hex_engine::PlaybackHandoffRejection::SessionRuntime
            | hex_engine::PlaybackHandoffRejection::ForeignTailSourcePaused
    )
}

#[allow(clippy::too_many_arguments)] // Disjoint EditorView fields borrowed alongside active audio control.
pub(super) fn record_paused_republish_rejection(
    paused_republish: &mut Option<crate::editor::audio_admission::PausedRepublish>,
    deferred_play: &mut Option<(PlaybackAction, Option<u64>, u64)>,
    status: &mut String,
    error: &mut Option<crate::editor::display_error::DisplayError>,
    project_generation: u64,
    state: Option<&ViewState>,
    audio_admission: &crate::editor::audio_admission::AudioAdmissionStatus,
    project: crate::editor::audio_admission::ProjectRevision,
    output: Option<u64>,
    audio_revision: u64,
    reason: hex_engine::PlaybackHandoffRejection,
) -> bool {
    use crate::editor::audio_admission::{AdmissionState, PausedRepublish};
    let Some(output) = output else {
        return false;
    };
    if project.generation != project_generation
        || state.is_none_or(|state| state.revision != project.revision)
        || !audio_admission.current().is_some_and(|current| {
            current.project == project
                && current.audio_revision == Some(audio_revision)
                && matches!(current.state, AdmissionState::Rejected(_))
        })
    {
        return false;
    }
    let mut changed = false;
    if reason == hex_engine::PlaybackHandoffRejection::ForeignTailSourcePaused
        || matches!(
            *paused_republish,
            Some(
                PausedRepublish::Submitted {
                    project: previous,
                    output: previous_output,
                    ..
                } | PausedRepublish::Exhausted {
                    project: previous,
                    output: previous_output,
                }
            ) if previous == project && previous_output == output
        )
    {
        *paused_republish = Some(PausedRepublish::Exhausted { project, output });
        *deferred_play = None;
        changed |= replace_status(status, "stop playback to apply edit");
        changed |= replace_error(error, reason.message());
        return changed;
    }
    *paused_republish = Some(PausedRepublish::WaitingForPause {
        project,
        output,
        audio_revision,
    });
    let submitted_status = super::submitted_audio_revision_status(audio_revision);
    if *status == submitted_status {
        changed |= replace_status(status, "audio update waiting for pause or stop");
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paused_republish_rejection_matches_only_expected_handoff_reasons() {
        assert!(is_paused_republish_rejection(
            hex_engine::PlaybackHandoffRejection::SessionRuntime
        ));
        assert!(is_paused_republish_rejection(
            hex_engine::PlaybackHandoffRejection::ForeignTailSourcePaused
        ));
    }

    #[test]
    fn clear_stale_playback_unavailable_error_clears_only_playback_unavailable_messages() {
        let mut stale: Option<String> =
            Some("playback source is unavailable · cannot seek safely".into());
        assert!(clear_stale_playback_unavailable_error(&mut stale));
        assert!(stale.is_none());

        let mut retry: Option<String> =
            Some("playback source is unavailable · retry playback when audio is ready".into());
        assert!(clear_stale_playback_unavailable_error(&mut retry));
        assert!(retry.is_none());

        let mut unrelated: Option<String> = Some("save failed · disk full".into());
        assert!(!clear_stale_playback_unavailable_error(&mut unrelated));
        assert_eq!(unrelated.as_deref(), Some("save failed · disk full"));
    }
}
