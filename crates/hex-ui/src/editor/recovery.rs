//! Output lifecycle admission and graph/cursor restoration, without canonical edits.
use super::EditorView;
use hex_engine::playback::PlaybackCommand;
use std::sync::atomic::Ordering::{Acquire, Relaxed};

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
        let Some(audio) = self.audio.as_mut() else {
            return false;
        };
        if let Some(position) = self.recovery.seek_needed(graph_is_current) {
            if let Ok(ticket) = audio
                .control
                .command_with_ticket(PlaybackCommand::Seek(position))
            {
                self.recovery.seeking(ticket);
                self.pending_seek_ticket = Some(ticket);
            }
        }
        let Some(status) = audio.control.status() else {
            return false;
        };
        if self.recovery.complete(
            graph_is_current,
            if self.document_audio_mode == hex_session::DocumentAudioMode::VerifiedStreaming {
                status.stream_applied_command_prefix.load(Acquire)
            } else {
                status.consumed_command.load(Acquire)
            },
            status.position.load(Relaxed),
            status.playing.load(Relaxed),
        ) {
            self.offline_cursor = status.position.load(Relaxed);
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
