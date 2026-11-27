//! Output lifecycle admission and graph/cursor restoration, without canonical edits.
use super::{
    EditorView,
    drag::BrowserDropDestination,
    recovery_state::{DeferredEditKind, DeferredGraphEdit, StrandedGestureRecovery},
};
use hex_engine::playback::{PlaybackAction, PlaybackCommand};
use hex_engine::{AudioSourceContext, NativeDeviceClass};
use hex_project::{AudioViewResizeOperation, ClipId, Command};
use hex_session::service::{
    DeviceClipboardAction, NativeDeviceDropSource, Request, SamplerEdit, TrackClipboardAction,
    ViewState,
};
use hex_session::tuning_gesture::TuningLibraryOperation;
use std::sync::atomic::Ordering::{Acquire, Relaxed};

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "recovery_tests.rs"]
pub(in crate::editor) mod tests;

#[cfg(all(test, not(feature = "ui-test-support")))]
mod classification_tests;

fn clip_play_start(request: &Request) -> Option<ClipId> {
    match request {
        Request::Edit(Command::ResizeAudioClipView {
            clip,
            operation: AudioViewResizeOperation::PlayStart { .. },
        }) => Some(*clip),
        _ => None,
    }
}

fn is_mixer_control_edit(request: &Request) -> bool {
    matches!(
        request,
        Request::Edit(
            Command::Gain { .. }
                | Command::SetTrackGains { .. }
                | Command::MasterGain { .. }
                | Command::ReturnGain { .. }
                | Command::Balance { .. }
        )
    )
}

fn is_track_switch_edit(request: &Request) -> bool {
    matches!(
        request,
        Request::Edit(
            Command::ToggleSolo { .. }
                | Command::ToggleSoloTracks { .. }
                | Command::ToggleMute(_)
                | Command::ToggleTracksMute(_)
                | Command::Mute { .. }
        )
    )
}

/// Exhaustively classifies whether a service request mutates/republishes the
/// resident playback graph (and therefore must recover degraded VST3 output
/// before submission) versus non-graph, control-only, or explicitly guarded
/// requests (`DeviceGesture`, `Save*`, `Open*`, `RestartAudio`, queries, etc.).
pub(super) fn is_graph_republishing_edit(request: &Request) -> bool {
    match request {
        // Core project edits, history, and clipboard mutations.
        Request::Edit(_)
        | Request::Undo
        | Request::Redo
        | Request::CutClips(_)
        | Request::CutTimeRange(_)
        | Request::PasteTimeRange { .. }
        | Request::InsertTimeRange { .. }
        | Request::InsertProjectSample { .. }
        | Request::BindUnassignedNativeSource { .. }
        | Request::AppendSourceUtility { .. }
        | Request::RemoveSourceUtility { .. }
        | Request::MoveSourceUtility { .. }
        | Request::RebindNativeSource { .. }
        | Request::ChooseNativeSourceReceiver { .. }
        | Request::RenameRetainedSource { .. }
        | Request::DeleteRetainedSource { .. }
        | Request::CreateRetainedNativeSource { .. }
        | Request::PaintMidiClip(_)
        | Request::ReplaceDeviceModulation { .. }
        | Request::MoveTrackDevices { .. }
        | Request::ReorderDevices { .. }
        | Request::ReorderTracks { .. }
        | Request::ReparentTracks { .. }
        | Request::EditArrangementSection { .. }
        | Request::MoveExistingClips { .. }
        | Request::DuplicateClips { .. }
        | Request::DuplicateAliasClips { .. }
        | Request::DuplicateSessionCell { .. }
        | Request::GroupClips { .. }
        | Request::GroupTimeRange { .. }
        | Request::GroupAudioEvents { .. }
        | Request::LoadDevicePreset { .. }
        | Request::InsertBrowserInstrumentTrack { .. }
        | Request::InsertPluginEffect(_)
        | Request::InsertEffectBusDevice { .. }
        | Request::InsertNestedCatalogDevice { .. }
        | Request::DropBrowserDeviceOnArrangement { .. }
        | Request::CopyArrangementClipToConsoleTrack { .. }
        | Request::CommitMidiTake { .. }
        | Request::RecoverDeviceAutomationTake { .. }
        | Request::GainLane { .. }
        | Request::DeviceAutomation { .. }
        | Request::DeviceAutomationControl { .. }
        | Request::DropNativeDevice { .. }
        | Request::AddNativeDevice { .. }
        | Request::ReplaceNativeDevice { .. }
        | Request::LoadTrackCatalogDevice { .. }
        | Request::CompareDevice { .. }
        | Request::SamplerEdit(_)
        | Request::SetDeviceParameter { .. }
        | Request::Import(_)
        | Request::RecoverTruncatedImport(_)
        | Request::RelinkMedia(_)
        | Request::ImportDeviceAsset { .. }
        | Request::LoadFirstSampler { .. }
        | Request::DropDrumRackPad { .. }
        | Request::AssignSampler { .. }
        | Request::ImportAudioBatch { .. }
        | Request::ImportToTrack { .. }
        | Request::ImportToTrackWithOverwrite { .. }
        | Request::ImportToNewTrack { .. }
        | Request::ImportToNewTrackWithGain { .. }
        | Request::ImportToSession { .. }
        | Request::ChangeProjectSampleRate { .. }
        | Request::BounceArrangement { .. }
        | Request::ConvertClip { .. } => true,

        // Clipboard Copy is read-only; Cut/Paste/Duplicate mutate the graph.
        Request::DeviceClipboard { action, .. } => {
            !matches!(action, DeviceClipboardAction::Copy { .. })
        }
        Request::TrackClipboard { action, .. } => !matches!(action, TrackClipboardAction::Copy(_)),
        // Only Load mutates the project's tuning; other library ops are file-only.
        Request::TuningLibrary { operation, .. } => {
            matches!(operation, TuningLibraryOperation::Load { .. })
        }

        // Explicitly guarded live gestures, non-graph control/lifecycle requests,
        // read-only observations/copies, saves, opens, and offline exports.
        Request::DeviceGesture(_)
        | Request::TuningGesture(_)
        | Request::CopyTimeRange(_)
        | Request::CopyClips(_)
        | Request::SaveNew(_)
        | Request::SaveProjectAs { .. }
        | Request::SaveCommittedParameterCopy { .. }
        | Request::SaveCurrent
        | Request::SaveCurrentWithFormatUpgrade { .. }
        | Request::SaveDevicePreset { .. }
        | Request::SaveDeviceUnitPreset { .. }
        | Request::SaveDeviceUnitDefault { .. }
        | Request::SaveDeviceDefault { .. }
        | Request::Open(_)
        | Request::OpenWith { .. }
        | Request::NewWithAudioMode { .. }
        | Request::OpenWithAudioMode { .. }
        | Request::RestartAudio { .. }
        | Request::RestartRecovery(_)
        | Request::RestorePlayback { .. }
        | Request::RepublishPausedPlayback
        | Request::RestoreStoppedPlayback
        | Request::ContinueStoppedPlaybackRestore
        | Request::ResidentArrangementSeek(_)
        | Request::PrepareTuningAudition { .. }
        | Request::PrepareLiveExpression { .. }
        | Request::PrepareComputerKeyboard { .. }
        | Request::PreparePausedInputMonitor { .. }
        | Request::TogglePluginEditor { .. }
        | Request::ReloadPlugin { .. }
        | Request::ContinuePluginEditor
        | Request::ContinueRetiringEditorReplay
        | Request::ContinuePluginEffect
        | Request::ContinueDeviceInsert
        | Request::ContinueRecovery
        | Request::ContinueSamplerEdit
        | Request::QueryPluginParameters(_)
        | Request::ContinuePluginParameters
        | Request::QueryPluginParameterConversion(_)
        | Request::ContinuePluginParameterConversion
        | Request::ContinuePluginStateRestore
        | Request::DeviceAutomationTake { .. }
        | Request::StartPluginEditorRecording { .. }
        | Request::StopPluginEditorRecording { .. }
        | Request::ContinuePluginEditorRecording { .. }
        | Request::QueryFailedPluginAutomation { .. }
        | Request::InspectSealedAutomationRecovery { .. }
        | Request::QueryDeviceAutomationCloseSafety { .. }
        | Request::PreparePluginEditorClose { .. }
        | Request::ArmNativeParameterRecording { .. }
        | Request::ArmGainRecording { .. }
        | Request::Export(_)
        | Request::ExportRange { .. } => false,
    }
}

