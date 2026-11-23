//! Editor audio-event polling, output rebinding, publication observation reconciliation and comparison authoring.
use super::*;

// A comparison may retain its exact source context while authoring advances. Normalize only
// contexts accepted by the sealed comparison binding; intermediate revisions remain stale.
pub(in crate::editor) fn publication_observation_for_view(
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

pub(super) fn relinquish_client_matches(
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
    pub(super) fn reconcile_comparison_authoring(&mut self, next: &Arc<ViewState>) {
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
                && candidate.clip_preview_identity.is_none()
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
            crate::editor::audio_admission::ProjectRevision {
                generation: self.project_generation,
                revision: previous_revision,
            },
            crate::editor::audio_admission::ProjectRevision {
                generation: self.project_generation,
                revision: next.revision,
            },
            applied_is_current,
        );
        if applied_is_current && self.applied_project_revision == Some(previous_revision) {
            self.applied_project_revision = Some(next.revision);
        }
    }

    pub(in crate::editor) fn restart_audio_for_current_document(&mut self) -> bool {
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
                        crate::editor::user_errors::submit(&error)
                    )
                    .into(),
                );
                self.status = "audio unavailable".into();
                false
            }
        }
    }

    pub(in crate::editor) fn retire_audio_for_future_generation(&mut self) {
        self.invalidate_prepared_seek();
        self.retire_audio_preserving_gain_take();
        self.prepared_seek = None;
        self.latched_seek_intent = None;
        self.pending_recovery_generation = None;
        self.clear_recovery();
        self.pending_playback = None;
        self.pending_resident_publication = None;
        self.pending_audio_commands.clear();
        self.pending_seek_ticket = None;
        self.pending_metronome_ticket = None;
        self.pending_session_ticket = None;
        self.playback_candidates.clear();
        self.audible_playback = None;
        self.deferred_toggle_play = None;
        self.deferred_metadata_seek = None;
        self.audio_admission.reset_audio();
    }

    /// Whether the current endpoint may stay open across a project replacement
    /// until the worker reports `OutputRebound` or `OutputReleased`. Only an
    /// idle Resident endpoint qualifies; anything else retires as before.
    pub(super) fn hold_audio_across_replacement(&self, next_generation: u64) -> bool {
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
    pub(super) fn apply_output_rebound(&mut self, output_identity: u64) {
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

    pub(super) fn retain_future_audio_result(&mut self, result: PendingEditorAudioResult) {
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

    pub(super) fn retain_publication_observation(
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

    pub(super) fn reconcile_publication_observation(&mut self) -> bool {
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

    pub(super) fn device_publication_view(
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

    pub(super) fn resident_publication_view(
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

    pub(super) fn reset_output_publication_tracking(&mut self, output_identity: Option<u64>) {
        self.cancel_held_clip_move_audition();
        self.clip_preview_may_apply = false;
        self.device_panel.spectrum_results.clear();
        self.invalidate_prepared_seek();
        self.prepared_seek = None;
        self.playback_candidates.clear();
        self.audible_playback = None;
        self.applied_project_revision = None;
        self.observed_rejected_revision = 0;
        self.paused_republish = None;
        self.audio_admission.reset_audio();
        // A new output prepares fresh endpoints; it reports its own set.
        self.degraded_plugins = Default::default();
        self.plugin_editor_windows.clear();
        if self
            .pending_publication_observation
            .as_ref()
            .is_some_and(|pending| Some(pending.output_identity) != output_identity)
        {
            self.pending_publication_observation = None;
        }
    }

    pub(super) fn apply_publication_observation(
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
                    && self.playback_candidates.len() < crate::editor::PLAYBACK_CANDIDATE_CAPACITY
                {
                    self.playback_candidates.push_back(PlaybackCandidate {
                        clip_preview_identity: None,
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
            crate::EditorPublicationState::VerifiedArrangementGainSubmitted { .. } => {
                self.status = "publishing gain preview".into();
            }
            crate::EditorPublicationState::VerifiedArrangementGainRefused(reason) => {
                self.status = format!("gain preview unavailable · {}", reason.to_lowercase());
            }
            crate::EditorPublicationState::VerifiedArrangementPlayStartSubmitted { .. } => {
                self.status = "publishing play start preview".into();
            }
            crate::EditorPublicationState::VerifiedArrangementPlayStartRefused(reason) => {
                self.status = format!("play start preview unavailable · {}", reason.to_lowercase());
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
                    u8::from(metadata.device_preview.is_some())
                        + u8::from(metadata.tuning_preview.is_some())
                        + u8::from(metadata.clip_preview.is_some())
                        > 1
                        || metadata
                            .clip_preview
                            .is_some_and(|identity| identity.context != context)
                }) {
                    self.error =
                        Some("audio publication has conflicting preview identities".into());
                    return;
                }
                let source_state = match metadata {
                    Some(metadata) => self.resident_publication_view(metadata),
                    None => self.state.as_ref().cloned(),
                };
                if let Some(state) = source_state {
                    let candidate = PlaybackCandidate {
                        clip_preview_identity: metadata.and_then(|metadata| metadata.clip_preview),
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
                    };
                    // If the audio callback already applied this revision before
                    // `worker_submitted` arrived, backfill the fallback entry
                    // immediately so preroll/observation queries never stay stranded.
                    if candidate.clip_preview_identity.is_none()
                        && let Some(audible) = self.audible_playback.as_mut().filter(|audible| {
                            audible.audio_revision == audio_revision
                                && audible.project_generation == context.project_generation
                        })
                    {
                        if audible.worker_requires_preroll.is_none() {
                            audible.worker_requires_preroll = candidate.worker_requires_preroll;
                        }
                        if audible.observation.is_none() {
                            audible.observation = candidate.observation.clone();
                        }
                        if audible.device_preview_identity.is_none() {
                            audible.device_preview_identity = candidate.device_preview_identity;
                        }
                        if audible.tuning_preview_identity.is_none() {
                            audible.tuning_preview_identity = candidate.tuning_preview_identity;
                        }
                        if audible.seek_request_id.is_none() {
                            audible.seek_request_id = candidate.seek_request_id;
                        }
                    }
                    if !self.playback_candidates.iter().any(|existing| {
                        existing.audio_revision == audio_revision
                            && existing.project_generation == context.project_generation
                    }) && self.playback_candidates.len()
                        < crate::editor::PLAYBACK_CANDIDATE_CAPACITY
                    {
                        self.playback_candidates.push_back(candidate);
                    }
                }
                // A section seek repairing a refused loop leaves the current
                // admission in place until the callback applies the seek.
                if metadata.is_none_or(|metadata| {
                    metadata.device_preview.is_none()
                        && metadata.tuning_preview.is_none()
                        && metadata.clip_preview.is_none()
                        && (metadata.seek_request.is_none()
                            || self.applied_project_revision == Some(authoring_revision))
                }) {
                    self.audio_admission.published(
                        crate::editor::audio_admission::ProjectRevision {
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
                            crate::editor::audio_admission::StoppedPlaybackRestore::Preparing
                            | crate::editor::audio_admission::StoppedPlaybackRestore::Ready => true,
                            crate::editor::audio_admission::StoppedPlaybackRestore::WaitingForEdits {
                                ..
                            } => false,
                            crate::editor::audio_admission::StoppedPlaybackRestore::Published(previous) => {
                                audio_revision >= previous
                            }
                        })
                {
                    self.stopped_playback_restore = Some(
                        crate::editor::audio_admission::StoppedPlaybackRestore::Published(audio_revision),
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
                        crate::editor::arrangement_seek::SeekStage::Published { audio_revision };
                    pending.cancellation_retry |= pending.cancelled;
                }
                if self.document_audio_mode == hex_session::DocumentAudioMode::VerifiedStreaming
                    && let Some(generation) = self.pending_recovery_generation
                {
                    self.recovery.restored(generation);
                }
                let applied_revision = self
                    .audio_status()
                    .map_or(0, |status| status.applied_revision.load(Acquire));
                record_submitted_audio_status(&mut self.status, audio_revision, applied_revision);
            }
            crate::EditorPublicationState::Refused(reason) => {
                self.audio_admission.unpublished_refusal(
                    crate::editor::audio_admission::ProjectRevision {
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
                crate::EditorAudioEvent::PluginTransportToggle { project_generation } => {
                    if project_generation == self.project_generation {
                        let _scope = crate::diagnostics::action_scope(cx, "hex_editor::TogglePlay");
                        self.toggle_play();
                        changed = true;
                    }
                }
                crate::EditorAudioEvent::PluginEditorWindows {
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
                        changed |= self.plugin_editor_windows != devices;
                        self.plugin_editor_windows = devices;
                    }
                }
                crate::EditorAudioEvent::PluginParameterTouched {
                    source,
                    binding,
                    device,
                    parameter,
                    sequence,
                    value,
                } => {
                    crate::editor::device_panel::vst3_parameters::touched(
                        self, source, binding, device, parameter, sequence, value,
                    );
                    changed = true;
                }
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
                    let message = crate::editor::audio_unavailable::unavailable_message(&error);
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
                            Some(crate::editor::audio_admission::LatencyRebuild::Waiting {
                                project: crate::editor::audio_admission::ProjectRevision {
                                    generation: self.project_generation,
                                    revision: state.revision,
                                },
                                output: source.output_identity,
                                runtime_audio: Some(source.audio_revision),
                            });
                        changed = true;
                    }
                }
                crate::EditorAudioEvent::PluginEditorRetirementIssue {
                    project_generation,
                    device: _,
                    issue,
                } => {
                    if project_generation == self.project_generation {
                        let (message, failed) = match issue {
                            crate::PluginEditorRetirementIssue::NeedsDisposition => ("vst3 editor changes are retained · pending project confirmation".to_owned(), true),
                            crate::PluginEditorRetirementIssue::Failure(reason) => (format!("vst3 audio needs attention · {} · recent edits may not be saved", reason.to_lowercase()), true),
                            crate::PluginEditorRetirementIssue::TailLatencyPending => ("vst3 tail latency changed · timing may be affected while the tail plays".to_owned(), false),
                        };
                        self.status = message.clone();
                        if failed {
                            self.error = Some(message.into());
                        }
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

    pub(super) fn poll_resident_publication(&mut self) -> bool {
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
                        == Some(crate::editor::audio_admission::StoppedPlaybackRestore::Ready)
                {
                    self.stopped_playback_restore = Some(
                        crate::editor::audio_admission::StoppedPlaybackRestore::Published(
                            audio_revision,
                        ),
                    );
                }
                self.gain_recorder.published(
                    pending
                        .prepared_revision
                        .as_ref()
                        .and_then(|prepared| prepared.gain_recording_track()),
                    audio_revision,
                );
                let project_revision = crate::editor::audio_admission::ProjectRevision {
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
                    clip_preview_identity: None,
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
                    seek.stage =
                        crate::editor::arrangement_seek::SeekStage::Published { audio_revision };
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
                let applied_revision = self
                    .audio_status()
                    .map_or(0, |status| status.applied_revision.load(Acquire));
                record_submitted_audio_status(&mut self.status, audio_revision, applied_revision);
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

    pub(in crate::editor) fn clear_route_telemetry(&mut self) {
        self.session_routes.clear();
    }

    /// Retire editor export custody for an `Export` or `ExportRange` the
    /// service refused before admission; no export event will ever arrive.
    pub(super) fn retire_unsubmitted_export(&mut self, request: &Request) {
        if matches!(request, Request::Export(_) | Request::ExportRange { .. }) {
            self.export_pending = false;
            if let Some(control) = self.export_control.take() {
                control.cancel();
            }
            self.export_window = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relinquish_client_matches_requires_exact_generation_revision_and_output() {
        let ctx = hex_engine::AudioSourceContext {
            project_generation: 3,
            editor_revision: 5,
        };
        assert!(relinquish_client_matches(3, Some(5), Some(11), 11, ctx));
        assert!(!relinquish_client_matches(4, Some(5), Some(11), 11, ctx));
        assert!(!relinquish_client_matches(3, Some(6), Some(11), 11, ctx));
        assert!(!relinquish_client_matches(3, Some(5), Some(12), 11, ctx));
        assert!(relinquish_client_matches(3, None, Some(11), 11, ctx));
        assert!(!relinquish_client_matches(3, Some(5), None, 11, ctx));
    }

    #[test]
    fn submitted_audio_status_respects_already_applied_and_clears_on_ack() {
        let mut status = String::from("audio batch imported · one undo");
        assert!(!record_submitted_audio_status(&mut status, 644, 644));
        assert_eq!(status, "audio batch imported · one undo");

        let mut stale_waiting = String::from("audio revision 643 submitted · waiting for playback");
        assert!(record_submitted_audio_status(&mut stale_waiting, 644, 644));
        assert_eq!(stale_waiting, "audio ready");

        let mut pending = String::from("preparing audio");
        assert!(record_submitted_audio_status(&mut pending, 645, 644));
        assert_eq!(
            pending,
            "audio revision 645 submitted · waiting for playback"
        );
        assert!(!clear_applied_submitted_audio_status(&mut pending, 644));
        assert!(clear_applied_submitted_audio_status(&mut pending, 645));
        assert_eq!(pending, "audio ready");
    }
}
