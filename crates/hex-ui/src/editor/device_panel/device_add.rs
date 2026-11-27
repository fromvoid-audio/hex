//! View-only catalog insertion target; canonical chain edits remain with Service.
use super::*;

#[derive(Clone, Copy)]
pub(super) enum Target {
    After {
        generation: u64,
        owner: DevicePanelOwner,
        after: DeviceId,
    },
    Nested {
        generation: u64,
        owner: DevicePanelOwner,
        parent: DeviceId,
    },
}

impl Target {
    pub(super) fn nested(generation: u64, owner: DevicePanelOwner, parent: DeviceId) -> Self {
        Self::Nested {
            generation,
            owner,
            parent,
        }
    }
}

/// Resolved catalog destination at activation time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Destination {
    Chain { before: Option<DeviceId> },
    Nested { parent: DeviceId },
}

/// Active nested parent when the searchable picker was opened from a nested slot.
pub(super) fn nested_parent(view: &EditorView, owner: DevicePanelOwner) -> Option<DeviceId> {
    let Target::Nested {
        generation,
        owner: target_owner,
        parent,
    } = view.device_panel.add_menu_target?
    else {
        return None;
    };
    if view.device_panel.owner != Some(owner)
        || target_owner != owner
        || generation != view.project_generation
    {
        return None;
    }
    let state = view.state.as_ref()?;
    nested::can_accept_nested_child(&state.project, parent).then_some(parent)
}

/// Resolve a chain-sibling `before` anchor for tests and callers expecting chain insertion.
#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn before(view: &EditorView, owner: DevicePanelOwner) -> Option<Option<DeviceId>> {
    match destination(view, owner)? {
        Destination::Chain { before } => Some(before),
        Destination::Nested { .. } => None,
    }
}

/// Resolve at activation time, so reordering preserves "after this device" and
/// deleting the source or changing owner never silently appends.
pub(super) fn destination(view: &EditorView, owner: DevicePanelOwner) -> Option<Destination> {
    if view.device_panel.owner != Some(owner) {
        return None;
    }
    let state = view.state.as_ref()?;
    let Some(target) = view.device_panel.add_menu_target else {
        return Some(Destination::Chain { before: None });
    };
    match target {
        Target::After {
            generation,
            owner: target_owner,
            after,
        } => {
            if target_owner != owner || generation != view.project_generation {
                return None;
            }
            let order = chain_ids(&state.project, owner);
            if let Some(index) = order.iter().position(|id| *id == after) {
                return Some(Destination::Chain {
                    before: order.get(index + 1).copied(),
                });
            }
            // Source post-instrument effects live in the retained source's child
            // unit rather than the track-local chain; insert into the same parent.
            let parent = state.project.device(after)?.parent()?;
            nested::can_accept_nested_child(&state.project, parent)
                .then_some(Destination::Nested { parent })
        }
        Target::Nested {
            generation,
            owner: target_owner,
            parent,
        } => {
            if target_owner != owner || generation != view.project_generation {
                return None;
            }
            nested::can_accept_nested_child(&state.project, parent)
                .then_some(Destination::Nested { parent })
        }
    }
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
        .on_click(cx.listener(move |view, _, window, cx| {
            cx.stop_propagation();
            let Some(owner) = owner.filter(|owner| {
                generation == view.project_generation
                    && view.device_panel.owner == Some(*owner)
                    && view.state.as_ref().is_some_and(|state| {
                        chain_ids(&state.project, *owner).contains(&device)
                            || state
                                .project
                                .device(device)
                                .and_then(hex_project::Device::parent)
                                .is_some_and(|parent| {
                                    nested::can_accept_nested_child(&state.project, parent)
                                })
                    })
            }) else {
                return;
            };
            view.device_panel.add_menu_target = Some(Target::After {
                generation,
                owner,
                after: device,
            });
            device_picker::open(view, window, cx);
        }))
}
