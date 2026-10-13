#[cfg(test)]
use super::EnqueuedNativeDeviceDrop;
use super::duplicate::Press;
use super::play_intent::AdmissionError;
use super::refresh::AudioFrame;
use super::seek::seek_is_pending;
use super::selection;
use super::viewport::Viewport;
use super::{
    EditorView, PendingPlayback, PendingPlaybackAction, Playback, PlaybackCandidate,
    UI_EVENT_BUDGET, UI_REQUEST_CAPACITY,
};
use gpui::Context;
use hex_audio::prepared::PublicationPolicy;
use hex_engine::playback::{PlaybackAction, PlaybackCommand};
use hex_session::device_gesture::GestureKey;
use hex_session::service::{DevicePreviewIdentity, Event, Request, SubmitError};
use std::{
    collections::VecDeque,
    sync::Arc,
    sync::atomic::Ordering::{Acquire, Relaxed},
    time::Instant,
};

#[cfg(all(test, feature = "ui-test-support"))]
mod project_replacement_tests;

impl EditorView {
    pub(super) fn clear_route_telemetry(&mut self) {
        self.session_routes.clear();
    }

    pub(super) fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        let logging_failure = crate::diagnostics::record_state(cx, &self.status, &self.error);
        let logging_changed = if let Some(failure) = logging_failure {
            let message = self
                .error
                .as_ref()
                .map_or_else(|| failure.clone(), |error| format!("{error} · {failure}"));
            replace_error(&mut self.error, &message)
        } else {
            false
        };
        let pending_before = self
            .worker_requests_pending
            .saturating_add(self.pending_requests.len());
        let mut visible_change = self.poll_audio_recovery() || logging_changed;
        for _ in 0..UI_EVENT_BUDGET {
            match self.service.try_event() {
                Ok(event) => {
                    if let Some(path) = self.receive(event) {
                        self.record_opened_project(path, cx);
                    }
                    visible_change = true;
                }
                Err(_) => break,
            }
        }
        visible_change |= self.poll_export();
        while let Some(request) = self.pending_requests.pop_front() {
            match self.service.submit(request) {
                Ok(()) => {
                    self.worker_requests_pending = self.worker_requests_pending.saturating_add(1)
                }
                Err(SubmitError::Full(request)) => {
                    self.pending_requests.push_front(request);
                    break;
                }
                Err(SubmitError::Busy(request)) => {
                    if matches!(request, Request::Export(_)) {
                        self.export_pending = false;
                    }
                    clear_rejected_scene_duplicate(&mut self.pending_scene_duplicate, &request);
                    visible_change |=
                        replace_error(&mut self.error, "an export is already pending");
                }
                Err(SubmitError::Closed(request)) => {
                    if matches!(request, Request::Export(_)) {
                        self.export_pending = false;
                    }
                    if matches!(request, Request::SaveNew(_) | Request::SaveCurrent) {
                        self.saving_project = false;
                    }
                    visible_change |= replace_error(&mut self.error, "project worker stopped");
                    if request.is_project_open() {
                        self.opening_project = false;
                    }
                    clear_rejected_scene_duplicate(&mut self.pending_scene_duplicate, &request);
                    if matches!(
                        &request,
                        Request::Edit(hex_project::Command::DuplicateClip(_))
                    ) {
                        self.duplicate_intent.reject();
                    }
                    break;
                }
            }
        }
        visible_change |= self.pump_device_gesture();
        let now = Instant::now();
        visible_change |= self.tick_time_gesture_autoscroll(now);
        visible_change |= self.tick_edit_cursor(now);
        let project_edit_pending = self.edit_pending();
        visible_change |= self.retire_stale_spectrum_target();
        visible_change |= self.retire_stale_processor_audition_target();
        if let Some(audio) = self.audio.as_mut() {
            let _ = audio.control.spectrum_target.retry_pending();
            let _ = audio.control.processor_audition.retry_pending();
            let desired_spectrum = audio.control.spectrum_target.desired();
            let spectrum = audio
                .spectrum
                .as_mut()
                .and_then(|reader| reader.try_latest());
            let spectrum = spectrum.filter(|result| {
                desired_spectrum.is_some_and(|target| {
                    result.stamp.target == target.token
                        && result.stamp.generation == target.project_generation
                        && result.stamp.sample_rate_hz == target.sample_rate_hz
                        && result.stamp.applied_revision >= target.activation_floor
                })
            });
            if self.device_panel.spectrum_result != spectrum {
                self.device_panel.spectrum_result = spectrum;
                visible_change = true;
            }
            audio.control.reclaim();
        } else if self.device_panel.spectrum_result.take().is_some() {
            visible_change = true;
        }
        if (self.prepared_seek.is_some() || self.latched_seek_intent.is_some())
            && self
                .audio
                .as_ref()
                .is_some_and(|audio| !super::arrangement_seek::audio_ready_for_seek(&audio.control))
        {
            self.invalidate_prepared_seek();
        }
        visible_change |= self.tick_prepared_arrangement_seek();
        let Some(audio) = self.audio.as_mut() else {
            let pending_after = self
                .worker_requests_pending
                .saturating_add(self.pending_requests.len());
            visible_change |= pending_before != pending_after;
            return self
                .redraw_gate
                .should_redraw(visible_change, None, Instant::now());
        };
        let seek_publication_inflight = self.prepared_seek.as_ref().is_some_and(|seek| {
            matches!(
                seek.stage,
                super::arrangement_seek::SeekStage::Published { .. }
            )
        });
        if !seek_publication_inflight
            && self.playback_candidates.len() < super::PLAYBACK_CANDIDATE_CAPACITY
            && let Some(pending) = self.pending_playback.take()
        {
            let prepared_revision = pending
                .prepared_revision
                .clone()
                .or_else(|| pending.playback.prepared_revision().cloned());
            let seek_request_id = pending.seek_request_id;
            let policy = if let Some(fence) = pending.fresh_runtime_fence {
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence)
            } else if pending.install_fresh_runtime {
                PublicationPolicy::InstallFreshRuntime
            } else {
                PublicationPolicy::InheritRuntime
            };
            match audio.control.publish_with_policy_and_context_generation(
                pending.playback,
                policy,
                pending.project_generation,
            ) {
                Ok(audio_revision) => {
                    let project_revision = super::audio_admission::ProjectRevision {
                        generation: pending.project_generation,
                        revision: pending.project_revision,
                    };
                    if pending.device_preview_identity.is_none() && seek_request_id.is_none() {
                        visible_change |= self
                            .audio_admission
                            .published(project_revision, audio_revision);
                    }
                    self.playback_candidates.push_back(PlaybackCandidate {
                        audio_revision,
                        project_generation: pending.project_generation,
                        state: pending.state,
                        prepared_revision,
                        seek_request_id,
                        device_preview_identity: pending.device_preview_identity,
                    });
                    if let Some(seek) = self.prepared_seek.as_mut()
                        && seek_request_id == Some(seek.request_id)
                    {
                        seek.stage =
                            super::arrangement_seek::SeekStage::Published { audio_revision };
                    }
                }
                Err(rejected) => {
                    self.pending_playback = Some(PendingPlayback {
                        playback: rejected.playback,
                        ..pending
                    });
                    if rejected.reason != "playback publication queue full"
                        && rejected.reason != "playback publication already pending"
                        && self.error.as_deref() != Some(rejected.reason)
                    {
                        let message = rejected.reason.to_lowercase();
                        visible_change |= replace_error(&mut self.error, &message);
                    }
                }
            }
        } else if self.playback_candidates.len() >= super::PLAYBACK_CANDIDATE_CAPACITY {
            visible_change |= replace_status(
                &mut self.status,
                "audio graph queue full · waiting for callback",
            );
        }
        if let Some((ticket, reason)) = audio.control.status.rejected_action()
            && ticket > self.observed_rejected_action
        {
            self.observed_rejected_action = ticket;
            if self.pending_seek_ticket == Some(ticket) {
                self.pending_seek_ticket = None;
            }
            visible_change |= replace_error(&mut self.error, reason.message());
        }
        if let Some(capability) = audio.control.status.session_capability() {
            let code = capability as u32;
            if self.observed_session_capability != Some(code) {
                let previous_message = self.session_capability_message.as_deref();
                self.observed_session_capability = Some(code);
                if let Some(message) = capability.message() {
                    let message = self
                        .session_capability_message
                        .as_deref()
                        .unwrap_or(message);
                    visible_change |= replace_error(&mut self.error, message);
                } else if previous_message
                    .is_some_and(|previous| self.error.as_deref() == Some(previous))
                {
                    self.error = None;
                    visible_change = true;
                }
            }
        }
        // Sample terminal rejection telemetry before the applied high-water
        // mark. The callback may publish both between UI polls; loading the
        // rejection snapshot first and then acquiring `applied_revision` lets
        // accepted work win accounting before older coalesced rejections prune
        // the remaining pending revisions.
        let active_gesture = (!self.device_gesture.has_pending_terminal())
            .then(|| self.device_gesture.snapshot())
            .flatten()
            .map(|snapshot| (snapshot.key, snapshot.sequence));
        let rejected_revision = audio
            .control
            .status
            .rejected_revision()
            .filter(|(revision, _)| *revision > self.observed_rejected_revision);
        if let Some((audio_revision, reason)) = rejected_revision {
            // Resolve the exact project-generation mapping before any applied
            // high-water reconciliation removes candidate metadata. Callback
            // telemetry is audio-revision-only, so stale/coalesced rejection
            // may describe an earlier project generation.
            if let Some(project) =
                rejected_candidate_project(&self.playback_candidates, audio_revision)
            {
                visible_change |=
                    self.audio_admission
                        .rejected(project, audio_revision, reason.message());
            } else if rejected_preview_candidate(
                &self.playback_candidates,
                audio_revision,
                self.project_generation,
                self.state.as_ref().map(|state| state.revision),
                active_gesture,
            ) {
                // Preview failures are transient, but remain visible even
                // though they do not change the canonical admission footer.
                visible_change |= replace_error(
                    &mut self.error,
                    &format!("preview rejected: {}", reason.message()),
                );
            }
        }
        let applied = audio.control.status.applied_revision.load(Acquire);
        if let Some((project, audio_revision)) = latest_applied_canonical_candidate(
            &self.playback_candidates,
            applied,
            rejected_revision.map(|(revision, _)| revision),
        ) {
            visible_change |= self.audio_admission.applied(project, audio_revision);
        }
        let audible = promote_playback_candidates(
            &mut self.playback_candidates,
            applied,
            rejected_revision.map(|(revision, _)| revision),
        );
        if let Some(candidate) = audible {
            let preview_source_is_valid =
                candidate.device_preview_identity.is_none_or(|identity| {
                    preview_source_is_current(
                        identity,
                        candidate.project_generation,
                        candidate.state.revision,
                        self.project_generation,
                        self.state.as_ref().map(|state| state.revision),
                        active_gesture,
                    )
                });
            if preview_source_is_valid {
                if candidate.project_generation == self.project_generation
                    && candidate.seek_request_id.is_none()
                {
                    self.applied_project_revision = Some(candidate.state.revision);
                }
                self.audible_playback = Some(candidate);
            } else {
                visible_change |= replace_error(
                    &mut self.error,
                    "device preview source did not match its prepared revision",
                );
            }
        }
        if let Some((revision, _)) = rejected_revision {
            self.observed_rejected_revision = revision;
            self.playback_candidates
                .retain(|candidate| candidate.audio_revision > revision);
        }
        visible_change |= super::arrangement_seek::resolve_prepared_seek_terminal(
            &mut self.prepared_seek,
            &mut self.status,
            &mut self.error,
            applied,
            rejected_revision,
        );
        while let Some(pending) = self.pending_audio_commands.pop_front() {
            let action_is_current = queued_action_is_current(
                pending,
                project_edit_pending,
                self.project_generation,
                self.state.as_ref().map(|state| state.project.as_ref()),
                self.state.as_ref().map(|state| state.revision),
            );
            if !action_is_current {
                if let Some(token) = pending.intent_token {
                    self.play_intent.clear_if_current(token);
                }
                if matches!(
                    pending.action,
                    PlaybackAction::Session(_) | PlaybackAction::LaunchAndPlay(_)
                ) {
                    replace_status(&mut self.status, "session request cancelled");
                }
                visible_change |= replace_error(
                    &mut self.error,
                    "session action became stale · select the cell and try again",
                );
                continue;
            }
            match audio.control.action_with_ticket(pending.action) {
                Ok(ticket) => {
                    if matches!(
                        pending.action,
                        PlaybackAction::Transport(
                            PlaybackCommand::Seek(_) | PlaybackCommand::PlayFrom(_)
                        )
                    ) {
                        self.pending_seek_ticket = Some(ticket);
                    }
                    if matches!(
                        pending.action,
                        PlaybackAction::Session(_) | PlaybackAction::LaunchAndPlay(_)
                    ) {
                        self.pending_session_ticket = Some((ticket, pending.generation));
                    }
                    if let Some(token) = pending.intent_token {
                        self.play_intent.attach_ticket(token, ticket);
                    }
                }
                Err(action) => {
                    self.pending_audio_commands
                        .push_front(PendingPlaybackAction { action, ..pending });
                    break;
                }
            }
        }
        let consumed_command = audio.control.status.consumed_command.load(Acquire);
        let actual_playing = audio.control.status.playing.load(Acquire);
        let rejected_action_ticket = audio
            .control
            .status
            .rejected_action()
            .map(|(ticket, _)| ticket);
        self.play_intent
            .reconcile(consumed_command, actual_playing, rejected_action_ticket);
        let (pending_session_ticket, session_outcome) = reconcile_session_ticket(
            self.pending_session_ticket,
            self.project_generation,
            consumed_command,
            rejected_action_ticket,
        );
        self.pending_session_ticket = pending_session_ticket;
        match session_outcome {
            Some(SessionTicketOutcome::Consumed) => {
                visible_change |= replace_status(&mut self.status, "session request consumed");
            }
            Some(SessionTicketOutcome::Rejected) => {
                visible_change |= replace_status(&mut self.status, "session request rejected");
            }
            None => {}
        }
        if self
            .pending_seek_ticket
            .is_some_and(|ticket| consumed_command >= ticket)
        {
            self.pending_seek_ticket = None;
            visible_change = true;
            replace_status(&mut self.status, "cursor ready");
        }
        if audio.control.status.device_fault.load(Relaxed) {
            self.session_routes.clear();
            self.pending_audio_commands.clear();
            self.pending_session_ticket = None;
            self.pending_seek_ticket = None;
            self.play_intent.clear();
            visible_change |= replace_status(&mut self.status, "audio device unavailable");
            visible_change |= replace_error(
                &mut self.error,
                "audio device failed · editing and saving remain available",
            );
        } else if audio.control.status.render_fault.load(Relaxed) {
            self.session_routes.clear();
            self.pending_audio_commands.clear();
            self.pending_seek_ticket = None;
            self.pending_session_ticket = None;
            self.play_intent.clear();
            visible_change |= replace_status(&mut self.status, "audio render fault · stopped");
            let diagnostic = audio.control.status.render_fault_snapshot().map_or_else(
                || "audio render fault · stopped · edit or undo to recover".to_owned(),
                |fault| {
                    format!(
                        "audio stopped · {}{} · frame {} · audio rev {} · edit or undo to recover",
                        fault.message,
                        if fault.truncated { " [truncated]" } else { "" },
                        fault.frame,
                        fault.audio_revision
                    )
                },
            );
            visible_change |= replace_error(&mut self.error, &diagnostic);
        }
        if !audio.control.status.device_fault.load(Relaxed)
            && !audio.control.status.render_fault.load(Relaxed)
            && !audio.control.status.shutdown_complete.load(Acquire)
        {
            visible_change |= self.session_routes.poll(Arc::clone(&audio.control.status));
        } else {
            self.session_routes.clear();
        }
        let frame = AudioFrame {
            playing: audio.control.status.playing.load(Relaxed),
            position: audio.control.status.position.load(Relaxed),
            peak_left: audio.control.status.peak_left.load(Relaxed),
            peak_right: audio.control.status.peak_right.load(Relaxed),
            applied_revision: audio.control.status.applied_revision.load(Relaxed),
            deadline_misses: audio.control.status.deadline_misses.load(Relaxed),
            callback_us: audio.control.status.callback_max_ns.load(Relaxed) / 1_000,
            render_fault: audio.control.status.render_fault.load(Relaxed),
        };
        let pending_after = self
            .worker_requests_pending
            .saturating_add(self.pending_requests.len());
        visible_change |= pending_before != pending_after;
        visible_change |= self.finish_audio_recovery_tick();
        self.redraw_gate
            .should_redraw(visible_change, Some(frame), Instant::now())
    }

    fn spectrum_target_is_visible_and_current(
        &self,
        target: hex_audio::spectrum_target::SpectrumTarget,
    ) -> bool {
        if !self.clip_view_open
            || !self.device_panel_active
            || self.project_generation != target.project_generation
            || target.identity.device_type != hex_engine::DeviceTypeId::from_static("hex.eq8")
        {
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let focused_owner = self
            .focused_return
            .filter(|id| {
                state
                    .project
                    .return_tracks()
                    .iter()
                    .any(|track| track.id() == *id)
            })
            .map(super::device_panel::DevicePanelOwner::Return)
            .or_else(|| {
                self.selected_track
                    .map(super::device_panel::DevicePanelOwner::Track)
            });

        match (focused_owner, target.identity.owner) {
            (
                Some(super::device_panel::DevicePanelOwner::Track(track_id)),
                hex_engine::ProcessorChainOwner::Track(owner_id),
            ) if track_id.value() == owner_id => {
                state.project.track(track_id).is_some_and(|track| {
                    track.devices().iter().any(|device| {
                        device.id().value() == target.identity.device_id
                            && device.state().type_id() == target.identity.device_type
                    })
                })
            }
            (
                Some(super::device_panel::DevicePanelOwner::Return(return_id)),
                hex_engine::ProcessorChainOwner::Return(owner_id),
            ) if return_id.value() == owner_id => state
                .project
                .return_tracks()
                .iter()
                .find(|track| track.id() == return_id)
                .is_some_and(|track| {
                    track.devices().iter().any(|device| {
                        device.id().value() == target.identity.device_id
                            && device.state().type_id() == target.identity.device_type
                    })
                }),
            _ => false,
        }
    }

    fn retire_stale_spectrum_target(&mut self) -> bool {
        let target = self
            .audio
            .as_ref()
            .and_then(|audio| audio.control.spectrum_target.desired());
        if target.is_some_and(|target| !self.spectrum_target_is_visible_and_current(target)) {
            if let Some(audio) = self.audio.as_mut() {
                let _ = audio.control.spectrum_target.clear();
            }
            self.device_panel.spectrum_result = None;
            return true;
        }
        if target.is_none() && self.device_panel.spectrum_result.take().is_some() {
            return true;
        }
        false
    }

    fn processor_audition_target_is_visible_and_current(
        &self,
        target: hex_audio::processor_audition::ProcessorAuditionTarget,
    ) -> bool {
        if !self.clip_view_open
            || !self.device_panel_active
            || self.project_generation != target.project_generation
            || target.identity.device_type != hex_engine::DeviceTypeId::from_static("hex.eq8")
        {
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let focused_owner = self
            .focused_return
            .filter(|id| {
                state
                    .project
                    .return_tracks()
                    .iter()
                    .any(|track| track.id() == *id)
            })
            .map(super::device_panel::DevicePanelOwner::Return)
            .or_else(|| {
                self.selected_track
                    .map(super::device_panel::DevicePanelOwner::Track)
            });
        match (focused_owner, target.identity.owner) {
            (
                Some(super::device_panel::DevicePanelOwner::Track(track_id)),
                hex_engine::ProcessorChainOwner::Track(owner_id),
            ) if track_id.value() == owner_id => {
                state.project.track(track_id).is_some_and(|track| {
                    track.devices().iter().any(|device| {
                        device.id().value() == target.identity.device_id
                            && device.state().type_id() == target.identity.device_type
                    })
                })
            }
            (
                Some(super::device_panel::DevicePanelOwner::Return(return_id)),
                hex_engine::ProcessorChainOwner::Return(owner_id),
            ) if return_id.value() == owner_id => state
                .project
                .return_tracks()
                .iter()
                .find(|track| track.id() == return_id)
                .is_some_and(|track| {
                    track.devices().iter().any(|device| {
                        device.id().value() == target.identity.device_id
                            && device.state().type_id() == target.identity.device_type
                    })
                }),
            _ => false,
        }
    }

    fn retire_stale_processor_audition_target(&mut self) -> bool {
        let target = self
            .audio
            .as_ref()
            .and_then(|audio| audio.control.processor_audition.desired());
        if target
            .is_some_and(|target| !self.processor_audition_target_is_visible_and_current(target))
        {
            if let Some(audio) = self.audio.as_mut() {
                let _ = audio.control.processor_audition.cancel_immediately();
            }
            super::device_panel::clear_eq8_audition_gesture_flags(self);
            return true;
        }
        false
    }

    fn receive(&mut self, event: Event) -> Option<std::path::PathBuf> {
        if event.save_complete {
            self.saving_project = false;
        }
        if event.request_complete {
            self.worker_requests_pending = self.worker_requests_pending.saturating_sub(1);
        }
        if event.open_complete {
            self.opening_project = false;
            self.pending_open_path = None;
            self.duplicate_intent.cancel_chain();
        }
        if event
            .restore_complete
            .is_some_and(|generation| self.recovery.generation() != Some(generation))
        {
            return None;
        }
        let successfully_opened_path =
            (event.open_complete && event.project_replaced && event.error.is_none())
                .then(|| event.state.document_path.clone())
                .flatten();
        if let Some(generation) = event.restore_complete {
            if let Some(error) = &event.error {
                self.fail_audio_recovery(error.clone());
            } else {
                self.recovery.restored(generation);
            }
        }
        // Scene duplication is admitted only with no pending worker request, so
        // its next completion is the exact response for this bounded intent.
        let scene_duplicate_intent = if event.request_complete && !event.gesture_request {
            self.pending_scene_duplicate.take()
        } else {
            None
        };
        let event_state = Arc::clone(&event.state);
        let revision = event_state.revision;
        if event.project_replaced
            || self
                .state
                .as_ref()
                .is_some_and(|previous| previous.revision != revision)
        {
            self.invalidate_prepared_seek();
        }
        if !event.project_replaced
            && self.viewport.is_none()
            && let Some(previous_horizon) = self.state.as_ref().map(|state| state.timeline_frames)
            && previous_horizon > 0
            && event_state.timeline_frames > previous_horizon
        {
            // Auto-fit is a one-time view choice. Once content extends the
            // project, keep the previously visible frame span so edits do not
            // make the Arrangement appear to zoom out unexpectedly.
            self.viewport = Some(Viewport::fit(previous_horizon));
        }
        if event.project_replaced {
            self.time_range_move_gesture = None;
            self.pending_time_range_move = None;
        } else if !event.gesture_request {
            self.move_time_selection_complete(&event);
        }
        if !event.project_replaced {
            super::loop_rail::cancel_stale(
                self,
                self.project_generation,
                revision,
                event_state.project.loop_region(),
            );
        }
        if self
            .state
            .as_ref()
            .is_some_and(|state| state.revision != revision)
        {
            self.cancel_midi_note_gesture();
        }
        self.drag_lanes
            .retain(|track, _| event.state.project.track(*track).is_some());
        self.drag_lane_epochs
            .retain(|track, _| event.state.project.track(*track).is_some());
        self.state = Some(event.state);
        if event.project_replaced {
            self.loop_rail_gesture = None;
            self.cancel_midi_note_gesture();
            self.midi_hit_cycle = None;
            self.midi_scene_cache.clear();
            if let Ok(mut cache) = self.waveform_geometry_cache.try_borrow_mut() {
                cache.clear();
            }
            self.piano_beat_start = hex_project::BeatTime::ZERO;
            self.piano_beat_span =
                hex_project::BeatTime::from_raw(4 * hex_project::BeatTime::ONE.raw());
            self.project_generation = self.project_generation.saturating_add(1);
            self.device_gesture.reset();
            self.device_gesture_barriers.clear();
            self.edit_cursor = None;
            self.time_selection = None;
            self.time_selection_project_scope = false;
            self.time_selection_gesture = None;
            self.time_range_move_gesture = None;
            self.pending_time_range_move = None;
            self.suppress_cursor_click = false;
            self.applied_project_revision = None;
            self.playback_candidates
                .retain(|candidate| candidate.seek_request_id.is_some());
            self.audible_playback = None;
            self.pending_playback = None;
            self.clear_route_telemetry();
            self.drag_lanes.clear();
            self.drag_lane_epochs.clear();
            self.arrangement_bounds = None;
            self.arrangement_bounds_epoch = 0;
            self.main_track_lane_bounds = None;
            self.main_track_lane_epoch = 0;
            self.active_clip_drag = None;
            self.pointer_down = None;
            self.stop();
            self.viewport = None;
            self.duplicate_intent.reject();
            self.pending_scene_duplicate = None;
            self.selected_track = None;
            self.selected_clip = None;
            self.selected_scene = None;
            self.selected_cell = None;
            self.session_copy_source = None;
            self.session_track_start = 0;
            self.midi_selection.clear();
        } else if let Some(state) = self.state.clone() {
            let previous_clip = self.selected_clip;
            if self.session_copy_source.is_some_and(|clip| {
                state.project.midi_clip(clip).is_none()
                    && !state
                        .project
                        .tracks()
                        .iter()
                        .any(|track| track.clips().iter().any(|item| item.id() == clip))
            }) {
                self.session_copy_source = None;
            }
            (self.selected_track, self.selected_clip) =
                selection::reconcile(&state.project, self.selected_track, self.selected_clip);
            if let Some(clip) = self.selected_clip
                && state.project.session_midi_clip(clip).is_some()
                && let Some(cell) = state.project.session_cells().iter().find(|cell| {
                    matches!(cell.content(), hex_project::SessionCellContent::Midi(item) if item.id() == clip)
                })
            {
                self.selected_track = Some(cell.track());
                self.selected_scene = Some(cell.scene());
                self.selected_cell = Some((cell.track(), cell.scene()));
            }
            if self
                .selected_scene
                .is_some_and(|scene| !state.project.scenes().iter().any(|item| item.id() == scene))
            {
                self.selected_scene = None;
            }
            if self.selected_cell.is_some_and(|(track, scene)| {
                state.project.track(track).is_none()
                    || !state.project.scenes().iter().any(|item| item.id() == scene)
            }) {
                self.selected_cell = None;
            }
            if self.selected_clip != previous_clip {
                self.cancel_midi_note_gesture();
                self.midi_hit_cycle = None;
                self.midi_selection.clear();
                self.piano_beat_start = hex_project::BeatTime::ZERO;
                if let Some(length) = self
                    .selected_clip
                    .and_then(|clip| state.project.midi_source(clip))
                    .map(|source| source.source_length().raw())
                {
                    self.piano_beat_span = hex_project::BeatTime::from_raw(
                        self.piano_beat_span.raw().min(length.max(1)),
                    );
                }
            }
            if let Some(clip) = self
                .selected_clip
                .and_then(|clip| state.project.midi_source(clip))
            {
                // Source markers and Arrangement duration may change independently.
                // Keep the editor inside the authored source even for the same clip.
                let source_length = clip.source_length().raw();
                self.piano_beat_span = hex_project::BeatTime::from_raw(
                    self.piano_beat_span.raw().min(source_length).max(1),
                );
                self.piano_beat_start =
                    hex_project::BeatTime::from_raw(super::midi_scene::clamp_start(
                        self.piano_beat_start.raw(),
                        source_length,
                        self.piano_beat_span.raw(),
                    ));
                let valid = clip.notes().iter().map(|note| note.id());
                match self.midi_selection.retain_valid(valid) {
                    Ok(true) => {
                        self.midi_note_gesture = None;
                        self.midi_group_gesture = None;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        self.midi_selection.clear();
                        self.midi_note_gesture = None;
                        self.midi_group_gesture = None;
                        self.error = Some(error.into());
                    }
                }
            } else if self.midi_selection.clear() {
                self.midi_note_gesture = None;
                self.midi_group_gesture = None;
            }
        }
        self.audio_admission
            .canonical_revision(super::audio_admission::ProjectRevision {
                generation: self.project_generation,
                revision,
            });
        if event.duplicate_complete {
            let created = match event.created {
                Some(hex_project::Created::Clip(clip)) => Some(clip),
                _ => None,
            };
            let completion = self.duplicate_intent.complete(created);
            if let Some(clip) = completion.select {
                self.midi_note_gesture = None;
                let track = self.state.as_ref().and_then(|state| {
                    state
                        .project
                        .tracks()
                        .iter()
                        .find(|track| track.clips().iter().any(|item| item.id() == clip))
                        .map(|track| track.id())
                });
                if let Some(track) = track {
                    self.selected_track = Some(track);
                    self.selected_clip = Some(clip);
                    self.piano_beat_start = hex_project::BeatTime::ZERO;
                    self.midi_hit_cycle = None;
                } else {
                    self.duplicate_intent.reject();
                }
            }
            if let Some(clip) = completion.submit {
                if !self.enqueue_duplicate_continuation(clip) {
                    self.duplicate_intent.reject();
                }
            }
        } else if let Some(hex_project::Created::Scene(scene)) = event.created {
            if let Some(intent) = scene_duplicate_intent {
                if intent.select_on_completion
                    && intent.project_generation == self.project_generation
                    && self.mode == super::EditorMode::Session
                    && !self.opening_project
                    && !self.close_prompt_open
                    && self.recovery.generation().is_none()
                    && self.selected_scene == Some(intent.source_scene)
                    && self.selected_cell.is_none()
                {
                    self.select_session_scene(scene);
                }
            } else if self.mode == super::EditorMode::Session {
                self.select_session_scene(scene);
            }
        } else if let Some(hex_project::Created::Track(track)) = event.created {
            self.midi_note_gesture = None;
            self.midi_group_gesture = None;
            self.selected_track = Some(track);
            self.selected_clip = None;
            self.midi_selection.clear();
        } else if let Some(hex_project::Created::Note(note)) = event.created {
            self.midi_selection.set_single(note);
        } else if let Some(hex_project::Created::Notes { clip, first, last }) = event.created {
            if let Some(current) = self
                .state
                .as_ref()
                .and_then(|state| state.project.midi_source(clip))
            {
                match super::midi_draw::created_range_ids(
                    self.selected_clip,
                    clip,
                    current.notes(),
                    first,
                    last,
                ) {
                    Ok(Some(created)) => {
                        if let Err(error) = self.midi_selection.select_all(created) {
                            self.error = Some(error.into());
                        }
                    }
                    Ok(None) => {}
                    Err(error) => self.error = Some(error.into()),
                }
            }
        } else if let Some(hex_project::Created::Clip(clip)) = event.created {
            let track = self.state.as_ref().and_then(|state| {
                state
                    .project
                    .tracks()
                    .iter()
                    .find(|track| {
                        track.clips().iter().any(|item| item.id() == clip)
                            || track.midi_clips().iter().any(|item| item.id() == clip)
                    })
                    .map(|track| track.id())
            });
            if let Some(track) = track {
                self.midi_note_gesture = None;
                self.midi_group_gesture = None;
                self.selected_track = Some(track);
                self.selected_clip = Some(clip);
                self.piano_beat_start = hex_project::BeatTime::ZERO;
                self.midi_hit_cycle = None;
                self.midi_selection.clear();
            }
        } else if let Some(hex_project::Created::SessionClip(clip)) = event.created {
            let owner = self.state.as_ref().and_then(|state| {
                state.project.session_cells().iter().find_map(|cell| {
                    let is_midi = matches!(
                        cell.content(),
                        hex_project::SessionCellContent::Midi(item) if item.id() == clip
                    );
                    let is_audio = matches!(
                        cell.content(),
                        hex_project::SessionCellContent::Audio(item) if item.id() == clip
                    );
                    (is_midi || is_audio).then_some((cell.track(), cell.scene(), is_midi))
                })
            });
            if let Some((track, scene, is_midi)) = owner {
                if is_midi {
                    self.select_session_midi(track, scene, clip);
                } else {
                    self.select_session_cell(track, scene);
                }
            }
        }
        if event.gesture_protocol_ack
            && let Some(ack) = event.gesture_ack.as_ref()
        {
            self.acknowledge_device_gesture(ack);
        } else if let Some(reason) = event
            .gesture_ack
            .as_ref()
            .and_then(|ack| ack.reason.as_ref())
        {
            self.error = Some(reason.clone());
        }
        if super::device_gesture::actor_retired_gesture(
            event.project_replaced,
            event.active_device_gesture.is_some(),
            event.gesture_ack.as_ref(),
            self.device_gesture.snapshot().map(|snapshot| snapshot.key),
        ) && self.device_gesture.snapshot().is_some()
        {
            self.device_gesture.reset();
            super::device_gesture::retire_deferred(
                &mut self.pending_requests,
                &mut self.device_gesture_barriers,
                event.project_replaced,
            );
        }
        let playback_capability =
            event
                .playback
                .as_ref()
                .map(|playback| match playback.session_capability() {
                    hex_engine::SessionCapability::Unavailable(error) => {
                        Some(error.to_string().to_lowercase())
                    }
                    _ => None,
                });
        if let Some(playback) = event.playback {
            let previous_reset = self
                .pending_playback
                .as_ref()
                .map(|pending| (pending.project_generation, pending.install_fresh_runtime));
            let source_state = event
                .device_preview
                .as_ref()
                .filter(|preview| {
                    preview.identity.project_generation == self.project_generation
                        && preview.identity.canonical_revision == revision
                })
                .map(|preview| Arc::clone(&preview.state))
                .unwrap_or_else(|| Arc::clone(&event_state));
            let preview_identity = event
                .device_preview
                .as_ref()
                .filter(|preview| {
                    preview.identity.project_generation == self.project_generation
                        && preview.identity.canonical_revision == revision
                })
                .map(|preview| preview.identity);
            // Retain the immutable source identity before Playback ownership can
            // move into the audio publication queue.
            let prepared_revision = playback.prepared_revision().cloned();
            let mut pending =
                pending_playback_for_state(source_state, self.project_generation, playback);
            pending.prepared_revision = prepared_revision;
            pending.device_preview_identity = preview_identity;
            pending.install_fresh_runtime = retained_install_fresh_policy(
                event.project_replaced,
                self.project_generation,
                previous_reset,
            );
            self.pending_playback = Some(pending);
        }
        if let Some(status) = event.status {
            self.status = status;
            self.error = None;
        }
        if let Some(error) = event.error {
            self.error = Some(error);
        }
        if let Some(message) = playback_capability {
            self.session_capability_message = message.clone();
            if let Some(message) = message {
                self.error = Some(message);
            }
        }
        if event.request_complete
            && !event.gesture_request
            && self.mode == super::EditorMode::Session
            && self.selected_cell.is_none()
            && self.selected_scene.is_some()
        {
            self.reveal_session_position(None, self.selected_scene);
        }
        successfully_opened_path
    }

    pub(super) fn enqueue(&mut self, request: Request) -> bool {
        self.enqueue_request(request, false)
    }

    fn enqueue_duplicate_continuation(&mut self, clip: hex_project::ClipId) -> bool {
        self.enqueue_request(
            Request::Edit(hex_project::Command::DuplicateClip(clip)),
            true,
        )
    }

    fn enqueue_request(&mut self, request: Request, duplicate_continuation: bool) -> bool {
        #[cfg(test)]
        match &request {
            Request::CopyTimeRange(_) => self.last_time_request = Some(("copy", None)),
            Request::CutTimeRange(_) => self.last_time_request = Some(("cut", None)),
            Request::PasteTimeRange { track, frame } => {
                self.last_time_request = Some(("paste", Some((*track, *frame))))
            }
            Request::InsertTimeRange { track, frame } => {
                self.last_time_request = Some(("insert", Some((*track, *frame))))
            }
            _ => {}
        }
        #[cfg(test)]
        if let Request::Edit(command) = &request {
            self.last_enqueued_edit = Some(command.clone());
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
        if let Request::DropNativeDevice {
            project_generation,
            revision,
            source,
            track,
            before,
            replace,
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
        } = &request
        {
            self.last_enqueued_import_to_new_track = Some((path.clone(), *before_track, *start));
        }
        if self.midi_stroke.is_some() {
            self.cancel_midi_stroke();
        }
        if !matches!(
            &request,
            Request::Open(_) | Request::OpenWith { .. } | Request::ImportLiveSet { .. }
        ) && !allows_non_open_request(
            self.close_prompt_open,
            self.opening_project,
            duplicate_continuation,
        ) {
            self.error = Some(if self.opening_project {
                "opening project · editing is paused".into()
            } else {
                "close confirmation open · editing is paused".into()
            });
            return false;
        }
        if !matches!(
            &request,
            Request::Open(_) | Request::OpenWith { .. } | Request::ImportLiveSet { .. }
        ) {
            self.pending_open_path = None;
        }
        if matches!(
            &request,
            Request::DeviceGesture(_)
                | Request::DropNativeDevice { .. }
                | Request::AddNativeDevice { .. }
                | Request::ReplaceNativeDevice { .. }
                | Request::SetDeviceParameter { .. }
                | Request::Import(_)
                | Request::ImportToTrack { .. }
                | Request::ImportToTrackWithOverwrite { .. }
                | Request::ImportToNewTrack { .. }
                | Request::ImportToSession { .. }
                | Request::ImportLiveSet { .. }
                | Request::Open(_)
                | Request::OpenWith { .. }
                | Request::Edit(_)
                | Request::CutTimeRange(_)
                | Request::PasteTimeRange { .. }
                | Request::InsertTimeRange { .. }
                | Request::Undo
                | Request::Redo
                | Request::RestorePlayback { .. }
        ) {
            self.invalidate_prepared_seek();
        }
        if matches!(
            &request,
            Request::Open(_)
                | Request::ImportLiveSet { .. }
                | Request::OpenWith { .. }
                | Request::Undo
                | Request::Redo
                | Request::SaveNew(_)
                | Request::SaveCurrent
                | Request::Export(_)
        ) && self.device_gesture.snapshot().is_some()
        {
            return self.enqueue_device_barrier(request);
        }
        let mut request = Some(request);
        if let Some(accepted) = self.enqueue_after_device_terminal(&mut request) {
            return accepted;
        }
        let Some(request) = request else {
            return false;
        };
        if !self.pending_requests.is_empty() {
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
        match self.service.submit(request) {
            Ok(()) => {
                self.worker_requests_pending = self.worker_requests_pending.saturating_add(1);
                true
            }
            Err(SubmitError::Full(request))
                if self.pending_requests.len() + self.device_gesture_barriers.len()
                    < UI_REQUEST_CAPACITY =>
            {
                self.pending_requests.push_back(request);
                true
            }
            Err(SubmitError::Full(_)) => {
                self.error = Some("request queue full · edit was not applied".into());
                false
            }
            Err(SubmitError::Busy(_)) => {
                self.error = Some("an export is already pending".into());
                false
            }
            Err(SubmitError::Closed(_)) => {
                self.error = Some("project worker stopped".into());
                false
            }
        }
    }

    pub(super) fn duplicate_selected(&mut self) {
        match self.duplicate_intent.press(self.selected_clip) {
            Ok(Press::Start(clip)) => {
                if !self.enqueue(Request::Edit(hex_project::Command::DuplicateClip(clip))) {
                    self.duplicate_intent.reject();
                }
            }
            Ok(Press::Queued | Press::NoSelection) => {}
            Err(reason) => self.error = Some(reason.into()),
        }
    }

    pub(super) fn request_seek(&mut self, frame: u64) {
        if self.recovery.generation().is_some() {
            self.error = Some("audio recovery pending · cannot seek yet".into());
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
                    self.desired_playing(audio.control.status.playing.load(Acquire))
                });
                self.request_prepared_seek(frame, start_playing);
                return;
            }
            Some(false) => {}
            None if self.audio.is_none() => {}
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
            match audio.control.action_with_ticket(command) {
                Ok(ticket) => self.pending_seek_ticket = Some(ticket),
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

    pub(super) fn seek_pending(&self) -> bool {
        if self.recovery.generation().is_some()
            || self.prepared_seek.is_some()
            || self.latched_seek_intent.is_some()
            || self.retired_seek_result.is_some()
        {
            return true;
        }
        let queued = self.pending_audio_commands.iter().any(|pending| {
            matches!(
                pending.action,
                PlaybackAction::Transport(PlaybackCommand::Seek(_) | PlaybackCommand::PlayFrom(_))
            )
        });
        let consumed = self
            .audio_status()
            .map_or(0, |status| status.consumed_command.load(Acquire));
        seek_is_pending(self.pending_seek_ticket, consumed, queued)
    }

    pub(super) fn cancel_duplicate_chain(&mut self) {
        self.duplicate_intent.cancel_chain();
    }

    pub(super) fn zoom_timeline(&mut self, inward: bool) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let horizon = state.timeline_frames;
        let playhead = self.cursor_position();
        let viewport = self
            .visible_viewport(horizon)
            .zoom(horizon, playhead, inward);
        self.viewport = if viewport == Viewport::fit(horizon) {
            None
        } else {
            Some(viewport)
        };
    }

    pub(super) fn zoom_timeline_wheel_at(
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
        let horizon = state.timeline_frames;
        let current = self.visible_viewport(horizon);
        let Some(anchor) = super::seek::pointer_to_frame(
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
        self.viewport = (next != Viewport::fit(horizon)).then_some(next);
        true
    }

    pub(super) fn fit_timeline(&mut self) {
        self.viewport = None;
    }

    pub(super) fn pan_timeline(&mut self, right: bool) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let horizon = state.timeline_frames;
        let current = self.visible_viewport(horizon);
        let panned = current.pan_half(horizon, right);
        self.viewport = if self.viewport.is_none() && panned == current {
            None
        } else {
            Some(panned)
        };
    }

    pub(super) fn queue_audio_command(&mut self, command: PlaybackCommand) {
        self.queue_playback_action(PlaybackAction::Transport(command), None);
    }

    pub(super) fn queue_session_command(
        &mut self,
        command: hex_engine::SessionPlaybackCommand,
        revision: u64,
    ) {
        let Some(audio) = self.audio.as_ref() else {
            self.error = Some("session playback requires an audio output".into());
            return;
        };
        if self.recovery.generation().is_some()
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
        if audio.control.status.session_capability()
            != Some(hex_engine::SessionCapabilityStatus::Available)
        {
            self.error = Some(
                audio
                    .control
                    .status
                    .session_capability()
                    .and_then(hex_engine::SessionCapabilityStatus::message)
                    .unwrap_or("session playback is unavailable for this project")
                    .to_lowercase(),
            );
            return;
        }
        self.queue_playback_action(PlaybackAction::Session(command), Some(revision));
    }

    pub(super) fn queue_session_launch_batch(
        &mut self,
        command: hex_engine::SessionPlaybackCommand,
        revision: u64,
    ) {
        self.invalidate_prepared_seek();
        if !matches!(
            command,
            hex_engine::SessionPlaybackCommand::LaunchClip { .. }
                | hex_engine::SessionPlaybackCommand::LaunchScene { .. }
        ) {
            self.error = Some("only session clips and scenes can start playback".into());
            return;
        }
        let Some(audio) = self.audio.as_ref() else {
            self.error = Some("session playback requires an audio output".into());
            return;
        };
        if self.recovery.generation().is_some()
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
        if audio.control.status.session_capability()
            != Some(hex_engine::SessionCapabilityStatus::Available)
        {
            let message = audio
                .control
                .status
                .session_capability()
                .and_then(hex_engine::SessionCapabilityStatus::message)
                .unwrap_or("session playback is unavailable for this project")
                .to_lowercase();
            self.error = Some(message);
            return;
        }
        let token = match super::play_intent::prepare_admission(
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
            super::session_grid::launch_action(command, revision, self.project_generation);
        pending.intent_token = Some(token);
        self.pending_audio_commands.push_back(pending);
        self.pending_session_ticket = None;
        self.play_intent.admit(token, true);
        self.status = "session launch requested · waiting for audio callback".into();
    }

    fn queue_playback_action(&mut self, action: PlaybackAction, revision: Option<u64>) {
        self.invalidate_prepared_seek();
        if self.audio.is_none() {
            self.error = Some("audio output is unavailable".into());
            return;
        }
        let desired_playing = intent_desired_playing(action);
        let intent_token = match super::play_intent::prepare_admission(
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
            let result = self
                .audio
                .as_mut()
                .map(|audio| audio.control.action_with_ticket(action));
            match result {
                Some(Ok(ticket)) => {
                    self.admit_play_intent(pending, desired_playing, Some(ticket));
                    if matches!(
                        action,
                        PlaybackAction::Transport(PlaybackCommand::PlayFrom(_))
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
                            PlaybackAction::Transport(PlaybackCommand::PlayFrom(_))
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
                PlaybackAction::Transport(PlaybackCommand::PlayFrom(_))
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

    fn admit_play_intent(
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

    fn report_intent_exhausted(&mut self) {
        self.error = Some("playback intent tokens exhausted · restart hex to continue".into());
    }

    pub(super) fn stop(&mut self) {
        self.invalidate_prepared_seek();
        self.offline_cursor = 0;
        self.recovery.stop();
        self.pending_audio_commands.clear();
        self.pending_seek_ticket = None;
        self.pending_session_ticket = None;
        self.play_intent.priority_stop();
        if let Some(audio) = &self.audio {
            audio.control.stop();
        }
        self.status = "stopped · returned to start".into();
    }
}

fn promote_playback_candidates(
    candidates: &mut VecDeque<PlaybackCandidate>,
    applied_high_water: u64,
    rejected_audio_revision: Option<u64>,
) -> Option<PlaybackCandidate> {
    let mut latest = None;
    while candidates
        .front()
        .is_some_and(|candidate| candidate.audio_revision <= applied_high_water)
    {
        if let Some(candidate) = candidates.pop_front()
            && Some(candidate.audio_revision) != rejected_audio_revision
        {
            latest = Some(candidate);
        }
    }
    latest
}

fn latest_applied_canonical_candidate(
    candidates: &VecDeque<PlaybackCandidate>,
    applied_high_water: u64,
    rejected_audio_revision: Option<u64>,
) -> Option<(super::audio_admission::ProjectRevision, u64)> {
    candidates
        .iter()
        .rfind(|candidate| {
            candidate.seek_request_id.is_none()
                && candidate.audio_revision <= applied_high_water
                && Some(candidate.audio_revision) != rejected_audio_revision
                && candidate.device_preview_identity.is_none()
        })
        .map(|candidate| {
            (
                super::audio_admission::ProjectRevision {
                    generation: candidate.project_generation,
                    revision: candidate.state.revision,
                },
                candidate.audio_revision,
            )
        })
}

fn rejected_candidate_project(
    candidates: &VecDeque<PlaybackCandidate>,
    audio_revision: u64,
) -> Option<super::audio_admission::ProjectRevision> {
    candidates
        .iter()
        .find(|candidate| {
            candidate.audio_revision == audio_revision
                && candidate.seek_request_id.is_none()
                && candidate.device_preview_identity.is_none()
        })
        .map(|candidate| super::audio_admission::ProjectRevision {
            generation: candidate.project_generation,
            revision: candidate.state.revision,
        })
}

fn rejected_preview_candidate(
    candidates: &VecDeque<PlaybackCandidate>,
    audio_revision: u64,
    project_generation: u64,
    canonical_revision: Option<u64>,
    active_gesture: Option<(GestureKey, u64)>,
) -> bool {
    candidates.iter().any(|candidate| {
        candidate.audio_revision == audio_revision
            && candidate.device_preview_identity.is_some_and(|identity| {
                preview_source_is_current(
                    identity,
                    candidate.project_generation,
                    candidate.state.revision,
                    project_generation,
                    canonical_revision,
                    active_gesture,
                )
            })
    })
}

fn preview_source_is_current(
    identity: DevicePreviewIdentity,
    candidate_generation: u64,
    candidate_revision: u64,
    current_generation: u64,
    current_revision: Option<u64>,
    active_gesture: Option<(GestureKey, u64)>,
) -> bool {
    identity.project_generation == candidate_generation
        && identity.project_generation == current_generation
        && identity.canonical_revision == candidate_revision
        && Some(identity.canonical_revision) == current_revision
        && active_gesture == Some((identity.key, identity.sequence))
}

fn pending_playback_for_state(
    state: Arc<hex_session::service::ViewState>,
    project_generation: u64,
    playback: Playback,
) -> PendingPlayback {
    PendingPlayback {
        project_revision: state.revision,
        project_generation,
        state,
        playback,
        install_fresh_runtime: false,
        fresh_runtime_fence: None,
        prepared_revision: None,
        seek_request_id: None,
        device_preview_identity: None,
    }
}

fn retained_install_fresh_policy(
    event_replaced_project: bool,
    project_generation: u64,
    previous: Option<(u64, bool)>,
) -> bool {
    event_replaced_project
        || previous.is_some_and(|(generation, reset)| generation == project_generation && reset)
}

fn intent_desired_playing(action: PlaybackAction) -> Option<bool> {
    match action {
        PlaybackAction::Transport(PlaybackCommand::Play | PlaybackCommand::PlayFrom(_))
        | PlaybackAction::LaunchAndPlay(_) => Some(true),
        PlaybackAction::Transport(PlaybackCommand::Pause) => Some(false),
        PlaybackAction::Transport(PlaybackCommand::Stop)
        | PlaybackAction::Transport(PlaybackCommand::Seek(_))
        | PlaybackAction::Session(_) => None,
    }
}

fn queued_action_is_current(
    pending: PendingPlaybackAction,
    project_edit_pending: bool,
    current_generation: u64,
    project: Option<&hex_project::Project>,
    current_revision: Option<u64>,
) -> bool {
    if pending.generation != current_generation {
        return false;
    }
    match pending.action {
        PlaybackAction::Transport(_) => true,
        PlaybackAction::Session(command) | PlaybackAction::LaunchAndPlay(command) => {
            !project_edit_pending
                && pending.revision.is_some_and(|revision| {
                    super::session_grid::action_is_current(
                        project,
                        command,
                        pending.generation,
                        current_generation,
                        revision,
                        current_revision,
                    )
                })
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionTicketOutcome {
    Consumed,
    Rejected,
}

fn reconcile_session_ticket(
    pending: Option<(u64, u64)>,
    current_generation: u64,
    consumed_high_water: u64,
    rejected_ticket: Option<u64>,
) -> (Option<(u64, u64)>, Option<SessionTicketOutcome>) {
    let Some((ticket, generation)) = pending else {
        return (None, None);
    };
    if generation != current_generation {
        return (None, None);
    }
    if rejected_ticket == Some(ticket) {
        return (None, Some(SessionTicketOutcome::Rejected));
    }
    if consumed_high_water >= ticket {
        return (None, Some(SessionTicketOutcome::Consumed));
    }
    (Some((ticket, generation)), None)
}

fn replace_error(error: &mut Option<String>, message: &str) -> bool {
    if error.as_deref() == Some(message) {
        false
    } else {
        *error = Some(message.to_owned());
        true
    }
}

fn replace_status(status: &mut String, message: &str) -> bool {
    if status == message {
        false
    } else {
        *status = message.to_owned();
        true
    }
}

fn clear_rejected_scene_duplicate(
    pending: &mut Option<super::PendingSceneDuplicate>,
    request: &Request,
) {
    if matches!(
        request,
        Request::Edit(hex_project::Command::DuplicateScene(_))
    ) {
        *pending = None;
    }
}

fn allows_non_open_request(
    close_prompt_open: bool,
    opening_project: bool,
    duplicate_continuation: bool,
) -> bool {
    !opening_project && (!close_prompt_open || duplicate_continuation)
}

#[cfg(test)]
mod tests {
    use super::{
        SessionTicketOutcome, allows_non_open_request, clear_rejected_scene_duplicate,
        intent_desired_playing, latest_applied_canonical_candidate, pending_playback_for_state,
        preview_source_is_current, promote_playback_candidates, queued_action_is_current,
        reconcile_session_ticket, rejected_candidate_project, rejected_preview_candidate,
        retained_install_fresh_policy,
    };
    use crate::editor::audio_admission::{AudioAdmissionStatus, ProjectRevision};
    use crate::editor::duplicate::{DuplicateIntent, Press};
    use crate::editor::{PendingPlaybackAction, PendingSceneDuplicate, PlaybackCandidate};
    use hex_engine::playback::Playback;
    use hex_engine::{PlaybackAction, PlaybackCommand, SessionPlaybackCommand};
    use hex_project::{ClipId, Command, Created, Editor, Project};
    use hex_session::device_gesture::GestureKey;
    use hex_session::service::{DevicePreviewIdentity, Request, ViewState};
    use std::{collections::VecDeque, sync::Arc};

    #[test]
    fn fresh_project_publication_survives_same_generation_candidate_coalescing_only() {
        assert!(retained_install_fresh_policy(true, 4, None));
        assert!(retained_install_fresh_policy(false, 4, Some((4, true))));
        assert!(!retained_install_fresh_policy(false, 4, None));
        assert!(!retained_install_fresh_policy(false, 4, Some((4, false))));
        assert!(!retained_install_fresh_policy(false, 5, Some((4, true))));
        assert!(retained_install_fresh_policy(true, 5, Some((4, true))));
    }

    #[test]
    fn exact_rejection_mapping_precedes_coalesced_applied_candidate_promotion() {
        let old = ProjectRevision {
            generation: 8,
            revision: 40,
        };
        let current = ProjectRevision {
            generation: 9,
            revision: 0,
        };
        let mut candidates = VecDeque::from([candidate(17, 40, 8), candidate(18, 0, 9)]);
        let rejected = rejected_candidate_project(&candidates, 17)
            .expect("rejected audio revision maps before candidate pruning");
        assert_eq!(rejected, old);

        let mut status = AudioAdmissionStatus::default();
        status.canonical_revision(current);
        status.published(current, 18);
        assert!(!status.rejected(rejected, 17, "stale generation rejection"));

        let audible = promote_playback_candidates(&mut candidates, 18, Some(17))
            .expect("new accepted candidate survives older rejection telemetry");
        assert_eq!(audible.audio_revision, 18);
        assert_eq!(audible.project_generation, current.generation);
        status.applied(current, audible.audio_revision);
        assert_eq!(
            status.current().unwrap().state,
            crate::editor::audio_admission::AdmissionState::Applied
        );
        assert_eq!(status.last_audible().unwrap().project, current);
    }

    #[test]
    fn same_revision_preview_apply_reject_and_cancel_do_not_change_canonical_admission() {
        let canonical = ProjectRevision {
            generation: 3,
            revision: 40,
        };
        let mut status = AudioAdmissionStatus::default();
        status.canonical_revision(canonical);
        status.published(canonical, 16);

        let mut preview = preview_candidate(17, 40, 3);
        let active = Some((
            GestureKey {
                generation: 3,
                token: 1,
            },
            1,
        ));
        assert!(rejected_candidate_project(&VecDeque::from([preview.clone()]), 17).is_none());
        assert!(rejected_preview_candidate(
            &VecDeque::from([preview.clone()]),
            17,
            3,
            Some(40),
            active,
        ));
        let current_identity = preview.device_preview_identity.expect("preview identity");
        assert!(preview_source_is_current(
            current_identity,
            3,
            40,
            3,
            Some(40),
            active,
        ));
        let stale_generation = preview_candidate(18, 40, 2);
        assert!(!rejected_preview_candidate(
            &VecDeque::from([stale_generation]),
            18,
            3,
            Some(40),
            active,
        ));
        assert!(!preview_source_is_current(
            current_identity,
            3,
            40,
            4,
            Some(0),
            active,
        ));
        let mut stale_sequence = preview_candidate(19, 40, 3);
        stale_sequence
            .device_preview_identity
            .as_mut()
            .expect("preview identity")
            .sequence = 0;
        assert!(!rejected_preview_candidate(
            &VecDeque::from([stale_sequence]),
            19,
            3,
            Some(40),
            active,
        ));
        // Cancel/commit retires the active gesture, so a delayed terminal
        // preview refusal cannot overwrite a newer status message.
        assert!(!rejected_preview_candidate(
            &VecDeque::from([preview.clone()]),
            17,
            3,
            Some(40),
            None,
        ));
        assert!(!preview_source_is_current(
            current_identity,
            3,
            40,
            3,
            Some(40),
            None,
        ));
        assert!(!rejected_preview_candidate(
            &VecDeque::from([preview.clone()]),
            17,
            4,
            Some(0),
            active,
        ));

        // A valid preview can become audible for this exact canonical source,
        // while its transient audio revision remains outside canonical status.
        let mut preview_candidates = VecDeque::from([candidate(16, 40, 3), preview.clone()]);
        let (applied_project, applied_audio) =
            latest_applied_canonical_candidate(&preview_candidates, 17, None)
                .expect("the canonical candidate applied before the transient preview");
        status.applied(applied_project, applied_audio);
        let audible_preview = promote_playback_candidates(&mut preview_candidates, 17, None)
            .expect("the callback promoted the preview graph");
        assert!(audible_preview.device_preview_identity.is_some());
        assert_eq!(
            status.current().unwrap().state,
            crate::editor::audio_admission::AdmissionState::Applied
        );
        assert_eq!(status.last_audible().unwrap().audio_revision, 16);

        // Cancellation restores canonical playback at the same project
        // revision; that terminal canonical candidate remains identifiable.
        preview = candidate(18, 40, 3);
        let mut restore_candidates = VecDeque::from([preview]);
        let restored = promote_playback_candidates(&mut restore_candidates, 18, None)
            .expect("cancel restored the canonical graph");
        assert_eq!(
            rejected_candidate_project(&VecDeque::from([restored]), 18),
            Some(canonical)
        );
        status.published(canonical, 18);
        status.applied(canonical, 18);
        assert_eq!(
            status.current().unwrap().state,
            crate::editor::audio_admission::AdmissionState::Applied
        );
        assert_eq!(status.last_audible().unwrap().audio_revision, 18);

        // A newer canonical revision stays pending when only its numeric
        // preview is accepted, then becomes applied on the terminal commit.
        let committed = ProjectRevision {
            generation: 3,
            revision: 41,
        };
        status.canonical_revision(committed);
        status.published(committed, 19);
        let preview = preview_candidate(20, 41, 3);
        let mut candidates = VecDeque::from([preview.clone()]);
        assert!(rejected_candidate_project(&candidates, 20).is_none());
        assert!(latest_applied_canonical_candidate(&candidates, 20, None).is_none());
        let audible_preview = promote_playback_candidates(&mut candidates, 20, None)
            .expect("the callback promoted the latest numeric preview");
        assert!(audible_preview.device_preview_identity.is_some());
        assert_eq!(status.current().unwrap().project, committed);
        assert_eq!(
            status.current().unwrap().state,
            crate::editor::audio_admission::AdmissionState::Pending
        );

        let canonical_candidate = candidate(21, 41, 3);
        assert_eq!(
            rejected_candidate_project(&VecDeque::from([canonical_candidate.clone()]), 21),
            Some(committed)
        );
        status.published(committed, 21);
        let applied =
            latest_applied_canonical_candidate(&VecDeque::from([canonical_candidate]), 21, None)
                .expect("terminal canonical candidate applied");
        status.applied(applied.0, applied.1);
        assert_eq!(
            status.current().unwrap().state,
            crate::editor::audio_admission::AdmissionState::Applied
        );
        assert_eq!(status.last_audible().unwrap().audio_revision, 21);
    }

    #[test]
    fn rejected_scene_duplicate_clears_only_its_completion_selection_intent() {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 4).expect("editor");
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "source".into(),
            })
            .expect("scene")
        else {
            panic!("scene creation returns a scene id")
        };
        let intent = || {
            Some(PendingSceneDuplicate {
                source_scene: scene,
                project_generation: 3,
                select_on_completion: true,
            })
        };
        let mut pending = intent();

        clear_rejected_scene_duplicate(
            &mut pending,
            &Request::Edit(Command::DuplicateScene(scene)),
        );
        assert!(pending.is_none());

        let mut pending = intent();
        clear_rejected_scene_duplicate(
            &mut pending,
            &Request::Edit(Command::MoveScene {
                scene,
                before: None,
            }),
        );
        assert!(pending.is_some());
    }

    fn candidate(audio_revision: u64, project_revision: u64, generation: u64) -> PlaybackCandidate {
        let project = Arc::new(Project::new(48_000).expect("project"));
        PlaybackCandidate {
            audio_revision,
            project_generation: generation,
            state: Arc::new(ViewState {
                project,
                waveforms: Default::default(),
                midi_preview: Arc::new(hex_engine::MidiArrangementPreview::default()),
                timeline_frames: 0,
                can_undo: false,
                can_redo: false,
                revision: project_revision,
                unsaved: false,
                document_path: None,
            }),
            prepared_revision: None,
            seek_request_id: None,
            device_preview_identity: None,
        }
    }

    fn preview_candidate(
        audio_revision: u64,
        project_revision: u64,
        generation: u64,
    ) -> PlaybackCandidate {
        let mut candidate = candidate(audio_revision, project_revision, generation);
        candidate.device_preview_identity = Some(DevicePreviewIdentity {
            project_generation: generation,
            key: GestureKey {
                generation,
                token: 1,
            },
            sequence: 1,
            canonical_revision: project_revision,
        });
        candidate
    }

    #[test]
    fn event_playback_retains_its_exact_read_only_view_until_publication_retry() {
        let captured = candidate(3, 12, 4).state;
        let exact = Arc::clone(&captured);
        let playback = Playback::new(hex_engine::Project::new(48_000, Vec::new()).unwrap());
        let pending = pending_playback_for_state(captured, 4, playback);
        assert!(Arc::ptr_eq(&pending.state, &exact));
        assert_eq!(pending.project_revision, 12);
        assert_eq!(pending.project_generation, 4);
    }

    #[test]
    fn applied_candidates_promote_before_coalesced_rejection_pruning() {
        let mut candidates = VecDeque::from([
            candidate(4, 40, 2),
            candidate(5, 50, 2),
            candidate(6, 60, 2),
            candidate(7, 70, 2),
        ]);
        assert!(promote_playback_candidates(&mut candidates, 3, None).is_none());
        let audible = promote_playback_candidates(&mut candidates, 5, None).expect("accepted view");
        assert_eq!(audible.audio_revision, 5);
        assert_eq!(audible.state.revision, 50);
        candidates.retain(|item| item.audio_revision > 7);
        assert!(candidates.is_empty());
        assert_eq!(audible.project_generation, 2);
    }

    #[test]
    fn rejected_unapplied_candidate_never_becomes_audible() {
        let mut candidates = VecDeque::from([candidate(4, 40, 2)]);
        assert!(promote_playback_candidates(&mut candidates, 3, None).is_none());
        candidates.retain(|item| item.audio_revision > 4);
        assert!(candidates.is_empty());
    }

    #[test]
    fn project_edit_backpressure_preserves_transport_but_invalidates_session_actions() {
        let project = Project::new(48_000).expect("project");
        let transport = PendingPlaybackAction {
            action: PlaybackAction::Transport(PlaybackCommand::Pause),
            generation: 8,
            revision: None,
            intent_token: None,
        };
        let session = PendingPlaybackAction {
            action: PlaybackAction::Session(SessionPlaybackCommand::StopAllClips {
                quantization: hex_engine::LaunchQuantization::Immediate,
            }),
            generation: 8,
            revision: Some(12),
            intent_token: None,
        };

        assert!(queued_action_is_current(
            transport,
            true,
            8,
            Some(&project),
            Some(12),
        ));
        assert!(!queued_action_is_current(
            session,
            true,
            8,
            Some(&project),
            Some(12),
        ));
        assert!(!queued_action_is_current(
            transport,
            false,
            9,
            Some(&project),
            Some(12),
        ));
    }

    #[test]
    fn session_ticket_feedback_clears_on_consumption_or_matching_rejection() {
        assert_eq!(
            reconcile_session_ticket(Some((5, 7)), 7, 4, None),
            (Some((5, 7)), None)
        );
        assert_eq!(
            reconcile_session_ticket(Some((5, 7)), 7, 5, None),
            (None, Some(SessionTicketOutcome::Consumed))
        );
        assert_eq!(
            reconcile_session_ticket(Some((5, 7)), 7, 5, Some(5)),
            (None, Some(SessionTicketOutcome::Rejected))
        );
        assert_eq!(
            reconcile_session_ticket(Some((5, 7)), 8, 5, None),
            (None, None)
        );
    }

    #[test]
    fn launch_and_transport_actions_have_explicit_play_intents() {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 1).expect("editor");
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .expect("scene")
        else {
            panic!("scene")
        };
        assert_eq!(
            intent_desired_playing(PlaybackAction::LaunchAndPlay(
                SessionPlaybackCommand::LaunchScene {
                    scene,
                    quantization: hex_engine::LaunchQuantization::Immediate,
                }
            )),
            Some(true)
        );
        assert_eq!(
            intent_desired_playing(PlaybackAction::Transport(PlaybackCommand::Play)),
            Some(true)
        );
        assert_eq!(
            intent_desired_playing(PlaybackAction::Transport(PlaybackCommand::Pause)),
            Some(false)
        );
        assert_eq!(
            intent_desired_playing(PlaybackAction::Transport(PlaybackCommand::Seek(1))),
            None
        );
    }

    #[test]
    fn close_prompt_preserves_already_accepted_duplicate_repeats() {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 1).expect("editor");
        let track = match editor
            .execute(Command::AddTrack {
                name: "track".into(),
            })
            .expect("track")
        {
            Created::Track(id) => id,
            other => panic!("unexpected result: {other:?}"),
        };
        let asset = match editor
            .execute(Command::AddAsset {
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 1,
            })
            .expect("asset")
        {
            Created::Asset(id) => id,
            other => panic!("unexpected result: {other:?}"),
        };
        let clips: Vec<ClipId> = (0..4)
            .map(|_| {
                match editor
                    .execute(Command::PlaceClip {
                        track,
                        asset,
                        start: 0,
                        duration: 1,
                        source_offset: 0,
                        source_length: 1,
                        looping: false,
                    })
                    .expect("clip")
                {
                    Created::Clip(id) => id,
                    other => panic!("unexpected result: {other:?}"),
                }
            })
            .collect();
        let [source, first, second, third] = clips.as_slice() else {
            panic!("expected four fixture clips");
        };

        let mut intent = DuplicateIntent::default();
        assert!(matches!(intent.press(Some(*source)), Ok(Press::Start(_))));
        assert!(matches!(intent.press(Some(*source)), Ok(Press::Queued)));
        assert!(matches!(intent.press(Some(*source)), Ok(Press::Queued)));

        let first_result = intent.complete(Some(*first));
        assert_eq!(first_result.submit, Some(*first));
        assert!(allows_non_open_request(true, false, true));
        assert!(!allows_non_open_request(true, false, false));

        let second_result = intent.complete(Some(*second));
        assert_eq!(second_result.submit, Some(*second));
        assert!(allows_non_open_request(true, false, true));

        // Canceling the prompt clears only the close gate; accepted repeats remain chained.
        let third_result = intent.complete(Some(*third));
        assert_eq!(third_result.submit, None);
        assert!(!intent.is_pending());
        assert!(allows_non_open_request(false, false, false));
        assert!(!allows_non_open_request(true, true, true));
    }
}
