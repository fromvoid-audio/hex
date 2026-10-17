//! Native OS audio-file drops use the same canonical imports as browser samples.
use super::{EditorMode, EditorView, drag::browser_start};
use gpui::{Context, Window};
use hex_session::import_batch::AudioBatchDestination;
use hex_session::import_batch_decode::{AudioBatchCancel, AudioBatchContext, AudioBatchPhase};
use hex_session::service::Request;
use std::path::PathBuf;

impl EditorView {
    pub(super) fn drop_external_audio_paths(
        &mut self,
        paths: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() || paths.len() > hex_project::MAX_ASSETS {
            self.error = Some("audio drop refused · choose 1..4096 audio files".into());
            cx.notify();
            return;
        }
        if self.mode != EditorMode::Arrangement
            || self.opening_project
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
            self.error = Some("audio drop canceled · arrangement is busy".into());
            cx.notify();
            return;
        }
        let Some(state) = self.state.as_ref() else {
            self.error = Some("audio drop canceled · no project is available".into());
            cx.notify();
            return;
        };
        let pointer = window.mouse_position();
        let (x, y) = (f32::from(pointer.x), f32::from(pointer.y));
        let viewport = self.visible_viewport(state.timeline_frames);
        let snap = self
            .snap_grid
            .with_platform_modifier(window.modifiers().platform);
        let request = match self.painted_arrangement_drop_target(x, y) {
            Some(super::arrangement_device_drop::ArrangementDropTarget::NewTrack {
                before_track,
                lane,
                ..
            }) => browser_start(
                &state.project,
                viewport.start,
                viewport.length,
                snap,
                x,
                lane,
            )
            .map(|start| AudioBatchDestination::NewTracks {
                before_track,
                start,
            }),
            Some(super::arrangement_device_drop::ArrangementDropTarget::ExistingTrack {
                track,
                lane,
            }) => browser_start(
                &state.project,
                viewport.start,
                viewport.length,
                snap,
                x,
                lane,
            )
            .map(|start| AudioBatchDestination::Track { track, start }),
            None => None,
        };
        let Some(request) = request else {
            self.error = Some(
                "audio drop canceled · choose a regular track or empty arrangement space".into(),
            );
            cx.notify();
            return;
        };
        let context = AudioBatchContext {
            project_generation: self.project_generation,
            revision: state.revision,
            active_gesture: false,
        };
        let (request, control) = match Request::audio_batch(paths.to_vec(), request, context) {
            Ok(request) => request,
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        // Only path metadata crosses the UI queue; PCM stays on the worker.
        if self.enqueue(request) {
            self.audio_batch = Some(control);
            self.audio_batch_progress = None;
            self.error = None;
            self.poll_audio_batch();
        }
        cx.notify();
    }
    pub(super) fn poll_audio_batch(&mut self) -> bool {
        let Some(control) = &self.audio_batch else {
            return false;
        };
        let progress = control.progress();
        if self.audio_batch_progress == Some(progress) {
            return false;
        }
        self.audio_batch_progress = Some(progress);
        let phase = match progress.phase {
            AudioBatchPhase::Queued => "queued",
            AudioBatchPhase::Decoding => "decoding",
            AudioBatchPhase::Converting => "converting",
            AudioBatchPhase::Prepared => "preparing",
            AudioBatchPhase::Compiling => "compiling",
            AudioBatchPhase::Committing => "committing",
            AudioBatchPhase::Completed | AudioBatchPhase::Canceled | AudioBatchPhase::Failed => {
                "result pending"
            }
        };
        self.status = if progress.cancellation_requested {
            "audio import cancellation requested · waiting for worker".into()
        } else {
            format!(
                "audio import · {phase} {}/{} · {} ready",
                progress.member, progress.total_members, progress.completed_members
            )
        };
        true
    }
    pub(super) fn cancel_audio_batch(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(control) = &self.audio_batch else {
            return false;
        };
        self.status = match control.request_cancel() {
            AudioBatchCancel::Requested | AudioBatchCancel::AlreadyRequested => {
                "audio import cancellation requested · waiting for worker"
            }
            AudioBatchCancel::TooLate => "audio import is committing · use undo after completion",
            AudioBatchCancel::Finished => "audio import result pending",
        }
        .into();
        cx.notify();
        true
    }
}
