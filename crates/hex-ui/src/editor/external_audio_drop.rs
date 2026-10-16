//! Native OS audio-file drops use the same canonical imports as browser samples.
use super::{EditorMode, EditorView, drag::browser_start};
use gpui::{Context, Window};
use hex_session::service::Request;
use std::path::PathBuf;

impl EditorView {
    pub(super) fn drop_external_audio_paths(
        &mut self,
        paths: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let [path] = paths else {
            self.error = Some("drop one audio file at a time".into());
            cx.notify();
            return;
        };
        if self.mode != EditorMode::Arrangement
            || self.opening_project
            || self.close_prompt_open
            || self.save_dialog_open
            || self.export_dialog_open
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
            .map(|start| Request::ImportToNewTrack {
                path: path.clone(),
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
            .map(|start| Request::ImportToTrackWithOverwrite {
                path: path.clone(),
                track,
                start,
            }),
            None => None,
        };
        let Some(request) = request else {
            self.error = Some(
                "audio drop canceled · choose a regular track or empty arrangement space".into(),
            );
            cx.notify();
            return;
        };
        // The existing bounded worker decodes/resamples, validates and commits
        // one candidate/history step. No filesystem or PCM work occurs here.
        if self.enqueue(request) {
            self.status = "importing audio to arrangement".into();
        }
        cx.notify();
    }
}
