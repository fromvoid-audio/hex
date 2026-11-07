//! Contextual Command-F dispatch. Canonical project commands own fade fitting,
//! source-handle eligibility and atomic history; other focus retains search.
use super::{EditorMode, EditorView};
use gpui::{Context, Window};
use hex_project::Command;
use hex_session::service::Request;

impl EditorView {
    pub(super) fn create_selection_fades(&mut self, window: &Window, cx: &Context<Self>) -> bool {
        if self.mode != EditorMode::Arrangement {
            return false;
        }
        let events_focused = self.audio_event_focus.is_focused(window);
        let arranger_focused = self.focus.is_focused(window) || self.track_header_focused(window);
        if events_focused {
            let Some((start, end)) = self.audio_loop_time_selection().filter(|(a, b)| a < b) else {
                return false;
            };
            if !self.arrangement_view_change_available(cx)
                || self.device_panel.has_pending_draft(cx)
                || self.audio_event_editor.gesture.is_some()
            {
                return true;
            }
            let Some(clip) = self.detail_clip_id() else {
                return true;
            };
            self.cancel_duplicate_chain();
            self.enqueue(Request::Edit(Command::CreateAudioEventFades {
                clip,
                start,
                end,
            }));
            return true;
        }
        if !arranger_focused
            || self.automation_view.enabled
            || self
                .time_selection
                .is_none_or(|s| s.anchor_frame == s.end_frame)
        {
            return false;
        }
        if !self.arrangement_view_change_available(cx) || self.device_panel.has_pending_draft(cx) {
            return true;
        }
        let Some(range) = self.normalized_time_selection() else {
            self.error = Some("fade time selection is no longer available".into());
            return true;
        };
        self.cancel_duplicate_chain();
        self.enqueue(Request::Edit(Command::CreateArrangementFades { range }));
        true
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "selection_fades_ui_tests.rs"]
mod tests;
