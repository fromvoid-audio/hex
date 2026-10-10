//! Compact descriptor-driven native device chain in the shared detail pane.
use super::EditorView;
use crate::theme;
use gpui::{
    App, Context, Div, ElementId, Entity, Stateful, Subscription, div, prelude::*, px, rgb,
};
use hex_engine::{NativeDeviceParameterView, NativeDeviceView};
use hex_lattice_contract::{NativeDeviceState, ParameterDescriptor, ParameterKind, ParameterValue};
use hex_project::{Command, DeviceId, TrackId};
use hex_session::device_gesture::GestureTarget;
use hex_session::service::{Request, ViewState};
use hex_widgets::{
    ControlStyle, NumericControl, NumericControlConfig, NumericControlEvent, NumericControlKind,
    NumericEventPhase, NumericScale, NumericSpec, Selector, SelectorChanged, SelectorConfig,
    Toggle, ToggleChanged, ToggleConfig, device_frame,
};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ParameterKey {
    device: DeviceId,
    parameter: hex_lattice_contract::ParameterId,
}

enum ParameterControl {
    Float {
        entity: Entity<NumericControl>,
        _subscription: Subscription,
        type_id: hex_lattice_contract::DeviceTypeId,
        descriptor: &'static ParameterDescriptor,
    },
    Bool {
        entity: Entity<Toggle>,
        _subscription: Subscription,
        type_id: hex_lattice_contract::DeviceTypeId,
        descriptor: &'static ParameterDescriptor,
    },
    Choice {
        entity: Entity<Selector>,
        _subscription: Subscription,
        type_id: hex_lattice_contract::DeviceTypeId,
        descriptor: &'static ParameterDescriptor,
    },
}

#[derive(Default)]
pub(super) struct DevicePanelState {
    track: Option<TrackId>,
    generation: u64,
    controls: HashMap<ParameterKey, ParameterControl>,
    views: HashMap<DeviceId, (NativeDeviceState, Result<NativeDeviceView, String>)>,
    catalog: Option<Result<Vec<hex_engine::NativeDeviceCatalogEntry>, String>>,
    pending_values: HashMap<ParameterKey, ParameterValue>,
    add_menu_open: bool,
    message: Option<String>,
}

impl DevicePanelState {
    pub(super) fn has_pending_draft(&self, cx: &App) -> bool {
        self.controls.values().any(|control| match control {
            ParameterControl::Float { entity, .. } => entity.read_with(cx, |control, _| {
                control.is_interacting() || control.is_editing()
            }),
            ParameterControl::Bool { .. } | ParameterControl::Choice { .. } => false,
        })
    }
}

pub(super) fn control_style() -> ControlStyle {
    ControlStyle {
        background: theme::color(theme::PaletteRole::Background),
        surface: theme::color(theme::PaletteRole::Surface),
        raised: theme::color(theme::PaletteRole::Raised),
        rail: theme::color(theme::PaletteRole::Border),
        border: theme::color(theme::PaletteRole::Border),
        text: theme::readable_text(
            theme::color(theme::PaletteRole::Surface),
            theme::color(theme::PaletteRole::Text),
        ),
        muted: theme::color(theme::PaletteRole::Muted),
        accent: theme::color(theme::PaletteRole::Secondary),
        hover: theme::color(theme::PaletteRole::Hover),
        font_size: 9.0,
    }
}

