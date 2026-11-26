//! Shared Arrangement track header input/output routing pickers, In/Auto/Off
//! monitoring toggle, and track-header drag-to-input assignment.
//!
//! All mutations dispatch canonical `hex_project::Command` edits through the
//! existing Session service boundary (`AssignTrackAudioInput`, `SetMidiSource`,
//! `SetTrackOutput`, `SetTrackInputMonitoring`). This module owns only view
//! popover state and clipped input-picker drop bounds.
use super::{EditorMode, EditorView, device_panel};
use crate::theme;
use gpui::{
    Bounds, Context, Div, ElementId, MouseButton, Pixels, Point, canvas, div, prelude::*, px, rgb,
};
use hex_project::{
    Command, InputMonitoring, Project, ReturnId, RoutingOwner, Track, TrackId,
    TrackOutputDestination,
};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TrackIoMenuKind {
    Input,
    Output,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TrackIoMenu {
    pub(super) track: TrackId,
    pub(super) kind: TrackIoMenuKind,
    pub(super) generation: u64,
}

#[derive(Default)]
pub(super) struct TrackIoUiState {
    pub(super) menu: Option<TrackIoMenu>,
    input_bounds_epoch: Option<(u64, u64)>,
    input_bounds: HashMap<TrackId, Bounds<Pixels>>,
}

/// Evaluate dropping `source` track's header onto `destination` track's input picker.
/// Returns the exact canonical command that menu selection would run, or an explicit
/// refusal reason when self-routed, stale/deleted, incompatible, or cyclic.
pub(super) fn evaluate_track_header_input_drop(
    project: &Project,
    source: TrackId,
    destination: TrackId,
) -> Result<Command, &'static str> {
    if source == destination {
        return Err("track cannot route into itself");
    }
    let Some(src_track) = project.track(source) else {
        return Err("dragged source track no longer exists");
    };
    let Some(dst_track) = project.track(destination) else {
        return Err("destination track no longer exists");
    };
    // Keep audio vs MIDI signal compatibility explicit:
    // - Group tracks carry audio only.
    // - An instrument destination accepts MIDI from non-group MIDI/instrument tracks,
    //   or audio when the dragged track is audio-only / a group.
    // - An audio destination accepts stereo audio input from any non-cyclic track.
    let source_has_midi_signal = !src_track.is_group()
        && (!src_track.midi_clips().is_empty() || src_track.instrument().is_some());
    let source_audio_only =
        src_track.is_group() || (!source_has_midi_signal && !src_track.clips().is_empty());
    if dst_track.instrument().is_some() && !source_audio_only && !src_track.is_group() {
        let eligible = project
            .eligible_midi_input_sources(destination)
            .map_err(|error| error.0)?;
        if !eligible.contains(&source) {
            return Err("routing cycles are unsupported");
        }
        Ok(Command::SetMidiSource {
            destination,
            source: Some(source),
        })
    } else {
        let owner = RoutingOwner::Track(source);
        let eligible = project
            .eligible_audio_input_sources(destination)
            .map_err(|error| error.0)?;
        if !eligible.contains(&owner) {
            return Err("routing cycles are unsupported");
        }
        Ok(Command::AssignTrackAudioInput {
            destination,
            source: Some(owner),
        })
    }
}

fn next_monitoring_mode(current: InputMonitoring) -> InputMonitoring {
    match current {
        InputMonitoring::Auto => InputMonitoring::In,
        InputMonitoring::In => InputMonitoring::Off,
        InputMonitoring::Off => InputMonitoring::Auto,
    }
}

fn monitoring_label(mode: InputMonitoring) -> &'static str {
    match mode {
        InputMonitoring::In => "in",
        InputMonitoring::Auto => "auto",
        InputMonitoring::Off => "off",
    }
}

fn track_short_label(project: &Project, id: TrackId) -> String {
    project
        .tracks()
        .iter()
        .enumerate()
        .find(|(_, t)| t.id() == id)
        .map_or_else(
            || "missing".to_owned(),
            |(index, t)| {
                format!(
                    "{} {}",
                    index + 1,
                    hex_session::service::text::verbatim(t.name())
                )
            },
        )
}