fn deferred_graph_edit_waiting_status(request: &Request) -> &'static str {
    match request {
        Request::Edit(Command::ToggleSolo { .. } | Command::ToggleSoloTracks { .. }) => {
            "restarting audio · solo will update when ready"
        }
        Request::Edit(
            Command::ToggleMute(_) | Command::ToggleTracksMute(_) | Command::Mute { .. },
        ) => "restarting audio · mute will update when ready",
        Request::Undo => "restarting audio · undo will apply when ready",
        Request::Redo => "restarting audio · redo will apply when ready",
        _ => "restarting audio · edit will apply when ready",
    }
}

fn deferred_graph_edit_applying_status(request: &Request) -> &'static str {
    match request {
        Request::Edit(Command::ToggleSolo { .. } | Command::ToggleSoloTracks { .. }) => {
            "updating solo"
        }
        Request::Edit(
            Command::ToggleMute(_) | Command::ToggleTracksMute(_) | Command::Mute { .. },
        ) => "updating mute",
        Request::Undo => "undoing",
        Request::Redo => "redoing",
        _ => "applying edit",
    }
}

fn request_is_current_for_recovery(
    request: &Request,
    state: &ViewState,
    context: AudioSourceContext,
) -> bool {
    match request {
        Request::InsertProjectSample {
            project_generation,
            revision,
            ..
        }
        | Request::BindUnassignedNativeSource {
            project_generation,
            revision,
            ..
        }
        | Request::AppendSourceUtility {
            project_generation,
            revision,
            ..
        }
        | Request::RemoveSourceUtility {
            project_generation,
            revision,
            ..
        }
        | Request::MoveSourceUtility {
            project_generation,
            revision,
            ..
        }
        | Request::RebindNativeSource {
            project_generation,
            revision,
            ..
        }
        | Request::ChooseNativeSourceReceiver {
            project_generation,
            revision,
            ..
        }
        | Request::RenameRetainedSource {
            project_generation,
            revision,
            ..
        }
        | Request::DeleteRetainedSource {
            project_generation,
            revision,
            ..
        }
        | Request::CreateRetainedNativeSource {
            project_generation,
            revision,
            ..
        }
        | Request::ReplaceDeviceModulation {
            project_generation,
            revision,
            ..
        }
        | Request::DeviceClipboard {
            project_generation,
            revision,
            ..
        }
        | Request::MoveTrackDevices {
            project_generation,
            revision,
            ..
        }
        | Request::ReorderDevices {
            project_generation,
            revision,
            ..
        }
        | Request::TrackClipboard {
            project_generation,
            revision,
            ..
        }
        | Request::ReorderTracks {
            project_generation,
            revision,
            ..
        }
        | Request::ReparentTracks {
            project_generation,
            revision,
            ..
        }
        | Request::EditArrangementSection {
            project_generation,
            revision,
            ..
        }
        | Request::MoveExistingClips {
            project_generation,
            revision,
            ..
        }
        | Request::DuplicateClips {
            project_generation,
            revision,
            ..
        }
        | Request::DuplicateAliasClips {
            project_generation,
            revision,
            ..
        }
        | Request::DuplicateSessionCell {
            project_generation,
            revision,
            ..
        }
        | Request::GroupClips {
            project_generation,
            revision,
            ..
        }
        | Request::GroupTimeRange {
            project_generation,
            revision,
            ..
        }
        | Request::GroupAudioEvents {
            project_generation,
            revision,
            ..
        }
        | Request::InsertBrowserInstrumentTrack {
            project_generation,
            revision,
            ..
        }
        | Request::InsertEffectBusDevice {
            project_generation,
            revision,
            ..
        }
        | Request::InsertNestedCatalogDevice {
            project_generation,
            revision,
            ..
        }
        | Request::DropBrowserDeviceOnArrangement {
            project_generation,
            revision,
            ..
        }
        | Request::CopyArrangementClipToConsoleTrack {
            project_generation,
            revision,
            ..
        }
        | Request::CommitMidiTake {
            project_generation,
            revision,
            ..
        }
        | Request::GainLane {
            project_generation,
            revision,
            ..
        }
        | Request::DeviceAutomation {
            project_generation,
            revision,
            ..
        }
        | Request::DeviceAutomationControl {
            project_generation,
            revision,
            ..
        }
        | Request::TuningLibrary {
            project_generation,
            revision,
            ..
        }
        | Request::DropNativeDevice {
            project_generation,
            revision,
            ..
        }
        | Request::LoadTrackCatalogDevice {
            project_generation,
            revision,
            ..
        }
        | Request::LoadFirstSampler {
            project_generation,
            revision,
            ..
        }
        | Request::ChangeProjectSampleRate {
            project_generation,
            revision,
            ..
        }
        | Request::BounceArrangement {
            project_generation,
            revision,
            ..
        }
        | Request::ConvertClip {
            project_generation,
            revision,
            ..
        } => {
            (*project_generation, *revision)
                == (context.project_generation, context.editor_revision)
        }
        Request::PaintMidiClip(paint) => {
            (paint.project_generation, paint.revision)
                == (context.project_generation, context.editor_revision)
        }
        Request::InsertPluginEffect(insertion) => insertion.context == context,
        Request::RecoverDeviceAutomationTake {
            context: target, ..
        } => *target == context,
        Request::SamplerEdit(edit) => {
            let (project_generation, revision) = match edit {
                SamplerEdit::Parameter(edit) => (edit.project_generation, edit.revision),
                SamplerEdit::Replacement(edit) => (edit.project_generation, edit.revision),
                SamplerEdit::Midi(edit) => (edit.project_generation, edit.revision),
                SamplerEdit::History(edit) => (edit.project_generation, edit.revision),
            };
            (project_generation, revision) == (context.project_generation, context.editor_revision)
        }
        Request::AddNativeDevice { track, .. } => state.project.track(*track).is_some(),
        Request::Edit(Command::RemoveDevice(device))
        | Request::Edit(Command::ToggleDeviceBypass { device }) => {
            state.project.device(*device).is_some()
        }
        Request::Edit(
            Command::Gain { track, .. }
            | Command::Balance { track, .. }
            | Command::ToggleSolo { track, .. }
            | Command::ToggleMute(track)
            | Command::Mute { track, .. },
        ) => state.project.track(*track).is_some(),
        Request::Edit(Command::ToggleSoloTracks { tracks, .. })
        | Request::Edit(Command::ToggleTracksMute(tracks)) => tracks
            .ids()
            .iter()
            .all(|track| state.project.track(*track).is_some()),
        Request::Edit(Command::SetTrackGains { gains }) => {
            !gains.is_empty()
                && gains
                    .iter()
                    .all(|(track, _)| state.project.track(*track).is_some())
        }
        Request::Edit(Command::MasterGain { .. }) => true,
        Request::Edit(Command::ReturnGain { return_track, .. }) => state
            .project
            .return_tracks()
            .iter()
            .any(|track| track.id() == *return_track),
        _ => true,
    }
}

