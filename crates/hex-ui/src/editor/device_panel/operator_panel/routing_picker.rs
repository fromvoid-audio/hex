//! Visual routing choices share the existing canonical algorithm command.
//! Popup navigation is view-only; only deliberate confirmation changes sound.
use super::algorithms::diagram;
use super::*;
use gpui::{AnchoredPositionMode, Corner, Window, anchored, deferred, point};

fn close(view: &mut EditorView, cx: &mut Context<EditorView>, device: DeviceId) {
    if let Some(state) = view.device_panel.operator_previews.get_mut(&device)
        && state.routing_popup.take().is_some()
    {
        cx.notify();
    }
}

fn choose(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    index: usize,
    owner: Option<context::Owner>,
) {
    if index < 11 && owner.is_some_and(|o| o.is_current(view)) {
        algorithms::select(view, cx, device, index as u32 + 1);
    }
    close(view, cx, device);
}

fn activate(
    view: &mut EditorView,
    window: &mut Window,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    owner: Option<context::Owner>,
) {
    if !owner.is_some_and(|o| o.is_current(view)) {
        return;
    }
    if let Some(index) = view
        .device_panel
        .operator_previews
        .get(&device)
        .and_then(|s| s.routing_popup)
    {
        choose(view, cx, device, index, owner);
    } else if preview::finalize_controls(view, cx, device) && finalize_controls(view, cx) {
        let key = ParameterKey {
            device,
            parameter: id(600),
        };
        let selected = view
            .device_panel
            .pending_values
            .get(&key)
            .copied()
            .or_else(|| {
                view.device_panel
                    .views
                    .get(&device)?
                    .2
                    .as_ref()
                    .ok()?
                    .parameters
                    .iter()
                    .find(|entry| entry.descriptor.id == key.parameter)
                    .map(|entry| entry.value)
            })
            .and_then(|value| {
                if let ParameterValue::Choice(choice) = value {
                    Some(choice)
                } else {
                    None
                }
            });
        let state = view
            .device_panel
            .operator_previews
            .entry(device)
            .or_default();
        state.routing_popup =
            Some(selected.filter(|v| (1..=11).contains(v)).unwrap_or(1) as usize - 1);
        if let Some(focus) = &state.routing_focus {
            focus.focus(window);
        }
        cx.notify();
    }
}

fn step(view: &mut EditorView, cx: &mut Context<EditorView>, device: DeviceId, forward: bool) {
    if let Some(state) = view.device_panel.operator_previews.get_mut(&device)
        && let Some(index) = state.routing_popup.as_mut()
    {
        *index = if forward {
            (*index + 1).min(10)
        } else {
            index.saturating_sub(1)
        };
        cx.notify();
    }
}

fn blur_listener(
    cx: &Context<EditorView>,
    device: DeviceId,
    focus: gpui::FocusHandle,
) -> impl Fn(Vec<gpui::Bounds<gpui::Pixels>>, &mut Window, &mut gpui::App) + 'static {
    let entity = cx.weak_entity();
    move |_, window, app| {
        let _ = entity.update(app, |view, cx| {
            let Some(state) = view.device_panel.operator_previews.get_mut(&device) else {
                return;
            };
            if state.routing_blur.is_none() {
                state.routing_blur =
                    Some(cx.on_focus_out(&focus, window, move |view, _, window, cx| {
                        if let Some(state) = view.device_panel.operator_previews.get_mut(&device)
                            && state.routing_popup.take().is_some()
                        {
                            cx.defer_in(window, |_, _, cx| cx.notify());
                        }
                    }));
            }
        });
    }
}

