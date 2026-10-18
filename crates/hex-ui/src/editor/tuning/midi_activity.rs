//! Gate highlights from the exact applied prepared graph and observed routes.
use super::*;
use hex_audio::prepared::SessionRouteOwner;
use hex_engine::SessionPlaybackOwner;
use std::sync::atomic::Ordering::Acquire;

impl EditorView {
    pub(in crate::editor) fn tick_tuning_midi_activity(&mut self) -> bool {
        if !self.tuning.open {
            return false;
        }
        let now = Instant::now();
        if self
            .tuning
            .midi_last_query
            .is_some_and(|last| now.saturating_duration_since(last) < Duration::from_millis(33))
        {
            return false;
        }
        self.tuning.midi_last_query = Some(now);
        let keys = (|| {
            let audio = self.audio.as_ref()?;
            let status = audio.control.status()?;
            if !status.playing.load(Acquire)
                || status.device_fault.load(Acquire)
                || status.render_fault.load(Acquire)
                || status.shutdown_complete.load(Acquire)
            {
                return None;
            }
            let audible = self.audible_playback.as_ref()?;
            if audible.project_generation != self.project_generation
                || status.applied_revision.load(Acquire) != audible.audio_revision
            {
                return None;
            }
            let prepared = audible.prepared_revision.as_ref()?;
            let identity = audio.control.output_identity()?;
            self.session_routes.poll(identity, status);
            let snapshot = self.session_routes.matching_snapshot(
                identity,
                Some((
                    audible.audio_revision,
                    audible.state.revision,
                    audible.project_generation,
                )),
                audible.state.revision,
                audible.project_generation,
            )?;
            if !self.session_routes.routes_available(
                snapshot,
                &audible.state.project,
                audible.state.revision,
                audible.project_generation,
            ) {
                return None;
            }
            let frame = status.position.load(Acquire);
            let performance = status.performance_frame.load(Acquire);
            let spans = self.tuning.midi_spans.as_mut()?;
            let mut keys = [0; 2];
            for track in audible.state.project.tracks() {
                let route_track = prepared.midi_activity_route_track(track.id()).ok()?;
                let route =
                    self.session_routes
                        .entry_for_track(snapshot, true, route_track.value())?;
                let owner =
                    match route.owner {
                        SessionRouteOwner::Arrangement => SessionPlaybackOwner::Arrangement,
                        SessionRouteOwner::Silence => continue,
                        SessionRouteOwner::Session { clip_id } => {
                            // Audio Session cells carry no MIDI gates.
                            let Some(clip) = prepared.session().midi_clips().iter().find(|clip| {
                                clip.track == route_track && clip.id.value() == clip_id
                            }) else {
                                continue;
                            };
                            SessionPlaybackOwner::Session { clip: clip.id }
                        }
                    };
                let activity = prepared
                    .midi_gate_keys(
                        track.id(),
                        owner,
                        route
                            .active_launch_beat_raw
                            .map(hex_project::BeatTime::from_raw),
                        (frame, performance),
                        spans,
                    )
                    .ok()?;
                keys[0] |= activity[0];
                keys[1] |= activity[1];
            }
            (status.applied_revision.load(Acquire) == audible.audio_revision).then_some(keys)
        })()
        .unwrap_or([0; 2]);
        let mut changed = self.tuning.midi_keys != keys;
        self.tuning.midi_keys = keys;
        for key in 0..128 {
            if (keys[key / 64] | self.tuning.held_keys[key / 64]) & (1_u64 << (key % 64)) != 0 {
                self.tuning.recent_keys[key] = Some(now);
            } else if self.tuning.recent_keys[key]
                .is_some_and(|at| now.duration_since(at) > Duration::from_millis(400))
            {
                self.tuning.recent_keys[key] = None;
                changed = true;
            }
        }
        changed
    }
}