impl EditorView {
    /// Every UI-queued request awaiting service submission, including one held
    /// behind an in-flight degraded-VST3 audio recovery.
    pub(in crate::editor) fn queued_requests(&self) -> impl Iterator<Item = &Request> {
        self.pending_requests
            .iter()
            .chain(self.device_panel.gesture_barriers.iter())
            .chain(
                self.audio_pub
                    .recovery
                    .deferred_edit
                    .as_ref()
                    .map(|edit| edit.request.as_ref()),
            )
    }

    /// Retire any released previews/receipts owned by an aborted recovery.
    pub(in crate::editor) fn clear_recovery(&mut self) {
        if self.audio_pub.recovery.holds_loop_region() {
            self.interaction.pending_loop = None;
        }
        if self.audio_pub.recovery.holds_clip_play_start() {
            self.audio_detail.clip_loop.retire_deferred_play_start();
        }
        if self.audio_pub.recovery.holds_mixer_edit() {
            self.track_gain_controls.retire_deferred_recovery();
        }
        let deferred = self.audio_pub.recovery.deferred_edit.take();
        self.audio_pub.recovery.fail();
        let Some(edit) = deferred else {
            return;
        };
        if edit.kind == DeferredEditKind::SampleDrop {
            self.browser.pending_sample_drop = None;
        }
        if edit.kind == DeferredEditKind::TimeDuplicate {
            self.interaction.pending_time_range_move = None;
        }
        if edit.kind == DeferredEditKind::ToggleDeviceBypass {
            self.device_panel.retire_unsubmitted_bypass(&edit.request);
        }
        self.retire_unsubmitted_request(&edit.request, Some("audio recovery did not complete"));
    }