fn return_short_label(project: &Project, id: ReturnId) -> String {
    project
        .return_tracks()
        .iter()
        .enumerate()
        .find(|(_, b)| b.id() == id)
        .map_or_else(
            || "missing".to_owned(),
            |(index, b)| {
                let letter = char::from(b'a' + (index.min(25) as u8));
                format!(
                    "{letter} {}",
                    hex_session::service::text::verbatim(b.name())
                )
            },
        )
}

fn input_picker_label(project: &Project, track: &Track) -> String {
    let id = track.id();
    let midi_source = project
        .midi_routes()
        .iter()
        .find(|route| route.destination() == id)
        .map(|route| route.source());
    let audio_sources: Vec<RoutingOwner> = project
        .audio_feeds()
        .iter()
        .filter(|feed| feed.destination() == id)
        .map(|feed| feed.source())
        .chain(
            project
                .tracks()
                .iter()
                .filter(|src| src.output_destination() == TrackOutputDestination::Track(id))
                .map(|src| RoutingOwner::Track(src.id())),
        )
        .collect();
    if track.instrument().is_some() {
        if let Some(src) = midi_source {
            return format!("m:{}", track_short_label(project, src));
        }
        if audio_sources.is_empty() {
            return "local".to_owned();
        }
    }
    match audio_sources.as_slice() {
        [] => "ext".to_owned(),
        [RoutingOwner::Track(src)] => track_short_label(project, *src),
        [RoutingOwner::Return(bus)] => return_short_label(project, *bus),
        [RoutingOwner::Main] => "main".to_owned(),
        _ => format!("{} in", audio_sources.len()),
    }
}

fn output_picker_label(project: &Project, track: &Track) -> String {
    match track.output_destination() {
        TrackOutputDestination::Default => {
            if let Some(parent) = track.parent_group() {
                format!("grp:{}", track_short_label(project, parent))
            } else {
                "main".to_owned()
            }
        }
        TrackOutputDestination::Main => "main".to_owned(),
        TrackOutputDestination::SendsOnly => "sends".to_owned(),
        TrackOutputDestination::Track(target) => track_short_label(project, target),
        TrackOutputDestination::Return(bus) => return_short_label(project, bus),
    }
}

impl EditorView {
    pub(super) fn track_io_drag_hovering_input(&self) -> bool {
        let Some((source, pointer)) = self.sidechain_drag() else {
            return false;
        };
        let Some(epoch) = self
            .state
            .as_ref()
            .map(|state| (self.project_generation, state.revision))
        else {
            return false;
        };
        if self.track_io.input_bounds_epoch != Some(epoch) {
            return false;
        }
        self.track_io
            .input_bounds
            .iter()
            .any(|(&track_id, bounds)| track_id != source && bounds.contains(&pointer))
    }

