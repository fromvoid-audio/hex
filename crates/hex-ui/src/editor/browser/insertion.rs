//! Enter captures one selected Arrangement target before bounded worker preparation.
use super::{EditorMode, EditorView};
use gpui::Context;
use hex_engine::playback::BrowserAuditionTiming;
use hex_session::import_batch_decode::AudioBatchContext;
use hex_session::service::Request;
use std::{path::PathBuf, sync::atomic::Ordering::Acquire};

impl EditorView {
    pub(super) fn insert_browser_project_sample(
        &mut self,
        generation: u64,
        revision: u64,
        asset: hex_project::AssetId,
        cx: &mut Context<Self>,
    ) {
        let request = (|| -> Result<Request, String> {
            let state = self
                .state
                .as_ref()
                .ok_or("sample insertion refused · no project")?;
            if generation != self.project_generation || revision != state.revision {
                return Err("sample insertion canceled · project changed".into());
            }
            state
                .project
                .existing_import_frames(asset)
                .map_err(|_| "sample insertion refused · source is unavailable")?;
            let (track, start, audition_timing) = self.browser_insertion_target(cx)?;
            Ok(Request::InsertProjectSample {
                project_generation: generation,
                revision,
                asset,
                track,
                start,
                warp: hex_project::AudioImportWarp::Off,
                audition_timing,
            })
        })();
        match request {
            Ok(request) => {
                if self.enqueue(request) {
                    self.stop_browser_preview_for_drop();
                    self.error = None;
                }
            }
            Err(error) => self.error = Some(error.into()),
        }
        cx.notify();
    }

    pub(super) fn insert_browser_audio(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let result = self.browser_insertion_request(path, cx);
        match result {
            Ok((request, control)) => {
                // Capture metadata only. Decode and the canonical overwrite commit stay on
                // the service worker; later UI selection/cursor changes cannot retarget it.
                let routed_audition = matches!(
                    &request,
                    Request::ImportAudioBatch {
                        audition_timing: Some(_),
                        ..
                    }
                );
                if self.enqueue(request) {
                    if routed_audition && self.browser.playback_output.is_some() {
                        self.cancel_browser_sample_playback();
                    }
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
        let (track, start, audition_timing) = self.browser_insertion_target(cx)?;
        Request::browser_audio_batch(
            path,
            track,
            start,
            AudioBatchContext {
                project_generation: self.project_generation,
                revision: self
                    .state
                    .as_ref()
                    .ok_or("sample insertion refused · no project")?
                    .revision,
                active_gesture: false,
            },
            audition_timing,
        )
    }

    fn browser_insertion_target(
        &self,
        cx: &gpui::App,
    ) -> Result<(hex_project::TrackId, u64, Option<BrowserAuditionTiming>), String> {
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
        let status = self.audio_status();
        let transport_epoch = status.map(|status| status.transport_epoch.load(Acquire));
        let start = grid
            .snap_frame(state.project.tempo_map(), self.cursor_position())
            .map_err(|_| "sample insertion refused · cannot resolve input grid")?;
        let audition_timing = status.zip(transport_epoch).and_then(|(status, epoch)| {
            (self.document_audio_mode == hex_session::DocumentAudioMode::Resident
                && status.playing.load(Acquire)
                && status.transport_epoch.load(Acquire) == epoch)
                .then_some(BrowserAuditionTiming {
                    grid,
                    transport_epoch: epoch,
                })
        });
        Ok((track, start, audition_timing))
    }
}