    pub(super) fn has_resident_vst3(&self) -> bool {
        self.audio_pub.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && self.state.as_ref().is_some_and(|state| {
                state
                    .project
                    .automation_devices()
                    .any(|device| device.state().type_id().as_str() == Ok("hex.vst3"))
            })
    }

    pub(super) fn holds_stopped_vst3_requests(&self) -> bool {
        self.audio_pub
            .stopped_playback_restore
            .is_some_and(|restore| {
                !matches!(
                    restore,
                    super::audio_admission::StoppedPlaybackRestore::WaitingForEdits { .. }
                )
            })
            && self.has_resident_vst3()
    }

    pub(super) fn has_degraded_vst3_output(&self) -> bool {
        self.audio.is_some()
            && self.audio_pub.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && self.audio_pub.stopped_playback_restore.is_none()
            && !self.device_panel.degraded_plugins.is_empty()
    }

    /// True only when the audio output itself has a device/render fault requiring
    /// full-engine restart. A single degraded VST3 plugin stays isolated as an
    /// aligned pass-through and must not tear down healthy playback or editors.
    pub(super) fn has_output_fault_requiring_recovery(&self) -> bool {
        self.audio_pub.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && self.audio_pub.stopped_playback_restore.is_none()
            && self.audio_status().is_some_and(|status| {
                status.device_fault.load(Acquire) || status.render_fault.load(Acquire)
            })
    }

    fn has_degraded_stranded_device_gesture(&self) -> bool {
        let Some(snapshot) = self.device_panel.gesture.snapshot() else {
            return false;
        };
        (self.has_output_fault_requiring_recovery() || self.has_degraded_vst3_output())
            && self.device_panel.gesture.is_recovery_pending()
            && !self.device_panel.gesture.has_pending_requests()
            && self.device_panel.gesture_barriers.is_empty()
            && snapshot.key.generation == self.project_generation
            && !self.owns_vst3_recording(snapshot.key)
    }

    pub(super) fn recoverable_graph_edit(&self, request: &Request) -> Option<DeferredEditKind> {
        let output_fault = self.has_output_fault_requiring_recovery();
        if clip_play_start(request).is_some()
            && self.pending_clip_loop_matches(request)
            && output_fault
        {
            return Some(DeferredEditKind::ClipPlayStart);
        }
        if !output_fault && self.audio_pub.recovery.deferred_edit.is_none() {
            if is_track_switch_edit(request) && self.has_degraded_stranded_device_gesture() {
                return Some(DeferredEditKind::GraphEdit);
            }
            return None;
        }
        if self.pending_time_duplicate_matches(request) {
            return Some(DeferredEditKind::TimeDuplicate);
        }
        if self.pending_loop_matches(request) {
            return Some(DeferredEditKind::LoopRegion);
        }
        if matches!(
            self.sample_drop_target(),
            Some(
                BrowserDropDestination::ExistingTrack { .. }
                    | BrowserDropDestination::NewTrack { .. }
            )
        ) && self.pending_sample_drop_matches(request)
        {
            return Some(DeferredEditKind::SampleDrop);
        }
        if matches!(request, Request::Edit(Command::RemoveDevice(_))) {
            return Some(DeferredEditKind::RemoveDevice);
        }
        if matches!(request, Request::Edit(Command::ToggleDeviceBypass { .. })) {
            return Some(DeferredEditKind::ToggleDeviceBypass);
        }
        if is_mixer_control_edit(request) {
            return Some(DeferredEditKind::MixerControl);
        }
        if matches!(request, Request::InsertPluginEffect(_)) {
            return Some(DeferredEditKind::PluginEffect);
        }
        if matches!(
            request,
            Request::DropBrowserDeviceOnArrangement {
                class: NativeDeviceClass::Effect,
                ..
            } | Request::DropNativeDevice {
                source: NativeDeviceDropSource::Browser {
                    class: NativeDeviceClass::Effect,
                    ..
                },
                ..
            } | Request::AddNativeDevice {
                class: NativeDeviceClass::Effect,
                ..
            } | Request::LoadTrackCatalogDevice {
                class: NativeDeviceClass::Effect,
                ..
            } | Request::InsertNestedCatalogDevice {
                class: NativeDeviceClass::Effect,
                ..
            }
        ) {
            return Some(DeferredEditKind::NativeEffect);
        }
        is_graph_republishing_edit(request).then_some(DeferredEditKind::GraphEdit)
    }

    pub(super) fn recovering_clip_play_start(&self, clip: ClipId) -> bool {
        self.state.as_ref().is_some_and(|state| {
            self.audio_pub.recovery.admits_clip_play_start(
                AudioSourceContext {
                    project_generation: self.project_generation,
                    editor_revision: state.revision,
                },
                clip,
            )
        })
    }

    pub(super) fn recovering_clip_play_start_request(&self, request: &Request) -> bool {
        clip_play_start(request).is_some_and(|clip| self.recovering_clip_play_start(clip))
            && self.pending_clip_loop_matches(request)
    }

    pub(super) fn recovering_mixer_edit_request(&self, request: &Request) -> bool {
        is_mixer_control_edit(request)
            && self.pending_mixer_edit_matches(request)
            && self.state.as_ref().is_some_and(|state| {
                self.audio_pub
                    .recovery
                    .admits_mixer_edit(AudioSourceContext {
                        project_generation: self.project_generation,
                        editor_revision: state.revision,
                    })
            })
    }

