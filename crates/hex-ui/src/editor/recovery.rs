//! Output lifecycle admission and graph/cursor restoration, without canonical edits.
use super::EditorView;
use hex_engine::playback::PlaybackCommand;
use std::sync::atomic::Ordering::{Acquire, Relaxed};

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "recovery_tests.rs"]
mod tests;

impl EditorView {
    pub(super) fn recover_audio(&mut self) {
        self.invalidate_prepared_seek();
        if self.close_prompt_open
            || self.opening_project
            || self.save_dialog_open
            || self.export_dialog_open
        {
            self.error = Some("finish the current dialog before restarting audio".into());
            return;
        }
        let Some(_state) = self.state.as_ref() else {
            self.error = Some("project is still loading".into());
            return;
        };
        let position = self.cursor_position();
        let generation = match self.recovery.begin(position) {
            Ok(generation) => generation,
            Err(message) => {
                self.error = Some(message.into());
                return;
            }
        };
        self.gain_recorder.discard();
        self.offline_cursor = position;
        if let Some(audio) = self.audio.take() {
            let mut audio = audio;
            audio.control.stop();
            audio.control.close();
            drop(audio);
        }
        self.clear_audio_accounting();
        if self.restart_audio_for_current_document() {
            self.pending_recovery_generation = Some(generation);
            self.status = "restarting audio output".into();
            self.error = None;
        } else {
            self.fail_audio_recovery("audio restart request was not admitted".into());
        }
    }

    pub(super) fn poll_audio_recovery(&mut self) -> bool {
        false
    }

    pub(super) fn finish_audio_recovery_tick(&mut self) -> bool {
        if self.recovery.generation().is_none() {
            return false;
        }
        let graph_is_current = self
            .state
            .as_ref()
            .is_some_and(|state| self.applied_project_revision == Some(state.revision));
        let failed = self
            .audio
            .as_ref()
            .and_then(|audio| audio.control.status())
            .is_none_or(|status| {
                status.device_fault.load(Relaxed) || status.render_fault.load(Relaxed)
            });
        if failed {
            self.fail_audio_recovery(
                "audio output failed during recovery · retry available".into(),
            );
            return true;
        }
        if let Some(position) = self.recovery.seek_needed(graph_is_current) {
            let prepared = position != 0
                && self.current_seek_requires_preroll() == Some(true)
                && self
                    .audio
                    .as_ref()
                    .is_some_and(|audio| audio.control.worker_owns_graph_publication());
            if prepared {
                let Some(generation) = self.recovery.generation() else {
                    return false;
                };
                if let Some(request) = self.request_recovery_prepared_seek(position, generation) {
                    self.recovery.prepared_seeking(generation, request);
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
                self.recovery.seeking(ticket);
                self.pending_seek_ticket = Some(ticket);
            }
        }
        let Some(status) = self.audio.as_ref().and_then(|audio| audio.control.status()) else {
            return false;
        };
        if self.recovery.prepared_revision().is_some_and(|revision| {
            status
                .rejected_revision()
                .is_some_and(|(rejected, _)| rejected == revision)
                && status.applied_revision.load(Acquire) < revision
        }) {
            self.fail_audio_recovery(
                "recovered cursor publication was rejected · retry available".into(),
            );
            return true;
        }
        let position = status.position.load(Relaxed);
        let playing = status.playing.load(Relaxed);
        let ready = if self.recovery.prepared_request().is_some() {
            let audible = self.audible_playback.as_ref().and_then(|candidate| {
                (candidate.project_generation == self.project_generation
                    && self
                        .state
                        .as_ref()
                        .is_some_and(|state| candidate.state.revision == state.revision)
                    && candidate.device_preview_identity.is_none()
                    && candidate.tuning_preview_identity.is_none())
                .then_some(
                    candidate
                        .seek_request_id
                        .map(|request| (request, candidate.audio_revision)),
                )
                .flatten()
            });
            self.recovery.complete_prepared(
                graph_is_current,
                audible,
                status.applied_revision.load(Acquire),
                position,
                playing,
            )
        } else {
            self.recovery.complete(
                graph_is_current,
                if self.document_audio_mode == hex_session::DocumentAudioMode::VerifiedStreaming {
                    status.stream_applied_command_prefix.load(Acquire)
                } else {
                    status.consumed_command.load(Acquire)
                },
                position,
                playing,
            )
        };
        if ready {
            self.offline_cursor = position;
            self.pending_recovery_generation = None;
            self.status = "audio ready · stopped".into();
            self.error = None;
            return true;
        }
        false
    }

    pub(super) fn fail_audio_recovery(&mut self, error: String) {
        self.offline_cursor = self.cursor_position();
        self.recovery.fail();
        if let Some(audio) = self.audio.take() {
            let mut audio = audio;
            audio.control.stop();
            audio.control.close();
            drop(audio);
        }
        self.pending_recovery_generation = None;
        self.pending_restart_request = None;
        self.clear_audio_accounting();
        self.status = "audio unavailable · editing and saving remain available".into();
        self.error = Some(error.into());
    }

    fn clear_audio_accounting(&mut self) {
        self.output_meter_frame = None;
        self.stopped_playback_restore = None;
        self.service.invalidate_arrangement_seek();
        self.prepared_seek = None;
        self.latched_seek_intent = None;
        self.playback_candidates.clear();
        self.audible_playback = None;
        self.audio_admission.reset_audio();
        self.applied_project_revision = None;
        self.pending_audio_commands.clear();
        self.pending_seek_ticket = None;
        self.pending_session_ticket = None;
        self.play_intent.clear();
        self.pending_playback = None;
        self.pending_resident_publication = None;
        self.clear_route_telemetry();
        self.observed_rejected_action = 0;
        self.observed_rejected_revision = 0;
        self.observed_session_capability = None;
        self.session_capability_message = None;
    }

    pub(super) fn cursor_position(&self) -> u64 {
        self.recovery.position().unwrap_or_else(|| {
            self.audio_status()
                .map_or(self.offline_cursor, |status| status.position.load(Relaxed))
        })
    }
}
