//! The editor's single request-submission table (#3860).
//!
//! Both the immediate `enqueue_request` path and the queued drain in `tick`
//! submit through [`EditorView::submit_to_service`]. It classifies which
//! editor-owned pending custodies a request matches *before* the request moves
//! into `Service::submit`, then stamps them on admission. A refusal that drops
//! the request (`Busy`, `Closed`, or `Full` with no UI queue room) retires the
//! same custodies through [`EditorView::on_request_refused`], so a refused
//! request leaves identical pending state whichever path submitted it.
//! A retryable `Full` keeps its custodies; the request is classified again
//! when it is resubmitted.
//!
//! Intents installed only after `enqueue_request` returns (the created-note
//! selection, which binds its tuning terminal after admission) are matched
//! here when they already exist, which covers the queued drain; the immediate
//! path stamps them where they are installed.
use super::super::truncated_import::ImportIntent;
use super::{EditorView, Request, SubmitError, clear_rejected_scene_duplicate, replace_error};
use hex_session::service::BusyReason;

/// Editor-owned pending custodies one request matches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PendingKinds {
    /// Read-only host observations are polled but never block editor gestures.
    parameter_observation: bool,
    trim: bool,
    loop_region: bool,
    group_copy: bool,
    group_selection: bool,
    clip_detail: bool,
    audio_events: bool,
    clip_loop: bool,
    bypass: Option<hex_project::DeviceId>,
    modulation_depth: bool,
    clip_move: bool,
    clip_paint: bool,
    section: bool,
    track_reorder: bool,
    duplicate: bool,
    track_clipboard: bool,
    device_duplicate: bool,
    midi_drag: bool,
    fade: bool,
    locator_move: bool,
    locator_add: Option<u64>,
    clip_gain: bool,
    clip_activation: bool,
    gain: bool,
    gain_take: bool,
    device_automation: bool,
    time_copy: bool,
    time_duplicate: bool,
    sample_drop: bool,
    project_rate: bool,
}

/// One request's classification, captured before submission moves it.
pub(super) struct Submission {
    pub(super) kinds: PendingKinds,
    import_intent: Option<ImportIntent>,
}

/// Why a request will never reach the project worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    Busy(BusyReason),
    Closed,
    /// The service queue was full and the UI queue had no room to retry.
    QueueFull,
}

/// A service refusal with the classification of the refused request.
pub(super) struct Refused {
    pub(super) error: SubmitError,
    pub(super) kinds: PendingKinds,
}

fn is_initial_observation(request: &Request) -> bool {
    request.is_read_only_observation()
        && !matches!(
            request,
            Request::ContinuePluginParameters | Request::ContinuePluginParameterConversion
        )
}

impl EditorView {
    pub(super) fn classify_submission(&self, request: &Request) -> Submission {
        Submission {
            kinds: PendingKinds {
                parameter_observation: is_initial_observation(request),
                trim: self.pending_trim_matches(request),
                loop_region: self.pending_loop_matches(request),
                group_copy: self.pending_group_copy_matches(request),
                group_selection: self.pending_group_selection_matches(request),
                clip_detail: self.audio_clip_controls.matches(request),
                audio_events: self.pending_audio_events_matches(request),
                clip_loop: self.pending_clip_loop_matches(request),
                bypass: match request {
                    Request::Edit(hex_project::Command::ToggleDeviceBypass { device })
                        if self.pending_bypass_matches(request) =>
                    {
                        Some(*device)
                    }
                    _ => None,
                },
                modulation_depth: self.pending_modulation_depth_matches(request),
                clip_move: self.pending_clip_move_matches(request),
                clip_paint: self.pending_clip_paint_matches(request),
                section: self.pending_section_matches(request),
                track_reorder: self.pending_track_reorder_matches(request),
                duplicate: self.pending_duplicate_matches(request),
                track_clipboard: self.pending_track_clipboard_matches(request),
                device_duplicate: self.device_panel.pending_duplicate_matches(request),
                midi_drag: self.pending_midi_drag_matches(request),
                fade: self.pending_fade_matches(request),
                locator_move: self.locators.matches(request),
                locator_add: self.locators.queued_add_frame(request),
                clip_gain: self.clip_gain_matches(request),
                clip_activation: self.clip_activation_matches(request),
                gain: self.pending_gain_matches(request),
                gain_take: self.pending_gain_take_matches(request),
                device_automation: self.pending_device_automation_matches(request),
                time_copy: self.pending_time_copy_matches(request),
                time_duplicate: self.pending_time_duplicate_matches(request),
                sample_drop: self.pending_sample_drop_matches(request),
                project_rate: self.project_rate_settings.matches(request),
            },
            import_intent: ImportIntent::capture(request),
        }
    }

