//! Return-owned effect rail, reusing the shared device descriptors and gestures.
use super::*;

pub(super) fn render(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    state: &ViewState,
    return_id: hex_project::ReturnId,
) -> Stateful<Div> {
    let Some(return_track) = state
        .project
        .return_tracks()
        .iter()
        .find(|track| track.id() == return_id)
    else {
        return empty_panel("return target is no longer available");
    };
    let devices = return_track.devices().to_vec();
    let alive: HashSet<_> = devices.iter().map(hex_project::Device::id).collect();
    let stale_keys: Vec<_> = view
        .device_panel
        .controls
        .keys()
        .filter(|key| !alive.contains(&key.device))
        .copied()
        .collect();
    let active_target = view
        .device_gesture
        .snapshot()
        .map(|snapshot| snapshot.target);
    let stale_target_has_work = active_target.is_some_and(|target| match target {
        GestureTarget::DeviceParameter { device, .. } => !alive.contains(&device),
        GestureTarget::ReturnSend { .. } => false,
    }) || stale_keys.iter().any(|key| {
        matches!(
            view.device_panel.controls.get(key),
            Some(ParameterControl::Float { entity, .. })
                if entity.read_with(cx, |control, _| {
                    control.is_interacting() || control.is_editing()
                })
        )
    });
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
    if stale_target_has_work {
        view.device_panel.message = Some("device target is no longer available".into());
    } else if view.device_panel.message.as_deref() == Some("device target is no longer available") {
        view.device_panel.message = None;
    }

    if view.device_panel.effect_catalog.is_none() {
        view.device_panel.effect_catalog = Some(
            hex_engine::native_device_catalog_for_class(NativeDeviceClass::Effect)
                .map_err(|error| error.to_string().to_lowercase()),
        );
    }
    let catalog = view
        .device_panel
        .effect_catalog
        .clone()
        .unwrap_or_else(|| Ok(Vec::new()));
    let mut root = div()
        .id("device-panel")
        .debug_selector(|| "device-panel".into())
        .h(px(DEVICE_PANEL_HEIGHT))
        .w_full()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .relative()
        .bg(rgb(theme::color(theme::PaletteRole::Surface)))
        .border_t_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)));
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
            cx.new(|_| super::super::transport::MiniTooltip {
                description: "add native effect".into(),
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
        .unwrap_or_else(|| format!("{} · return effects", return_track.name()));
    let header = div()
        .id("device-panel-header")
        .debug_selector(|| "device-panel-header".into())
        .h(px(DEVICE_PANEL_HEADER_HEIGHT))
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
    let mut chain = div()
        .id("device-chain-scroll")
        .debug_selector(|| "device-chain-scroll".into())
        .flex_1()
        .min_h_0()
        .flex()
        .gap(px(DEVICE_CHAIN_PADDING))
        .p(px(DEVICE_CHAIN_PADDING))
        .overflow_x_scroll()
        .overflow_y_hidden()
        .drag_over::<drag::NativeDeviceDrag>(move |style, _, _, _| {
            style.bg(rgb(theme::color(theme::PaletteRole::Hover)))
        })
        .on_drop(
            cx.listener(move |view, payload: &drag::NativeDeviceDrag, window, cx| {
                view.drop_native_device_to_return(payload, return_id, window, cx);
            }),
        );
    for (index, device) in devices.iter().enumerate() {
        chain = chain.child(render_return_effect(view, cx, &devices, index, device));
    }
    chain = chain.child(
        div()
            .min_w(px(40.0))
            .h(px(DEVICE_FRAME_HEIGHT))
            .flex_shrink_0(),
    );
    root = root.child(header).child(chain);
    if catalog.is_err() || add_open {
        root = root.child(render_return_add_menu(cx, return_id, catalog));
    }
    root
}

fn render_return_effect(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    devices: &[hex_project::Device],
    index: usize,
    device: &hex_project::Device,
) -> Stateful<Div> {
    let id = device.id();
    let device_view = view
        .device_panel
        .views
        .remove(&id)
        .filter(|(class, cached, _)| {
            *class == NativeDeviceClass::Effect && cached == device.state()
        })
        .map(|(_, _, result)| result)
        .unwrap_or_else(|| {
            hex_engine::native_device_view_for_class(NativeDeviceClass::Effect, device.state())
                .map_err(|error| error.to_string().to_lowercase())
        });
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
    if let Some(before) = before_up {
        actions = actions.child(
            action_button(
                format!("return-device-up-{}", id.value()),
                "‹",
                "move effect left",
                theme::color(theme::PaletteRole::Text),
            )
            .on_click(cx.listener(move |view, _, _, cx| {
                if finalize_controls(view, cx) {
                    view.enqueue_device_barrier(Request::Edit(Command::MoveReturnDevice {
                        device: id,
                        before: Some(before),
                    }));
                }
                cx.notify();
            })),
        );
    }
    if has_down {
        actions = actions.child(
            action_button(
                format!("return-device-down-{}", id.value()),
                "›",
                "move effect right",
                theme::color(theme::PaletteRole::Text),
            )
            .on_click(cx.listener(move |view, _, _, cx| {
                if finalize_controls(view, cx) {
                    view.enqueue_device_barrier(Request::Edit(Command::MoveReturnDevice {
                        device: id,
                        before: after_down,
                    }));
                }
                cx.notify();
            })),
        );
    }
    actions = actions.child(
        action_button(
            format!("return-device-remove-{}", id.value()),
            "×",
            "remove effect",
            theme::color(theme::PaletteRole::Primary),
        )
        .on_click(cx.listener(move |view, _, _, cx| {
            if finalize_controls(view, cx) {
                view.enqueue_device_barrier(Request::Edit(Command::RemoveDevice(id)));
            }
            cx.notify();
        })),
    );
    let rendered = match &device_view {
        Ok(device_view) => {
            let mut body = div()
                .id(ElementId::Name(
                    format!("device-body-{}", id.value()).into(),
                ))
                .debug_selector(move || format!("device-body-{}", id.value()))
                .w_full()
                .h(px(DEVICE_FRAME_BODY_HEIGHT))
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_wrap()
                .content_start()
                .gap(px(2.0))
                .p(px(3.0));
            for parameter in &device_view.parameters {
                body = body.child(parameter_control(
                    view,
                    cx,
                    id,
                    DeviceControlIdentity {
                        class: NativeDeviceClass::Effect,
                        type_id: device_view.type_id,
                    },
                    parameter,
                ));
            }
            device_frame(
                theme::text(device_view.name.clone()),
                control_style(),
                actions,
                body,
            )
            .into_any_element()
        }
        Err(error) => device_frame(
            theme::text(
                device
                    .state()
                    .type_id()
                    .as_str()
                    .unwrap_or("unknown device"),
            ),
            control_style(),
            actions,
            div()
                .h(px(DEVICE_FRAME_BODY_HEIGHT))
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Primary)))
                .child(theme::text(format!("unsupported device state · {error}"))),
        )
        .into_any_element(),
    };
    view.device_panel.views.insert(
        id,
        (
            NativeDeviceClass::Effect,
            device.state().clone(),
            device_view,
        ),
    );
    div()
        .id(ElementId::Name(
            format!("effect-device-{}", id.value()).into(),
        ))
        .debug_selector(move || format!("effect-device-{}", id.value()))
        .w(px(DEVICE_FRAME_WIDTH))
        .h(px(DEVICE_FRAME_HEIGHT))
        .flex_shrink_0()
        .child(rendered)
}