    /// Hold one admitted edit through the fresh graph and cursor. The sample
    /// request already carries the policy captured at its original drop.
    pub(super) fn recover_before_graph_edit(
        &mut self,
        kind: DeferredEditKind,
        request: Request,
    ) -> bool {
        let retire_refused = |view: &mut Self, request: &Request| {
            if kind == DeferredEditKind::MixerControl {
                view.retire_deferred_mixer_edit();
            }
            let error = view.error.as_ref().map(ToString::to_string);
            view.retire_unsubmitted_request(request, error.as_deref());
        };
        let continuing_play_start = kind == DeferredEditKind::ClipPlayStart
            && clip_play_start(&request).is_some_and(|clip| self.recovering_clip_play_start(clip));
        let continuing_mixer_edit =
            kind == DeferredEditKind::MixerControl && self.recovering_mixer_edit_request(&request);
        let continuing = continuing_play_start || continuing_mixer_edit;
        if self.audio_pub.recovery.generation().is_some()
            && (!continuing || self.audio_pub.recovery.deferred_edit.is_some())
        {
            self.error = Some("audio recovery pending · wait before editing".into());
            retire_refused(self, &request);
            return false;
        }
        let stranded_gesture = (!continuing
            && is_track_switch_edit(&request)
            && self.has_degraded_stranded_device_gesture())
        .then(|| {
            let snapshot = self.device_panel.gesture.snapshot()?;
            Some(StrandedGestureRecovery {
                key: snapshot.key,
                reconcilable: self.device_panel.gesture.reconcilable_snapshot(),
                had_stranded_commit: self.device_panel.gesture.has_stranded_commit(),
                is_plugin: matches!(
                    snapshot.target,
                    hex_session::device_gesture::GestureTarget::PluginParameter { .. }
                ),
            })
        })
        .flatten();
        let blocked_by_edit = if stranded_gesture.is_some() {
            self.edit_pending_excluding_device_gesture()
        } else {
            self.edit_pending()
        };
        if !continuing && (blocked_by_edit || self.audio_pub.pending_restart_request.is_some()) {
            self.error = Some("finish the current edit before restarting audio".into());
            retire_refused(self, &request);
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            retire_refused(self, &request);
            return false;
        };
        let context = AudioSourceContext {
            project_generation: self.project_generation,
            editor_revision: state.revision,
        };
        let current = match kind {
            DeferredEditKind::SampleDrop => self.pending_sample_drop_matches(&request),
            DeferredEditKind::TimeDuplicate => self.pending_time_duplicate_matches(&request),
            DeferredEditKind::LoopRegion => self.pending_loop_matches(&request),
            DeferredEditKind::ClipPlayStart => self.pending_clip_loop_matches(&request),
            DeferredEditKind::NativeEffect
            | DeferredEditKind::PluginEffect
            | DeferredEditKind::RemoveDevice
            | DeferredEditKind::ToggleDeviceBypass
            | DeferredEditKind::MixerControl
            | DeferredEditKind::GraphEdit => {
                request_is_current_for_recovery(&request, state, context)
            }
        };
        if !current {
            self.error = Some("edit canceled · project changed".into());
            retire_refused(self, &request);
            return false;
        }

        if !continuing {
            let playing = self
                .audio_status()
                .is_some_and(|status| status.playing.load(Acquire));
            self.recover_audio();
            if self.audio_pub.recovery.generation().is_none() {
                retire_refused(self, &request);
                return false;
            }
            if kind == DeferredEditKind::TimeDuplicate {
                self.mark_time_duplicate_recovery_started();
            }
            if playing {
                self.defer_play_until_audio_recovers();
            }
        }
        let is_pan = matches!(request, Request::Edit(Command::Balance { .. }));
        let graph_status = deferred_graph_edit_waiting_status(&request);
        self.audio_pub.recovery.deferred_edit = Some(DeferredGraphEdit {
            kind,
            context,
            request: Box::new(request),
            stranded_gesture,
        });
        self.status = match kind {
            DeferredEditKind::NativeEffect => "restarting audio · device will be added when ready",
            DeferredEditKind::PluginEffect => "restarting audio · plug-in will be added when ready",
            DeferredEditKind::RemoveDevice => {
                "restarting audio · device will be removed when ready"
            }
            DeferredEditKind::ToggleDeviceBypass => {
                "restarting audio · device bypass will update when ready"
            }
            DeferredEditKind::SampleDrop => "restarting audio · sample will be imported when ready",
            DeferredEditKind::TimeDuplicate => {
                "restarting audio · time will be duplicated when ready"
            }
            DeferredEditKind::LoopRegion => "restarting audio · loop will change when ready",
            DeferredEditKind::ClipPlayStart => {
                "restarting audio · play start will change when ready"
            }
            DeferredEditKind::MixerControl => {
                if is_pan {
                    "restarting audio · pan will update when ready"
                } else {
                    "restarting audio · volume will update when ready"
                }
            }
            DeferredEditKind::GraphEdit => graph_status,
        }
        .into();
        true
    }