    /// Classify, submit and stamp one request. On refusal the original request
    /// returns with its classification; the caller requeues a retryable `Full`
    /// or passes every dropped request to [`Self::on_request_refused`].
    #[allow(clippy::result_large_err)] // The refused request returns intact for retry.
    pub(super) fn submit_to_service(&mut self, request: Request) -> Result<(), Refused> {
        let submission = self.classify_submission(&request);
        match self.service.submit(request) {
            Ok(()) => {
                self.on_request_submitted(submission);
                Ok(())
            }
            Err(error) => Err(Refused {
                error,
                kinds: submission.kinds,
            }),
        }
    }

    fn on_request_submitted(&mut self, submission: Submission) {
        let Submission {
            kinds,
            import_intent,
        } = submission;
        if kinds.parameter_observation {
            self.wake_poll();
        } else {
            self.worker_submitted();
        }
        if kinds.project_rate {
            self.project_rate_settings
                .submitted(self.worker_requests_pending);
        }
        if kinds.sample_drop {
            self.sample_drop_submitted();
        }
        self.truncated_import_submitted(import_intent);
        if kinds.time_copy {
            self.time_copy_submitted();
        }
        if kinds.time_duplicate {
            self.time_duplicate_submitted();
        }
        if kinds.trim {
            self.trim_request_submitted();
        }
        if kinds.loop_region {
            self.loop_request_submitted();
        }
        if kinds.group_copy {
            self.group_copy_submitted();
        }
        if kinds.group_selection {
            self.group_selection_submitted();
        }
        if kinds.clip_detail {
            self.audio_clip_controls
                .submitted(self.worker_requests_pending);
        }
        if kinds.audio_events {
            self.audio_events_submitted();
        }
        if kinds.clip_loop {
            self.clip_loop_submitted();
        }
        if let Some(device) = kinds.bypass {
            self.bypass_submitted(&Request::Edit(hex_project::Command::ToggleDeviceBypass {
                device,
            }));
        }
        if kinds.modulation_depth {
            self.modulation_depth_submitted();
        }
        if kinds.clip_move {
            self.clip_move_submitted();
        }
        if kinds.clip_paint {
            self.clip_paint_submitted();
        }
        if kinds.section {
            self.section_submitted();
        }
        if kinds.track_reorder {
            self.track_reorder_submitted();
        }
        if kinds.track_clipboard {
            self.track_clipboard_submitted();
        }
        if kinds.device_duplicate {
            self.device_panel
                .duplicate_submitted(self.worker_requests_pending);
        }
        if kinds.duplicate {
            self.duplicate_submitted();
        }
        if kinds.midi_drag {
            self.midi_drag_submitted();
        }
        if kinds.locator_move {
            self.locators.submitted(self.worker_requests_pending);
        }
        if let Some(frame) = kinds.locator_add {
            self.locators
                .add_submitted(frame, self.worker_requests_pending);
        }
        if kinds.fade {
            self.fade_submitted();
        }
        if kinds.clip_gain {
            self.clip_gain_submitted();
        }
        if kinds.clip_activation {
            self.clip_activation_submitted();
        }
        if kinds.gain {
            self.gain_submitted();
        }
        if kinds.gain_take {
            self.gain_take_submitted();
        }
        if kinds.device_automation {
            self.device_automation_submitted();
        }
    }

