//! Opt-in Arrangement history playback. The existing seek owner retains all
//! intent, worker, native publication, cancellation and terminal display custody.
use super::{EditorMode, EditorView, context_menu::EditorContextAction, device_panel};
use crate::context_menu::{MenuItem, PopupMenu};
use gpui::{Context, Pixels, Point, Window};

#[derive(Clone)]
pub(super) struct MenuAction {
    generation: u64,
    revision: u64,
    frame: u64,
}

impl EditorView {
    pub(super) fn open_history_playback_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !device_panel::finalize_controls(self, cx) {
            return;
        }
        if self.mode != EditorMode::Arrangement {
            return;
        }
        self.cancel_time_selection_gesture();
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let action = MenuAction {
            generation: self.project_generation,
            revision: state.revision,
            frame: self.playback_start_frame(),
        };
        let focus = cx.focus_handle();
        let previous_focus = window.focused(cx);
        focus.focus(window);
        self.context_menu = Some(PopupMenu {
            scroll: gpui::ScrollHandle::new(),
            position,
            icon_grid: None,
            selected: 0,
            focus,
            previous_focus,
            items: vec![MenuItem {
                label: "play with history",
                swatch: None,
                checked: false,
                action: EditorContextAction::HistoryPlayback(action),
            }],
        });
        cx.notify();
    }

    pub(super) fn history_playback_menu_current(&self, action: &MenuAction) -> bool {
        action.generation == self.project_generation
            && self
                .state
                .as_ref()
                .is_some_and(|state| state.revision == action.revision)
    }

    pub(super) fn history_playback_menu_action(
        &mut self,
        action: MenuAction,
        cx: &mut Context<Self>,
    ) {
        let error = if action.generation != self.project_generation
            || self
                .state
                .as_ref()
                .is_none_or(|state| state.revision != action.revision)
        {
            Some("history playback target changed · open the menu again")
        } else if self.mode != EditorMode::Arrangement {
            Some("history playback requires arrangement view")
        } else if self.state.as_ref().is_some_and(|state| {
            state
                .project
                .loop_region()
                .is_some_and(|region| region.enabled())
        }) {
            Some("turn the arrangement loop off to play with history")
        } else if action.frame > hex_session::arrangement_seek::MAX_SEEK_WORK_FRAMES {
            Some("history playback limit exceeded · choose an earlier play start")
        } else {
            None
        };
        if let Some(error) = error {
            self.error = Some(error.into());
        } else {
            self.request_history_playback(action.frame);
        }
        cx.notify();
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
