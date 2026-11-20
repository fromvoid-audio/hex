//! Device disclosure belongs to the editor. The canonical chain, playback and
//! parameter ownership stay independent of the visible card width.
use super::*;
use gpui::IntoElement;

pub(super) const CARD_WIDTH: f32 = hex_widgets::DEVICE_FRAME_HEADER_WIDTH
    + 2.0 * hex_widgets::DEVICE_FRAME_BORDER_WIDTH
    + device_meter::WIDTH;

pub(super) fn frame(
    view: &EditorView,
    cx: &mut Context<EditorView>,
    device: &hex_project::Device,
    class: NativeDeviceClass,
    metadata: Option<&NativeDeviceView>,
    leading: impl IntoElement,
    actions: impl IntoElement,
) -> Div {
    let title = if device.state().type_id().as_str() == Ok("hex.vst3") {
        view.vst3_device_title(device)
    } else {
        theme::text(metadata.map_or_else(
            || {
                device
                    .state()
                    .type_id()
                    .as_str()
                    .unwrap_or("unknown device")
                    .to_owned()
            },
            |metadata| metadata.name.clone(),
        ))
    };
    hex_widgets::device_frame_header_only(
        device_drag_title(view, cx, device, class, title),
        theme::device_frame_style(view.device_panel.is_selected(device.id())),
        leading,
        actions,
    )
}

/// The first press already captured the selection for header dragging. Keep
/// that same snapshot through the second press so a double-click can act on
/// the group that was selected before the first ordinary click.
pub(super) fn header_selection(
    view: &EditorView,
    device: DeviceId,
) -> Option<&hex_project::DeviceSelectionIds> {
    view.device_panel
        .drag_selection
        .as_ref()
        .filter(|press| {
            press.generation == view.project_generation
                && Some(press.owner) == view.device_panel.owner
                && press.device == device
                && view
                    .state
                    .as_ref()
                    .is_some_and(|state| state.revision == press.revision)
        })
        .map(|press| &press.selected)
}

pub(super) fn toggle_header(view: &mut EditorView, device: DeviceId, cx: &mut Context<EditorView>) {
    let selected = header_selection(view, device).cloned();
    let collapse = !view.device_panel.collapsed.contains(&device);
    apply(view, device, collapse, selected.as_ref(), cx);
}

pub(super) fn set_selected(
    view: &mut EditorView,
    device: DeviceId,
    collapse: bool,
    cx: &mut Context<EditorView>,
) {
    apply(view, device, collapse, None, cx);
}

fn apply(
    view: &mut EditorView,
    device: DeviceId,
    collapse: bool,
    header_selection: Option<&hex_project::DeviceSelectionIds>,
    cx: &mut Context<EditorView>,
) {
    let Some((state, owner)) = view.state.as_ref().zip(view.device_panel.owner) else {
        return;
    };
    if !view.clip_view_open
        || !view.device_panel_active
        || view.device_panel.generation != view.project_generation
        || selected_owner(view, state) != Some(owner)
        || !view.device_panel.is_selected(device)
    {
        return;
    }
    let order = chain_ids(&state.project, owner);
    if !order.contains(&device) {
        return;
    }
    let selected: Vec<_> = order
        .into_iter()
        .filter(|id| {
            header_selection.map_or_else(
                || view.device_panel.is_selected(*id),
                |press| press.ids().contains(id),
            )
        })
        .collect();
    // A folded body cannot receive pointer release or finish a numeric draft.
    if collapse
        && (view.device_panel.has_pending_draft(cx)
            || view.sampler_parameter.numeric_draft_active()
            || view.device_gesture.snapshot().is_some()
                && !view.device_gesture.has_pending_terminal()
            || view.active_native_device_drag.is_some())
    {
        return;
    }
    let panel = &mut view.device_panel;
    if header_selection.is_some() {
        panel.select_only(Some(device));
        panel
            .selected_others
            .extend(selected.iter().copied().filter(|id| *id != device));
    }
    let mut changed = header_selection.is_some();
    for id in selected {
        changed |= if collapse {
            panel.collapsed.insert(id)
        } else {
            panel.collapsed.remove(&id)
        };
    }
    if changed {
        panel.chain_window.reveal_after_prepare(device);
        cx.notify();
    }
}
