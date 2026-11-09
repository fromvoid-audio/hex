//! Return-owned effect rail, reusing the shared device descriptors and gestures.
use super::*;

pub(super) fn render(
    view: &mut EditorView,
    window: Option<&Window>,
    cx: &mut Context<EditorView>,
    state: &ViewState,
    owner: EffectBusOwner,
) -> Stateful<Div> {
    let Some(devices) = owner.devices(&state.project).map(<[_]>::to_vec) else {
        return empty_panel("effect target is no longer available");
    };
    let alive: HashSet<_> = devices.iter().map(hex_project::Device::id).collect();
    view.device_panel.reconcile_selection(&alive);
    let stale_keys: Vec<_> = view
        .device_panel
        .controls
        .iter()
        .filter(|(key, control)| {
            devices
                .iter()
                .find(|device| device.id() == key.device)
                .is_none_or(|device| {
                    !parameter_control_matches_identity(
                        control,
                        DeviceControlIdentity {
                            class: NativeDeviceClass::Effect,
                            type_id: device.state().type_id(),
                        },
                    )
                })
        })
        .map(|(key, _)| *key)
        .collect();
    let changed_types: HashSet<_> = view
        .device_panel
        .views
        .iter()
        .filter_map(|(id, (_, cached, _))| {
            devices
                .iter()
                .find(|device| device.id() == *id)
                .filter(|device| device.state().type_id() != cached.type_id())
                .map(|_| *id)
        })
        .collect();
    let active_target = view
        .device_gesture
        .snapshot()
        .map(|snapshot| snapshot.target);
    super::snapshot::retire_invalid_owner(view, &alive);
    let stale_target_has_work = active_target.is_some_and(|target| match target {
        GestureTarget::DeviceParameter { device, .. } => !alive.contains(&device),
        GestureTarget::DeviceParameterPair { device, .. } => !alive.contains(&device),
        GestureTarget::DeviceParameterSnapshot { device }
        | GestureTarget::PluginParameter { device, .. }
        | GestureTarget::EqNodeCreation { device } => !alive.contains(&device),
        GestureTarget::ReturnSend { .. }
        | GestureTarget::TrackGain { .. }
        | GestureTarget::TrackGains { .. }
        | GestureTarget::TrackBalance { .. }
        | GestureTarget::MainGain
        | GestureTarget::ReturnGain { .. } => false,
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
    for device in &devices {
        cache_device_metadata(&mut view.device_panel, device, NativeDeviceClass::Effect);
    }
    parameter_rows::reconcile(view, cx);
    view.device_panel
        .meter_states
        .retain(|id, _| alive.contains(id));
    retire_transfer_previews(&mut view.device_panel, |id| {
        devices
            .iter()
            .find(|device| device.id() == id)
            .map(|device| device.state().type_id())
    });
    // The shared retirement helper owns both legacy and current EQ graphs.
    let alive_eq8: HashSet<_> = devices
        .iter()
        .filter(|device| matches!(device.state().type_id().as_str(), Ok("hex.eq8" | "hex.eq")))
        .map(hex_project::Device::id)
        .collect();
    retire_removed_eq8_graphs(view, cx, &alive_eq8);
    view.device_panel
        .eq8_panels
        .retain(|device, _| alive_eq8.contains(device));
    view.device_panel
        .eq8_audition_toggles
        .retain(|device, _| alive_eq8.contains(device));
    view.device_panel
        .eq8_responses
        .retain(|device, _| alive_eq8.contains(device));
    view.device_panel
        .eq8_graph_gestures
        .retain(|device, _| alive_eq8.contains(device));
    if let Some(audio) = view.audio.as_mut()
        && let Some(target) = audio.control.audition_desired()
        && target.identity.owner == owner.processor()
        && !devices.iter().any(|device| {
            device.id().value() == target.identity.device_id
                && device.state().type_id() == target.identity.device_type
                && matches!(
                    target.identity.device_type.as_str(),
                    Ok("hex.eq8" | "hex.eq")
                )
        })
    {
        let _ = audio.control.audition_cancel();
    }
    view.device_panel
        .pending_values
        .retain(|key, _| alive.contains(&key.device) && !changed_types.contains(&key.device));
    if stale_target_has_work {
        view.device_panel.message = Some("device target is no longer available".into());
    } else if view.device_panel.message.as_deref() == Some("device target is no longer available") {
        view.device_panel.message = None;
    }

    if view.device_panel.effect_catalog.is_none() {
        view.device_panel.effect_catalog = Some(
            hex_engine::native_device_catalog_for_class(NativeDeviceClass::Effect).map_err(
                |error| hex_session::service::text::lowercase_outside_verbatim(&error.to_string()),
            ),
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
                kind: crate::tooltips::Kind::Identification,
                description: "add native effect".into(),
            })
            .into()
        })
        .child(theme::text(if add_open { "×" } else { "+" }))
        .on_click(cx.listener(|view, _, window, cx| {
            view.device_panel.add_menu_target = None;
            if view.device_panel.add_menu_open {
                view.device_panel.add_menu_open = false;
                view.focus.focus(window);
            } else {
                device_picker::open(view, window, cx);
            }
            cx.notify();
        }));
    let title = view
        .device_panel
        .message
        .clone()
        .unwrap_or_else(|| owner.title(&state.project).unwrap_or_default());
    let track_name_view = title_label(title, view.device_panel.message.is_some());
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
        .child(track_name_view)
        .child(add_button);
    let chain = super::return_drag::render_chain(view, window, cx, owner, &devices);
    let chain = navigation::rail(view, chain);
    root = root.child(header).child(chain);
    if catalog.is_err() {
        root = root.child(menu_message("device catalog unavailable"));
    }
    root
}

