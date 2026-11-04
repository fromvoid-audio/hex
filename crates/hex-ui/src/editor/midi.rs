//! Canonical MIDI clip authoring commands and note parameter edits.
use super::EditorView;
use hex_project::{BeatTime, ClipId, Command, MidiSource, NoteId};
use hex_session::service::Request;

impl EditorView {
    pub(super) fn edit_midi_source(
        &mut self,
        clip_id: ClipId,
        revision: u64,
        generation: u64,
        field: super::midi_source_controls::SourceField,
        direction: i8,
    ) {
        if self.edit_pending()
            || self.project_generation != generation
            || self.detail_clip_id() != Some(clip_id)
            || self
                .state
                .as_ref()
                .is_none_or(|state| state.revision != revision)
        {
            return;
        }
        let Some(source) = self
            .state
            .as_ref()
            .and_then(|state| state.project.midi_source(clip_id))
        else {
            return;
        };
        let arrangement = self
            .state
            .as_ref()
            .and_then(|state| state.project.midi_clip(clip_id));
        if field == super::midi_source_controls::SourceField::ArrangementLength
            && arrangement.is_none()
        {
            return;
        }
        let source_values = super::midi_source_controls::SourceValues::from_source(
            source,
            arrangement.map_or(0, |clip| clip.start().raw()),
            arrangement.map_or(source.source_length().raw(), |clip| {
                clip.arrangement_length().raw()
            }),
        );
        let latest_note_end =
            if field == super::midi_source_controls::SourceField::SourceLength && direction < 0 {
                source
                    .notes()
                    .iter()
                    .filter_map(|note| note.start().raw().checked_add(note.duration().raw()))
                    .max()
                    .unwrap_or(0)
            } else {
                0
            };
        let step = self.midi_snap_grid.movement_step();
        let changed = match super::midi_source_controls::adjust(
            source_values,
            field,
            direction,
            step,
            latest_note_end,
        ) {
            Ok(Some(changed)) => changed,
            Ok(None) => return,
            Err(error) => {
                self.error = Some(error.into());
                return;
            }
        };
        let Some(command) = self.state.as_ref().and_then(|state| {
            super::midi_source_controls::edit_command(&state.project, clip_id, changed)
        }) else {
            return;
        };
        self.enqueue(Request::Edit(command));
    }

    /// Whole-clip selection for workspace actions; never infer it from the inspector.
    pub(super) fn selected_midi_clip_data(&self) -> Option<ClipId> {
        let state = self.state.as_ref()?;
        let id = self.selected_clip?;
        state.project.midi_source(id).map(|_| id)
    }

    /// Current inspector source, including a retained non-selection context.
    pub(super) fn detail_midi_clip_data(&self) -> Option<ClipId> {
        let id = self.detail_clip_id()?;
        self.state.as_ref()?.project.midi_source(id).map(|_| id)
    }

    pub(super) fn selected_arrangement_midi_clip_data(&self) -> Option<ClipId> {
        let state = self.state.as_ref()?;
        let id = self.selected_clip?;
        state.project.midi_clip(id).map(|_| id)
    }

    pub(super) fn add_midi_track(&mut self) {
        if self.edit_pending() {
            return;
        }
        self.insert_default_track(self.new_track_insertion_anchor());
    }

