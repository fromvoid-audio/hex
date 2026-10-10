//! Output lifecycle admission and graph/cursor restoration, without canonical edits.
use super::EditorView;
use hex_engine::playback::PlaybackCommand;
use hex_session::service::Request;
use std::sync::atomic::Ordering::{Acquire, Relaxed};

impl EditorView {
    pub(super) fn recover_audio(&mut self) {
        if self.close_prompt_open
            || self.opening_project
            || self.save_dialog_open
            || self.export_dialog_open
        {
            self.error = Some("finish the current dialog before restarting audio".into());
            return;
        }
        let Some(state) = self.state.as_ref() else {
            self.error = Some("project is still loading".into());
            return;
        };
        let rate = state.project.sample_rate();
        let position = self.cursor_position();
        if let Err(message) = self.recovery.begin(position) {
            self.error = Some(message.into());
            return;
        }
        self.offline_cursor = position;
        if let Some(audio) = self.audio.take() {
            audio.control.stop();
            drop(audio);
        }
        self.clear_audio_accounting();
        match hex_audio::request_recovery_open(rate) {
            Ok(handle) => {
                self.recovery_open = Some(handle);
                self.status = "restarting audio output".into();
                self.error = None;
            }
            Err(error) => self.fail_audio_recovery(error.to_string()),
        }
    }

    pub(super) fn poll_audio_recovery(&mut self) -> bool {
        let Some(handle) = self.recovery_open.as_mut() else {
            return false;
        };
        let result = match handle.try_take() {
            Ok(None) => return false,
            Ok(Some(result)) => result,
            Err(error) => Err(error.to_string()),
        };
        self.recovery_open = None;
        match result {
            Ok(audio) => {
                let Some(generation) = self.recovery.generation() else {
                    drop(audio);
                    return true;
                };
                self.clear_audio_accounting();
                self.audio = Some(audio);
                self.recovery.opened(generation);
                if self.enqueue(Request::RestorePlayback { generation }) {
                    self.status = "restoring audio graph".into();
                    self.error = None;
                } else {
                    self.fail_audio_recovery(
                        "audio opened but graph restore was not admitted".into(),
                    );
                }
            }
            Err(error) => self.fail_audio_recovery(error),
        }
        true
    }

    pub(super) fn finish_audio_recovery_tick(&mut self) -> bool {
        if self.recovery.generation().is_none() {
            return false;
        }
        let graph_is_current = self
            .state
            .as_ref()
            .is_some_and(|state| self.applied_project_revision == Some(state.revision));
        let Some(audio) = self.audio.as_mut() else {
            return false;
        };
        if audio.control.status.device_fault.load(Relaxed)
            || audio.control.status.render_fault.load(Relaxed)
        {
            self.fail_audio_recovery(
                "audio output failed during recovery · retry available".into(),
            );
            return true;
        }
        if let Some(position) = self.recovery.seek_needed(graph_is_current) {
            if let Ok(ticket) = audio
                .control
                .command_with_ticket(PlaybackCommand::Seek(position))
            {
                self.recovery.seeking(ticket);
                self.pending_seek_ticket = Some(ticket);
            }
        }
        let status = &audio.control.status;
        if self.recovery.complete(
            graph_is_current,
            status.consumed_command.load(Acquire),
            status.position.load(Relaxed),
            status.playing.load(Relaxed),
        ) {
            self.offline_cursor = status.position.load(Relaxed);
            self.status = "audio ready · stopped".into();
            self.error = None;
            return true;
        }
        false
    }

    pub(super) fn fail_audio_recovery(&mut self, error: String) {
        self.offline_cursor = self.cursor_position();
        self.recovery_open = None;
        self.recovery.fail();
        if let Some(audio) = self.audio.take() {
            audio.control.stop();
            drop(audio);
        }
        self.clear_audio_accounting();
        self.status = "audio unavailable · editing and saving remain available".into();
        self.error = Some(error);
    }

    fn clear_audio_accounting(&mut self) {
        self.playback_candidates.clear();
        self.audible_playback = None;
        self.audio_admission.reset_audio();
        self.applied_project_revision = None;
        self.pending_audio_commands.clear();
        self.pending_seek_ticket = None;
        self.pending_session_ticket = None;
        self.play_intent.clear();
        self.pending_playback = None;
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