/// Commit every live numeric draft before a save, panel transition, or destructive edit.
/// A bad typed value stays in its field and blocks the caller.
pub(super) fn finalize_controls(view: &mut EditorView, cx: &mut Context<EditorView>) -> bool {
    let mut first_error = None;
    let active = view
        .device_gesture
        .snapshot()
        .map(|snapshot| snapshot.target);
    let mut keys: Vec<_> = view.device_panel.controls.keys().copied().collect();
    keys.sort_by_key(|key| {
        u8::from(
            !active.is_some_and(|target| {
                target.device == key.device && target.parameter == key.parameter
            }),
        )
    });
    for key in keys {
        let Some(ParameterControl::Float { entity, .. }) = view.device_panel.controls.get(&key)
        else {
            continue;
        };
        let entity = entity.clone();
        let finalized =
            match entity.update(cx, |control, cx| control.prepare_finish_edit_for_owner(cx)) {
                Ok(value) => value,
                Err(error) => {
                    first_error.get_or_insert_with(|| error.to_string());
                    continue;
                }
            };
        if let Some(finalized) = finalized {
            let active_gesture_matches = view.device_gesture.snapshot().is_some_and(|snapshot| {
                snapshot.target.device == key.device
                    && snapshot.target.parameter == key.parameter
                    && !view.device_gesture.has_pending_terminal()
            });
            if finalized.previous != finalized.value || active_gesture_matches {
                if let Err(error) = commit_owned_numeric_edit(view, key, &finalized) {
                    first_error.get_or_insert_with(|| error.to_owned());
                    continue;
                }
                view.device_panel
                    .pending_values
                    .insert(key, ParameterValue::Float(finalized.value as f32));
            }
            entity.update(cx, |control, cx| {
                control.accept_finish_edit_for_owner(finalized, cx)
            });
        }
    }
    if let Some(error) = first_error {
        view.error = Some(format!("finish the device value edit: {error}").to_lowercase());
        false
    } else {
        true
    }
}

fn commit_owned_numeric_edit(
    view: &mut EditorView,
    key: ParameterKey,
    finalized: &hex_widgets::NumericControlFinalValue,
) -> Result<(), &'static str> {
    let target = GestureTarget {
        device: key.device,
        parameter: key.parameter,
    };
    let value = ParameterValue::Float(finalized.value as f32);
    let active = view.device_gesture.snapshot();
    if active.is_some_and(|snapshot| snapshot.target != target)
        || view.device_gesture.has_pending_terminal()
    {
        if view.enqueue_device_barrier(Request::SetDeviceParameter {
            device: target.device,
            parameter: target.parameter,
            value,
        }) {
            return Ok(());
        }
        return Err("device edit queue is full");
    }
    if active.is_some() {
        view.change_device_gesture(value)?;
        view.finish_device_gesture(value)?;
        return Ok(());
    }
    view.begin_device_gesture(target, ParameterValue::Float(finalized.previous as f32))?;
    if finalized.value != finalized.previous {
        view.change_device_gesture(value)?;
    }
    view.finish_device_gesture(value)
}

pub(super) fn view(view: &mut EditorView, cx: &mut Context<EditorView>) -> Stateful<Div> {
    if view.device_panel.generation != view.project_generation {
        // Project replacement retires the worker gesture. Drop old subscriptions
        // before stale controls can send events into the replacement project.
        cancel_all_controls(view, cx);
        view.device_panel.controls.clear();
        view.device_panel.views.clear();
        view.device_panel.pending_values.clear();
        view.device_panel.track = None;
        view.device_panel.generation = view.project_generation;
    }
    let Some(state) = view.state.as_ref().cloned() else {
        return empty_panel("project is still loading");
    };
    let Some(selected_track) = view.selected_track else {
        if let Some(previous_track) = view.device_panel.track {
            if state.project.track(previous_track).is_none() {
                cancel_all_controls(view, cx);
                view.device_panel.message = Some("device target is no longer available".into());
            } else if !finalize_controls(view, cx) {
                view.device_panel.message = Some("finish the active device value first".into());
                return render_track(view, cx, &state, previous_track);
            } else if view.device_gesture.snapshot().is_some() {
                view.device_panel.message = Some("finishing device edit".into());
                return render_track(view, cx, &state, previous_track);
            }
            view.device_panel.controls.clear();
            view.device_panel.views.clear();
            view.device_panel.pending_values.clear();
            view.device_panel.track = None;
        }
        return empty_panel("select a track to view devices");
    };

    if view.device_panel.track != Some(selected_track) {
        if let Some(previous_track) = view.device_panel.track
            && state.project.track(previous_track).is_none()
        {
            cancel_all_controls(view, cx);
            view.device_panel.message = Some("device target is no longer available".into());
            view.device_panel.track = Some(selected_track);
        } else if !finalize_controls(view, cx) {
            view.device_panel.message = Some("finish the active device value first".into());
            if let Some(previous_track) = view.device_panel.track
                && state.project.track(previous_track).is_some()
            {
                return render_track(view, cx, &state, previous_track);
            }
        } else {
            if view.device_gesture.snapshot().is_some()
                && let Some(previous_track) = view.device_panel.track
                && state.project.track(previous_track).is_some()
            {
                view.device_panel.message = Some("finishing device edit".into());
                return render_track(view, cx, &state, previous_track);
            }
            view.device_panel.controls.clear();
            view.device_panel.views.clear();
            view.device_panel.pending_values.clear();
            view.device_panel.track = Some(selected_track);
            view.device_panel.message = None;
        }
    }
    let track = view.device_panel.track.unwrap_or(selected_track);
    render_track(view, cx, &state, track)
}

