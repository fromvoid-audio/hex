//! View-only catalog insertion target; canonical chain edits remain with Service.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct Target {
    generation: u64,
    owner: DevicePanelOwner,
    after: DeviceId,
}

/// Resolve at activation time, so reordering preserves "after this device" and
/// deleting the source or changing owner never silently appends.
pub(super) fn before(view: &EditorView, owner: DevicePanelOwner) -> Option<Option<DeviceId>> {
    if view.device_panel.owner != Some(owner) {
        return None;
    }
    let state = view.state.as_ref()?;
    let Some(target) = view.device_panel.add_menu_target else {
        return Some(None);
    };
    if target.owner != owner || target.generation != view.project_generation {
        return None;
    }
    let order = chain_ids(&state.project, owner);
    let index = order.iter().position(|id| *id == target.after)?;
    Some(order.get(index + 1).copied())
}

pub(super) fn button(
    view: &EditorView,
    device: DeviceId,
    cx: &mut Context<EditorView>,
) -> Stateful<Div> {
    let owner = view.device_panel.owner;
    let generation = view.project_generation;
    let selector = format!("device-{}-add-after", device.value());
    hex_widgets::action(theme::text("+"), theme::action_style())
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .size(px(10.0))
        .flex_shrink_0()
        .rounded_full()
        .text_size(px(10.0))
        .line_height(px(10.0))
        .tooltip(|_, cx| {
            cx.new(|_| super::super::transport::MiniTooltip {
                kind: crate::tooltips::Kind::Identification,
                description: "insert device here".into(),
            })
            .into()
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |view, _, _, cx| {
            cx.stop_propagation();
            let Some(owner) = owner.filter(|owner| {
                generation == view.project_generation
                    && view.device_panel.owner == Some(*owner)
                    && view
                        .state
                        .as_ref()
                        .is_some_and(|state| chain_ids(&state.project, *owner).contains(&device))
            }) else {
                return;
            };
            view.device_panel.add_menu_target = Some(Target {
                generation,
                owner,
                after: device,
            });
            view.device_panel.add_menu_open = true;
            cx.notify();
        }))
}