    fn submit_recovered_graph_edit(&mut self) {
        let Some(pending) = self.audio_pub.recovery.deferred_edit.take() else {
            return;
        };
        let sample_drop = pending.kind == DeferredEditKind::SampleDrop;
        let time_duplicate = pending.kind == DeferredEditKind::TimeDuplicate;
        let loop_region = pending.kind == DeferredEditKind::LoopRegion;
        let clip_play_start = pending.kind == DeferredEditKind::ClipPlayStart;
        let remove_device = pending.kind == DeferredEditKind::RemoveDevice;
        let toggle_bypass = pending.kind == DeferredEditKind::ToggleDeviceBypass;
        let plugin_effect = pending.kind == DeferredEditKind::PluginEffect;
        let mixer_control = pending.kind == DeferredEditKind::MixerControl;
        let graph_edit = pending.kind == DeferredEditKind::GraphEdit;
        let is_pan = matches!(
            pending.request.as_ref(),
            Request::Edit(Command::Balance { .. })
        );
        let graph_status = deferred_graph_edit_applying_status(pending.request.as_ref());
        if (sample_drop && !self.pending_sample_drop_matches(&pending.request))
            || (time_duplicate && !self.pending_time_duplicate_matches(&pending.request))
            || (loop_region && !self.pending_loop_matches(&pending.request))
            || (clip_play_start && !self.pending_clip_loop_matches(&pending.request))
            || (mixer_control
                && self.track_gain_controls.has_queued_recovery()
                && !self.pending_mixer_edit_matches(&pending.request))
        {
            return;
        }
        let refusal_error = if time_duplicate && !self.pending_time_duplicate_intent_current() {
            Some("edit canceled · time selection changed during audio recovery")
        } else if pending.context.project_generation != self.project_generation
            || self
                .state
                .as_ref()
                .is_none_or(|state| state.revision != pending.context.editor_revision)
        {
            Some("edit canceled · project changed during audio recovery")
        } else if (sample_drop
            || time_duplicate
            || loop_region
            || clip_play_start
            || remove_device
            || toggle_bypass
            || mixer_control
            || graph_edit)
            && self.edit_pending()
        {
            // A worker edit without an installed reply can already have
            // changed the captured revision for these unversioned requests.
            Some("edit canceled · project work pending during audio recovery")
        } else if self.has_output_fault_requiring_recovery() {
            Some("audio output failed during recovery · edit was canceled")
        } else {
            None
        };
        if let Some(message) = refusal_error {
            self.error = Some(message.into());
            if loop_region {
                self.loop_state_installed(false);
            }
            if mixer_control {
                self.retire_deferred_mixer_edit();
            }
            self.retire_unsubmitted_request(&pending.request, Some(message));
            return;
        }
        if matches!(
            pending.request.as_ref(),
            Request::Edit(Command::ResizeAudioClipView { .. })
        ) {
            self.mark_replayed_trim(&pending.request);
            self.mark_replayed_clip_loop(&pending.request);
        }
        let mut stranded_reconcile_note: Option<&'static str> = None;
        if let Some(stranded) = pending.stranded_gesture
            && stranded.had_stranded_commit
        {
            match (self.state.as_ref(), stranded.reconcilable) {
                (Some(state), Some(snapshot)) if snapshot.manual_edit => {
                    match hex_session::device_gesture::reconcile_command_for_project(
                        &state.project,
                        snapshot,
                    ) {
                        Ok(Some(command)) => {
                            if !self.enqueue_request(Request::Edit(command), false) {
                                stranded_reconcile_note = Some(
                                    "interrupted parameter edit could not be restored · audio restarted",
                                );
                            }
                        }
                        Ok(None) => {
                            stranded_reconcile_note = Some(if stranded.is_plugin {
                                "interrupted plug-in parameter edit was not saved · audio restarted"
                            } else {
                                "interrupted parameter edit was not saved · audio restarted"
                            });
                        }
                        Err(_) => {
                            stranded_reconcile_note = Some(
                                "interrupted parameter edit could not be restored · audio restarted",
                            );
                        }
                    }
                }
                (Some(_), Some(_)) => {}
                _ => {
                    stranded_reconcile_note = Some(if stranded.is_plugin {
                        "interrupted plug-in parameter edit was not saved · audio restarted"
                    } else {
                        "interrupted parameter edit was not saved · audio restarted"
                    });
                }
            }
        }
        if self.enqueue_request(*pending.request, false) {
            self.status = if sample_drop {
                "importing sample to arrangement".into()
            } else if time_duplicate {
                "duplicating time".into()
            } else if loop_region {
                "applying loop".into()
            } else if clip_play_start {
                "applying play start".into()
            } else if remove_device {
                "removing device".into()
            } else if toggle_bypass {
                "updating device bypass".into()
            } else if mixer_control {
                if is_pan {
                    "updating pan".into()
                } else {
                    "updating volume".into()
                }
            } else if plugin_effect {
                "adding plug-in".into()
            } else if graph_edit {
                graph_status.into()
            } else {
                "adding device".into()
            };
        } else if loop_region {
            self.loop_state_installed(false);
        } else if mixer_control {
            self.retire_deferred_mixer_edit();
        }
        if let Some(note) = stranded_reconcile_note
            && self.error.is_none()
        {
            self.error = Some(note.into());
        }
    }

    pub(super) fn defer_play_until_audio_recovers(&mut self) {
        self.audio_pub.deferred_play = Some((
            PlaybackAction::Transport(PlaybackCommand::Play),
            None,
            self.project_generation,
        ));
        self.status = "restarting audio output · playback starts when ready".into();
    }

    /// A contained VST3 worker fault leaves the applied graph playable. Only
    /// recover on Play when the output itself cannot serve the start.
    pub(super) fn recover_degraded_play_at(&mut self, frame: u64) -> bool {
        if self.audio_pub.recovery.generation().is_some()
            || !self.has_degraded_vst3_output()
            || self.audio_status().is_some_and(|status| {
                status.applied_revision.load(Acquire) != 0
                    && !status.device_fault.load(Acquire)
                    && !status.render_fault.load(Acquire)
                    && !status.shutdown_complete.load(Acquire)
            })
        {
            return false;
        }
        self.recover_audio_at(frame);
        if self.audio_pub.recovery.generation().is_some() {
            self.defer_play_until_audio_recovers();
        }
        true
    }

    pub(super) fn recover_audio(&mut self) {
        let position = self.audio_pub.recovery.position().unwrap_or_else(|| {
            self.audio_status()
                .map_or(self.audio_pub.offline_cursor, |status| {
                    status.recovery_position()
                })
        });
        self.recover_audio_at(position);
    }

