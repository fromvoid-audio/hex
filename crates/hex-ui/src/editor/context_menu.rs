use super::{EditorView, ViewState, device_panel};
use crate::context_menu::{MenuItem, PopupMenu};
use gpui::{Context, Div, Pixels, Point, Window};
use hex_lattice_contract::{ParameterId, ParameterValue};
use hex_project::{Command, DeviceId, TrackId};
use hex_session::service::Request;

#[derive(Clone)]
pub(super) struct InsertTrackAction {
    generation: u64,
    before_track: Option<TrackId>,
}

#[derive(Clone)]
pub(super) struct UtilityModeAction {
    generation: u64,
    owner: device_panel::DevicePanelOwner,
    device: DeviceId,
    mid_side: bool,
}

#[derive(Clone)]
pub(super) enum EditorContextAction {
    InsertTrack(InsertTrackAction),
    UtilityMode(UtilityModeAction),
}

impl EditorView {
    pub(super) fn open_track_context_menu(
        &mut self,
        position: Point<Pixels>,
        after_track: Option<TrackId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_time_selection_gesture();
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
                action: EditorContextAction::InsertTrack(InsertTrackAction {
                    generation: self.project_generation,
                    before_track,
                }),
            }],
        });
        cx.notify();
    }

    pub(super) fn open_utility_mode_context_menu(
        &mut self,
        position: Point<Pixels>,
        device: DeviceId,
        enabled: bool,
        mid_side: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !enabled || !device_panel::finalize_controls(self, cx) {
            return;
        }
        let Some(owner) = self.device_panel.owner else {
            return;
        };
        let previous_focus = self
            .context_menu
            .take()
            .and_then(|menu| menu.previous_focus)
            .or_else(|| window.focused(cx));
        let focus = cx.focus_handle();
        window.focus(&focus);
        let (label, target_mode) = if mid_side {
            ("switch to width", false)
        } else {
            ("switch to mid/side", true)
        };
        self.context_menu = Some(PopupMenu {
            position,
            items: vec![MenuItem {
                label,
                action: EditorContextAction::UtilityMode(UtilityModeAction {
                    generation: self.project_generation,
                    owner,
                    device,
                    mid_side: target_mode,
                }),
            }],
            selected: 0,
            focus,
            previous_focus,
        });
        cx.notify();
    }

    pub(super) fn dismiss_context_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = self.context_menu.take() {
            window.focus(menu.previous_focus.as_ref().unwrap_or(&self.focus));
            cx.notify();
        }
    }

    fn select_context_item(
        &mut self,
        action: EditorContextAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_context_menu(window, cx);
        match action {
            EditorContextAction::InsertTrack(action) => {
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
            EditorContextAction::UtilityMode(action) => {
                if action.generation != self.project_generation
                    || self.device_panel.owner != Some(action.owner)
                {
                    return;
                }
                let available = self
                    .state
                    .as_ref()
                    .is_some_and(|state| utility_device_exists(&state.project, action.device));
                if !available {
                    return;
                }
                let key = device_panel::ParameterKey {
                    device: action.device,
                    parameter: ParameterId::from_static(9),
                };
                let value = ParameterValue::Bool(action.mid_side);
                self.device_panel.pending_values.insert(key, value);
                if !self.enqueue_device_barrier(Request::SetDeviceParameter {
                    device: action.device,
                    parameter: key.parameter,
                    value,
                }) {
                    self.device_panel.pending_values.remove(&key);
                    self.error = Some("could not change Utility width mode".into());
                }
                cx.notify();
            }
        }
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
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if self.context_menu.as_ref().is_some_and(|menu| {
            menu.items.iter().any(|item| match &item.action {
                EditorContextAction::InsertTrack(_) => false,
                EditorContextAction::UtilityMode(action) => {
                    let owner_matches = self.device_panel.owner == Some(action.owner);
                    let device_exists = self
                        .state
                        .as_ref()
                        .is_some_and(|state| utility_device_exists(&state.project, action.device));
                    !owner_matches || !device_exists || action.generation != self.project_generation
                }
            })
        }) {
            self.dismiss_context_menu(window, cx);
            return None;
        }
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

fn utility_device_exists(project: &hex_project::Project, id: DeviceId) -> bool {
    project.tracks().iter().any(|track| {
        track.ordered_device_chain().any(|node| {
            let device = match node {
                hex_project::TrackDeviceRef::Instrument(device)
                | hex_project::TrackDeviceRef::Effect(device) => device,
            };
            device.id() == id && device.state().type_id().as_str() == Ok("hex.utility")
        })
    }) || project.return_tracks().iter().any(|track| {
        track.devices().iter().any(|device| {
            device.id() == id && device.state().type_id().as_str() == Ok("hex.utility")
        })
    })
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
