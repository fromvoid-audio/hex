//! Transient Arrangement presentation. Gain edits use the existing lane actor;
//! switching views never changes authored automation, clip activation or history.
use super::{EditorMode, EditorView};
use gpui::{Context, KeyDownEvent, Modifiers, Window};
use hex_session::service::GainLaneEdit;

#[derive(Default)]
pub(super) struct AutomationView {
    pub(super) enabled: bool,
    gain_panel_was_open: bool,
}
impl EditorView {
    pub(super) fn automation_view_key(
        &mut self,
        event: &KeyDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.keystroke.key != "a"
            || event.keystroke.modifiers != Modifiers::default()
            || event.is_held
            || self.computer_keyboard.enabled
            || !(self.focus.is_focused(window) || self.track_header_focused(window))
        {
            return false;
        }
        self.toggle_automation_view(cx)
    }

    pub(super) fn toggle_automation_view(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.arrangement_view_change_available(cx) {
            return false;
        }
        if self.automation_view.enabled {
            self.automation_view.enabled = false;
            if !self.automation_view.gain_panel_was_open {
                self.gain_lane.track = None;
                self.gain_lane.selected = None;
            }
        } else {
            self.automation_view.gain_panel_was_open = self.gain_lane.track.is_some();
            self.automation_view.enabled = true;
        }
        cx.notify();
        true
    }

    pub(super) fn delete_automation_point(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.mode != EditorMode::Arrangement
            || !self.automation_view.enabled
            || !self.focus.is_focused(window)
        {
            return false;
        }
        // Consume an empty automation selection too: retained clips are background.
        if let Some(beat) = self.gain_lane.selected {
            self.gain_edit(GainLaneEdit::Remove(beat), cx);
        }
        cx.notify();
        true
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