fn cancel_all_controls(view: &mut EditorView, cx: &mut Context<EditorView>) {
    let keys: Vec<_> = view.device_panel.controls.keys().copied().collect();
    for key in keys {
        if let Some(mut control) = view.device_panel.controls.remove(&key) {
            cancel_control(view, cx, key, &mut control);
        }
    }
    view.device_panel.views.clear();
    view.device_panel.pending_values.clear();
}

fn render_track(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    state: &ViewState,
    track_id: TrackId,
) -> Stateful<Div> {
    let Some(track) = state.project.track(track_id) else {
        return empty_panel("selected track is no longer available");
    };
    let devices = track.devices().to_vec();
    let alive: HashSet<_> = devices.iter().map(|device| device.id()).collect();
    let stale_keys: Vec<_> = view
        .device_panel
        .controls
        .keys()
        .filter(|key| !alive.contains(&key.device))
        .copied()
        .collect();
    if !stale_keys.is_empty() {
        for key in stale_keys {
            if let Some(mut control) = view.device_panel.controls.remove(&key) {
                cancel_control(view, cx, key, &mut control);
            }
        }
        view.device_panel
            .controls
            .retain(|key, _| alive.contains(&key.device));
        view.device_panel.views.retain(|id, _| alive.contains(id));
        view.device_panel
            .pending_values
            .retain(|key, _| alive.contains(&key.device));
        view.device_panel.message = Some("device target is no longer available".into());
    }

    if view.device_panel.catalog.is_none() {
        view.device_panel.catalog = Some(
            hex_engine::native_device_catalog().map_err(|error| error.to_string().to_lowercase()),
        );
    }
    let catalog = view
        .device_panel
        .catalog
        .clone()
        .unwrap_or_else(|| Ok(Vec::new()));

    let mut root = div()
        .id("device-panel")
        .debug_selector(|| "device-panel".into())
        .h(px(188.0))
        .w_full()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .bg(rgb(theme::color(theme::PaletteRole::Surface)))
        .border_t_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)));

    let mut chain = div()
        .id("device-chain-scroll")
        .debug_selector(|| "device-chain-scroll".into())
        .flex_1()
        .min_h_0()
        .flex()
        .gap(px(3.0))
        .p(px(3.0))
        .overflow_x_scroll()
        .overflow_y_hidden();

    for (index, device) in devices.iter().enumerate() {
        let device_id = device.id();
        let cached = view
            .device_panel
            .views
            .remove(&device_id)
            .filter(|(cached, _)| cached == device.state())
            .map(|(_, result)| result);
        let device_view = cached.unwrap_or_else(|| {
            hex_engine::native_device_view(device.state())
                .map_err(|error| error.to_string().to_lowercase())
        });
        let rendered = match &device_view {
            Ok(device_view) => {
                let mut body = div()
                    .w(px(286.0))
                    .h(px(162.0))
                    .flex_shrink_0()
                    .flex()
                    .flex_wrap()
                    .content_start()
                    .gap(px(2.0))
                    .p(px(3.0));
                for parameter in &device_view.parameters {
                    body = body.child(parameter_control(
                        view,
                        cx,
                        device_id,
                        device_view.type_id,
                        parameter,
                    ));
                }
                let header_actions = device_actions(track_id, &devices, index, cx);
                device_frame(
                    theme::text(device_view.name.clone()),
                    control_style(),
                    header_actions,
                    body,
                )
            }
            Err(error) => {
                let body = div()
                    .w(px(286.0))
                    .h(px(162.0))
                    .flex_shrink_0()
                    .p(px(5.0))
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Primary)))
                    .child(theme::text(format!("unsupported device state · {error}")));
                device_frame(
                    theme::text(
                        device
                            .state()
                            .type_id()
                            .as_str()
                            .unwrap_or("unknown device"),
                    ),
                    control_style(),
                    div(),
                    body,
                )
            }
        };
        view.device_panel
            .views
            .insert(device_id, (device.state().clone(), device_view));
        chain = chain.child(rendered);
    }

    let add_open = view.device_panel.add_menu_open;
    let add_button = div()
        .id("device-add-toggle")
        .debug_selector(|| "device-add-toggle".into())
        .size(px(18.0))
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(theme::color(theme::PaletteRole::SecondaryText)))
        .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Raised))))
        .tooltip(|_, cx| {
            cx.new(|_| super::transport::MiniTooltip {
                description: "add native device".into(),
            })
            .into()
        })
        .child(theme::text(if add_open { "×" } else { "+" }))
        .on_click(cx.listener(|view, _, _, cx| {
            view.device_panel.add_menu_open = !view.device_panel.add_menu_open;
            cx.notify();
        }));
    let title = view
        .device_panel
        .message
        .clone()
        .unwrap_or_else(|| format!("{} · devices", track.name()));
    let mut header = div()
        .id("device-panel-header")
        .debug_selector(|| "device-panel-header".into())
        .h(px(22.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(5.0))
        .px(px(5.0))
        .text_xs()
        .bg(rgb(theme::color(theme::PaletteRole::Surface)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(rgb(if view.device_panel.message.is_some() {
                    theme::color(theme::PaletteRole::Primary)
                } else {
                    theme::color(theme::PaletteRole::Muted)
                }))
                .child(theme::text(title)),
        )
        .child(add_button);
    if catalog.is_err() {
        header = header.child(menu_message("device catalog unavailable"));
    }
    root = root.child(header).child(chain);
    if add_open {
        root = root.child(render_add_menu(cx, track_id, catalog));
    }
    root
}

fn render_add_menu(
    cx: &mut Context<EditorView>,
    track: TrackId,
    catalog: Result<Vec<hex_engine::NativeDeviceCatalogEntry>, String>,
) -> impl IntoElement {
    let mut menu = div()
        .id("device-add-menu")
        .debug_selector(|| "device-add-menu".into())
        .h(px(23.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(3.0))
        .px(px(4.0))
        .bg(rgb(theme::color(theme::PaletteRole::Raised)));
    match catalog {
        Ok(entries) if entries.is_empty() => {
            menu = menu.child(menu_message("no native devices available"));
        }
        Ok(entries) => {
            for entry in entries {
                let type_id = entry.type_id;
                let type_label = type_id.as_str().unwrap_or("invalid device").to_owned();
                let action_id = format!("add-native-device-{type_label}");
                menu = menu.child(
                    div()
                        .id(ElementId::Name(action_id.clone().into()))
                        .debug_selector(move || action_id.clone())
                        .h(px(18.0))
                        .px(px(5.0))
                        .flex()
                        .items_center()
                        .text_xs()
                        .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                        .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Hover))))
                        .cursor(gpui::CursorStyle::PointingHand)
                        .child(theme::text(entry.name))
                        .on_click(cx.listener(move |view, _, _, cx| {
                            if finalize_controls(view, cx)
                                && view.enqueue_device_barrier(Request::AddNativeDevice {
                                    track,
                                    type_id: Box::new(type_id),
                                    before: None,
                                })
                            {
                                view.device_panel.add_menu_open = false;
                                view.device_panel.message = None;
                            }
                            cx.notify();
                        })),
                );
            }
        }
        Err(error) => menu = menu.child(menu_message(&format!("device catalog error · {error}"))),
    }
    menu
}

