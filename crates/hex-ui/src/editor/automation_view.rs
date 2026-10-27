//! Transient Arrangement presentation. Gain and native-device edits use their existing actors;
//! switching views never changes authored automation, clip activation or history.
use super::{EditorMode, EditorView};
use gpui::{Context, Window};
use hex_session::service::GainLaneEdit;

#[derive(Default)]
pub(super) struct AutomationView {
    pub(super) enabled: bool,
    pub(super) devices: super::device_automation::Lanes,
    gain_panel_was_open: bool,
    pub(super) gain_preview: Option<hex_project::TrackId>,
}
impl EditorView {
    pub(super) fn toggle_automation_view(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.arrangement_view_change_available(cx) {
            return false;
        }
        if self.automation_view.enabled {
            self.automation_view.enabled = false;
            self.automation_view.gain_preview = self.gain_lane.track;
            if !self.automation_view.gain_panel_was_open {
                self.gain_lane.track = None;
                self.gain_lane.selected = None;
            }
        } else {
            self.automation_view.gain_panel_was_open = self.gain_lane.track.is_some();
            if self.gain_lane.track.is_none() {
                self.gain_lane.track = self.automation_view.gain_preview.filter(|id| {
                    self.state
                        .as_ref()
                        .is_some_and(|state| state.project.track(*id).is_some())
                });
            }
            self.automation_view.enabled = true;
        }
        self.track_scroll.remeasure();
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
        if self.delete_device_automation_point(cx) {
            cx.notify();
            return true;
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
