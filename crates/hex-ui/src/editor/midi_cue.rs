//! Ruler cue dispatch uses the existing Arrangement seek or accepted Session
//! route command. It never edits source start or paints an optimistic playhead.
use super::EditorView;
use hex_audio::prepared::SessionRouteOwner;
use hex_engine::SessionPlaybackCommand;
use hex_project::{BeatTime, ClipId, SessionCellContent};

mod timing;
use timing::arrangement_destination;
pub(super) use timing::arrangement_elapsed;

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
        let track = state.project.track(self.selected_track?)?;
        let placement = track.midi_clips().iter().find(|item| item.id() == clip);
        let (source, offset) = if let Some(placement) = placement {
            if !placement.active() {
                return None;
            }
            (placement.source(), placement.playback_offset())
        } else {
            let (selected_track, scene) = self.selected_cell?;
            if selected_track != track.id() {
                return None;
            }
            match state.project.session_cell(selected_track, scene)? {
                SessionCellContent::Midi(item) if item.id() == clip && item.active() => {
                    (item.source(), item.playback_offset())
                }
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
            arrangement_elapsed(
                state.project.tempo_map(),
                placement.start(),
                placement.arrangement_length(),
                status.position.load(Acquire),
            )?
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
        let playback = source
            .playback_for_duration(BeatTime::from_raw(1))
            .and_then(|playback| playback.with_playback_offset(offset))
            .ok()?;
        let phase = playback.source_phase_at_elapsed(elapsed)?;
        // A publication can change during scalar reads; never paint it against
        // another compiled revision's mapping.
        (status.applied_revision.load(Acquire) == audible.audio_revision).then_some(phase.raw())
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_cue_ui_tests.rs"]
mod ui_tests;
