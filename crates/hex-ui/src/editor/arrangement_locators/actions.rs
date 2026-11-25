//! Arrangement locator navigation, creation, and committed-view reconciliation.
use super::*;

impl EditorView {
    /// Navigate strictly past the stopped play start or the scheduled running
    /// cursor. At either end there is no wrap. Running seeks retain transport.
    pub(in crate::editor) fn navigate_locator(&mut self, forward: bool, window: &Window) {
        if self.mode != EditorMode::Arrangement
            || !(self.focus.is_focused(window) || self.locators.focus.is_focused(window))
            || self.track_header_focused(window)
        {
            return;
        }
        let playing = self.desired_playing(
            self.audio_status()
                .is_some_and(|status| status.playing.load(Acquire)),
        );
        let early_source = if playing && !self.locator_edit_available() {
            self.locator_applied_source()
                .map(|source| (source, false))
                .or_else(|| self.locator_play_source().map(|source| (source, true)))
        } else {
            None
        };
        let available = if playing {
            self.locator_edit_available() || early_source.is_some()
        } else {
            self.locator_committed_action_available()
        };
        if !available {
            return;
        }
        let reference = if playing {
            self.cursor_position()
        } else {
            self.playback_start_frame()
        };
        let state = if playing && early_source.is_none() {
            self.state.as_ref()
        } else {
            self.locator_state()
        };
        let Some(state) = state else {
            return;
        };
        let locators = state.project.arrangement_locators();
        let index = if forward {
            Some(locators.partition_point(|locator| locator.frame() <= reference))
        } else {
            locators
                .partition_point(|locator| locator.frame() < reference)
                .checked_sub(1)
        };
        let Some(frame) = index
            .and_then(|index| locators.get(index))
            .map(|locator| locator.frame())
        else {
            return;
        };
        if playing {
            if let Some((source, needs_preparation)) = early_source {
                if needs_preparation {
                    self.request_committed_locator_seek(frame, source, None);
                } else {
                    self.queue_applied_source_seek(frame, source);
                }
            } else {
                self.request_seek(frame);
            }
        } else {
            self.place_locator_play_start(frame);
        }
    }

    pub(super) fn locator_color(&self, row: &ArrangementLocatorAppearance, frame: u64) -> u32 {
        if frame == row.frame {
            Self::locator_display_color(row.color)
        } else {
            self.locator_color_at_frame(frame)
        }
    }

    pub(super) fn locator_color_at_frame(&self, frame: u64) -> u32 {
        let color = self.state.as_ref().and_then(|state| {
            let sections = state.project.arrangement_sections();
            let tempo = state.project.tempo_map();
            let index = sections.partition_point(|section| {
                tempo
                    .beat_to_frame(section.end())
                    .is_ok_and(|end| end <= frame)
            });
            let section = sections.get(index)?;
            tempo
                .beat_to_frame(section.start())
                .ok()
                .filter(|start| *start <= frame)
                .map(|_| section.color())
        });
        Self::locator_display_color(color)
    }

    pub(super) fn locator_display_color(color: Option<hex_project::ElementColor>) -> u32 {
        color.map_or_else(
            || theme::color(theme::PaletteRole::Muted),
            |color| theme::element_colors(color).accent,
        )
    }

    pub(in crate::editor) fn refresh_locators(&mut self, state: &ViewState, viewport: Viewport) {
        if self
            .locators
            .cache
            .as_ref()
            .is_none_or(|(previous, _)| !previous.ptr_eq(&Arc::downgrade(&state.project)))
        {
            self.locators.cache = Some((
                Arc::downgrade(&state.project),
                state.project.arrangement_locator_appearances(),
            ));
        }
        if self
            .locators
            .drag
            .as_ref()
            .is_some_and(|drag| !drag.source.current(self) || drag.viewport != viewport)
        {
            self.locators.drag = None;
        }
        if self
            .locators
            .name
            .as_ref()
            .is_some_and(|name| !name.source.current(self))
        {
            self.locators.name = None;
        }
    }

    pub(super) fn locator_state(&self) -> Option<&Arc<ViewState>> {
        self.locators.committed.as_ref().or(self.state.as_ref())
    }

    pub(in crate::editor) fn locator_display_state(&self) -> Option<Arc<ViewState>> {
        self.locator_state().cloned()
    }

