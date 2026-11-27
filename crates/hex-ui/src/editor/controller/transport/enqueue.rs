//! Editor request admission, clip-scope wrapping, and queue submission.
use super::*;

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

    pub(in super::super) fn enqueue_with_clip_scope(
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
            self.device_panel.sampler_edit_resume = Some(frame);
            if accepted {
                replace_status(
                    &mut self.status,
                    "applying sampler edit · playback will resume",
                );
            }
        }
        if accepted && let Some(mut selection) = selection {
            self.bind_created_selection_terminal(&mut selection);
            self.midi_editor.pending_created_note_selection = Some(selection);
            // Immediate submission happens inside enqueue_request, before this
            // intent is installed. Queued/tuning-barrier requests are stamped
            // only when their actual Service submission succeeds in tick.
            let queued = self
                .pending_requests
                .iter()
                .chain(self.device_panel.gesture_barriers.iter())
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

    pub(in super::super) fn mutates_clipboard_or_project(request: &Request) -> bool {
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
                | Request::InsertNestedCatalogDevice { .. }
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
                || self.device_panel.sampler_parameter.numeric_draft_active())
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
        ) && let Some(control) = &self.persistence.bounce_control
        {
            control.cancel();
        }
        let open_save = self.persistence.project_open.permits_save()
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
            if self.audio_pub.pending_recovery_generation == Some(*generation)
                && self.audio_pub.recovery.generation() == Some(*generation));
        if (self.persistence.project_open.busy() || self.persistence.pending_open_path.is_some())
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
                self.midi_editor.recorder.accepts_commit(*request_id)
            }
            Request::PrepareComputerKeyboard { .. } | Request::PrepareLiveExpression { .. } => {
                self.midi_editor.recorder.preparing()
            }
            _ => false,
        };
        if self.midi_editor.recorder.blocks_edits() && !own_midi_request {
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
            let sampler_conflict = self.device_panel.sampler_parameter.numeric_draft_active();
            if gain_conflict || automation_conflict || sampler_conflict {
                self.error = Some("finish or cancel the local draft before editing".into());
                return false;
            }
        }
        #[cfg(test)]
        match &request {
            Request::CopyTimeRange(_) => self.test_probes.last_time_request = Some(("copy", None)),
            Request::CutClips(_) | Request::CutTimeRange(_) => {
                self.test_probes.last_time_request = Some(("cut", None))
            }
            Request::PasteTimeRange { track, frame } => {
                self.test_probes.last_time_request = Some(("paste", Some((*track, *frame))))
            }
            Request::InsertTimeRange { track, frame } => {
                self.test_probes.last_time_request = Some(("insert", Some((*track, *frame))))
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
            self.test_probes.last_enqueued_arrangement_device_drop = Some(*destination);
            self.test_probes.last_enqueued_device_track_gain = Some(*new_track_gain);
        }
        #[cfg(test)]
        if let Request::ConvertClip {
            project_generation,
            revision,
            clip,
            to_audio,
        } = &request
        {
            self.test_probes.last_enqueued_clip_conversion =
                Some((*project_generation, *revision, *clip, *to_audio));
        }
        #[cfg(test)]
        if let Request::Edit(command)
        | Request::EditArrangementSection { command, .. }
        | Request::MoveExistingClips { command, .. } = &request
        {
            self.test_probes.last_enqueued_edit = Some(command.clone());
        }
        #[cfg(test)]
        if let Request::PaintMidiClip(intent) = &request {
            self.test_probes.last_enqueued_edit =
                Some(hex_project::Command::PlaceMidiClipWithOverwrite {
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
            self.test_probes.last_enqueued_native_device_add = Some((*track, *class, **type_id));
        }
        #[cfg(test)]
        if let Request::ReplaceNativeDevice {
            device,
            class,
            type_id,
        } = &request
        {
            self.test_probes.last_enqueued_native_device_replace =
                Some((*device, *class, **type_id));
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
                    self.test_probes.last_enqueued_native_device_add =
                        Some((*track, *class, *type_id));
                }
                hex_session::service::TrackCatalogPlacement::ReplaceInstrument(device)
                | hex_session::service::TrackCatalogPlacement::ReplaceMidiProcessor(device) => {
                    self.test_probes.last_enqueued_native_device_replace =
                        Some((*device, *class, *type_id));
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
            self.test_probes.last_enqueued_native_device_drop = Some(EnqueuedNativeDeviceDrop {
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
                    self.test_probes.last_enqueued_import_to_track =
                        Some((path.clone(), *track, *start, options.overwrite))
                }
                hex_session::import_batch::AudioBatchDestination::NewTracks {
                    before_track,
                    start,
                } => {
                    self.test_probes.last_enqueued_import_to_new_track =
                        Some((path.clone(), *before_track, *start))
                }
            }
        }
        #[cfg(test)]
        match &request {
            Request::ImportToTrack { path, track, start } => {
                self.test_probes.last_enqueued_import_to_track =
                    Some((path.clone(), *track, *start, false));
            }
            Request::ImportToTrackWithOverwrite { path, track, start } => {
                self.test_probes.last_enqueued_import_to_track =
                    Some((path.clone(), *track, *start, true));
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
            self.test_probes.last_enqueued_import_to_new_track =
                Some((path.clone(), *before_track, *start));
        }
        self.midi_editor.creation = None;
        if self.midi_editor.stroke.is_some() {
            self.cancel_midi_stroke();
        }
        if request.is_project_open() {
            self.cancel_plugin_effect();
            self.retire_sampler_parameter();
            self.cancel_first_sampler_load(None);
            if let Some(control) = &self.persistence.audio_batch {
                control.request_cancel();
            }
        }
        if !request.is_project_open()
            && !allows_non_open_request(
                self.persistence.close_prompt_open,
                self.persistence.opening_project,
                duplicate_continuation,
            )
        {
            self.error = Some(if self.persistence.opening_project {
                "opening project · editing is paused".into()
            } else {
                "close confirmation open · editing is paused".into()
            });
            return false;
        }
        // An owned recovery continuation is not a new user intent: a selected
        // picker path may still be awaiting the next render's Open reconciliation.
        if !request.is_project_open() && !recovery_restore {
            self.persistence.pending_open_path = None;
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
                | Request::InsertNestedCatalogDevice { .. }
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
        if request.finalizes_parameter_gesture() && self.device_panel.gesture.snapshot().is_some() {
            return self.enqueue_device_barrier(request);
        }
        let mut request = Some(request);
        if let Some(accepted) = self.enqueue_after_device_terminal(&mut request) {
            return accepted;
        }
        let Some(request) = request else {
            return false;
        };
        if self.settings.tuning.client.snapshot().is_some() && request.finalizes_parameter_gesture()
        {
            if self.pending_requests.len() + self.device_panel.gesture_barriers.len()
                >= UI_REQUEST_CAPACITY
            {
                self.error = Some("request queue full · edit was not applied".into());
                return false;
            }
            if let Err(reason) = self.finalize_tuning_gesture() {
                self.settings.tuning.error = Some(reason.into());
                return false;
            }
        }
        if self.settings.tuning.client.has_pending_requests() {
            if self.pending_requests.len() + self.device_panel.gesture_barriers.len()
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
            if self.pending_requests.len() + self.device_panel.gesture_barriers.len()
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
                if self.pending_requests.len() + self.device_panel.gesture_barriers.len()
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
}
