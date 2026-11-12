//! Output lifecycle admission and graph/cursor restoration, without canonical edits.
use super::{EditorView, audio_admission::ProjectRevision, recovery_state::DeferredNativeEffect};
use hex_engine::NativeDeviceClass;
use hex_engine::playback::{PlaybackAction, PlaybackCommand};
use hex_session::service::{NativeDeviceDropSource, Request};
use std::sync::atomic::Ordering::{Acquire, Relaxed};

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "recovery_tests.rs"]
pub(in crate::editor) mod tests;

impl EditorView {
    pub(super) fn has_resident_vst3(&self) -> bool {
        self.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && self.state.as_ref().is_some_and(|state| {
                state
                    .project
                    .automation_devices()
                    .any(|device| device.state().type_id().as_str() == Ok("hex.vst3"))
            })
    }

    pub(super) fn holds_stopped_vst3_requests(&self) -> bool {
        self.stopped_playback_restore.is_some_and(|restore| {
            !matches!(
                restore,
                super::audio_admission::StoppedPlaybackRestore::WaitingForEdits { .. }
            )
        }) && self.has_resident_vst3()
    }

    pub(super) fn has_degraded_vst3_output(&self) -> bool {
        self.audio.is_some()
            && self.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && self.stopped_playback_restore.is_none()
            && !self.degraded_plugins.is_empty()
    }

    pub(super) fn native_effect_insert_needs_recovery(&self, request: &Request) -> bool {
        (self.has_degraded_vst3_output() || self.recovery.generation().is_some())
            && matches!(
                request,
                Request::DropBrowserDeviceOnArrangement {
                    class: NativeDeviceClass::Effect,
                    ..
                } | Request::DropNativeDevice {
                    source: NativeDeviceDropSource::Browser {
                        class: NativeDeviceClass::Effect,
                        ..
                    },
                    ..
                } | Request::AddNativeDevice {
                    class: NativeDeviceClass::Effect,
                    ..
                }
            )
    }

    /// Capture a single device insertion before retiring the degraded output.
    /// The normal command validates the same destination again after recovery.
    pub(super) fn recover_before_native_effect_insert(&mut self, request: Request) -> bool {
        if self.recovery.generation().is_some() {
            self.error = Some("audio recovery pending · wait before adding a device".into());
            return false;
        }
        if self.edit_pending() || self.pending_restart_request.is_some() {
            self.error =
                Some("finish the current edit before restarting audio to add the device".into());
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let project = ProjectRevision {
            generation: self.project_generation,
            revision: state.revision,
        };
        let current = match &request {
            Request::DropBrowserDeviceOnArrangement {
                project_generation,
                revision,
                ..
            }
            | Request::DropNativeDevice {
                project_generation,
                revision,
                ..
            } => (*project_generation, *revision) == (project.generation, project.revision),
            Request::AddNativeDevice { track, .. } => state.project.track(*track).is_some(),
            _ => false,
        };
        if !current {
            self.error = Some("device insertion canceled · project changed".into());
            return false;
        }

        let playing = self
            .audio_status()
            .is_some_and(|status| status.playing.load(Acquire));
        self.recover_audio();
        if self.recovery.generation().is_none() {
            return false;
        }
        self.recovery.native_effect = Some(DeferredNativeEffect {
            project,
            request: Box::new(request),
        });
        if playing {
            self.defer_play_until_audio_recovers();
        }
        self.status = "restarting audio · device will be added when ready".into();
        true
    }

    fn submit_recovered_native_effect(&mut self) {
        let Some(pending) = self.recovery.native_effect.take() else {
            return;
        };
        if pending.project.generation != self.project_generation
            || self
                .state
                .as_ref()
                .is_none_or(|state| state.revision != pending.project.revision)
        {
            self.error =
                Some("device insertion canceled · project changed during audio recovery".into());
        } else if self.has_degraded_vst3_output() {
            self.error = Some("plug-in failed during audio recovery · device was not added".into());
        } else if self.enqueue(*pending.request) {
            self.status = "adding device".into();
        }
    }

    pub(super) fn defer_play_until_audio_recovers(&mut self) {
        self.deferred_play = Some((
            PlaybackAction::Transport(PlaybackCommand::Play),
            None,
            self.project_generation,
        ));
        self.status = "restarting audio output · playback starts when ready".into();
    }

    /// Use the requested start as the restored cursor before admitting Play.
    pub(super) fn recover_degraded_play_at(&mut self, frame: u64) -> bool {
        if self.recovery.generation().is_some() || !self.has_degraded_vst3_output() {
            return false;
        }
        self.recover_audio_at(frame);
        if self.recovery.generation().is_some() {
            self.defer_play_until_audio_recovers();
        }
        true
    }

    pub(super) fn recover_audio(&mut self) {
        let position = self.recovery.position().unwrap_or_else(|| {
            self.audio_status()
                .map_or(self.offline_cursor, |status| status.recovery_position())
        });
        self.recover_audio_at(position);
    }

    pub(super) fn recover_audio_at(&mut self, position: u64) {
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
        if self.seek_pending() && self.recovery.generation().is_none() {
            self.error =
                Some("seek pending · wait for audio or press stop before restarting".into());
            return;
        }
        self.invalidate_prepared_seek();
        let generation = match self.recovery.begin(position) {
            Ok(generation) => generation,
            Err(message) => {
                self.error = Some(message.into());
                return;
            }
        };
        self.offline_cursor = position;
        if !self.retire_audio_preserving_gain_take() {
            self.recovery.fail();
            return;
        }
        self.clear_audio_accounting();
        if self.restart_audio_for_current_document() {
            self.pending_recovery_generation = Some(generation);
            self.status = "restarting audio output".into();
            self.error = None;
        } else {
            self.fail_audio_recovery("audio restart request was not admitted");
        }
    }

    /// Moves a changed default output route (#4131) through the same recovery
    /// as restart audio: the cursor is kept and the graph restored, and
    /// playback stays stopped. Waits without an error while a dialog, load,
    /// pending seek, restart or recovery is active; drops the request when
    /// the named output is no longer current.
    pub(super) fn follow_output_route(&mut self) -> bool {
        let Some(identity) = self.route_follow else {
            return false;
        };
        if self
            .audio
            .as_ref()
            .and_then(|audio| audio.control.output_identity())
            != Some(identity)
        {
            self.route_follow = None;
            return false;
        }
        if self.close_prompt_open
            || self.opening_project
            || self.save_dialog_open
            || self.export_dialog_open
            || self.state.is_none()
            || self.seek_pending()
            || self.recovery.generation().is_some()
            || self.pending_restart_request.is_some()
        {
            return false;
        }
        self.route_follow = None;
        self.recover_audio();
        if self.pending_recovery_generation.is_some() {
            self.status = "audio output changed · reopening".into();
        }
        true
    }

    pub(super) fn poll_audio_recovery(&mut self) -> bool {
        false
    }

    pub(super) fn finish_audio_recovery_tick(&mut self) -> bool {
        if self.recovery.generation().is_none() {
            return false;
        }
        let reliable = self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.control.has_action_outcomes());
        if reliable && self.recovery.seek_failed() {
            self.fail_audio_recovery("recovered cursor request was not applied · retry available");
            return true;
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
            self.fail_audio_recovery("audio output failed during recovery · retry available");
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
            self.fail_audio_recovery("recovered cursor publication was rejected · retry available");
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
        } else if reliable {
            self.recovery
                .complete_exact(graph_is_current, position, playing)
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
            self.submit_recovered_native_effect();
            return true;
        }
        false
    }