    pub(in crate::editor) fn observe_locator_add_view(
        &mut self,
        observation: hex_session::service::ArrangementEditViewObservation,
    ) -> bool {
        let hex_session::service::ArrangementEditViewKind::LocatorAdd(id) = observation.kind else {
            return false;
        };
        if self.persistence.opening_project
            || self.persistence.close_prompt_open
            || observation.project_generation != self.project_generation
            || observation.first_previous_revision != observation.previous_revision
            || self
                .locator_state()
                .is_none_or(|state| state.revision != observation.previous_revision)
        {
            return false;
        }
        if self
            .locator_state()
            .is_some_and(|state| state.project.arrangement_locator(id).is_some())
        {
            return false;
        }
        let Some(state) = observation.state.upgrade() else {
            return false;
        };
        if observation.previous_revision.checked_add(1) != Some(state.revision) {
            return false;
        }
        let Some(locator) = state.project.arrangement_locator(id) else {
            return false;
        };
        if !self
            .locators
            .adds
            .iter()
            .any(|add| !add.receipt.is_queued() && add.frame == locator.frame())
        {
            return false;
        }
        self.locators.committed = Some(state);
        true
    }

    pub(in crate::editor) fn add_locator(&mut self, window: &Window) {
        if self.mode != EditorMode::Arrangement
            || !(self.focus.is_focused(window) || self.locators.focus.is_focused(window))
            || self.track_header_focused(window)
            || self.computer_keyboard.accepts_key("i")
            || !self.locator_add_available()
        {
            return;
        }
        let frame = self
            .normalized_time_selection()
            .map(|range| range.start())
            .or_else(|| {
                self.edit_cursor
                    .filter(|cursor| cursor.generation == self.project_generation)
                    .map(|cursor| cursor.frame)
            })
            .unwrap_or_else(|| self.playback_start_frame());
        let Some(state) = self.state.as_ref() else {
            return;
        };
        if self.locators.adds.iter().any(|add| add.frame == frame)
            || state
                .project
                .arrangement_locators()
                .binary_search_by_key(&frame, |locator| locator.frame())
                .is_ok()
        {
            self.status = "locator already exists here".into();
            return;
        }
        self.locators.adds.push(PendingAdd {
            receipt: Receipt::new(self.project_generation),
            frame,
        });
        if !self.enqueue(Request::Edit(Command::AddArrangementLocator { frame })) {
            self.locators.refuse_add(frame);
        }
    }

    pub(super) fn locator_add_available(&self) -> bool {
        // Additional presses can join this sequence while it owns all of the
        // pending work. Other edit and pointer guards still apply.
        self.locator_controls_available()
            && !self.edit_blocked_outside_request_queue()
            && self.worker_requests_pending
                == self
                    .locators
                    .adds
                    .iter()
                    .filter(|add| !add.receipt.is_queued())
                    .count()
            && self
                .pending_requests
                .iter()
                .all(|request| self.locators.queued_add_frame(request).is_some())
    }

    pub(super) fn locator_edit_available(&self) -> bool {
        !self.edit_pending() && self.locator_controls_available()
    }

    // Naming and stopped navigation may use the committed view only when it
    // contains every admitted locator add and no other work is pending.
    pub(super) fn locator_committed_action_available(&self) -> bool {
        self.locator_edit_available()
            || (self.locator_add_available()
                && self.pending_requests.is_empty()
                && self.device_panel.gesture_barriers.is_empty()
                && self.locators.committed.as_ref().is_some_and(|state| {
                    self.locators.adds.iter().all(|add| {
                        !add.receipt.is_queued()
                            && state
                                .project
                                .arrangement_locators()
                                .binary_search_by_key(&add.frame, |locator| locator.frame())
                                .is_ok()
                    })
                }))
    }

    /// An applied acknowledgement still needs its exact view even when new
    /// work has closed admission for another seek.
    pub(in crate::editor) fn committed_locator_seek_ack_view(
        &self,
        source: hex_audio::prepared::AppliedAudioSource,
    ) -> Option<&Arc<ViewState>> {
        let state = self.locator_state()?;
        (self.project_generation == source.context.project_generation
            && state.audio_authoring_revision(source.context) == Some(state.revision))
        .then_some(state)
    }

    /// New seeks additionally require a complete view and no unrelated work.
    pub(in crate::editor) fn committed_locator_seek_view(
        &self,
        source: hex_audio::prepared::AppliedAudioSource,
    ) -> Option<&Arc<ViewState>> {
        let state = self.committed_locator_seek_ack_view(source)?;
        (self.mode == EditorMode::Arrangement && self.locator_committed_action_available())
            .then_some(state)
    }

