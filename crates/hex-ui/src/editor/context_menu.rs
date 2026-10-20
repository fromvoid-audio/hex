#[path = "adaptive_grid_menu.rs"]
mod adaptive_grid_menu;

#[cfg(all(test, feature = "ui-test-support"))]
use super::ViewState;
use super::{EditorView, device_panel};
use crate::context_menu::{MenuItem, PopupMenu};
use gpui::{Context, Div, Pixels, Point, Window};
use hex_lattice_contract::{ParameterId, ParameterValue};
use hex_project::{ClipFlavor, ClipId, Command, DeviceId, ElementColor, SceneId, TrackId};
use hex_session::service::Request;
use hex_session::snapshot_exports::SnapshotProjectRead;
use hex_session::snapshot_exports::ui_consumers::SnapshotSessionCellContentRead;

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
    RemoveBrowserSource(super::browser::BrowserSection),
    AdaptiveGrid {
        generation: u64,
        density: super::musical_time::AdaptiveGridDensity,
    },
    PlaceIcon {
        path: std::path::PathBuf,
        id: Option<&'static str>,
    },
    InsertTrack(InsertTrackAction),
    InsertScene {
        generation: u64,
        revision: u64,
        before: Option<SceneId>,
    },
    UtilityMode(UtilityModeAction),
    DeviceComparison(super::device_comparison::Intent),
    TrackColor {
        generation: u64,
        track: TrackId,
        color: ElementColor,
    },
    ClipFlavor {
        generation: u64,
        track: TrackId,
        clip: ClipId,
        flavor: ClipFlavor,
    },
}

impl EditorView {
    pub(super) fn open_console_scene_context_menu(
        &mut self,
        position: Point<Pixels>,
        before: Option<SceneId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mode != super::EditorMode::Session
            || self.edit_pending()
            || !device_panel::finalize_controls(self, cx)
        {
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Ok(Some(revision)) = state.read_with(|snapshot| {
            if before.is_some_and(|id| snapshot.model().scene(id).is_none()) {
                return None;
            }
            Some(snapshot.revision())
        }) else {
            return;
        };
        let previous_focus = self
            .context_menu
            .take()
            .and_then(|menu| menu.previous_focus)
            .or_else(|| window.focused(cx));
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.context_menu = Some(PopupMenu {
            icon_grid: None,
            position,
            selected: 0,
            focus,
            previous_focus,
            items: vec![MenuItem {
                label: "add scene",
                swatch: None,
                checked: false,
                action: EditorContextAction::InsertScene {
                    generation: self.project_generation,
                    revision,
                    before,
                },
            }],
        });
        cx.notify();
    }
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
        let Ok(before_track) =
            state.read_with(|snapshot| insertion_anchor(snapshot.model(), after_track))
        else {
            return;
        };
        self.open_track_context_menu_at(position, before_track, window, cx);
    }
    fn open_track_context_menu_at(
        &mut self,
        position: Point<Pixels>,
        before_track: Option<TrackId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_time_selection_gesture();
        let previous_focus = self
            .context_menu
            .take()
            .and_then(|menu| menu.previous_focus)
            .or_else(|| window.focused(cx));
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.context_menu = Some(PopupMenu {
            icon_grid: None,
            position,
            selected: 0,
            focus,
            previous_focus,
            items: vec![MenuItem {
                label: "insert track",
                swatch: None,
                checked: false,
                action: EditorContextAction::InsertTrack(InsertTrackAction {
                    generation: self.project_generation,
                    before_track,
                }),
            }],
        });
        self.append_adaptive_grid_choices();
        cx.notify();
    }

