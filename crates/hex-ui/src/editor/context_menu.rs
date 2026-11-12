#[path = "adaptive_grid_menu.rs"]
mod adaptive_grid_menu;

use super::{EditorView, ViewState, device_panel};
use crate::context_menu::{MenuItem, PopupMenu};
use gpui::{Context, Div, Pixels, Point, Window};
use hex_lattice_contract::{ParameterId, ParameterValue};
use hex_project::{ClipFlavor, ClipId, Command, DeviceId, ElementColor, SceneId, TrackId};
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

#[derive(Clone, Copy)]
enum ClipAliasOperation {
    Duplicate(super::duplicate::DuplicateKind),
    MakeUnique,
    ApplySettings,
}

#[derive(Clone)]
pub(super) struct ClipAliasAction {
    generation: u64,
    revision: u64,
    track: TrackId,
    clip: ClipId,
    operation: ClipAliasOperation,
}

#[derive(Clone)]
pub(super) enum EditorContextAction {
    SourcePicker(device_panel::SourcePickerChoice),
    HistoryPlayback(super::history_playback::MenuAction),
    Tags(super::track_tags::Action),
    Automation(super::device_automation::Action),
    ClipAlias(ClipAliasAction),
    Section(super::arrangement_sections::MenuAction),
    BounceTrack {
        generation: u64,
        revision: u64,
        track: TrackId,
    },
    TrackClipboard {
        generation: u64,
        revision: u64,
        action: hex_session::service::TrackClipboardAction,
    },
    TrackGroup {
        generation: u64,
        revision: u64,
        command: Command,
    },
    LoadFirstSampler(super::sampler_load::Target),
    CancelSamplerLoad {
        request_id: u64,
    },
    RemoveBrowserSource(super::browser::BrowserSection),
    RelocateBrowserPlace(std::path::PathBuf),
    AdaptiveGrid {
        generation: u64,
        density: super::musical_time::AdaptiveGridDensity,
    },
    WaveformDecibels {
        generation: u64,
        enabled: bool,
    },
    InputQuantization {
        generation: u64,
        policy: hex_project::InputQuantization,
    },
    LaunchQuantization {
        generation: u64,
        quantization: hex_engine::LaunchQuantization,
    },
    PlaceIcon {
        path: std::path::PathBuf,
        id: Option<&'static str>,
    },
    InsertTrack(InsertTrackAction),
    InsertReturn {
        generation: u64,
    },
    InsertScene {
        generation: u64,
        revision: u64,
        before: Option<SceneId>,
    },
    UtilityMode(UtilityModeAction),
    AddModulator(device_panel::AddModulatorAction),
    DeviceComparison(super::device_comparison::Intent),
    DevicePreset(super::device_preset::Intent),
    RenameTrackName {
        generation: u64,
        revision: u64,
        target: super::track_name::Target,
    },
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
        if before.is_some_and(|id| !state.project.scenes().iter().any(|scene| scene.id() == id)) {
            return;
        }
        let revision = state.revision;
        let previous_focus = self
            .context_menu
            .take()
            .and_then(|menu| menu.previous_focus)
            .or_else(|| window.focused(cx));
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.context_menu = Some(PopupMenu {
            scroll: gpui::ScrollHandle::new(),
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
            scroll: gpui::ScrollHandle::new(),
            icon_grid: None,
            position,
            selected: 0,
            focus,
            previous_focus,
            items: vec![
                MenuItem {
                    label: "insert track",
                    swatch: None,
                    checked: false,
                    action: EditorContextAction::InsertTrack(InsertTrackAction {
                        generation: self.project_generation,
                        before_track,
                    }),
                },
                MenuItem {
                    label: "insert return track",
                    swatch: None,
                    checked: false,
                    action: EditorContextAction::InsertReturn {
                        generation: self.project_generation,
                    },
                },
            ],
        });
        let revision = state.revision;
        let generation = self.project_generation;
        let paste_before = if after_track.is_some_and(|id| self.track_selection.contains(id)) {
            self.track_paste_anchor()
        } else {
            before_track
        };
        if let Some(tracks) = after_track.and_then(|id| self.track_membership(Some(id))) {
            use hex_session::service::TrackClipboardAction as Action;
            let group_tracks = self.track_group_membership(after_track);
            if let Some(menu) = &mut self.context_menu {
                menu.items.extend(group_tracks.map(|tracks| MenuItem {
                    label: "group tracks",
                    swatch: None,
                    checked: false,
                    action: EditorContextAction::TrackGroup {
                        generation,
                        revision,
                        command: Command::GroupTracks {
                            tracks,
                            name: "group".into(),
                        },
                    },
                }));
                if let Some(track) = after_track.filter(|id| {
                    state
                        .project
                        .track(*id)
                        .is_some_and(|track| track.is_group())
                }) {
                    menu.items.push(MenuItem {
                        label: "ungroup tracks",
                        swatch: None,
                        checked: false,
                        action: EditorContextAction::TrackGroup {
                            generation,
                            revision,
                            command: Command::UngroupTrack(track),
                        },
                    });
                    menu.items.push(MenuItem {
                        label: "delete group and tracks",
                        swatch: None,
                        checked: false,
                        action: EditorContextAction::TrackGroup {
                            generation,
                            revision,
                            command: Command::RemoveTrack(track),
                        },
                    });
                }
            }
            let choices = [
                ("cut tracks · cmd+x", Action::Cut(tracks.clone())),
                ("copy tracks · cmd+c", Action::Copy(tracks.clone())),
                (
                    "duplicate tracks · cmd+d",
                    Action::Duplicate {
                        tracks: tracks.clone(),
                        without_clips: false,
                    },
                ),
                (
                    "duplicate tracks without clips",
                    Action::Duplicate {
                        tracks,
                        without_clips: true,
                    },
                ),
                (
                    "paste tracks · cmd+v",
                    Action::Paste {
                        before: paste_before,
                    },
                ),
            ];
            if let Some(menu) = &mut self.context_menu {
                menu.items
                    .extend(choices.into_iter().map(|(label, action)| MenuItem {
                        label,
                        swatch: None,
                        checked: false,
                        action: EditorContextAction::TrackClipboard {
                            generation,
                            revision,
                            action,
                        },
                    }));
            }
        } else if let Some(menu) = &mut self.context_menu {
            menu.items.push(MenuItem {
                label: "paste tracks · cmd+v",
                swatch: None,
                checked: false,
                action: EditorContextAction::TrackClipboard {
                    generation,
                    revision,
                    action: hex_session::service::TrackClipboardAction::Paste {
                        before: before_track,
                    },
                },
            });
        }
        if let Some(track) = after_track
            && let Some(menu) = &mut self.context_menu
        {
            menu.items.push(MenuItem {
                label: "bounce in place",
                swatch: None,
                checked: false,
                action: EditorContextAction::BounceTrack {
                    generation,
                    revision,
                    track,
                },
            });
        }
        if let Some(track) = after_track {
            if let Some(menu) = &mut self.context_menu {
                menu.items.push(MenuItem {
                    label: "edit tags · comma separated",
                    swatch: None,
                    checked: false,
                    action: EditorContextAction::Tags(super::track_tags::Action::Edit {
                        generation,
                        revision,
                        track,
                    }),
                });
            }
            self.append_automation_menu(device_panel::DevicePanelOwner::Track(track));
        }
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
        if !device_panel::finalize_controls(self, cx) {
            return;
        }
        let Some(color) = self
            .state
            .as_ref()
            .and_then(|s| s.project.track(track))
            .map(|t| t.color())
        else {
            return;
        };
        self.open_track_context_menu(position, Some(track), window, cx);
        if let Some(menu) = &mut self.context_menu {
            menu.items.push(MenuItem {
                label: "rename track · cmd+r",
                swatch: None,
                checked: false,
                action: EditorContextAction::RenameTrackName {
                    generation: self.project_generation,
                    revision: self.state.as_ref().map_or(0, |state| state.revision),
                    target: super::track_name::Target::Track(track),
                },
            });
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

    pub(super) fn open_return_name_context_menu(
        &mut self,
        position: Point<Pixels>,
        id: hex_project::ReturnId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.edit_pending() || !device_panel::finalize_controls(self, cx) {
            return;
        }
        let target = super::track_name::Target::Return(id);
        let Some(state) = &self.state else {
            return;
        };
        if target.name(&state.project).is_none() {
            return;
        }
        let revision = state.revision;
        self.open_track_context_menu(position, None, window, cx);
        self.append_automation_menu(device_panel::DevicePanelOwner::Return(id));
        if let Some(menu) = &mut self.context_menu {
            menu.items.push(MenuItem {
                label: "rename track · cmd+r",
                swatch: None,
                checked: false,
                action: EditorContextAction::RenameTrackName {
                    generation: self.project_generation,
                    revision,
                    target,
                },
            });
        }
        cx.notify();
    }

    pub(super) fn open_clip_context_menu(
        &mut self,
        position: Point<Pixels>,
        track: TrackId,
        clip: ClipId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !device_panel::finalize_controls(self, cx) {
            return;
        }
        self.cancel_time_selection_gesture();
        let Some(flavor) = self
            .state
            .as_ref()
            .filter(|s| clip_belongs_to_track(&s.project, track, clip))
            .and_then(|s| s.project.clip_flavor(clip))
        else {
            return;
        };
        let previous_focus = self
            .context_menu
            .take()
            .and_then(|m| m.previous_focus)
            .or_else(|| window.focused(cx));
        let focus = cx.focus_handle();
        window.focus(&focus);
        let target = flavor.toggled();
        self.context_menu = Some(PopupMenu {
            scroll: gpui::ScrollHandle::new(),
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
                swatch: self
                    .state
                    .as_ref()
                    .and_then(|s| s.project.track(track))
                    .map(|t| crate::theme::clip_fill(t.color(), target, false)),
                checked: false,
                action: EditorContextAction::ClipFlavor {
                    generation: self.project_generation,
                    track,
                    clip,
                    flavor: target,
                },
            }],
        });
        if let Some(state) = &self.state {
            let generation = self.project_generation;
            let revision = state.revision;
            let linked = state.project.clip_alias(clip).is_some();
            let mut entries = vec![
                (
                    "duplicate · d",
                    ClipAliasOperation::Duplicate(super::duplicate::DuplicateKind::Independent),
                ),
                (
                    "duplicate alias · cmd+d",
                    ClipAliasOperation::Duplicate(super::duplicate::DuplicateKind::Alias),
                ),
            ];
            if linked {
                entries.push(("make unique · shift+i", ClipAliasOperation::MakeUnique));
                entries.push((
                    "apply settings to aliases",
                    ClipAliasOperation::ApplySettings,
                ));
            }
            if let Some(menu) = &mut self.context_menu {
                menu.items
                    .extend(entries.into_iter().map(|(label, operation)| MenuItem {
                        label,
                        swatch: None,
                        checked: false,
                        action: EditorContextAction::ClipAlias(ClipAliasAction {
                            generation,
                            revision,
                            track,
                            clip,
                            operation,
                        }),
                    }));
            }
        }
        if self.mode == super::EditorMode::Arrangement {
            self.append_adaptive_grid_choices();
        }
        cx.notify();
    }

    pub(super) fn append_utility_mode_choice(&mut self, device: DeviceId, mid_side: bool) {
        let Some(owner) = self.device_panel.owner else {
            return;
        };
        if let Some(menu) = &mut self.context_menu {
            menu.items.push(MenuItem {
                label: if mid_side {
                    "switch to width"
                } else {
                    "switch to mid/side"
                },
                swatch: None,
                checked: false,
                action: EditorContextAction::UtilityMode(UtilityModeAction {
                    generation: self.project_generation,
                    owner,
                    device,
                    mid_side: !mid_side,
                }),
            });
        }
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
            scroll: gpui::ScrollHandle::new(),
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
        self.dismiss_context_menu(window, cx);
        match action {
            EditorContextAction::SourcePicker(choice) => choice.perform(self, cx),
            EditorContextAction::HistoryPlayback(action) => {
                self.history_playback_menu_action(action, cx)
            }
            EditorContextAction::Tags(action) => self.perform_tag_action(action, window, cx),
            EditorContextAction::Automation(action) => {
                self.automation_menu_action(action, window, cx)
            }
            EditorContextAction::ClipAlias(target) => {
                if !self.clip_alias_menu_current(&target) || self.edit_pending() {
                    return;
                }
                let console = self.state.as_ref().and_then(|s| {
                    s.project.session_cells().iter().find_map(|cell| {
                        if cell.track() != target.track {
                            return None;
                        }
                        match cell.content() {
                            hex_project::SessionCellContent::Midi(c) if c.id() == target.clip => {
                                Some((cell.scene(), true))
                            }
                            hex_project::SessionCellContent::Audio(c) if c.id() == target.clip => {
                                Some((cell.scene(), false))
                            }
                            _ => None,
                        }
                    })
                });
                if let Some((scene, midi)) = console {
                    if midi {
                        self.select_session_midi(target.track, scene, target.clip);
                    } else {
                        self.select_session_cell(target.track, scene);
                    }
                } else if !self.selected_clip_ids().contains(&target.clip) {
                    self.select_clip_header(
                        target.track,
                        target.clip,
                        gpui::Modifiers::default(),
                        window,
                        cx,
                    );
                }
                match target.operation {
                    ClipAliasOperation::Duplicate(kind) => self.duplicate_selected_with_kind(kind),
                    ClipAliasOperation::MakeUnique => self.make_selected_clips_unique(),
                    ClipAliasOperation::ApplySettings => {
                        self.enqueue(Request::Edit(Command::ApplyClipSettingsToAliases(
                            target.clip,
                        )));
                    }
                }
                cx.notify();
            }
            EditorContextAction::InputQuantization { generation, policy } => {
                if generation == self.project_generation {
                    self.set_input_quantization(policy);
                    cx.notify();
                }
            }
            EditorContextAction::LaunchQuantization {
                generation,
                quantization,
            } => {
                if generation == self.project_generation {
                    self.session_quantization = quantization;
                    cx.notify();
                }
            }
            EditorContextAction::Section(action) => self.section_menu_action(action, window, cx),
            EditorContextAction::LoadFirstSampler(target) => self.load_first_sampler(target, cx),
            EditorContextAction::CancelSamplerLoad { request_id } => {
                self.cancel_first_sampler_load(Some(request_id))
            }
            EditorContextAction::RemoveBrowserSource(source) => {
                self.remove_browser_source(source, window, cx);
            }
            EditorContextAction::RelocateBrowserPlace(path) => {
                self.prompt_relocate_browser_place(path, cx);
            }
            EditorContextAction::AdaptiveGrid {
                generation,
                density,
            } => {
                if generation == self.project_generation {
                    self.set_adaptive_grid_density(density);
                    cx.notify();
                }
            }
            EditorContextAction::WaveformDecibels {
                generation,
                enabled,
            } => {
                if generation == self.project_generation {
                    self.waveform_geometry_cache.set_amplitude_scale(
                        super::waveform::WaveformAmplitudeScale::from_decibels(enabled),
                    );
                    cx.notify();
                }
            }
            EditorContextAction::DeviceComparison(target) => {
                self.perform_device_comparison(target, cx)
            }
            EditorContextAction::AddModulator(action) => action.perform(self),
            EditorContextAction::DevicePreset(target) => {
                self.perform_device_preset_action(target, cx)
            }
            EditorContextAction::BounceTrack {
                generation,
                revision,
                track,
            } => {
                self.bounce_track_in_place(track, generation, revision);
                cx.notify();
            }
            EditorContextAction::TrackClipboard {
                generation,
                revision,
                action,
            } => self.start_track_clipboard(generation, revision, action),
            EditorContextAction::TrackGroup {
                generation,
                revision,
                command,
            } => self.perform_track_group(generation, revision, command, window, cx),
            EditorContextAction::PlaceIcon { path, id } => {
                self.set_browser_place_icon(path, id, cx)
            }
            EditorContextAction::RenameTrackName {
                generation,
                revision,
                target,
            } => self.begin_captured_name(target, generation, revision, window, cx),
            EditorContextAction::TrackColor {
                generation,
                track,
                color,
            } => {
                if generation == self.project_generation
                    && self
                        .state
                        .as_ref()
                        .is_some_and(|s| s.project.track(track).is_some())
                {
                    self.enqueue(Request::Edit(Command::SetTrackColor { track, color }));
                    cx.notify();
                }
            }
            EditorContextAction::ClipFlavor {
                generation,
                track,
                clip,
                flavor,
            } => {
                if generation == self.project_generation
                    && self
                        .state
                        .as_ref()
                        .is_some_and(|s| clip_belongs_to_track(&s.project, track, clip))
                {
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
            EditorContextAction::InsertReturn { generation } => {
                if generation != self.project_generation {
                    return;
                }
                let Some(state) = &self.state else {
                    return;
                };
                let count = state.project.return_tracks().len();
                if count >= hex_project::MAX_RETURN_TRACKS {
                    self.error = Some("return track limit reached".into());
                    cx.notify();
                    return;
                }
                let gain = match self.track_defaults.captured_gain() {
                    Ok(gain) => gain,
                    Err(error) => {
                        self.error = Some(error.into());
                        cx.notify();
                        return;
                    }
                };
                if device_panel::enqueue_return_edit(
                    self,
                    Command::AddReturnTrackWithGain {
                        name: format!("return {}", count + 1),
                        gain,
                    },
                    cx,
                ) {
                    self.error = None;
                }
                cx.notify();
            }
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
                self.insert_default_track(action.before_track);
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

    pub(super) fn perform_track_group(
        &mut self,
        generation: u64,
        revision: u64,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.track_name.is_editing()
            || self.clip_name_edit.is_some()
            || self.track_reorder_active()
            || self.track_resize_active()
            || self.active_clip_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_midi_trim_drag.is_some()
            || self.active_fade_drag.is_some()
            || self.time_selection_gesture.is_some()
        {
            self.error = Some("finish the current edit before changing tracks".into());
            cx.notify();
            return;
        }
        if generation == self.project_generation
            && self
                .state
                .as_ref()
                .is_some_and(|state| state.revision == revision)
            && !self.edit_pending()
            && device_panel::finalize_controls(self, cx)
        {
            self.focus.focus(window);
            self.enqueue(Request::Edit(command));
            cx.notify();
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
                if menu.icon_grid.is_none() {
                    menu.scroll.scroll_to_item(menu.selected);
                }
            }
            cx.notify();
        }
    }

    fn clip_alias_menu_current(&self, target: &ClipAliasAction) -> bool {
        target.generation == self.project_generation
            && self.state.as_ref().is_some_and(|s| {
                s.revision == target.revision
                    && clip_belongs_to_track(&s.project, target.track, target.clip)
                    && (matches!(target.operation, ClipAliasOperation::Duplicate(_))
                        || s.project.clip_alias(target.clip).is_some())
            })
    }

    pub(super) fn context_menu_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if self.context_menu.as_ref().is_some_and(|menu| {
            menu.items.iter().any(|item| match &item.action {
                EditorContextAction::SourcePicker(choice) => !choice.current(self),
                EditorContextAction::HistoryPlayback(action) => {
                    !self.history_playback_menu_current(action)
                }
                EditorContextAction::Tags(action) => action.stale(self),
                EditorContextAction::Automation(action) => self.automation_action_stale(action),
                EditorContextAction::ClipAlias(target) => !self.clip_alias_menu_current(target),
                EditorContextAction::Section(action) => self.section_menu_stale(action),
                EditorContextAction::LoadFirstSampler(target) => {
                    !self.first_sampler_target_current(target)
                }
                EditorContextAction::CancelSamplerLoad { request_id } => {
                    !self.first_sampler_pending_matches(*request_id)
                }
                EditorContextAction::DeviceComparison(target) => {
                    !super::device_comparison::current(self, target)
                }
                EditorContextAction::AddModulator(action) => !action.current(self),
                EditorContextAction::DevicePreset(target) => {
                    !super::device_preset::current(self, target)
                }
                EditorContextAction::PlaceIcon { path, .. } => {
                    !self.browser_place_icon_assignable(path)
                }
                EditorContextAction::RemoveBrowserSource(source) => {
                    !self.browser_source_removable(source)
                }
                EditorContextAction::RelocateBrowserPlace(path) => {
                    !self.browser_place_relocatable(path)
                }
                EditorContextAction::BounceTrack {
                    generation,
                    revision,
                    track,
                } => {
                    *generation != self.project_generation
                        || self.state.as_ref().is_none_or(|s| {
                            s.revision != *revision || s.project.track(*track).is_none()
                        })
                }
                EditorContextAction::TrackClipboard {
                    generation,
                    revision,
                    ..
                }
                | EditorContextAction::TrackGroup {
                    generation,
                    revision,
                    ..
                } => {
                    *generation != self.project_generation
                        || self.state.as_ref().is_none_or(|s| s.revision != *revision)
                }
                EditorContextAction::InsertTrack(_) => false,
                EditorContextAction::InsertReturn { generation } => {
                    *generation != self.project_generation
                }
                EditorContextAction::AdaptiveGrid { generation, .. }
                | EditorContextAction::WaveformDecibels { generation, .. }
                | EditorContextAction::LaunchQuantization { generation, .. }
                | EditorContextAction::InputQuantization { generation, .. } => {
                    *generation != self.project_generation
                }
                EditorContextAction::InsertScene {
                    generation,
                    revision,
                    before,
                } => {
                    self.mode != super::EditorMode::Session
                        || *generation != self.project_generation
                        || self.state.as_ref().is_none_or(|state| {
                            state.revision != *revision
                                || before.is_some_and(|id| {
                                    !state.project.scenes().iter().any(|scene| scene.id() == id)
                                })
                        })
                }
                EditorContextAction::RenameTrackName {
                    generation,
                    revision,
                    target,
                } => {
                    *generation != self.project_generation
                        || self.state.as_ref().is_none_or(|state| {
                            state.revision != *revision || target.name(&state.project).is_none()
                        })
                }
                EditorContextAction::TrackColor {
                    generation, track, ..
                } => {
                    *generation != self.project_generation
                        || self
                            .state
                            .as_ref()
                            .is_none_or(|s| s.project.track(*track).is_none())
                }
                EditorContextAction::ClipFlavor {
                    generation,
                    track,
                    clip,
                    ..
                } => {
                    *generation != self.project_generation
                        || !self
                            .state
                            .as_ref()
                            .is_some_and(|s| clip_belongs_to_track(&s.project, *track, *clip))
                }
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
            if !menu
                .items
                .iter()
                .any(|item| matches!(item.action, EditorContextAction::Automation(_) | EditorContextAction::SourcePicker(_)))
            {
                return crate::context_menu::popup(
                    menu,
                    window,
                    cx,
                    Self::select_context_item,
                    Self::dismiss_context_menu,
                    Self::navigate_context_menu,
                );
            }
            crate::context_menu::popup_with_labels(
                menu,
                window,
                cx,
                Self::select_context_item,
                Self::dismiss_context_menu,
                Self::navigate_context_menu,
                context_item_label,
            )
        })
    }
}

pub(super) fn context_item_label(item: &MenuItem<EditorContextAction>) -> gpui::SharedString {
    match &item.action {
        EditorContextAction::SourcePicker(choice) => choice.label.clone(),
        EditorContextAction::Tags(action) => action.label(),
        EditorContextAction::Automation(super::device_automation::Action::Show(entry, _))
            if item.label == "parameter" =>
        {
            entry.label.clone()
        }
        EditorContextAction::Automation(super::device_automation::Action::Device {
            label, ..
        }) => label.clone(),
        _ => item.label.into(),
    }
}

fn clip_belongs_to_track(project: &hex_project::Project, track: TrackId, clip: ClipId) -> bool {
    project.track(track).is_some_and(|t| {
        t.clips().iter().any(|c| c.id() == clip) || t.midi_clips().iter().any(|c| c.id() == clip)
    }) || project.session_cells().iter().any(|cell| {
        cell.track() == track
            && match cell.content() {
                hex_project::SessionCellContent::Audio(c) => c.id() == clip,
                hex_project::SessionCellContent::Midi(c) => c.id() == clip,
                _ => false,
            }
    })
}

fn utility_device_exists(project: &hex_project::Project, id: DeviceId) -> bool {
    project
        .main_devices()
        .iter()
        .any(|device| device.id() == id && device.state().type_id().as_str() == Ok("hex.utility"))
        || project.tracks().iter().any(|track| {
            track.ordered_device_chain().any(|node| {
                let device = match node {
                    hex_project::TrackDeviceRef::Instrument(device)
                    | hex_project::TrackDeviceRef::Effect(device) => device,
                };
                device.id() == id && device.state().type_id().as_str() == Ok("hex.utility")
            })
        })
        || project.return_tracks().iter().any(|track| {
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
    fn stale_group_delete_action_cannot_remove_newer_project_tracks(cx: &mut TestAppContext) {
        use super::super::track_reorder_ui_test::{live_tracks, receive};
        use hex_project::TrackSelectionIds;
        let (mut visual, editor, ids) = live_tracks(cx, 1, false);
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&ids).unwrap(),
                name: "group".into(),
            })))
        });
        receive(&mut visual, &editor);
        let group = editor.read_with(&visual, |view, _| {
            view.state
                .as_ref()
                .unwrap()
                .project
                .tracks()
                .iter()
                .find(|track| track.is_group())
                .unwrap()
                .id()
        });
        let action = visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                view.open_track_context_menu(point(px(100.0), px(100.0)), Some(group), window, cx);
                view.context_menu
                    .as_ref()
                    .unwrap()
                    .items
                    .iter()
                    .find(|item| item.label == "delete group and tracks")
                    .unwrap()
                    .action
                    .clone()
            })
        });
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddTrack {
                name: "newer track".into()
            })))
        });
        receive(&mut visual, &editor);
        let before = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.snapshot()
        });
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| view.select_context_item(action, window, cx))
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
            assert_eq!(view.worker_requests_pending, 0);
        });
    }

    #[gpui::test]
    fn stale_group_action_cannot_edit_newer_project(cx: &mut TestAppContext) {
        use super::super::track_reorder_ui_test::{live_tracks, receive};
        let (mut visual, editor, ids) = live_tracks(cx, 1, false);
        let action = visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                view.open_track_context_menu(point(px(100.0), px(100.0)), Some(ids[0]), window, cx);
                view.context_menu
                    .as_ref()
                    .unwrap()
                    .items
                    .iter()
                    .find(|item| item.label == "group tracks")
                    .unwrap()
                    .action
                    .clone()
            })
        });
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddTrack {
                name: "newer track".into()
            })))
        });
        receive(&mut visual, &editor);
        let revision = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().revision);
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| view.select_context_item(action, window, cx))
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().revision, revision);
            assert!(!view.state.as_ref().unwrap().project.has_group_tracks());
            assert_eq!(view.worker_requests_pending, 0);
        });
    }

    #[gpui::test]
    fn group_action_refuses_during_track_name_edit(cx: &mut TestAppContext) {
        use super::super::track_reorder_ui_test::{bounds, live_tracks};
        let (mut visual, editor, ids) = live_tracks(cx, 1, false);
        let action = visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                view.open_track_context_menu(point(px(100.0), px(100.0)), Some(ids[0]), window, cx);
                view.context_menu
                    .as_ref()
                    .unwrap()
                    .items
                    .iter()
                    .find(|item| item.label == "group tracks")
                    .unwrap()
                    .action
                    .clone()
            })
        });
        visual.simulate_keystrokes("escape");
        let at = bounds(&mut visual, ids[0]).center();
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-r");
        visual.run_until_parked();
        assert!(editor.read_with(&visual, |view, _| view.track_name.is_editing()));
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| view.select_context_item(action, window, cx))
        });
        editor.read_with(&visual, |view, _| {
            assert!(view.track_name.is_editing());
            assert!(!view.state.as_ref().unwrap().project.has_group_tracks());
            assert_eq!(view.worker_requests_pending, 0);
        });
    }

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
                    hex_engine::compile::project_with_midi_preview(&project, &Default::default())
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
                assert!(menu.items.iter().all(|item| matches!(
                    &item.action,
                    EditorContextAction::ClipFlavor { .. }
                        | EditorContextAction::ClipAlias(_)
                        | EditorContextAction::AdaptiveGrid { .. }
                        | EditorContextAction::WaveformDecibels { .. }
                )));
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
                        source_overviews: Default::default(),
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
    fn global_launch_dropdown_renders_all_choices_and_restores_focus_without_project_edits(
        cx: &mut TestAppContext,
    ) {
        use gpui::AppContext;
        cx.update(super::super::bind_editor_keys);
        let (project, _, _, _) = audio_project();
        let before = project.clone();
        let directory = ColorTestDirectory::new();
        let (handle, editor) = color_test_window(cx, project, &directory.0);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let bounds = visual.debug_bounds("global-launch-quantization").unwrap();
        let position = bounds.center();
        visual.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            let menu = view.context_menu.as_ref().unwrap();
            assert_eq!(
                menu.items.iter().map(|item| item.label).collect::<Vec<_>>(),
                [
                    "none", "8 bars", "4 bars", "2 bars", "1 bar", "1/2", "1/2t", "1/4", "1/4t",
                    "1/8", "1/8t", "1/16", "1/16t", "1/32"
                ]
            );
            assert_eq!(menu.items.iter().filter(|item| item.checked).count(), 1);
        });
        visual.simulate_keystrokes("down down down down enter");
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(view.context_menu.is_none());
            assert_eq!(
                view.session_quantization,
                hex_engine::LaunchQuantization::OneBar
            );
            assert_eq!(view.state.as_ref().unwrap().project.as_ref(), &before);
        });
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                let generation = view.project_generation;
                view.project_generation += 1;
                view.select_context_item(
                    EditorContextAction::LaunchQuantization {
                        generation,
                        quantization: hex_engine::LaunchQuantization::Immediate,
                    },
                    window,
                    cx,
                );
                assert_eq!(
                    view.session_quantization,
                    hex_engine::LaunchQuantization::OneBar
                );
            });
        })
        .unwrap();
    }

    #[gpui::test]
    fn track_header_offers_eight_non_blue_swatches_and_stale_clip_action_is_ignored(
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
            assert_eq!(
                menu.items
                    .iter()
                    .filter(|item| item.swatch.is_some())
                    .count(),
                8
            );
            let choices: Vec<_> = menu
                .items
                .iter()
                .filter_map(|item| match item.action {
                    EditorContextAction::TrackColor { color, .. } => Some(color),
                    _ => None,
                })
                .collect();
            assert_eq!(choices, ElementColor::TRACK_CHOICES);
            assert!(!choices.contains(&ElementColor::Gray));
            assert!(!choices.contains(&ElementColor::Blue));
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
            assert!(matches!(view.last_enqueued_edit, Some(Command::InsertDefaultTrackWithGain { before_track: Some(anchor), .. }) if anchor == target));
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
                Some(Command::InsertDefaultTrackWithGain {
                    before_track: None,
                    ..
                })
            ));
        });
    }

    #[gpui::test]
    fn context_rename_uses_clicked_ordinary_or_return_identity_and_real_history(
        cx: &mut TestAppContext,
    ) {
        use crate::editor::midi_canvas::creation_tests::{fixture, pump};
        for return_target in [false, true] {
            let (mut visual, editor, environment, _) = fixture(cx);
            for command in [
                Command::AddTrack {
                    name: "target".into(),
                },
                Command::AddReturnTrack {
                    name: "return".into(),
                },
            ] {
                editor.update(&mut visual, |view, _| {
                    assert!(view.enqueue(Request::Edit(command)))
                });
                pump(&editor, &mut visual);
            }
            let (first, target, return_id, original) = editor.read_with(&visual, |view, _| {
                let p = &view.state.as_ref().unwrap().project;
                (
                    p.tracks()[0].id(),
                    p.tracks()[1].id(),
                    p.return_tracks()[0].id(),
                    p.clone(),
                )
            });
            let first_name = Box::leak(format!("track-name-{}", first.value()).into_boxed_str());
            let first_hit = visual.debug_bounds(first_name).unwrap().center();
            visual.simulate_click(first_hit, Modifiers::default());
            visual.run_until_parked();
            let selector = if return_target {
                format!("return-track-focus-{}", return_id.value())
            } else {
                format!("arrangement-track-header-{}", target.value())
            };
            let hit = visual
                .debug_bounds(Box::leak(selector.into_boxed_str()))
                .unwrap()
                .center();
            visual.simulate_mouse_down(hit, MouseButton::Right, Modifiers::default());
            visual.simulate_mouse_up(hit, MouseButton::Right, Modifiers::default());
            visual.run_until_parked();
            let index = editor.read_with(&visual, |view, _| {
                assert_eq!(view.selected_track, Some(first));
                view.context_menu
                    .as_ref()
                    .expect("clicked row menu")
                    .items
                    .iter()
                    .position(|item| item.label == "rename track · cmd+r")
                    .expect("rename action for clicked row")
            });
            for _ in 0..index {
                visual.simulate_keystrokes("down");
            }
            visual.simulate_keystrokes("enter");
            visual.run_until_parked();
            let input = if return_target {
                "return-name-input"
            } else {
                "track-name-input"
            };
            assert!(visual.debug_bounds(input).is_some());
            assert!(
                visual.debug_bounds("browser-search-clear").is_none(),
                "shared name mode must not acquire a search clear action"
            );
            visual.simulate_input("Lead Ω");
            visual.simulate_keystrokes("enter");
            pump(&editor, &mut visual);
            let renamed = editor.read_with(&visual, |view, _| {
                let p = &view.state.as_ref().unwrap().project;
                if return_target {
                    assert_eq!(
                        p.return_tracks()
                            .iter()
                            .find(|track| track.id() == return_id)
                            .unwrap()
                            .name(),
                        "Lead Ω"
                    );
                    assert_eq!(p.track(target), original.track(target));
                } else {
                    assert_eq!(p.track(target).unwrap().name(), "Lead Ω");
                    assert_eq!(
                        p.return_tracks()
                            .iter()
                            .find(|track| track.id() == return_id),
                        original
                            .return_tracks()
                            .iter()
                            .find(|track| track.id() == return_id)
                    );
                }
                assert_eq!(p.track(first), original.track(first));
                p.clone()
            });
            for (request, expected) in [(Request::Undo, &original), (Request::Redo, &renamed)] {
                editor.update(&mut visual, |view, _| assert!(view.enqueue(request)));
                pump(&editor, &mut visual);
                editor.read_with(&visual, |view, _| {
                    assert_eq!(&*view.state.as_ref().unwrap().project, &**expected)
                });
            }
            // The same inline controller also owns keyboard rename and Escape.
            visual.simulate_keystrokes("cmd-r");
            visual.run_until_parked();
            assert!(visual.debug_bounds(input).is_some());
            visual.simulate_input("cancelled draft");
            visual.simulate_keystrokes("escape");
            visual.run_until_parked();
            editor.read_with(&visual, |view, _| {
                assert_eq!(view.worker_requests_pending, 0);
                assert_eq!(view.state.as_ref().unwrap().project, renamed);
            });
            let path = environment.root().join("renamed.hex");
            editor.update(&mut visual, |view, _| {
                assert!(view.enqueue(Request::SaveNew(path.clone())))
            });
            pump(&editor, &mut visual);
            editor.update(&mut visual, |view, _| {
                assert!(view.enqueue(Request::Open(path)))
            });
            pump(&editor, &mut visual);
            editor.read_with(&visual, |view, _| {
                assert_eq!(view.state.as_ref().unwrap().project, renamed)
            });
        }
    }

    #[gpui::test]
    fn context_rename_stale_revision_deleted_target_and_replaced_document_are_noops(
        cx: &mut TestAppContext,
    ) {
        use crate::editor::midi_canvas::creation_tests::{fixture, pump};
        for return_target in [false, true] {
            for control in 0..3 {
                let (mut visual, editor, _environment, _) = fixture(cx);
                editor.update(&mut visual, |view, _| {
                    assert!(view.enqueue(Request::Edit(Command::AddReturnTrack {
                        name: "return".into()
                    })))
                });
                pump(&editor, &mut visual);
                let (track, return_id) = editor.read_with(&visual, |view, _| {
                    let p = &view.state.as_ref().unwrap().project;
                    (p.tracks()[0].id(), p.return_tracks()[0].id())
                });
                let selector = if return_target {
                    format!("return-track-focus-{}", return_id.value())
                } else {
                    format!("arrangement-track-header-{}", track.value())
                };
                let bounds = visual
                    .debug_bounds(Box::leak(selector.into_boxed_str()))
                    .unwrap();
                let hit = point(bounds.left() + px(10.0), bounds.top() + px(4.0));
                visual.simulate_mouse_down(hit, MouseButton::Right, Modifiers::default());
                visual.simulate_mouse_up(hit, MouseButton::Right, Modifiers::default());
                visual.run_until_parked();
                let answer = editor.read_with(&visual, |view, _| {
                    view.context_menu
                        .as_ref()
                        .unwrap()
                        .items
                        .iter()
                        .find(|item| item.label == "rename track · cmd+r")
                        .unwrap()
                        .action
                        .clone()
                });
                let request = match control {
                    0 => Request::Edit(Command::Gain { track, gain: 0.5 }),
                    1 if return_target => Request::Edit(Command::RemoveReturnTrack(return_id)),
                    1 => Request::Edit(Command::RemoveTrack(track)),
                    _ => Request::NewWithAudioMode {
                        mode: hex_session::DocumentAudioMode::Resident,
                        reservation: None,
                    },
                };
                editor.update(&mut visual, |view, _| assert!(view.enqueue(request)));
                pump(&editor, &mut visual);
                let before = editor.read_with(&visual, |view, _| {
                    let state = view.state.as_ref().unwrap();
                    assert!(view.context_menu.is_none(), "stale menu retires");
                    (
                        state.project.clone(),
                        state.revision,
                        state.can_undo,
                        state.can_redo,
                    )
                });
                // Deliver a genuinely captured answer after its original menu/source retired.
                visual.update(|window, cx| {
                    editor.update(cx, |view, cx| view.select_context_item(answer, window, cx))
                });
                visual.run_until_parked();
                editor.read_with(&visual, |view, _| {
                    let state = view.state.as_ref().unwrap();
                    assert!(!view.track_name.is_editing());
                    assert_eq!(view.worker_requests_pending, 0);
                    assert_eq!(
                        (
                            state.project.clone(),
                            state.revision,
                            state.can_undo,
                            state.can_redo
                        ),
                        before
                    );
                });
            }
        }
    }
    fn track_menu_inserts_return_with_real_history_and_saved_routing(
        cx: &mut TestAppContext,
        mode: super::super::EditorMode,
    ) {
        use crate::editor::midi_canvas::creation_tests::{fixture, pump};
        let (mut visual, editor, environment, _) = fixture(cx);
        editor.update(&mut visual, |view, cx| {
            view.mode = mode;
            cx.notify();
        });
        visual.run_until_parked();
        editor.update(&mut visual, |view, _| {
            view.track_defaults.set_test_state(0.5, false, true)
        });
        let original = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.clone()
        });
        let track = original.tracks()[0].id();
        let prefix = if mode == super::super::EditorMode::Arrangement {
            "arrangement-track-header"
        } else {
            "session-track"
        };
        let header = visual
            .debug_bounds(Box::leak(
                format!("{prefix}-{}", track.value()).into_boxed_str(),
            ))
            .unwrap();
        visual.simulate_mouse_down(header.center(), MouseButton::Right, Modifiers::default());
        visual.simulate_mouse_up(header.center(), MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            let menu = view.context_menu.as_ref().unwrap();
            assert_eq!(menu.items[0].label, "insert track");
            assert_eq!(menu.items[1].label, "insert return track");
            assert!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .return_tracks()
                    .is_empty()
            );
        });
        visual.simulate_keystrokes("down enter");
        editor.update(&mut visual, |view, _| {
            view.track_defaults.set_test_state(0.25, false, true)
        });
        pump(&editor, &mut visual);
        let inserted = editor.read_with(&visual, |view, _| {
            let p = &view.state.as_ref().unwrap().project;
            assert_eq!(p.tracks(), original.tracks());
            assert_eq!(p.return_tracks().len(), 1);
            assert_eq!(p.return_tracks()[0].name(), "return 1");
            assert_eq!(p.return_tracks()[0].gain(), 0.5);
            assert!(p.return_tracks()[0].devices().is_empty());
            assert!(view.context_menu.is_none());
            p.clone()
        });
        editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            let mut expected = original.snapshot();
            expected.next_id = inserted.snapshot().next_id;
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), expected);
        });
        editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, inserted)
        });
        let path = environment.root().join("return-menu.hex");
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::SaveNew(path.clone())))
        });
        pump(&editor, &mut visual);
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Open(path)))
        });
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, inserted)
        });
    }

    #[gpui::test]
    fn track_menu_return_arrangement(cx: &mut TestAppContext) {
        track_menu_inserts_return_with_real_history_and_saved_routing(
            cx,
            super::super::EditorMode::Arrangement,
        );
    }

    #[gpui::test]
    fn track_menu_return_session(cx: &mut TestAppContext) {
        track_menu_inserts_return_with_real_history_and_saved_routing(
            cx,
            super::super::EditorMode::Session,
        );
    }
    #[gpui::test]
    fn return_menu_refuses_unavailable_defaults_without_an_edit(cx: &mut TestAppContext) {
        use crate::editor::midi_canvas::creation_tests::fixture;
        for loading in [true, false] {
            let (mut visual, editor, _environment, _) = fixture(cx);
            visual.run_until_parked();
            editor.update(&mut visual, |view, _| {
                view.track_defaults.set_test_state(0.5, loading, false)
            });
            let before = editor.read_with(&visual, |view, _| {
                view.state.as_ref().unwrap().project.clone()
            });
            let action = visual.update(|window, cx| {
                editor.update(cx, |view, cx| {
                    view.open_track_context_menu(point(px(600.0), px(100.0)), None, window, cx);
                    view.context_menu
                        .as_ref()
                        .unwrap()
                        .items
                        .iter()
                        .find(|item| item.label == "insert return track")
                        .unwrap()
                        .action
                        .clone()
                })
            });
            visual.update(|window, cx| {
                editor.update(cx, |view, cx| view.select_context_item(action, window, cx))
            });
            editor.read_with(&visual, |view, _| {
                assert!(view.error.as_deref().unwrap().contains(if loading {
                    "loading"
                } else {
                    "open settings"
                }));
                assert_eq!(view.worker_requests_pending, 0);
                assert_eq!(view.state.as_ref().unwrap().project, before);
            });
        }
    }

    #[gpui::test]
    fn return_insertion_refuses_stale_menu_and_capacity_consumed_after_open(
        cx: &mut TestAppContext,
    ) {
        use crate::editor::midi_canvas::creation_tests::{fixture, pump};
        let (mut visual, editor, _environment, _) = fixture(cx);
        // Capture from a real current menu, then change document generation.
        let action = visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                view.open_track_context_menu(point(px(600.0), px(100.0)), None, window, cx);
                view.context_menu
                    .as_ref()
                    .unwrap()
                    .items
                    .iter()
                    .find(|item| item.label == "insert return track")
                    .unwrap()
                    .action
                    .clone()
            })
        });
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::NewWithAudioMode {
                mode: hex_session::DocumentAudioMode::Resident,
                reservation: None,
            }))
        });
        pump(&editor, &mut visual);
        let original = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.clone()
        });
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                view.select_context_item(action, window, cx);
                assert_eq!(view.worker_requests_pending, 0);
                assert_eq!(view.state.as_ref().unwrap().project, original);
            })
        });
        for index in 0..hex_project::MAX_RETURN_TRACKS - 1 {
            editor.update(&mut visual, |view, _| {
                assert!(view.enqueue(Request::Edit(Command::AddReturnTrack {
                    name: format!("return {index}")
                })))
            });
            pump(&editor, &mut visual);
        }
        let action = visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                view.open_track_context_menu(point(px(600.0), px(100.0)), None, window, cx);
                view.context_menu
                    .as_ref()
                    .unwrap()
                    .items
                    .iter()
                    .find(|item| item.label == "insert return track")
                    .unwrap()
                    .action
                    .clone()
            })
        });
        // The last slot is consumed while this menu's generation stays current.
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddReturnTrack {
                name: "last slot".into()
            })))
        });
        pump(&editor, &mut visual);
        let full = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.clone()
        });
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                view.select_context_item(action, window, cx);
                assert_eq!(view.error.as_deref(), Some("return track limit reached"));
                assert_eq!(view.worker_requests_pending, 0);
                assert!(view.context_menu.is_none());
                assert_eq!(view.state.as_ref().unwrap().project, full);
            })
        });
    }
    #[gpui::test]
    fn alias_menu_applies_local_settings_and_refuses_captured_stale_action(
        cx: &mut TestAppContext,
    ) {
        use super::super::drag::group_drag_ui_test::{click, fixture, pump};
        let (_files, handle, editor, originals) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        click(&mut visual, originals[0], false);
        visual.simulate_keystrokes("cmd-d");
        pump(&editor, &mut visual);
        let (track, alias) = editor.read_with(&visual, |view, _| {
            (view.selected_track.unwrap(), view.selected_clip.unwrap())
        });
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::SetClipFades {
                clip: alias,
                fade_in: 120,
                fade_out: 240
            })))
        });
        pump(&editor, &mut visual);
        let placements = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.tracks()[0]
                .clips()
                .iter()
                .map(|c| (c.id(), c.start(), c.duration()))
                .collect::<Vec<_>>()
        });
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                view.open_clip_context_menu(point(px(600.0), px(100.0)), track, alias, window, cx)
            })
        });
        visual.run_until_parked();
        let apply = editor.read_with(&visual, |view, _| {
            let menu = view.context_menu.as_ref().unwrap();
            for label in [
                "duplicate · d",
                "duplicate alias · cmd+d",
                "make unique · shift+i",
                "apply settings to aliases",
            ] {
                assert!(menu.items.iter().any(|item| item.label == label));
            }
            menu.items
                .iter()
                .find(|item| item.label == "apply settings to aliases")
                .unwrap()
                .action
                .clone()
        });
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| view.select_context_item(apply, window, cx))
        });
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            let p = &view.state.as_ref().unwrap().project;
            assert_eq!(
                p.clip(originals[0]).unwrap().retained_fade(),
                p.clip(alias).unwrap().retained_fade()
            );
            assert_eq!(
                p.tracks()[0]
                    .clips()
                    .iter()
                    .map(|c| (c.id(), c.start(), c.duration()))
                    .collect::<Vec<_>>(),
                placements
            );
        });
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                view.open_clip_context_menu(point(px(600.0), px(100.0)), track, alias, window, cx)
            })
        });
        let stale = editor.read_with(&visual, |view, _| {
            view.context_menu
                .as_ref()
                .unwrap()
                .items
                .iter()
                .find(|item| item.label == "make unique · shift+i")
                .unwrap()
                .action
                .clone()
        });
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::RenameTrack {
                track,
                name: "later".into()
            })))
        });
        pump(&editor, &mut visual);
        let before = editor.read_with(&visual, |view, _| {
            (
                view.state.as_ref().unwrap().revision,
                view.state.as_ref().unwrap().project.clone(),
            )
        });
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| view.select_context_item(stale, window, cx))
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.worker_requests_pending, 0);
            assert_eq!(
                (
                    view.state.as_ref().unwrap().revision,
                    view.state.as_ref().unwrap().project.clone()
                ),
                before
            );
        });
    }
}
