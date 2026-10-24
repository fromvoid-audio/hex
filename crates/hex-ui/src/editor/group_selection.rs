//! Whole-clip Arrangement grouping from the focused editor shortcut.
use super::{EditorMode, EditorView};
use gpui::Window;
use hex_session::service::Request;

impl EditorView {
    pub(super) fn group_selected_clips(&mut self, window: &Window) {
        if self.mode != EditorMode::Arrangement
            || !self.focus.is_focused(window)
            || self.midi_focus.is_focused(window)
            || self.audio_event_focus.is_focused(window)
            || self.computer_keyboard.accepts_key("g")
        {
            return;
        }
        if self.close_prompt_open {
            self.error = Some("finish the close action before editing".into());
            return;
        }
        if self.edit_pending() {
            self.error = Some("applying edit · wait before grouping clips".into());
            return;
        }
        let clips = self.selected_clip_ids();
        if clips.len() < 2 {
            return;
        }
        let Some(state) = &self.state else {
            return;
        };
        self.enqueue(Request::GroupClips {
            project_generation: self.project_generation,
            revision: state.revision,
            clips,
        });
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "group_selection_ui_tests.rs"]
mod ui_tests;