fn device_actions(
    track: TrackId,
    devices: &[hex_project::Device],
    index: usize,
    cx: &mut Context<EditorView>,
) -> Div {
    let device = &devices[index];
    let id = device.id();
    let bypassed = device.bypassed();
    let before_up = index.checked_sub(1).map(|previous| devices[previous].id());
    let after_down = devices.get(index + 2).map(|device| device.id());
    let has_down = index + 1 < devices.len();
    let mut actions = div().flex().items_center().gap(px(2.0));
    actions = actions.child(
        action_button(
            format!("device-bypass-{}", id.value()),
            if bypassed { "⊘" } else { "●" },
            "toggle device bypass",
            if bypassed {
                theme::color(theme::PaletteRole::Muted)
            } else {
                theme::color(theme::PaletteRole::SecondaryText)
            },
        )
        .on_click(cx.listener(move |view, _, _, cx| {
            if finalize_controls(view, cx) {
                view.enqueue_device_barrier(Request::Edit(Command::SetDeviceBypass {
                    device: id,
                    bypassed: !bypassed,
                }));
            }
            cx.notify();
        })),
    );
    actions = actions.child(
        action_button(
            format!("device-up-{}", id.value()),
            "‹",
            "move device left",
            theme::color(theme::PaletteRole::Text),
        )
        .on_click(cx.listener(move |view, _, _, cx| {
            if let Some(before) = before_up
                && finalize_controls(view, cx)
            {
                view.enqueue_device_barrier(Request::Edit(Command::MoveDevice {
                    device: id,
                    track,
                    before: Some(before),
                }));
            }
            cx.notify();
        })),
    );
    actions = actions.child(
        action_button(
            format!("device-down-{}", id.value()),
            "›",
            "move device right",
            theme::color(theme::PaletteRole::Text),
        )
        .on_click(cx.listener(move |view, _, _, cx| {
            if has_down && finalize_controls(view, cx) {
                view.enqueue_device_barrier(Request::Edit(Command::MoveDevice {
                    device: id,
                    track,
                    before: after_down,
                }));
            }
            cx.notify();
        })),
    );
    actions.child(
        action_button(
            format!("device-remove-{}", id.value()),
            "×",
            "remove device",
            theme::color(theme::PaletteRole::Primary),
        )
        .on_click(cx.listener(move |view, _, _, cx| {
            if finalize_controls(view, cx) {
                view.enqueue_device_barrier(Request::Edit(Command::RemoveDevice(id)));
            }
            cx.notify();
        })),
    )
}

