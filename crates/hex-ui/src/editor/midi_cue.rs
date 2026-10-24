//! Ruler cue dispatch uses the existing Arrangement seek or accepted Session
//! route command. It never edits source start or paints an optimistic playhead.
use super::EditorView;
use hex_audio::prepared::SessionRouteOwner;
use hex_engine::SessionPlaybackCommand;
use hex_project::{BeatTime, ClipId, Project, SessionCellContent};

fn arrangement_destination(
    project: &Project,
    clip: ClipId,
    source_beat: BeatTime,
) -> Result<Option<u64>, &'static str> {
    let Some(clip) = project.midi_clip(clip) else {
        return Ok(None);
    };
    let playback = clip
        .source_playback()
        .map_err(|_| "midi source timing is invalid")?;
    let occurrence = playback
        .note_occurrences(source_beat, BeatTime::from_raw(1))
        .map_err(|_| "cue is outside the playable midi source")?
        .iter()
        .next()
        .ok_or("cue is outside this clip's playable time")?;
    let absolute = clip
        .start()
        .raw()
        .checked_add(occurrence.timeline_start().raw())
        .ok_or("midi cue position overflow")?;
    project
        .tempo_map()
        .beat_to_frame(BeatTime::from_raw(absolute))
        .map(Some)
        .map_err(|_| "midi cue tempo conversion failed")
}

