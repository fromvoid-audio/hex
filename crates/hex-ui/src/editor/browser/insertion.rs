//! Enter captures one selected Arrangement target before bounded worker preparation.
use super::{EditorMode, EditorView};
use gpui::Context;
use hex_session::import_batch::AudioBatchDestination;
use hex_session::import_batch_decode::AudioBatchContext;
use hex_session::service::Request;
use std::path::PathBuf;

impl EditorView {
    pub(super) fn insert_browser_audio(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let result = self.browser_insertion_request(path, cx);
        match result {
            Ok((request, control)) => {
                // Capture metadata only. Decode and the canonical overwrite commit stay on
                // the service worker; later UI selection/cursor changes cannot retarget it.
                if self.enqueue(request) {
                    self.audio_batch = Some(control);
                    self.audio_batch_progress = None;
                    self.error = None;
                    self.poll_audio_batch();
                }
            }
            Err(error) => self.error = Some(error.into()),
        }
        cx.notify();
    }

    fn browser_insertion_request(
        &self,
        path: PathBuf,
        cx: &gpui::App,
    ) -> Result<
        (
            Request,
            std::sync::Arc<hex_session::import_batch_decode::AudioBatchControl>,
        ),
        String,
    > {
        if self.browser_place_unavailable() {
            return Err("sample insertion refused · source is unavailable".into());
        }
        if self.mode != EditorMode::Arrangement {
            return Err("sample insertion refused · select an arrangement track".into());
        }
        if self.opening_project
            || self.close_prompt_open
            || self.save_dialog_open
            || self.export_dialog_open
            || self.audio_batch.is_some()
            || self.device_gesture.snapshot().is_some()
            || self.device_gesture.has_pending_terminal()
            || !self.device_gesture_barriers.is_empty()
            || self.device_panel.has_pending_draft(cx)
            || self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
            || self.active_clip_drag.is_some()
            || self.active_browser_audio_drag.is_some()
            || self.active_native_device_drag.is_some()
        {
            return Err("sample insertion refused · arrangement is busy".into());
        }
        let state = self
            .state
            .as_ref()
            .ok_or("sample insertion refused · no project")?;
        let track = self
            .selected_track
            .filter(|id| state.project.track(*id).is_some())
            .ok_or("sample insertion refused · select an arrangement track")?;
        let grid = self.resolve_input_grid()?;
        let start = grid
            .snap_frame(state.project.tempo_map(), self.cursor_position())
            .map_err(|_| "sample insertion refused · cannot resolve input grid")?;
        Request::audio_batch(
            vec![path],
            AudioBatchDestination::Track { track, start },
            AudioBatchContext {
                project_generation: self.project_generation,
                revision: state.revision,
                active_gesture: false,
            },
        )
    }
}