    /// Compact I/O routing pickers (`in` / `out`) and `in`/`auto`/`off` monitor
    /// toggle placed inside the track header's second line when `header_height >= TRACK_ROW_HEIGHT`.
    pub(super) fn build_track_io_row(
        &mut self,
        project: &Project,
        revision: u64,
        track: &Track,
        cx: &mut Context<Self>,
    ) -> Div {
        let track_id = track.id();
        let generation = self.project_generation;
        let epoch = (generation, revision);
        if self.track_io.input_bounds_epoch != Some(epoch) {
            self.track_io.input_bounds.clear();
            self.track_io.input_bounds_epoch = Some(epoch);
        }
        let drag = self.sidechain_drag();
        let drag_verdict = drag
            .map(|(source, _)| evaluate_track_header_input_drop(project, source, track_id).is_ok());
        let input_open = self.track_io.menu
            == Some(TrackIoMenu {
                track: track_id,
                kind: TrackIoMenuKind::Input,
                generation,
            });
        let output_open = self.track_io.menu
            == Some(TrackIoMenu {
                track: track_id,
                kind: TrackIoMenuKind::Output,
                generation,
            });
        let input_label = input_picker_label(project, track);
        let output_label = output_picker_label(project, track);
        let entity = cx.entity();
        let valid_outline = rgb(theme::color(theme::PaletteRole::Primary));
        let invalid_outline = rgb(theme::color(theme::PaletteRole::Error));
        let border_color = rgb(theme::control_style().border);
        let surface = rgb(theme::color(theme::PaletteRole::Surface));
        let active_bg = rgb(theme::color(theme::PaletteRole::Raised));
        let text_color = rgb(theme::color(theme::PaletteRole::Text));

        let input_picker = div()
            .id(("track-input-picker", track_id.value()))
            .debug_selector(move || format!("track-input-picker-{}", track_id.value()))
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(14.0))
            .px(px(2.0))
            .flex()
            .items_center()
            .overflow_hidden()
            .text_size(px(8.5))
            .bg(if input_open { active_bg } else { surface })
            .text_color(text_color)
            .border_1()
            .border_color(border_color)
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |view, _, _, cx| {
                cx.stop_propagation();
                let next = TrackIoMenu {
                    track: track_id,
                    kind: TrackIoMenuKind::Input,
                    generation,
                };
                view.session_grid.send_menu_track = None;
                view.track_io.menu = if view.track_io.menu == Some(next) {
                    None
                } else {
                    Some(next)
                };
                cx.notify();
            }))
            .tooltip(move |_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: "input routing · click or drop track header".into(),
                })
                .into()
            })
            .child(div().truncate().child(theme::text(input_label)))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let mask = window.content_mask().bounds;
                        let clipped = bounds.intersects(&mask).then(|| bounds.intersect(&mask));
                        entity.update(cx, |view, _| {
                            if view.track_io.input_bounds_epoch == Some(epoch) {
                                if let Some(rect) = clipped {
                                    view.track_io.input_bounds.insert(track_id, rect);
                                } else {
                                    view.track_io.input_bounds.remove(&track_id);
                                }
                            }
                        });
                        drag.zip(drag_verdict)
                            .filter(|((_, point), _)| {
                                clipped.is_some_and(|visible| visible.contains(point))
                            })
                            .map(|(_, valid)| valid)
                    },
                    move |bounds, hovered_valid, window, _| {
                        if let Some(valid) = hovered_valid {
                            window.paint_quad(gpui::outline(
                                bounds,
                                if valid {
                                    valid_outline
                                } else {
                                    invalid_outline
                                },
                                gpui::BorderStyle::Solid,
                            ));
                        }
                    },
                )
                .absolute()
                .inset_0(),
            );

        let output_picker = div()
            .id(("track-output-picker", track_id.value()))
            .debug_selector(move || format!("track-output-picker-{}", track_id.value()))
            .flex_1()
            .min_w_0()
            .h(px(14.0))
            .px(px(2.0))
            .flex()
            .items_center()
            .overflow_hidden()
            .text_size(px(8.5))
            .bg(if output_open { active_bg } else { surface })
            .text_color(text_color)
            .border_1()
            .border_color(border_color)
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |view, _, _, cx| {
                cx.stop_propagation();
                let next = TrackIoMenu {
                    track: track_id,
                    kind: TrackIoMenuKind::Output,
                    generation,
                };
                view.session_grid.send_menu_track = None;
                view.track_io.menu = if view.track_io.menu == Some(next) {
                    None
                } else {
                    Some(next)
                };
                cx.notify();
            }))
            .tooltip(move |_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: "output routing · click to choose destination".into(),
                })
                .into()
            })
            .child(div().truncate().child(theme::text(output_label)));

        let monitor_button = (!track.is_group()).then(|| {
            let mode = track.input_monitoring();
            let (bg, fg) = match mode {
                InputMonitoring::In => (
                    theme::color(theme::PaletteRole::Primary),
                    theme::color(theme::PaletteRole::Background),
                ),
                InputMonitoring::Auto => (
                    theme::color(theme::PaletteRole::Surface),
                    theme::color(theme::PaletteRole::Text),
                ),
                InputMonitoring::Off => (
                    theme::color(theme::PaletteRole::Background),
                    theme::color(theme::PaletteRole::Muted),
                ),
            };
            let next_mode = next_monitoring_mode(mode);
            div()
                .id(("track-monitor", track_id.value()))
                .debug_selector(move || format!("track-monitor-{}", track_id.value()))
                .w(px(26.0))
                .h(px(14.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(8.5))
                .bg(rgb(bg))
                .text_color(rgb(fg))
                .border_1()
                .border_color(border_color)
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |view, _, _, cx| {
                    cx.stop_propagation();
                    if view.project_generation != generation {
                        return;
                    }
                    view.set_track_input_monitoring(track_id, next_mode, cx);
                }))
                .tooltip(move |_, cx| {
                    cx.new(|_| super::transport::MiniTooltip {
                        kind: crate::tooltips::Kind::Identification,
                        description: match mode {
                            InputMonitoring::In => {
                                "input monitor: in · passes input, suppresses clips · click for off"
                            }
                            InputMonitoring::Auto => {
                                "input monitor: auto · passes when armed, inhibits on clips · click for in"
                            }
                            InputMonitoring::Off => {
                                "input monitor: off · software monitoring disabled · click for auto"
                            }
                        }
                        .into(),
                    })
                    .into()
                })
                .child(theme::text(monitoring_label(mode)))
        });

        div()
            .w_full()
            .h(px(16.0))
            .flex()
            .items_center()
            .gap(px(2.0))
            .min_w_0()
            .children([input_picker, output_picker])
            .children(monitor_button)
    }

    pub(super) fn set_track_input_monitoring(
        &mut self,
        track: TrackId,
        mode: InputMonitoring,
        cx: &mut Context<Self>,
    ) {
        if mode == InputMonitoring::Off {
            // Release any live held notes cleanly when disabling monitoring.
            self.computer_keyboard.release_all();
        }
        device_panel::enqueue_return_edit(
            self,
            Command::SetTrackInputMonitoring { track, mode },
            cx,
        );
        cx.notify();
    }

    pub(super) fn track_io_menu_overlay(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<Div>> {
        if self.mode != EditorMode::Arrangement {
            self.track_io.menu = None;
            return None;
        }
        let menu = self.track_io.menu?;
        if menu.generation != self.project_generation {
            self.track_io.menu = None;
            return None;
        }
        let state = self.state.as_ref().cloned()?;
        let Some(track) = state.project.track(menu.track).cloned() else {
            self.track_io.menu = None;
            return None;
        };
        let track_id = track.id();
        let lane = self.timeline_caches.drag_lanes.get(&track_id).copied();
        let window_bounds = window.bounds();
        let window_width = f32::from(window_bounds.size.width);
        let window_height = f32::from(window_bounds.size.height);
        let menu_width = 210.0_f32;
        let menu_height = 176.0_f32;
        let (left, top) = if let Some(lane) = lane {
            let left = (lane.left + lane.width + 4.0)
                .clamp(4.0, (window_width - menu_width - 4.0).max(4.0));
            let below = lane.top + self.track_height(track_id) + 1.0;
            let top = if below + menu_height <= window_height {
                below
            } else {
                (lane.top - menu_height - 1.0).max(4.0)
            };
            (left, top)
        } else {
            (
                (window_width - menu_width - 12.0).max(4.0),
                48.0_f32.min((window_height - menu_height).max(4.0)),
            )
        };

        let mut list = div()
            .id("track-io-menu")
            .debug_selector(|| "track-io-menu".into())
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(menu_width))
            .max_h(px(menu_height))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .p_1()
            .gap(px(2.0))
            .text_xs()
            .bg(rgb(theme::color(theme::PaletteRole::Raised)))
            .border_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

        match menu.kind {
            TrackIoMenuKind::Input => {
                if track.instrument().is_some() {
                    list = list.child(
                        div()
                            .px_1()
                            .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                            .child(theme::text("midi input")),
                    );
                    let current_midi = state
                        .project
                        .midi_routes()
                        .iter()
                        .find(|r| r.destination() == track_id)
                        .map(|r| r.source());
                    let eligible_midi = state
                        .project
                        .eligible_midi_input_sources(track_id)
                        .unwrap_or_default();
                    list = list.children([io_menu_item(
                        "track-io-midi-local".into(),
                        format!(
                            "{} local midi",
                            if current_midi.is_none() { "●" } else { "○" }
                        ),
                        true,
                        Some(Command::SetMidiSource {
                            destination: track_id,
                            source: None,
                        }),
                        cx,
                    )]);
                    for candidate in state
                        .project
                        .tracks()
                        .iter()
                        .filter(|t| t.id() != track_id && !t.is_group())
                    {
                        let cid = candidate.id();
                        let allowed = eligible_midi.contains(&cid);
                        let label = format!(
                            "{} {}{}",
                            if current_midi == Some(cid) {
                                "●"
                            } else {
                                "○"
                            },
                            track_short_label(&state.project, cid),
                            if allowed { "" } else { " · cycle" }
                        );
                        list = list.children([io_menu_item(
                            format!("track-io-midi-track-{}", cid.value()),
                            label,
                            allowed,
                            allowed.then_some(Command::SetMidiSource {
                                destination: track_id,
                                source: Some(cid),
                            }),
                            cx,
                        )]);
                    }
                }
                list = list.child(
                    div()
                        .px_1()
                        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                        .child(theme::text("audio input")),
                );
                let eligible_audio = state
                    .project
                    .eligible_audio_input_sources(track_id)
                    .unwrap_or_default();
                let active_audio: Vec<RoutingOwner> = state
                    .project
                    .audio_feeds()
                    .iter()
                    .filter(|f| f.destination() == track_id)
                    .map(|f| f.source())
                    .collect();
                list = list.children([io_menu_item(
                    "track-io-audio-none".into(),
                    format!(
                        "{} ext. in",
                        if active_audio.is_empty() {
                            "●"
                        } else {
                            "○"
                        }
                    ),
                    true,
                    Some(Command::AssignTrackAudioInput {
                        destination: track_id,
                        source: None,
                    }),
                    cx,
                )]);
                for candidate in state.project.tracks().iter().filter(|t| t.id() != track_id) {
                    let owner = RoutingOwner::Track(candidate.id());
                    let allowed = eligible_audio.contains(&owner);
                    let active = active_audio.contains(&owner)
                        || candidate.output_destination()
                            == TrackOutputDestination::Track(track_id);
                    let label = format!(
                        "{} {}{}",
                        if active { "●" } else { "○" },
                        track_short_label(&state.project, candidate.id()),
                        if allowed { "" } else { " · cycle" }
                    );
                    list = list.children([io_menu_item(
                        format!("track-io-audio-track-{}", candidate.id().value()),
                        label,
                        allowed,
                        allowed.then_some(Command::AssignTrackAudioInput {
                            destination: track_id,
                            source: Some(owner),
                        }),
                        cx,
                    )]);
                }
                for bus in state.project.return_tracks() {
                    let owner = RoutingOwner::Return(bus.id());
                    let allowed = eligible_audio.contains(&owner);
                    let active = active_audio.contains(&owner);
                    let label = format!(
                        "{} {}{}",
                        if active { "●" } else { "○" },
                        return_short_label(&state.project, bus.id()),
                        if allowed { "" } else { " · cycle" }
                    );
                    list = list.children([io_menu_item(
                        format!("track-io-audio-return-{}", bus.id().value()),
                        label,
                        allowed,
                        allowed.then_some(Command::AssignTrackAudioInput {
                            destination: track_id,
                            source: Some(owner),
                        }),
                        cx,
                    )]);
                }
            }
            TrackIoMenuKind::Output => {
                list = list.child(
                    div()
                        .px_1()
                        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                        .child(theme::text("audio output")),
                );
                let current = track.output_destination();
                let eligible = state
                    .project
                    .eligible_output_destinations(track_id)
                    .unwrap_or_default();
                let default_label = if let Some(parent) = track.parent_group() {
                    format!("default ({})", track_short_label(&state.project, parent))
                } else {
                    "default (main)".to_owned()
                };
                for (selector, label, dest) in [
                    (
                        "track-io-out-default".to_owned(),
                        default_label,
                        TrackOutputDestination::Default,
                    ),
                    (
                        "track-io-out-main".to_owned(),
                        "main".to_owned(),
                        TrackOutputDestination::Main,
                    ),
                    (
                        "track-io-out-sends-only".to_owned(),
                        "sends only".to_owned(),
                        TrackOutputDestination::SendsOnly,
                    ),
                ] {
                    let allowed = eligible.contains(&dest);
                    list = list.children([io_menu_item(
                        selector,
                        format!("{} {label}", if current == dest { "●" } else { "○" }),
                        allowed,
                        allowed.then_some(Command::SetTrackOutput {
                            track: track_id,
                            destination: dest,
                        }),
                        cx,
                    )]);
                }
                for candidate in state.project.tracks().iter().filter(|t| t.id() != track_id) {
                    let dest = TrackOutputDestination::Track(candidate.id());
                    let allowed = eligible.contains(&dest);
                    let label = format!(
                        "{} {}{}",
                        if current == dest { "●" } else { "○" },
                        track_short_label(&state.project, candidate.id()),
                        if allowed { "" } else { " · cycle" }
                    );
                    list = list.children([io_menu_item(
                        format!("track-io-out-track-{}", candidate.id().value()),
                        label,
                        allowed,
                        allowed.then_some(Command::SetTrackOutput {
                            track: track_id,
                            destination: dest,
                        }),
                        cx,
                    )]);
                }
                for bus in state.project.return_tracks() {
                    let dest = TrackOutputDestination::Return(bus.id());
                    let allowed = eligible.contains(&dest);
                    let label = format!(
                        "{} {}{}",
                        if current == dest { "●" } else { "○" },
                        return_short_label(&state.project, bus.id()),
                        if allowed { "" } else { " · cycle" }
                    );
                    list = list.children([io_menu_item(
                        format!("track-io-out-return-{}", bus.id().value()),
                        label,
                        allowed,
                        allowed.then_some(Command::SetTrackOutput {
                            track: track_id,
                            destination: dest,
                        }),
                        cx,
                    )]);
                }
            }
        }
        Some(list)
    }
}

