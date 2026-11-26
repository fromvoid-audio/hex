//! Transport, session launch, metronome, and stop dispatch.
use super::*;

impl EditorView {
    pub(in crate::editor) fn queue_audio_command(&mut self, command: PlaybackCommand) {
        self.queue_playback_action(PlaybackAction::Transport(command), None);
    }

    pub(in crate::editor) fn queue_session_command(
        &mut self,
        command: hex_engine::SessionPlaybackCommand,
        revision: u64,
    ) {
        self.settings.playback.pending = None;
        let Some(audio) = self.audio.as_ref() else {
            self.error = Some("session playback requires an audio output".into());
            return;
        };
        if self.audio_pub.recovery.generation().is_some()
            || self.audio_pub.stopped_playback_restore.is_some()
            || self.persistence.opening_project
            || self.persistence.close_prompt_open
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
        if self.audio_pub.recovery.generation().is_some()
            || self.audio_pub.stopped_playback_restore.is_some()
            || self.persistence.opening_project
            || self.persistence.close_prompt_open
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
            self.audio_pub.pending_audio_commands.len(),
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
        self.audio_pub.pending_audio_commands.push_back(pending);
        self.session_grid.pending_session_ticket = None;
        self.play_intent.admit(token, true);
        self.status = "session launch requested · waiting for audio callback".into();
    }

    pub(in crate::editor) fn metronome_enabled(&self) -> bool {
        self.audio_status()
            .is_some_and(|status| status.metronome_enabled.load(Acquire))
    }

    pub(in crate::editor) fn metronome_pending(&self) -> bool {
        self.audio_pub
            .pending_metronome_ticket
            .is_some_and(|ticket| {
                if self
                    .audio
                    .as_ref()
                    .is_some_and(|audio| audio.control.has_action_outcomes())
                {
                    return true;
                }
                self.audio_status().is_some_and(|status| {
                    audio_command_prefix(status, self.audio_pub.document_audio_mode) < ticket
                })
            })
            || self
                .audio_pub
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
            || !self.audio_pub.pending_audio_commands.is_empty()
            || self.audio_pub.stopped_playback_restore.is_some()
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
        self.audio_pub.latency_rebuild = None;
        self.device_panel.sampler_edit_resume = None;
        self.audio_pub.deferred_play = None;
        self.audio_pub.deferred_toggle_play = None;
        self.audio_pub.deferred_metadata_seek = None;
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
        self.audio_pub.pending_seek_ticket = Some(ticket);
        clear_stale_playback_unavailable_error(&mut self.error);
        #[cfg(test)]
        {
            self.test_probes.last_transport_command = Some(command);
        }
    }

    /// Seek only if the callback still owns the observed source. Never place a
    /// rejected fenced action on the ordinary, generation-only UI queue.
    pub(in crate::editor) fn queue_applied_source_seek(
        &mut self,
        frame: u64,
        source: hex_audio::prepared::AppliedAudioSource,
    ) {
        self.settings.playback.pending = None;
        self.wake_poll();
        if !self.audio_pub.pending_audio_commands.is_empty()
            || self.audio_pub.stopped_playback_restore.is_some()
            || self.seek_pending()
        {
            self.error = Some("applying edit · wait for audio".into());
            return;
        }
        self.audio_pub.latency_rebuild = None;
        self.device_panel.sampler_edit_resume = None;
        self.audio_pub.deferred_play = None;
        self.audio_pub.deferred_toggle_play = None;
        self.audio_pub.deferred_metadata_seek = None;
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
        self.audio_pub.pending_seek_ticket = Some(ticket);
        clear_stale_playback_unavailable_error(&mut self.error);
        #[cfg(test)]
        {
            self.test_probes.last_transport_command = Some(PlaybackCommand::Seek(frame));
        }
    }