    pub(super) fn add_midi_clip(&mut self) {
        if self.edit_pending() {
            return;
        }
        if self.seek_pending() {
            self.error =
                Some("seek pending · wait for the cursor before adding a midi clip".into());
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(track) = self.selected_track.and_then(|id| state.project.track(id)) else {
            self.error = Some("select a track before adding a midi clip".into());
            return;
        };
        let frame = self.audio_status().map_or(self.offline_cursor, |s| {
            s.position.load(std::sync::atomic::Ordering::Relaxed)
        });
        let Ok(beat) = state.project.tempo_map().frame_to_beat(frame) else {
            self.error = Some("cursor exceeds the musical beat range".into());
            return;
        };
        let Ok(start) = beat.snap_to(BeatTime::ONE) else {
            self.error = Some("could not snap the cursor to a beat".into());
            return;
        };
        let Some(end) = start.raw().checked_add(4 * BeatTime::ONE.raw()) else {
            self.error = Some("midi clip would exceed the beat range".into());
            return;
        };
        let _ = end;
        self.piano_beat_start = BeatTime::ZERO;
        self.piano_beat_span = BeatTime::from_raw(4 * BeatTime::ONE.raw());
        self.midi_selection.clear();
        self.enqueue(Request::Edit(Command::PlaceMidiClip {
            track: track.id(),
            start,
            length: BeatTime::from_raw(4 * BeatTime::ONE.raw()),
        }));
    }

    pub(super) fn nudge_midi_clip(&mut self, clip: ClipId, right: bool, platform: bool) {
        if self.edit_pending() {
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(item) = state.project.midi_clip(clip) else {
            return;
        };
        let Some((grid, viewport, width)) = self.nudge_grid_context(platform) else {
            return;
        };
        let start = if grid.is_off() {
            if right {
                item.start().raw().checked_add(BeatTime::ONE.raw())
            } else {
                Some(item.start().raw().saturating_sub(BeatTime::ONE.raw()))
            }
        } else {
            grid.adjacent_beat(&state.project, item.start(), viewport, width, right)
                .map(BeatTime::raw)
        };
        let Some(start) = start else {
            if grid.is_off() {
                self.error = Some("midi clip move exceeds the beat range".into());
            }
            return;
        };
        if start.checked_add(item.length().raw()).is_none() {
            self.error = Some("midi clip move exceeds the beat range".into());
            return;
        }
        let Some(track) = state
            .project
            .tracks()
            .iter()
            .find(|t| t.midi_clips().iter().any(|item| item.id() == clip))
            .map(|t| t.id())
        else {
            return;
        };
        self.enqueue(Request::Edit(Command::MoveMidiClip {
            clip,
            track,
            start: BeatTime::from_raw(start),
        }));
    }

    pub(super) fn remove_selected_note(&mut self) {
        if self.edit_pending() {
            return;
        }
        let Some(clip) = self.detail_midi_clip_data() else {
            return;
        };
        let notes = self.midi_selection.ids().to_vec();
        if notes.is_empty() {
            return;
        }
        self.finish_live_midi_marquees();
        self.midi_note_gesture = None;
        self.midi_group_gesture = None;
        let accepted = if notes.len() == 1 {
            self.enqueue(Request::Edit(Command::RemoveMidiNote(notes[0])))
        } else {
            self.enqueue(Request::Edit(Command::RemoveMidiNotes { clip, notes }))
        };
        if accepted {
            self.midi_selection.clear();
        }
    }

    pub(super) fn select_all_midi_notes(&mut self) {
        if self.edit_pending() {
            return;
        }
        self.finish_live_midi_marquees();
        let Some(clip) = self.midi_clip() else {
            return;
        };
        let mut notes = Vec::new();
        if notes.try_reserve_exact(clip.notes().len()).is_err() {
            self.error = Some("could not reserve select-all midi notes".into());
            return;
        }
        notes.extend(clip.notes().iter().map(|note| note.id()));
        self.midi_note_gesture = None;
        self.midi_group_gesture = None;
        if let Err(error) = self.midi_selection.select_all(notes) {
            self.error = Some(error.into());
        } else {
            self.clear_midi_time_selection();
            self.audition_midi_selection();
        }
    }

    /// With a note selection, arrows retain musical nudge behavior. Otherwise they
    /// move the local insertion cursor, never transport or canonical project time.
    /// Grid-off navigation uses the existing sixteenth-note movement step.
    pub(super) fn navigate_piano_cursor(&mut self, right: bool) {
        if self.midi_step.enabled && right {
            self.enter_midi_step();
            return;
        }
        if !self.midi_step.enabled && !self.midi_selection.is_empty() {
            self.nudge_selected_note(right);
            return;
        }
        if self.edit_pending() || self.detail_midi_clip_data().is_none() {
            return;
        }
        self.clear_midi_time_selection();
        self.cancel_midi_note_gesture();
        let current = self.midi_marker.unwrap_or(self.piano_beat_start.raw());
        let step = self.midi_snap_grid.movement_step();
        let next = if right {
            let Some(next) = current.checked_add(step).filter(|value| *value < u64::MAX) else {
                return;
            };
            next
        } else {
            current.saturating_sub(step)
        };
        self.midi_marker = Some(next);
        self.reveal_piano_cursor(next);
    }

    pub(super) fn reveal_piano_cursor(&mut self, next: u64) {
        let step = self.midi_snap_grid.movement_step();
        // Keep the insertion position visible without changing zoom or source extent.
        let span = self.piano_beat_span.raw().max(1);
        let start = self.piano_beat_start.raw();
        if next < start {
            self.piano_beat_start = BeatTime::from_raw(next);
        } else if next >= start.saturating_add(span) {
            self.piano_beat_start = BeatTime::from_raw(
                next.saturating_sub(span.saturating_sub(step.min(span / 2).max(1))),
            )
            .min(BeatTime::from_raw(u64::MAX - span));
        }
    }

    pub(super) fn nudge_selected_note(&mut self, right: bool) {
        if self.midi_selection.len() > 1 {
            let step = i128::from(self.midi_snap_grid.movement_step());
            self.nudge_selected_group(if right { step } else { -step }, 0);
            return;
        }
        if self.midi_selection.len() != 1 {
            return;
        }
        let Some(note) = self.midi_selection.primary() else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        self.update_note_field(
            note,
            state.revision,
            self.project_generation,
            NoteField::Onset(if right { 1 } else { -1 }),
        );
    }

    pub(super) fn transpose_selected_note(&mut self, up: bool) {
        self.transpose_selected_note_by(if up { 1 } else { -1 });
    }

    pub(super) fn transpose_selected_note_by(&mut self, semitones: i16) {
        if self.midi_selection.len() > 1 {
            self.nudge_selected_group(0, semitones);
            return;
        }
        if self.midi_selection.len() != 1 {
            return;
        }
        let Some(note) = self.midi_selection.primary() else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        self.update_note_field(
            note,
            state.revision,
            self.project_generation,
            NoteField::Pitch(semitones),
        );
    }

    pub(super) fn adjust_selected_group_current(
        &mut self,
        parameter: super::midi_group_drag::GroupParameter,
    ) {
        let Some(clip) = self.detail_midi_clip_data() else {
            return;
        };
        let Some(revision) = self.state.as_ref().map(|state| state.revision) else {
            return;
        };
        self.adjust_selected_group(
            parameter,
            clip,
            revision,
            self.project_generation,
            self.midi_selection.epoch(),
        );
    }

    fn nudge_selected_group(&mut self, time_delta: i128, pitch_delta: i16) {
        if self.edit_pending()
            || self.midi_selection.len() < 2
            || self.midi_selection.len() > hex_project::MAX_NOTES
        {
            return;
        }
        let Some(clip_id) = self.detail_midi_clip_data() else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let revision = state.revision;
        let generation = self.project_generation;
        let clip_length = match state.project.midi_source(clip_id) {
            Some(clip) => clip.source_length().raw(),
            None => return,
        };
        let selected = self.midi_selection.shared_ids();
        let mut originals = Vec::new();
        if originals.try_reserve_exact(selected.len()).is_err() {
            self.error = Some("could not reserve selected midi notes".into());
            return;
        }
        let Some(clip) = state.project.midi_source(clip_id) else {
            return;
        };
        for note in clip.notes() {
            if selected.binary_search(&note.id()).is_ok() {
                originals.push(super::midi_group_drag::GroupNote {
                    id: note.id(),
                    active: note.active(),
                    start: note.start().raw(),
                    duration: note.duration().raw(),
                    pitch: note.pitch(),
                    channel: note.channel(),
                    velocity: note.velocity(),
                });
            }
        }
        if originals.len() != selected.len()
            || self.project_generation != generation
            || self
                .state
                .as_ref()
                .is_none_or(|latest| latest.revision != revision)
            || self.detail_midi_clip_data() != Some(clip_id)
            || self.edit_pending()
        {
            self.error = Some("selected midi notes changed · nudge canceled".into());
            return;
        }
        self.finish_live_midi_marquees();
        self.cancel_midi_note_gesture();
        let updates = match super::midi_group_drag::nudge_updates(
            &originals,
            u64::MAX,
            time_delta,
            pitch_delta,
        ) {
            Ok(Some(updates)) => updates,
            Ok(None) => return,
            Err(error) => {
                self.error = Some(error.into());
                return;
            }
        };
        let command = if updates.iter().any(|note| {
            note.start
                .raw()
                .checked_add(note.duration.raw())
                .is_none_or(|end| end > clip_length)
        }) {
            Command::UpdateMidiNotesWithSourceExtent {
                clip: clip_id,
                notes: updates,
            }
        } else {
            Command::UpdateMidiNotes {
                clip: clip_id,
                notes: updates,
            }
        };
        self.enqueue(Request::Edit(command));
    }

    pub(super) fn adjust_selected_group(
        &mut self,
        parameter: super::midi_group_drag::GroupParameter,
        expected_clip: ClipId,
        expected_revision: u64,
        expected_generation: u64,
        expected_selection_epoch: u64,
    ) {
        if self.edit_pending()
            || self.midi_selection.len() < 2
            || self.midi_selection.len() > hex_project::MAX_NOTES
            || self.detail_midi_clip_data() != Some(expected_clip)
            || self.project_generation != expected_generation
            || self.midi_selection.epoch() != expected_selection_epoch
            || self
                .state
                .as_ref()
                .is_none_or(|state| state.revision != expected_revision)
        {
            return;
        }
        let clip_id = expected_clip;
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let revision = expected_revision;
        let generation = expected_generation;
        let selection_epoch = expected_selection_epoch;
        let clip_length = match state.project.midi_source(clip_id) {
            Some(clip) => clip.source_length().raw(),
            None => return,
        };
        let selected = self.midi_selection.shared_ids();
        let Some(clip) = state.project.midi_source(clip_id) else {
            return;
        };
        let originals = match super::midi_group_drag::capture_selected_notes(
            clip.notes()
                .iter()
                .map(|note| super::midi_group_drag::GroupNote {
                    id: note.id(),
                    active: note.active(),
                    start: note.start().raw(),
                    duration: note.duration().raw(),
                    pitch: note.pitch(),
                    channel: note.channel(),
                    velocity: note.velocity(),
                }),
            &selected,
        ) {
            Ok(originals) => originals,
            Err(error) => {
                self.error = Some(error.into());
                return;
            }
        };
        if self.project_generation != generation
            || self
                .state
                .as_ref()
                .is_none_or(|latest| latest.revision != revision)
            || self.detail_midi_clip_data() != Some(clip_id)
            || self.midi_selection.epoch() != selection_epoch
            || self.midi_selection.shared_ids().as_ref() != selected.as_ref()
            || self.edit_pending()
        {
            self.error = Some("selected midi notes changed · edit canceled".into());
            return;
        }
        let grid_step = self.midi_snap_grid.movement_step();
        self.finish_live_midi_marquees();
        self.cancel_midi_note_gesture();
        let updates = match super::midi_group_drag::parameter_updates(
            &originals,
            clip_length,
            parameter,
            grid_step,
        ) {
            Ok(Some(updates)) => updates,
            Ok(None) => return,
            Err(error) => {
                self.error = Some(error.into());
                return;
            }
        };
        if self.project_generation != generation
            || self
                .state
                .as_ref()
                .is_none_or(|latest| latest.revision != revision)
            || self.detail_midi_clip_data() != Some(clip_id)
            || self.midi_selection.epoch() != selection_epoch
            || self.midi_selection.shared_ids().as_ref() != selected.as_ref()
            || self.edit_pending()
        {
            self.error = Some("selected midi notes changed · edit canceled".into());
            return;
        }
        self.enqueue(Request::Edit(Command::UpdateMidiNotes {
            clip: clip_id,
            notes: updates,
        }));
    }

    pub(super) fn edit_pending(&self) -> bool {
        (self.gain_recorder.blocks_edits() || self.midi_recorder.blocks_edits())
            || self.automation_view.devices.pointer_active()
            || self.sampler_parameter_pending()
            || self.opening_project
            || self.close_prompt_open
            || self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
            || self.device_gesture_pending()
    }

    pub(super) fn midi_clip(&self) -> Option<&MidiSource> {
        self.state
            .as_ref()?
            .project
            .midi_source(self.detail_clip_id()?)
    }

    pub(super) fn update_note_field(
        &mut self,
        note_id: NoteId,
        revision: u64,
        generation: u64,
        field: NoteField,
    ) {
        if self.midi_selection.len() != 1
            || self.midi_selection.primary() != Some(note_id)
            || self.edit_pending()
            || self.project_generation != generation
            || self.state.as_ref().is_none_or(|s| s.revision != revision)
        {
            return;
        }
        let Some((old_start, old_duration, old_pitch, channel, old_velocity)) = self
            .state
            .as_ref()
            .and_then(|s| s.project.midi_note(note_id))
            .map(|n| {
                (
                    n.start().raw(),
                    n.duration().raw(),
                    n.pitch(),
                    n.channel(),
                    n.velocity(),
                )
            })
        else {
            self.midi_selection.clear();
            return;
        };
        let mut start = old_start;
        let mut duration = old_duration;
        let mut pitch = old_pitch;
        let mut velocity = old_velocity;
        let step = self.midi_snap_grid.movement_step();
        match field {
            NoteField::Pitch(delta) => pitch = (i16::from(pitch) + delta).clamp(0, 127) as u8,
            NoteField::Onset(delta) => {
                start = if delta < 0 {
                    start.saturating_sub(step)
                } else {
                    start.saturating_add(step).min(u64::MAX - duration)
                }
            }
            NoteField::Duration(delta) => {
                duration = if delta < 0 {
                    duration
                        .saturating_sub(step)
                        .max(self.midi_snap_grid.minimum_gate().min(duration))
                } else {
                    let available = self
                        .midi_clip()
                        .map_or(0, |clip| clip.source_length().raw().saturating_sub(start));
                    duration.saturating_add(step).min(available)
                }
            }
            NoteField::Velocity(delta) => {
                velocity = (i16::from(velocity) + delta).clamp(1, 127) as u8
            }
        }
        let Some(clip) = self.midi_clip() else {
            return;
        };
        if !clip.notes().iter().any(|item| item.id() == note_id) {
            return;
        }
        let Some(end) = start.checked_add(duration) else {
            self.error = Some("midi note end exceeds the beat range".into());
            return;
        };
        let grows_source = end > clip.source_length().raw();
        if grows_source && !matches!(field, NoteField::Onset(_)) {
            self.error = Some("edit would move the note outside its clip".into());
            return;
        }
        let Some(clip_id) = self.detail_midi_clip_data() else {
            return;
        };
        self.finish_live_midi_marquees();
        let command = if grows_source {
            Command::UpdateMidiNotesWithSourceExtent {
                clip: clip_id,
                notes: vec![hex_project::MidiNoteUpdate {
                    note: note_id,
                    start: BeatTime::from_raw(start),
                    duration: BeatTime::from_raw(duration),
                    pitch,
                    channel,
                    velocity,
                }],
            }
        } else {
            Command::UpdateMidiNote {
                note: note_id,
                start: BeatTime::from_raw(start),
                duration: BeatTime::from_raw(duration),
                pitch,
                channel,
                velocity,
            }
        };
        self.enqueue(Request::Edit(command));
    }
}

#[derive(Clone, Copy)]
pub(super) enum NoteField {
    Pitch(i16),
    Onset(i8),
    Duration(i8),
    Velocity(i16),
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_command_ui_tests.rs"]
mod tests;
