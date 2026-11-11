//! Workspace panel composition and navigation preserve local control ownership.
use super::{EditorMode, EditorView};
use crate::theme;
use gpui::{Context, Div, KeyDownEvent, Modifiers, MouseUpEvent, Window, div, prelude::*, px, rgb};

pub(super) const PANEL_GUTTER: f32 = 3.0;

impl EditorView {
    /// Actual main-panel content width, after the browser, gutter and surround.
    pub(super) fn workspace_body_width(&self, window: &Window) -> f32 {
        (f32::from(window.bounds().size.width)
            - self.browser_content_width(window)
            - PANEL_GUTTER
            - 2.0 * hex_widgets::PANEL_FRAME_INSET)
            .max(0.0)
    }

    pub(super) fn workspace_panel_chrome(&self) -> f32 {
        let frame_and_gap = 2.0 * hex_widgets::PANEL_FRAME_INSET + PANEL_GUTTER;
        frame_and_gap * if self.clip_view_open { 2.0 } else { 1.0 }
    }

    pub(super) fn render_workspace(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        clip_view: Div,
    ) -> Div {
        let content = self.project_spectrum_panel(window, cx);
        let console = self.mode == EditorMode::Session;
        let main = if console {
            self.session_grid(window, cx)
        } else {
            self.arrangement(window, cx)
        };
        let main = hex_widgets::panel_frame(theme::workspace_panel_style(), main)
            .debug_selector(|| "workspace-main-panel".into())
            .flex_1()
            .min_h_0()
            .mt(px(PANEL_GUTTER))
            .mb(px(if self.clip_view_open {
                PANEL_GUTTER
            } else {
                0.0
            }));
        let clip_view = if self.clip_view_open {
            hex_widgets::panel_frame(theme::workspace_panel_style(), clip_view)
                .debug_selector(|| "workspace-editor-panel".into())
                .flex_shrink_0()
        } else {
            clip_view
        };
        let workspace_content = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(content)
            .child(main)
            .child(clip_view)
            .child(self.mixer_strip(cx, console))
            .child(self.routing_controls(cx));
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .gap(px(PANEL_GUTTER))
            .bg(rgb(theme::workspace_gutter_fill()))
            .child(
                self.browser_view(window, cx)
                    .debug_selector(|| "resize-browser-hit-region".to_owned())
                    .capture_any_mouse_up(cx.listener(|view, event: &MouseUpEvent, window, cx| {
                        if view.finish_trim_pointer_release(event, true, window, cx) {
                            cx.stop_active_drag(window);
                            cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
                            cx.stop_propagation();
                        }
                    })),
            )
            .child(workspace_content.flex_1().min_w_0())
    }

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
            || self.clip_paint_held()
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
