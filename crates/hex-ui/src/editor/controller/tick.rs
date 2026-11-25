//! Per-frame editor tick loop, spectrum target visibility and processor audition lifecycle.
use super::*;

fn install_current_applied_fallback(
    recovery: &mut crate::editor::recovery_state::RecoveryState,
    pending_recovery_generation: Option<u64>,
    current_state: Option<&Arc<ViewState>>,
    applied_project_revision: &mut Option<u64>,
    audible_playback: &mut Option<PlaybackCandidate>,
    source: hex_audio::prepared::AppliedAudioSource,
) {
    restore_verified_recovery_from_applied(recovery, pending_recovery_generation);
    // Actual ACK can arrive after Submitted metadata was coalesced.
    // Reconstruct only nonowning canonical view metadata, never an
    // audio owner or a state for an older, unavailable Project snapshot.
    if let Some(state) = current_state
        .filter(|state| state.audio_authoring_revision(source.context) == Some(state.revision))
    {
        *applied_project_revision = Some(state.revision);
        if audible_playback.as_ref().is_none_or(|candidate| {
            candidate.audio_revision != source.audio_revision
                || candidate.project_generation != source.context.project_generation
        }) {
            // Preserve known preroll metadata across same-revision updates
            // (such as EQ device-gesture previews) when the callback applies
            // the new audio revision before the UI drains `worker_submitted`.
            let retained_requires_preroll = audible_playback
                .as_ref()
                .filter(|previous| {
                    previous.project_generation == source.context.project_generation
                        && previous.state.revision == state.revision
                })
                .and_then(|previous| {
                    previous.worker_requires_preroll.or_else(|| {
                        previous
                            .prepared_revision
                            .as_ref()
                            .map(|prepared| prepared.arrangement_seek_requires_preroll())
                    })
                });
            *audible_playback = Some(PlaybackCandidate {
                clip_preview_identity: None,
                worker_requires_preroll: retained_requires_preroll,
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

impl EditorView {
    pub(in crate::editor) fn tick(&mut self, cx: &mut Context<Self>) -> bool {
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
                crate::editor::device_panel::trace_vst3_track_panel(self, snapshot);
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
        visible_change |= self.tick_automation_close(cx);
        if let Ok(view) = self.service.try_track_switch_view() {
            visible_change |= self.observe_track_switch_view(view);
        }
        let paste_view = match self.service.try_arrangement_edit_view() {
            Ok(view) if matches!(view.kind, ArrangementEditViewKind::Paste) => Some(view),
            Ok(view) if matches!(view.kind, ArrangementEditViewKind::LocatorAdd(_)) => {
                visible_change |= self.observe_locator_add_view(view);
                None
            }
            Ok(view) if matches!(view.kind, ArrangementEditViewKind::TimeRangeCopy) => {
                visible_change |= self.observe_time_range_copy_view(view);
                None
            }
            Ok(view) => {
                visible_change |= self.observe_duplicate_view(view);
                None
            }
            Err(_) => None,
        };
        visible_change |= self.poll_retired_action_outcomes();
        if self.retired_action_outcomes.is_none()
            && self.audio.as_ref().is_some_and(|audio| {
                audio.control.has_action_outcomes() && audio.control.status().is_none()
            })
        {
            visible_change |= self.retire_audio_preserving_gain_take();
        }
        visible_change |= self.poll_update(cx);
        visible_change |= self.sync_held_clip_move_audition();
        self.sync_held_stretch_audition();
        self.sync_held_play_start_audition();
        visible_change |= self.poll_browser_sample_playback();
        visible_change |= self.tick_gain_recording();
        visible_change |= self.tick_native_automation_recording();
        visible_change |= self.tick_automation_take_recovery();
        visible_change |= self.tick_midi_recording();
        visible_change |= self.tick_audio_recording();
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
                    visible_change |= self.automation_close_disconnected(cx);
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
                    if self.abandon_bounce() {
                        self.error = Some("bounce worker stopped".into());
                        visible_change = true;
                    }
                    if let Some(path) = self.audio_recorder.pending_commit_path.take() {
                        self.error = Some(
                            format!(
                                "audio recording commit outcome unknown · take retained at {}",
                                hex_session::service::text::verbatim(path.display())
                            )
                            .into(),
                        );
                        visible_change = true;
                    }
                    if self.cancel_new_preparation() {
                        visible_change |= replace_error(
                            &mut self.error,
                            "project worker stopped · new project cancelled",
                        );
                    }
                    self.submitted_clip_cuts.clear();
                    self.pending_created_note_selection = None;
                    self.midi_step.cancel_pending();
                    visible_change |= self.retire_pending_trim();
                    visible_change |= self.duplicate_intent.reject();
                    visible_change |= self.pending_time_range_move.take().is_some();
                    visible_change |= self.ahead_time_range_move.take().is_some();
                    visible_change |= self.pending_track_clipboard.take().is_some();
                    visible_change |= self.locators.clear_adds();
                    visible_change |= self.pending_group_copy.take().is_some();
                    visible_change |= self.committed_paste_view.take().is_some();
                    visible_change |= self.committed_time_range_copy_view.take().is_some();
                    visible_change |= self.retire_clip_detail();
                    visible_change |= self.retire_pending_audio_events();
                    visible_change |= self.retire_pending_clip_loop();
                    visible_change |= self.clear_pending_bypass();
                    visible_change |= self.cancel_modulation_target_drag();
                    visible_change |= self.retire_pending_modulation_depth();
                    visible_change |= self.cancel_audio_loop_gesture();
                    visible_change |= self.retire_pending_clip_move();
                    visible_change |= self.retire_pending_clip_paint();
                    visible_change |= self.retire_pending_section();
                    visible_change |= self.retire_pending_track_reorder();
                    visible_change |= self.retire_pending_midi_drag();
                    visible_change |= self.retire_pending_fade();
                    visible_change |= self.retire_clip_gain();
                    visible_change |= self.retire_clip_activation();
                    visible_change |= self.retire_pending_gain();
                    visible_change |= self.retire_pending_device_automation();
                    visible_change |= self.retire_all_eq_creations();
                    visible_change |= self.pending_loop.take().is_some();
                    visible_change |= self.section_launch_after_loop.take().is_some();
                    self.browser_insertion_flight = None;
                    visible_change |= !self.pending_browser_insertions.is_empty();
                    self.pending_browser_insertions.clear();
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
            }
        }
        // Earlier terminals may have been queued when this paste committed.
        if let Some(view) = paste_view {
            visible_change |= self.observe_paste_view(view);
        }
        if !self.edit_pending() {
            self.midi_step.cancel_pending();
        }
        visible_change |= self.submit_deferred_clip_move();
        visible_change |= self.submit_queued_section();
        visible_change |= self.drain_pending_browser_insertion(cx);
        visible_change |= self.poll_editor_audio_events(cx);
        visible_change |= self.poll_export(cx);
        visible_change |= self.poll_bounce_progress();
        visible_change |= self.pump_tuning_gesture();
        if !self.tuning.client.has_pending_requests() && !self.tuning.client.is_recovery_pending() {
            while self.pending_requests.front().is_some_and(|request| {
                !self.holds_stopped_vst3_requests()
                    || matches!(request, Request::RestoreStoppedPlayback)
            }) && let Some(request) = self.pending_requests.pop_front()
            {
                let Err(refused) = self.submit_to_service(request) else {
                    continue;
                };
                match refused.error {
                    SubmitError::Full(request) => {
                        self.pending_requests.push_front(*request);
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
        visible_change |= self.tick_stopped_vst3_restore();
        visible_change |= self.retire_abandoned_bypass();
        visible_change |= self.retire_abandoned_mixer_edit();
        visible_change |= self.retire_abandoned_modulation_depth();
        visible_change |= self.retire_abandoned_duplicate();
        visible_change |= self.retire_abandoned_track_clipboard();
        visible_change |= self.retire_abandoned_empty_track();
        visible_change |= self.retire_abandoned_device_duplicate();
        visible_change |= self.retire_abandoned_group_copy();
        visible_change |= self.retire_abandoned_locator_adds();
        visible_change |= self.retire_abandoned_clip_detail();
        visible_change |= self.retire_abandoned_clip_move();
        visible_change |= self.retire_abandoned_midi_drag();
        visible_change |= self.retire_abandoned_fade();
        visible_change |= self.retire_abandoned_clip_gain();
        visible_change |= self.retire_abandoned_clip_activation();
        let now = Instant::now();
        visible_change |= self.tick_time_gesture_autoscroll(now);
        visible_change |= self.tick_clip_gesture_autoscroll(now, cx);
        visible_change |= self.tick_track_reorder(now);
        visible_change |= self.poll_committed_copy();
        visible_change |= self.tick_edit_cursor(now);
        let project_edit_pending = self.edit_pending();
        visible_change |= self.tick_project_spectrum(cx);
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
            && self.audio.as_ref().is_some_and(|audio| {
                !crate::editor::arrangement_seek::audio_ready_for_seek(&audio.control)
            })
        {
            self.invalidate_prepared_seek();
        }
        visible_change |= self.tick_section_launch();
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
        let applied_section_source = applied_seek_source.filter(|_| {
            self.prepared_seek.as_ref().is_some_and(|pending| {
                pending.kind.section().is_some() && pending.device_preview_identity.is_none()
            })
        });
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
            visible_change |= crate::editor::device_panel::tick_hovered_meter(self);
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
                crate::editor::arrangement_seek::SeekStage::Published { .. }
            )
        });
        if !seek_publication_inflight
            && self.pending_resident_publication.is_none()
            && self.playback_candidates.len() < crate::editor::PLAYBACK_CANDIDATE_CAPACITY
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
                    .and_then(|seek| seek.kind.quantization())
                    .map_or(
                        PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
                        |quantization| PublicationPolicy::InstallQuantizedSeekWhenReady {
                            fence,
                            quantization,
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
        } else if self.playback_candidates.len() >= crate::editor::PLAYBACK_CANDIDATE_CAPACITY {
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
            .filter(|(revision, _)| {
                // The callback can refuse before the worker sends the view
                // metadata. Reconcile that exact candidate when it arrives.
                *revision > self.observed_rejected_revision
                    || (*revision == self.observed_rejected_revision
                        && self
                            .playback_candidates
                            .iter()
                            .any(|candidate| candidate.audio_revision == *revision))
            });
        if let Some((audio_revision, reason)) = rejected_revision {
            if reason == hex_engine::PlaybackHandoffRejection::LatencyLayoutChanged
                && let Some(output) = audio.control.output_identity()
            {
                self.latency_rebuild =
                    Some(crate::editor::audio_admission::LatencyRebuild::Unresolved {
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
                    == Some(
                        crate::editor::audio_admission::StoppedPlaybackRestore::Published(
                            audio_revision,
                        ),
                    )
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
                    self.latency_rebuild =
                        Some(crate::editor::audio_admission::LatencyRebuild::Waiting {
                            project,
                            output,
                            runtime_audio: None,
                        });
                } else if is_paused_republish_rejection(reason) {
                    visible_change |= record_paused_republish_rejection(
                        &mut self.paused_republish,
                        &mut self.deferred_play,
                        &mut self.status,
                        &mut self.error,
                        self.project_generation,
                        self.state.as_deref(),
                        &self.audio_admission,
                        project,
                        audio.control.output_identity(),
                        audio_revision,
                        reason,
                    );
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
            } else if is_paused_republish_rejection(reason)
                && let Some(output) = audio.control.output_identity()
                && !matches!(
                    self.paused_republish,
                    Some(
                        crate::editor::audio_admission::PausedRepublish::Submitted {
                            project,
                            output: submitted_output,
                            ..
                        } | crate::editor::audio_admission::PausedRepublish::Exhausted {
                            project,
                            output: submitted_output,
                        }
                    ) if project.generation == self.project_generation
                        && submitted_output == output
                )
            {
                self.paused_republish = Some(
                    crate::editor::audio_admission::PausedRepublish::Unresolved {
                        generation: self.project_generation,
                        output,
                        audio_revision,
                        reason,
                    },
                );
            }
        }
        let applied = audio
            .control
            .status()
            .map_or(0, |status| status.applied_revision.load(Acquire));
        if let Some(crate::editor::audio_admission::LatencyRebuild::Restoring {
            project,
            output,
            runtime_audio: Some(previous),
            installed,
            ..
        }) = self.latency_rebuild.as_mut()
            && let Some(crate::editor::audio_admission::StoppedPlaybackRestore::Published(required)) =
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
            Some(crate::editor::audio_admission::LatencyRebuild::Restoring {
                runtime_audio: Some(_),
                installed: false,
                ..
            })
        );
        // Ordinary native-source ACKs can arrive after their view candidate
        // was coalesced. Retire the VST3 panel gate on the exact installed
        // source and repaint so it can request the renewed controller page.
        if !awaiting_native_restore
            && let Some(crate::editor::audio_admission::StoppedPlaybackRestore::Published(required)) =
                self.stopped_playback_restore
            && let Some(source) = audio
                .control
                .status()
                .and_then(|status| status.native_effect_source())
            && source.audio_revision == required
            && audio.control.output_identity() == Some(source.output_identity)
            && source.context.project_generation == self.project_generation
            && self.state.as_ref().is_some_and(|state| {
                state.audio_authoring_revision(source.context) == Some(state.revision)
            })
        {
            self.stopped_playback_restore = None;
            visible_change = true;
        }
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

        // Submission metadata can lag or be coalesced independently of the
        // callback. During a held move, an unknown newer applied revision must
        // not leave the previously audible canonical graph reported as current.
        if self.clip_preview_may_apply
            && applied != 0
            && (self
                .audible_playback
                .as_ref()
                .is_some_and(|candidate| candidate.audio_revision != applied)
                || self
                    .applied_clip_preview
                    .is_some_and(|(revision, _)| revision != applied)
                || (self.audible_playback.is_none()
                    && self.applied_clip_preview.is_none()
                    && self.applied_project_revision.is_some()
                    && self
                        .audio_admission
                        .current()
                        .and_then(|current| current.audio_revision)
                        != Some(applied)))
            && !self
                .playback_candidates
                .iter()
                .any(|candidate| candidate.audio_revision == applied)
        {
            visible_change |= self.applied_project_revision.take().is_some();
            visible_change |= self.applied_clip_preview.take().is_some();
            visible_change |= self.audible_playback.take().is_some();
            visible_change |= self.audio_admission.preview_applied(applied);
        }

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
        if let Some(candidate) = audible.filter(|candidate| {
            candidate.clip_preview_identity.is_none() || candidate.audio_revision == applied
        }) {
            if candidate.clip_preview_identity.is_some() {
                self.applied_project_revision = None;
                visible_change |= self
                    .audio_admission
                    .preview_applied(candidate.audio_revision);
            }
            let source = if let Some(identity) = candidate.clip_preview_identity {
                applied_clip_candidate_source(
                    &candidate,
                    identity,
                    applied,
                    self.project_generation,
                    self.state.as_ref().map(|state| state.revision),
                    self.expected_clip_preview,
                )
            } else if candidate.seek_request_id.is_some_and(|request_id| {
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
            if candidate.clip_preview_identity.is_some() {
                let actual = (source == AppliedCandidateSource::Current)
                    .then(|| {
                        candidate
                            .clip_preview_identity
                            .map(|identity| (applied, identity))
                    })
                    .flatten();
                visible_change |= self.applied_clip_preview != actual;
                self.applied_clip_preview = actual;
                visible_change |= self.audible_playback.take().is_some();
            } else if source == AppliedCandidateSource::Current {
                visible_change |= self.applied_clip_preview.take().is_some();
                if candidate.tuning_preview_identity.is_some() {
                    self.tuning.broadcast.observe(
                        candidate.project_generation,
                        candidate.state.revision,
                        candidate.state.project.tuning(),
                    );
                }
                let applied_section = candidate.seek_request_id.is_some_and(|request| {
                    applied_section_source == Some((request, candidate.audio_revision))
                });
                if candidate.project_generation == self.project_generation && applied_section {
                    self.applied_project_revision = Some(candidate.state.revision);
                    visible_change |= self.audio_admission.applied(
                        crate::editor::audio_admission::ProjectRevision {
                            generation: candidate.project_generation,
                            revision: candidate.state.revision,
                        },
                        candidate.audio_revision,
                    );
                } else if candidate.project_generation == self.project_generation
                    && candidate.seek_request_id.is_none()
                    && candidate.clip_preview_identity.is_none()
                {
                    self.applied_project_revision = Some(candidate.state.revision);
                    if matches!(
                        self.stopped_playback_restore,
                        Some(crate::editor::audio_admission::StoppedPlaybackRestore::Published(required))
                            if candidate.audio_revision > required
                                || (candidate.audio_revision == required && !awaiting_native_restore)
                    ) {
                        self.stopped_playback_restore = None;
                        visible_change = true;
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
            install_current_applied_fallback(
                &mut self.recovery,
                self.pending_recovery_generation,
                self.state.as_ref(),
                &mut self.applied_project_revision,
                &mut self.audible_playback,
                source,
            );
        }
        visible_change |= clear_applied_submitted_audio_status(&mut self.status, applied);
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
            visible_change |= crate::editor::arrangement_seek::resolve_prepared_seek_terminal(
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
                            PlaybackCommand::Seek(_)
                                | PlaybackCommand::PlayFrom(_)
                                | PlaybackCommand::PlayFromWithNoteChase { .. }
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
                    Some(crate::editor::audio_admission::LatencyRebuild::Fading { ticket, .. })
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
                            clear_stale_playback_unavailable_error(&mut self.error);
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
                    Some(crate::editor::audio_admission::LatencyRebuild::Fading { .. })
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
                clear_stale_playback_unavailable_error(&mut self.error);
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
            self.deferred_toggle_play = None;
            self.deferred_metadata_seek = None;
            self.play_intent.clear();
            visible_change |= replace_status(&mut self.status, "audio device unavailable");
            let diagnostic = "audio device failed · restart audio to restore playback";
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
            self.deferred_toggle_play = None;
            self.deferred_metadata_seek = None;
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
        } else if audio
            .control
            .status()
            .is_some_and(|status| status.take_stream_read_failure())
        {
            visible_change |= replace_status(&mut self.status, "streamed media read failed");
            let diagnostic = "streamed media read failed · check original media and retry playback";
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
        if outcome_failure.is_some() {
            self.clear_recovery();
        }
        let pending_after = self
            .worker_requests_pending
            .saturating_add(self.pending_requests.len());
        visible_change |= pending_before != pending_after;
        visible_change |= self.follow_output_route();
        visible_change |= self.finish_audio_recovery_tick();
        visible_change |= self.tick_track_meters(Instant::now());
        visible_change |= crate::editor::device_panel::tick_hovered_meter(self);
        visible_change |= self.tick_track_performance(Instant::now());
        visible_change |= self.tick_latency_rebuild();
        visible_change |= self.tick_paused_republish();
        visible_change |= self.tick_sampler_edit_resume();
        visible_change |= self.tick_deferred_play();
        visible_change |= self.tick_deferred_toggle_play_and_metadata_seek();
        self.observe_audio_frame(frame, visible_change, Instant::now())
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn install_current_applied_fallback(
        &mut self,
        source: hex_audio::prepared::AppliedAudioSource,
    ) {
        install_current_applied_fallback(
            &mut self.recovery,
            self.pending_recovery_generation,
            self.state.as_ref(),
            &mut self.applied_project_revision,
            &mut self.audible_playback,
            source,
        );
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
        let focused_owner = crate::editor::device_panel::displayed_owner(self)
            .or_else(|| crate::editor::device_panel::selected_owner(self, state));
        match (focused_owner, identity.owner) {
            (
                Some(crate::editor::device_panel::DevicePanelOwner::Main),
                hex_engine::ProcessorChainOwner::Main,
            ) => state.project.main_devices().iter().any(|device| {
                device.id().value() == identity.device_id
                    && device.state().type_id() == identity.device_type
            }),
            (
                Some(crate::editor::device_panel::DevicePanelOwner::Track(track_id)),
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
                Some(crate::editor::device_panel::DevicePanelOwner::Return(return_id)),
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

    pub(super) fn retire_stale_spectrum_target(&mut self) -> bool {
        crate::editor::device_panel::reconcile_eq_spectrum(self)
    }

    pub(super) fn processor_audition_target_is_visible_and_current(
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
        let focused_owner = crate::editor::device_panel::selected_owner(self, state);
        match (focused_owner, target.identity.owner) {
            (
                Some(crate::editor::device_panel::DevicePanelOwner::Main),
                hex_engine::ProcessorChainOwner::Main,
            ) => state.project.main_devices().iter().any(|device| {
                device.id().value() == target.identity.device_id
                    && device.state().type_id() == target.identity.device_type
            }),
            (
                Some(crate::editor::device_panel::DevicePanelOwner::Track(track_id)),
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
                Some(crate::editor::device_panel::DevicePanelOwner::Return(return_id)),
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

    pub(super) fn retire_stale_processor_audition_target(&mut self) -> bool {
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
            crate::editor::device_panel::clear_eq8_audition_gesture_flags(self);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_non_open_request_respects_open_and_close_prompt_guards() {
        assert!(allows_non_open_request(false, false, false));
        assert!(!allows_non_open_request(false, true, false));
        assert!(!allows_non_open_request(true, false, false));
        assert!(allows_non_open_request(true, false, true));
        assert!(!allows_non_open_request(true, true, true));
    }
}
