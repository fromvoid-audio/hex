//! Workspace navigation leaves focused controls and modal editors in charge of Tab.
use super::{EditorMode, EditorView};
use gpui::{Context, KeyDownEvent, Modifiers, Window};

impl EditorView {
    pub(super) fn workspace_view_name(&self) -> &'static str {
        match self.mode {
            EditorMode::Arrangement => "arrange view",
            EditorMode::Session => "console view",
        }
    }

    /// Bubbling, never capture/global: children retain local traversal and text input.
    pub(super) fn workspace_tab(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.keystroke.key != "tab"
            || event.keystroke.modifiers != Modifiers::default()
            || event.is_held
            || !(self.focus.is_focused(window) || self.track_header_focused(window))
            || self.track_name.is_editing()
            || self.clip_name_edit.is_some()
            || self.tuning.open
            || self.theme_settings.open
            || self.controller_input_is_open()
            || self.context_menu.is_some()
            || (self.clip_view_open
                && self.device_panel_active
                && self.device_panel.add_menu_is_open(self.state.as_deref()))
            || self.send_menu_track.is_some()
            || self.project_tempo_editing(cx)
            || self.gain_numeric_editing(cx)
            || self.gain_lane.pointer_is_active()
            || self.clip_numeric_editing(cx)
            || self.close_prompt_open
            || self.opening_project
            || self.project_open.busy()
            || self.pending_open_path.is_some()
            || self.export_dialog_open
            || self.save_dialog_open
            || self.preset_dialog_open
            || self.track_resize_active()
            || self.track_reorder_active()
            || self.arrangement_pan.is_some()
            || self.pointer_down.is_some()
            || self.project_spectrum.resize_is_active()
            || self.clip_detail_resize.is_active()
            || self.device_panel.navigation.is_active()
            || self.clip_paint_gesture.is_some()
            || self.active_clip_drag.is_some()
            || self.active_browser_audio_drag.is_some()
            || self.active_native_device_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_midi_trim_drag.is_some()
            || self.active_fade_drag.is_some()
            || self.time_selection_gesture.is_some()
            || self.time_range_move_gesture.is_some()
            || self.loop_rail_gesture.is_some()
            || self.scrub_active()
        {
            return false;
        }
        self.set_mode(match self.mode {
            EditorMode::Arrangement => EditorMode::Session,
            EditorMode::Session => EditorMode::Arrangement,
        });
        // A focused track header leaves the tree when Console mounts.
        self.focus.focus(window);
        cx.notify();
        true
    }
}
