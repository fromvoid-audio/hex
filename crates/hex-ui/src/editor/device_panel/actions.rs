//! Device header action buttons and parameter control builders.
use super::*;

pub(in crate::editor) fn instrument_actions(
    view: &EditorView,
    track: TrackId,
    device: &hex_project::Device,
    selected: bool,
    presets_available: bool,
    cx: &mut Context<EditorView>,
) -> Div {
    let id = device.id();
    let actions = div().flex().flex_col().items_center().gap(px(2.0));
    let actions = if presets_available {
        let content = crate::editor::device_preset::actions(
            view,
            id,
            NativeDeviceClass::Instrument,
            selected,
            cx,
        );
        actions.child(content)
    } else {
        actions
    };
    let actions = if device.state().type_id().as_str() == Ok("hex.vst3")
        && !view.device_panel.degraded_plugins.contains(id.value())
    {
        actions.child(vst3_editor_button(view, id, selected, cx))
    } else {
        actions
    };
    let actions = actions.child(
        action_button(
            format!("instrument-replace-{}", id.value()),
            "↻",
            "replace instrument",
            theme::DeviceActionRole::Normal,
            selected,
        )
        .on_click(cx.listener(|view, _, window, cx| {
            view.device_panel.add_menu_target = None;
            device_picker::open(view, window, cx);
        })),
    );
    let actions = if presets_available {
        actions.child(super::device_comparison::actions(
            view,
            device,
            NativeDeviceClass::Instrument,
            selected,
            cx,
        ))
    } else {
        actions
    };
    actions.child(
        action_button(
            format!("instrument-remove-{}", id.value()),
            "×",
            "remove instrument",
            theme::DeviceActionRole::Destructive,
            selected,
        )
        .on_click(cx.listener(move |view, _, window, cx| {
            if finalize_controls(view, cx)
                && view.enqueue_device_barrier(Request::Edit(Command::RemoveInstrument { track }))
            {
                keyboard::after_pointer_removal(view, id, window, cx);
            }
            cx.notify();
        })),
    )
}

pub(in crate::editor) fn bypass_action(
    view: &EditorView,
    device: &hex_project::Device,
    class: NativeDeviceClass,
    cx: &mut Context<EditorView>,
) -> Stateful<Div> {
    let id = device.id();
    let bypassed = view.device_panel.bypass.displayed(id, device.bypassed());
    let (on_color, off_color) = theme::device_enable_colors();
    let tooltip = if class == NativeDeviceClass::Instrument {
        "toggle instrument bypass"
    } else {
        "toggle device bypass"
    };
    hex_widgets::device_state_indicator(
        format!("device-bypass-{}", id.value()).into(),
        !bypassed,
        on_color,
        off_color,
    )
    .tooltip(move |_, cx| {
        cx.new(|_| crate::editor::transport::MiniTooltip {
            kind: crate::tooltips::Kind::Identification,
            description: tooltip.into(),
        })
        .into()
    })
    .on_click(cx.listener(move |view, _, _, cx| {
        if finalize_controls(view, cx) {
            view.toggle_device_bypass(id);
        }
        cx.notify();
    }))
}

