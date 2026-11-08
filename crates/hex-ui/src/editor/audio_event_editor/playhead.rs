//! Read-only accepted Arrangement cursor. The project projection owns source
//! timing; callback revision and route ownership prevent optimistic/stale paint.
use super::*;
use hex_audio::prepared::SessionRouteOwner;
use std::sync::atomic::Ordering::Acquire;

impl EditorView {
    pub(in crate::editor) fn audio_playhead_root_position(&self) -> Option<u64> {
        let state = self.state.as_ref()?;
        let (track, clip_id, ..) = self.detail_audio_clip_data()?;
        let clip = state
            .project
            .track(track)?
            .clips()
            .iter()
            .find(|c| c.id() == clip_id)?;
        if !clip.active() {
            return None;
        }
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
        let route = self
            .session_routes
            .entry_for_track(snapshot, true, track.value())?;
        if route.owner != SessionRouteOwner::Arrangement {
            return None;
        }
        let position = hex_project::AudioViewProjection::new(clip, state.project.tempo_map())
            .ok()?
            .root_position_for_frame(status.position.load(Acquire))
            .ok()??;
        (status.applied_revision.load(Acquire) == audible.audio_revision).then_some(position)
    }

    /// Original-source tab uses media frames rather than the events root clock.
    /// This is the nominal authored read position, not an individual grain/FFT
    /// window. Grouped sources have no single original-media cursor.
    pub(in crate::editor) fn audio_playhead_media_frame(&self) -> Option<u64> {
        let position = self.audio_playhead_root_position()?;
        let (_, root, _) = self.event_context()?;
        if root.domain.events.is_some() {
            return None;
        }
        let state = self.state.as_ref()?;
        let domain = &root.domain;
        let read = if domain.warp_enabled {
            if position == 0 {
                domain.source_phase
            } else {
                let beat = u128::from(position) + u128::from(domain.warp_phase_beat().ok()?);
                u64::try_from(domain.warp_source_position_q32(beat).ok()? / (1_i128 << 32)).ok()?
            }
        } else {
            root.root_frame_at(position, state.project.tempo_map())
                .ok()?
                .checked_add(domain.source_phase)?
        };
        let read = if domain.looping && domain.source_length > 0 {
            read % domain.source_length
        } else if read < domain.source_length {
            read
        } else {
            return None;
        };
        domain.source_offset.checked_add(read)
    }
}
