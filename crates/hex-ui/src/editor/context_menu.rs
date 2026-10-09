use super::{EditorView, ViewState};
use crate::context_menu::{MenuItem, PopupMenu};
use gpui::{Context, Div, Pixels, Point, Window};
use hex_project::{Command, TrackId};
use hex_session::service::Request;

#[derive(Clone)]
pub(super) struct InsertTrackAction {
    generation: u64,
    before_track: Option<TrackId>,
}

impl EditorView {
    pub(super) fn open_track_context_menu(
        &mut self,
        position: Point<Pixels>,
        after_track: Option<TrackId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = &self.state else {
            return;
        };
        let before_track = insertion_anchor(state, after_track);
        let previous_focus = self
            .context_menu
            .take()
            .and_then(|menu| menu.previous_focus)
            .or_else(|| window.focused(cx));
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.context_menu = Some(PopupMenu {
            position,
            selected: 0,
            focus,
            previous_focus,
            items: vec![MenuItem {
                label: "insert track",
                action: InsertTrackAction {
                    generation: self.project_generation,
                    before_track,
                },
            }],
        });
        cx.notify();
    }

    fn dismiss_context_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = self.context_menu.take() {
            window.focus(menu.previous_focus.as_ref().unwrap_or(&self.focus));
            cx.notify();
        }
    }

    fn select_context_item(
        &mut self,
        action: InsertTrackAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_context_menu(window, cx);
        if action.generation != self.project_generation {
            return;
        }
        let Some(state) = &self.state else {
            return;
        };
        if action
            .before_track
            .is_some_and(|track| state.project.track(track).is_none())
        {
            self.error = Some("track insertion anchor no longer exists".into());
            cx.notify();
            return;
        }
        let name = format!("track {}", state.project.tracks().len() + 1);
        self.enqueue(Request::Edit(Command::InsertTrack {
            name,
            before_track: action.before_track,
        }));
        cx.notify();
    }

    fn navigate_context_menu(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.context_menu {
            let count = menu.items.len();
            if count > 0 {
                menu.selected = if forward {
                    (menu.selected + 1) % count
                } else {
                    (menu.selected + count - 1) % count
                };
            }
            cx.notify();
        }
    }

    pub(super) fn context_menu_overlay(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        self.context_menu.as_ref().map(|menu| {
            crate::context_menu::popup(
                menu,
                window,
                cx,
                Self::select_context_item,
                Self::dismiss_context_menu,
                Self::navigate_context_menu,
            )
        })
    }
}

fn insertion_anchor(state: &ViewState, after_track: Option<TrackId>) -> Option<TrackId> {
    after_track
        .and_then(|track| {
            state
                .project
                .tracks()
                .iter()
                .position(|candidate| candidate.id() == track)
        })
        .and_then(|index| state.project.tracks().get(index + 1))
        .map(|track| track.id())
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::super::clip_drag_ui_test::{audio_project, editor_window};
    use super::*;
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
    use std::sync::Arc;

    #[gpui::test]
    fn right_click_track_inserts_below_clicked_track(cx: &mut TestAppContext) {
        let (project, source, target, _) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let selector = Box::leak(format!("arrangement-lane-{}", source.value()).into_boxed_str());
        let bounds = visual.debug_bounds(selector).expect("track lane");
        let position = point(bounds.origin.x + px(12.0), bounds.origin.y + px(20.0));
        visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
        visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        let item = visual.debug_bounds("context-menu-item").expect("menu row");
        let click = point(item.origin.x + px(15.0), item.origin.y + px(12.0));
        visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |view, _| {
            assert!(view.context_menu.is_none());
            assert!(matches!(view.last_enqueued_edit, Some(Command::InsertTrack { before_track: Some(anchor), .. }) if anchor == target));
        });
    }

    #[gpui::test]
    fn menu_escape_outside_click_and_enter_do_not_leak_to_editor(cx: &mut TestAppContext) {
        let (project, _, _, _) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        let open = |visual: &mut VisualTestContext| {
            visual.update(|window, cx| {
                editor.update(cx, |view, cx| {
                    view.open_track_context_menu(point(px(600.0), px(100.0)), None, window, cx)
                })
            });
            visual.run_until_parked();
        };
        open(&mut visual);
        visual.simulate_keystrokes("escape");
        editor.read_with(cx, |view, _| {
            assert!(view.context_menu.is_none());
            assert!(view.last_enqueued_edit.is_none());
        });
        open(&mut visual);
        visual.simulate_mouse_down(
            point(px(5.0), px(100.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.simulate_mouse_up(
            point(px(5.0), px(100.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        editor.read_with(cx, |view, _| {
            assert!(view.context_menu.is_none());
            assert!(view.last_enqueued_edit.is_none());
        });
        open(&mut visual);
        visual.simulate_keystrokes("down up enter");
        editor.read_with(cx, |view, _| {
            assert!(view.context_menu.is_none());
            assert!(matches!(
                view.last_enqueued_edit,
                Some(Command::InsertTrack {
                    before_track: None,
                    ..
                })
            ));
        });
    }
}
