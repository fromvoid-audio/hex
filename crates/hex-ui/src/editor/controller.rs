#[cfg(test)]
use super::EnqueuedNativeDeviceDrop;
use super::play_intent::AdmissionError;
use super::refresh::AudioFrame;
use super::seek::seek_is_pending;
use super::selection;
use super::{
    EditorView, PendingEditorAudioResult, PendingEditorPublicationObservation, PendingPlayback,
    PendingPlaybackAction, PendingResidentPublication, Playback, PlaybackCandidate,
    UI_EVENT_BUDGET, UI_REQUEST_CAPACITY,
};
use gpui::{App, Context};
use hex_audio::prepared::PublicationPolicy;
use hex_engine::playback::{PlaybackAction, PlaybackCommand};
use hex_session::device_gesture::GestureKey;
use hex_session::service::{DevicePreviewIdentity, Event, Request, SubmitError, ViewState};
use std::{
    collections::VecDeque,
    sync::Arc,
    sync::atomic::Ordering::{Acquire, Relaxed},
    time::Instant,
};

#[cfg(all(test, feature = "ui-test-support"))]
mod export_refusal_tests;
#[cfg(all(test, feature = "ui-test-support"))]
mod project_replacement_tests;
mod submission;
#[cfg(all(test, feature = "ui-test-support"))]
mod verified_gain_tests;

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
pub(super) enum PublicationObservationDisposition {
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

// A comparison may retain its exact source context while authoring advances. Normalize only
// contexts accepted by the sealed comparison binding; intermediate revisions remain stale.
pub(super) fn publication_observation_for_view(
    generation: u64,
    view: Option<&ViewState>,
    output: Option<u64>,
    observed: hex_engine::AudioSourceContext,
    observed_output: u64,
) -> PublicationObservationDisposition {
    let authored = view.map(|view| hex_engine::AudioSourceContext {
        project_generation: generation,
        editor_revision: view.revision,
    });
    let current = view.and_then(|view| {
        let source = view.canonical_audio_source.as_ref()?.context();
        Some(
            authored
                .filter(|context| view.audio_authoring_revision(*context) == Some(view.revision))
                .unwrap_or(source),
        )
    });
    let observed = view
        .filter(|view| view.audio_authoring_revision(observed) == Some(view.revision))
        .and(authored)
        .unwrap_or(observed);
    publication_observation_disposition(
        generation,
        view.map(|view| view.revision),
        current,
        output,
        observed,
        observed_output,
    )
}

fn relinquish_client_matches(
    generation: u64,
    revision: Option<u64>,
    output: Option<u64>,
    old_output: u64,
    target: hex_engine::AudioSourceContext,
) -> bool {
    generation == target.project_generation
        && revision.is_none_or(|r| r <= target.editor_revision)
        && output == Some(old_output)
}

impl EditorView {
    /// Consume only the worker's ordered comparison proof. Runtime/source owners,
    /// output identity, publication tickets and audio revision never change here.
    fn reconcile_comparison_authoring(&mut self, next: &Arc<ViewState>) {
        let Some(previous) = self.state.as_ref() else {
            return;
        };
        let Some(proof) = next.comparison_audio.as_ref() else {
            return;
        };
        if !proof.follows(previous, next, self.project_generation) {
            return;
        }
        let previous_revision = previous.revision;
        let matches = |state: &Arc<ViewState>, generation: u64| {
            generation == self.project_generation
                && state.revision == previous_revision
                && state
                    .canonical_audio_source
                    .as_ref()
                    .zip(next.canonical_audio_source.as_ref())
                    .is_some_and(|(old, retained)| Arc::ptr_eq(old, retained))
        };
        let canonical = |candidate: &PlaybackCandidate| {
            candidate.seek_request_id.is_none()
                && candidate.device_preview_identity.is_none()
                && candidate.tuning_preview_identity.is_none()
        };
        let resident_applied = self.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && self.audible_playback.as_ref().is_some_and(|candidate| {
                canonical(candidate)
                    && matches(&candidate.state, candidate.project_generation)
                    && self.audio.as_ref().is_some_and(|audio| {
                        audio.control.status().is_some_and(|status| {
                            status.applied_revision.load(Acquire) == candidate.audio_revision
                        })
                    })
            });
        let verified_applied = self.document_audio_mode
            == hex_session::DocumentAudioMode::VerifiedStreaming
            && self.audio.as_ref().is_some_and(|audio| {
                audio
                    .control
                    .status()
                    .and_then(|status| status.applied_audio_source())
                    .is_some_and(|source| {
                        audio.control.output_identity() == Some(source.output_identity)
                            && source.context.project_generation == self.project_generation
                            && previous.audio_authoring_revision(source.context)
                                == Some(previous_revision)
                    })
            });
        let applied_is_current = resident_applied || verified_applied;
        // This proof preserves the actual active provider/runtime and selected
        // stable TrackId. Advance only this input's authoring correlation; never
        // replace its lease, held notes, request identity or slot reservations.
        if self
            .computer_keyboard
            .input
            .as_ref()
            .is_some_and(|input| !input.is_cancelled())
            && let Some((generation, revision, _)) = &mut self.computer_keyboard.target
            && *generation == self.project_generation
            && *revision == previous_revision
        {
            *revision = next.revision;
        }

        if let Some(pending) = self.pending_playback.as_mut()
            && pending.seek_request_id.is_none()
            && pending.device_preview_identity.is_none()
            && pending.tuning_preview_identity.is_none()
            && matches(&pending.state, pending.project_generation)
        {
            pending.state = Arc::clone(next);
            pending.project_revision = next.revision;
        }
        if let Some(pending) = self.pending_resident_publication.as_mut()
            && pending.seek_request_id.is_none()
            && pending.device_preview_identity.is_none()
            && pending.tuning_preview_identity.is_none()
            && matches(&pending.state, pending.project_generation)
        {
            pending.state = Arc::clone(next);
            pending.project_revision = next.revision;
        }
        for candidate in &mut self.playback_candidates {
            if canonical(candidate) && matches(&candidate.state, candidate.project_generation) {
                candidate.state = Arc::clone(next);
            }
        }
        if let Some(candidate) = self.audible_playback.as_mut()
            && canonical(candidate)
            && matches(&candidate.state, candidate.project_generation)
        {
            candidate.state = Arc::clone(next);
        }
        self.audio_admission.comparison_revision(
            super::audio_admission::ProjectRevision {
                generation: self.project_generation,
                revision: previous_revision,
            },
            super::audio_admission::ProjectRevision {
                generation: self.project_generation,
                revision: next.revision,
            },
            applied_is_current,
        );
        if applied_is_current && self.applied_project_revision == Some(previous_revision) {
            self.applied_project_revision = Some(next.revision);
        }
    }

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
                self.worker_submitted();
                self.pending_restart_request = Some((project_generation, request_id));
                self.status = "preparing audio output".into();
                true
            }
            Err(error) => {
                self.pending_restart_request = None;
                self.error = Some(
                    format!(
                        "could not restart audio · {}",
                        super::user_errors::submit(&error)
                    )
                    .into(),
                );
                self.status = "audio unavailable".into();
                false
            }
        }
    }

    pub(super) fn retire_audio_for_future_generation(&mut self) {
        self.invalidate_prepared_seek();
        self.retire_audio_preserving_gain_take();
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

    /// Whether the current endpoint may stay open across a project replacement
    /// until the worker reports `OutputRebound` or `OutputReleased`. Only an
    /// idle Resident endpoint qualifies; anything else retires as before.
    fn hold_audio_across_replacement(&self, next_generation: u64) -> bool {
        self.audio.as_ref().is_some_and(|audio| {
            audio.mode == hex_session::DocumentAudioMode::Resident
                && audio.control.output_identity().is_some()
                && audio.control.status().is_some()
        }) && !self.gain_recorder.busy()
            && self.pending_audio_result.as_ref().is_none_or(|pending| {
                matches!(pending, PendingEditorAudioResult::Rebound { .. })
                    || pending.project_generation() < next_generation
            })
    }

    /// Keep the held endpoint for the current generation when the worker kept
    /// that exact output; otherwise request a fresh one.
    fn apply_output_rebound(&mut self, output_identity: u64) {
        if let Some(audio) = self
            .audio
            .as_mut()
            .filter(|audio| audio.control.output_identity() == Some(output_identity))
        {
            if audio.mode == hex_session::DocumentAudioMode::Resident
                && audio.control.worker_owns_graph_publication()
            {
                audio
                    .control
                    .bind_resident_transport(self.project_generation);
            }
            self.status = "audio ready".into();
            self.reconcile_publication_observation();
            return;
        }
        // The kept output has no live client here (it was already closed), so
        // retire anything stale and reopen through the ordinary restart.
        self.retire_audio_preserving_gain_take();
        self.restart_audio_for_current_document();
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
        if publication_observation_for_view(
            self.project_generation,
            self.state.as_deref(),
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
        let disposition = publication_observation_for_view(
            self.project_generation,
            self.state.as_deref(),
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
                if let crate::EditorPublicationState::VerifiedPreviewSubmitted { identity, .. } =
                    &pending.state
                    && self
                        .device_publication_view(pending.context, *identity)
                        .is_none()
                {
                    return false;
                }
                if let crate::EditorPublicationState::ResidentSubmitted { metadata, .. } =
                    &pending.state
                    && metadata.device_preview.is_some() != metadata.tuning_preview.is_some()
                    && self.resident_publication_view(*metadata).is_none()
                {
                    // Audio and canonical events use different bounded queues. Do not label a
                    // transient graph with the canonical view when its matching view is delayed.
                    return false;
                }
                if let Some(pending) = self.pending_publication_observation.take() {
                    self.apply_publication_observation(pending.context, pending.state);
                }
                true
            }
        }
    }

    fn device_publication_view(
        &self,
        context: hex_engine::AudioSourceContext,
        identity: hex_session::service::DevicePreviewIdentity,
    ) -> Option<Arc<ViewState>> {
        let current = self.state.as_ref()?;
        if context.project_generation != self.project_generation
            || current.audio_authoring_revision(context) != Some(current.revision)
        {
            return None;
        }
        self.resident_device_preview
            .as_ref()
            .filter(|preview| {
                preview.identity == identity
                    && identity.project_generation == self.project_generation
                    && identity.canonical_revision == current.revision
                    && preview.state.revision == current.revision
            })
            .map(|preview| Arc::clone(&preview.state))
    }

    fn resident_publication_view(
        &self,
        metadata: hex_session::service::ResidentPublicationMetadata,
    ) -> Option<Arc<ViewState>> {
        let current = self.state.as_ref()?;
        if metadata.context.project_generation != self.project_generation
            || current.audio_authoring_revision(metadata.context) != Some(current.revision)
        {
            return None;
        }
        match (metadata.device_preview, metadata.tuning_preview) {
            (Some(identity), None) => self.device_publication_view(metadata.context, identity),
            (None, Some(identity)) => self
                .resident_tuning_preview
                .as_ref()
                .filter(|preview| {
                    preview.identity == identity
                        && identity.project_generation == self.project_generation
                        && identity.canonical_revision == current.revision
                        && preview.state.revision == current.revision
                })
                .map(|preview| Arc::clone(&preview.state)),
            (None, None) => self
                .state
                .as_ref()
                .filter(|state| {
                    state.audio_authoring_revision(metadata.context) == Some(state.revision)
                })
                .cloned(),
            (Some(_), Some(_)) => None,
        }
    }

    fn reset_output_publication_tracking(&mut self, output_identity: Option<u64>) {
        self.device_panel.spectrum_results.clear();
        self.invalidate_prepared_seek();
        self.prepared_seek = None;
        self.playback_candidates.clear();
        self.audible_playback = None;
        self.applied_project_revision = None;
        self.observed_rejected_revision = 0;
        self.audio_admission.reset_audio();
        // A new output prepares fresh endpoints; it reports its own set.
        self.degraded_plugins = Default::default();
        if self
            .pending_publication_observation
            .as_ref()
            .is_some_and(|pending| Some(pending.output_identity) != output_identity)
        {
            self.pending_publication_observation = None;
        }
    }

    fn apply_publication_observation(
        &mut self,
        context: hex_engine::AudioSourceContext,
        state: crate::EditorPublicationState,
    ) {
        let Some(authoring_revision) = self
            .state
            .as_ref()
            .and_then(|state| state.audio_authoring_revision(context))
        else {
            return;
        };
        match state {
            crate::EditorPublicationState::VerifiedPreviewSubmitted {
                audio_revision,
                identity,
            } => {
                if let Some(state) = self.device_publication_view(context, identity)
                    && !self.playback_candidates.iter().any(|candidate| {
                        candidate.audio_revision == audio_revision
                            && candidate.project_generation == context.project_generation
                    })
                    && self.playback_candidates.len() < super::PLAYBACK_CANDIDATE_CAPACITY
                {
                    self.playback_candidates.push_back(PlaybackCandidate {
                        worker_requires_preroll: None,
                        observation: None,
                        audio_revision,
                        project_generation: context.project_generation,
                        state,
                        prepared_revision: None,
                        seek_request_id: None,
                        device_preview_identity: Some(identity),
                        tuning_preview_identity: None,
                    });
                }
                self.status = "publishing gain preview".into();
            }
            crate::EditorPublicationState::Preparing => self.status = "preparing audio".into(),
            crate::EditorPublicationState::Busy => {
                self.status = "audio busy · waiting to publish".into()
            }
            crate::EditorPublicationState::Submitted { audio_revision }
            | crate::EditorPublicationState::ResidentSubmitted { audio_revision, .. } => {
                let metadata = match state {
                    crate::EditorPublicationState::ResidentSubmitted { metadata, .. } => {
                        Some(metadata)
                    }
                    _ => None,
                };
                let observation = match &state {
                    crate::EditorPublicationState::ResidentSubmitted { observation, .. } => {
                        Some(Arc::clone(observation))
                    }
                    _ => None,
                };
                if metadata.is_some_and(|metadata| metadata.context != context) {
                    self.error =
                        Some("audio publication metadata does not match its context".into());
                    return;
                }
                if metadata.is_some_and(|metadata| {
                    metadata.device_preview.is_some() && metadata.tuning_preview.is_some()
                }) {
                    self.error =
                        Some("audio publication has conflicting preview identities".into());
                    return;
                }
                let source_state = match metadata {
                    Some(metadata) => self.resident_publication_view(metadata),
                    None => self.state.as_ref().cloned(),
                };
                if let Some(state) = source_state
                    && !self.playback_candidates.iter().any(|candidate| {
                        candidate.audio_revision == audio_revision
                            && candidate.project_generation == context.project_generation
                    })
                    && self.playback_candidates.len() < super::PLAYBACK_CANDIDATE_CAPACITY
                {
                    self.playback_candidates.push_back(PlaybackCandidate {
                        worker_requires_preroll: metadata.map(|metadata| metadata.requires_preroll),
                        observation,
                        audio_revision,
                        project_generation: context.project_generation,
                        state,
                        prepared_revision: None,
                        seek_request_id: metadata.and_then(|metadata| metadata.seek_request),
                        device_preview_identity: metadata
                            .and_then(|metadata| metadata.device_preview),
                        tuning_preview_identity: metadata
                            .and_then(|metadata| metadata.tuning_preview),
                    });
                }
                if metadata.is_none_or(|metadata| {
                    metadata.device_preview.is_none() && metadata.tuning_preview.is_none()
                }) {
                    self.audio_admission.published(
                        super::audio_admission::ProjectRevision {
                            generation: context.project_generation,
                            revision: authoring_revision,
                        },
                        audio_revision,
                    );
                    if let Some(metadata) = metadata {
                        self.midi_recording_published(
                            context,
                            metadata.computer_keyboard_request,
                            audio_revision,
                        );
                        self.gain_recorder
                            .published(metadata.gain_recording_track, audio_revision);
                    }
                }
                if metadata.is_some_and(|metadata| metadata.stopped_restore)
                    && self
                        .stopped_playback_restore
                        .is_some_and(|restore| match restore {
                            super::audio_admission::StoppedPlaybackRestore::Preparing
                            | super::audio_admission::StoppedPlaybackRestore::Ready => true,
                            super::audio_admission::StoppedPlaybackRestore::Published(previous) => {
                                audio_revision >= previous
                            }
                        })
                {
                    self.stopped_playback_restore = Some(
                        super::audio_admission::StoppedPlaybackRestore::Published(audio_revision),
                    );
                }
                if let Some(request_id) = metadata.and_then(|metadata| metadata.seek_request) {
                    self.recovery.prepared_published(request_id, audio_revision);
                }
                if let Some(request_id) = metadata.and_then(|metadata| metadata.seek_request)
                    && let Some(pending) = self.prepared_seek.as_mut().filter(|pending| {
                        pending.prepared_revision.is_none()
                            && pending.request_id == request_id
                            && pending.project_generation == context.project_generation
                            && pending.project_revision == authoring_revision
                    })
                {
                    pending.stage =
                        super::arrangement_seek::SeekStage::Published { audio_revision };
                    pending.cancellation_retry |= pending.cancelled;
                }
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
                        revision: authoring_revision,
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

    pub(in crate::editor) fn poll_editor_audio_events(&mut self, cx: &mut App) -> bool {
        let mut changed = false;
        for _ in 0..UI_EVENT_BUDGET {
            let event = match self.audio_events.try_recv() {
                Ok(event) => event,
                Err(std::sync::mpsc::TryRecvError::Empty)
                | Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            };
            crate::diagnostics::record_audio_backend_event(cx, &event);
            match event {
                crate::EditorAudioEvent::RelinquishClient {
                    old_output_identity,
                    target,
                } => {
                    if target.project_generation > self.project_generation
                        && self
                            .audio
                            .as_ref()
                            .and_then(|a| a.control.output_identity())
                            == Some(old_output_identity)
                    {
                        // A replacement not yet installed here retires this
                        // output; close it now so the fresh open can proceed.
                        self.retire_audio_preserving_gain_take();
                        changed = true;
                        continue;
                    }
                    if !relinquish_client_matches(
                        self.project_generation,
                        self.state.as_ref().map(|s| s.revision),
                        self.audio
                            .as_ref()
                            .and_then(|a| a.control.output_identity()),
                        old_output_identity,
                        target,
                    ) {
                        continue;
                    }
                    if !self.retire_audio_preserving_gain_take() {
                        changed = true;
                        continue;
                    }
                    self.status = "preparing sampler audio".into();
                    changed = true;
                }
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
                        && let Some(previous) = self.pending_audio_result.take()
                    {
                        previous.close();
                    }
                    let new_identity = audio.control.output_identity();
                    if self
                        .audio
                        .as_ref()
                        .is_none_or(|current| current.control.output_identity() != new_identity)
                    {
                        // Audio revision numbers restart with the output. Preserve only an
                        // observation already correlated to this newly opened endpoint.
                        self.reset_output_publication_tracking(new_identity);
                    }
                    let preserve_outcome_error = self.retired_action_outcomes.is_some()
                        || self
                            .audio
                            .as_ref()
                            .is_some_and(|current| current.control.has_action_outcomes());
                    if !self.retire_audio_preserving_gain_take() {
                        audio.control.close();
                        changed = true;
                        continue;
                    }
                    self.document_audio_mode = audio.mode;
                    self.audio = Some(audio);
                    let mut recovery_status = None;
                    if restart_request.is_some()
                        && let Some(generation) = self.pending_recovery_generation
                    {
                        self.recovery.opened(generation);
                        match self.document_audio_mode {
                            hex_session::DocumentAudioMode::Resident => {
                                if self.enqueue(Request::RestorePlayback { generation }) {
                                    recovery_status = Some("restoring audio graph");
                                } else {
                                    self.fail_audio_recovery(
                                        "audio opened but graph restore was not admitted",
                                    );
                                }
                            }
                            hex_session::DocumentAudioMode::VerifiedStreaming => {
                                recovery_status = Some("preparing streamed audio");
                            }
                        }
                    }
                    self.pending_restart_request = None;
                    self.status = recovery_status.unwrap_or("audio ready").into();
                    if !preserve_outcome_error {
                        self.error = None;
                    }
                    self.reconcile_publication_observation();
                    changed = true;
                }
                crate::EditorAudioEvent::OutputRebound {
                    project_generation,
                    output_identity,
                } => {
                    match audio_event_disposition(
                        self.project_generation,
                        self.pending_restart_request,
                        project_generation,
                        None,
                    ) {
                        AudioEventDisposition::Ignore => continue,
                        AudioEventDisposition::Future => {
                            // Keep the held endpoint; apply with the replacement.
                            self.retain_future_audio_result(PendingEditorAudioResult::Rebound {
                                project_generation,
                                output_identity,
                            });
                            continue;
                        }
                        AudioEventDisposition::Current => {}
                    }
                    self.apply_output_rebound(output_identity);
                    changed = true;
                }
                crate::EditorAudioEvent::OutputReleased {
                    project_generation: _,
                    old_output_identity,
                } => {
                    if self
                        .audio
                        .as_ref()
                        .and_then(|a| a.control.output_identity())
                        == Some(old_output_identity)
                    {
                        self.retire_audio_preserving_gain_take();
                        self.status = "preparing audio output".into();
                        changed = true;
                    }
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
                        && let Some(previous) = self.pending_audio_result.take()
                    {
                        previous.close();
                    }
                    self.retire_audio_preserving_gain_take();
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
                    let message = super::audio_unavailable::unavailable_message(&error);
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
                crate::EditorAudioEvent::OutputRouteChanged { output_identity } => {
                    if self
                        .audio
                        .as_ref()
                        .and_then(|audio| audio.control.output_identity())
                        == Some(output_identity)
                    {
                        self.route_follow = Some(output_identity);
                        changed = true;
                    }
                }
                crate::EditorAudioEvent::NativeLatencyRestart { source } => {
                    if self.latency_rebuild.is_none()
                        && self.stopped_playback_restore.is_none()
                        && self.project_generation == source.context.project_generation
                        && self
                            .audio
                            .as_ref()
                            .and_then(|audio| audio.control.status())
                            .and_then(|status| status.native_effect_source())
                            == Some(source)
                        && let Some(state) = self.state.as_ref()
                        && state.audio_authoring_revision(source.context) == Some(state.revision)
                    {
                        self.latency_rebuild =
                            Some(super::audio_admission::LatencyRebuild::Waiting {
                                project: super::audio_admission::ProjectRevision {
                                    generation: self.project_generation,
                                    revision: state.revision,
                                },
                                output: source.output_identity,
                                runtime_audio: Some(source.audio_revision),
                            });
                        changed = true;
                    }
                }
                crate::EditorAudioEvent::DegradedPlugins {
                    project_generation,
                    output_identity,
                    devices,
                } => {
                    if project_generation == self.project_generation
                        && self
                            .audio
                            .as_ref()
                            .and_then(|audio| audio.control.output_identity())
                            == Some(output_identity)
                    {
                        self.degraded_plugins =
                            crate::editor_backend::DegradedDevices::new(devices);
                        changed = true;
                    }
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
                    worker_requires_preroll: None,
                    observation: None,
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
                if self
                    .prepared_seek
                    .as_ref()
                    .is_some_and(|seek| seek.cancelled)
                {
                    // The seek was invalidated while this worker owned its
                    // runtime. Completing cancellation must not erase a newer
                    // intent already retained for the same held gesture.
                    let latest = self.latched_seek_intent.take();
                    self.invalidate_prepared_seek();
                    self.latched_seek_intent = latest;
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
                if self
                    .prepared_seek
                    .as_ref()
                    .is_some_and(|seek| seek.cancelled)
                {
                    // The seek was invalidated while this worker owned its
                    // runtime. Completing cancellation must not erase a newer
                    // intent already retained for the same held gesture.
                    let latest = self.latched_seek_intent.take();
                    self.invalidate_prepared_seek();
                    self.latched_seek_intent = latest;
                }
            }
        }
        true
    }

    pub(super) fn clear_route_telemetry(&mut self) {
        self.session_routes.clear();
    }

    /// Retire editor export custody for an `Export` or `ExportRange` the
    /// service refused before admission; no export event will ever arrive.
    fn retire_unsubmitted_export(&mut self, request: &Request) {
        if matches!(request, Request::Export(_) | Request::ExportRange { .. }) {
            self.export_pending = false;
            if let Some(control) = self.export_control.take() {
                control.cancel();
            }
            self.export_window = None;
        }
    }

    pub(super) fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        if crate::diagnostics::actions_enabled(cx) {
            let query_bytes = self.browser.search.read(cx).text().len();
            crate::diagnostics::record_view(cx, |snapshot| {
                use std::fmt::Write as _;
                let _ = write!(
                    snapshot,
                    " generation={} revision={:?} track={:?} clip={:?} scene={:?} mode={:?} viewport={:?} browser_open={} browser_query_bytes={} forced_time_selection={}",
                    self.project_generation,
                    self.state.as_ref().map(|s| s.revision),
                    self.selected_track.map(|id| id.value()),
                    self.selected_clip.map(|id| id.value()),
                    self.selected_scene.map(|id| id.value()),
                    self.mode,
                    self.viewport,
                    self.browser.open,
                    query_bytes,
                    self.time_selection_key_held
                );
            });
        }
        crate::diagnostics::record_audio(cx, self.audio.as_mut());
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
        if let Ok(view) = self.service.try_solo_view() {
            visible_change |= self.observe_solo_view(view);
        }
        visible_change |= self.poll_retired_action_outcomes();
        if self.retired_action_outcomes.is_none()
            && self.audio.as_ref().is_some_and(|audio| {
                audio.control.has_action_outcomes() && audio.control.status().is_none()
            })
        {
            visible_change |= self.retire_audio_preserving_gain_take();
        }
        visible_change |= self.poll_update(cx);
        visible_change |= self.poll_browser_sample_playback();
        visible_change |= self.tick_gain_recording();
        visible_change |= self.tick_midi_recording();
        visible_change |= self.tick_sampler_parameter();
        visible_change |= self.tick_computer_keyboard();
        visible_change |= self.tick_tuning_audition();
        visible_change |= self.tuning.broadcast.tick(cx);
        visible_change |= self.tick_tuning_midi_activity();
        visible_change |= self.tick_controller_input(cx);
        visible_change |= self.tick_midi_pitch_feedback();
        visible_change |= self.poll_audio_batch();
        for _ in 0..UI_EVENT_BUDGET {
            match self.service.try_event() {
                Ok(event) => {
                    let update_save = event.save_complete.then_some((
                        event.save_correlation,
                        event.error.is_none()
                            && !event.save_confirmation_required
                            && !event.state.unsaved,
                        event.save_confirmation_required,
                    ));
                    let open_save = (event.save_complete && !event.save_confirmation_required)
                        .then_some((
                            event.error.is_none() && !event.state.unsaved,
                            event.state.revision,
                        ));
                    let saved_project_path =
                        (event.save_complete && event.error.is_none() && !event.state.unsaved)
                            .then(|| event.state.document_path.clone())
                            .flatten();
                    let install = crate::diagnostics::actions_enabled(cx)
                        .then(|| (event.state.revision, std::time::Instant::now()));
                    let opened = self.receive(event);
                    if let Some((revision, started)) = install {
                        crate::diagnostics::record_ui_install(cx, revision, started.elapsed());
                    }
                    if let Some(path) = opened {
                        self.record_opened_project(path.clone(), cx);
                        self.record_project_directory(path, cx);
                    } else if let Some(path) = saved_project_path {
                        self.record_opened_project(path.clone(), cx);
                        self.record_project_directory(path, cx);
                    }
                    if let Some((success, revision)) = open_save {
                        self.complete_open_save(success, revision, cx);
                    }
                    if let Some((correlation, success, confirmation_required)) = update_save {
                        self.update_save_terminal(correlation, success, confirmation_required, cx);
                    }
                    visible_change = true;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    if self.state.is_none() {
                        let reason = self
                            .service
                            .startup_failure()
                            .unwrap_or("project worker stopped before loading");
                        visible_change |= replace_error(
                            &mut self.error,
                            &format!("{reason} · restart hex to try again"),
                        );
                    }
                    self.midi_service_disconnected();
                    self.gain_take_service_disconnected();
                    if self.cancel_new_preparation() {
                        visible_change |= replace_error(
                            &mut self.error,
                            "project worker stopped · new project cancelled",
                        );
                    }
                    self.pending_created_note_selection = None;
                    self.midi_step.cancel_pending();
                    visible_change |= self.retire_pending_trim();
                    visible_change |= self.duplicate_intent.reject();
                    visible_change |= self.pending_track_clipboard.take().is_some();
                    visible_change |= self.pending_group_copy.take().is_some();
                    visible_change |= self.retire_clip_detail();
                    visible_change |= self.retire_pending_audio_events();
                    visible_change |= self.retire_pending_clip_loop();
                    visible_change |= self.clear_pending_bypass();
                    visible_change |= self.cancel_audio_loop_gesture();
                    visible_change |= self.retire_pending_clip_move();
                    visible_change |= self.retire_pending_section();
                    visible_change |= self.retire_pending_track_reorder();
                    visible_change |= self.retire_pending_midi_drag();
                    visible_change |= self.retire_pending_fade();
                    visible_change |= self.retire_clip_gain();
                    visible_change |= self.retire_pending_gain();
                    visible_change |= self.retire_pending_device_automation();
                    visible_change |= self.retire_all_eq_creations();
                    visible_change |= self.pending_loop.take().is_some();
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
            }
        }
        if !self.edit_pending() {
            self.midi_step.cancel_pending();
        }
        visible_change |= self.submit_queued_section();
        visible_change |= self.poll_editor_audio_events(cx);
        visible_change |= self.poll_export(cx);
        visible_change |= self.pump_tuning_gesture();
        if !self.tuning.client.has_pending_requests() && !self.tuning.client.is_recovery_pending() {
            while let Some(request) = self.pending_requests.pop_front() {
                let Err(refused) = self.submit_to_service(request) else {
                    continue;
                };
                match refused.error {
                    SubmitError::Full(request) => {
                        self.pending_requests.push_front(request);
                        break;
                    }
                    SubmitError::Busy(request, reason) => {
                        visible_change |= self.on_request_refused(
                            refused.kinds,
                            &request,
                            submission::Refusal::Busy(reason),
                        );
                    }
                    SubmitError::Closed(request) => {
                        visible_change |= self.on_request_refused(
                            refused.kinds,
                            &request,
                            submission::Refusal::Closed,
                        );
                        break;
                    }
                }
            }
        }
        visible_change |= self.pump_device_gesture();
        visible_change |= self.retire_abandoned_bypass();
        visible_change |= self.retire_abandoned_duplicate();
        visible_change |= self.retire_abandoned_track_clipboard();
        visible_change |= self.retire_abandoned_group_copy();
        visible_change |= self.retire_abandoned_clip_detail();
        visible_change |= self.retire_abandoned_clip_move();
        visible_change |= self.retire_abandoned_midi_drag();
        visible_change |= self.retire_abandoned_fade();
        visible_change |= self.retire_abandoned_clip_gain();
        let now = Instant::now();
        visible_change |= self.tick_time_gesture_autoscroll(now);
        visible_change |= self.tick_clip_gesture_autoscroll(now, cx);
        visible_change |= self.tick_track_reorder(now);
        visible_change |= self.tick_edit_cursor(now);
        let project_edit_pending = self.edit_pending();
        visible_change |= self.tick_project_spectrum();
        visible_change |= self.tick_sample_recorder();
        visible_change |= self.retire_stale_spectrum_target();
        visible_change |= self.retire_stale_processor_audition_target();
        if let Some(audio) = self.audio.as_mut() {
            let _ = audio.control.spectrum_retry_pending();
            let _ = audio.control.audition_retry_pending();
            // Read into a retained buffer: a steady-state tick allocates nothing.
            let spectra = &mut self.device_panel.spectrum_scratch;
            audio.control.spectrum_latest_pairs_into(spectra);
            let results = &mut self.device_panel.spectrum_results;
            let changed = results.len() != spectra.len()
                || spectra
                    .iter()
                    .any(|result| results.get(&result.output.stamp.target) != Some(result));
            if changed {
                results.clear();
                results.extend(
                    spectra
                        .drain(..)
                        .map(|result| (result.output.stamp.target, result)),
                );
                visible_change = true;
            }
            audio.control.reclaim_preview();
        } else if !self.device_panel.spectrum_results.is_empty() {
            self.device_panel.spectrum_results.clear();
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
        // Snapshot private ACK-source eligibility before borrowing the audio
        // client. This does not admit publication: promotion below still needs
        // actual applied/rejected telemetry, including cancellation losing CAS.
        // No UI event can change this owner during this synchronous tick.
        let applied_seek_source = self
            .prepared_seek
            .as_ref()
            .and_then(|pending| self.native_seek_applied_source(pending));
        let current_seek_source = self
            .prepared_seek
            .as_ref()
            .and_then(|seek| {
                self.seek_publication_is_current(seek.request_id, seek.device_preview_identity)
                    .then_some((seek.request_id, seek.device_preview_identity))
            })
            .or_else(|| {
                self.recovery
                    .prepared_request()
                    .filter(|request| self.seek_publication_is_current(*request, None))
                    .map(|request| (request, None))
            });
        let retired_action_message = self.retired_action_message.take();
        let Some(audio) = self.audio.as_mut() else {
            let pending_after = self
                .worker_requests_pending
                .saturating_add(self.pending_requests.len());
            visible_change |= pending_before != pending_after;
            visible_change |= self.tick_track_meters(Instant::now());
            visible_change |= self.tick_track_performance(Instant::now());
            return self.observe_audio_frame(None, visible_change, Instant::now());
        };
        let reliable_outcomes = audio.control.has_action_outcomes();
        let mut outcome_batch = [None; hex_audio::prepared::ACTION_OUTCOME_CAPACITY];
        let mut outcome_failure = None;
        if let Some(reader) = audio.control.action_outcomes_mut() {
            for slot in &mut outcome_batch {
                let Some(receipt) = reader.try_take() else {
                    break;
                };
                *slot = Some(receipt);
            }
            outcome_failure = reader.failure();
        }
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
                self.prepared_seek
                    .as_ref()
                    .filter(|seek| Some(seek.request_id) == pending.seek_request_id)
                    .and_then(|seek| seek.section)
                    .map_or(
                        PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
                        |section| PublicationPolicy::InstallSectionWhenReady {
                            fence,
                            quantization: section.quantization,
                        },
                    )
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
        if !reliable_outcomes
            && let Some((ticket, reason)) = audio
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
            if reason == hex_engine::PlaybackHandoffRejection::LatencyLayoutChanged
                && let Some(output) = audio.control.output_identity()
            {
                self.latency_rebuild = Some(super::audio_admission::LatencyRebuild::Unresolved {
                    generation: self.project_generation,
                    output,
                    audio_revision,
                });
                visible_change |= replace_error(&mut self.error, reason.message());
            }
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
                if reason == hex_engine::PlaybackHandoffRejection::LatencyLayoutChanged
                    && project.generation == self.project_generation
                    && self
                        .state
                        .as_ref()
                        .is_some_and(|state| state.revision == project.revision)
                    && audio
                        .control
                        .status()
                        .is_some_and(|status| status.playing.load(Acquire))
                    && let Some(output) = audio.control.output_identity()
                {
                    self.latency_rebuild = Some(super::audio_admission::LatencyRebuild::Waiting {
                        project,
                        output,
                        runtime_audio: None,
                    });
                }
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
                if reason == hex_engine::PlaybackHandoffRejection::LatencyLayoutChanged {
                    self.latency_rebuild = None;
                }
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
        if let Some(super::audio_admission::LatencyRebuild::Restoring {
            project,
            output,
            runtime_audio: Some(previous),
            installed,
            ..
        }) = self.latency_rebuild.as_mut()
            && let Some(super::audio_admission::StoppedPlaybackRestore::Published(required)) =
                self.stopped_playback_restore
            && required > *previous
            && let Some(source) = audio
                .control
                .status()
                .and_then(|status| status.recording_source())
            && source.audio_revision == required
            && source.output_identity == *output
            && source.context.project_generation == project.generation
            && self.state.as_ref().is_some_and(|state| {
                state.revision == project.revision
                    && state.audio_authoring_revision(source.context) == Some(project.revision)
            })
        {
            *installed = true;
            self.applied_project_revision = Some(project.revision);
            self.stopped_playback_restore = None;
        }
        let awaiting_native_restore = matches!(
            self.latency_rebuild,
            Some(super::audio_admission::LatencyRebuild::Restoring {
                runtime_audio: Some(_),
                installed: false,
                ..
            })
        );
        let (reconciled, current_applied) = reconcile_verified_source_binding(
            &mut self.audio_admission,
            self.document_audio_mode,
            audio.control.output_identity(),
            self.project_generation,
            self.state.as_ref().map(AsRef::as_ref),
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
        let seek_metadata_consumed = audible.as_ref().is_some_and(|candidate| {
            applied_seek_source
                == candidate
                    .seek_request_id
                    .map(|request| (request, candidate.audio_revision))
        });
        if let Some(candidate) = audible {
            let source = if candidate.seek_request_id.is_some_and(|request_id| {
                current_seek_source != Some((request_id, candidate.device_preview_identity))
            }) {
                AppliedCandidateSource::SupersededPreview
            } else {
                applied_tuning_candidate_source(
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
                })
            };
            if source == AppliedCandidateSource::Current {
                if candidate.tuning_preview_identity.is_some() {
                    self.tuning.broadcast.observe(
                        candidate.project_generation,
                        candidate.state.revision,
                        candidate.state.project.tuning(),
                    );
                }
                if candidate.project_generation == self.project_generation
                    && candidate.seek_request_id.is_none()
                {
                    self.applied_project_revision = Some(candidate.state.revision);
                    if matches!(
                        self.stopped_playback_restore,
                        Some(super::audio_admission::StoppedPlaybackRestore::Published(required))
                            if candidate.audio_revision > required
                                || (candidate.audio_revision == required && !awaiting_native_restore)
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
            if let Some(state) = self.state.as_ref().filter(|state| {
                state.audio_authoring_revision(source.context) == Some(state.revision)
            }) {
                self.applied_project_revision = Some(state.revision);
                if self.audible_playback.as_ref().is_none_or(|candidate| {
                    candidate.audio_revision != source.audio_revision
                        || candidate.project_generation != source.context.project_generation
                }) {
                    self.audible_playback = Some(PlaybackCandidate {
                        worker_requires_preroll: None,
                        observation: None,
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
        let awaiting_seek_metadata = applied_seek_source.is_some_and(|(request, revision)| {
            applied == revision
                && !seek_metadata_consumed
                && rejected_revision.is_none_or(|(rejected, _)| rejected != revision)
                && self.audible_playback.as_ref().is_none_or(|candidate| {
                    candidate.seek_request_id != Some(request)
                        || candidate.audio_revision != revision
                })
        });
        if !awaiting_seek_metadata {
            visible_change |= super::arrangement_seek::resolve_prepared_seek_terminal(
                &mut self.prepared_seek,
                &mut self.status,
                &mut self.error,
                applied,
                rejected_revision,
            );
        }
        if outcome_failure.is_some() {
            audio.control.stop();
            self.pending_audio_commands.clear();
            self.pending_seek_ticket = None;
            self.pending_metronome_ticket = None;
            self.pending_session_ticket = None;
            self.play_intent.clear();
            self.recovery.fail();
            self.pending_recovery_generation = None;
            visible_change |= replace_error(
                &mut self.error,
                "audio command outcomes unavailable · restart audio",
            );
        }
        while outcome_failure.is_none() {
            let Some(pending) = self.pending_audio_commands.pop_front() else {
                break;
            };
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
            match if let Some(trace) = &self.transport_trace {
                trace.attempt(audio, pending.action)
            } else {
                audio.control.action_with_ticket(pending.action)
            } {
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
        let mut action_errors = retired_action_message.unwrap_or_default();
        if reliable_outcomes {
            self.play_intent.reconcile_actual(actual_playing);
            for receipt in outcome_batch.into_iter().flatten() {
                self.play_intent
                    .reconcile_receipt(receipt.ticket, actual_playing);
                self.recovery.observe_receipt(receipt);
                if matches!(
                    self.latency_rebuild,
                    Some(super::audio_admission::LatencyRebuild::Fading { ticket, .. })
                        if ticket == receipt.ticket
                ) && !matches!(receipt.outcome, hex_audio::prepared::ActionOutcome::Applied)
                {
                    self.latency_rebuild = None;
                    visible_change |=
                        replace_status(&mut self.status, "latency rebuild stop was interrupted");
                }
                if self.pending_metronome_ticket == Some(receipt.ticket) {
                    self.pending_metronome_ticket = None;
                    visible_change = true;
                }
                if self.pending_seek_ticket == Some(receipt.ticket) {
                    self.pending_seek_ticket = None;
                    visible_change = true;
                    match receipt.outcome {
                        hex_audio::prepared::ActionOutcome::Applied => {
                            replace_status(&mut self.status, "cursor ready");
                        }
                        _ => {
                            replace_status(&mut self.status, "seek was not applied");
                        }
                    }
                }
                let (pending, result) = reconcile_session_receipt(
                    self.pending_session_ticket,
                    self.project_generation,
                    receipt,
                );
                self.pending_session_ticket = pending;
                match result {
                    Some(SessionTicketOutcome::Consumed) => {
                        visible_change |=
                            replace_status(&mut self.status, "session request consumed");
                    }
                    Some(SessionTicketOutcome::Rejected) => {
                        visible_change |=
                            replace_status(&mut self.status, "session request rejected");
                    }
                    Some(SessionTicketOutcome::Superseded) => {
                        visible_change |=
                            replace_status(&mut self.status, "session request superseded");
                    }
                    Some(SessionTicketOutcome::Interrupted) => {
                        visible_change |=
                            replace_status(&mut self.status, "session request interrupted");
                    }
                    None => {}
                }
                match receipt.outcome {
                    hex_audio::prepared::ActionOutcome::Rejected(reason) => {
                        if !action_errors.is_empty() {
                            action_errors.push_str(" · ");
                        }
                        action_errors.push_str(reason.message());
                    }
                    hex_audio::prepared::ActionOutcome::Interrupted => {
                        if !action_errors.is_empty() {
                            action_errors.push_str(" · ");
                        }
                        action_errors.push_str("audio action interrupted");
                    }
                    _ => {}
                }
            }
            if outcome_failure.is_some() {
                if matches!(
                    self.latency_rebuild,
                    Some(super::audio_admission::LatencyRebuild::Fading { .. })
                ) {
                    self.latency_rebuild = None;
                }
                if !action_errors.is_empty() {
                    action_errors.push_str(" · ");
                }
                action_errors.push_str("audio command outcomes unavailable · restart audio");
            }
            if !action_errors.is_empty() {
                visible_change |= replace_error(&mut self.error, &action_errors);
            }
        } else {
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
                Some(SessionTicketOutcome::Superseded) => {
                    visible_change |=
                        replace_status(&mut self.status, "session request superseded");
                }
                Some(SessionTicketOutcome::Interrupted) => {
                    visible_change |=
                        replace_status(&mut self.status, "session request interrupted");
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
            let diagnostic = "audio device failed · editing and saving remain available";
            let message = fault_with_action_errors(&self.error, diagnostic, &action_errors);
            visible_change |= replace_error(&mut self.error, &message);
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
            let message = fault_with_action_errors(&self.error, &diagnostic, &action_errors);
            visible_change |= replace_error(&mut self.error, &message);
        } else if audio
            .control
            .status()
            .is_some_and(|status| status.stream_source_changed.load(Acquire))
        {
            self.stream_source_warning = true;
            visible_change |= replace_status(&mut self.status, "streamed media unavailable");
            let diagnostic = "original media changed · reopen or relink it";
            let message = fault_with_action_errors(&self.error, diagnostic, &action_errors);
            visible_change |= replace_error(&mut self.error, &message);
        } else if self.stream_source_warning {
            self.stream_source_warning = false;
            if self.status == "streamed media unavailable" {
                visible_change |= replace_status(&mut self.status, "audio ready");
            }
            if self.error.as_deref() == Some("original media changed · reopen or relink it") {
                self.error = None;
                visible_change = true;
            }
        }
        if audio.control.status().is_some_and(|status| {
            !status.device_fault.load(Relaxed)
                && !status.render_fault.load(Relaxed)
                && !status.shutdown_complete.load(Acquire)
        }) {
            if let Some(identity) = audio.control.output_identity()
                && let Some(status) = audio.control.status()
            {
                visible_change |= self.session_routes.poll(identity, status);
            }
        } else {
            self.session_routes.clear();
        }
        let frame = audio.control.status().map(|status| {
            let mut frame = AudioFrame::capture(status);
            frame.source = audio.control.output_identity().unwrap_or_default();
            frame
        });
        let pending_after = self
            .worker_requests_pending
            .saturating_add(self.pending_requests.len());
        visible_change |= pending_before != pending_after;
        visible_change |= self.follow_output_route();
        visible_change |= self.finish_audio_recovery_tick();
        visible_change |= self.tick_track_meters(Instant::now());
        visible_change |= self.tick_track_performance(Instant::now());
        visible_change |= self.tick_latency_rebuild();
        visible_change |= self.tick_sampler_edit_resume();
        visible_change |= self.tick_deferred_play();
        self.observe_audio_frame(frame, visible_change, Instant::now())
    }

    pub(in crate::editor) fn spectrum_target_is_visible_and_current(
        &self,
        target: hex_audio::spectrum_target::SpectrumTarget,
    ) -> bool {
        self.spectrum_identity_is_visible_and_current(target.identity, target.project_generation)
    }
    pub(in crate::editor) fn spectrum_identity_is_visible_and_current(
        &self,
        identity: hex_engine::PostNodeIdentity,
        generation: u64,
    ) -> bool {
        if !self.clip_view_open
            || !self.device_panel_active
            || self.project_generation != generation
            || !matches!(identity.device_type.as_str(), Ok("hex.eq8" | "hex.eq"))
        {
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let focused_owner = super::device_panel::displayed_owner(self)
            .or_else(|| super::device_panel::selected_owner(self, state));
        match (focused_owner, identity.owner) {
            (
                Some(super::device_panel::DevicePanelOwner::Main),
                hex_engine::ProcessorChainOwner::Main,
            ) => state.project.main_devices().iter().any(|device| {
                device.id().value() == identity.device_id
                    && device.state().type_id() == identity.device_type
            }),
            (
                Some(super::device_panel::DevicePanelOwner::Track(track_id)),
                hex_engine::ProcessorChainOwner::Track(owner_id),
            ) if track_id.value() == owner_id => {
                state.project.track(track_id).is_some_and(|track| {
                    track.devices().iter().any(|device| {
                        device.id().value() == identity.device_id
                            && device.state().type_id() == identity.device_type
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
                        device.id().value() == identity.device_id
                            && device.state().type_id() == identity.device_type
                    })
                }),
            _ => false,
        }
    }

    fn retire_stale_spectrum_target(&mut self) -> bool {
        super::device_panel::reconcile_eq_spectrum(self)
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
        let focused_owner = super::device_panel::selected_owner(self, state);
        match (focused_owner, target.identity.owner) {
            (
                Some(super::device_panel::DevicePanelOwner::Main),
                hex_engine::ProcessorChainOwner::Main,
            ) => state.project.main_devices().iter().any(|device| {
                device.id().value() == target.identity.device_id
                    && device.state().type_id() == target.identity.device_type
            }),
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

    pub(super) fn receive(&mut self, mut event: Event) -> Option<std::path::PathBuf> {
        if !event.project_replaced
            && let Some(previous) = self.state.as_ref()
            && let Some(proof) = event.state.comparison_audio.as_ref()
            && !proof.follows(previous, &event.state, self.project_generation)
            && !(previous.revision == event.state.revision
                && previous.project == event.state.project
                && previous
                    .canonical_audio_source
                    .as_ref()
                    .zip(event.state.canonical_audio_source.as_ref())
                    .is_some_and(|(old, current)| {
                        Arc::ptr_eq(old, current)
                            && current.context().project_generation == self.project_generation
                            && event.state.audio_authoring_revision(current.context())
                                == Some(event.state.revision)
                    }))
        {
            // Worker events are ordered and retained under backpressure. A
            // duplicate old proof or mismatched source cannot advance authoring,
            // admission, an input target, or any pending publication owner.
            return None;
        }
        if self
            .solo_observation
            .as_ref()
            .is_some_and(|view| event.project_replaced || event.state.revision >= view.revision)
        {
            self.solo_observation = None;
        }
        if event.request_complete && event.error.is_some() {
            self.cancel_new_preparation();
        }
        self.document_format_upgrade_required = event.document_format_upgrade_required;
        self.recovery_failure = event.recovery_failure.clone();
        let sampler_edit_complete = event.sampler_edit_complete;
        self.plugin_effect_complete(&event);
        let sampler_load_complete = event.sampler_load_complete;
        let sampler_load_succeeded = event.error.is_none();
        if self
            .audio_clip_controls
            .observe(self.project_generation, &event)
        {
            self.retire_sample_bpm_control();
        }
        let audio_placement_selection = self.audio_events_request_complete(&event);
        self.clip_loop_request_complete(&event);
        self.bypass_request_complete(&event);
        self.clip_move_request_complete(&event);
        self.truncated_import
            .observe(self.project_generation, &event);
        self.section_request_complete(&event);
        self.track_reorder_complete(&event);
        self.midi_drag_request_complete(&event);
        self.advance_midi_origin_view(&event);
        self.fade_request_complete(&event);
        self.clip_gain_complete(&event);
        self.gain_request_complete(&event);
        self.gain_take_request_complete(&event);
        self.device_automation_complete(&event);
        self.eq_creation_request_complete(&event);
        let retired_trim = self.trim_request_complete(&event);
        let loop_complete = self.loop_request_complete(&event);
        let group_copy_intent = self.group_copy_request_complete(&event);
        let midi_step_complete = self.take_completed_midi_step(&event);
        let created_note_selection = self.take_created_note_selection(&event);
        let duplicate_completion = self.take_duplicate_completion(&mut event);
        let track_clipboard_completion = self.take_track_clipboard_completion(&mut event);
        if event.project_replaced {
            self.time_selection_key_held = false;
            self.pending_plugin_effect.take();
            self.retire_sampler_parameter();
            self.retire_first_sampler_load();
            self.cancel_open_after_save();
            self.invalidate_prepared_seek();
            let next_generation = self.project_generation.saturating_add(1);
            // A held endpoint stays open until the worker names the outcome
            // (`OutputRebound`/`OutputReleased`) or a fresh result replaces it.
            if !self.hold_audio_across_replacement(next_generation) {
                self.retire_audio_preserving_gain_take();
            }
            if self
                .pending_audio_result
                .as_ref()
                .is_some_and(|pending| pending.project_generation() < next_generation)
                && let Some(pending) = self.pending_audio_result.take()
            {
                pending.close();
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
        self.reconcile_group_selection(&event);
        if event.bounce_complete.as_ref().is_some_and(|completed| {
            self.bounce_control
                .as_ref()
                .is_some_and(|active| active.same_job(completed))
        }) {
            self.bounce_control = None;
        }
        if event.project_replaced
            && let Some(control) = self.bounce_control.take()
        {
            control.cancel();
        }
        // A bounce drop's ghost retires with its job's terminal, in the same
        // receive that installs the adopted (or unchanged, refused) state.
        if self.bounce_control.is_none() {
            self.pending_bounce_drop = None;
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
                self.controller_expression_prepared();
            } else {
                input.cancel();
                return None;
            }
        }
        if event.open_complete {
            self.opening_project = false;
            self.pending_open_path = None;
            self.pending_open_mode = None;
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
                && self
                    .state
                    .as_ref()
                    .is_some_and(|state| state.revision == event.state.revision);
            if !current {
                input.cancel();
                return None;
            }
            self.tuning.audition_preparing = false;
            if input.is_cancelled() {
                self.cancel_tuning_audition();
                if let Some(error) = event.error {
                    self.tuning.error = Some(error.clone());
                    self.error = Some(error.into());
                }
                return None;
            }
        }
        if let Some(request_id) = event.computer_keyboard_complete {
            let note_audition =
                self.midi_audition_preparation_matches(request_id, event.state.revision);
            let current = note_audition
                || ((self.computer_keyboard.enabled || self.controller_wants_live_notes())
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
                        .is_some_and(|state| state.revision == event.state.revision));
            if current {
                self.computer_keyboard.prepared = event.error.is_none();
                if event.error.is_some() {
                    self.midi_recording_preparation_failed();
                }
                let audition_ok = note_audition && event.error.is_none();
                if let Some(error) = event.error {
                    self.error = Some(error.into());
                    self.cancel_computer_keyboard();
                    self.computer_keyboard.suspended = true;
                }
                if audition_ok && let Err(error) = self.trigger_prepared_midi_audition() {
                    self.error = Some(error.to_string().into());
                    self.cancel_computer_keyboard();
                    return None;
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
        self.observe_effect_bus_barrier(&event);
        let event_state = Arc::clone(&event.state);
        let revision = event_state.revision;
        if !event.project_replaced {
            // A successful comparison commit can also report a later recovery
            // checkpoint error. Reconcile its sealed, ordered proof regardless
            // of that diagnostic; a refusal without valid proof cannot advance
            // audio correlation. The error is still surfaced below.
            self.reconcile_comparison_authoring(&event_state);
        }
        if event.project_replaced
            || self
                .state
                .as_ref()
                .is_some_and(|previous| previous.revision != revision)
        {
            self.resident_device_preview = None;
            self.resident_tuning_preview = None;
            self.invalidate_prepared_seek();
            self.cancel_active_browser_audio_drag();
        }
        // Resize previews retain their own captured viewport through the exact
        // terminal below. Never adopt that private extent as canonical tail.
        let resize_owns_viewport = retired_trim.is_some()
            || self.pending_trim.is_some()
            || self.active_trim_drag.is_some()
            || self.active_midi_trim_drag.is_some();
        self.reconcile_arranger_navigation(
            &event_state,
            event.project_replaced,
            resize_owns_viewport,
        );
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
            self.cancel_stale_midi_creation_gestures(revision);
        }
        self.drag_lanes
            .retain(|track, _| event.state.project.track(*track).is_some());
        self.drag_lane_epochs
            .retain(|track, _| event.state.project.track(*track).is_some());
        self.track_drop_lanes
            .retain(|track, _| event.state.project.track(*track).is_some());
        self.gain_lane.reconcile(
            self.project_generation,
            revision,
            &event.state.project,
            event.project_replaced,
        );
        // Initial population may choose the first clip. Subsequent control
        // acknowledgements must not recreate a deliberately empty selection.
        let initialize_clip_selection = self
            .state
            .as_ref()
            .is_none_or(|state| state.project.tracks().is_empty())
            && !event.state.project.tracks().is_empty();
        let save_confirmation_required =
            event.save_confirmation_required && !event.project_replaced;
        let loop_was_enabled = self
            .state
            .as_ref()
            .and_then(|state| state.project.loop_region())
            .is_some_and(|region| region.enabled());
        self.state = Some(event.state);
        if let Some(selection) = audio_placement_selection {
            self.install_audio_placement_selection(selection);
        }
        if self.selection_loop_start.is_some_and(|(_, region)| {
            self.state
                .as_ref()
                .and_then(|state| state.project.loop_region())
                != Some(region)
        }) {
            self.selection_loop_start = None;
        }
        if !event.project_replaced {
            self.retire_point_cursor_on_loop_enable(loop_was_enabled);
        }
        self.gain_take_state_installed();
        self.device_automation_state_installed(event.project_replaced);
        if save_confirmation_required {
            self.queue_save_confirmation();
        }
        self.observe_midi_take_completion(event.midi_take_complete);
        if loop_complete {
            self.loop_state_installed(event.error.is_none());
        }
        if event.project_replaced {
            self.loop_rail_gesture = None;
            self.cancel_midi_note_gesture();
            self.midi_hit_cycle = None;
            self.midi_scene_cache.clear();
            if let Ok(mut cache) = self.waveform_geometry_cache.try_borrow_mut() {
                cache.clear();
            }
            self.piano_beat_start = 0;
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
                && let Some(pending) = self.pending_audio_result.take()
            {
                match pending {
                    PendingEditorAudioResult::Opened {
                        project_generation,
                        restart_request,
                        mut audio,
                    } if audio_event_disposition(
                        self.project_generation,
                        self.pending_restart_request,
                        project_generation,
                        restart_request,
                    ) == AudioEventDisposition::Current =>
                    {
                        if self.audio.as_ref().is_some_and(|old| {
                            old.control.has_action_outcomes() && old.control.status().is_none()
                        }) {
                            audio.control.close();
                            self.error =
                                Some("waiting for previous audio outcomes · retry audio".into());
                        } else {
                            self.document_audio_mode = audio.mode;
                            self.audio = Some(audio);
                        }
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
                        let message = super::audio_unavailable::unavailable_message(&error);
                        self.status = message.clone();
                        self.error = Some(message.into());
                        self.pending_restart_request = None;
                    }
                    PendingEditorAudioResult::Rebound {
                        project_generation,
                        output_identity,
                    } if project_generation == self.project_generation => {
                        self.apply_output_rebound(output_identity);
                    }
                    result => result.close(),
                }
            }
            self.device_gesture.reset();
            self.device_gesture_barriers.clear();
            self.refuse_effect_bus_barriers("effect insertion canceled · project replaced");
            self.edit_cursor = None;
            self.locked_play_start = None;
            self.loop_start_over_clip = None;
            self.selection_loop_start = None;
            self.clip_paint_gesture = None;
            self.cancel_section_pointer();
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
            self.degraded_plugins = Default::default();
            self.drag_lanes.clear();
            self.track_drop_lanes.clear();
            self.drag_lane_epochs.clear();
            self.arrangement_bounds = None;
            self.arrangement_bounds_epoch = 0;
            self.main_track_lane_bounds = None;
            self.main_track_lane_epoch = 0;
            self.retire_active_clip_drag();
            self.pointer_down = None;
            // The document transaction owns transport and publication during
            // replacement. Reset the UI without revoking a staged effect endpoint.
            self.stop_transport(false);
            self.viewport = None;
            self.duplicate_intent.reject();
            self.pending_scene_duplicate = None;
            self.selected_track = None;
            self.device_panel.main_selected = None;
            self.track_selection = super::selection::TrackSelection::default();
            self.selected_clip = None;
            self.selected_scene = None;
            self.selected_cell = None;
            self.session_copy_source = None;
            self.session_track_start = 0;
            self.midi_selection.clear();
        } else if let Some(state) = self.state.clone() {
            let previous_clip = self.detail_clip_id();
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
            if self.selected_track.is_some() || self.focused_return.is_some() {
                self.device_panel.main_selected = None;
            }
            if self.device_panel.main_selected != Some(self.project_generation) {
                // Worker updates must not replace an active range with a clip.
                let range_owns_selection = self.selected_clip.is_none()
                    && self
                        .edit_cursor
                        .is_some_and(|cursor| cursor.generation == self.project_generation)
                    && (self.time_selection_gesture.is_some()
                        || self.time_range_move_gesture.is_some()
                        || self.normalized_time_selection().is_some());
                (self.selected_track, self.selected_clip) =
                    selection::reconcile(&state.project, self.selected_track, self.selected_clip);
                if initialize_clip_selection
                    && self.selected_clip.is_none()
                    && let Some((_, track, clip)) = selection::neighbor(&state.project, None, true)
                {
                    self.selected_track = Some(track);
                    self.selected_clip = clip;
                }
                if range_owns_selection {
                    self.selected_clip = None;
                }
            }
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
            if self.detail_clip_id() != previous_clip {
                self.cancel_midi_note_gesture();
                self.midi_hit_cycle = None;
                self.midi_selection.clear();
                self.piano_beat_start = 0;
                if let Some(length) = self
                    .detail_clip_id()
                    .and_then(|clip| state.project.midi_source(clip))
                    .map(|source| source.source_length().raw())
                {
                    self.piano_beat_span = hex_project::BeatTime::from_raw(
                        self.piano_beat_span.raw().min(length.max(1)),
                    );
                }
            }
            if let Some(clip) = self
                .detail_clip_id()
                .and_then(|clip| state.project.midi_source(clip))
            {
                // Source markers and Arrangement duration may change independently.
                // Preserve manual navigation across ACKs within the bounded authoring domain.
                let source_length = clip.source_length().raw();
                self.piano_beat_span = hex_project::BeatTime::from_raw(
                    self.piano_beat_span
                        .raw()
                        .min(super::midi_scene::zoom_limit(source_length))
                        .max(1),
                );
                self.piano_beat_start = super::midi_scene::clamp_start(
                    self.piano_beat_start,
                    self.piano_beat_span.raw(),
                );
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
        if let Some(completion) = track_clipboard_completion {
            self.complete_track_clipboard(completion);
        }
        if let Some(completion) = duplicate_completion {
            self.complete_clip_duplicate(completion);
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
        } else if let Some(hex_project::Created::Clips { first, last }) = event.created {
            if let Some(intent) = group_copy_intent
                && intent.is_current(self)
            {
                let count = self.state.as_ref().map_or(0, |state| {
                    intent.expected_count(&state.project, first, last)
                });
                self.select_copied_group(first, last, count);
            }
        } else if let Some(hex_project::Created::Track(track)) = event.created {
            self.focused_return = None;
            self.device_panel.main_selected = None;
            if let Some(state) = &self.state {
                self.track_selection
                    .select(&state.project, track, false, false);
            }
            self.midi_note_gesture = None;
            self.midi_group_gesture = None;
            self.selected_track = Some(track);
            self.selected_clip = None;
            self.midi_selection.clear();
            self.reconcile_arrangement_focus();
            self.arrangement_focus.reveal_created_track(track);
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
                self.clip_selection.only(track, clip);
                self.piano_beat_start = 0;
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
            let matched = self
                .tuning
                .client
                .acknowledge_with_snapshot(ack, event.active_tuning_gesture);
            let retired = !event.tuning_gesture_request
                && event.active_tuning_gesture.is_none()
                && key == Some(ack.key);
            if !ack.accepted
                && (matched || retired)
                && (retired
                    || matches!(
                        ack.phase,
                        hex_session::tuning_gesture::TuningGesturePhase::Begin
                            | hex_session::tuning_gesture::TuningGesturePhase::Commit
                            | hex_session::tuning_gesture::TuningGesturePhase::Cancel
                    ))
            {
                self.refuse_tuning_barriers();
            }
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
        // Canonical publication remains independent of audio availability.
        // Active or rejected transient gestures retain their applied table.
        if event.tuning_preview.is_none()
            && event.active_tuning_gesture.is_none()
            && self.tuning.client.snapshot().is_none()
        {
            self.tuning.broadcast.observe(
                self.project_generation,
                revision,
                event_state.project.tuning(),
            );
        }
        if let Some(result) = event.tuning_library.as_ref() {
            self.tuning_library_complete(result);
        }
        let mut matched_scroll_ack = None;
        if event.gesture_protocol_ack
            && let Some(ack) = event.gesture_ack.as_ref()
        {
            if self.acknowledge_device_gesture_with_snapshot(ack, event.active_device_gesture) {
                matched_scroll_ack = Some(ack);
            }
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
            self.refuse_deferred_tuning_writes();
            if super::device_gesture::retire_failed_tuning_library_barriers(
                &mut self.device_gesture_barriers,
            ) {
                self.tuning.library_pending = None;
            }
            if let Some(snapshot) = self.device_gesture.snapshot() {
                self.bind_tuning_library_device_terminal(snapshot.key, false);
            }
            self.refuse_effect_bus_barriers("effect insertion canceled · device edit was retired");
            self.device_gesture.reset();
            super::device_gesture::retire_deferred(
                &mut self.pending_requests,
                &mut self.device_gesture_barriers,
                event.project_replaced,
            );
        }
        super::device_panel::eq_scroll_after_worker_event(
            self,
            matched_scroll_ack,
            event.project_replaced,
        );
        if let Some(preview) = event.device_preview.as_ref().filter(|preview| {
            preview.identity.project_generation == self.project_generation
                && preview.identity.canonical_revision == revision
        }) {
            self.resident_device_preview = Some(hex_session::service::DevicePreview {
                identity: preview.identity,
                state: Arc::clone(&preview.state),
            });
        }
        if let Some(preview) = event.tuning_preview.as_ref().filter(|preview| {
            preview.identity.project_generation == self.project_generation
                && preview.identity.canonical_revision == revision
        }) {
            self.resident_tuning_preview = Some(hex_session::service::TuningPreview {
                identity: preview.identity,
                state: Arc::clone(&preview.state),
            });
        }
        let playback_capability =
            event
                .playback
                .as_ref()
                .map(|playback| match playback.session_capability() {
                    hex_engine::SessionCapability::Unavailable(error) => Some(
                        hex_session::service::text::lowercase_outside_verbatim(&error.to_string()),
                    ),
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
            self.retire_seek_before_playback_replacement(preview_identity);
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
        if let Some(reason) = self.take_effect_bus_barrier_error() {
            self.error = Some(reason.into());
        }
        self.finish_sampler_parameter(sampler_edit_complete);
        self.first_sampler_load_complete(sampler_load_complete, sampler_load_succeeded);
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
        // Canonical state, generation, gain/loop display extent and selection
        // have now been installed. Normalize only the locally retired owner.
        self.normalize_completed_trim_viewport(retired_trim);
        self.apply_created_note_selection(created_note_selection);
        self.apply_completed_midi_step(midi_step_complete);
        if let Some(path) = &successfully_opened_path {
            self.check_leftover_recovery(Some(path.clone()));
        }
        successfully_opened_path
    }

    pub(super) fn enqueue(&mut self, request: Request) -> bool {
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

    /// Source indices/coordinates deliberately belong to the displayed source.
    /// Whole-clip property controls must use ordinary enqueue instead.
    pub(super) fn enqueue_focused_source_edit(&mut self, command: hex_project::Command) -> bool {
        self.enqueue_with_clip_scope(Request::Edit(command), true)
    }

    fn enqueue_with_clip_scope(&mut self, request: Request, focused_source: bool) -> bool {
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

    pub(super) fn enqueue_duplicate_continuation(&mut self, request: Request) -> bool {
        self.enqueue_request(request, true)
    }

    fn enqueue_request(&mut self, request: Request, duplicate_continuation: bool) -> bool {
        self.wake_poll();
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
        if self.gain_recorder.blocks_edits() && !self.pending_gain_take_matches(&request) {
            self.error = Some(if self.gain_recorder.has_retained_take() {
                "retry or explicitly discard the retained gain take before another project action"
                    .into()
            } else {
                "finish or cancel gain recording before another project action".into()
            });
            return false;
        }
        if self.gain_lane.draft.is_some() && matches!(&request, Request::Edit(_)) {
            self.error = Some("finish or cancel the gain point gesture before editing".into());
            return false;
        }
        if self.automation_view.devices.point_draft_active() && matches!(&request, Request::Edit(_))
        {
            self.error =
                Some("finish or cancel the automation point gesture before editing".into());
            return false;
        }
        if matches!(&request, Request::Edit(_))
            && (self.gain_lane.numeric_draft_active()
                || self.sampler_parameter.numeric_draft_active())
        {
            self.error = Some("finish or cancel the numeric draft before editing".into());
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
        if matches!(
            &request,
            Request::DeviceGesture(_)
                | Request::DropNativeDevice { .. }
                | Request::DropBrowserDeviceOnArrangement { .. }
                | Request::InsertBrowserInstrumentTrack { .. }
                | Request::InsertEffectBusDevice { .. }
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
                | Request::DeviceClipboard {
                    action: hex_session::service::DeviceClipboardAction::Cut { .. }
                        | hex_session::service::DeviceClipboardAction::Paste { .. }
                        | hex_session::service::DeviceClipboardAction::Duplicate { .. },
                    ..
                }
                | Request::TrackClipboard {
                    action: hex_session::service::TrackClipboardAction::Cut(_)
                        | hex_session::service::TrackClipboardAction::Duplicate { .. }
                        | hex_session::service::TrackClipboardAction::Paste { .. },
                    ..
                }
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
                | Request::BounceArrangement { .. }
                | Request::ConvertClip { .. }
                | Request::CutTimeRange(_)
                | Request::PasteTimeRange { .. }
                | Request::InsertTimeRange { .. }
                | Request::Undo
                | Request::SamplerEdit(hex_session::service::SamplerEdit::History(_))
                | Request::SamplerEdit(hex_session::service::SamplerEdit::Midi(_))
                | Request::Redo
                | Request::RestorePlayback { .. }
                | Request::RestoreStoppedPlayback
        ) {
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
        let Err(refused) = self.submit_to_service(request) else {
            return true;
        };
        let (request, refusal) = match refused.error {
            SubmitError::Full(request)
                if self.pending_requests.len() + self.device_gesture_barriers.len()
                    < UI_REQUEST_CAPACITY =>
            {
                self.pending_requests.push_back(request);
                return true;
            }
            SubmitError::Full(request) => (request, submission::Refusal::QueueFull),
            SubmitError::Busy(request, reason) => (request, submission::Refusal::Busy(reason)),
            SubmitError::Closed(request) => (request, submission::Refusal::Closed),
        };
        self.on_request_refused(refused.kinds, &request, refusal);
        false
    }

    pub(super) fn request_seek(&mut self, frame: u64) {
        self.wake_poll();
        self.latency_rebuild = None;
        if self.section_install_pending() {
            self.error = Some("applying edit · wait for audio".into());
            return;
        }
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
            match if let Some(trace) = &self.transport_trace {
                trace.attempt(audio, command)
            } else {
                audio.control.action_with_ticket(command)
            } {
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

    pub(super) fn cancel_duplicate_chain(&mut self) {
        self.duplicate_intent.cancel_chain();
    }

    pub(super) fn zoom_timeline(&mut self, inward: bool) {
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
        let horizon = self.viewport_horizon(state.timeline_frames);
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
        self.install_arranger_navigation(next);
        true
    }

    pub(super) fn fit_timeline(&mut self) {
        self.clear_retained_navigation();
        self.viewport = None;
    }

    pub(super) fn pan_timeline(&mut self, right: bool) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let horizon = self.viewport_horizon(state.timeline_frames);
        let current = self.visible_viewport(horizon);
        let panned = current.pan_half(horizon, right);
        self.install_arranger_navigation(panned);
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
        self.wake_poll();
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

    pub(super) fn toggle_metronome(&mut self) {
        if !self.metronome_pending() {
            self.queue_playback_action(
                PlaybackAction::SetMetronome(!self.metronome_enabled()),
                None,
            );
        }
    }

    fn queue_playback_action(&mut self, action: PlaybackAction, revision: Option<u64>) {
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
        if self.stopped_playback_restore.is_some() && intent_desired_playing(action) == Some(true) {
            self.deferred_play = Some((action, revision, self.project_generation));
            replace_status(
                &mut self.status,
                "restoring stopped playback · playback starts when ready",
            );
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

    /// Rebuild only a current, rejected canonical graph. A fresh graph drops
    /// effect history, so retain the user's transport intent and never restart
    /// a superseded project, output or explicit transport action.
    /// A streamed document's sampler edit replaces its output only while
    /// stopped, so a playing edit stops first and resumes here at the same
    /// frame (audio restarts at the cursor). Resident documents apply live.
    fn sampler_edit_resume_frame(&self, request: &Request) -> Option<u64> {
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

    pub(super) fn tick_sampler_edit_resume(&mut self) -> bool {
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

    /// Start a Play held behind the stopped restore once that restore
    /// settles, whether it succeeded or failed; a replaced project drops it.
    pub(super) fn tick_deferred_play(&mut self) -> bool {
        if self.stopped_playback_restore.is_some() {
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

    pub(super) fn tick_latency_rebuild(&mut self) -> bool {
        use super::audio_admission::LatencyRebuild;
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
            let project = super::audio_admission::ProjectRevision {
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

    fn request_latency_restore(
        &mut self,
        project: super::audio_admission::ProjectRevision,
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
            Some(super::audio_admission::StoppedPlaybackRestore::Preparing);
        self.latency_rebuild = Some(super::audio_admission::LatencyRebuild::Restoring {
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
    fn clear_latency_waiting(&mut self) -> bool {
        if self.error.as_deref() == Some(LATENCY_EDIT_WAITING) {
            self.error = None;
            true
        } else {
            false
        }
    }

    pub(super) fn stop(&mut self) {
        self.stop_transport(true);
    }

    fn stop_transport(&mut self, stop_output: bool) {
        use super::audio_admission::LatencyRebuild;
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
        let restore = stop_output
            && self.stopped_playback_restore.is_none()
            && !self.edit_pending()
            && self.device_gesture.snapshot().is_none()
            && self.tuning.client.snapshot().is_none()
            && self.recovery.generation().is_none()
            && self.audio.as_ref().is_some_and(|audio| {
                self.state.as_ref().is_some_and(|state| {
                    native_restore
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
        // Publish the MIDI finish intent before any input prefix is revoked.
        if stop_output && let Some(audio) = &self.audio {
            if let Some(trace) = &self.transport_trace {
                trace.stop(audio);
            }
            audio.control.stop();
        }
        self.cancel_tuning_audition();
        self.stop_computer_keyboard();
        self.cancel_scrub();
        self.invalidate_prepared_seek();
        self.offline_cursor = 0;
        self.recovery.stop();
        self.pending_audio_commands.clear();
        self.pending_metronome_ticket = None;
        self.pending_seek_ticket = None;
        self.pending_session_ticket = None;
        self.play_intent.priority_stop();
        self.status = "stopped · returned to start".into();
        if restore && !self.update_preparing() && self.enqueue(Request::RestoreStoppedPlayback) {
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

fn reconcile_verified_source_binding(
    admission: &mut super::audio_admission::AudioAdmissionStatus,
    mode: hex_session::DocumentAudioMode,
    output_identity: Option<u64>,
    generation: u64,
    current: Option<&ViewState>,
    source: Option<hex_audio::prepared::AppliedAudioSource>,
) -> (bool, Option<hex_audio::prepared::AppliedAudioSource>) {
    if let Some(source) = source
        && (mode == hex_session::DocumentAudioMode::VerifiedStreaming
            || current.is_some_and(|state| {
                hex_engine::native_device_resources::requires_private_sampler_sources(
                    &state.project,
                )
            }))
        && output_identity == Some(source.output_identity)
        && source.context.project_generation == generation
        && let Some(revision) =
            current.and_then(|state| state.audio_authoring_revision(source.context))
    {
        return (
            admission.applied(
                super::audio_admission::ProjectRevision {
                    generation,
                    revision,
                },
                source.audio_revision,
            ),
            Some(source),
        );
    }
    // Preserve factual historical ACKs, without accepting any old context for
    // the current authoring view. Exact source/output matching above is required.
    reconcile_verified_applied_source(admission, mode, output_identity, generation, None, source)
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
    Superseded,
    Interrupted,
}

fn reconcile_session_receipt(
    pending: Option<(u64, u64)>,
    current_generation: u64,
    receipt: hex_audio::prepared::ActionOutcomeReceipt,
) -> (Option<(u64, u64)>, Option<SessionTicketOutcome>) {
    let Some((ticket, generation)) = pending else {
        return (None, None);
    };
    if generation != current_generation {
        return (None, None);
    }
    if ticket != receipt.ticket {
        return (Some((ticket, generation)), None);
    }
    let outcome = match receipt.outcome {
        hex_audio::prepared::ActionOutcome::Applied => SessionTicketOutcome::Consumed,
        hex_audio::prepared::ActionOutcome::Rejected(_) => SessionTicketOutcome::Rejected,
        hex_audio::prepared::ActionOutcome::Superseded => SessionTicketOutcome::Superseded,
        hex_audio::prepared::ActionOutcome::Interrupted => SessionTicketOutcome::Interrupted,
    };
    (None, Some(outcome))
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

fn fault_with_action_errors(
    existing: &Option<super::display_error::DisplayError>,
    diagnostic: &str,
    action_errors: &str,
) -> String {
    if !action_errors.is_empty() {
        return format!("{diagnostic} · {action_errors}");
    }
    if let Some(existing) = existing.as_deref()
        && existing
            .strip_prefix(diagnostic)
            .is_some_and(|rest| rest.starts_with(" · "))
    {
        return existing.to_owned();
    }
    diagnostic.to_owned()
}

/// Shown while a refused latency edit in an instrument/MIDI project waits for
/// Pause or Stop, which apply it without cutting sounding voices.
pub(super) const LATENCY_EDIT_WAITING: &str =
    "edit waiting · latency layout could not change live · applies on pause or stop";

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
        reconcile_session_receipt, reconcile_session_ticket, rejected_candidate_project,
        rejected_preview_candidate, retained_install_fresh_policy,
    };
    use crate::editor::audio_admission::{AudioAdmissionStatus, ProjectRevision};
    use crate::editor::{PendingPlaybackAction, PendingSceneDuplicate, PlaybackCandidate};
    use hex_engine::playback::Playback;
    use hex_engine::{PlaybackAction, PlaybackCommand, SessionPlaybackCommand};
    use hex_project::{Command, Created, Editor, Project};
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
            worker_requires_preroll: None,
            observation: None,
            audio_revision,
            project_generation: generation,
            state: Arc::new(ViewState {
                canonical_audio_source: None,
                comparison_audio: None,
                project,
                waveforms: Default::default(),
                source_overviews: Default::default(),
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
    fn earlier_session_rejection_is_not_reported_as_consumed() {
        let (mut control, mut callback) = hex_audio::prepared::channel(48_000).unwrap();
        let mut reader = control
            .enable_action_outcomes_on_control(&mut callback)
            .unwrap();
        let playback = Playback::new(hex_engine::Project::new(48_000, Vec::new()).unwrap());
        control.publish(playback).unwrap();
        callback.begin_block();
        let first = control
            .launch_and_play_with_ticket(hex_engine::SessionPlaybackCommand::Stop)
            .unwrap();
        let second = control
            .action_with_ticket(PlaybackAction::SetMetronome(true))
            .unwrap();
        callback.begin_block();
        assert_eq!(
            control
                .status
                .consumed_command
                .load(std::sync::atomic::Ordering::Acquire),
            second
        );
        assert_eq!(
            reconcile_session_receipt(Some((first, 7)), 7, reader.try_take().unwrap()),
            (None, Some(SessionTicketOutcome::Rejected)),
            "a later rejection cannot turn the earlier failed action into consumed feedback"
        );
        assert_eq!(reader.try_take().unwrap().ticket, second);
        control.request_shutdown();
        control
            .retire_action_outcomes_on_control(&mut callback)
            .unwrap();
        callback.begin_block();
        control.reclaim();
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
    fn close_prompt_allows_only_owned_duplicate_continuations() {
        assert!(allows_non_open_request(true, false, true));
        assert!(!allows_non_open_request(true, false, false));
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

#[cfg(all(test, feature = "ui-test-support"))]
mod tuning_audition_error_tests {
    use super::*;
    use gpui::TestAppContext;
    fn next(service: &hex_session::service::Service) -> Event {
        let deadline = crate::test_wait::hang_deadline();
        loop {
            match service.try_event() {
                Ok(event) => return event,
                Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(2))
                }
                Err(error) => panic!("worker did not reply: {error}"),
            }
        }
    }
    #[gpui::test]
    fn review_current_audition_worker_refusal_is_visible_but_stale_owner_is_ignored(
        cx: &mut TestAppContext,
    ) {
        let service = hex_session::service::Service::new(48_000, 8, 1_000_000).unwrap();
        let initial = next(&service);
        let (_, editor) = crate::editor::clip_drag_ui_test::editor_window(
            cx,
            initial.state.project.as_ref().clone(),
            Arc::default(),
        );
        let old = Arc::new(hex_engine::tuning_audition::TuningAuditionInput::new(0));
        let active = Arc::new(hex_engine::tuning_audition::TuningAuditionInput::new(0));
        editor.update(cx, |view, _| {
            view.state = Some(Arc::clone(&initial.state));
            view.tuning.open = true;
            view.tuning.audition_input = Some(Arc::clone(&active));
        });
        for (input, stale) in [(old, true), (Arc::clone(&active), false)] {
            service
                .submit(Request::PrepareTuningAudition {
                    revision: 99,
                    input,
                })
                .unwrap();
            let event = next(&service);
            assert!(event.error.is_some());
            assert!(
                event
                    .tuning_audition_complete
                    .as_ref()
                    .unwrap()
                    .is_cancelled()
            );
            editor.update(cx, |view, _| {
                view.receive(event);
                if stale {
                    assert!(view.error.is_none());
                    assert!(Arc::ptr_eq(
                        view.tuning.audition_input.as_ref().unwrap(),
                        &active
                    ));
                } else {
                    assert!(view.tuning.error.is_some());
                    assert!(view.tuning.audition_input.is_none());
                }
            });
        }
    }
}

#[cfg(test)]
mod comparison_source_binding_tests {
    use super::super::audio_admission::{AdmissionState, AudioAdmissionStatus, ProjectRevision};
    use super::*;
    #[test]
    fn comparison_exact_source_maps_verified_observations_without_status_oscillation_or_old_ack_promotion()
     {
        // Actual worker-minted binding; AppliedAudioSource below is a nonowning
        // protocol fixture, not proof that a native constructor/callback ran.
        let service = hex_session::service::Service::new(48_000, 16, 0).unwrap();
        let deadline = crate::test_wait::hang_deadline();
        let receive = || loop {
            assert!(Instant::now() < deadline, "bounded comparison worker");
            match service.try_event() {
                Ok(event) => break event,
                Err(std::sync::mpsc::TryRecvError::Empty) => std::thread::yield_now(),
                Err(error) => panic!("worker {error}"),
            }
        };
        receive();
        service
            .submit(Request::Edit(hex_project::Command::InsertTrackWithDevice {
                name: "source".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            }))
            .unwrap();
        let initial = receive();
        let device = initial.state.project.tracks()[0].instrument().unwrap();
        service
            .submit(Request::CompareDevice {
                owner: hex_session::service::preset::Owner {
                    project_generation: 0,
                    revision: initial.state.revision,
                    device: device.id(),
                    class: hex_engine::NativeDeviceClass::Instrument,
                    type_id: device.state().type_id(),
                },
                expected: hex_project::DeviceVariant::A,
                action: hex_project::DeviceComparisonAction::Switch,
            })
            .unwrap();
        let metadata = receive();
        let source = metadata.state.canonical_audio_source.as_ref().unwrap();
        let actual = hex_audio::prepared::AppliedAudioSource {
            context: source.context(),
            audio_revision: 31,
            output_identity: 41,
        };
        let mode = hex_session::DocumentAudioMode::VerifiedStreaming;
        let mut status = AudioAdmissionStatus::default();
        status.canonical_revision(ProjectRevision {
            generation: 0,
            revision: metadata.state.revision,
        });
        assert_eq!(
            reconcile_verified_source_binding(
                &mut status,
                mode,
                Some(41),
                0,
                Some(&metadata.state),
                None
            ),
            (false, None)
        );
        assert_eq!(status.current().unwrap().state, AdmissionState::Pending);
        let (changed, current) = reconcile_verified_source_binding(
            &mut status,
            mode,
            Some(41),
            0,
            Some(&metadata.state),
            Some(actual),
        );
        assert!(changed);
        assert_eq!(current, Some(actual));
        assert_eq!(
            status.current().unwrap().project.revision,
            metadata.state.revision
        );
        assert_eq!(status.current().unwrap().state, AdmissionState::Applied);
        assert_eq!(
            reconcile_verified_source_binding(
                &mut status,
                mode,
                Some(41),
                0,
                Some(&metadata.state),
                Some(actual)
            ),
            (false, Some(actual))
        );
        assert_eq!(
            reconcile_verified_source_binding(
                &mut status,
                mode,
                Some(42),
                0,
                Some(&metadata.state),
                Some(actual)
            ),
            (false, None)
        );
        assert_eq!(
            reconcile_verified_source_binding(
                &mut status,
                mode,
                Some(41),
                1,
                Some(&metadata.state),
                Some(actual)
            ),
            (false, None)
        );
        service
            .submit(Request::Edit(hex_project::Command::Mute {
                track: initial.state.project.tracks()[0].id(),
                muted: true,
            }))
            .unwrap();
        let audio = receive();
        status.canonical_revision(ProjectRevision {
            generation: 0,
            revision: audio.state.revision,
        });
        status.unpublished_refusal(
            ProjectRevision {
                generation: 0,
                revision: audio.state.revision,
            },
            "refused",
        );
        let (_, current) = reconcile_verified_source_binding(
            &mut status,
            mode,
            Some(41),
            0,
            Some(&audio.state),
            Some(actual),
        );
        assert!(current.is_none());
        assert_eq!(
            status.current().unwrap().state,
            AdmissionState::Rejected("refused")
        );
        assert_eq!(
            status.current().unwrap().project.revision,
            audio.state.revision
        );
    }
}

#[cfg(test)]
mod sampler_native_ui_protocol_tests {
    use super::*;
    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn worker_preview_ack_waits_for_exact_submission_metadata(cx: &mut gpui::TestAppContext) {
        worker_preview_ack_case(cx, false);
        worker_preview_ack_case(cx, true);
    }

    #[cfg(feature = "ui-test-support")]
    fn worker_preview_ack_case(cx: &mut gpui::TestAppContext, loaded_sampler: bool) {
        let _output_fixture_guard = crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
        use hex_audio::prepared::PublicationPolicy;
        use hex_engine::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
        let service = hex_session::service::Service::with_native_reservation(
            48_000,
            8,
            8_000_000,
            hex_session::ResidentNativeReservation {
                storage: hex_engine::AudioStorageLedgerReservation {
                    data_rows: 512,
                    handle_rows: 256,
                    root_rows: 128,
                    max_retained_bytes: 20_000_000,
                    max_construction_peak_bytes: 20_000_000,
                },
                max_scan_frames: 100_000,
            },
        )
        .unwrap();
        let receive = || {
            let deadline = crate::test_wait::hang_deadline();
            loop {
                match service.try_event() {
                    Ok(event) => break event,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "worker_preview_ack_case: hang guard expired"
                        );
                        std::thread::yield_now();
                    }
                    Err(error) => panic!("{error}"),
                }
            }
        };
        receive();
        let command = if loaded_sampler {
            hex_project::Command::AddMidiTrack {
                name: "ack sampler".into(),
            }
        } else {
            hex_project::Command::InsertTrackWithDevice {
                name: "ack fixture".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            }
        };
        service.submit(Request::Edit(command)).unwrap();
        let mut event = receive();
        let limits = hex_engine::AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 256,
            root_rows: 128,
            max_retained_bytes: 20_000_000,
            max_construction_peak_bytes: 20_000_000,
        };
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut sources = ResidentDeviceAssets::default();
        let sample_path = loaded_sampler.then(|| {
            std::env::temp_dir().join(format!(
                "hex-preview-ack-{}-{}.wav",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            ))
        });
        if let Some(path) = &sample_path {
            let mut wav = Vec::new();
            wav.extend_from_slice(b"RIFF");
            wav.extend_from_slice(&52_u32.to_le_bytes());
            wav.extend_from_slice(b"WAVEfmt ");
            wav.extend_from_slice(&16_u32.to_le_bytes());
            wav.extend_from_slice(&1_u16.to_le_bytes());
            wav.extend_from_slice(&2_u16.to_le_bytes());
            wav.extend_from_slice(&48_000_u32.to_le_bytes());
            wav.extend_from_slice(&192_000_u32.to_le_bytes());
            wav.extend_from_slice(&4_u16.to_le_bytes());
            wav.extend_from_slice(&16_u16.to_le_bytes());
            wav.extend_from_slice(b"data");
            wav.extend_from_slice(&16_u32.to_le_bytes());
            for _ in 0..8 {
                wav.extend_from_slice(&8192_i16.to_le_bytes());
            }
            std::fs::write(path, wav).unwrap();
            let track = event.state.project.tracks()[0].id();
            service
                .submit(Request::LoadFirstSampler {
                    request_id: 1,
                    project_generation: 0,
                    track,
                    path: path.clone(),
                    revision: event.state.revision,
                    empty_sampler: None,
                    cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                })
                .unwrap();
            event = receive();
            assert!(event.error.is_none(), "{:?}", event.error);
            let hex_project::Created::Asset(asset) = event.created.unwrap() else {
                panic!("asset")
            };
            sources
                .insert_on_control(
                    (
                        asset,
                        hex_engine::OwnedAudioSource::try_resident(48_000, vec![[0.25; 2]; 4])
                            .ok()
                            .unwrap(),
                    ),
                    0,
                    limits,
                    4,
                    &cancel,
                    || Ok::<_, hex_engine::native_device_resources::ResourcePreparationError>(()),
                )
                .unwrap();
        }
        let resources = PreparedDeviceResources::prepare_on_control(
            &sources,
            &event.state.project,
            1_000_000,
            &cancel,
        )
        .unwrap();
        let prepared = Arc::new(
            hex_engine::PreparedRevision::prepare_with_device_resources(
                &event.state.project,
                &std::collections::BTreeMap::new(),
                Arc::new(hex_engine::SineTable::new()),
                None,
                &hex_engine::audio_transform::AudioTransformPreparation::default(),
                &resources,
                100_000,
                &cancel,
            )
            .unwrap(),
        );
        let observation = Arc::new(
            hex_engine::PreparedPlaybackObservation::prepare_on_control(&prepared).unwrap(),
        );
        let canonical = event.state;
        let context = canonical.canonical_audio_source.as_ref().unwrap().context();
        let deadline = crate::test_wait::hang_deadline();
        let (mut output, mut backend) = loop {
            match hex_audio::worker_fixture::open(48_000) {
                Ok(output) => break output,
                Err(error)
                    if error == "audio output is open or still shutting down"
                        && Instant::now() < deadline =>
                {
                    std::thread::yield_now()
                }
                Err(error) => panic!("{error}"),
            }
        };
        output
            .publication
            .control()
            .unwrap()
            .configure_device_resource_publication_on_control(limits)
            .unwrap();
        let client = output.take_client().unwrap();
        assert!(client.worker_owns_graph_publication());
        let publish = |output: &mut hex_audio::WorkerPreparedOutput| {
            output
                .publication
                .control()
                .unwrap()
                .publish_with_policy_and_context_generation(
                    hex_engine::playback::Playback::try_from_prepared_with_device_resources(
                        Arc::clone(&prepared),
                        &resources,
                    )
                    .unwrap(),
                    PublicationPolicy::InheritRuntime,
                    context.project_generation,
                )
                .unwrap()
        };
        let preview_revision = publish(&mut output);
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        assert_eq!(
            backend.status().unwrap().applied_revision.load(Acquire),
            preview_revision
        );
        assert!(
            backend.status().unwrap().applied_audio_source().is_none(),
            "ordinary publication uses protected metadata, not eligible-constructor source ACKs"
        );
        let (_, editor) = super::super::clip_drag_ui_test::editor_window(
            cx,
            canonical.project.as_ref().clone(),
            Arc::clone(&canonical.midi_preview),
        );
        editor.update(cx, |view, cx| {
            view.service = service;
            view.state = Some(Arc::clone(&canonical));
            view.project_generation = context.project_generation;
            view.document_audio_mode = hex_session::DocumentAudioMode::Resident;
            view.audio = Some(crate::editor_backend::EditorAudio {
                control: client,
                sample_rate: 48_000,
                label: "private control".into(),
                spectrum_error: None,
                mode: hex_session::DocumentAudioMode::Resident,
            });
            view.pending_playback = None;
            view.playback_candidates.clear();
            view.applied_project_revision = None;
            let key = view
                .device_gesture
                .begin(
                    context.project_generation,
                    hex_session::device_gesture::GestureTarget::TrackGain {
                        track: canonical.project.tracks()[0].id(),
                    },
                    hex_session::device_gesture::ParameterValue::Float(1.0),
                )
                .unwrap();
            view.device_gesture.next_request().unwrap();
            assert!(
                view.device_gesture
                    .acknowledge(&hex_session::device_gesture::GestureAck {
                        preview_retained: false,
                        key,
                        phase: hex_session::device_gesture::GesturePhase::Begin,
                        sequence: 0,
                        accepted: true,
                        reason: None,
                    })
            );
            let identity = hex_session::service::DevicePreviewIdentity {
                project_generation: context.project_generation,
                key,
                sequence: 0,
                canonical_revision: canonical.revision,
            };
            view.resident_device_preview = Some(hex_session::service::DevicePreview {
                identity,
                state: Arc::clone(&canonical),
            });
            let before = view.audio_admission;
            view.tick(cx);
            assert!(
                view.audible_playback.is_none(),
                "context-only preview ACK fabricated canonical audio"
            );
            assert!(view.applied_project_revision.is_none());
            assert_eq!(view.audio_admission, before);
            let mut metadata = hex_session::service::ResidentPublicationMetadata {
                context,
                device_preview: Some(identity),
                tuning_preview: None,
                seek_request: None,
                stopped_restore: false,
                computer_keyboard_request: None,
                gain_recording_track: None,
                requires_preroll: false,
            };
            let output_identity = view
                .audio
                .as_ref()
                .unwrap()
                .control
                .output_identity()
                .unwrap();
            view.retain_publication_observation(
                context,
                output_identity,
                crate::EditorPublicationState::ResidentSubmitted {
                    audio_revision: preview_revision,
                    observation: Arc::clone(&observation),
                    metadata,
                },
            );
            view.tick(cx);
            let audible = view.audible_playback.as_ref().unwrap();
            assert_eq!(audible.device_preview_identity, Some(identity));
            assert!(Arc::ptr_eq(
                audible.observation.as_ref().unwrap(),
                &observation
            ));
            // A subsequent canonical graph still waits for its own protected metadata.
            let canonical_revision = publish(&mut output);
            backend.render(&mut [[0.0; 2]; 64]).unwrap();
            view.tick(cx);
            assert_eq!(
                view.audible_playback.as_ref().unwrap().audio_revision,
                preview_revision
            );
            metadata.device_preview = None;
            view.retain_publication_observation(
                context,
                output_identity,
                crate::EditorPublicationState::ResidentSubmitted {
                    audio_revision: canonical_revision,
                    observation: Arc::clone(&observation),
                    metadata,
                },
            );
            view.tick(cx);
            let audible = view.audible_playback.as_ref().unwrap();
            assert_eq!(audible.audio_revision, canonical_revision);
            assert!(audible.device_preview_identity.is_none());
            if loaded_sampler {
                view.audio = None;
                return;
            }

            // A real metadata-only comparison retains the old source context. A subsequent
            // runtime publication uses the current authored revision and must not wait forever.
            view.device_gesture.reset();
            view.resident_device_preview = None;
            let device = canonical.project.tracks()[0].instrument().unwrap();
            view.service
                .submit(Request::CompareDevice {
                    owner: hex_session::service::preset::Owner {
                        project_generation: context.project_generation,
                        revision: canonical.revision,
                        device: device.id(),
                        class: hex_engine::NativeDeviceClass::Instrument,
                        type_id: device.state().type_id(),
                    },
                    expected: hex_project::DeviceVariant::A,
                    action: hex_project::DeviceComparisonAction::Switch,
                })
                .unwrap();
            let deadline = crate::test_wait::hang_deadline();
            let event = loop {
                match view.service.try_event() {
                    Ok(event) => break event,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "worker_preview_ack_case: hang guard expired"
                        );
                        std::thread::yield_now();
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            assert!(event.error.is_none(), "{:?}", event.error);
            view.receive(event);
            let authored = Arc::clone(view.state.as_ref().unwrap());
            assert!(authored.comparison_audio.is_some());
            assert!(authored.revision > context.editor_revision);
            assert_eq!(
                authored.canonical_audio_source.as_ref().unwrap().context(),
                context
            );
            let runtime_context = hex_engine::AudioSourceContext {
                editor_revision: authored.revision,
                ..context
            };
            let runtime_prepared = Arc::new(
                hex_engine::PreparedRevision::prepare(
                    &authored.project,
                    &std::collections::BTreeMap::new(),
                    Arc::new(hex_engine::SineTable::new()),
                    None,
                )
                .unwrap(),
            );
            let runtime_observation = Arc::new(
                hex_engine::PreparedPlaybackObservation::prepare_on_control(&runtime_prepared)
                    .unwrap(),
            );
            let runtime_revision = output
                .publication
                .control()
                .unwrap()
                .publish_with_policy_and_context_generation(
                    hex_engine::playback::Playback::try_from_prepared_with_session(
                        runtime_prepared,
                    )
                    .unwrap(),
                    PublicationPolicy::InheritRuntime,
                    context.project_generation,
                )
                .unwrap();
            metadata.context = runtime_context;
            view.retain_publication_observation(
                runtime_context,
                output_identity,
                crate::EditorPublicationState::ResidentSubmitted {
                    audio_revision: runtime_revision,
                    observation: runtime_observation,
                    metadata,
                },
            );
            assert!(
                view.pending_publication_observation.is_none(),
                "current authored publication waited on retained source context"
            );
            assert_eq!(
                view.playback_candidates.back().unwrap().audio_revision,
                runtime_revision
            );
            backend.render(&mut [[0.0; 2]; 64]).unwrap();
            view.tick(cx);
            assert_eq!(
                view.audible_playback.as_ref().unwrap().audio_revision,
                runtime_revision
            );
            assert_eq!(view.applied_project_revision, Some(authored.revision));
            view.audio = None;
        });
        backend.close();
        drop(output);
        if let Some(path) = sample_path {
            std::fs::remove_file(path).unwrap();
        }
    }
    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn resident_publication_correlates_preview_restart_and_stopped_restore(
        cx: &mut gpui::TestAppContext,
    ) {
        let service = hex_session::service::Service::new(16_000, 8, 0).unwrap();
        let deadline = crate::test_wait::hang_deadline();
        let event = loop {
            match service.try_event() {
                Ok(event) => break event,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "resident_publication_correlates_preview_restart_and_stopped_restore: hang guard expired"
                    );
                    std::thread::yield_now();
                }
                Err(error) => panic!("{error}"),
            }
        };
        let observation = Arc::new(
            hex_engine::PreparedPlaybackObservation::prepare_on_control(
                event
                    .playback
                    .as_ref()
                    .unwrap()
                    .prepared_revision()
                    .unwrap(),
            )
            .unwrap(),
        );
        let canonical = event.state;
        let context = canonical.canonical_audio_source.as_ref().unwrap().context();
        let preview = Arc::new(ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: Arc::clone(&canonical.project),
            waveforms: Default::default(),
            source_overviews: Default::default(),
            audio_clip_waveforms: Default::default(),
            midi_preview: Arc::clone(&canonical.midi_preview),
            gain_preview: canonical.gain_preview.clone(),
            timeline_frames: 1234,
            can_undo: false,
            can_redo: false,
            revision: canonical.revision,
            unsaved: false,
            document_path: None,
        });
        let (_, editor) = super::super::clip_drag_ui_test::editor_window(
            cx,
            canonical.project.as_ref().clone(),
            Arc::clone(&canonical.midi_preview),
        );
        editor.update(cx, |view, _| {
            view.state = Some(Arc::clone(&canonical));
            view.project_generation = context.project_generation;
            let identity = hex_session::service::DevicePreviewIdentity {
                project_generation: context.project_generation,
                key: hex_session::device_gesture::GestureKey {
                    generation: context.project_generation,
                    token: 1,
                },
                sequence: 2,
                canonical_revision: canonical.revision,
            };
            let metadata = hex_session::service::ResidentPublicationMetadata {
                context,
                device_preview: Some(identity),
                tuning_preview: None,
                seek_request: None,
                stopped_restore: false,
                computer_keyboard_request: None,
                gain_recording_track: None,
                requires_preroll: true,
            };
            let before = view.audio_admission;
            view.apply_publication_observation(
                context,
                crate::EditorPublicationState::ResidentSubmitted {
                    audio_revision: 7,
                    observation: Arc::clone(&observation),
                    metadata,
                },
            );
            assert!(view.playback_candidates.is_empty());
            assert_eq!(view.audio_admission, before);
            view.resident_device_preview = Some(hex_session::service::DevicePreview {
                identity,
                state: Arc::clone(&preview),
            });
            let mut stale = metadata;
            stale.device_preview.as_mut().unwrap().sequence -= 1;
            assert!(view.resident_publication_view(stale).is_none());
            view.apply_publication_observation(
                context,
                crate::EditorPublicationState::ResidentSubmitted {
                    audio_revision: 7,
                    observation: Arc::clone(&observation),
                    metadata,
                },
            );
            let candidate = view.playback_candidates.back().unwrap();
            assert!(Arc::ptr_eq(&candidate.state, &preview));
            assert!(candidate.prepared_revision.is_none());
            assert!(Arc::ptr_eq(
                candidate.observation.as_ref().unwrap(),
                &observation,
            ));
            assert!(
                view.audible_playback.is_none(),
                "submission is not application"
            );
            assert_eq!(candidate.worker_requires_preroll, Some(true));
            assert!(Arc::ptr_eq(view.state.as_ref().unwrap(), &canonical));
            assert_eq!(view.audio_admission, before);
            view.applied_project_revision = Some(canonical.revision);
            view.observed_rejected_revision = 70;
            view.pending_publication_observation = Some(PendingEditorPublicationObservation {
                context,
                output_identity: 41,
                state: crate::EditorPublicationState::ResidentSubmitted {
                    audio_revision: 7,
                    observation: Arc::clone(&observation),
                    metadata,
                },
            });
            view.reset_output_publication_tracking(Some(42));
            assert!(view.playback_candidates.is_empty());
            assert!(view.audible_playback.is_none());
            assert!(view.applied_project_revision.is_none());
            assert_eq!(view.observed_rejected_revision, 0);
            assert!(view.pending_publication_observation.is_none());
            view.pending_publication_observation = Some(PendingEditorPublicationObservation {
                context,
                output_identity: 42,
                state: crate::EditorPublicationState::ResidentSubmitted {
                    audio_revision: 7,
                    observation: Arc::clone(&observation),
                    metadata,
                },
            });
            view.reset_output_publication_tracking(Some(42));
            assert!(view.pending_publication_observation.is_some());
            view.apply_publication_observation(
                context,
                crate::EditorPublicationState::ResidentSubmitted {
                    audio_revision: 7,
                    observation: Arc::clone(&observation),
                    metadata,
                },
            );
            assert_eq!(view.playback_candidates.len(), 1);
            view.stopped_playback_restore =
                Some(super::super::audio_admission::StoppedPlaybackRestore::Preparing);
            let mut canonical_metadata = metadata;
            canonical_metadata.device_preview = None;
            view.apply_publication_observation(
                context,
                crate::EditorPublicationState::ResidentSubmitted {
                    audio_revision: 8,
                    observation: Arc::clone(&observation),
                    metadata: canonical_metadata,
                },
            );
            assert_eq!(
                view.stopped_playback_restore,
                Some(super::super::audio_admission::StoppedPlaybackRestore::Preparing),
                "an ordinary canonical submission cannot complete stopped restoration"
            );
            canonical_metadata.stopped_restore = true;
            view.apply_publication_observation(
                context,
                crate::EditorPublicationState::ResidentSubmitted {
                    audio_revision: 9,
                    observation: Arc::clone(&observation),
                    metadata: canonical_metadata,
                },
            );
            assert_eq!(
                view.stopped_playback_restore,
                Some(super::super::audio_admission::StoppedPlaybackRestore::Published(9)),
                "only the tagged submission binds restoration to its actual audio revision"
            );
        });
    }

    #[test]
    fn targeted_release_ignores_newer_authoring_other_output_and_other_document() {
        let context = hex_engine::AudioSourceContext {
            project_generation: 4,
            editor_revision: 7,
        };
        assert!(relinquish_client_matches(4, Some(6), Some(12), 12, context));
        assert!(!relinquish_client_matches(
            4,
            Some(8),
            Some(12),
            12,
            context
        ));
        assert!(!relinquish_client_matches(
            4,
            Some(7),
            Some(13),
            12,
            context
        ));
        assert!(!relinquish_client_matches(
            5,
            Some(7),
            Some(12),
            12,
            context
        ));
    }
    #[test]
    fn native_sampler_applied_source_recovers_lost_submitted_metadata_without_legacy_promotion() {
        // UI protocol metadata only; actual native proof/PCM is covered by the
        // Service/Desktop eligible publication and callback integration probes.
        let service = hex_session::service::Service::new(16000, 8, 0).unwrap();
        let end = crate::test_wait::hang_deadline();
        let event = loop {
            match service.try_event() {
                Ok(e) => break e,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    assert!(
                        std::time::Instant::now() < end,
                        "native_sampler_applied_source_recovers_lost_submitted_metadata_without_legacy_promotion: hang guard expired"
                    );
                    std::thread::yield_now();
                }
                Err(e) => panic!("{e}"),
            }
        };
        let context = event
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context();
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(16000).unwrap(), 4).unwrap();
        let hex_project::Created::Track(track) = editor
            .execute(hex_project::Command::AddMidiTrack {
                name: "sampler".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        editor
            .execute(hex_project::Command::InsertInstrument {
                track,
                state: hex_lattice_contract::NativeDeviceState::new(
                    hex_lattice_contract::DeviceTypeId::new("hex.instrument.sampler").unwrap(),
                    1,
                    vec![],
                )
                .unwrap(),
            })
            .unwrap();
        let state = ViewState {
            canonical_audio_source: event.state.canonical_audio_source.clone(),
            comparison_audio: None,
            project: Arc::new(editor.project().clone()),
            waveforms: Default::default(),
            source_overviews: Default::default(),
            audio_clip_waveforms: Default::default(),
            midi_preview: event.state.midi_preview.clone(),
            gain_preview: event.state.gain_preview.clone(),
            timeline_frames: 16000,
            can_undo: false,
            can_redo: false,
            revision: context.editor_revision,
            unsaved: false,
            document_path: None,
        };
        let mut admission = super::super::audio_admission::AudioAdmissionStatus::default();
        admission.canonical_revision(super::super::audio_admission::ProjectRevision {
            generation: context.project_generation,
            revision: state.revision,
        });
        let source = hex_audio::prepared::AppliedAudioSource {
            context,
            output_identity: 12,
            audio_revision: 3,
        };
        assert_eq!(
            reconcile_verified_source_binding(
                &mut admission,
                hex_session::DocumentAudioMode::Resident,
                Some(13),
                context.project_generation,
                Some(&state),
                Some(source)
            ),
            (false, None)
        );
        assert_eq!(
            reconcile_verified_source_binding(
                &mut admission,
                hex_session::DocumentAudioMode::Resident,
                Some(12),
                context.project_generation,
                Some(&state),
                Some(source)
            )
            .1,
            Some(source)
        );
        let stale = hex_audio::prepared::AppliedAudioSource {
            context: hex_engine::AudioSourceContext {
                editor_revision: context.editor_revision + 1,
                ..context
            },
            ..source
        };
        assert_eq!(
            reconcile_verified_source_binding(
                &mut admission,
                hex_session::DocumentAudioMode::Resident,
                Some(12),
                context.project_generation,
                Some(&state),
                Some(stale)
            )
            .1,
            None
        );
    }
}