pub(super) fn render_return_effect(
    view: &mut EditorView,
    window: Option<&Window>,
    cx: &mut Context<EditorView>,
    owner: EffectBusOwner,
    device: &hex_project::Device,
) -> Stateful<Div> {
    let id = device.id();
    let selected = view.device_panel.is_selected(id);
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
                .map_err(|error| {
                    hex_session::service::text::lowercase_outside_verbatim(&error.to_string())
                })
        });
    let leading = bypass_action(view, device, NativeDeviceClass::Effect, cx);
    let mut actions = div().flex().flex_col().items_center().gap(px(2.0));
    if device_view.is_ok() {
        actions = actions
            .child(super::super::device_preset::actions(
                view,
                id,
                NativeDeviceClass::Effect,
                selected,
                cx,
            ))
            .child(super::super::device_comparison::actions(
                view,
                device,
                NativeDeviceClass::Effect,
                selected,
                cx,
            ));
    }
    if device.state().type_id().as_str() == Ok("hex.vst3")
        && !view.degraded_plugins.contains(id.value())
    {
        actions = actions.child(vst3_editor_button(id, selected, cx));
    }
    actions = actions.child(
        action_button(
            format!("return-device-remove-{}", id.value()),
            "×",
            "remove effect",
            theme::DeviceActionRole::Destructive,
            selected,
        )
        .on_click(cx.listener(move |view, _, window, cx| {
            if finalize_controls(view, cx)
                && view.enqueue_device_barrier(Request::Edit(Command::RemoveDevice(id)))
            {
                keyboard::after_pointer_removal(view, id, window, cx);
            }
            cx.notify();
        })),
    );
    let rendered = match &device_view {
        _ if device.state().type_id().as_str() == Ok("hex.vst3") => {
            let body = vst3_status_body(view, device, cx);
            hex_widgets::device_frame_with_modulators(
                device_drag_title(
                    view,
                    cx,
                    device,
                    NativeDeviceClass::Effect,
                    view.vst3_device_title(device),
                ),
                theme::device_frame_style(selected),
                leading,
                actions,
                body,
                super::modulators::vst3_panel(view, cx, device),
            )
        }
        Ok(device_view) => {
            let body = if device_view.type_id.as_str() == Ok("hex.utility") {
                super::utility_panel::render_body(view, cx, id, device_view)
            } else if device_view.type_id.as_str() == Ok("hex.limiter") {
                super::limiter_panel::render_body(view, cx, device, device_view)
            } else if device_view.type_id.as_str() == Ok("hex.compressor") {
                super::compressor_panel::render_body(view, cx, id, device.state(), device_view)
            } else if device_view.type_id.as_str() == Ok("hex.saturator") {
                super::saturator_panel::render_body(view, cx, id, device.state(), device_view)
            } else if device_view.type_id.as_str() == Ok("hex.reverb") {
                super::reverb_panel::render_body(view, cx, id, device_view)
            } else if device_view.type_id.as_str() == Ok("hex.delay") {
                super::delay_panel::render_body(view, cx, id, device.state(), device_view)
            } else if device_view.type_id.as_str() == Ok("hex.overdrive") {
                super::overdrive_panel::render_body(view, cx, id, device.state(), device_view)
            } else if device_view.type_id.as_str() == Ok("hex.rattle") {
                super::rattle_panel::render_body(view, cx, device, device_view)
            } else if device_view.type_id.as_str() == Ok("hex.gate") {
                super::gate_panel::render_body(view, cx, id, device_view)
            } else if device_view.type_id.as_str() == Ok("hex.auto-pan") {
                super::auto_pan_panel::render_body(view, cx, id, device_view)
            } else if device_view.type_id.as_str() == Ok("hex.eq") {
                let sample_rate = view
                    .state
                    .as_ref()
                    .map_or(48000, |state| state.project.sample_rate());
                super::eq_panel::render_body(
                    view,
                    cx,
                    id,
                    owner.processor(),
                    device.state(),
                    device_view,
                    sample_rate,
                )
            } else if device_view.type_id.as_str() == Ok("hex.eq8") {
                super::eq8_panel::render_body(
                    view,
                    cx,
                    id,
                    owner.processor(),
                    device.state(),
                    device_view,
                    view.state
                        .as_ref()
                        .map_or(48_000, |state| state.project.sample_rate()),
                )
            } else {
                parameter_rows::body(
                    view,
                    window,
                    cx,
                    id,
                    DeviceControlIdentity {
                        class: NativeDeviceClass::Effect,
                        type_id: device_view.type_id,
                    },
                    device_view,
                )
            };
            hex_widgets::device_frame_with_modulators(
                device_drag_title(
                    view,
                    cx,
                    device,
                    NativeDeviceClass::Effect,
                    theme::text(device_view.name.clone()),
                ),
                theme::device_frame_style(selected),
                leading,
                actions,
                body,
                super::modulators::panel(view, cx, device, device_view, true),
            )
        }
        Err(error) => {
            let title = theme::text(
                device
                    .state()
                    .type_id()
                    .as_str()
                    .unwrap_or("unknown device"),
            );
            let body = crate::device_composition::unsupported_body(theme::text(format!(
                "unsupported device state · {error}"
            )))
            .id(ElementId::Name(
                format!("device-body-{}", id.value()).into(),
            ))
            .debug_selector(move || format!("device-body-{}", id.value()));
            device_frame(
                device_drag_title(view, cx, device, NativeDeviceClass::Effect, title),
                theme::device_frame_style(selected),
                leading,
                actions,
                body,
            )
        }
    };
    view.device_panel.views.insert(
        id,
        (
            NativeDeviceClass::Effect,
            device.state().clone(),
            device_view,
        ),
    );
    let card_width = device_card_width(view, NativeDeviceClass::Effect, device);
    let rendered = rendered
        .h_full()
        .debug_selector(move || format!("device-shell-{}", id.value()));
    let rendered = div()
        .w(px(card_width - super::device_meter::WIDTH))
        .flex_shrink_0()
        .child(rendered);
    div()
        .id(ElementId::Name(
            format!("effect-device-{}", id.value()).into(),
        ))
        .debug_selector(move || format!("effect-device-{}", id.value()))
        .w(px(card_width))
        .h(px(DEVICE_FRAME_HEIGHT))
        .flex()
        .flex_shrink_0()
        .capture_any_mouse_down(device_selection_listener(
            id,
            view.project_generation,
            view.device_panel.owner,
            cx,
        ))
        .child(rendered)
        .child(super::device_meter::render(view, device, cx))
}