impl EditorView {
    pub(super) fn request_midi_source_cue(&mut self, clip: ClipId, source_beat: u64) {
        if self.detail_clip_id() != Some(clip) || self.edit_pending() {
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let source_beat = BeatTime::from_raw(source_beat);
        match arrangement_destination(&state.project, clip, source_beat) {
            Ok(Some(frame)) => {
                self.request_seek(frame);
                return;
            }
            Err(message) => {
                self.error = Some(message.into());
                return;
            }
            Ok(None) => {}
        }
        let Some(track) = state.project.session_cells().iter().find_map(|cell| {
            matches!(cell.content(), SessionCellContent::Midi(item) if item.id() == clip)
                .then_some(cell.track())
        }) else {
            return;
        };
        let revision = state.revision;
        let audible = self.audible_playback.as_ref().map(|candidate| {
            (
                candidate.audio_revision,
                candidate.state.revision,
                candidate.project_generation,
            )
        });
        let snapshot = self.audio.as_ref().and_then(|audio| {
            self.session_routes.matching_snapshot(
                audio.control.output_identity()?,
                audible,
                revision,
                self.project_generation,
            )
        });
        let Some(snapshot) = snapshot else {
            self.error = Some("wait for the current clip audio before cueing".into());
            return;
        };
        let ready = self.session_routes.routes_available(
            snapshot,
            &state.project,
            revision,
            self.project_generation,
        );
        let route = self
            .session_routes
            .entry_for_track(snapshot, ready, track.value());
        if !route.is_some_and(|route| {
            route.owner
                == SessionRouteOwner::Session {
                    clip_id: clip.value(),
                }
                && route.active_origin.is_some()
        }) {
            self.error = Some("launch this clip before cueing it".into());
            return;
        }
        self.queue_session_command(
            SessionPlaybackCommand::CueMidiClip {
                track,
                clip,
                source_beat,
                expected_route_generation: snapshot.route_state_generation,
            },
            revision,
        );
    }
}

impl EditorView {
    /// Only accepted, revision-bound playback state supplies this cursor. The
    /// insertion cursor and persisted source-start marker keep their own state.
    pub(super) fn midi_playhead_source_beat(&self) -> Option<u64> {
        use std::sync::atomic::Ordering::Acquire;
        let state = self.state.as_ref()?;
        let clip = self.detail_clip_id()?;
        let audible = self.audible_playback.as_ref()?;
        if audible.project_generation != self.project_generation
            || audible.state.revision != state.revision
        {
            return None;
        }
        let audio = self.audio.as_ref()?;
        let status = audio.control.status()?;
        if status.applied_revision.load(Acquire) != audible.audio_revision
            || status.device_fault.load(Acquire)
            || status.render_fault.load(Acquire)
            || status.shutdown_complete.load(Acquire)
        {
            return None;
        }
        let snapshot = self.session_routes.matching_snapshot(
            audio.control.output_identity()?,
            Some((
                audible.audio_revision,
                state.revision,
                self.project_generation,
            )),
            state.revision,
            self.project_generation,
        )?;
        // Restrict per-frame lookup to the selected owner; never walk every
        // track/clip to paint one piano-roll cursor.
        let track = state.project.track(self.selected_track?)?;
        let placement = track.midi_clips().iter().find(|item| item.id() == clip);
        let source = if let Some(placement) = placement {
            placement.source()
        } else {
            let (selected_track, scene) = self.selected_cell?;
            if selected_track != track.id() {
                return None;
            }
            match state.project.session_cell(selected_track, scene)? {
                SessionCellContent::Midi(item) if item.id() == clip => item.source(),
                _ => return None,
            }
        };
        let elapsed = if let Some(placement) = placement {
            let route = self
                .session_routes
                .entry_for_track(snapshot, true, track.id().value())?;
            if route.owner != SessionRouteOwner::Arrangement {
                return None;
            }
            let absolute = state
                .project
                .tempo_map()
                .frame_to_beat(status.position.load(Acquire))
                .ok()?;
            let elapsed = absolute.raw().checked_sub(placement.start().raw())?;
            if elapsed >= placement.arrangement_length().raw() {
                return None;
            }
            BeatTime::from_raw(elapsed)
        } else {
            let route = self
                .session_routes
                .entry_for_track(snapshot, true, track.id().value())?;
            if route.owner
                != (SessionRouteOwner::Session {
                    clip_id: clip.value(),
                })
            {
                return None;
            }
            let frame = status.performance_frame.load(Acquire);
            let beat = match &audible.observation {
                Some(observation) => observation
                    .activity()
                    .performance_beat_at(frame, snapshot.clock_anchor)
                    .ok()?,
                None => audible
                    .prepared_revision
                    .as_ref()?
                    .midi_performance_beat_at(frame, snapshot.clock_anchor)
                    .ok()?,
            };
            route.active_origin?.elapsed_at(beat)?
        };
        let playback = hex_project::MidiSourcePlayback::new(
            source.source_length(),
            source.source_start(),
            source
                .looping()
                .then_some((source.loop_start(), source.loop_end())),
            source.source_length(),
        )
        .ok()?;
        let phase = playback.source_phase_at_elapsed(elapsed)?;
        // A publication can change during scalar reads; never paint it against
        // another compiled revision's mapping.
        (status.applied_revision.load(Acquire) == audible.audio_revision).then_some(phase.raw())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Tempo};
    fn beat(value: u64) -> BeatTime {
        BeatTime::from_quarters(value).unwrap()
    }
    #[test]
    fn arrangement_cue_projects_intro_repeat_and_absolute_tempo_before_seeking() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack { name: "cue".into() })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: beat(8),
                length: beat(4),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        let source = |length| Command::SetMidiClipSource {
            clip,
            source_length: beat(8),
            source_start: beat(4),
            loop_start: beat(2),
            loop_end: beat(6),
            looping: true,
            length: beat(length),
        };
        editor.execute(source(4)).unwrap();
        editor
            .execute(Command::SetTempoPoint {
                beat: beat(10),
                tempo: Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        assert_eq!(
            arrangement_destination(editor.project(), clip, beat(4)),
            Ok(Some(192_000))
        );
        assert_eq!(
            arrangement_destination(editor.project(), clip, beat(5)),
            Ok(Some(216_000))
        );
        assert_eq!(
            arrangement_destination(editor.project(), clip, beat(3)),
            Ok(Some(288_000))
        );
        assert!(arrangement_destination(editor.project(), clip, beat(1)).is_err());
        assert!(arrangement_destination(editor.project(), clip, beat(6)).is_err());
        editor.execute(source(2)).unwrap();
        assert!(arrangement_destination(editor.project(), clip, beat(3)).is_err());
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_cue_ui_tests.rs"]
mod ui_tests;