fn io_menu_item(
    key: String,
    label: String,
    enabled: bool,
    command: Option<Command>,
    cx: &mut Context<EditorView>,
) -> gpui::Stateful<Div> {
    let mut item = div()
        .id(ElementId::Name(key.clone().into()))
        .debug_selector(move || key.clone())
        .h(px(18.0))
        .px_1()
        .flex()
        .items_center()
        .truncate()
        .text_color(rgb(theme::color(if enabled {
            theme::PaletteRole::Text
        } else {
            theme::PaletteRole::Muted
        })));
    if enabled && let Some(command) = command {
        item = item
            .cursor_pointer()
            .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Hover))))
            .on_click(cx.listener(move |view, _, _, cx| {
                cx.stop_propagation();
                view.track_io.menu = None;
                device_panel::enqueue_return_edit(view, command.clone(), cx);
                cx.notify();
            }));
    } else {
        item = item.opacity(0.55);
    }
    item.child(theme::text(label))
}

/// Handle dropping a dragged track header onto another track's input picker.
/// Assigns input ONLY via the same canonical command as menu selection; never
/// reorders, reparents, duplicates, or mutates on invalid/stale/cyclic targets.
pub(super) fn drop_track_header_on_input(
    view: &mut EditorView,
    source: TrackId,
    drag_generation: u64,
    position: Point<Pixels>,
    cx: &mut Context<EditorView>,
) -> bool {
    if view.mode != EditorMode::Arrangement || drag_generation != view.project_generation {
        return false;
    }
    let Some(state) = view.state.as_ref() else {
        return false;
    };
    let epoch = (view.project_generation, state.revision);
    if view.track_io.input_bounds_epoch != Some(epoch) {
        return false;
    }
    let Some(destination) = view
        .track_io
        .input_bounds
        .iter()
        .find_map(|(&track_id, bounds)| bounds.contains(&position).then_some(track_id))
    else {
        return false;
    };
    match evaluate_track_header_input_drop(&state.project, source, destination) {
        Ok(command) => {
            view.track_io.menu = None;
            device_panel::enqueue_return_edit(view, command, cx);
            cx.notify();
        }
        Err(reason) => {
            view.error = Some(reason.into());
            cx.notify();
        }
    }
    true
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, size};
    use hex_project::{Created, Editor};
    use std::sync::Arc;

    #[gpui::test]
    fn track_header_io_pickers_and_monitor_toggle_dispatch_canonical_commands(
        cx: &mut TestAppContext,
    ) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(src) = editor
            .execute(Command::AddTrack { name: "src".into() })
            .unwrap()
        else {
            panic!("src");
        };
        let Created::Track(bus) = editor
            .execute(Command::AddTrack { name: "bus".into() })
            .unwrap()
        else {
            panic!("bus");
        };
        let (handle, entity) = super::super::clip_drag_ui_test::editor_window(
            cx,
            editor.project().clone(),
            Arc::default(),
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(1100.0), px(700.0)));
        visual.run_until_parked();

        // 1. Output picker on `src` -> select `bus` via canonical `SetTrackOutput`.
        let out_bounds = visual
            .debug_bounds(Box::leak(
                format!("track-output-picker-{}", src.value()).into_boxed_str(),
            ))
            .expect("output picker bounds");
        visual.simulate_mouse_down(out_bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(out_bounds.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();

        let item_bounds = visual
            .debug_bounds(Box::leak(
                format!("track-io-out-track-{}", bus.value()).into_boxed_str(),
            ))
            .expect("output menu item bounds");
        visual.simulate_mouse_down(
            item_bounds.center(),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.simulate_mouse_up(
            item_bounds.center(),
            MouseButton::Left,
            Modifiers::default(),
        );
        entity.update(&mut visual, |view, _| {
            assert!(matches!(
                view.test_probes.last_enqueued_edit,
                Some(Command::SetTrackOutput {
                    track,
                    destination: TrackOutputDestination::Track(target),
                }) if track == src && target == bus
            ));
        });

        // 2. Monitor button cycles Auto -> In via canonical `SetTrackInputMonitoring`.
        let mon_bounds = visual
            .debug_bounds(Box::leak(
                format!("track-monitor-{}", bus.value()).into_boxed_str(),
            ))
            .expect("monitor button bounds");
        visual.simulate_mouse_down(mon_bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(mon_bounds.center(), MouseButton::Left, Modifiers::default());
        entity.update(&mut visual, |view, _| {
            assert!(matches!(
                view.test_probes.last_enqueued_edit,
                Some(Command::SetTrackInputMonitoring {
                    track,
                    mode: InputMonitoring::In,
                }) if track == bus
            ));
        });
    }

    #[gpui::test]
    fn dragging_track_header_onto_input_picker_assigns_input_without_reordering(
        cx: &mut TestAppContext,
    ) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(audio_src) = editor
            .execute(Command::AddTrack {
                name: "drums".into(),
            })
            .unwrap()
        else {
            panic!("drums");
        };
        let Created::Track(audio_bus) = editor
            .execute(Command::AddTrack {
                name: "drum bus".into(),
            })
            .unwrap()
        else {
            panic!("drum bus");
        };
        let Created::Track(synth) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "synth".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("synth");
        };
        let (handle, entity) = super::super::clip_drag_ui_test::editor_window(
            cx,
            editor.project().clone(),
            Arc::default(),
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(1100.0), px(700.0)));
        visual.run_until_parked();

        // Drag `drums` header onto `drum bus` input picker -> AssignTrackAudioInput (not reorder!).
        let src_name = visual
            .debug_bounds(Box::leak(
                format!("track-name-{}", audio_src.value()).into_boxed_str(),
            ))
            .expect("source name bounds");
        let dst_input = visual
            .debug_bounds(Box::leak(
                format!("track-input-picker-{}", audio_bus.value()).into_boxed_str(),
            ))
            .expect("destination input bounds");
        visual.simulate_mouse_down(src_name.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(dst_input.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_mouse_up(dst_input.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        entity.update(&mut visual, |view, _| {
            assert!(matches!(
                view.test_probes.last_enqueued_edit,
                Some(Command::AssignTrackAudioInput {
                    destination,
                    source: Some(RoutingOwner::Track(source)),
                }) if destination == audio_bus && source == audio_src
            ));
            assert!(!view.track_reorder_active());
        });

        // MIDI vs audio drop compatibility and cycle/self/stale refusal checks.
        assert!(matches!(
            evaluate_track_header_input_drop(editor.project(), audio_src, synth),
            Ok(Command::SetMidiSource { destination, source: Some(source) })
                if destination == synth && source == audio_src
        ));
        assert!(evaluate_track_header_input_drop(editor.project(), audio_src, audio_src).is_err());

        // Cyclic drop is refused cleanly with zero mutation.
        editor
            .execute(Command::SetTrackOutput {
                track: audio_bus,
                destination: TrackOutputDestination::Track(audio_src),
            })
            .unwrap();
        assert!(evaluate_track_header_input_drop(editor.project(), audio_src, audio_bus).is_err());
    }

    #[gpui::test]
    fn track_io_menu_blocks_selection_delete_and_yields_playback_overlay(cx: &mut TestAppContext) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "lead".into(),
            })
            .unwrap()
        else {
            panic!("lead");
        };
        let (handle, entity) = super::super::clip_drag_ui_test::editor_window(
            cx,
            editor.project().clone(),
            Arc::default(),
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(1100.0), px(700.0)));
        visual.run_until_parked();

        // Select the track and confirm baseline overlay state is clear.
        entity.update(&mut visual, |view, cx| {
            view.selected_track = Some(track);
            assert_eq!(view.active_overlay_kind(), None);
            assert!(!view.has_blocking_overlay());
            assert!(!view.has_foreground_overlay());
            assert!(!view.selection_delete_blocked());
            cx.notify();
        });
        visual.run_until_parked();

        // Open the Track I/O output picker menu.
        let out_bounds = visual
            .debug_bounds(Box::leak(
                format!("track-output-picker-{}", track.value()).into_boxed_str(),
            ))
            .expect("output picker bounds");
        visual.simulate_mouse_down(out_bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(out_bounds.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();

        entity.update(&mut visual, |view, cx| {
            assert_eq!(
                view.active_overlay_kind(),
                Some(super::super::ActiveOverlayKind::TrackIoMenu)
            );
            assert!(view.has_blocking_overlay());
            assert!(view.has_foreground_overlay());
            assert!(!view.uses_sibling_playback_overlay());
            assert!(view.selection_delete_blocked());
            assert!(!view.track_view_change_available(cx));
        });

        // Pressing Backspace/Delete while Track I/O menu is open must NOT delete the track.
        visual.simulate_keystrokes("backspace");
        visual.run_until_parked();
        entity.read_with(&visual, |view, _| {
            assert!(view.track_io.menu.is_some());
            assert!(
                view.test_probes.last_enqueued_edit.is_none(),
                "backspace must not enqueue delete while track I/O menu is open: {:?}",
                view.test_probes.last_enqueued_edit
            );
            assert_eq!(view.state.as_ref().unwrap().project.tracks().len(), 1);
        });
    }
}
