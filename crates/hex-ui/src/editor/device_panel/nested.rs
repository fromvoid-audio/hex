//! Generic shared host helpers for serial nested device units.
//!
//! Canonical ownership stays in `hex-project` (`Device::parent`, `DeviceLocation`),
//! admission in `hex-engine`, and worker edits in `hex-session`. This module owns
//! only UI collapse state queries, membership markers, effective-bypass display,
//! and the right-end nested-device widget (expand/close + picker/browser drop).
use super::*;
use hex_project::{
    Device, DeviceId, DeviceOwner, DeviceUnitClass, MAX_SERIAL_DEVICE_DEPTH, Project,
};

/// Nesting depth within the canonical owning chain (`0` for root devices).
pub(in crate::editor) fn depth(project: &Project, device: DeviceId) -> usize {
    hex_engine::project_device_nesting_depth(project, device).unwrap_or(0)
}

/// Number of canonical descendants under `parent` in the same owning chain.
pub(in crate::editor) fn descendant_count(project: &Project, parent: DeviceId) -> usize {
    let Some(location) = project.device_location(parent) else {
        return 0;
    };
    let mut count = 0usize;
    let check_chain = |chain: &[Device]| -> usize {
        let mut total = 0usize;
        for candidate in chain {
            if is_descendant_of(project, candidate.id(), parent) {
                total += 1;
            }
        }
        total
    };
    match location.owner {
        DeviceOwner::Track(id) => {
            if let Some(track) = project.track(id) {
                count = check_chain(track.devices());
            }
        }
        DeviceOwner::Return(id) => {
            if let Some(bus) = project.return_tracks().iter().find(|bus| bus.id() == id) {
                count = check_chain(bus.devices());
            }
        }
        DeviceOwner::Main => {
            count = check_chain(project.main_devices());
        }
        DeviceOwner::Source(id) => {
            if let Some(source) = project.native_source(id) {
                for candidate in source.post_instrument_effects() {
                    if is_descendant_of(project, candidate.id(), parent) {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}

fn is_descendant_of(project: &Project, candidate: DeviceId, ancestor: DeviceId) -> bool {
    let Some(mut current) = project.device(candidate) else {
        return false;
    };
    for _ in 0..MAX_SERIAL_DEVICE_DEPTH {
        let Some(parent) = current.parent() else {
            return false;
        };
        if parent == ancestor {
            return true;
        }
        let Some(next) = project.device(parent) else {
            return false;
        };
        current = next;
    }
    false
}

/// Whether `device` is hidden purely in the UI because any strict ancestor is
/// currently collapsed in `collapsed_children`. Never alters audio or project state.
pub(in crate::editor) fn hidden_by_collapsed_ancestor(
    collapsed_children: &HashSet<DeviceId>,
    project: &Project,
    device: DeviceId,
) -> bool {
    if collapsed_children.is_empty() {
        return false;
    }
    let Some(mut current) = project.device(device) else {
        return false;
    };
    for _ in 0..MAX_SERIAL_DEVICE_DEPTH {
        let Some(parent) = current.parent() else {
            return false;
        };
        if collapsed_children.contains(&parent) {
            return true;
        }
        let Some(next) = project.device(parent) else {
            return false;
        };
        current = next;
    }
    false
}

/// Whether any strict ancestor of `device` is effectively bypassed in the UI
/// (combining canonical `bypassed()` with pending bypass receipts).
pub(in crate::editor) fn ancestor_displayed_bypassed(
    view: &EditorView,
    project: &Project,
    device: DeviceId,
) -> bool {
    let Some(mut current) = project.device(device) else {
        return false;
    };
    for _ in 0..MAX_SERIAL_DEVICE_DEPTH {
        let Some(parent) = current.parent() else {
            return false;
        };
        let Some(ancestor) = project.device(parent) else {
            return false;
        };
        if view
            .device_panel
            .bypass
            .displayed(ancestor.id(), ancestor.bypassed())
        {
            return true;
        }
        current = ancestor;
    }
    false
}

/// Whether `device` currently admits at least one fresh nested catalog effect.
/// Never returns `true` for MIDI processors or depth/capacity-capped hosts.
pub(in crate::editor) fn can_accept_nested_child(project: &Project, device: DeviceId) -> bool {
    let Some(location) = project.device_location(device) else {
        return false;
    };
    if location.class == DeviceUnitClass::MidiProcessor {
        return false;
    }
    let Some(current_depth) = hex_engine::project_device_nesting_depth(project, device) else {
        return false;
    };
    if current_depth + 1 >= MAX_SERIAL_DEVICE_DEPTH {
        return false;
    }
    match location.owner {
        DeviceOwner::Track(id) => project
            .track(id)
            .is_some_and(|track| track.devices().len() < hex_project::MAX_DEVICES_PER_TRACK),
        DeviceOwner::Return(id) => project
            .return_tracks()
            .iter()
            .find(|bus| bus.id() == id)
            .is_some_and(|bus| bus.devices().len() < hex_project::MAX_DEVICES_PER_TRACK),
        DeviceOwner::Main => project.main_devices().len() < hex_project::MAX_DEVICES_PER_TRACK,
        DeviceOwner::Source(id) => project
            .native_source(id)
            .is_some_and(|source| source.devices().count() < hex_project::MAX_SOURCE_DEVICES),
    }
}

/// Semantic accent color for a nested child's parent-membership marker by depth.
pub(in crate::editor) fn membership_marker_color(nest_depth: usize) -> u32 {
    match nest_depth {
        0 => theme::color(theme::PaletteRole::Border),
        1 => theme::color(theme::PaletteRole::Secondary),
        2 => theme::color(theme::PaletteRole::Primary),
        _ => theme::color(theme::PaletteRole::WarmAccent),
    }
}

/// Overlay membership marker bar for nested child cards (`device.parent().is_some()`).
/// Absolute positioning preserves exact 200 px height and card width/culling geometry.
pub(in crate::editor) fn membership_marker(device: &Device, nest_depth: usize) -> Option<Div> {
    let parent = device.parent()?;
    let id = device.id();
    let color = membership_marker_color(nest_depth);
    Some(
        div()
            .debug_selector(move || format!("device-nested-marker-{}", id.value()))
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(2.0))
            .bg(rgb(color))
            .child(
                div()
                    .debug_selector(move || {
                        format!(
                            "device-nested-parent-{}-depth-{}",
                            parent.value(),
                            nest_depth
                        )
                    })
                    .size_full(),
            ),
    )
}

/// Right-end nested-device controls placed at the top of the device's meter strip.
/// Emits no decorative controls when a device neither has children nor admits nesting.
pub(in crate::editor) fn end_slot_controls(
    view: &EditorView,
    device: &Device,
    cx: &mut Context<EditorView>,
) -> Option<Div> {
    let state = view.state.as_ref()?;
    let id = device.id();
    let descendants = descendant_count(&state.project, id);
    let can_add = can_accept_nested_child(&state.project, id);
    if descendants == 0 && !can_add {
        return None;
    }
    let owner = view.device_panel.owner;
    let generation = view.project_generation;
    let mut column = div()
        .debug_selector(move || format!("device-{}-nested-slot", id.value()))
        .w_full()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .items_center()
        .gap(px(2.0));

    if descendants > 0 {
        let collapsed = view.device_panel.collapsed_children.contains(&id);
        let toggle_selector = format!("device-{}-nested-toggle", id.value());
        let glyph = if collapsed { "▸" } else { "▾" };
        let tooltip_label: gpui::SharedString = if collapsed {
            format!("expand {descendants} nested")
        } else {
            format!("collapse {descendants} nested")
        }
        .into();
        let style = if collapsed {
            theme::device_header_action_style(false, theme::DeviceActionRole::Enabled)
        } else {
            theme::action_style()
        };
        column = column.child(
            hex_widgets::action(theme::text(glyph), style)
                .id(ElementId::Name(toggle_selector.clone().into()))
                .debug_selector(move || toggle_selector.clone())
                .size(px(10.0))
                .flex_shrink_0()
                .rounded_sm()
                .text_size(px(8.0))
                .line_height(px(10.0))
                .tooltip(move |_, cx| {
                    let description = tooltip_label.clone();
                    cx.new(|_| super::super::transport::MiniTooltip {
                        kind: crate::tooltips::Kind::Identification,
                        description,
                    })
                    .into()
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |view, _, _, cx| {
                    cx.stop_propagation();
                    if view.project_generation != generation {
                        return;
                    }
                    if !finalize_controls(view, cx) {
                        return;
                    }
                    if !view.device_panel.collapsed_children.remove(&id) {
                        view.device_panel.collapsed_children.insert(id);
                    }
                    cx.notify();
                })),
        );
    }

    if can_add {
        let add_selector = format!("device-{}-add-nested", id.value());
        let drop_style_color = theme::color(theme::PaletteRole::Secondary);
        let add_button = hex_widgets::action(theme::text("↳"), theme::action_style())
            .id(ElementId::Name(add_selector.clone().into()))
            .debug_selector(move || add_selector.clone())
            .size(px(10.0))
            .flex_shrink_0()
            .rounded_sm()
            .text_size(px(8.0))
            .line_height(px(10.0))
            .drag_over::<drag::NativeDeviceDrag>(move |style, _, _, _| {
                style.bg(rgb(drop_style_color))
            })
            .tooltip(|_, cx| {
                cx.new(|_| super::super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: "insert nested device".into(),
                })
                .into()
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |view, _, window, cx| {
                cx.stop_propagation();
                let Some(owner) = owner.filter(|owner| {
                    generation == view.project_generation
                        && view.device_panel.owner == Some(*owner)
                        && view
                            .state
                            .as_ref()
                            .is_some_and(|state| can_accept_nested_child(&state.project, id))
                }) else {
                    return;
                };
                view.device_panel.add_menu_target =
                    Some(device_add::Target::nested(generation, owner, id));
                device_picker::open(view, window, cx);
            }))
            .on_drop(
                cx.listener(move |view, payload: &drag::NativeDeviceDrag, _, cx| {
                    cx.stop_propagation();
                    view.drop_browser_device_into_nested(payload, id, cx);
                }),
            );
        column = column.child(add_button);
    }

    Some(column)
}

impl EditorView {
    /// Atomic browser-device drop onto one canonical parent's nested insertion target.
    /// Uses the same `Request::InsertNestedCatalogDevice` contract as the searchable picker.
    pub(in crate::editor) fn drop_browser_device_into_nested(
        &mut self,
        payload: &drag::NativeDeviceDrag,
        parent: DeviceId,
        cx: &mut Context<Self>,
    ) {
        self.device_panel.active_native_drag = None;
        let Some(state) = self.state.as_ref() else {
            return;
        };
        if payload.project_generation != self.project_generation
            || payload.revision != state.revision
        {
            self.device_panel.message =
                Some("nested device drop canceled · project changed".into());
            cx.notify();
            return;
        }
        let drag::NativeDeviceDragSource::Browser { class, type_id } = payload.source else {
            self.device_panel.message =
                Some("nested slot accepts compatible browser effects only".into());
            cx.notify();
            return;
        };
        if class != NativeDeviceClass::Effect {
            self.device_panel.message =
                Some("nested device slots accept compatible effects only".into());
            cx.notify();
            return;
        }
        if let Err(reason) =
            hex_engine::native_effect_admitted_for_nested_parent(&state.project, parent, type_id)
        {
            self.device_panel.message = Some(reason.into());
            cx.notify();
            return;
        }
        let request = Request::InsertNestedCatalogDevice {
            project_generation: self.project_generation,
            revision: state.revision,
            parent,
            class,
            type_id,
            control: Some(Arc::new(
                hex_session::service::PluginEffectControl::default(),
            )),
        };
        if finalize_controls(self, cx) && self.enqueue_catalog_device(request) {
            self.device_panel.collapsed_children.remove(&parent);
            self.device_panel.message = None;
        }
        cx.notify();
    }
}
