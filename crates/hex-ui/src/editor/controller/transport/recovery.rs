//! Deferred play, paused republish, latency rebuild, and stopped VST3 restore.
use super::*;

impl EditorView {
    /// Rebuild only a current, rejected canonical graph. A fresh graph drops
    /// effect history, so retain the user's transport intent and never restart
    /// a superseded project, output or explicit transport action.
    /// A streamed document's sampler edit replaces its output only while
    /// stopped, so a playing edit stops first and resumes here at the same
    /// frame (audio restarts at the cursor). Resident documents apply live.
    pub(in super::super) fn sampler_edit_resume_frame(&self, request: &Request) -> Option<u64> {
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

    pub(in crate::editor) fn tick_sampler_edit_resume(&mut self) -> bool {
        let Some(frame) = self.device_panel.sampler_edit_resume else {
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
        self.device_panel.sampler_edit_resume = None;
        self.queue_audio_command(PlaybackCommand::PlayFrom(frame));
        true
    }

    /// Starting the callback with a stopped VST3 terminal in flight can
    /// invalidate its transport fence before the original source commits.
    pub(in super::super) fn stopped_plugin_parameter_terminal_pending(&self) -> bool {
        self.device_panel.gesture.has_pending_terminal()
            && self.device_panel.gesture.snapshot().is_some_and(|gesture| {
                matches!(
                    gesture.target,
                    hex_session::device_gesture::GestureTarget::PluginParameter { .. }
                )
            })
            && self
                .audio_status()
                .is_some_and(|status| !status.playing.load(Acquire))
    }

    /// Start an explicit Play once the stopped restore, VST3 edit or audio
    /// recovery settles. A replaced project drops the intent.
    pub(in crate::editor) fn tick_deferred_play(&mut self) -> bool {
        if self.stopped_playback_restore.is_some()
            || self.recovery.generation().is_some()
            || self.pending_loop_region().is_some()
            || self.track_gain_controls.has_committed_recovery()
            || self.stopped_plugin_parameter_terminal_pending()
            || self.paused_republish_blocks_play()
        {
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

    pub(in crate::editor) fn awaiting_applied_playback_metadata(&self) -> bool {
        self.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && self.audible_playback.as_ref().is_some_and(|candidate| {
                candidate.project_generation == self.project_generation
                    && candidate.worker_requires_preroll.is_none()
                    && candidate.prepared_revision.is_none()
            })
            && self.normal_start_source().is_some()
    }

    pub(in crate::editor) fn can_defer_toggle_play_for_inflight_publication(&self) -> bool {
        if self.persistence.opening_project
            || self.persistence.close_prompt_open
            || self.state.is_none()
        {
            return false;
        }
        let Some(audio) = self.audio.as_ref() else {
            return false;
        };
        let Some(status) = audio.control.status() else {
            return false;
        };
        if status.stopped_edit_pending()
            || status.device_fault.load(Acquire)
            || status.render_fault.load(Acquire)
            || status.shutdown_complete.load(Acquire)
        {
            return false;
        }
        self.edit_pending()
            || self.pending_playback.is_some()
            || self.pending_resident_publication.is_some()
            || !self.playback_candidates.is_empty()
            || self.awaiting_applied_playback_metadata()
            || self.audio_admission.current().is_some_and(|admission| {
                admission.project.generation == self.project_generation
                    && admission.state == crate::editor::audio_admission::AdmissionState::Pending
            })
    }

    pub(in crate::editor) fn tick_deferred_toggle_play_and_metadata_seek(&mut self) -> bool {
        let mut changed = false;
        if let Some((frame, generation)) = self.deferred_metadata_seek {
            if generation != self.project_generation
                || self.persistence.opening_project
                || self.persistence.close_prompt_open
            {
                self.deferred_metadata_seek = None;
                changed = true;
            } else if self.current_seek_requires_preroll().is_some() {
                self.deferred_metadata_seek = None;
                self.request_seek(frame);
                changed = true;
            } else if !self.awaiting_applied_playback_metadata() {
                self.deferred_metadata_seek = None;
                if self.error.is_none() {
                    self.error = Some("playback source is unavailable · cannot seek safely".into());
                }
                changed = true;
            }
        }
        self.tick_deferred_toggle_play() || changed
    }

    pub(super) fn tick_deferred_toggle_play(&mut self) -> bool {
        let Some(intent) = self.deferred_toggle_play else {
            return false;
        };
        if intent.generation != self.project_generation
            || self.persistence.opening_project
            || self.persistence.close_prompt_open
        {
            self.deferred_toggle_play = None;
            return true;
        }
        let output = self
            .audio
            .as_ref()
            .and_then(|audio| audio.control.output_identity());
        let mut source = self.normal_start_source();
        match intent.target {
            TogglePlayTarget::DeviceProposal { output: expected } => {
                if output == Some(expected) && self.device_panel.pending_plugin_effect.is_some() {
                    return false;
                }
                self.deferred_toggle_play = None;
                return true;
            }
            TogglePlayTarget::CommittedDevice {
                output: expected,
                revision,
                audio_revision,
            } => {
                if output != Some(expected)
                    || self
                        .state
                        .as_ref()
                        .is_none_or(|state| state.revision != revision)
                {
                    self.deferred_toggle_play = None;
                    return true;
                }
                source = source.filter(|source| {
                    source.context.editor_revision == revision
                        && audio_revision.is_none_or(|revision| source.audio_revision == revision)
                });
            }
            TogglePlayTarget::Publication => {}
        }
        if let Some(source) = source
            && (self.document_audio_mode == hex_session::DocumentAudioMode::Resident
                || self.current_seek_requires_preroll().is_some())
        {
            if matches!(intent.target, TogglePlayTarget::Publication) {
                self.queue_normal_play(intent.frame);
            } else {
                self.queue_normal_start(crate::editor::playback_settings::Start::Source {
                    frame: intent.frame,
                    source,
                    allow_playing: false,
                });
            }
            return true;
        }
        if self.can_defer_toggle_play_for_inflight_publication() {
            return false;
        }
        self.deferred_toggle_play = None;
        if self.error.is_none() {
            self.error = Some("playback source is unavailable · cannot seek safely".into());
        }
        true
    }

    pub(in crate::editor) fn paused_republish_waiting_for_current(&self) -> bool {
        matches!(
            self.paused_republish,
            Some(crate::editor::audio_admission::PausedRepublish::WaitingForPause { project, output, .. })
                if project.generation == self.project_generation
                    && self
                        .state
                        .as_ref()
                        .is_some_and(|state| state.revision == project.revision)
                    && self
                        .audio
                        .as_ref()
                        .and_then(|audio| audio.control.output_identity())
                        == Some(output)
        )
    }

    pub(in crate::editor) fn paused_republish_blocks_play(&self) -> bool {
        let current = |project: crate::editor::audio_admission::ProjectRevision, output: u64| {
            project.generation == self.project_generation
                && self
                    .state
                    .as_ref()
                    .is_some_and(|state| state.revision == project.revision)
                && self
                    .audio
                    .as_ref()
                    .and_then(|audio| audio.control.output_identity())
                    == Some(output)
        };
        match self.paused_republish {
            Some(crate::editor::audio_admission::PausedRepublish::Submitted {
                project,
                output,
                ..
            }) => current(project, output),
            Some(crate::editor::audio_admission::PausedRepublish::WaitingForPause {
                project,
                output,
                ..
            }) => {
                let actual_playing = self
                    .audio_status()
                    .is_some_and(|status| status.playing.load(Acquire));
                current(project, output) && !self.play_intent.desired_playing(actual_playing)
            }
            _ => false,
        }
    }

    pub(in crate::editor) fn paused_republish_exhausted_for_current(&self) -> bool {
        matches!(
            self.paused_republish,
            Some(crate::editor::audio_admission::PausedRepublish::Exhausted { project, output })
                if project.generation == self.project_generation
                    && self
                        .state
                        .as_ref()
                        .is_some_and(|state| state.revision == project.revision)
                    && self
                        .audio
                        .as_ref()
                        .and_then(|audio| audio.control.output_identity())
                        == Some(output)
        )
    }

    pub(in crate::editor) fn tick_paused_republish(&mut self) -> bool {
        use crate::editor::audio_admission::{AdmissionState, PausedRepublish};
        let Some(republish) = self.paused_republish else {
            return false;
        };
        if let PausedRepublish::Unresolved {
            generation,
            output,
            audio_revision,
            reason,
        } = republish
        {
            if generation != self.project_generation
                || self
                    .audio
                    .as_ref()
                    .and_then(|audio| audio.control.output_identity())
                    != Some(output)
            {
                self.paused_republish = None;
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
                || candidate.clip_preview_identity.is_some()
                || self
                    .state
                    .as_ref()
                    .is_none_or(|state| state.revision != candidate.state.revision)
            {
                self.paused_republish = None;
                return false;
            }
            let project = crate::editor::audio_admission::ProjectRevision {
                generation,
                revision: candidate.state.revision,
            };
            self.audio_admission
                .rejected(project, audio_revision, reason.message());
            return record_paused_republish_rejection(
                &mut self.paused_republish,
                &mut self.deferred_play,
                &mut self.status,
                &mut self.error,
                self.project_generation,
                self.state.as_deref(),
                &self.audio_admission,
                project,
                Some(output),
                audio_revision,
                reason,
            );
        }
        let (project, output) = match republish {
            PausedRepublish::WaitingForPause {
                project, output, ..
            }
            | PausedRepublish::Submitted {
                project, output, ..
            }
            | PausedRepublish::Exhausted { project, output } => (project, output),
            PausedRepublish::Unresolved { .. } => return false,
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
            if matches!(republish, PausedRepublish::Submitted { .. }) {
                self.deferred_play = None;
            }
            self.paused_republish = None;
            return false;
        }
        if self.applied_project_revision == Some(project.revision) {
            self.paused_republish = None;
            return true;
        }
        match republish {
            PausedRepublish::Unresolved { .. } | PausedRepublish::Exhausted { .. } => false,
            PausedRepublish::WaitingForPause { audio_revision, .. } => {
                if self.edit_pending()
                    || self.persistence.opening_project
                    || self.persistence.close_prompt_open
                    || self.persistence.save_dialog_open
                    || self.device_panel.gesture.snapshot().is_some()
                    || self.settings.tuning.client.snapshot().is_some()
                    || self.midi_editor.recorder.blocks_edits()
                    || self.gain_recorder.blocks_edits()
                    || self.computer_keyboard.input.is_some()
                    || !self.pending_audio_commands.is_empty()
                    || self.session_grid.pending_session_ticket.is_some()
                    || self.pending_seek_ticket.is_some()
                    || self.recovery.generation().is_some()
                    || self.stopped_playback_restore.is_some()
                    || self.section_launch_waiting()
                    || self.prepared_seek.is_some()
                    || self.latency_rebuild.is_some()
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
                if playing {
                    return false;
                }
                if self.enqueue(Request::RepublishPausedPlayback) {
                    self.paused_republish = Some(PausedRepublish::Submitted {
                        project,
                        output,
                        rejected_audio_revision: audio_revision,
                    });
                    self.status = "applying audio update at the paused cursor".into();
                    return true;
                }
                false
            }
            PausedRepublish::Submitted {
                rejected_audio_revision,
                ..
            } => {
                if self.edit_pending()
                    || !self.playback_candidates.is_empty()
                    || self
                        .audio
                        .as_ref()
                        .and_then(|audio| audio.control.status())
                        .is_some_and(|status| status.publication_in_flight())
                {
                    return false;
                }
                if self.audio_admission.current().is_some_and(|current| {
                    current.project == project
                        && current.audio_revision == Some(rejected_audio_revision)
                        && matches!(current.state, AdmissionState::Rejected(_))
                }) {
                    self.paused_republish = Some(PausedRepublish::Exhausted { project, output });
                    self.deferred_play = None;
                    return replace_status(&mut self.status, "stop playback to apply edit");
                }
                false
            }
        }
    }

    pub(in crate::editor) fn tick_latency_rebuild(&mut self) -> bool {
        use crate::editor::audio_admission::LatencyRebuild;
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
            let project = crate::editor::audio_admission::ProjectRevision {
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
                    || self.persistence.opening_project
                    || self.persistence.close_prompt_open
                    || self.persistence.save_dialog_open
                    || self.device_panel.gesture.snapshot().is_some()
                    || self.settings.tuning.client.snapshot().is_some()
                    || self.midi_editor.recorder.blocks_edits()
                    || self.gain_recorder.blocks_edits()
                    || self.computer_keyboard.input.is_some()
                    || !self.pending_audio_commands.is_empty()
                    || self.session_grid.pending_session_ticket.is_some()
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

    pub(in super::super) fn request_latency_restore(
        &mut self,
        project: crate::editor::audio_admission::ProjectRevision,
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
            Some(crate::editor::audio_admission::StoppedPlaybackRestore::Preparing);
        self.latency_rebuild = Some(crate::editor::audio_admission::LatencyRebuild::Restoring {
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
    pub(in super::super) fn clear_latency_waiting(&mut self) -> bool {
        if self.error.as_deref() == Some(LATENCY_EDIT_WAITING) {
            self.error = None;
            true
        } else {
            false
        }
    }

    pub(in super::super) fn tick_stopped_vst3_restore(&mut self) -> bool {
        use crate::editor::audio_admission::StoppedPlaybackRestore;
        let Some(StoppedPlaybackRestore::WaitingForEdits { generation, output }) =
            self.stopped_playback_restore
        else {
            return false;
        };
        if generation != self.project_generation
            || self
                .audio
                .as_ref()
                .and_then(|audio| audio.control.output_identity())
                != Some(output)
            || !self.has_resident_vst3()
        {
            self.stopped_playback_restore = None;
            return true;
        }
        if self.edit_pending()
            || self.settings.tuning.client.snapshot().is_some()
            || self.update_preparing()
        {
            return false;
        }
        self.stopped_playback_restore = None;
        if self.has_output_fault_requiring_recovery() {
            self.recover_audio_at(0);
        } else {
            self.queue_stopped_playback_restore();
        }
        true
    }

    pub(in super::super) fn queue_stopped_playback_restore(&mut self) {
        if self.enqueue(Request::RestoreStoppedPlayback) {
            self.stopped_playback_restore =
                Some(crate::editor::audio_admission::StoppedPlaybackRestore::Preparing);
            crate::editor::device_panel::vst3_parameters::refresh(self);
            self.status = "stopped · restoring current sound".into();
            self.error = None;
        }
    }
}

// Called only after exact current context/output identity has been reconciled.
pub(in crate::editor) const fn is_paused_republish_rejection(
    reason: hex_engine::PlaybackHandoffRejection,
) -> bool {
    matches!(
        reason,
        hex_engine::PlaybackHandoffRejection::SessionRuntime
            | hex_engine::PlaybackHandoffRejection::HeldArrangementGateChanged
            | hex_engine::PlaybackHandoffRejection::ForeignTailSourcePaused
    )
}

#[allow(clippy::too_many_arguments)] // Disjoint EditorView fields borrowed alongside active audio control.
pub(in crate::editor) fn record_paused_republish_rejection(
    paused_republish: &mut Option<crate::editor::audio_admission::PausedRepublish>,
    deferred_play: &mut Option<(PlaybackAction, Option<u64>, u64)>,
    status: &mut String,
    error: &mut Option<crate::editor::display_error::DisplayError>,
    project_generation: u64,
    state: Option<&ViewState>,
    audio_admission: &crate::editor::audio_admission::AudioAdmissionStatus,
    project: crate::editor::audio_admission::ProjectRevision,
    output: Option<u64>,
    audio_revision: u64,
    reason: hex_engine::PlaybackHandoffRejection,
) -> bool {
    use crate::editor::audio_admission::{AdmissionState, PausedRepublish};
    let Some(output) = output else {
        return false;
    };
    if project.generation != project_generation
        || state.is_none_or(|state| state.revision != project.revision)
        || !audio_admission.current().is_some_and(|current| {
            current.project == project
                && current.audio_revision == Some(audio_revision)
                && matches!(current.state, AdmissionState::Rejected(_))
        })
    {
        return false;
    }
    let mut changed = false;
    if reason == hex_engine::PlaybackHandoffRejection::ForeignTailSourcePaused
        || matches!(
            *paused_republish,
            Some(
                PausedRepublish::Submitted {
                    project: previous,
                    output: previous_output,
                    ..
                } | PausedRepublish::Exhausted {
                    project: previous,
                    output: previous_output,
                }
            ) if previous == project && previous_output == output
        )
    {
        *paused_republish = Some(PausedRepublish::Exhausted { project, output });
        *deferred_play = None;
        changed |= replace_status(status, "stop playback to apply edit");
        changed |= replace_error(error, reason.message());
        return changed;
    }
    *paused_republish = Some(PausedRepublish::WaitingForPause {
        project,
        output,
        audio_revision,
    });
    let submitted_status = super::super::submitted_audio_revision_status(audio_revision);
    if *status == submitted_status {
        changed |= replace_status(status, "audio update waiting for pause or stop");
    }
    changed
}