fn action_button(
    id: String,
    icon: &'static str,
    tooltip: &'static str,
    color: u32,
) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.clone().into()))
        .debug_selector(move || id.clone())
        .size(px(14.0))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.0))
        .text_color(rgb(color))
        .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Hover))))
        .cursor(gpui::CursorStyle::PointingHand)
        .tooltip(move |_, cx| {
            cx.new(|_| super::transport::MiniTooltip {
                description: tooltip.into(),
            })
            .into()
        })
        .child(theme::text(icon))
}

fn parameter_control(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    type_id: hex_lattice_contract::DeviceTypeId,
    parameter: &NativeDeviceParameterView,
) -> Div {
    let style = control_style();
    let key = ParameterKey {
        device,
        parameter: parameter.descriptor.id,
    };
    let descriptor = parameter.descriptor;
    let gesture_value = view.device_gesture.snapshot().filter(|snapshot| {
        snapshot.target.device == key.device && snapshot.target.parameter == key.parameter
    });
    let pending_value = view.device_panel.pending_values.get(&key).copied();
    if pending_value.is_some()
        && view.worker_requests_pending == 0
        && view.pending_requests.is_empty()
        && view.device_gesture_barriers.is_empty()
        && view.device_gesture.snapshot().is_none()
    {
        view.device_panel.pending_values.remove(&key);
    }
    let value = gesture_value.map_or_else(
        || {
            view.device_panel
                .pending_values
                .get(&key)
                .copied()
                .unwrap_or(parameter.value)
        },
        |snapshot| snapshot.value,
    );
    let compatible = matches!(
        (descriptor.kind, value),
        (ParameterKind::Float { .. }, ParameterValue::Float(_))
            | (ParameterKind::Bool, ParameterValue::Bool(_))
            | (ParameterKind::Choice { .. }, ParameterValue::Choice(_))
    );
    if !compatible {
        if let Some(mut control) = view.device_panel.controls.remove(&key) {
            cancel_control(view, cx, key, &mut control);
        }
        return unsupported_parameter(descriptor.label);
    }
    let matching = match view.device_panel.controls.get(&key) {
        Some(ParameterControl::Float {
            type_id: old_type,
            descriptor: old_descriptor,
            ..
        }) => {
            *old_type == type_id
                && *old_descriptor == descriptor
                && matches!(descriptor.kind, ParameterKind::Float { .. })
        }
        Some(ParameterControl::Bool {
            type_id: old_type,
            descriptor: old_descriptor,
            ..
        }) => {
            *old_type == type_id
                && *old_descriptor == descriptor
                && matches!(descriptor.kind, ParameterKind::Bool)
        }
        Some(ParameterControl::Choice {
            type_id: old_type,
            descriptor: old_descriptor,
            ..
        }) => {
            *old_type == type_id
                && *old_descriptor == descriptor
                && matches!(descriptor.kind, ParameterKind::Choice { .. })
        }
        None => false,
    };
    if !matching {
        if let Some(mut old) = view.device_panel.controls.remove(&key) {
            cancel_control(view, cx, key, &mut old);
        }
        if create_parameter_control(view, cx, key, type_id, descriptor, value, style).is_none() {
            return unsupported_parameter(descriptor.label);
        }
    }
    match view.device_panel.controls.get(&key) {
        Some(ParameterControl::Float { entity, .. }) => {
            if let ParameterValue::Float(value) = value {
                entity.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                    let _ = control.set_external_value(f64::from(value), cx);
                });
            } else {
                entity.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                });
            }
            div().w(px(136.0)).h(px(26.0)).child(entity.clone())
        }
        Some(ParameterControl::Bool { entity, .. }) => {
            if let ParameterValue::Bool(value) = value {
                entity.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                    control.set_checked(value, cx);
                });
            } else {
                entity.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                });
            }
            div().w(px(136.0)).h(px(22.0)).child(entity.clone())
        }
        Some(ParameterControl::Choice { entity, .. }) => {
            if let ParameterValue::Choice(value) = value {
                entity.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                    let _ = control.set_selected(value as usize, cx);
                });
            } else {
                entity.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                });
            }
            div().w(px(136.0)).h(px(22.0)).child(entity.clone())
        }
        None => unsupported_parameter(descriptor.label),
    }
}