    pub(super) fn recover_audio_at(&mut self, position: u64) {
        if self.persistence.close_prompt_open
            || self.persistence.opening_project
            || self.persistence.save_dialog_open
            || self.persistence.export_dialog_open
        {
            self.error = Some("finish the current dialog before restarting audio".into());
            return;
        }
        let Some(_state) = self.state.as_ref() else {
            self.error = Some("project is still loading".into());
            return;
        };
        if self.seek_pending() && self.audio_pub.recovery.generation().is_none() {
            self.error =
                Some("seek pending · wait for audio or press stop before restarting".into());
            return;
        }
        self.invalidate_prepared_seek();
        let generation = match self.audio_pub.recovery.begin(position) {
            Ok(generation) => generation,
            Err(message) => {
                self.error = Some(message.into());
                return;
            }
        };
        self.settings.playback.pending = None;
        self.audio_pub.offline_cursor = position;
        if !self.retire_audio_preserving_gain_take() {
            self.clear_recovery();
            return;
        }
        self.clear_audio_accounting();
        if self.restart_audio_for_current_document() {
            self.audio_pub.pending_recovery_generation = Some(generation);
            self.status = "restarting audio output".into();
            self.error = None;
        } else {
            self.fail_audio_recovery("audio restart request was not admitted");
        }
    }

    /// Moves a changed default output route (#4131) through the same recovery
    /// as restart audio: the cursor is kept and the graph restored, and
    /// playback stays stopped. Waits without an error while a dialog, load,
    /// pending seek, restart or recovery is active; drops the request when
    /// the named output is no longer current.
    pub(super) fn follow_output_route(&mut self) -> bool {
        let Some(identity) = self.audio_pub.route_follow else {
            return false;
        };
        if self
            .audio
            .as_ref()
            .and_then(|audio| audio.control.output_identity())
            != Some(identity)
        {
            self.audio_pub.route_follow = None;
            return false;
        }
        if self.persistence.close_prompt_open
            || self.persistence.opening_project
            || self.persistence.save_dialog_open
            || self.persistence.export_dialog_open
            || self.state.is_none()
            || self.seek_pending()
            || self.audio_pub.recovery.generation().is_some()
            || self.audio_pub.pending_restart_request.is_some()
        {
            return false;
        }
        self.audio_pub.route_follow = None;
        self.recover_audio();
        if self.audio_pub.pending_recovery_generation.is_some() {
            self.status = "audio output changed · reopening".into();
        }
        true
    }

    pub(super) fn poll_audio_recovery(&mut self) -> bool {
        false
    }