    pub(super) fn fail_audio_recovery(&mut self, error: impl Into<hex_audio::OutputOpenError>) {
        let error = error.into();
        self.offline_cursor = self.cursor_position();
        self.recovery.fail();
        self.retire_audio_preserving_gain_take();
        self.pending_recovery_generation = None;
        self.pending_restart_request = None;
        self.clear_audio_accounting();
        if error.rate_mismatch().is_some() {
            let message = super::audio_unavailable::unavailable_message(&error);
            self.status = message.clone();
            self.error = Some(message.into());
            return;
        }
        self.status = "audio unavailable · editing and saving remain available".into();
        self.error = Some(error.to_string().into());
    }

    fn clear_audio_accounting(&mut self) {
        self.output_meter_frame = None;
        self.stopped_playback_restore = None;
        self.deferred_play = None;
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

    /// Scheduled cursor: the frame edits, insertion, zoom anchors and transport
    /// commands act on.
    pub(super) fn cursor_position(&self) -> u64 {
        cursor_frame(
            self.recovery.position(),
            self.audio_status(),
            self.offline_cursor,
            Cursor::Scheduled,
        )
    }

    /// Display-only playhead: the frame now leaving Main output, which trails
    /// the scheduled cursor by the graph's Main latency.
    pub(super) fn audible_cursor_position(&self) -> u64 {
        cursor_frame(
            self.recovery.position(),
            self.audio_status(),
            self.offline_cursor,
            Cursor::Audible,
        )
    }
}

#[derive(Clone, Copy)]
enum Cursor {
    Scheduled,
    Audible,
}

/// A pending recovery position wins, then the live callback status, then the
/// offline cursor.
fn cursor_frame(
    recovery: Option<u64>,
    status: Option<&hex_audio::prepared::Status>,
    offline: u64,
    cursor: Cursor,
) -> u64 {
    recovery.unwrap_or_else(|| {
        status.map_or(offline, |status| match cursor {
            Cursor::Scheduled => status.position.load(Relaxed),
            Cursor::Audible => status.audible_position.load(Relaxed),
        })
    })
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod cursor_tests {
    use super::{Cursor, cursor_frame};
    use std::sync::atomic::Ordering::Relaxed;

    #[test]
    fn playhead_shows_the_audible_frame_while_edits_use_the_scheduled_one() {
        let status = hex_audio::prepared::Status::default();
        status.position.store(960, Relaxed);
        status.audible_position.store(672, Relaxed);
        assert_eq!(cursor_frame(None, Some(&status), 0, Cursor::Audible), 672);
        assert_eq!(cursor_frame(None, Some(&status), 0, Cursor::Scheduled), 960);
        // Recovery and offline cursors are not latency-shifted.
        for cursor in [Cursor::Audible, Cursor::Scheduled] {
            assert_eq!(cursor_frame(Some(48), Some(&status), 0, cursor), 48);
            assert_eq!(cursor_frame(None, None, 96, cursor), 96);
        }
    }
}