fn create_parameter_control(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    key: ParameterKey,
    type_id: hex_lattice_contract::DeviceTypeId,
    descriptor: &'static ParameterDescriptor,
    value: ParameterValue,
    style: ControlStyle,
) -> Option<()> {
    let id = format!(
        "device-{}-parameter-{}",
        key.device.value(),
        key.parameter.value()
    );
    match (descriptor.kind, value) {
        (ParameterKind::Float { min, max, unit }, ParameterValue::Float(value)) if min < max => {
            let default = match descriptor.default {
                ParameterValue::Float(default) => f64::from(default),
                _ => return None,
            };
            let scale = if min < 0.0 && max > 0.0 {
                NumericScale::Bipolar
            } else {
                NumericScale::Linear
            };
            let spec = NumericSpec::new(
                f64::from(min),
                f64::from(max),
                default,
                scale,
                2,
                unit.map(|unit| std::sync::Arc::<str>::from(theme::text(unit).as_ref())),
            )
            .ok()?;
            let config = NumericControlConfig::new(
                id,
                theme::text(descriptor.label),
                NumericControlKind::HorizontalSlider,
                spec,
                f64::from(value),
                style,
            )
            .ok()?;
            let entity = cx.new(|control_cx| NumericControl::from_config(control_cx, config));
            let target = GestureTarget {
                device: key.device,
                parameter: key.parameter,
            };
            let subscription =
                cx.subscribe(&entity, move |view, _, event: &NumericControlEvent, cx| {
                    let value = ParameterValue::Float(event.value as f32);
                    match event.phase {
                        NumericEventPhase::Begin => {
                            if let Err(error) = view.begin_device_gesture(target, value) {
                                view.error = Some(error.to_owned());
                            }
                        }
                        NumericEventPhase::Change => {
                            if let Err(error) = view.change_device_gesture(value) {
                                view.error = Some(error.to_owned());
                            }
                        }
                        NumericEventPhase::Commit => {
                            if let Err(error) = view.finish_device_gesture(value) {
                                view.error = Some(error.to_owned());
                            }
                        }
                        NumericEventPhase::Cancel => {
                            if let Err(error) = view.cancel_device_gesture() {
                                view.error = Some(error.to_owned());
                            }
                        }
                    }
                    cx.notify();
                });
            view.device_panel.controls.insert(
                key,
                ParameterControl::Float {
                    entity,
                    _subscription: subscription,
                    type_id,
                    descriptor,
                },
            );
            Some(())
        }
        (ParameterKind::Bool, ParameterValue::Bool(value)) => {
            let config = ToggleConfig::new(id, theme::text(descriptor.label), value, style).ok()?;
            let entity = cx.new(|control_cx| Toggle::from_config(control_cx, config));
            let target = key;
            let subscription = cx.subscribe(&entity, move |view, _, event: &ToggleChanged, cx| {
                let value = ParameterValue::Bool(event.checked);
                view.device_panel.pending_values.insert(target, value);
                if !view.enqueue_device_barrier(Request::SetDeviceParameter {
                    device: target.device,
                    parameter: target.parameter,
                    value,
                }) {
                    view.device_panel.pending_values.remove(&target);
                }
                cx.notify();
            });
            view.device_panel.controls.insert(
                key,
                ParameterControl::Bool {
                    entity,
                    _subscription: subscription,
                    type_id,
                    descriptor,
                },
            );
            Some(())
        }
        (ParameterKind::Choice { labels }, ParameterValue::Choice(value)) => {
            let choices = labels.iter().copied().map(theme::text);
            let config = SelectorConfig::new(id, choices, value as usize, style).ok()?;
            let entity = cx.new(|control_cx| Selector::from_config(control_cx, config));
            let target = key;
            let subscription =
                cx.subscribe(&entity, move |view, _, event: &SelectorChanged, cx| {
                    let Ok(value) = u32::try_from(event.index) else {
                        return;
                    };
                    let value = ParameterValue::Choice(value);
                    view.device_panel.pending_values.insert(target, value);
                    if !view.enqueue_device_barrier(Request::SetDeviceParameter {
                        device: target.device,
                        parameter: target.parameter,
                        value,
                    }) {
                        view.device_panel.pending_values.remove(&target);
                    }
                    cx.notify();
                });
            view.device_panel.controls.insert(
                key,
                ParameterControl::Choice {
                    entity,
                    _subscription: subscription,
                    type_id,
                    descriptor,
                },
            );
            Some(())
        }
        _ => None,
    }
}

fn cancel_control(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    key: ParameterKey,
    control: &mut ParameterControl,
) {
    if let ParameterControl::Float { entity, .. } = control {
        entity.update(cx, |control, cx| control.cancel(cx));
        if view.device_gesture.snapshot().is_some_and(|snapshot| {
            snapshot.target.device == key.device && snapshot.target.parameter == key.parameter
        }) {
            let _ = view.cancel_device_gesture();
        }
    }
}

fn unsupported_parameter(label: &str) -> Div {
    div()
        .w(px(136.0))
        .h(px(22.0))
        .flex()
        .items_center()
        .text_xs()
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(format!("{label} · unsupported")))
}

fn menu_message(message: &str) -> Div {
    div()
        .h(px(18.0))
        .flex()
        .items_center()
        .px(px(4.0))
        .text_xs()
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(message))
}

fn empty_panel(message: &str) -> Stateful<Div> {
    div()
        .id("device-panel-empty")
        .debug_selector(|| "device-panel-empty".into())
        .h(px(188.0))
        .w_full()
        .flex_shrink_0()
        .flex()
        .items_center()
        .px(px(8.0))
        .bg(rgb(theme::color(theme::PaletteRole::Surface)))
        .text_xs()
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(message))
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