    /// Retire every custody owned by a request that will never reach the
    /// worker and show the refusal's actual reason. Returns whether visible
    /// editor state changed.
    pub(super) fn on_request_refused(
        &mut self,
        kinds: PendingKinds,
        request: &Request,
        refusal: Refusal,
    ) -> bool {
        let mut visible = false;
        if matches!(request, Request::RestoreStoppedPlayback) {
            visible |= self.stopped_playback_restore.take().is_some();
        }
        if kinds.sample_drop {
            visible |= self.pending_sample_drop.take().is_some();
        }
        self.midi_take_not_submitted(request);
        self.gain_take_not_submitted(request);
        self.native_automation_request_not_submitted(request, refusal == Refusal::Closed);
        self.automation_take_recovery_not_submitted(request, refusal == Refusal::Closed);
        if refusal == Refusal::Closed {
            visible |= self.cancel_new_preparation();
        }
        self.retire_unsubmitted_export(request);
        if let Request::BounceArrangement { control, .. } = request
            && self
                .bounce_control
                .as_ref()
                .is_some_and(|active| active.same_job(control))
        {
            visible |= self.abandon_bounce();
        }
        clear_rejected_scene_duplicate(&mut self.pending_scene_duplicate, request);
        if matches!(request, Request::ImportAudioBatch { .. }) {
            if let Some(control) = self.audio_batch.take() {
                control.request_cancel();
            }
            self.audio_batch_progress = None;
        }
        if matches!(
            request,
            Request::SaveNew(_)
                | Request::SaveProjectAs { .. }
                | Request::SaveCurrent
                | Request::SaveCurrentWithFormatUpgrade { .. }
        ) {
            self.saving_project = false;
            self.cancel_open_after_save();
        }
        if request.is_project_open() {
            self.opening_project = false;
        }
        if kinds.time_copy {
            self.pending_created_note_selection = None;
        }
        if kinds.time_duplicate {
            visible |= self.refuse_time_duplicate(request);
        }
        if kinds.trim {
            visible |= self.retire_pending_trim();
        }
        if kinds.loop_region {
            visible |= self.pending_loop.take().is_some();
        }
        if kinds.group_copy {
            visible |= self.pending_group_copy.take().is_some();
        }
        if kinds.group_selection {
            self.retire_group_selection();
        }
        if kinds.audio_events {
            visible |= self.retire_pending_audio_events();
        }
        if kinds.clip_loop {
            visible |= self.retire_pending_clip_loop();
        }
        if kinds.bypass.is_some() {
            visible |= self.retire_unsubmitted_bypass(request);
        }
        if kinds.modulation_depth {
            visible |= self.retire_pending_modulation_depth();
        }
        if kinds.clip_move {
            visible |= self.retire_pending_clip_move();
        }
        if kinds.clip_paint {
            visible |= self.retire_pending_clip_paint();
        }
        if kinds.section {
            visible |= self.refuse_section(request);
        }
        if kinds.track_reorder {
            visible |= self.retire_pending_track_reorder();
        }
        if kinds.track_clipboard {
            visible |= self.pending_track_clipboard.take().is_some();
        }
        if kinds.device_duplicate {
            visible |= self.device_panel.pending_duplicate.take().is_some();
        }
        if kinds.duplicate {
            visible |= self.duplicate_intent.reject();
        }
        if kinds.midi_drag {
            visible |= self.retire_pending_midi_drag();
        }
        if kinds.fade {
            visible |= self.retire_pending_fade();
        }
        if kinds.locator_move {
            visible |= self.locators.refuse();
        }
        if let Some(frame) = kinds.locator_add {
            visible |= self.locators.refuse_add(frame);
        }
        if kinds.clip_gain {
            visible |= self.retire_clip_gain();
        }
        if kinds.clip_activation {
            visible |= self.retire_clip_activation();
        }
        if kinds.gain {
            visible |= self.retire_pending_gain();
        }
        if kinds.device_automation {
            visible |= self.retire_pending_device_automation();
        }
        let message = match refusal {
            Refusal::Busy(reason) => super::super::user_errors::busy(reason),
            Refusal::Closed => "project worker stopped",
            Refusal::QueueFull => "request queue full · edit was not applied",
        };
        if let Request::InsertPluginEffect(insertion) = request {
            visible |= self.retire_plugin_effect(&insertion.control, false, Some(message));
        }
        if kinds.project_rate {
            self.project_rate_settings.refused(message.into());
            visible = true;
        }
        visible |= replace_error(&mut self.error, message);
        visible
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;

#[cfg(all(test, not(feature = "ui-test-support")))]
mod observation_tests {
    use super::*;

    #[test]
    fn recovery_reads_do_not_count_as_editor_mutations() {
        assert!(is_initial_observation(
            &Request::QueryFailedPluginAutomation { request_id: 7 }
        ));
        assert!(!is_initial_observation(&Request::ContinuePluginParameters));
        assert!(!is_initial_observation(
            &Request::ContinuePluginParameterConversion
        ));
        assert!(!is_initial_observation(&Request::Undo));
    }
}
