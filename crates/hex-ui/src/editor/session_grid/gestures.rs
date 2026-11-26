//! Session grid navigation, scene/cell selection, launch and copy/edit gestures.
use super::*;

impl EditorView {
    pub(in crate::editor) fn navigate_session(
        &mut self,
        direction: crate::editor::session_navigation::Direction,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if self.persistence.opening_project || self.persistence.close_prompt_open {
            return;
        }
        let selection = self.state.as_ref().and_then(|state| {
            crate::editor::session_navigation::resolve_visible_navigation(
                &state.project,
                &self.arrangement_focus.projection.console_indices,
                self.session_grid.selected_cell,
                self.session_grid.selected_scene,
                direction,
            )
        });
        match selection {
            Some(crate::editor::session_navigation::Selection::Scene(scene)) => {
                if self.focus_empty_details(window, cx) {
                    self.advance_selection_intent();
                    self.select_session_scene(scene);
                }
            }
            Some(crate::editor::session_navigation::Selection::Cell { track, scene }) => {
                let content = self
                    .state
                    .as_ref()
                    .and_then(|state| state.project.session_cell(track, scene))
                    .and_then(|content| match content {
                        SessionCellContent::Midi(clip) => Some(Some(clip.id())),
                        SessionCellContent::Audio(_) => Some(None),
                        SessionCellContent::Stop => None,
                    });
                match content {
                    Some(Some(clip)) => {
                        if self.focus_clip_details(window, cx) {
                            self.advance_selection_intent();
                            self.select_session_midi(track, scene, clip);
                        }
                    }
                    Some(None) => {
                        if self.focus_clip_details(window, cx) {
                            self.advance_selection_intent();
                            self.select_session_cell(track, scene);
                        }
                    }
                    None => {
                        self.focus_track_details(window);
                        self.advance_selection_intent();
                        self.select_session_cell(track, scene);
                    }
                }
            }
            None => {}
        }
    }

