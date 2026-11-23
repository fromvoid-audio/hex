//! Ordered session worker event ingestion and canonical state/receipt reconciliation.
use super::*;

impl EditorView {
    pub(in crate::editor) fn receive(&mut self, mut event: Event) -> Option<std::path::PathBuf> {
        // Read acknowledgements must not retire any edit's ordered completion
        // custody, even if the worker consumed the query while an edit queued.
        if event.plugin_parameter_observation {
            event.request_complete = false;
        }
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
            .track_switch_observation
            .as_ref()
            .is_some_and(|view| event.project_replaced || event.state.revision >= view.revision)
        {
            self.track_switch_observation = None;
        }
        if event.request_complete && event.error.is_some() {
            self.cancel_new_preparation();
        }
        let clip_cut_completed = self.observe_clip_cuts(&event);
        self.document_format_upgrade_required = event.document_format_upgrade_required;
        self.recovery_failure = event.recovery_failure.clone();
        self.observe_automation_close(&event);
        let sampler_edit_complete = event.sampler_edit_complete;
        if let Some(offer) = event.device_insert_confirmation.take() {
            self.install_device_confirmation(offer);
        }
        self.plugin_effect_complete(&event);
        self.native_automation_take_completion(&event);
        self.observe_automation_take_recovery(&event);
        if let Some(result) = event.plugin_parameters.take() {
            crate::editor::device_panel::vst3_parameters::install_result(
                self,
                result,
                event.state.revision,
            );
        }
        if let Some(result) = event.plugin_parameter_conversion.take() {
            crate::editor::device_panel::vst3_parameters::install_conversion_result(
                self,
                result,
                event.state.revision,
            );
        }
        let sampler_load_complete = event.sampler_load_complete;
        let sampler_load_succeeded = event.error.is_none();
        let gain_control_preview = self.audio_clip_controls.has_gain_preview();
        let play_start_control_preview = self.audio_clip_controls.has_play_start_preview();
        if self
            .audio_clip_controls
            .observe(self.project_generation, &event)
        {
            if gain_control_preview {
                self.set_held_gain_audition(None);
                self.service.retire_arrangement_gain_audition();
            }
            if play_start_control_preview {
                self.sync_held_play_start_audition();
                self.service.retire_arrangement_play_start_audition();
            }
            self.retire_sample_bpm_control();
        }
        let audio_placement_selection = self.audio_events_request_complete(&event);
        self.clip_loop_request_complete(&event);
        self.bypass_request_complete(&event);
        self.mixer_edit_request_complete(&event);
        self.modulation_depth_complete(&event);
        let clip_drop = self.clip_move_request_complete(&event);
        self.clip_paint_request_complete(&event);
        self.truncated_import
            .observe(self.project_generation, &event);
        self.section_request_complete(&event);
        self.project_rate_settings
            .observe(self.project_generation, &event);
        self.track_reorder_complete(&event);
        self.sample_drop_complete(&event);
        let midi_edit_audition = self.midi_drag_request_complete(&event);
        self.advance_midi_origin_view(&event);
        self.fade_request_complete(&event);
        if event.project_replaced {
            self.expected_clip_preview = None;
            self.applied_clip_preview = None;
            self.clip_preview_may_apply = false;
            self.pending_requests
                .retain(|request| self.locators.queued_add_frame(request).is_none());
        }
        self.locators.observe(self.project_generation, &event);
        self.clip_gain_complete(&event);
        self.clip_activation_complete(&event);
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
        self.observe_empty_track(&event);
        if event.project_replaced {
            self.time_selection_key_held = false;
            self.section_loop_key = Default::default();
            self.clear_device_confirmation_on_project_replace();
            self.pending_plugin_effect.take();
            self.failed_plugin_proposal.take();
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
            self.clear_recovery();
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
            self.deferred_toggle_play = None;
            self.deferred_metadata_seek = None;
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
            self.pending_browser_insertions.clear();
        }
        if event.request_complete
            && event.computer_keyboard_complete.is_none()
            && event.error.is_some()
            && event.playback.is_none()
        {
            self.gain_recorder.arm_failed();
        }
        let mut recorded_audio_failed_path = None;
        if event
            .audio_batch_complete
            .as_ref()
            .is_some_and(|completed| {
                self.audio_batch
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(active, completed))
            })
        {
            if event.error.is_some() {
                recorded_audio_failed_path = self.audio_recorder.pending_commit_path.take();
            } else {
                self.audio_recorder.pending_commit_path = None;
            }
            self.audio_batch = None;
            self.audio_batch_progress = None;
        }
        self.reconcile_group_selection(&event);
        let bounce_drop_cursor = if event.bounce_complete.as_ref().is_some_and(|completed| {
            self.bounce_control
                .as_ref()
                .is_some_and(|active| active.same_job(completed))
        }) {
            self.bounce_control = None;
            self.pending_bounce_drop
                .take()
                .and_then(|p| p.complete(&event))
        } else {
            None
        };
        if event.project_replaced {
            self.abandon_bounce();
        }
        // A bounce drop's ghost retires with its job's terminal, in the same
        // receive that installs the adopted (or unchanged, refused) state.
        if self.bounce_control.is_none() {
            self.pending_bounce_drop = None;
        }
        if event.save_complete {
            self.saving_project = false;
        }
        if event.request_complete && !event.plugin_parameter_observation {
            self.worker_requests_pending = self.worker_requests_pending.saturating_sub(1);
        }
        if self
            .browser_insertion_flight
            .as_mut()
            .is_some_and(|flight| flight.finished(self.project_generation, &event))
        {
            self.browser_insertion_flight = None;
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
                let audition_ok = event.error.is_none();
                if let Some(error) = event.error {
                    self.error = Some(error.into());
                    self.cancel_computer_keyboard();
                    self.computer_keyboard.suspended = true;
                }
                if audition_ok {
                    let _ = self.try_trigger_midi_audition_now();
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
        self.observe_catalog_barrier(&event);
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
        let time_range_was_pending = self.pending_time_range_move.is_some();
        let recovery_before_time_completion = self.recovery.generation();
        let time_duplicate_repeats = if event.project_replaced {
            self.time_range_move_gesture = None;
            self.pending_time_range_move = None;
            self.ahead_time_range_move = None;
            0
        } else {
            self.complete_time_duplicate_pair(&event)
        };
        let recovering_refused_time_copy = event.range_copy_source_unavailable.is_some()
            && recovery_before_time_completion.is_none()
            && self.recovery.deferred_edit.as_ref().is_some_and(|edit| {
                edit.kind == crate::editor::recovery_state::DeferredEditKind::TimeDuplicate
            });
        if !event.project_replaced {
            crate::editor::loop_rail::cancel_stale(
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
        self.complete_device_duplicate(&event);
        if event.project_replaced
            || self
                .committed_paste_view
                .as_ref()
                .is_some_and(|view| event.state.revision >= view.revision)
        {
            self.committed_paste_view = None;
        }
        if event.project_replaced
            || (time_range_was_pending
                && self.pending_time_range_move.is_none()
                && event.request_complete
                && !event.plugin_parameter_observation)
        {
            self.committed_time_range_copy_view = None;
        }
        self.state = Some(event.state);
        if let Some(state) = &self.state {
            self.audio_recorder
                .reconcile_audio_arm(&state.project, event.project_replaced);
        }
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
            self.playback_settings.pending = None;
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
                        let message = crate::editor::audio_unavailable::unavailable_message(&error);
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
            self.refuse_device_gesture_barriers("device load canceled · project replaced");
            self.edit_cursor = None;
            self.clip_drop_cursor_epoch = None;
            self.locked_play_start = None;
            self.locked_play_start_drag = None;
            self.bottom_play_start_marker_bounds.set(None);
            self.play_start_at_beginning = false;
            self.escape_stop_armed = false;
            self.loop_start_over_clip = None;
            self.selection_loop_start = None;
            self.clip_paint_gesture = None;
            self.cancel_section_pointer();
            self.time_selection = None;
            self.time_selection_project_scope = false;
            self.time_selection_gesture = None;
            self.time_range_move_gesture = None;
            self.pending_time_range_move = None;
            self.ahead_time_range_move = None;
            self.suppress_cursor_click = false;
            self.applied_project_revision = None;
            self.playback_candidates
                .retain(|candidate| candidate.seek_request_id.is_some());
            self.audible_playback = None;
            self.pending_playback = None;
            self.pending_resident_publication = None;
            self.clear_route_telemetry();
            self.degraded_plugins = Default::default();
            self.plugin_editor_windows.clear();
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
            self.track_selection = crate::editor::selection::TrackSelection::default();
            self.selected_clip = None;
            self.selected_scene = None;
            self.selected_cell = None;
            self.session_copy_source = None;
            self.session_track_start = 0;
            self.midi_selection.clear();
            self.arrangement_focus.reset();
            self.reconcile_arrangement_focus();
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
                let previous_primary = self.selected_clip;
                (self.selected_track, self.selected_clip) =
                    selection::reconcile(&state.project, self.selected_track, previous_primary);
                if clip_cut_completed {
                    let survivor = self.surviving_cut_selection(&state.project, state.revision);
                    if previous_primary.is_some() && self.selected_clip != previous_primary {
                        (self.selected_track, self.selected_clip) =
                            selection::reconcile(&state.project, self.selected_track, survivor);
                    }
                }
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
                        .min(crate::editor::midi_scene::zoom_limit(source_length))
                        .max(1),
                );
                self.piano_beat_start = crate::editor::midi_scene::clamp_start(
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
            .canonical_revision(crate::editor::audio_admission::ProjectRevision {
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
                    && self.mode == crate::editor::EditorMode::Session
                    && !self.opening_project
                    && !self.close_prompt_open
                    && self.recovery.generation().is_none()
                    && self.selected_scene == Some(intent.source_scene)
                    && self.selected_cell.is_none()
                {
                    self.select_session_scene(scene);
                }
            } else if self.mode == crate::editor::EditorMode::Session {
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
        } else if let Some(hex_project::Created::Track(track)) = event.created
            && clip_drop.as_ref().is_none_or(|(cursor, moved)| {
                moved.is_none()
                    || cursor.track != track
                    || self.selection_intent_epoch == Some(cursor.epoch)
            })
        {
            let moved_selection = self.selected_track == Some(track)
                && clip_drop
                    .as_ref()
                    .is_some_and(|(cursor, moved)| cursor.track == track && moved.is_some());
            self.focused_return = None;
            self.device_panel.main_selected = None;
            if let Some(state) = &self.state {
                self.track_selection
                    .select(&state.project, track, false, false);
            }
            self.midi_note_gesture = None;
            self.midi_group_gesture = None;
            self.selected_track = Some(track);
            if !moved_selection {
                self.selected_clip = None;
                self.midi_selection.clear();
            }
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
        if event.active_device_gesture.is_none()
            && let Some(ack) = event.gesture_ack.as_ref()
            && self.device_gesture.snapshot().is_some_and(|snapshot| {
                snapshot.key == ack.key
                    && matches!(
                        snapshot.target,
                        hex_session::device_gesture::GestureTarget::PluginParameter { .. }
                    )
            })
        {
            self.probe_failed_plugin_automation();
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
        self.observe_vst3_automation_arm(
            event.project_replaced,
            event.gesture_ack.as_ref(),
            event.active_device_gesture.is_some(),
            revision,
        );
        if crate::editor::device_gesture::actor_retired_gesture(
            event.project_replaced,
            event.active_device_gesture.is_some(),
            event.gesture_ack.as_ref(),
            self.device_gesture.snapshot().map(|snapshot| snapshot.key),
        ) && self.device_gesture.snapshot().is_some()
        {
            self.refuse_deferred_tuning_writes();
            if crate::editor::device_gesture::retire_failed_tuning_library_barriers(
                &mut self.device_gesture_barriers,
            ) {
                self.tuning.library_pending = None;
            }
            if let Some(snapshot) = self.device_gesture.snapshot() {
                self.bind_tuning_library_device_terminal(snapshot.key, false);
            }
            if event.project_replaced {
                self.refuse_device_gesture_barriers("device load canceled · project replaced");
            } else {
                self.refuse_catalog_barriers("device load canceled · device edit was retired");
            }
            self.device_gesture.reset();
            crate::editor::device_gesture::retire_deferred(
                &mut self.pending_requests,
                &mut self.device_gesture_barriers,
                event.project_replaced,
            );
        }
        crate::editor::device_panel::eq_scroll_after_worker_event(
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
                    Some(crate::editor::audio_admission::StoppedPlaybackRestore::Ready);
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
        } else if event.save_complete && event.error.is_some() && !save_confirmation_required {
            self.status = "project was not saved".into();
        }
        if recovering_refused_time_copy {
            self.status = "restarting audio · time will be duplicated when ready".into();
        }
        if let Some(error) = event.error.filter(|_| !recovering_refused_time_copy) {
            let mut displayed: crate::editor::display_error::DisplayError = event
                .audio_batch_failure
                .map_or_else(|| error.into(), Into::into);
            if let Some(path) = recorded_audio_failed_path {
                displayed = displayed.with_context(&format!(
                    "take retained at {}",
                    hex_session::service::text::verbatim(path.display())
                ));
            }
            self.error = Some(displayed);
        } else if let Some(error) = stopped_restore_error {
            self.error = Some(error.into());
        }
        if let Some(message) = playback_capability {
            self.session_capability_message = message.clone();
            if let Some(message) = message {
                self.error = Some(message.into());
            }
        }
        if let Some(reason) = self.take_catalog_barrier_error() {
            self.error = Some(reason.into());
        }
        self.finish_sampler_parameter(sampler_edit_complete);
        self.first_sampler_load_complete(sampler_load_complete, sampler_load_succeeded);
        self.reconcile_publication_observation();
        if event.request_complete
            && !event.gesture_request
            && !event.tuning_gesture_request
            && self.mode == crate::editor::EditorMode::Session
            && self.selected_cell.is_none()
            && self.selected_scene.is_some()
        {
            self.reveal_session_position(None, self.selected_scene);
        }
        // Canonical state, generation, gain/loop display extent and selection
        // have now been installed. Normalize only the locally retired owner.
        self.normalize_completed_trim_viewport(retired_trim);
        use crate::editor::midi_pending_drag::EditAudition;
        let clip = self.detail_clip_id();
        let audition_created =
            matches!(midi_edit_audition, Some(EditAudition::Created(id)) if clip == Some(id));
        self.apply_created_note_selection(created_note_selection, audition_created);
        if matches!(midi_edit_audition, Some(EditAudition::Moved(id)) if clip == Some(id)) {
            self.audition_midi_selection();
        }
        self.apply_completed_midi_step(midi_step_complete);
        if let Some((cursor, moved_clip)) =
            clip_drop.or(bounce_drop_cursor.map(|cursor| (cursor, None)))
        {
            self.install_drop_cursor(cursor, moved_clip);
        }
        if let Some(path) = &successfully_opened_path {
            self.check_leftover_recovery(Some(path.clone()));
        }
        self.start_ahead_time_duplicate();
        self.continue_time_duplicate(time_duplicate_repeats);
        successfully_opened_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_status_only_reports_change_when_text_differs() {
        let mut status = String::from("ready");
        assert!(!replace_status(&mut status, "ready"));
        assert!(replace_status(&mut status, "saved"));
        assert_eq!(status, "saved");
    }
}