    pub(super) fn open_track_color_context_menu(
        &mut self,
        position: Point<Pixels>,
        track: TrackId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = &self.state else {
            return;
        };
        let Ok(Some((color, before_track))) = state.read_with(|snapshot| {
            let project = snapshot.model();
            Some((
                project.track(track)?.color(),
                insertion_anchor(project, Some(track)),
            ))
        }) else {
            return;
        };
        if !device_panel::finalize_controls(self, cx) {
            return;
        }
        self.open_track_context_menu_at(position, before_track, window, cx);
        if let Some(menu) = &mut self.context_menu {
            menu.items.extend(
                ElementColor::TRACK_CHOICES
                    .into_iter()
                    .map(|choice| MenuItem {
                        label: choice.key(),
                        swatch: Some(crate::theme::element_colors(choice).accent),
                        checked: color == choice,
                        action: EditorContextAction::TrackColor {
                            generation: self.project_generation,
                            track,
                            color: choice,
                        },
                    }),
            );
        }
        self.append_adaptive_grid_choices();
    }

    pub(super) fn open_clip_context_menu(
        &mut self,
        position: Point<Pixels>,
        track: TrackId,
        clip: ClipId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Ok(Some((flavor, color))) = state.read_with(|snapshot| {
            let project = snapshot.model();
            if !clip_belongs_to_track(project, track, clip) {
                return None;
            }
            Some((project.clip_flavor(clip)?, project.track(track)?.color()))
        }) else {
            return;
        };
        if !device_panel::finalize_controls(self, cx) {
            return;
        }
        self.cancel_time_selection_gesture();
        let previous_focus = self
            .context_menu
            .take()
            .and_then(|m| m.previous_focus)
            .or_else(|| window.focused(cx));
        let focus = cx.focus_handle();
        window.focus(&focus);
        let target = flavor.toggled();
        self.context_menu = Some(PopupMenu {
            icon_grid: None,
            position,
            selected: 0,
            focus,
            previous_focus,
            items: vec![MenuItem {
                label: if target == ClipFlavor::Accent {
                    "use accent flavor"
                } else {
                    "use muted flavor"
                },
                swatch: Some(crate::theme::clip_fill(color, target, false)),
                checked: false,
                action: EditorContextAction::ClipFlavor {
                    generation: self.project_generation,
                    track,
                    clip,
                    flavor: target,
                },
            }],
        });
        if self.mode == super::EditorMode::Arrangement {
            self.append_adaptive_grid_choices();
        }
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
            icon_grid: None,
            position,
            items: vec![MenuItem {
                label,
                swatch: None,
                checked: false,
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
        let Some(state) = &self.state else {
            return;
        };
        let Ok((valid, track_count)) = state.read_with(|snapshot| {
            let project = snapshot.model();
            let valid = match &action {
                EditorContextAction::DeviceComparison(_)
                | EditorContextAction::PlaceIcon { .. }
                | EditorContextAction::RemoveBrowserSource(_)
                | EditorContextAction::AdaptiveGrid { .. } => true,
                EditorContextAction::InsertScene {
                    generation,
                    revision,
                    before,
                } => {
                    self.mode == super::EditorMode::Session
                        && *generation == self.project_generation
                        && *revision == snapshot.revision()
                        && before.is_none_or(|id| project.scene(id).is_some())
                }
                EditorContextAction::TrackColor {
                    generation, track, ..
                } => *generation == self.project_generation && project.track(*track).is_some(),
                EditorContextAction::ClipFlavor {
                    generation,
                    track,
                    clip,
                    ..
                } => {
                    *generation == self.project_generation
                        && clip_belongs_to_track(project, *track, *clip)
                }
                EditorContextAction::InsertTrack(action) => action
                    .before_track
                    .is_none_or(|track| project.track(track).is_some()),
                EditorContextAction::UtilityMode(action) => {
                    action.generation == self.project_generation
                        && self.device_panel.owner == Some(action.owner)
                        && utility_device_exists(project, action.device)
                }
            };
            (valid, project.tracks().len())
        }) else {
            return;
        };
        // A refused comparison keeps the same menu and focus for retry.
        if !matches!(&action, EditorContextAction::DeviceComparison(_)) {
            self.dismiss_context_menu(window, cx);
        }
        match action {
            EditorContextAction::RemoveBrowserSource(source) => {
                self.remove_browser_source(source, window, cx);
            }
            EditorContextAction::AdaptiveGrid {
                generation,
                density,
            } => {
                if generation == self.project_generation {
                    self.snap_grid = self.snap_grid.with_density(density);
                    self.status = format!("arranger · {}", density.label());
                    cx.notify();
                }
            }
            EditorContextAction::DeviceComparison(target) => {
                if self.perform_device_comparison(target, cx) {
                    self.dismiss_context_menu(window, cx);
                }
            }
            EditorContextAction::PlaceIcon { path, id } => {
                self.set_browser_place_icon(path, id, cx)
            }
            EditorContextAction::TrackColor {
                generation,
                track,
                color,
            } => {
                if generation == self.project_generation && valid {
                    self.enqueue(Request::Edit(Command::SetTrackColor { track, color }));
                    cx.notify();
                }
            }
            EditorContextAction::ClipFlavor {
                generation,
                track: _,
                clip,
                flavor,
            } => {
                if generation == self.project_generation && valid {
                    self.enqueue(Request::Edit(Command::SetClipFlavor { clip, flavor }));
                    cx.notify();
                }
            }
            EditorContextAction::InsertScene {
                generation,
                revision,
                before,
            } => {
                self.insert_console_scene(before, generation, revision, cx);
            }
            EditorContextAction::InsertTrack(action) => {
                if action.generation != self.project_generation {
                    return;
                }
                if !valid {
                    self.error = Some("track insertion anchor no longer exists".into());
                    cx.notify();
                    return;
                }
                let name = format!("track {}", track_count + 1);
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
                if !valid {
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
                EditorContextAction::DeviceComparison(target) => {
                    matches!(super::device_comparison::current(self, target), Ok(false))
                }
                EditorContextAction::PlaceIcon { path, .. } => {
                    !self.browser_place_icon_assignable(path)
                }
                EditorContextAction::RemoveBrowserSource(source) => {
                    !self.browser_source_removable(source)
                }
                EditorContextAction::InsertTrack(_) => false,
                EditorContextAction::AdaptiveGrid { generation, .. } => {
                    *generation != self.project_generation
                }
                action => self.state.as_ref().is_none_or(|state| {
                    matches!(
                        state.read_with(|snapshot| {
                            let project = snapshot.model();
                            match action {
                                EditorContextAction::InsertScene {
                                    generation,
                                    revision,
                                    before,
                                } => {
                                    self.mode != super::EditorMode::Session
                                        || *generation != self.project_generation
                                        || snapshot.revision() != *revision
                                        || before.is_some_and(|id| project.scene(id).is_none())
                                }
                                EditorContextAction::TrackColor {
                                    generation, track, ..
                                } => {
                                    *generation != self.project_generation
                                        || project.track(*track).is_none()
                                }
                                EditorContextAction::ClipFlavor {
                                    generation,
                                    track,
                                    clip,
                                    ..
                                } => {
                                    *generation != self.project_generation
                                        || !clip_belongs_to_track(project, *track, *clip)
                                }
                                EditorContextAction::UtilityMode(action) => {
                                    self.device_panel.owner != Some(action.owner)
                                        || !utility_device_exists(project, action.device)
                                        || action.generation != self.project_generation
                                }
                                EditorContextAction::DeviceComparison(_)
                                | EditorContextAction::PlaceIcon { .. }
                                | EditorContextAction::RemoveBrowserSource(_)
                                | EditorContextAction::InsertTrack(_)
                                | EditorContextAction::AdaptiveGrid { .. } => false,
                            }
                        }),
                        Ok(true)
                    )
                }),
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

fn clip_belongs_to_track(project: SnapshotProjectRead<'_>, track: TrackId, clip: ClipId) -> bool {
    project.track(track).is_some_and(|track| {
        track.clips().any(|item| item.id() == clip)
            || track.midi_clips().any(|item| item.id() == clip)
    }) || project.session_cells().any(|cell| {
        cell.track() == track
            && match cell.content() {
                SnapshotSessionCellContentRead::Audio(item) => item.id() == clip,
                SnapshotSessionCellContentRead::Midi(item) => item.id() == clip,
                _ => false,
            }
    })
}
fn utility_device_exists(project: SnapshotProjectRead<'_>, id: DeviceId) -> bool {
    project
        .device(id)
        .is_some_and(|device| device.type_id().as_str() == Ok("hex.utility"))
}
fn insertion_anchor(
    project: SnapshotProjectRead<'_>,
    after_track: Option<TrackId>,
) -> Option<TrackId> {
    let index = after_track.and_then(|id| project.tracks().position(|track| track.id() == id))?;
    project.tracks().nth(index + 1).map(|track| track.id())
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::super::clip_drag_ui_test::{audio_project, editor_window};
    use super::*;
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
    use std::sync::Arc;

    #[gpui::test]
    fn clip_right_click_toggles_flavor_without_track_insertion(cx: &mut TestAppContext) {
        for midi in [false, true] {
            let (project, track, _, clip) = if midi {
                super::super::clip_drag_ui_test::midi_project()
            } else {
                audio_project()
            };
            let original_color = project.track(track).unwrap().color();
            let preview = if midi {
                Arc::new(
                    hex_engine::compile::project_with_midi_preview(
                        &project,
                        &hex_project::SortedEntries::new(),
                    )
                    .expect("midi preview")
                    .1,
                )
            } else {
                Arc::default()
            };
            let (handle, editor) = editor_window(cx, project, preview);
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.run_until_parked();
            let selector = Box::leak(
                format!(
                    "{}clip-visual-{}",
                    if midi { "midi-" } else { "" },
                    clip.value()
                )
                .into_boxed_str(),
            );
            let bounds = visual.debug_bounds(selector).expect("clip");
            let point = point(
                bounds.origin.x + bounds.size.width / 2.0,
                bounds.origin.y + px(20.0),
            );
            visual.simulate_mouse_down(point, MouseButton::Right, Modifiers::default());
            visual.simulate_mouse_up(point, MouseButton::Right, Modifiers::default());
            visual.run_until_parked();
            editor.read_with(cx, |view, _| {
                let menu = view.context_menu.as_ref().expect("clip menu");
                assert_eq!(menu.items.len(), 4);
                assert_eq!(menu.items[0].label, "use accent flavor");
            });
            visual.simulate_keystrokes("enter");
            editor.read_with(cx, |view, _| {
                assert!(view.context_menu.is_none());
                assert!(matches!(view.last_enqueued_edit, Some(Command::SetClipFlavor { clip: id, flavor: ClipFlavor::Accent }) if id == clip));
                assert_eq!(view.state.as_ref().unwrap().project.track(track).unwrap().color(), original_color);
            });
        }
    }

    // Private bounded startup for the new palette regression; never ambient
    // preferences/library roots or a production constructor/global setting edit.
    struct ColorTestDirectory(std::path::PathBuf);
    impl ColorTestDirectory {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "hex-track-colors-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).expect("private unique color test directory");
            std::fs::create_dir(path.join("samples")).unwrap();
            Self(path)
        }
    }
    impl Drop for ColorTestDirectory {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("remove only owned fixture directory");
        }
    }
    fn color_test_window(
        cx: &mut TestAppContext,
        project: hex_project::Project,
        directory: &std::path::Path,
    ) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
        use gpui::AppContext;
        let handle = *cx.add_window(|_, _| gpui::Empty);
        let editor = cx
            .update_window(handle, |_, window, cx| {
                let service = hex_session::service::Service::new(48_000, 32, 8_000_000).unwrap();
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new_for_test_with_browser_directory(window, cx, service, directory)
                });
                editor.update(cx, |view, cx| {
                    view.state = Some(Arc::new(hex_session::service::ViewState {
                        canonical_audio_source: None,
                        comparison_audio: None,
                        project: Arc::new(project),
                        waveforms: Default::default(),
                        audio_clip_waveforms: Default::default(),
                        midi_preview: Arc::default(),
                        gain_preview: Ok(Arc::default()),
                        timeline_frames: 384_000,
                        can_undo: false,
                        can_redo: false,
                        revision: 1,
                        unsaved: false,
                        document_path: None,
                    }));
                    assert_eq!(view.browser.directory, directory.join("samples"));
                    assert!(view.audio.is_none());
                    cx.notify();
                });
                editor
            })
            .unwrap();
        (handle, editor)
    }

    #[gpui::test]
    fn track_header_offers_nine_non_gray_swatches_and_stale_clip_action_is_ignored(
        cx: &mut TestAppContext,
    ) {
        let (project, track, _, clip) = audio_project();
        let mut model = hex_project::Editor::new(project, 8).unwrap();
        model
            .execute(Command::SetTrackColor {
                track,
                color: ElementColor::Orange,
            })
            .unwrap();
        let project = model.project().clone();
        let directory = ColorTestDirectory::new();
        let (handle, editor) = color_test_window(cx, project, &directory.0);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let selector =
            Box::leak(format!("arrangement-track-header-{}", track.value()).into_boxed_str());
        let bounds = visual.debug_bounds(selector).expect("track header");
        let position = point(bounds.origin.x + px(10.0), bounds.origin.y + px(4.0));
        visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
        visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            let menu = view.context_menu.as_ref().expect("track menu");
            assert_eq!(menu.items.len(), 13);
            assert_eq!(
                menu.items
                    .iter()
                    .filter(|item| item.swatch.is_some())
                    .count(),
                9
            );
            let choices: Vec<_> = menu
                .items
                .iter()
                .filter_map(|item| match item.action {
                    EditorContextAction::TrackColor { color, .. } => Some(color),
                    _ => None,
                })
                .collect();
            assert_eq!(choices, ElementColor::ALL[1..]);
            assert!(!choices.contains(&ElementColor::Gray));
            // Color and density are independent checkmark groups in one popup.
            let checked_colors: Vec<_> = menu
                .items
                .iter()
                .filter(|item| {
                    item.checked && matches!(item.action, EditorContextAction::TrackColor { .. })
                })
                .collect();
            assert_eq!(checked_colors.len(), 1);
            let checked_densities: Vec<_> = menu
                .items
                .iter()
                .filter_map(|item| {
                    if let EditorContextAction::AdaptiveGrid { density, .. } = item.action {
                        item.checked.then_some(density)
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(checked_densities, vec![view.snap_grid.density()]);
            assert_eq!(menu.items.iter().filter(|item| item.checked).count(), 2);
            assert!(
                checked_colors[0].label
                    == view
                        .state
                        .as_ref()
                        .unwrap()
                        .project
                        .track(track)
                        .unwrap()
                        .color()
                        .key()
            );
        });
        let swatch = visual
            .debug_bounds("context-menu-swatch-cyan")
            .expect("cyan swatch");
        let click = point(swatch.origin.x + px(5.0), swatch.origin.y + px(5.0));
        visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |view, _| {
            assert!(matches!(view.last_enqueued_edit, Some(Command::SetTrackColor { track: id, color: ElementColor::Cyan }) if id == track));
        });
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                view.last_enqueued_edit = None;
                let old_generation = view.project_generation;
                view.project_generation += 1;
                view.select_context_item(
                    EditorContextAction::ClipFlavor {
                        generation: old_generation,
                        track,
                        clip,
                        flavor: ClipFlavor::Accent,
                    },
                    window,
                    cx,
                );
                assert!(view.last_enqueued_edit.is_none());
            })
        });
    }

    #[gpui::test]
    fn right_click_track_inserts_below_clicked_track(cx: &mut TestAppContext) {
        let (project, source, target, _) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let selector = Box::leak(format!("arrangement-lane-{}", source.value()).into_boxed_str());
        let bounds = visual.debug_bounds(selector).expect("track lane");
        let position = point(
            bounds.origin.x + bounds.size.width - px(12.0),
            bounds.origin.y + px(20.0),
        );
        visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
        visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        let panel = visual
            .debug_bounds("context-menu-panel")
            .expect("menu panel");
        // Insert remains the FIRST row; the generic row selector is ambiguous
        // now that the same popup also contains adaptive-density choices.
        let click = point(panel.origin.x + px(15.0), panel.origin.y + px(14.0));
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