    pub(in crate::editor) fn launch_session_selection(&mut self) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let revision = state.revision;
        if self.session_grid.selected_cell.is_some_and(|(track, _)| {
            !self
                .arrangement_focus
                .projection
                .console_indices
                .iter()
                .any(|index| {
                    state
                        .project
                        .tracks()
                        .get(*index)
                        .is_some_and(|item| item.id() == track)
                })
        }) {
            return;
        }
        let Some(target) = crate::editor::session_navigation::resolve_enter_target(
            &state.project,
            self.session_grid.selected_cell,
            self.session_grid.selected_scene,
        ) else {
            return;
        };
        match target {
            crate::editor::session_navigation::EnterTarget::LaunchClip { track, clip } => {
                self.launch_clip(track, clip, revision, self.session_grid.quantization);
            }
            crate::editor::session_navigation::EnterTarget::StopTrack(track) => {
                self.queue_session_command(
                    SessionPlaybackCommand::StopTrack {
                        track,
                        quantization: self.session_grid.quantization,
                    },
                    revision,
                );
            }
            crate::editor::session_navigation::EnterTarget::LaunchScene(scene) => {
                self.launch_scene(scene, revision, self.session_grid.quantization);
            }
        }
    }

    pub(in crate::editor) fn duplicate_selected_scene(&mut self) {
        let Some((scene, revision, generation)) =
            self.session_grid.selected_scene.and_then(|scene| {
                self.state
                    .as_ref()
                    .map(|state| (scene, state.revision, self.project_generation))
            })
        else {
            return;
        };
        self.duplicate_scene_if_current(scene, revision, generation);
    }

    pub(in crate::editor) fn move_selected_scene(
        &mut self,
        direction: crate::editor::session_navigation::Direction,
    ) {
        let Some((scene, revision, generation)) =
            self.session_grid.selected_scene.and_then(|scene| {
                self.state
                    .as_ref()
                    .map(|state| (scene, state.revision, self.project_generation))
            })
        else {
            return;
        };
        self.move_scene_if_current(scene, direction, revision, generation);
    }

    pub(in crate::editor) fn duplicate_scene_if_current(
        &mut self,
        scene: SceneId,
        revision: u64,
        generation: u64,
    ) {
        if self.scene_edit_is_current(scene, revision, generation)
            && self.enqueue(Request::Edit(Command::DuplicateScene(scene)))
        {
            self.session_grid.pending_scene_duplicate =
                Some(crate::editor::PendingSceneDuplicate {
                    source_scene: scene,
                    project_generation: generation,
                    select_on_completion: true,
                });
        }
    }

    pub(in crate::editor) fn cancel_scene_duplicate_selection(&mut self) {
        if let Some(intent) = &mut self.session_grid.pending_scene_duplicate {
            intent.select_on_completion = false;
        }
    }

    pub(in crate::editor) fn move_scene_if_current(
        &mut self,
        scene: SceneId,
        direction: crate::editor::session_navigation::Direction,
        revision: u64,
        generation: u64,
    ) {
        let Some(target) = self.state.as_ref().and_then(|state| {
            crate::editor::session_navigation::resolve_scene_move(&state.project, scene, direction)
        }) else {
            return;
        };
        if self.scene_edit_is_current(scene, revision, generation) {
            self.enqueue(Request::Edit(Command::MoveScene {
                scene: target.scene,
                before: target.before,
            }));
        }
    }

    pub(super) fn scene_edit_is_current(
        &self,
        scene: SceneId,
        revision: u64,
        generation: u64,
    ) -> bool {
        self.mode == EditorMode::Session
            && self.session_grid.selected_cell.is_none()
            && self.session_grid.selected_scene == Some(scene)
            && self.project_generation == generation
            && self.state.as_ref().is_some_and(|state| {
                state.revision == revision
                    && state.project.scenes().iter().any(|item| item.id() == scene)
            })
            && !self.edit_pending()
            && self.audio_pub.recovery.generation().is_none()
            && self.persistence.pending_open_path.is_none()
            && !self.persistence.save_dialog_open
            && !self.persistence.export_dialog_open
            && !self.persistence.export_pending
            && !self.persistence.saving_project
    }

    pub(in crate::editor) fn session_track_page(
        &self,
        project: &hex_project::Project,
        visible_columns: usize,
    ) -> Arc<TrackPage> {
        self.pending_console_reorder_page().unwrap_or_else(|| {
            let visible = &self.arrangement_focus.projection.console_indices;
            Arc::new(TrackPage::capture_with_proposals(
                project,
                visible,
                self.session_grid.track_start,
                visible_columns,
                &self.track_proposal_anchors(),
            ))
        })
    }

    pub(in crate::editor) fn reveal_session_track_proposal(&mut self, before: Option<TrackId>) {
        if self.mode != EditorMode::Session {
            return;
        }
        let Some(state) = &self.state else {
            return;
        };
        let visible = &self.arrangement_focus.projection.console_indices;
        let Some(index) = anchor_ordinal(state.project.tracks(), visible, before) else {
            return;
        };
        let columns = self.session_grid.visible_columns.max(1);
        let range = crate::editor::console_track_page::visible_track_range(
            visible.len(),
            self.session_grid.track_start,
            columns,
        );
        if index < range.start || index > range.end {
            self.session_grid.track_start = index.min(visible.len().saturating_sub(columns));
        }
    }

    pub(in crate::editor) fn retain_session_track_proposal_page(
        &mut self,
        proposal: TrackProposal,
    ) {
        let Some(state) = &self.state else {
            return;
        };
        let page = self.session_track_page(&state.project, self.session_grid.visible_columns);
        if page.proposal_index(proposal).is_some() {
            self.session_grid.track_start = page.range.start;
        }
    }

    pub(super) fn session_callback_current(&self, revision: u64, generation: u64) -> bool {
        !self.edit_pending()
            && self.project_generation == generation
            && self
                .state
                .as_ref()
                .is_some_and(|state| state.revision == revision)
    }

    pub(super) fn cell_still_contains(
        &self,
        track: TrackId,
        scene: SceneId,
        expected: CellExpectation,
    ) -> bool {
        self.state.as_ref().is_some_and(|state| {
            match (expected, state.project.session_cell(track, scene)) {
                (CellExpectation::Midi(expected), Some(SessionCellContent::Midi(current))) => {
                    current.id() == expected
                }
                (CellExpectation::Audio(expected), Some(SessionCellContent::Audio(current))) => {
                    current.id() == expected
                }
                (CellExpectation::Stop, Some(SessionCellContent::Stop)) => true,
                (CellExpectation::Empty, None) => true,
                _ => false,
            }
        })
    }

    pub(in crate::editor) fn select_session_midi(
        &mut self,
        track: TrackId,
        scene: SceneId,
        clip: ClipId,
    ) {
        self.cancel_duplicate_chain();
        if self.detail_clip_id() != Some(clip) {
            self.zoom_fit.clear_clip();
        }
        self.cancel_scene_duplicate_selection();
        if let Some(selected) = self.selected_clip
            && self.state.as_ref().is_some_and(|state| {
                state.project.midi_clip(selected).is_some()
                    || state
                        .project
                        .tracks()
                        .iter()
                        .any(|track| track.clips().iter().any(|clip| clip.id() == selected))
            })
        {
            self.session_grid.copy_source = Some(selected);
        }
        self.session_grid.selected_scene = Some(scene);
        self.session_grid.selected_cell = Some((track, scene));
        self.selected_track = Some(track);
        self.selected_clip = Some(clip);
        self.mode = EditorMode::Session;
        self.midi_editor.selection.clear();
        self.cancel_midi_note_gesture();
        self.cancel_midi_stroke();
        self.midi_editor.group_gesture = None;
        self.midi_editor.hit_cycle = None;
        self.midi_editor.piano_beat_start = 0;
        self.fit_clip_editor_to_selected_clip();
        self.reveal_session_position(Some(track), Some(scene));
    }

    pub(in crate::editor) fn select_session_scene(&mut self, scene: SceneId) {
        self.cancel_duplicate_chain();
        self.zoom_fit.clear_clip();
        self.cancel_scene_duplicate_selection();
        if let Some(selected) = self.selected_clip
            && self.state.as_ref().is_some_and(|state| {
                state.project.midi_clip(selected).is_some()
                    || state
                        .project
                        .tracks()
                        .iter()
                        .any(|track| track.clips().iter().any(|clip| clip.id() == selected))
            })
        {
            self.session_grid.copy_source = Some(selected);
        }
        self.session_grid.selected_scene = Some(scene);
        self.session_grid.selected_cell = None;
        self.selected_track = None;
        self.selected_clip = None;
        self.mode = EditorMode::Session;
        self.midi_editor.selection.clear();
        self.cancel_midi_note_gesture();
        self.cancel_midi_stroke();
        self.midi_editor.group_gesture = None;
        self.midi_editor.hit_cycle = None;
        self.reveal_session_position(None, Some(scene));
    }

    pub(in crate::editor) fn select_session_cell(&mut self, track: TrackId, scene: SceneId) {
        self.cancel_duplicate_chain();
        if self.session_grid.selected_cell != Some((track, scene)) {
            self.zoom_fit.clear_clip();
        }
        self.cancel_scene_duplicate_selection();
        if let Some(selected) = self.selected_clip
            && self.state.as_ref().is_some_and(|state| {
                state.project.midi_clip(selected).is_some()
                    || state
                        .project
                        .tracks()
                        .iter()
                        .any(|track| track.clips().iter().any(|clip| clip.id() == selected))
            })
        {
            self.session_grid.copy_source = Some(selected);
        }
        self.session_grid.selected_scene = Some(scene);
        self.session_grid.selected_cell = Some((track, scene));
        self.selected_track = Some(track);
        self.selected_clip = None;
        self.mode = EditorMode::Session;
        self.midi_editor.selection.clear();
        self.cancel_midi_note_gesture();
        self.cancel_midi_stroke();
        self.midi_editor.group_gesture = None;
        self.midi_editor.hit_cycle = None;
        self.reveal_session_position(Some(track), Some(scene));
    }

    pub(in crate::editor) fn reveal_session_position(
        &mut self,
        track: Option<TrackId>,
        scene: Option<SceneId>,
    ) {
        let positions = self.state.as_ref().map(|state| {
            (
                track.and_then(|track| {
                    self.arrangement_focus
                        .projection
                        .console_indices
                        .iter()
                        .position(|index| {
                            state
                                .project
                                .tracks()
                                .get(*index)
                                .is_some_and(|item| item.id() == track)
                        })
                }),
                scene.and_then(|scene| {
                    state
                        .project
                        .scenes()
                        .iter()
                        .position(|item| item.id() == scene)
                }),
            )
        });
        let Some((track_index, scene_index)) = positions else {
            return;
        };
        if let Some(index) = track_index.filter(|_| !self.track_reorder_active()) {
            let page_size = self.session_grid.visible_columns.max(1);
            if index < self.session_grid.track_start
                || index >= self.session_grid.track_start.saturating_add(page_size)
            {
                self.session_grid.track_start = index / page_size * page_size;
            }
        }
        if let Some(index) = scene_index {
            self.session_grid
                .scene_scroll
                .scroll_to_item(index, gpui::ScrollStrategy::Top);
        }
    }

    pub(in crate::editor) fn set_mode(&mut self, mode: EditorMode) {
        if self.mode != mode {
            if self.mode == EditorMode::Arrangement {
                self.cancel_pending_loop_focus();
            }
            self.section_loop_key = Default::default();
            self.cancel_arrangement_pan();
            self.cancel_active_browser_audio_drag();
            if self.interaction.time_selection_gesture.is_some()
                || self.interaction.time_range_move_gesture.is_some()
            {
                self.cancel_time_selection_gesture();
            }
            self.cancel_locked_play_start_drag();
            self.cancel_scene_duplicate_selection();
        }
        if mode == EditorMode::Session
            && let Some(selected) = self.selected_clip
            && self
                .state
                .as_ref()
                .is_some_and(|state| state.project.midi_clip(selected).is_some())
        {
            self.session_grid.copy_source = Some(selected);
        }
        self.mode = mode;
    }

    pub(super) fn launch_clip(
        &mut self,
        _track: TrackId,
        clip: ClipId,
        revision: u64,
        quantization: LaunchQuantization,
    ) {
        self.queue_session_launch_batch(
            SessionPlaybackCommand::LaunchClip { clip, quantization },
            revision,
        );
    }

    pub(super) fn launch_scene(
        &mut self,
        scene: SceneId,
        revision: u64,
        quantization: LaunchQuantization,
    ) {
        self.queue_session_launch_batch(
            SessionPlaybackCommand::LaunchScene {
                scene,
                quantization,
            },
            revision,
        );
    }

    pub(super) fn create_session_midi(&mut self, track: TrackId, scene: SceneId) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        if self.edit_pending() || state.project.track(track).is_none() {
            return;
        }
        if !matches!(
            state.project.session_cell(track, scene),
            None | Some(SessionCellContent::Stop)
        ) {
            self.error = Some("session cell is occupied".into());
            return;
        }
        self.enqueue(Request::Edit(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "midi clip".into(),
            source_length: BeatTime::from_quarters(4).unwrap_or(BeatTime::ONE),
        }));
    }

    pub(super) fn copy_arrangement_midi_to_session(&mut self, track: TrackId, scene: SceneId) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(source) = self
            .session_grid
            .copy_source
            .or(self.selected_clip)
            .filter(|clip| state.project.midi_clip(*clip).is_some())
        else {
            self.error = Some("select an arrangement midi clip to copy".into());
            return;
        };
        let source_track = state.project.tracks().iter().find_map(|track| {
            track
                .midi_clips()
                .iter()
                .any(|clip| clip.id() == source)
                .then_some(track.id())
        });
        if source_track != Some(track) {
            self.error = Some("copy into the session cell on the source track".into());
            return;
        }
        if self.edit_pending()
            || state.project.track(track).is_none()
            || !matches!(
                state.project.session_cell(track, scene),
                None | Some(SessionCellContent::Stop)
            )
        {
            self.error = Some("session destination cell is unavailable".into());
            return;
        }
        self.enqueue(Request::Edit(Command::CopyArrangementMidiToSession {
            source,
            scene,
        }));
    }

    pub(super) fn copy_arrangement_audio_to_session(&mut self, track: TrackId, scene: SceneId) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(source) = self
            .session_grid
            .copy_source
            .or(self.selected_clip)
            .filter(|clip| {
                state
                    .project
                    .tracks()
                    .iter()
                    .any(|item| item.clips().iter().any(|source| source.id() == *clip))
            })
        else {
            self.error = Some("select an arrangement audio clip to copy".into());
            return;
        };
        let source_track = state.project.tracks().iter().find_map(|item| {
            item.clips()
                .iter()
                .any(|clip| clip.id() == source)
                .then_some(item.id())
        });
        if source_track != Some(track) {
            self.error = Some("copy audio into the session cell on the source track".into());
            return;
        }
        if self.edit_pending()
            || state.project.track(track).is_none()
            || state.project.session_cell(track, scene).is_some()
        {
            self.error = Some("session destination cell is unavailable".into());
            return;
        }
        self.enqueue(Request::Edit(Command::CopyArrangementAudioClipToSession {
            source,
            scene,
        }));
    }

    pub(super) fn set_session_stop_cell(&mut self, track: TrackId, scene: SceneId) {
        if self.edit_pending() {
            return;
        }
        self.enqueue(Request::Edit(Command::SetSessionStopCell { track, scene }));
    }
}