    pub(in super::super) fn queue_playback_action(
        &mut self,
        action: PlaybackAction,
        revision: Option<u64>,
    ) {
        if !matches!(action, PlaybackAction::SetMetronome(_)) {
            self.settings.playback.pending = None;
        }
        self.wake_poll();
        if !matches!(action, PlaybackAction::SetMetronome(_)) {
            self.audio_pub.latency_rebuild = None;
            self.device_panel.sampler_edit_resume = None;
            self.audio_pub.deferred_play = None;
        }
        if self.section_install_pending()
            && (intent_desired_playing(action) == Some(true)
                || matches!(action, PlaybackAction::Transport(PlaybackCommand::Seek(_))))
        {
            self.error = Some("applying edit · wait for audio".into());
            return;
        }
        if intent_desired_playing(action) == Some(true)
            && (self.audio_pub.stopped_playback_restore.is_some()
                || self.stopped_plugin_parameter_terminal_pending())
        {
            self.audio_pub.deferred_play = Some((action, revision, self.project_generation));
            let message = if self.audio_pub.stopped_playback_restore.is_some() {
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
            ) && self.audio_pub.recovery.generation().is_some()
            {
                // Recovery already owns this start. Keep its explicit policy
                // in the existing generation-bound deferred action slot.
                self.audio_pub.deferred_play = Some((action, revision, self.project_generation));
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
            self.audio_pub.pending_audio_commands.len(),
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
        if self.audio_pub.pending_audio_commands.is_empty() {
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
                        self.audio_pub.pending_metronome_ticket = Some(ticket);
                    }
                    self.admit_play_intent(pending, desired_playing, Some(ticket));
                    if matches!(
                        action,
                        PlaybackAction::Transport(
                            PlaybackCommand::PlayFrom(_)
                                | PlaybackCommand::PlayFromWithNoteChase { .. }
                        )
                    ) {
                        self.audio_pub.pending_seek_ticket = Some(ticket);
                    }
                    if matches!(
                        action,
                        PlaybackAction::Session(_) | PlaybackAction::LaunchAndPlay(_)
                    ) {
                        self.session_grid.pending_session_ticket =
                            Some((ticket, self.project_generation));
                        replace_status(
                            &mut self.status,
                            "session request queued · waiting for audio callback",
                        );
                    }
                }
                Some(Err(rejected)) => {
                    if self.audio_pub.pending_audio_commands.len() < UI_REQUEST_CAPACITY {
                        self.audio_pub
                            .pending_audio_commands
                            .push_back(PendingPlaybackAction {
                                action: rejected,
                                ..pending
                            });
                        if matches!(
                            rejected,
                            PlaybackAction::Session(_) | PlaybackAction::LaunchAndPlay(_)
                        ) {
                            self.session_grid.pending_session_ticket = None;
                        }
                        self.admit_play_intent(pending, desired_playing, None);
                        if matches!(
                            rejected,
                            PlaybackAction::Transport(
                                PlaybackCommand::PlayFrom(_)
                                    | PlaybackCommand::PlayFromWithNoteChase { .. }
                            )
                        ) {
                            self.audio_pub.pending_seek_ticket = None;
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
            self.audio_pub.pending_audio_commands.push_back(pending);
            if matches!(
                action,
                PlaybackAction::Transport(
                    PlaybackCommand::PlayFrom(_) | PlaybackCommand::PlayFromWithNoteChase { .. }
                )
            ) {
                self.audio_pub.pending_seek_ticket = None;
            }
            if matches!(
                action,
                PlaybackAction::Session(_) | PlaybackAction::LaunchAndPlay(_)
            ) {
                self.session_grid.pending_session_ticket = None;
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

    pub(in super::super) fn admit_play_intent(
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

    pub(in super::super) fn report_intent_exhausted(&mut self) {
        self.error = Some("playback intent tokens exhausted · restart hex to continue".into());
    }

    pub(in crate::editor) fn stop(&mut self) {
        self.settings.playback.pending = None;
        self.interaction.escape_stop_armed = false;
        if self.finish_audio_recording_before_stop() {
            return;
        }
        self.stop_transport(true);
    }

    pub(in crate::editor) fn stop_at_beginning(&mut self) {
        self.interaction.play_start_at_beginning = true;
        self.stop();
    }

    /// Only an Escape that reaches the transport arms the next keyboard stop.
    /// Use desired state so the second stop can reset before callback feedback.
    pub(in crate::editor) fn stop_from_escape(&mut self) {
        let playing = self
            .audio_status()
            .is_some_and(|status| status.playing.load(Acquire));
        if self.interaction.escape_stop_armed && !self.desired_playing(playing) {
            self.stop_at_beginning();
        } else {
            self.stop();
        }
        self.interaction.escape_stop_armed = true;
    }

    pub(in super::super) fn stop_transport(&mut self, stop_output: bool) {
        self.settings.playback.pending = None;
        self.audio_pub.paused_republish = None;
        self.cancel_native_automation_touch();
        use crate::editor::audio_admission::LatencyRebuild;
        let native_restore = matches!(
            self.audio_pub.latency_rebuild.take(),
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
        self.midi_editor.step.cancel_pending();
        // Stop explicitly discards voice histories. A previously refused edit
        // or restoration can now install the canonical sound without inheriting
        // an incompatible preview's history. Keep construction on the worker.
        let can_restore = stop_output
            && self.audio_pub.stopped_playback_restore.is_none()
            && !self.edit_pending()
            && self.device_panel.gesture.snapshot().is_none()
            && self.settings.tuning.client.snapshot().is_none()
            && self.audio_pub.recovery.generation().is_none();
        let restart_degraded = can_restore && self.has_output_fault_requiring_recovery();
        let restore = can_restore
            && !restart_degraded
            && self.audio.as_ref().is_some_and(|audio| {
                self.state.as_ref().is_some_and(|state| {
                    native_restore
                        // Priority Stop ends the transport epochs even when
                        // the same project revision is already audible.
                        || self.has_resident_vst3()
                        || self.audio_pub.applied_project_revision != Some(state.revision)
                        || self.audio_pub.audible_playback.as_ref().is_none_or(|audible| {
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
            if self.audio_pub.recovery.generation().is_some() {
                return self.has_resident_vst3() && status.applied_revision.load(Acquire) == 0;
            }
            matches!(
                self.audio_pub.stopped_playback_restore,
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
        self.audio_pub.offline_cursor = 0;
        self.audio_pub.recovery.stop();
        self.audio_pub.pending_audio_commands.clear();
        self.audio_pub.pending_metronome_ticket = None;
        self.audio_pub.pending_seek_ticket = None;
        self.session_grid.pending_session_ticket = None;
        self.play_intent.priority_stop();
        self.audio_pub.deferred_play = None;
        self.audio_pub.deferred_toggle_play = None;
        self.audio_pub.deferred_metadata_seek = None;
        self.status = if self.interaction.play_start_at_beginning {
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
            && self.audio_pub.stopped_playback_restore.is_none()
            && self.audio_pub.recovery.generation().is_none()
            && self.has_resident_vst3()
            && let Some(output) = self
                .audio
                .as_ref()
                .and_then(|audio| audio.control.output_identity())
        {
            // Read-only inventory and in-flight edit terminals can overlap Stop.
            // Let them drain before renewing the exact stopped document/output.
            self.audio_pub.stopped_playback_restore = Some(
                crate::editor::audio_admission::StoppedPlaybackRestore::WaitingForEdits {
                    generation: self.project_generation,
                    output,
                },
            );
        }
    }
}
