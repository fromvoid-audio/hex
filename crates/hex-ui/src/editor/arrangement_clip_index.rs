//! Thin ViewState adapter over `hex_ui_model::arrangement_clip_index`.
use super::ViewState;
use hex_project::TrackId;
pub(super) use hex_ui_model::arrangement_clip_index::{
    IndexedClip, IndexedClipKind, TrackClipIndex,
};

#[derive(Default)]
pub(super) struct ArrangementClipIndex(hex_ui_model::arrangement_clip_index::ArrangementClipIndex);

impl ArrangementClipIndex {
    pub(super) fn ensure_current(&mut self, generation: u64, state: &ViewState) {
        self.0.ensure_current_tracks(
            generation,
            state.revision,
            std::ptr::from_ref(state) as usize,
            state.project.tracks(),
            |clip_id| {
                state
                    .midi_preview
                    .get(clip_id)
                    .map(|preview| (preview.frame_start, preview.frame_end))
            },
        );
    }

    pub(super) fn track(&self, track_id: TrackId) -> Option<&TrackClipIndex> {
        self.0.track(track_id)
    }
}