    pub(in crate::editor) fn committed_locator_seek_context_is_current(
        &self,
        source: hex_audio::prepared::AppliedAudioSource,
    ) -> bool {
        let Some(audio) = self.audio.as_ref() else {
            return false;
        };
        let Some(status) = audio.control.status() else {
            return false;
        };
        self.committed_locator_seek_view(source).is_some()
            && self.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && self.pending_audio_commands.is_empty()
            && self.stopped_playback_restore.is_none()
            && !self.section_install_pending()
            && self.recovery.permits_seek(None)
            && !self.device_panel.gesture.has_pending_terminal()
            && self.device_panel.gesture_barriers.is_empty()
            && self.settings.tuning.client.snapshot().is_none()
            && audio.control.output_identity() == Some(source.output_identity)
            && !status.device_fault.load(Acquire)
            && !status.render_fault.load(Acquire)
            && !status.shutdown_complete.load(Acquire)
            && !status.stopped_edit_pending()
            && status.playing.load(Acquire)
            && self.play_intent.desired_playing(true)
    }

    pub(in crate::editor) fn committed_locator_seek_source_is_current(
        &self,
        source: hex_audio::prepared::AppliedAudioSource,
    ) -> bool {
        self.committed_locator_seek_context_is_current(source)
            && self
                .audio_status()
                .and_then(|status| status.recording_source())
                == Some(source)
    }

    /// A committed add may drive playback only after its exact resident,
    /// non-preview source is installed by the current callback.
    pub(super) fn locator_applied_source(&self) -> Option<hex_audio::prepared::AppliedAudioSource> {
        self.locator_source_for_action(false)
    }

    /// Immediate clicks use interactive PlayFrom, including while playing;
    /// keyboard navigation retains the stricter stateful-seek guard.
    pub(in crate::editor) fn locator_play_source(
        &self,
    ) -> Option<hex_audio::prepared::AppliedAudioSource> {
        self.locator_source_for_action(true)
    }

    pub(super) fn locator_source_for_action(
        &self,
        interactive_play: bool,
    ) -> Option<hex_audio::prepared::AppliedAudioSource> {
        let committed = self.locators.committed.as_ref()?;
        if self.locators.adds.is_empty()
            || !self.locator_committed_action_available()
            || self.document_audio_mode != hex_session::DocumentAudioMode::Resident
            || !self.pending_audio_commands.is_empty()
            || self.seek_pending()
            || self.stopped_playback_restore.is_some()
            || self.section_install_pending()
            || self.settings.tuning.client.snapshot().is_some()
        {
            return None;
        }
        let audio = self.audio.as_ref()?;
        let status = audio.control.status()?;
        let playing = status.playing.load(Acquire);
        if self.desired_playing(playing) != playing
            || status.device_fault.load(Acquire)
            || status.render_fault.load(Acquire)
            || status.shutdown_complete.load(Acquire)
            || status.stopped_edit_pending()
        {
            return None;
        }
        let observation = self.pending_publication_observation.as_ref()?;
        let crate::EditorPublicationState::ResidentSubmitted {
            audio_revision,
            metadata,
            ..
        } = &observation.state
        else {
            return None;
        };
        let source = status.recording_source()?;
        if observation.output_identity != source.output_identity
            || audio.control.output_identity() != Some(source.output_identity)
            || observation.context != source.context
            || metadata.context != source.context
            || committed.audio_authoring_revision(source.context) != Some(committed.revision)
            || *audio_revision != source.audio_revision
            // Interactive PlayFrom resets and chases the current runtime.
            // Running keyboard seeks still need stateful preparation.
            || (metadata.requires_preroll && playing && !interactive_play)
            || metadata.clip_preview.is_some()
            || metadata.device_preview.is_some()
            || metadata.tuning_preview.is_some()
            || metadata.seek_request.is_some()
            || metadata.stopped_restore
            || metadata.computer_keyboard_request.is_some()
            || metadata.gain_recording_track.is_some()
            || status.recording_source() != Some(source)
        {
            return None;
        }
        Some(source)
    }

    pub(super) fn locator_controls_available(&self) -> bool {
        self.locators.pending.is_none()
            && self.locators.name.is_none()
            && self.context_menu.is_none()
            && !self.settings.theme.open
            && !self.settings.tuning.open
    }

    pub(in crate::editor) fn retire_abandoned_locator_adds(&mut self) -> bool {
        let before = self.locators.adds.len();
        self.locators.adds.retain(|add| {
            !add.receipt.is_queued()
                || self
                    .pending_requests
                    .iter()
                    .chain(self.device_panel.gesture_barriers.iter())
                    .chain(
                        self.recovery
                            .deferred_edit
                            .as_ref()
                            .map(|edit| edit.request.as_ref()),
                    )
                    .any(|request| add.matches(request))
        });
        self.locators.adds.len() != before
    }
}