pub(super) fn render(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    canonical: Option<ParameterValue>,
) -> Stateful<Div> {
    let owner = context::Owner::capture(view, device);
    let selected = canonical.and_then(|value| {
        match effective_parameter_value(
            view,
            ParameterKey {
                device,
                parameter: id(600),
            },
            value,
        ) {
            ParameterValue::Choice(choice) => Some(choice),
            _ => None,
        }
    });
    let state = view
        .device_panel
        .operator_previews
        .entry(device)
        .or_default();
    let focus = state
        .routing_focus
        .get_or_insert_with(|| cx.focus_handle().tab_index(0).tab_stop(true))
        .clone();
    let open = state.routing_popup;
    let icon = div().relative().w(px(38.0)).h(px(34.0));
    let icon = match selected {
        Some(choice @ 1..=11) => icon
            .debug_selector(move || format!("operator-routing-current-{}-{choice}", device.value()))
            .child(diagram(choice as usize - 1, false)),
        _ => icon
            .debug_selector(move || format!("operator-routing-current-{}-custom", device.value()))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(8.0))
            .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
            .child(theme::text("custom")),
    };
    let mut root = hex_widgets::device_focus_target(super::super::device_layout_style())
        .on_children_prepainted(blur_listener(cx, device, focus.clone()))
        .id(ElementId::Name(
            format!("operator-preview-{}-route-link", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-preview-{}-route-link", device.value()))
        .track_focus(&focus)
        .key_context(if open.is_some() {
            "hex_operator_routing hex_operator_routing_open"
        } else {
            "hex_operator_routing"
        })
        .w(px(40.0))
        .h(px(36.0))
        .relative()
        .flex()
        .items_center()
        .cursor(gpui::CursorStyle::PointingHand)
        .tooltip(|_, cx| {
            cx.new(|_| crate::editor::transport::MiniTooltip {
                kind: crate::tooltips::Kind::control("routing presets"),
                description: theme::text(
                    "routing presets · enter or space · alt-enter for global settings",
                ),
            })
            .into()
        })
        .child(icon)
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(move |view, _, window, cx| {
                cx.stop_propagation();
                if view
                    .device_panel
                    .operator_previews
                    .get(&device)
                    .is_some_and(|s| s.routing_popup.is_some())
                {
                    close(view, cx, device);
                } else {
                    activate(view, window, cx, device, owner);
                }
            }),
        )
        .on_mouse_down_out(cx.listener(move |view, _, _, cx| close(view, cx, device)))
        .on_key_down(cx.listener(move |view, event: &gpui::KeyDownEvent, _, cx| {
            if event.keystroke.key == "tab" {
                close(view, cx, device);
            }
        }))
        .on_action(cx.trace(move |view, _: &ActivateRouting, window, cx| {
            activate(view, window, cx, device, owner);
            cx.stop_propagation();
        }))
        .on_action(cx.trace(move |view, _: &RoutingSettings, _, cx| {
            close(view, cx, device);
            if let Some(owner) = owner {
                context::select_routing(view, cx, owner);
            }
            cx.stop_propagation();
        }))
        .on_action(cx.trace(move |view, _: &CloseRouting, _, cx| {
            close(view, cx, device);
            cx.stop_propagation();
        }))
        .on_action(cx.trace(move |view, _: &NextRouting, _, cx| {
            step(view, cx, device, true);
            cx.stop_propagation();
        }))
        .on_action(cx.trace(move |view, _: &PreviousRouting, _, cx| {
            step(view, cx, device, false);
            cx.stop_propagation();
        }));
    if let Some(highlighted) = open {
        let style = super::super::control_style();
        let mut popup = div()
            .id(ElementId::Name(
                format!("operator-routing-popup-{}", device.value()).into(),
            ))
            .debug_selector(move || format!("operator-routing-popup-{}", device.value()))
            .w(px(170.0))
            .p(px(3.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .rounded(px(3.0))
            .border_1()
            .border_color(rgb(style.border))
            .bg(rgb(style.surface))
            .occlude();
        for first in (0..11).step_by(4) {
            let mut row = div().flex().gap(px(2.0));
            for index in first..(first + 4).min(11) {
                let active = selected == Some(index as u32 + 1);
                let field = hex_widgets::device_segment(
                    theme::text(""),
                    theme::device_segment_style(active),
                )
                .id(ElementId::Name(
                    format!("operator-routing-option-{}-{index}", device.value()).into(),
                ))
                .debug_selector(move || {
                    format!("operator-routing-option-{}-{index}", device.value())
                })
                .w(px(38.0))
                .h(px(34.0))
                .flex_shrink_0()
                .relative()
                .border_1()
                .border_color(rgb(if index == highlighted {
                    style.accent
                } else {
                    style.border
                }))
                .cursor(gpui::CursorStyle::PointingHand)
                .child(diagram(index, active))
                .tooltip(move |_, cx| {
                    cx.new(|_| crate::editor::transport::MiniTooltip {
                        kind: crate::tooltips::Kind::Identification,
                        description: theme::text(format!("algorithm {}", index + 1)),
                    })
                    .into()
                })
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |view, _, _, cx| {
                        cx.stop_propagation();
                        choose(view, cx, device, index, owner);
                    }),
                );
                row = row.child(field);
            }
            popup = popup.child(row);
        }
        let content = popup;
        let content = deferred(
            anchored()
                .anchor(Corner::BottomLeft)
                .position(point(px(0.0), px(0.0)))
                .position_mode(AnchoredPositionMode::Local)
                .child(content),
        )
        .with_priority(100);
        root = root.child(content);
    }
    root
}