fn render_return_add_menu(
    cx: &mut Context<EditorView>,
    return_track: hex_project::ReturnId,
    effects: Result<Vec<hex_engine::NativeDeviceCatalogEntry>, String>,
) -> impl IntoElement {
    let mut menu = div()
        .id("device-add-menu")
        .debug_selector(|| "device-add-menu".into())
        .absolute()
        .top(px(DEVICE_PANEL_HEADER_HEIGHT))
        .right(px(4.0))
        .h(px(23.0))
        .flex()
        .items_center()
        .gap(px(3.0))
        .px(px(4.0))
        .bg(rgb(theme::color(theme::PaletteRole::Raised)));
    match effects {
        Ok(entries) if !entries.is_empty() => {
            for entry in entries {
                let type_id = entry.type_id;
                let label = format!("effect · {}", entry.name);
                let id = format!(
                    "add-return-effect-{}",
                    type_id.as_str().unwrap_or("unknown")
                );
                menu = menu.child(
                    div()
                        .id(ElementId::Name(id.clone().into()))
                        .debug_selector(move || id.clone())
                        .h(px(18.0))
                        .px(px(5.0))
                        .flex()
                        .items_center()
                        .text_xs()
                        .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                        .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Hover))))
                        .cursor(gpui::CursorStyle::PointingHand)
                        .child(theme::text(label))
                        .on_click(cx.listener(move |view, _, _, cx| {
                            let request = match hex_engine::default_native_device_state_for_class(
                                NativeDeviceClass::Effect,
                                type_id,
                            ) {
                                Ok(state) => Request::Edit(Command::InsertReturnDevice {
                                    return_track,
                                    before: None,
                                    state,
                                }),
                                Err(error) => {
                                    view.device_panel.message =
                                        Some(error.to_string().to_lowercase());
                                    cx.notify();
                                    return;
                                }
                            };
                            if finalize_controls(view, cx) && view.enqueue_device_barrier(request) {
                                view.device_panel.add_menu_open = false;
                                view.device_panel.message = None;
                            }
                            cx.notify();
                        })),
                );
            }
        }
        Ok(_) => menu = menu.child(menu_message("no native effects available")),
        Err(error) => menu = menu.child(menu_message(&format!("device catalog error · {error}"))),
    }
    menu
}
