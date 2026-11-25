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
        let snapshot = self.session_grid.routes.matching_snapshot(
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
            .session_grid
            .routes
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

    /// Normalized `[0, 1]` playhead position for whichever audio clip detail
    /// editor is currently mounted (Arrangement Events, Arrangement Source, or
    /// Session Audio). Used by the isolated `AudioDetailPlayheadView` subview.
    pub(in crate::editor) fn audio_detail_playhead_fraction(&self) -> Option<f32> {
        if !self.audio_detail.clip_view_open || self.device_panel.active {
            return None;
        }
        match self.mode {
            super::super::EditorMode::Session => {
                let (track, scene) = self.session_grid.selected_cell?;
                let state = self.state.as_ref()?;
                let hex_project::SessionCellContent::Audio(clip) =
                    state.project.session_cell(track, scene)?
                else {
                    return None;
                };
                self.session_audio_playhead(track, clip)
            }
            super::super::EditorMode::Arrangement => {
                let state = self.state.as_ref()?;
                if self.audio_detail.event.source_view {
                    let (_, clip_id, ..) = self.detail_audio_clip_data()?;
                    let clip = state.project.clip(clip_id)?;
                    let asset_frames = state.project.asset(clip.asset())?.playback_frames();
                    let frame = self
                        .audio_playhead_media_frame()
                        .filter(|frame| *frame < asset_frames)?;
                    let viewport = self.audio_source_viewport();
                    if !viewport.contains(i128::from(frame)) {
                        return None;
                    }
                    return viewport.fraction(i128::from(frame)).map(|f| f as f32);
                }
                let (clip_id, canonical_root, _) = self.event_context()?;
                let owner = (clip_id, self.project_generation);
                let pending = self
                    .audio_detail
                    .event
                    .pending
                    .as_ref()
                    .filter(|p| p.owner == owner);
                let placement = pending.and_then(|p| p.placement.as_ref()).or_else(|| {
                    self.audio_detail
                        .event
                        .gesture
                        .as_ref()
                        .and_then(|g| g.placement.as_ref())
                });
                let root = placement
                    .map_or_else(|| canonical_root.clone(), |p| p.clip().audio_event_root());
                let root_window = self
                    .event_root_window(&canonical_root)?
                    .shifted(placement.map_or(0, |p| p.frame_shift()))?;
                let position = self.audio_playhead_root_position()?;
                let display_pos = placement.map_or(Some(position), |p| p.position(position))?;
                let frame = root
                    .root_frame_at(display_pos, state.project.tempo_map())
                    .ok()?;
                if !root_window.contains(i128::from(frame)) {
                    return None;
                }
                root_window.fraction(i128::from(frame)).map(|f| f as f32)
            }
        }
    }
}
