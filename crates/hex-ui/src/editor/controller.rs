#[cfg(test)]
use super::EnqueuedNativeDeviceDrop;
use super::duplicate::Press;
use super::play_intent::AdmissionError;
use super::refresh::AudioFrame;
use super::seek::seek_is_pending;
use super::selection;
use super::viewport::Viewport;
use super::{
    EditorView, PendingEditorAudioResult, PendingEditorPublicationObservation, PendingPlayback,
    PendingPlaybackAction, PendingResidentPublication, Playback, PlaybackCandidate,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AudioEventDisposition {
    Current,
    Future,
    Ignore,
}

fn audio_event_disposition(
    current_generation: u64,
    pending_restart: Option<(u64, u64)>,
    event_generation: u64,
    restart_request: Option<u64>,
) -> AudioEventDisposition {
    if event_generation < current_generation {
        return AudioEventDisposition::Ignore;
    }
    if event_generation == current_generation {
        return match restart_request {
            Some(request_id) if pending_restart == Some((event_generation, request_id)) => {
                AudioEventDisposition::Current
            }
            Some(_) => AudioEventDisposition::Ignore,
            None if pending_restart
                .is_some_and(|(generation, _)| generation == event_generation) =>
            {
                AudioEventDisposition::Ignore
            }
            None => AudioEventDisposition::Current,
        };
    }
    match restart_request {
        Some(request_id) if pending_restart == Some((event_generation, request_id)) => {
            AudioEventDisposition::Future
        }
        Some(_) => AudioEventDisposition::Ignore,
        None => AudioEventDisposition::Future,
    }
}

fn future_audio_result_replaces(slot_generation: Option<u64>, event_generation: u64) -> bool {
    slot_generation.is_none_or(|slot_generation| event_generation >= slot_generation)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PublicationObservationDisposition {
    Apply,
    Wait,
    Stale,
}

fn publication_observation_replaces(
    existing: Option<hex_engine::AudioSourceContext>,
    incoming: hex_engine::AudioSourceContext,
) -> bool {
    existing.is_none_or(|existing| {
        (incoming.project_generation, incoming.editor_revision)
            >= (existing.project_generation, existing.editor_revision)
    })
}

fn publication_observation_disposition(
    current_generation: u64,
    current_revision: Option<u64>,
    current_context: Option<hex_engine::AudioSourceContext>,
    current_output_identity: Option<u64>,
    observation_context: hex_engine::AudioSourceContext,
    observation_output_identity: u64,
) -> PublicationObservationDisposition {
    let observed = (
        observation_context.project_generation,
        observation_context.editor_revision,
    );
    if observation_context.project_generation < current_generation
        || current_revision.is_some_and(|revision| {
            observation_context.project_generation == current_generation
                && observation_context.editor_revision < revision
        })
    {
        return PublicationObservationDisposition::Stale;
    }
    let Some(current_context) = current_context else {
        return PublicationObservationDisposition::Wait;
    };
    let current = (
        current_context.project_generation,
        current_context.editor_revision,
    );
    if observed < current {
        return PublicationObservationDisposition::Stale;
    }
    if observed > current {
        return PublicationObservationDisposition::Wait;
    }
    match current_output_identity {
        Some(identity) if identity == observation_output_identity => {
            PublicationObservationDisposition::Apply
        }
        Some(_) => PublicationObservationDisposition::Stale,
        None => PublicationObservationDisposition::Wait,
    }
}

impl EditorView {
    pub(super) fn restart_audio_for_current_document(&mut self) -> bool {
        let Some(request_id) = self.restart_request_id.checked_add(1) else {
            self.error = Some("audio restart request id exhausted".into());
            return false;
        };
        self.restart_request_id = request_id;
        let project_generation = self.project_generation;
        let request = Request::RestartAudio {
            project_generation,
            request_id,
        };
        match self.service.submit(request) {
            Ok(()) => {
                self.pending_restart_request = Some((project_generation, request_id));
                self.status = "preparing audio output".into();
                true
            }
            Err(error) => {
                self.pending_restart_request = None;
                self.error = Some(
                    format!("could not restart audio · {error:?}")
                        .to_lowercase()
                        .into(),
                );
                self.status = "audio unavailable".into();
                false
            }
        }
    }

    fn retire_audio_for_future_generation(&mut self) {
        self.invalidate_prepared_seek();
        if let Some(audio) = self.audio.as_mut() {
            audio.control.close();
        }
        self.audio = None;
        self.gain_recorder.discard();
        self.prepared_seek = None;
        self.latched_seek_intent = None;
        self.pending_recovery_generation = None;
        self.recovery.fail();
        self.pending_playback = None;
        self.pending_resident_publication = None;
        self.pending_audio_commands.clear();
        self.pending_seek_ticket = None;
        self.pending_metronome_ticket = None;
        self.pending_session_ticket = None;
        self.playback_candidates.clear();
        self.audible_playback = None;
        self.audio_admission.reset_audio();
    }

    fn retain_future_audio_result(&mut self, result: PendingEditorAudioResult) {
        let slot_generation = self
            .pending_audio_result
            .as_ref()
            .map(PendingEditorAudioResult::project_generation);
        if !future_audio_result_replaces(slot_generation, result.project_generation()) {
            result.close();
            return;
        }
        if let Some(previous) = self.pending_audio_result.take() {
            previous.close();
        }
        self.pending_audio_result = Some(result);
    }

    fn retain_publication_observation(
        &mut self,
        context: hex_engine::AudioSourceContext,
        output_identity: u64,
        state: crate::EditorPublicationState,
    ) {
        let current_context = self
            .state
            .as_ref()
            .and_then(|state| state.canonical_audio_source.as_ref())
            .map(|snapshot| snapshot.context());
        if publication_observation_disposition(
            self.project_generation,
            self.state.as_ref().map(|state| state.revision),
            current_context,
            self.audio
                .as_ref()
                .and_then(|audio| audio.control.output_identity()),
            context,
            output_identity,
        ) == PublicationObservationDisposition::Stale
        {
            return;
        }
        let existing_context = self
            .pending_publication_observation
            .as_ref()
            .map(|pending| pending.context);
        if !publication_observation_replaces(existing_context, context) {
            return;
        }
        self.pending_publication_observation = Some(PendingEditorPublicationObservation {
            context,
            output_identity,
            state,
        });
        self.reconcile_publication_observation();
    }

    fn reconcile_publication_observation(&mut self) -> bool {
        let Some(pending) = self.pending_publication_observation.as_ref() else {
            return false;
        };
        let current_context = self
            .state
            .as_ref()
            .and_then(|state| state.canonical_audio_source.as_ref())
            .map(|snapshot| snapshot.context());
        let disposition = publication_observation_disposition(
            self.project_generation,
            self.state.as_ref().map(|state| state.revision),
            current_context,
            self.audio
                .as_ref()
                .and_then(|audio| audio.control.output_identity()),
            pending.context,
            pending.output_identity,
        );
        match disposition {
            PublicationObservationDisposition::Wait => false,
            PublicationObservationDisposition::Stale => {
                self.pending_publication_observation = None;
                true
            }
            PublicationObservationDisposition::Apply => {
                if let Some(pending) = self.pending_publication_observation.take() {
                    self.apply_publication_observation(pending.context, pending.state);
                }
                true
            }
        }
    }

    fn apply_publication_observation(
        &mut self,
        context: hex_engine::AudioSourceContext,
        state: crate::EditorPublicationState,
    ) {
        match state {
            crate::EditorPublicationState::Preparing => {
                self.status = "preparing streamed audio".into()
            }
            crate::EditorPublicationState::Busy => {
                self.status = "audio busy · waiting to publish".into()
            }
            crate::EditorPublicationState::Submitted { audio_revision } => {
                if let Some(state) = self
                    .state
                    .as_ref()
                    .filter(|state| state.revision == context.editor_revision)
                    .cloned()
                    && !self.playback_candidates.iter().any(|candidate| {
                        candidate.audio_revision == audio_revision
                            && candidate.project_generation == context.project_generation
                    })
                    && self.playback_candidates.len() < super::PLAYBACK_CANDIDATE_CAPACITY
                {
                    self.playback_candidates.push_back(PlaybackCandidate {
                        audio_revision,
                        project_generation: context.project_generation,
                        state,
                        prepared_revision: None,
                        seek_request_id: None,
                        device_preview_identity: None,
                        tuning_preview_identity: None,
                    });
                }
                self.audio_admission.published(
                    super::audio_admission::ProjectRevision {
                        generation: context.project_generation,
                        revision: context.editor_revision,
                    },
                    audio_revision,
                );
                if self.document_audio_mode == hex_session::DocumentAudioMode::VerifiedStreaming
                    && let Some(generation) = self.pending_recovery_generation
                {
                    self.recovery.restored(generation);
                }
                self.status =
                    format!("audio revision {audio_revision} submitted · waiting for playback");
            }
            crate::EditorPublicationState::Refused(reason) => {
                self.audio_admission.unpublished_refusal(
                    super::audio_admission::ProjectRevision {
                        generation: context.project_generation,
                        revision: context.editor_revision,
                    },
                    "audio publication refused",
                );
                self.status = format!("audio publication refused · {}", reason.to_lowercase());
                if self.pending_recovery_generation.is_some() {
                    self.fail_audio_recovery(reason);
                }
            }
            crate::EditorPublicationState::Fault(reason) => {
                self.status = format!("audio fault · {}", reason.to_lowercase());
                if self.pending_recovery_generation.is_some() {
                    self.fail_audio_recovery(reason);
                }
            }
        }
    }

    fn poll_editor_audio_events(&mut self) -> bool {
        let mut changed = false;
        for _ in 0..UI_EVENT_BUDGET {
            let event = match self.audio_events.try_recv() {
                Ok(event) => event,
                Err(std::sync::mpsc::TryRecvError::Empty)
                | Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            };
            match event {
                crate::EditorAudioEvent::Opened {
                    project_generation,
                    restart_request,
                    mut audio,
                } => {
                    match audio_event_disposition(
                        self.project_generation,
                        self.pending_restart_request,
                        project_generation,
                        restart_request,
                    ) {
                        AudioEventDisposition::Ignore => {
                            audio.control.close();
                            continue;
                        }
                        AudioEventDisposition::Future => {
                            self.retire_audio_for_future_generation();
                            if self
                                .pending_restart_request
                                .is_some_and(|(generation, _)| generation < project_generation)
                            {
                                self.pending_restart_request = None;
                            }
                            self.retain_future_audio_result(PendingEditorAudioResult::Opened {
                                project_generation,
                                restart_request,
                                audio,
                            });
                            changed = true;
                            continue;
                        }
                        AudioEventDisposition::Current => {}
                    }
                    if self
                        .pending_audio_result
                        .as_ref()
                        .is_some_and(|pending| pending.project_generation() <= project_generation)
                    {
                        if let Some(previous) = self.pending_audio_result.take() {
                            previous.close();
                        }
                    }
                    if let Some(current) = self.audio.as_mut() {
                        current.control.close();
                    }
                    self.document_audio_mode = audio.mode;
                    self.audio = Some(audio);
                    let mut recovery_status = None;
                    if restart_request.is_some() {
                        if let Some(generation) = self.pending_recovery_generation {
                            self.recovery.opened(generation);
                            match self.document_audio_mode {
                                hex_session::DocumentAudioMode::Resident => {
                                    if self.enqueue(Request::RestorePlayback { generation }) {
                                        recovery_status = Some("restoring audio graph");
                                    } else {
                                        self.fail_audio_recovery(
                                            "audio opened but graph restore was not admitted"
                                                .into(),
                                        );
                                    }
                                }
                                hex_session::DocumentAudioMode::VerifiedStreaming => {
                                    recovery_status = Some("preparing streamed audio");
                                }
                            }
                        }
                    }
                    self.pending_restart_request = None;
                    self.status = recovery_status.unwrap_or("audio ready").into();
                    self.error = None;
                    self.reconcile_publication_observation();
                    changed = true;
                }
                crate::EditorAudioEvent::Unavailable {
                    project_generation,
                    restart_request,
                    error,
                } => {
                    match audio_event_disposition(
                        self.project_generation,
                        self.pending_restart_request,
                        project_generation,
                        restart_request,
                    ) {
                        AudioEventDisposition::Ignore => continue,
                        AudioEventDisposition::Future => {
                            self.retire_audio_for_future_generation();
                            if self
                                .pending_restart_request
                                .is_some_and(|(generation, _)| generation < project_generation)
                            {
                                self.pending_restart_request = None;
                            }
                            self.retain_future_audio_result(
                                PendingEditorAudioResult::Unavailable {
                                    project_generation,
                                    restart_request,
                                    error,
                                },
                            );
                            changed = true;
                            continue;
                        }
                        AudioEventDisposition::Current => {}
                    }
                    if self
                        .pending_audio_result
                        .as_ref()
                        .is_some_and(|pending| pending.project_generation() <= project_generation)
                    {
                        if let Some(previous) = self.pending_audio_result.take() {
                            previous.close();
                        }
                    }
                    if let Some(audio) = self.audio.as_mut() {
                        audio.control.close();
                    }
                    self.audio = None;
                    if self
                        .pending_publication_observation
                        .as_ref()
                        .is_some_and(|pending| {
                            pending.context.project_generation <= project_generation
                        })
                    {
                        self.pending_publication_observation = None;
                    }
                    if self.pending_recovery_generation.is_some() {
                        self.fail_audio_recovery(error);
                        changed = true;
                        continue;
                    }
                    self.pending_restart_request = None;
                    self.pending_recovery_generation = None;
                    let message = format!("audio unavailable · {}", error.to_lowercase());
                    self.status = message.clone();
                    self.error = Some(message.into());
                    changed = true;
                }
                crate::EditorAudioEvent::Publication {
                    context,
                    output_identity,
                    state,
                } => {
                    self.retain_publication_observation(context, output_identity, state);
                    changed = true;
                }
            }
        }
        changed
    }

    fn poll_resident_publication(&mut self) -> bool {
        let response = self
            .audio
            .as_mut()
            .and_then(|audio| audio.control.try_resident_publication_response());
        let Some(response) = response else {
            return false;
        };
        let Some(pending) = self.pending_resident_publication.take() else {
            self.error = Some("audio returned an untracked resident publication".into());
            return true;
        };
        if response.token != pending.token {
            self.pending_resident_publication = Some(pending);
            self.error = Some("audio returned a stale resident publication".into());
            return true;
        }
        match response.result {
            Ok(audio_revision) => {
                if pending.install_fresh_runtime
                    && self.stopped_playback_restore
                        == Some(super::audio_admission::StoppedPlaybackRestore::Ready)
                {
                    self.stopped_playback_restore = Some(
                        super::audio_admission::StoppedPlaybackRestore::Published(audio_revision),
                    );
                }
                self.gain_recorder.published(
                    pending
                        .prepared_revision
                        .as_ref()
                        .and_then(|prepared| prepared.gain_recording_track()),
                    audio_revision,
                );
                let project_revision = super::audio_admission::ProjectRevision {
                    generation: pending.project_generation,
                    revision: pending.project_revision,
                };
                if pending.device_preview_identity.is_none()
                    && pending.tuning_preview_identity.is_none()
                    && pending.seek_request_id.is_none()
                {
                    self.audio_admission
                        .published(project_revision, audio_revision);
                }
                self.playback_candidates.push_back(PlaybackCandidate {
                    audio_revision,
                    project_generation: pending.project_generation,
                    state: pending.state,
                    prepared_revision: pending.prepared_revision,
                    seek_request_id: pending.seek_request_id,
                    device_preview_identity: pending.device_preview_identity,
                    tuning_preview_identity: pending.tuning_preview_identity,
                });
                if let Some(seek) = self.prepared_seek.as_mut()
                    && pending.seek_request_id == Some(seek.request_id)
                {
                    seek.stage = super::arrangement_seek::SeekStage::Published { audio_revision };
                }
                self.status =
                    format!("audio revision {audio_revision} submitted · waiting for playback");
            }
            Err(rejected) => {
                self.pending_playback = Some(PendingPlayback {
                    project_revision: pending.project_revision,
                    project_generation: pending.project_generation,
                    state: pending.state,
                    playback: rejected.playback,
                    install_fresh_runtime: pending.install_fresh_runtime,
                    fresh_runtime_fence: pending.fresh_runtime_fence,
                    prepared_revision: pending.prepared_revision,
                    seek_request_id: pending.seek_request_id,
                    device_preview_identity: pending.device_preview_identity,
                    tuning_preview_identity: pending.tuning_preview_identity,
                });
                self.error = Some(rejected.reason.to_lowercase().into());
            }
        }
        true
    }

    pub(super) fn clear_route_telemetry(&mut self) {
        self.session_routes.clear();
    }

    pub(super) fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        let logging_failure =
            crate::diagnostics::record_state(cx, &self.status, self.error.as_deref());
        let logging_changed = if let Some(failure) = logging_failure {
            let message = self.error.as_ref().map_or_else(
                || failure.clone().into(),
                |error| error.with_context(&failure),
            );
            let changed = self.error.as_ref() != Some(&message);
            self.error = Some(message);
            changed
        } else {
            false
        };
        let pending_before = self
            .worker_requests_pending
            .saturating_add(self.pending_requests.len());
        let mut visible_change = self.poll_audio_recovery() || logging_changed;
        visible_change |= self.poll_browser_sample_playback();
        visible_change |= self.tick_gain_recording();
        visible_change |= self.tick_computer_keyboard();
        visible_change |= self.tick_tuning_audition();
        visible_change |= self.tuning.broadcast.tick(cx);
        visible_change |= self.tick_tuning_midi_activity();
        visible_change |= self.tick_controller_input(cx);
        visible_change |= self.poll_audio_batch();
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
        visible_change |= self.poll_editor_audio_events();
        visible_change |= self.poll_export(cx);
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
                    if matches!(&request, Request::ImportAudioBatch { .. }) {
                        if let Some(control) = self.audio_batch.take() {
                            control.request_cancel();
                        }
                        self.audio_batch_progress = None;
                    }
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
        visible_change |= self.pump_tuning_gesture();
        let now = Instant::now();
        visible_change |= self.tick_time_gesture_autoscroll(now);
        visible_change |= self.tick_clip_gesture_autoscroll(now, cx);
        visible_change |= self.tick_edit_cursor(now);
        let project_edit_pending = self.edit_pending();
        visible_change |= self.retire_stale_spectrum_target();
        visible_change |= self.retire_stale_processor_audition_target();
        if let Some(audio) = self.audio.as_mut() {
            let _ = audio.control.spectrum_retry_pending();
            let _ = audio.control.audition_retry_pending();
            let desired_spectrum = audio.control.spectrum_desired();
            let spectrum = audio.control.spectrum_latest_pair();
            let spectrum = spectrum.filter(|result| {
                desired_spectrum.is_some_and(|target| {
                    result.output.stamp.target == target.token
                        && result.output.stamp.generation == target.project_generation
                        && result.output.stamp.sample_rate_hz == target.sample_rate_hz
                        && result.output.stamp.applied_revision >= target.activation_floor
                })
            });
            if self.device_panel.spectrum_result != spectrum {
                self.device_panel.spectrum_result = spectrum;
                visible_change = true;
            }
            audio.control.reclaim_preview();
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
        visible_change |= self.tick_scrub();
        visible_change |= self.poll_resident_publication();
        let Some(audio) = self.audio.as_mut() else {
            let pending_after = self
                .worker_requests_pending
                .saturating_add(self.pending_requests.len());
            visible_change |= pending_before != pending_after;
            return self.observe_audio_frame(None, visible_change, Instant::now());
        };
        let seek_publication_inflight = self.prepared_seek.as_ref().is_some_and(|seek| {
            matches!(
                seek.stage,
                super::arrangement_seek::SeekStage::Published { .. }
            )
        });
        if !seek_publication_inflight
            && self.pending_resident_publication.is_none()
            && self.playback_candidates.len() < super::PLAYBACK_CANDIDATE_CAPACITY
            && let Some(pending) = self.pending_playback.take()
        {
            let prepared_revision = pending
                .prepared_revision
                .clone()
                .or_else(|| pending.playback.prepared_revision().cloned());
            let policy = if let Some(fence) = pending.fresh_runtime_fence {
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence)
            } else if pending.install_fresh_runtime {
                PublicationPolicy::InstallFreshRuntime
            } else {
                PublicationPolicy::InheritRuntime
            };
            if self.document_audio_mode == hex_session::DocumentAudioMode::VerifiedStreaming {
                self.error =
                    Some("verified streaming does not accept resident playback owners".into());
                drop(pending.playback);
            } else {
                let PendingPlayback {
                    project_revision,
                    project_generation,
                    state,
                    playback,
                    install_fresh_runtime,
                    fresh_runtime_fence,
                    seek_request_id,
                    device_preview_identity,
                    tuning_preview_identity,
                    ..
                } = pending;
                match audio.control.forward_resident_publication(
                    playback,
                    policy,
                    project_generation,
                ) {
                    Ok(token) => {
                        self.pending_resident_publication = Some(PendingResidentPublication {
                            token,
                            project_revision,
                            project_generation,
                            state,
                            install_fresh_runtime,
                            fresh_runtime_fence,
                            prepared_revision,
                            seek_request_id,
                            device_preview_identity,
                            tuning_preview_identity,
                        });
                        self.status = "audio publication queued · waiting for worker".into();
                    }
                    Err(rejected) => {
                        self.pending_playback = Some(PendingPlayback {
                            project_revision,
                            project_generation,
                            state,
                            playback: rejected.playback,
                            install_fresh_runtime,
                            fresh_runtime_fence,
                            prepared_revision,
                            seek_request_id,
                            device_preview_identity,
                            tuning_preview_identity,
                        });
                        if rejected.reason != "resident publication busy"
                            && self.error.as_deref() != Some(rejected.reason)
                        {
                            visible_change |=
                                replace_error(&mut self.error, &rejected.reason.to_lowercase());
                        }
                    }
                }
            }
        } else if self.playback_candidates.len() >= super::PLAYBACK_CANDIDATE_CAPACITY {
            visible_change |= replace_status(
                &mut self.status,
                "audio graph queue full · waiting for callback",
            );
        }
        if let Some((ticket, reason)) = audio
            .control
            .status()
            .and_then(|status| status.rejected_action())
            && ticket > self.observed_rejected_action
        {
            self.observed_rejected_action = ticket;
            if self.pending_seek_ticket == Some(ticket) {
                self.pending_seek_ticket = None;
            }
            visible_change |= replace_error(&mut self.error, reason.message());
        }
        if let Some(capability) = audio
            .control
            .status()
            .and_then(|status| status.session_capability())
        {
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
            .status()
            .and_then(|status| status.rejected_revision())
            .filter(|(revision, _)| *revision > self.observed_rejected_revision);
        if let Some((audio_revision, reason)) = rejected_revision {
            // Resolve the exact project-generation mapping before any applied
            // high-water reconciliation removes candidate metadata. Callback
            // telemetry is audio-revision-only, so stale/coalesced rejection
            // may describe an earlier project generation.
            if let Some(project) =
                rejected_candidate_project(&self.playback_candidates, audio_revision)
            {
                if self.stopped_playback_restore
                    == Some(super::audio_admission::StoppedPlaybackRestore::Published(
                        audio_revision,
                    ))
                {
                    self.stopped_playback_restore = None;
                }
                visible_change |=
                    self.audio_admission
                        .rejected(project, audio_revision, reason.message());
            } else if rejected_preview_candidate(
                &self.playback_candidates,
                audio_revision,
                self.project_generation,
                self.state.as_ref().map(|state| state.revision),
                active_gesture,
            ) || rejected_tuning_preview_candidate(
                &self.playback_candidates,
                audio_revision,
                self.project_generation,
                self.state.as_ref().map(|s| s.revision),
                self.tuning.client.snapshot().map(|s| (s.key, s.sequence)),
            ) {
                // Preview failures are transient, but remain visible even
                // though they do not change the canonical admission footer.
                visible_change |= replace_error(
                    &mut self.error,
                    &format!("preview rejected: {}", reason.message()),
                );
            }
        }
        let applied = audio
            .control
            .status()
            .map_or(0, |status| status.applied_revision.load(Acquire));
        let (reconciled, current_applied) = reconcile_verified_applied_source(
            &mut self.audio_admission,
            self.document_audio_mode,
            audio.control.output_identity(),
            self.project_generation,
            self.state.as_ref().map(|state| state.revision),
            audio
                .control
                .status()
                .and_then(|status| status.applied_audio_source()),
        );
        visible_change |= reconciled;

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
            let source = applied_tuning_candidate_source(
                &candidate,
                self.project_generation,
                self.state.as_ref().map(|s| s.revision),
                self.tuning.client.snapshot().map(|s| (s.key, s.sequence)),
            )
            .unwrap_or_else(|| {
                applied_candidate_source(
                    &candidate,
                    self.project_generation,
                    self.state.as_ref().map(|state| state.revision),
                    active_gesture,
                )
            });
            if source == AppliedCandidateSource::Current {
                if candidate.project_generation == self.project_generation
                    && candidate.seek_request_id.is_none()
                {
                    self.applied_project_revision = Some(candidate.state.revision);
                    if matches!(
                        self.stopped_playback_restore,
                        Some(super::audio_admission::StoppedPlaybackRestore::Published(required))
                            if candidate.audio_revision >= required
                    ) {
                        self.stopped_playback_restore = None;
                    }
                }
                self.audible_playback = Some(candidate);
            } else if source == AppliedCandidateSource::InvalidPreview {
                visible_change |= replace_error(
                    &mut self.error,
                    "device preview source did not match its prepared revision",
                );
            }
        }
        if let Some(source) = current_applied {
            restore_verified_recovery_from_applied(
                &mut self.recovery,
                self.pending_recovery_generation,
            );
            // Actual ACK can arrive after Submitted metadata was coalesced.
            // Reconstruct only nonowning canonical view metadata, never an
            // audio owner or a state for an older, unavailable Project snapshot.
            if let Some(state) = self
                .state
                .as_ref()
                .filter(|state| state.revision == source.context.editor_revision)
            {
                self.applied_project_revision = Some(source.context.editor_revision);
                if self.audible_playback.as_ref().is_none_or(|candidate| {
                    candidate.audio_revision != source.audio_revision
                        || candidate.project_generation != source.context.project_generation
                }) {
                    self.audible_playback = Some(PlaybackCandidate {
                        audio_revision: source.audio_revision,
                        project_generation: source.context.project_generation,
                        state: Arc::clone(state),
                        prepared_revision: None,
                        seek_request_id: None,
                        device_preview_identity: None,
                        tuning_preview_identity: None,
                    });
                }
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
                    if matches!(pending.action, PlaybackAction::SetMetronome(_)) {
                        self.pending_metronome_ticket = Some(ticket);
                    }
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
        let consumed_command = audio.control.status().map_or(0, |status| {
            audio_command_prefix(status, self.document_audio_mode)
        });
        let actual_playing = audio
            .control
            .status()
            .is_some_and(|status| status.playing.load(Acquire));
        if self
            .pending_metronome_ticket
            .is_some_and(|ticket| consumed_command >= ticket)
        {
            let rejected = audio
                .control
                .status()
                .and_then(|status| status.rejected_action())
                .is_some_and(|(ticket, _)| self.pending_metronome_ticket == Some(ticket));
            self.pending_metronome_ticket = None;
            visible_change = true;
            if rejected {
                replace_error(&mut self.error, "metronome request rejected · try again");
            }
        }
        let rejected_action_ticket = audio
            .control
            .status()
            .and_then(|status| status.rejected_action())
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
        if audio
            .control
            .status()
            .is_some_and(|status| status.device_fault.load(Relaxed))
        {
            self.session_routes.clear();
            self.pending_audio_commands.clear();
            self.pending_metronome_ticket = None;
            self.pending_session_ticket = None;
            self.pending_seek_ticket = None;
            self.play_intent.clear();
            visible_change |= replace_status(&mut self.status, "audio device unavailable");
            visible_change |= replace_error(
                &mut self.error,
                "audio device failed · editing and saving remain available",
            );
        } else if audio
            .control
            .status()
            .is_some_and(|status| status.render_fault.load(Relaxed))
        {
            self.session_routes.clear();
            self.pending_audio_commands.clear();
            self.pending_metronome_ticket = None;
            self.pending_seek_ticket = None;
            self.pending_session_ticket = None;
            self.play_intent.clear();
            visible_change |= replace_status(&mut self.status, "audio render fault · stopped");
            let diagnostic = audio.control.status().and_then(|status| status.render_fault_snapshot()).map_or_else(
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
        if audio.control.status().is_some_and(|status| {
            !status.device_fault.load(Relaxed)
                && !status.render_fault.load(Relaxed)
                && !status.shutdown_complete.load(Acquire)
        }) {
            if let Some(identity) = audio.control.output_identity() {
                if let Some(status) = audio.control.status() {
                    visible_change |= self.session_routes.poll(identity, status);
                }
            }
        } else {
            self.session_routes.clear();
        }
        let frame = audio.control.status().map(AudioFrame::capture);
        let pending_after = self
            .worker_requests_pending
            .saturating_add(self.pending_requests.len());
        visible_change |= pending_before != pending_after;
        visible_change |= self.finish_audio_recovery_tick();
        self.observe_audio_frame(frame, visible_change, Instant::now())
    }

    fn spectrum_target_is_visible_and_current(
        &self,
        target: hex_audio::spectrum_target::SpectrumTarget,
    ) -> bool {
        if !self.clip_view_open
            || !self.device_panel_active
            || self.project_generation != target.project_generation
            || !matches!(
                target.identity.device_type.as_str(),
                Ok("hex.eq8" | "hex.eq")
            )
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
            .and_then(|audio| audio.control.spectrum_desired());
        if target.is_some_and(|target| !self.spectrum_target_is_visible_and_current(target)) {
            if let Some(audio) = self.audio.as_mut() {
                let _ = audio.control.spectrum_clear();
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
            || !matches!(
                target.identity.device_type.as_str(),
                Ok("hex.eq8" | "hex.eq")
            )
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
            .and_then(|audio| audio.control.audition_desired());
        if target
            .is_some_and(|target| !self.processor_audition_target_is_visible_and_current(target))
        {
            if let Some(audio) = self.audio.as_mut() {
                let _ = audio.control.audition_cancel();
            }
            super::device_panel::clear_eq8_audition_gesture_flags(self);
            return true;
        }
        false
    }

    fn receive(&mut self, event: Event) -> Option<std::path::PathBuf> {
        if event.project_replaced {
            self.invalidate_prepared_seek();
            if let Some(audio) = self.audio.as_mut() {
                audio.control.close();
            }
            self.audio = None;
            let next_generation = self.project_generation.saturating_add(1);
            if self
                .pending_audio_result
                .as_ref()
                .is_some_and(|pending| pending.project_generation() < next_generation)
            {
                if let Some(pending) = self.pending_audio_result.take() {
                    pending.close();
                }
            }
            if self
                .pending_restart_request
                .is_some_and(|(generation, _)| generation < next_generation)
            {
                self.pending_restart_request = None;
            }
            self.pending_recovery_generation = None;
            self.recovery.fail();
            self.pending_playback = None;
            self.pending_resident_publication = None;
            self.prepared_seek = None;
            self.latched_seek_intent = None;
            self.pending_audio_commands.clear();
            self.pending_seek_ticket = None;
            self.pending_metronome_ticket = None;
            self.pending_session_ticket = None;
            self.playback_candidates.clear();
            self.audible_playback = None;
            self.audio_admission.reset_audio();
        }
        if event.stopped_playback_restore_complete
            && event.error.is_some()
            && event.playback.is_none()
        {
            self.stopped_playback_restore = None;
        }
        if event.project_replaced {
            self.stopped_playback_restore = None;
        }
        if event.request_complete
            && event.computer_keyboard_complete.is_none()
            && event.error.is_some()
            && event.playback.is_none()
        {
            self.gain_recorder.arm_failed();
        }
        if event
            .audio_batch_complete
            .as_ref()
            .is_some_and(|completed| {
                self.audio_batch
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(active, completed))
            })
        {
            self.audio_batch = None;
            self.audio_batch_progress = None;
        }
        if event.save_complete {
            self.saving_project = false;
        }
        if event.request_complete {
            self.worker_requests_pending = self.worker_requests_pending.saturating_sub(1);
        }
        if let Some(input) = &event.live_expression_complete {
            if self
                .state
                .as_ref()
                .is_some_and(|state| state.revision == event.state.revision)
                && self.controller_input_matches(input)
            {
                self.connect_prepared_controller();
            } else {
                input.cancel();
                return None;
            }
        }
        if event.open_complete {
            self.opening_project = false;
            self.pending_open_path = None;
            self.pending_open_mode = None;
            self.pending_new_mode = None;
            self.duplicate_intent.cancel_chain();
            if !event.project_replaced || event.error.is_some() {
                self.requested_audio_mode = None;
            }
        }
        if event
            .restore_complete
            .is_some_and(|generation| self.recovery.generation() != Some(generation))
        {
            return None;
        }
        if let Some(input) = event.tuning_audition_complete.as_ref() {
            let current = self.tuning.open
                && input.project_generation == self.project_generation
                && self
                    .tuning
                    .audition_input
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(active, input))
                && !input.is_cancelled()
                && self
                    .state
                    .as_ref()
                    .is_some_and(|state| state.revision == event.state.revision);
            if !current {
                input.cancel();
                return None;
            }
        }
        if let Some(request_id) = event.computer_keyboard_complete {
            let current = self.computer_keyboard.enabled
                && !self.computer_keyboard.suspended
                && request_id == self.computer_keyboard.request_id
                && self.computer_keyboard.target
                    == self
                        .selected_track
                        .map(|track| (self.project_generation, event.state.revision, track))
                && self
                    .computer_keyboard
                    .input
                    .as_ref()
                    .is_some_and(|input| !input.is_cancelled())
                && self
                    .state
                    .as_ref()
                    .is_some_and(|state| state.revision == event.state.revision);
            if current {
                if let Some(error) = event.error {
                    self.error = Some(error.into());
                }
                if let Some(playback) = event.playback {
                    let previous_reset = self
                        .pending_playback
                        .as_ref()
                        .map(|pending| (pending.project_generation, pending.install_fresh_runtime));
                    let mut pending = pending_playback_for_state(
                        Arc::clone(&event.state),
                        self.project_generation,
                        playback,
                    );
                    pending.install_fresh_runtime = retained_install_fresh_policy(
                        false,
                        self.project_generation,
                        previous_reset,
                    );
                    self.pending_playback = Some(pending);
                }
            }
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
        let scene_duplicate_intent =
            if event.request_complete && !event.gesture_request && !event.tuning_gesture_request {
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
        } else if !event.gesture_request && !event.tuning_gesture_request {
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
        self.gain_lane.reconcile(
            self.project_generation,
            revision,
            &event.state.project,
            event.project_replaced,
        );
        // Reuse the native preview identity gate: an older worker preview must
        // not retune external clients after a newer gesture or cancel won.
        let mts_tuning = match event.tuning_preview.as_ref() {
            Some(preview)
                if tuning_preview_current(
                    preview.identity,
                    self.project_generation,
                    revision,
                    self.project_generation,
                    Some(revision),
                    self.tuning
                        .client
                        .snapshot()
                        .map(|snapshot| (snapshot.key, snapshot.sequence)),
                ) =>
            {
                Some(preview.state.project.tuning())
            }
            Some(_) => None,
            None => Some(event.state.project.tuning()),
        };
        let mts_generation = if event.project_replaced {
            self.project_generation.saturating_add(1)
        } else {
            self.project_generation
        };
        if let Some(tuning) = mts_tuning {
            self.tuning
                .broadcast
                .observe(mts_generation, revision, tuning);
        }
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
            self.document_audio_mode = self
                .requested_audio_mode
                .take()
                .or_else(|| {
                    self.state
                        .as_ref()
                        .and_then(|state| state.canonical_audio_source.as_ref())
                        .map(|snapshot| snapshot.audio_mode())
                })
                .unwrap_or(hex_session::DocumentAudioMode::Resident);
            if self
                .pending_audio_result
                .as_ref()
                .is_some_and(|pending| pending.project_generation() <= self.project_generation)
            {
                if let Some(pending) = self.pending_audio_result.take() {
                    match pending {
                        PendingEditorAudioResult::Opened {
                            project_generation,
                            restart_request,
                            audio,
                        } if audio_event_disposition(
                            self.project_generation,
                            self.pending_restart_request,
                            project_generation,
                            restart_request,
                        ) == AudioEventDisposition::Current =>
                        {
                            self.document_audio_mode = audio.mode;
                            self.audio = Some(audio);
                            self.pending_restart_request = None;
                        }
                        PendingEditorAudioResult::Unavailable {
                            project_generation,
                            restart_request,
                            error,
                        } if audio_event_disposition(
                            self.project_generation,
                            self.pending_restart_request,
                            project_generation,
                            restart_request,
                        ) == AudioEventDisposition::Current =>
                        {
                            let message = format!("audio unavailable · {}", error.to_lowercase());
                            self.status = message.clone();
                            self.error = Some(message.into());
                            self.pending_restart_request = None;
                        }
                        result => result.close(),
                    }
                }
            }
            self.device_gesture.reset();
            self.device_gesture_barriers.clear();
            self.edit_cursor = None;
            self.clip_paint_gesture = None;
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
            self.pending_resident_publication = None;
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
            self.track_selection = super::selection::TrackSelection::default();
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
            self.track_selection.reconcile(&state.project);
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
        if let Some(ack) = event.tuning_gesture_ack.as_ref() {
            let key = self.tuning.client.snapshot().map(|s| s.key);
            self.tuning.client.acknowledge(ack);
            if !event.tuning_gesture_request
                && event.active_tuning_gesture.is_none()
                && key == Some(ack.key)
            {
                self.tuning.client.reset();
            }
            if let Some(reason) = ack.reason.as_ref() {
                self.tuning.error = Some(reason.clone());
            }
        }
        if event.project_replaced {
            self.tuning.client.reset();
        }
        if let Some(result) = event.tuning_library.as_ref() {
            self.tuning_library_complete(result);
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
            self.error = Some(reason.clone().into());
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
        let mut stopped_restore_error = None;
        let event_playback = event.playback;
        if self.document_audio_mode == hex_session::DocumentAudioMode::VerifiedStreaming
            && event_playback.is_some()
        {
            self.error = Some("worker returned resident playback for verified streaming".into());
        }
        if let Some(mut playback) = event_playback
            .filter(|_| self.document_audio_mode == hex_session::DocumentAudioMode::Resident)
        {
            if event.stopped_playback_restore_complete {
                self.stopped_playback_restore =
                    Some(super::audio_admission::StoppedPlaybackRestore::Ready);
                // Preserve the transient click toggle across the stopped reset.
                // Voice/filter histories and the transport still start fresh.
                if let Err(reason) = playback
                    .try_apply_action(PlaybackAction::SetMetronome(self.metronome_enabled()))
                {
                    stopped_restore_error = Some(reason.message().to_owned());
                }
            }
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
                .or_else(|| {
                    event
                        .tuning_preview
                        .as_ref()
                        .filter(|p| {
                            p.identity.project_generation == self.project_generation
                                && p.identity.canonical_revision == revision
                        })
                        .map(|p| Arc::clone(&p.state))
                })
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
            pending.tuning_preview_identity = event
                .tuning_preview
                .as_ref()
                .filter(|p| {
                    p.identity.project_generation == self.project_generation
                        && p.identity.canonical_revision == revision
                })
                .map(|p| p.identity);
            pending.install_fresh_runtime = retained_install_fresh_policy(
                event.project_replaced || event.stopped_playback_restore_complete,
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
            self.error = Some(
                event
                    .audio_batch_failure
                    .map_or_else(|| error.into(), Into::into),
            );
        } else if let Some(error) = stopped_restore_error {
            self.error = Some(error.into());
        }
        if let Some(message) = playback_capability {
            self.session_capability_message = message.clone();
            if let Some(message) = message {
                self.error = Some(message.into());
            }
        }
        self.reconcile_publication_observation();
        if event.request_complete
            && !event.gesture_request
            && !event.tuning_gesture_request
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
        if self.gain_recorder.blocks_edits() {
            self.error =
                Some("finish or cancel gain recording before another project action".into());
            return false;
        }
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
        if let Request::DropBrowserDeviceOnArrangement { destination, .. } = &request {
            self.last_enqueued_arrangement_device_drop = Some(*destination);
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
        if let Request::ImportAudioBatch {
            paths, destination, ..
        } = &request
        {
            if let [path] = paths.as_slice() {
                match destination {
                    hex_session::import_batch::AudioBatchDestination::Track { track, start } => {
                        self.last_enqueued_import_to_track =
                            Some((path.clone(), *track, *start, true))
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
        if request.is_project_open() {
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
        if !request.is_project_open() {
            self.pending_open_path = None;
        }
        if matches!(
            &request,
            Request::DeviceGesture(_)
                | Request::DropNativeDevice { .. }
                | Request::DropBrowserDeviceOnArrangement { .. }
                | Request::AddNativeDevice { .. }
                | Request::ReplaceNativeDevice { .. }
                | Request::LoadDevicePreset { .. }
                | Request::SetDeviceParameter { .. }
                | Request::Import(_)
                | Request::ImportAudioBatch { .. }
                | Request::ImportToTrack { .. }
                | Request::ImportToTrackWithOverwrite { .. }
                | Request::ImportToNewTrack { .. }
                | Request::ImportToSession { .. }
                | Request::Open(_)
                | Request::OpenWith { .. }
                | Request::NewWithAudioMode { .. }
                | Request::OpenWithAudioMode { .. }
                | Request::GainLane { .. }
                | Request::Edit(_)
                | Request::ConvertClip { .. }
                | Request::CutTimeRange(_)
                | Request::PasteTimeRange { .. }
                | Request::InsertTimeRange { .. }
                | Request::Undo
                | Request::Redo
                | Request::RestorePlayback { .. }
                | Request::RestoreStoppedPlayback
        ) {
            self.invalidate_prepared_seek();
        }
        if matches!(
            &request,
            Request::Open(_)
                | Request::OpenWith { .. }
                | Request::NewWithAudioMode { .. }
                | Request::OpenWithAudioMode { .. }
                | Request::Undo
                | Request::Redo
                | Request::SaveNew(_)
                | Request::SaveCurrent
                | Request::SaveDevicePreset { .. }
                | Request::LoadDevicePreset { .. }
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
        if self.stopped_playback_restore.is_some() {
            self.error = Some("restoring stopped playback · wait before seeking".into());
            return;
        }
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
        let consumed = self.audio_status().map_or(0, |status| {
            audio_command_prefix(status, self.document_audio_mode)
        });
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

    pub(super) fn metronome_enabled(&self) -> bool {
        self.audio_status()
            .is_some_and(|status| status.metronome_enabled.load(Acquire))
    }

    pub(super) fn metronome_pending(&self) -> bool {
        self.pending_metronome_ticket.is_some_and(|ticket| {
            self.audio_status().is_some_and(|status| {
                audio_command_prefix(status, self.document_audio_mode) < ticket
            })
        }) || self
            .pending_audio_commands
            .iter()
            .any(|pending| matches!(pending.action, PlaybackAction::SetMetronome(_)))
    }

    pub(super) fn toggle_metronome(&mut self) {
        if !self.metronome_pending() {
            self.queue_playback_action(
                PlaybackAction::SetMetronome(!self.metronome_enabled()),
                None,
            );
        }
    }

    fn queue_playback_action(&mut self, action: PlaybackAction, revision: Option<u64>) {
        if self.stopped_playback_restore.is_some() && intent_desired_playing(action) == Some(true) {
            self.error = Some("restoring stopped playback · wait before playing".into());
            return;
        }
        if !matches!(action, PlaybackAction::SetMetronome(_)) {
            self.invalidate_prepared_seek();
        }
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
                    if matches!(action, PlaybackAction::SetMetronome(_)) {
                        self.pending_metronome_ticket = Some(ticket);
                    }
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
        // Stop explicitly discards voice histories. A previously refused edit
        // or restoration can now install the canonical sound without inheriting
        // an incompatible preview's history. Keep construction on the worker.
        let restore = self.stopped_playback_restore.is_none()
            && !self.edit_pending()
            && self.device_gesture.snapshot().is_none()
            && self.tuning.client.snapshot().is_none()
            && self.recovery.generation().is_none()
            && self.audio.as_ref().is_some_and(|audio| {
                self.state.as_ref().is_some_and(|state| {
                    self.applied_project_revision != Some(state.revision)
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
        self.cancel_computer_keyboard();
        self.computer_keyboard.suspended = true;
        self.cancel_scrub();
        self.invalidate_prepared_seek();
        self.offline_cursor = 0;
        self.recovery.stop();
        self.pending_audio_commands.clear();
        self.pending_metronome_ticket = None;
        self.pending_seek_ticket = None;
        self.pending_session_ticket = None;
        self.play_intent.priority_stop();
        if let Some(audio) = &self.audio {
            audio.control.stop();
        }
        self.status = "stopped · returned to start".into();
        if restore && self.enqueue(Request::RestoreStoppedPlayback) {
            self.stopped_playback_restore =
                Some(super::audio_admission::StoppedPlaybackRestore::Preparing);
            self.status = "stopped · restoring current sound".into();
            self.error = None;
        }
    }
}

// Called only after exact current context/output identity has been reconciled.
fn restore_verified_recovery_from_applied(
    recovery: &mut super::recovery_state::RecoveryState,
    pending: Option<u64>,
) {
    if let Some(generation) = pending {
        recovery.restored(generation);
    }
}

fn reconcile_verified_applied_source(
    admission: &mut super::audio_admission::AudioAdmissionStatus,
    mode: hex_session::DocumentAudioMode,
    output_identity: Option<u64>,
    generation: u64,
    current_revision: Option<u64>,
    source: Option<hex_audio::prepared::AppliedAudioSource>,
) -> (bool, Option<hex_audio::prepared::AppliedAudioSource>) {
    let Some(source) = source.filter(|source| {
        mode == hex_session::DocumentAudioMode::VerifiedStreaming
            && output_identity == Some(source.output_identity)
            && source.context.project_generation == generation
    }) else {
        return (false, None);
    };
    let changed = admission.applied(
        super::audio_admission::ProjectRevision {
            generation,
            revision: source.context.editor_revision,
        },
        source.audio_revision,
    );
    (
        changed,
        (current_revision == Some(source.context.editor_revision)).then_some(source),
    )
}

fn audio_command_prefix(
    status: &hex_audio::prepared::Status,
    mode: hex_session::DocumentAudioMode,
) -> u64 {
    if mode == hex_session::DocumentAudioMode::VerifiedStreaming {
        status.stream_applied_command_prefix.load(Acquire)
    } else {
        status.consumed_command.load(Acquire)
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
                && candidate.tuning_preview_identity.is_none()
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
                && candidate.tuning_preview_identity.is_none()
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppliedCandidateSource {
    Current,
    SupersededPreview,
    InvalidPreview,
}

fn applied_candidate_source(
    candidate: &PlaybackCandidate,
    generation: u64,
    revision: Option<u64>,
    active: Option<(GestureKey, u64)>,
) -> AppliedCandidateSource {
    let Some(identity) = candidate.device_preview_identity else {
        return AppliedCandidateSource::Current;
    };
    // A terminal gesture or newer owner/sequence can overtake callback receipt.
    // Keep such notifications out of current state and error reporting.
    if identity.project_generation != generation
        || Some(identity.canonical_revision) != revision
        || active != Some((identity.key, identity.sequence))
    {
        return AppliedCandidateSource::SupersededPreview;
    }
    if preview_source_is_current(
        identity,
        candidate.project_generation,
        candidate.state.revision,
        generation,
        revision,
        active,
    ) {
        AppliedCandidateSource::Current
    } else {
        AppliedCandidateSource::InvalidPreview
    }
}

fn tuning_preview_current(
    identity: hex_session::service::TuningPreviewIdentity,
    candidate_generation: u64,
    candidate_revision: u64,
    generation: u64,
    revision: Option<u64>,
    active: Option<(hex_session::tuning_gesture::TuningGestureKey, u64)>,
) -> bool {
    identity.project_generation == candidate_generation
        && identity.project_generation == generation
        && identity.canonical_revision == candidate_revision
        && Some(identity.canonical_revision) == revision
        && active == Some((identity.key, identity.sequence))
}
fn applied_tuning_candidate_source(
    candidate: &PlaybackCandidate,
    generation: u64,
    revision: Option<u64>,
    active: Option<(hex_session::tuning_gesture::TuningGestureKey, u64)>,
) -> Option<AppliedCandidateSource> {
    let identity = candidate.tuning_preview_identity?;
    Some(
        if identity.project_generation != generation
            || Some(identity.canonical_revision) != revision
            || active != Some((identity.key, identity.sequence))
        {
            AppliedCandidateSource::SupersededPreview
        } else if candidate.device_preview_identity.is_none()
            && tuning_preview_current(
                identity,
                candidate.project_generation,
                candidate.state.revision,
                generation,
                revision,
                active,
            )
        {
            AppliedCandidateSource::Current
        } else {
            AppliedCandidateSource::InvalidPreview
        },
    )
}
fn rejected_tuning_preview_candidate(
    candidates: &VecDeque<PlaybackCandidate>,
    audio_revision: u64,
    generation: u64,
    revision: Option<u64>,
    active: Option<(hex_session::tuning_gesture::TuningGestureKey, u64)>,
) -> bool {
    candidates.iter().any(|candidate| {
        candidate.audio_revision == audio_revision
            && candidate.tuning_preview_identity.is_some_and(|identity| {
                tuning_preview_current(
                    identity,
                    candidate.project_generation,
                    candidate.state.revision,
                    generation,
                    revision,
                    active,
                )
            })
    })
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
        tuning_preview_identity: None,
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
        | PlaybackAction::Session(_)
        | PlaybackAction::SetMetronome(_) => None,
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
        PlaybackAction::Transport(_) | PlaybackAction::SetMetronome(_) => true,
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

fn replace_error(error: &mut Option<super::display_error::DisplayError>, message: &str) -> bool {
    if error.as_deref() == Some(message) {
        false
    } else {
        *error = Some(message.into());
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
mod audio_event_correlation_tests {
    use super::{
        AudioEventDisposition as D, PublicationObservationDisposition as P,
        audio_event_disposition, future_audio_result_replaces, publication_observation_disposition,
        publication_observation_replaces,
    };
    use hex_engine::AudioSourceContext;

    fn context(generation: u64, revision: u64) -> AudioSourceContext {
        AudioSourceContext {
            project_generation: generation,
            editor_revision: revision,
        }
    }

    #[test]
    fn automatic_same_generation_result_cannot_replace_explicit_restart() {
        let pending = Some((7, 41));
        assert_eq!(audio_event_disposition(7, pending, 7, None), D::Ignore);
        // Both automatic success and automatic failure use the same correlation rule.
        assert_eq!(audio_event_disposition(7, pending, 7, Some(41)), D::Current);
        assert_eq!(audio_event_disposition(7, pending, 7, Some(40)), D::Ignore);
    }

    #[test]
    fn future_unavailable_result_is_current_after_canonical_generation_advances() {
        assert_eq!(audio_event_disposition(7, None, 8, None), D::Future);
        assert_eq!(audio_event_disposition(8, None, 8, None), D::Current);
    }

    #[test]
    fn stale_results_and_unmatched_restart_requests_are_ignored() {
        assert_eq!(audio_event_disposition(8, None, 7, None), D::Ignore);
        assert_eq!(
            audio_event_disposition(8, Some((8, 41)), 8, Some(40)),
            D::Ignore
        );
        assert_eq!(
            audio_event_disposition(8, Some((8, 41)), 9, Some(41)),
            D::Ignore
        );
    }

    #[test]
    fn future_auto_result_supersedes_older_restart_and_replays_on_new_generation() {
        let older_restart = Some((7, 41));
        assert_eq!(
            audio_event_disposition(7, older_restart, 8, None),
            D::Future
        );
        assert!(future_audio_result_replaces(Some(7), 8));
        assert_eq!(audio_event_disposition(8, None, 8, None), D::Current);
        assert!(!future_audio_result_replaces(Some(9), 8));
    }

    #[test]
    fn future_result_stays_queued_until_its_generation_and_replaces_only_older_results() {
        assert!(future_audio_result_replaces(None, 10));
        assert!(!future_audio_result_replaces(Some(10), 9));
        assert!(future_audio_result_replaces(Some(10), 11));
        assert_eq!(audio_event_disposition(8, None, 10, None), D::Future);
        assert_eq!(audio_event_disposition(10, None, 10, None), D::Current);
    }

    #[test]
    fn submitted_waits_for_state_and_opened_client_in_either_order_then_consumes_once() {
        let submitted = context(5, 12);

        let mut pending = Some((submitted, 70));
        assert_eq!(
            publication_observation_disposition(
                5,
                Some(11),
                Some(context(5, 11)),
                None,
                submitted,
                70
            ),
            P::Wait
        );
        assert_eq!(
            publication_observation_disposition(5, Some(12), Some(submitted), None, submitted, 70),
            P::Wait
        );
        assert_eq!(
            publication_observation_disposition(
                5,
                Some(12),
                Some(submitted),
                Some(70),
                submitted,
                70
            ),
            P::Apply
        );
        if pending.is_some_and(|(context, identity)| {
            publication_observation_disposition(
                5,
                Some(12),
                Some(submitted),
                Some(70),
                context,
                identity,
            ) == P::Apply
        }) {
            pending.take();
        }
        assert!(
            pending.is_none(),
            "a matching submitted observation is consumed"
        );

        // The reverse arrival order (matching client before the canonical state).
        assert_eq!(
            publication_observation_disposition(
                5,
                Some(11),
                Some(context(5, 11)),
                Some(70),
                submitted,
                70
            ),
            P::Wait
        );
        assert_eq!(
            publication_observation_disposition(
                5,
                Some(12),
                Some(submitted),
                Some(70),
                submitted,
                70
            ),
            P::Apply
        );
    }

    #[test]
    fn same_document_newer_revision_is_held_until_that_revision_arrives() {
        let newer = context(5, 13);
        assert_eq!(
            publication_observation_disposition(
                5,
                Some(12),
                Some(context(5, 12)),
                Some(70),
                newer,
                70
            ),
            P::Wait
        );
        assert_eq!(
            publication_observation_disposition(5, Some(13), Some(newer), Some(70), newer, 70),
            P::Apply
        );
    }

    #[test]
    fn stale_contexts_and_wrong_output_roots_are_discarded() {
        let current = context(5, 13);
        assert_eq!(
            publication_observation_disposition(
                5,
                Some(13),
                Some(current),
                Some(70),
                context(4, 99),
                70
            ),
            P::Stale
        );
        assert_eq!(
            publication_observation_disposition(
                5,
                Some(13),
                Some(current),
                Some(70),
                context(5, 12),
                70
            ),
            P::Stale
        );
        assert_eq!(
            publication_observation_disposition(5, Some(13), Some(current), Some(71), current, 70),
            P::Stale
        );
    }

    #[test]
    fn newer_publication_context_cannot_be_overwritten_by_an_older_observation() {
        let newest = context(6, 2);
        let older = context(5, 900);
        assert!(publication_observation_replaces(None, newest));
        assert!(!publication_observation_replaces(Some(newest), older));
        assert!(publication_observation_replaces(Some(older), newest));
        assert!(publication_observation_replaces(Some(newest), newest));
    }
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

    #[test]
    fn delayed_preview_application_distinguishes_superseded_and_invalid_current_sources() {
        use super::{AppliedCandidateSource, applied_candidate_source};
        let preview = preview_candidate(17, 40, 3);
        let identity = preview.device_preview_identity.unwrap();
        let active = Some((identity.key, identity.sequence));
        let mut queued = VecDeque::from([candidate(16, 40, 3), preview.clone()]);
        let applied = promote_playback_candidates(&mut queued, 17, None).unwrap();
        assert_eq!(
            applied_candidate_source(&applied, 3, Some(40), active),
            AppliedCandidateSource::Current
        );
        for (generation, revision, active) in [
            (3, Some(40), None),   // cancel/commit terminal retired the gesture
            (3, Some(41), active), // canonical commit overtook callback receipt
            (4, Some(40), active), // project/owner replaced
            (3, Some(40), Some((identity.key, identity.sequence + 1))),
            (
                3,
                Some(40),
                Some((
                    GestureKey {
                        generation: 3,
                        token: identity.key.token + 1,
                    },
                    identity.sequence,
                )),
            ),
        ] {
            assert_eq!(
                applied_candidate_source(&applied, generation, revision, active),
                AppliedCandidateSource::SupersededPreview
            );
        }
        let mut wrong_generation = preview.clone();
        wrong_generation.project_generation = 2;
        assert_eq!(
            applied_candidate_source(&wrong_generation, 3, Some(40), active),
            AppliedCandidateSource::InvalidPreview
        );
        let mut wrong_revision = preview_candidate(18, 39, 3);
        wrong_revision.device_preview_identity = Some(identity);
        assert_eq!(
            applied_candidate_source(&wrong_revision, 3, Some(40), active),
            AppliedCandidateSource::InvalidPreview
        );
        let mut rejected = VecDeque::from([preview]);
        assert!(promote_playback_candidates(&mut rejected, 17, Some(17)).is_none());
        assert_eq!(
            applied_candidate_source(&candidate(18, 40, 3), 3, Some(40), None),
            AppliedCandidateSource::Current
        );
    }
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
                canonical_audio_source: None,
                project,
                waveforms: Default::default(),
                audio_clip_waveforms: std::collections::BTreeMap::new(),
                midi_preview: Arc::new(hex_engine::MidiArrangementPreview::default()),
                gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
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
            tuning_preview_identity: None,
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

#[cfg(test)]
mod verified_actual_applied_tests {
    use super::*;
    #[test]
    fn actual_current_ack_restores_recovery_without_a_submitted_event() {
        let mut recovery = super::super::recovery_state::RecoveryState::default();
        let generation = recovery.begin(640).unwrap();
        assert!(recovery.opened(generation));
        assert_eq!(recovery.seek_needed(true), None);
        restore_verified_recovery_from_applied(&mut recovery, Some(generation + 1));
        assert_eq!(recovery.seek_needed(true), None);
        restore_verified_recovery_from_applied(&mut recovery, Some(generation));
        restore_verified_recovery_from_applied(&mut recovery, Some(generation));
        assert_eq!(recovery.seek_needed(true), Some(640));
        assert_eq!(recovery.seek_needed(false), None);
    }

    #[test]
    fn coalesced_submission_keeps_actual_audible_context_when_newer_edit_refuses() {
        use super::super::audio_admission::{AudioAdmissionStatus, ProjectRevision};
        let mut admission = AudioAdmissionStatus::default();
        let mode = hex_session::DocumentAudioMode::VerifiedStreaming;
        let source = hex_audio::prepared::AppliedAudioSource {
            context: hex_engine::AudioSourceContext {
                project_generation: 2,
                editor_revision: 7,
            },
            audio_revision: 11,
            output_identity: 13,
        };
        // Submitted(A) was never received; B is already canonical/refused.
        admission.canonical_revision(ProjectRevision {
            generation: 2,
            revision: 8,
        });
        assert!(admission.unpublished_refusal(
            ProjectRevision {
                generation: 2,
                revision: 8
            },
            "audio publication refused",
        ));
        let (changed, current) = reconcile_verified_applied_source(
            &mut admission,
            mode,
            Some(13),
            2,
            Some(8),
            Some(source),
        );
        assert!(changed && current.is_none());
        assert_eq!(admission.last_audible().unwrap().project.revision, 7);
        assert_eq!(admission.current().unwrap().project.revision, 8);
        assert!(admission.label().contains("rev 8 rejected"));
        assert!(admission.label().contains("rev 7 audible"));
        assert_eq!(
            reconcile_verified_applied_source(&mut admission, mode, Some(13), 2, Some(8), None),
            (false, None)
        );
        assert!(admission.label().contains("rev 7 audible"));
        // Only exact current state/client correlation may recover seek metadata.
        assert_eq!(
            reconcile_verified_applied_source(
                &mut admission,
                mode,
                Some(99),
                2,
                Some(7),
                Some(source)
            ),
            (false, None)
        );
        let (_, current) = reconcile_verified_applied_source(
            &mut admission,
            mode,
            Some(13),
            2,
            Some(7),
            Some(source),
        );
        assert_eq!(current, Some(source));
        assert_eq!(
            reconcile_verified_applied_source(
                &mut admission,
                mode,
                Some(13),
                3,
                Some(7),
                Some(source)
            ),
            (false, None)
        );
        let actual_b = hex_audio::prepared::AppliedAudioSource {
            context: hex_engine::AudioSourceContext {
                project_generation: 2,
                editor_revision: 8,
            },
            audio_revision: 12,
            ..source
        };
        reconcile_verified_applied_source(
            &mut admission,
            mode,
            Some(13),
            2,
            Some(8),
            Some(actual_b),
        );
        assert_eq!(
            admission.current().unwrap().state,
            super::super::audio_admission::AdmissionState::Applied
        );
        assert!(!admission.unpublished_refusal(
            ProjectRevision {
                generation: 2,
                revision: 8
            },
            "late refusal"
        ));
    }
}