    pub(super) fn finish_audio_recovery_tick(&mut self) -> bool {
        let Some(recovery_generation) = self.audio_pub.recovery.generation() else {
            return false;
        };
        let reliable = self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.control.has_action_outcomes());
        if reliable && self.audio_pub.recovery.seek_failed() {
            self.fail_audio_recovery("recovered cursor request was not applied · retry available");
            return true;
        }
        let graph_is_current = self
            .state
            .as_ref()
            .is_some_and(|state| self.audio_pub.applied_project_revision == Some(state.revision));
        let failed = self
            .audio
            .as_ref()
            .and_then(|audio| audio.control.status())
            .is_none_or(|status| {
                status.device_fault.load(Relaxed)
                    || status.render_fault.load(Relaxed)
                    || status.shutdown_complete.load(Acquire)
            });
        if failed {
            self.fail_audio_recovery("audio output failed during recovery · retry available");
            return true;
        }
        if let Some(position) = self.audio_pub.recovery.seek_needed(graph_is_current) {
            let prepared = position != 0
                && self.current_seek_requires_preroll() == Some(true)
                && self
                    .audio
                    .as_ref()
                    .is_some_and(|audio| audio.control.worker_owns_graph_publication());
            if prepared {
                // A completed restoration can advance the callback before its
                // protected source metadata reaches this view. Wait for the
                // exact source before capturing another preparation fence.
                let source_ready = self.audio.as_ref().is_some_and(|audio| {
                    audio.control.status().is_some_and(|status| {
                        self.audio_pub
                            .audible_playback
                            .as_ref()
                            .is_some_and(|candidate| {
                                candidate.audio_revision == status.applied_revision.load(Acquire)
                            })
                    })
                });
                if !source_ready
                    || self.audio_pub.pending_playback.is_some()
                    || self.audio_pub.pending_resident_publication.is_some()
                    || !self.audio_pub.playback_candidates.is_empty()
                {
                    return false;
                }
                let Some(generation) = self.audio_pub.recovery.generation() else {
                    return false;
                };
                if let Some(request) = self.request_recovery_prepared_seek(position, generation) {
                    self.audio_pub
                        .recovery
                        .prepared_seeking(generation, request);
                } else {
                    let reason = self.error.as_ref().map_or_else(
                        || "could not prepare recovered cursor · retry available".to_owned(),
                        ToString::to_string,
                    );
                    self.fail_audio_recovery(reason);
                }
                return true;
            } else if let Some(audio) = self.audio.as_mut()
                && let Ok(ticket) = audio
                    .control
                    .command_with_ticket(PlaybackCommand::Seek(position))
            {
                self.audio_pub.recovery.seeking(ticket);
                self.audio_pub.pending_seek_ticket = Some(ticket);
            }
        }
        let Some(status) = self.audio.as_ref().and_then(|audio| audio.control.status()) else {
            return false;
        };
        if self
            .audio_pub
            .recovery
            .prepared_revision()
            .is_some_and(|revision| {
                status
                    .rejected_revision()
                    .is_some_and(|(rejected, _)| rejected == revision)
                    && status.applied_revision.load(Acquire) < revision
            })
        {
            self.fail_audio_recovery("recovered cursor publication was rejected · retry available");
            return true;
        }
        let position = status.position.load(Relaxed);
        let playing = status.playing.load(Relaxed);
        let ready = if self.audio_pub.recovery.prepared_request().is_some() {
            let audible = self
                .audio_pub
                .audible_playback
                .as_ref()
                .and_then(|candidate| {
                    (candidate.project_generation == self.project_generation
                        && self
                            .state
                            .as_ref()
                            .is_some_and(|state| candidate.state.revision == state.revision)
                        && candidate.device_preview_identity.is_none()
                        && candidate.tuning_preview_identity.is_none()
                        && candidate.clip_preview_identity.is_none())
                    .then_some(
                        candidate
                            .seek_request_id
                            .map(|request| (request, candidate.audio_revision)),
                    )
                    .flatten()
                });
            self.audio_pub.recovery.complete_prepared(
                graph_is_current,
                audible,
                status.applied_revision.load(Acquire),
                position,
                playing,
            )
        } else if reliable {
            self.audio_pub
                .recovery
                .complete_exact(graph_is_current, position, playing)
        } else {
            self.audio_pub.recovery.complete(
                graph_is_current,
                if self.audio_pub.document_audio_mode
                    == hex_session::DocumentAudioMode::VerifiedStreaming
                {
                    status.stream_applied_command_prefix.load(Acquire)
                } else {
                    status.consumed_command.load(Acquire)
                },
                position,
                playing,
            )
        };
        if ready {
            self.audio_pub.offline_cursor = position;
            self.audio_pub.pending_recovery_generation = None;
            self.status = "audio ready · stopped".into();
            self.error = None;
            self.submit_recovered_graph_edit();
            self.normal_start_recovery_completed(recovery_generation);
            return true;
        }
        false
    }

    pub(super) fn fail_audio_recovery(&mut self, error: impl Into<hex_audio::OutputOpenError>) {
        self.settings.playback.pending = None;
        let error = error.into();
        let lost_stranded_commit =
            self.audio_pub
                .recovery
                .deferred_edit
                .as_ref()
                .is_some_and(|edit| {
                    edit.stranded_gesture
                        .is_some_and(|stranded| stranded.had_stranded_commit)
                });
        self.audio_pub.offline_cursor = self.cursor_position();
        self.clear_recovery();
        self.retire_audio_preserving_gain_take();
        self.audio_pub.pending_recovery_generation = None;
        self.audio_pub.pending_restart_request = None;
        self.clear_audio_accounting();
        if error.rate_mismatch().is_some() {
            let message = super::audio_unavailable::unavailable_message(&error);
            self.status = message.clone();
            self.error = Some(message.into());
            return;
        }
        self.status = "audio unavailable · restart audio to restore playback".into();
        self.error = Some(if lost_stranded_commit {
            format!("{error} · interrupted parameter edit was not saved").into()
        } else {
            error.to_string().into()
        });
    }

    fn clear_audio_accounting(&mut self) {
        self.output_meter_frame = None;
        self.audio_pub.stopped_playback_restore = None;
        self.audio_pub.paused_republish = None;
        self.audio_pub.deferred_play = None;
        self.service.invalidate_arrangement_seek();
        self.audio_pub.prepared_seek = None;
        self.audio_pub.latched_seek_intent = None;
        self.audio_pub.playback_candidates.clear();
        self.audio_pub.audible_playback = None;
        self.audio_pub.audio_admission.reset_audio();
        self.audio_pub.applied_project_revision = None;
        self.audio_pub.pending_audio_commands.clear();
        self.audio_pub.pending_seek_ticket = None;
        self.session_grid.pending_session_ticket = None;
        self.play_intent.clear();
        self.audio_pub.pending_playback = None;
        self.audio_pub.pending_resident_publication = None;
        self.clear_route_telemetry();
        self.audio_pub.observed_rejected_action = 0;
        self.audio_pub.observed_rejected_revision = 0;
        self.session_grid.observed_capability = None;
        self.session_grid.capability_message = None;
    }

    /// Scheduled cursor: the frame edits, insertion, zoom anchors and transport
    /// commands act on.
    pub(super) fn cursor_position(&self) -> u64 {
        cursor_frame(
            self.audio_pub.recovery.position(),
            self.audio_status(),
            self.audio_pub.offline_cursor,
            Cursor::Scheduled,
        )
    }

    /// Display-only playhead: the frame now leaving Main output, which trails
    /// the scheduled cursor by the graph's Main latency.
    pub(super) fn audible_cursor_position(&self) -> u64 {
        cursor_frame(
            self.audio_pub.recovery.position(),
            self.audio_status(),
            self.audio_pub.offline_cursor,
            Cursor::Audible,
        )
    }
}

#[derive(Clone, Copy)]
enum Cursor {
    Scheduled,
    Audible,
}

/// A pending recovery position wins, then the live callback status, then the
/// offline cursor.
fn cursor_frame(
    recovery: Option<u64>,
    status: Option<&hex_audio::prepared::Status>,
    offline: u64,
    cursor: Cursor,
) -> u64 {
    recovery.unwrap_or_else(|| {
        status.map_or(offline, |status| match cursor {
            Cursor::Scheduled => status.position.load(Relaxed),
            Cursor::Audible => status.audible_position.load(Relaxed),
        })
    })
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod cursor_tests {
    use super::{Cursor, cursor_frame};
    use std::sync::atomic::Ordering::Relaxed;

    #[test]
    fn playhead_shows_the_audible_frame_while_edits_use_the_scheduled_one() {
        let status = hex_audio::prepared::Status::default();
        status.position.store(960, Relaxed);
        status.audible_position.store(672, Relaxed);
        assert_eq!(cursor_frame(None, Some(&status), 0, Cursor::Audible), 672);
        assert_eq!(cursor_frame(None, Some(&status), 0, Cursor::Scheduled), 960);
        // Recovery and offline cursors are not latency-shifted.
        for cursor in [Cursor::Audible, Cursor::Scheduled] {
            assert_eq!(cursor_frame(Some(48), Some(&status), 0, cursor), 48);
            assert_eq!(cursor_frame(None, None, 96, cursor), 96);
        }
    }
}
