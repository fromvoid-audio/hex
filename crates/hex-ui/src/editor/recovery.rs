//! Output lifecycle admission and graph/cursor restoration, without canonical edits.
use super::{
    EditorView,
    drag::BrowserDropDestination,
    pending_sample_drop::PendingSampleDrop,
    recovery_state::{DeferredEditKind, DeferredGraphEdit, RecoveryState},
};
use hex_engine::playback::{PlaybackAction, PlaybackCommand};
use hex_engine::{AudioSourceContext, NativeDeviceClass};
use hex_session::service::{NativeDeviceDropSource, Request};
use std::sync::atomic::Ordering::{Acquire, Relaxed};

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "recovery_tests.rs"]
pub(in crate::editor) mod tests;

/// Drop the released preview only if it still belongs to the deferred import.
pub(super) fn clear_recovery(
    recovery: &mut RecoveryState,
    sample_drop: &mut Option<PendingSampleDrop>,
    time_move: &mut Option<super::time_selection::PendingTimeRangeMove>,
) {
    if recovery
        .deferred_edit
        .as_ref()
        .is_some_and(|edit| edit.kind == DeferredEditKind::SampleDrop)
    {
        *sample_drop = None;
    }
    if recovery
        .deferred_edit
        .as_ref()
        .is_some_and(|edit| edit.kind == DeferredEditKind::TimeDuplicate)
    {
        *time_move = None;
    }
    recovery.fail();
}

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

    pub(super) fn recoverable_graph_edit(&self, request: &Request) -> Option<DeferredEditKind> {
        if !self.has_degraded_vst3_output() && self.recovery.deferred_edit.is_none() {
            return None;
        }
        if self.pending_time_duplicate_matches(request) {
            return Some(DeferredEditKind::TimeDuplicate);
        }
        if matches!(
            self.sample_drop_target(),
            Some(
                BrowserDropDestination::ExistingTrack { .. }
                    | BrowserDropDestination::NewTrack { .. }
            )
        ) && self.pending_sample_drop_matches(request)
        {
            return Some(DeferredEditKind::SampleDrop);
        }
        matches!(
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
        .then_some(DeferredEditKind::NativeEffect)
    }

    /// Hold one admitted edit through the fresh graph and cursor. The sample
    /// request already carries the policy captured at its original drop.
    pub(super) fn recover_before_graph_edit(
        &mut self,
        kind: DeferredEditKind,
        request: Request,
    ) -> bool {
        if self.recovery.generation().is_some() {
            self.error = Some("audio recovery pending · wait before editing".into());
            return false;
        }
        if self.edit_pending() || self.pending_restart_request.is_some() {
            self.error = Some("finish the current edit before restarting audio".into());
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let context = AudioSourceContext {
            project_generation: self.project_generation,
            editor_revision: state.revision,
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
            } => {
                (*project_generation, *revision)
                    == (context.project_generation, context.editor_revision)
            }
            Request::AddNativeDevice { track, .. } => state.project.track(*track).is_some(),
            _ => match kind {
                DeferredEditKind::SampleDrop => self.pending_sample_drop_matches(&request),
                DeferredEditKind::TimeDuplicate => self.pending_time_duplicate_matches(&request),
                DeferredEditKind::NativeEffect => false,
            },
        };
        if !current {
            self.error = Some("edit canceled · project changed".into());
            return false;
        }

        let playing = self
            .audio_status()
            .is_some_and(|status| status.playing.load(Acquire));
        self.recover_audio();
        if self.recovery.generation().is_none() {
            return false;
        }
        if kind == DeferredEditKind::TimeDuplicate {
            self.mark_time_duplicate_recovery_started();
        }
        self.recovery.deferred_edit = Some(DeferredGraphEdit {
            kind,
            context,
            request: Box::new(request),
        });
        if playing {
            self.defer_play_until_audio_recovers();
        }
        self.status = match kind {
            DeferredEditKind::NativeEffect => "restarting audio · device will be added when ready",
            DeferredEditKind::SampleDrop => "restarting audio · sample will be imported when ready",
            DeferredEditKind::TimeDuplicate => {
                "restarting audio · time will be duplicated when ready"
            }
        }
        .into();
        true
    }

    fn submit_recovered_graph_edit(&mut self) {
        let Some(pending) = self.recovery.deferred_edit.take() else {
            return;
        };
        let sample_drop = pending.kind == DeferredEditKind::SampleDrop;
        let time_duplicate = pending.kind == DeferredEditKind::TimeDuplicate;
        if (sample_drop && !self.pending_sample_drop_matches(&pending.request))
            || (time_duplicate && !self.pending_time_duplicate_matches(&pending.request))
        {
            return;
        }
        let accepted = if time_duplicate && !self.pending_time_duplicate_intent_current() {
            self.error =
                Some("edit canceled · time selection changed during audio recovery".into());
            false
        } else if pending.context.project_generation != self.project_generation
            || self
                .state
                .as_ref()
                .is_none_or(|state| state.revision != pending.context.editor_revision)
        {
            self.error = Some("edit canceled · project changed during audio recovery".into());
            false
        } else if (sample_drop || time_duplicate) && self.edit_pending() {
            // A worker edit without an installed reply can already have
            // changed the captured revision for these unversioned requests.
            self.error = Some("edit canceled · project work pending during audio recovery".into());
            false
        } else if self.has_degraded_vst3_output() {
            self.error = Some("plug-in failed during audio recovery · edit was canceled".into());
            false
        } else {
            self.enqueue_request(*pending.request, false)
        };
        if accepted {
            self.status = if sample_drop {
                "importing sample to arrangement".into()
            } else if time_duplicate {
                "duplicating time".into()
            } else {
                "adding device".into()
            };
        } else if sample_drop {
            self.pending_sample_drop = None;
        } else if time_duplicate {
            self.pending_time_range_move = None;
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

    /// A contained VST3 worker fault leaves the applied graph playable. Only
    /// recover on Play when the output itself cannot serve the start.
    pub(super) fn recover_degraded_play_at(&mut self, frame: u64) -> bool {
        if self.recovery.generation().is_some()
            || !self.has_degraded_vst3_output()
            || self.audio_status().is_some_and(|status| {
                status.applied_revision.load(Acquire) != 0
                    && !status.device_fault.load(Acquire)
                    && !status.render_fault.load(Acquire)
                    && !status.shutdown_complete.load(Acquire)
            })
        {
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
            clear_recovery(
                &mut self.recovery,
                &mut self.pending_sample_drop,
                &mut self.pending_time_range_move,
            );
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
                    && candidate.tuning_preview_identity.is_none()
                    && candidate.clip_preview_identity.is_none())
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
            self.submit_recovered_graph_edit();
            return true;
        }
        false
    }

    pub(super) fn fail_audio_recovery(&mut self, error: impl Into<hex_audio::OutputOpenError>) {
        let error = error.into();
        self.offline_cursor = self.cursor_position();
        clear_recovery(
            &mut self.recovery,
            &mut self.pending_sample_drop,
            &mut self.pending_time_range_move,
        );
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