pub(in crate::editor) fn device_actions(
    view: &EditorView,
    device: &hex_project::Device,
    selected: bool,
    presets_available: bool,
    cx: &mut Context<EditorView>,
) -> Div {
    let id = device.id();
    let mut actions = div().flex().flex_col().items_center().gap(px(2.0));
    if presets_available {
        let content = crate::editor::device_preset::actions(
            view,
            id,
            NativeDeviceClass::Effect,
            selected,
            cx,
        );
        actions = actions.child(content);
    }
    if device.state().type_id().as_str() == Ok("hex.vst3")
        && !view.device_panel.degraded_plugins.contains(id.value())
    {
        actions = actions.child(vst3_editor_button(view, id, selected, cx));
    }
    if presets_available {
        actions = actions.child(super::device_comparison::actions(
            view,
            device,
            NativeDeviceClass::Effect,
            selected,
            cx,
        ));
    }
    actions.child(
        action_button(
            format!("device-remove-{}", id.value()),
            "×",
            "remove device",
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
    )
}

pub(in crate::editor) fn vst3_editor_button(
    view: &EditorView,
    id: DeviceId,
    selected: bool,
    cx: &mut Context<EditorView>,
) -> Stateful<Div> {
    let open = view
        .device_panel
        .plugin_editor_windows
        .contains(&id.value());
    action_button(
        format!("device-vst3-editor-{}", id.value()),
        "↗",
        if open {
            "hide plugin editor"
        } else {
            "show plugin editor"
        },
        if open {
            theme::DeviceActionRole::Enabled
        } else {
            theme::DeviceActionRole::Normal
        },
        selected,
    )
    .on_click(cx.listener(move |view, _, _, cx| {
        view.enqueue(Request::TogglePluginEditor {
            project_generation: view.project_generation,
            device: id,
        });
        cx.notify();
    }))
}

pub(in crate::editor) fn action_button(
    id: String,
    icon: &'static str,
    tooltip: &'static str,
    role: theme::DeviceActionRole,
    selected: bool,
) -> Stateful<Div> {
    button_details(
        hex_widgets::device_action(
            theme::text(icon),
            theme::device_header_action_style(selected, role),
        ),
        id,
        tooltip,
    )
}

/// Latched header/toolbar settings retain segment artwork, distinct from commands.
pub(in crate::editor) fn segment_button(
    id: String,
    label: &'static str,
    tooltip: &'static str,
    role: theme::DeviceActionRole,
    selected: bool,
) -> Stateful<Div> {
    button_details(
        hex_widgets::device_segment(
            theme::text(label),
            theme::device_header_action_style(selected, role),
        ),
        id,
        tooltip,
    )
}

pub(in crate::editor) fn button_details(
    surface: Div,
    id: String,
    tooltip: &'static str,
) -> Stateful<Div> {
    surface
        .id(ElementId::Name(id.clone().into()))
        .debug_selector(move || id.clone())
        .cursor(gpui::CursorStyle::PointingHand)
        .tooltip(move |_, cx| {
            cx.new(|_| crate::editor::transport::MiniTooltip {
                kind: crate::tooltips::Kind::Identification,
                description: tooltip.into(),
            })
            .into()
        })
}

// Host-specific composition policy stays outside the shared bindings.
pub(in crate::editor) fn binding_policy(
    _view: &EditorView,
    _key: ParameterKey,
    identity: DeviceControlIdentity,
    descriptor: ParameterDescriptor,
    presentation: ControlPresentation,
    display: ControlText,
) -> bindings::BindingPolicy {
    let tab_navigation = identity.type_id.as_str() == Ok("hex.instrument.operator");
    let toggle_appearance = if presentation == ControlPresentation::CompactButton {
        match display.label {
            Some("") => ToggleAppearance::MicroSquare,
            Some("bass mono audition") => ToggleAppearance::CompactIconButton,
            _ => ToggleAppearance::CompactButton,
        }
    } else {
        ToggleAppearance::Standard
    };
    let choice_order = match descriptor.kind {
        ParameterKind::Choice { labels } => Some(operator_panel::choice_order(
            identity,
            descriptor.id,
            labels.len(),
        )),
        _ => None,
    };
    bindings::BindingPolicy {
        tab_navigation,
        toggle_appearance,
        choice_order,
    }
}

/// Bind the active Arrangement trim on the persistent device toolbar.
pub(in crate::editor) fn parameter_control(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameter: &NativeDeviceParameterView,
) -> Div {
    parameter_control_with_presentation(
        view,
        cx,
        device,
        identity,
        parameter,
        ControlPresentation::Standard,
        ControlText::default(),
    )
}

pub(in crate::editor) fn parameter_control_with_presentation(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameter: &NativeDeviceParameterView,
    presentation: ControlPresentation,
    display: ControlText,
) -> Div {
    parameter_control_with_label(
        view,
        cx,
        device,
        identity,
        parameter,
        presentation,
        display,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(in crate::editor) fn parameter_control_with_label(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameter: &NativeDeviceParameterView,
    presentation: ControlPresentation,
    display: ControlText,
    label_override: Option<gpui::SharedString>,
) -> Div {
    let label_for_width = label_override
        .as_ref()
        .map(|s| s.as_ref())
        .or(display.label);
    let compact_toggle_width = match label_for_width {
        Some("bass mono" | "mono" | "maximize" | "auto" | "mid/side") => 64.0,
        Some("bass mono audition" | "r" | "q") => 14.0,
        Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "on" | "fix" | "") => 18.0,
        Some("ø l" | "ø r" | "mute" | "dc") => 31.0,
        _ => 40.0,
    };
    let control = bindings::render_parameter(
        view,
        cx,
        device,
        identity,
        parameter,
        bindings::BindingPresentation {
            kind: presentation,
            text: display,
            label_override,
            policy: binding_policy,
            compact_toggle_width,
        },
    );
    if let Some(element) = modulators::target_overlay(view, cx, device, parameter) {
        control.relative().child(element)
    } else {
        control
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_panel_action_constants_remain_positive() {
        const { assert!(DEVICE_PANEL_HEIGHT > DEVICE_FRAME_HEIGHT) };
        const { assert!(DEVICE_FRAME_WIDTH > 0.0) };
    }
}
