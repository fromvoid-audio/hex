//! Timeline seek, zoom, fit, and pan request helpers.
use super::*;

impl EditorView {
    pub(in crate::editor) fn request_seek(&mut self, frame: u64) {
        self.settings.playback.pending = None;
        self.deferred_toggle_play = None;
        self.deferred_metadata_seek = None;
        self.wake_poll();
        self.latency_rebuild = None;
        if self.section_install_pending() {
            self.error = Some("applying edit · wait for audio".into());
            return;
        }
        if self.recovery.generation().is_some() {
            self.recovery.retarget(frame);
            if let Some((
                PlaybackAction::Transport(
                    PlaybackCommand::PlayFrom(position)
                    | PlaybackCommand::PlayFromWithNoteChase {
                        frame: position, ..
                    },
                ),
                _,
                _,
            )) = self.deferred_play.as_mut()
            {
                *position = frame;
            }
            self.offline_cursor = frame;
            self.status = "restarting audio · playback position updated".into();
            self.error = None;
            return;
        }
        if self.stopped_playback_restore.is_some() {
            self.error = Some("restoring stopped playback · wait before seeking".into());
            return;
        }
        if self.persistence.opening_project || self.persistence.close_prompt_open {
            self.error = Some(if self.persistence.opening_project {
                "opening project · seeking is paused".into()
            } else {
                "close confirmation open · seeking is paused".into()
            });
            return;
        }
        match self.current_seek_requires_preroll() {
            Some(true) => {
                let start_playing = self.audio.as_ref().is_some_and(|audio| {
                    self.desired_playing(
                        audio
                            .control
                            .status()
                            .is_some_and(|status| status.playing.load(Acquire)),
                    )
                });
                self.request_prepared_seek(frame, start_playing);
                return;
            }
            Some(false) => {}
            None if self.audio.is_none() => {}
            None if self.awaiting_applied_playback_metadata() => {
                self.deferred_metadata_seek = Some((frame, self.project_generation));
                self.status = "waiting for playback metadata · seek queued".into();
                clear_stale_playback_unavailable_error(&mut self.error);
                self.wake_poll();
                return;
            }
            None => {
                self.error = Some("playback source is unavailable · cannot seek safely".into());
                return;
            }
        }
        if self.seek_pending() {
            self.error = Some("seek pending · wait for the cursor".into());
            return;
        }
        let Some(audio) = self.audio.as_mut() else {
            self.error = Some("audio output is unavailable · cannot seek".into());
            return;
        };

        let command = PlaybackAction::Transport(PlaybackCommand::Seek(frame));
        if self.pending_audio_commands.is_empty() {
            match if let Some(trace) = &self.transport_trace {
                trace.attempt(audio, command)
            } else {
                audio.control.action_with_ticket(command)
            } {
                Ok(ticket) => {
                    self.pending_seek_ticket = Some(ticket);
                    clear_stale_playback_unavailable_error(&mut self.error);
                }
                Err(rejected) if self.pending_audio_commands.len() < UI_REQUEST_CAPACITY => {
                    self.pending_audio_commands
                        .push_back(PendingPlaybackAction {
                            action: rejected,
                            generation: self.project_generation,
                            revision: None,
                            intent_token: None,
                        });
                    self.status = "seek queued".into();
                }
                Err(_) => {
                    self.error = Some("playback command queue full · seek was not applied".into())
                }
            }
        } else if self.pending_audio_commands.len() < UI_REQUEST_CAPACITY {
            self.pending_audio_commands
                .push_back(PendingPlaybackAction {
                    action: command,
                    generation: self.project_generation,
                    revision: None,
                    intent_token: None,
                });
            self.status = "seek queued".into();
        } else {
            self.error = Some("playback command queue full · seek was not applied".into());
        }
    }

    pub(in crate::editor) fn seek_pending(&self) -> bool {
        if self.recovery.generation().is_some()
            || self.section_launch_waiting()
            || self.prepared_seek.is_some()
            || self.latched_seek_intent.is_some()
            || self.retired_seek_result.is_some()
        {
            return true;
        }
        let queued = self.pending_audio_commands.iter().any(|pending| {
            matches!(
                pending.action,
                PlaybackAction::Transport(
                    PlaybackCommand::Seek(_)
                        | PlaybackCommand::PlayFrom(_)
                        | PlaybackCommand::PlayFromWithNoteChase { .. }
                )
            )
        });
        let consumed = self.audio_status().map_or(0, |status| {
            audio_command_prefix(status, self.document_audio_mode)
        });
        if self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.control.has_action_outcomes())
        {
            queued || self.pending_seek_ticket.is_some()
        } else {
            seek_is_pending(self.pending_seek_ticket, consumed, queued)
        }
    }

    pub(in crate::editor) fn cancel_duplicate_chain(&mut self) {
        self.duplicate_intent.cancel_chain();
        self.cancel_time_duplicate_repeats();
    }

    pub(in crate::editor) fn zoom_timeline(&mut self, inward: bool) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let horizon = self.viewport_horizon(state.timeline_frames);
        let playhead = self.cursor_position();
        let viewport = self
            .visible_viewport(horizon)
            .zoom(horizon, playhead, inward);
        self.install_arranger_navigation(viewport);
    }

    pub(in crate::editor) fn zoom_timeline_wheel_at(
        &mut self,
        pointer_x: gpui::Pixels,
        timeline_left: f32,
        timeline_width: f32,
        delta_y: f32,
    ) -> bool {
        const PIXELS_PER_STEP: f32 = 80.0;
        const MAX_ACCUMULATED_PIXELS: f32 = PIXELS_PER_STEP * 4.0;
        if !delta_y.is_finite() || delta_y == 0.0 {
            return false;
        }
        self.zoom_wheel_accum = (self.zoom_wheel_accum + delta_y)
            .clamp(-MAX_ACCUMULATED_PIXELS, MAX_ACCUMULATED_PIXELS);
        let steps = (self.zoom_wheel_accum / PIXELS_PER_STEP).trunc() as i8;
        if steps == 0 {
            return false;
        }
        self.zoom_wheel_accum -= f32::from(steps) * PIXELS_PER_STEP;

        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let horizon = self.viewport_horizon(state.timeline_frames);
        let current = self.visible_viewport(horizon);
        let Some(anchor) = crate::editor::seek::pointer_to_frame(
            f32::from(pointer_x),
            timeline_left,
            timeline_width,
            current,
        ) else {
            return false;
        };
        let mut next = current;
        for _ in 0..steps.unsigned_abs() {
            next = next.zoom_at(horizon, anchor, steps < 0);
        }
        if next == current {
            return false;
        }
        self.install_arranger_navigation(next);
        true
    }

    pub(in crate::editor) fn fit_timeline(&mut self) {
        self.clear_retained_navigation();
        self.viewport = None;
    }

    pub(in crate::editor) fn pan_timeline(&mut self, right: bool) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let horizon = self.viewport_horizon(state.timeline_frames);
        let current = self.visible_viewport(horizon);
        let panned = current.pan_half(horizon, right);
        self.install_arranger_navigation(panned);
    }
}
