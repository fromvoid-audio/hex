//! Source-stage Utility modulator card rendering.
use super::*;
use crate::editor::device_comparison;

pub(in crate::editor) const SOURCE_EFFECT_CARD_WIDTH: f32 = 172.0;

/// Static Utility matrix controls share the verified source handoff.
pub(in crate::editor) fn source_utility_gain_available(
    project: &hex_project::Project,
    device: &Device,
) -> bool {
    hex_engine::project_source_utility_matrix_edit_supported(
        project,
        device.id(),
        utility_panel::GAIN,
    )
}

/// The bound receiver owns these controls. The card reuses the ordinary
/// modulator housing and deliberately has no track-chain drop geometry.
pub(in crate::editor) fn source_effect_card(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: &Device,
) -> Stateful<Div> {
    let id = device.id();
    let selector = format!("source-effect-modulation-{}", id.value());
    let measured = view.device_panel.chain_window.measure_source_effect(id);
    let entity = cx.entity();
    let outer = div()
        .on_children_prepainted(move |children, _, cx| {
            let width = children.first().map_or(SOURCE_EFFECT_CARD_WIDTH, |frame| {
                f32::from(frame.size.width).max(SOURCE_EFFECT_CARD_WIDTH)
            });
            if width.is_finite() && measured.replace(width) != width {
                let entity = entity.clone();
                cx.defer(move |cx| entity.update(cx, |_, cx| cx.notify()));
            }
        })
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .min_w(px(SOURCE_EFFECT_CARD_WIDTH))
        .h(px(DEVICE_FRAME_HEIGHT))
        .flex()
        .flex_col()
        .flex_shrink_0();
    let Some(state) = view.state.as_ref() else {
        return outer;
    };
    let Some(mut metadata) = view
        .device_panel
        .views
        .get(&id)
        .and_then(|(_, _, metadata)| metadata.as_ref().ok())
        .cloned()
    else {
        return outer.child(theme::text("source effect unavailable"));
    };
    // The housing owns scrolling; let this compact card contribute its full
    // content height so its lower mapping targets are not clipped.
    let mut body = hex_widgets::device_body(theme::device_style(theme::DeviceStyleRole::Layout))
        .w(px(SOURCE_EFFECT_CARD_WIDTH
            - hex_widgets::DEVICE_FRAME_HEADER_WIDTH
            - 2.0 * hex_widgets::DEVICE_FRAME_BORDER_WIDTH))
        .flex()
        .flex_col()
        .gap(px(1.0));
    body = body.child(theme::text("source effect"));
    let mode_parameter = utility_panel::MID_SIDE_ENABLED;
    let mode_eligible = hex_engine::project_source_utility_matrix_edit_supported(
        &state.project,
        id,
        mode_parameter,
    );
    let pending_mode = view.device_panel.gesture.snapshot().and_then(|snapshot| {
        match (snapshot.target, snapshot.value) {
            (GestureTarget::DeviceParameter { device, parameter }, ParameterValue::Bool(value))
                if device == id && parameter == mode_parameter =>
            {
                Some(value)
            }
            _ => None,
        }
    });
    let mode_row = mode_eligible
        .then(|| {
            metadata
                .parameters
                .iter()
                .find(|row| row.enabled && row.descriptor.id == mode_parameter)
                .copied()
        })
        .flatten();
    let mono_row = hex_engine::project_source_utility_matrix_edit_supported(
        &state.project,
        id,
        utility_panel::MONO,
    )
    .then(|| {
        metadata
            .parameters
            .iter()
            .find(|row| row.enabled && row.descriptor.id == utility_panel::MONO)
            .copied()
    })
    .flatten();
    let polarity_rows =
        [utility_panel::INVERT_LEFT, utility_panel::INVERT_RIGHT].map(|parameter| {
            (metadata.type_id == hex_lattice_contract::DeviceTypeId::from_static("hex.utility"))
                .then(|| {
                    metadata
                        .parameters
                        .iter()
                        .find(|row| row.descriptor.id == parameter)
                        .copied()
                })
                .flatten()
                .map(|mut row| {
                    row.enabled &= hex_engine::project_source_utility_matrix_edit_supported(
                        &state.project,
                        id,
                        parameter,
                    );
                    row
                })
        });
    let mute_row = (metadata.type_id
        == hex_lattice_contract::DeviceTypeId::from_static("hex.utility"))
    .then(|| {
        metadata
            .parameters
            .iter()
            .find(|row| row.descriptor.id == utility_panel::MUTE)
            .copied()
    })
    .flatten()
    .map(|mut row| {
        row.enabled &= hex_engine::project_source_utility_mute_edit_supported(&state.project, id);
        row
    });
    let channel_row = (metadata.type_id
        == hex_lattice_contract::DeviceTypeId::from_static("hex.utility"))
    .then(|| {
        metadata
            .parameters
            .iter()
            .find(|row| row.descriptor.id == utility_panel::CHANNEL)
            .copied()
    })
    .flatten()
    .map(|mut row| {
        row.enabled &= hex_engine::project_source_utility_matrix_edit_supported(
            &state.project,
            id,
            utility_panel::CHANNEL,
        );
        row
    });
    let controls = [
        (utility_panel::GAIN, "gain", None),
        (
            utility_panel::BALANCE,
            "balance",
            Some(numeric_display::NumericDisplayCodec::Balance),
        ),
        (
            utility_panel::WIDTH,
            "width",
            Some(numeric_display::NumericDisplayCodec::Percent),
        ),
        (
            utility_panel::MID_SIDE_BALANCE,
            "mid/side",
            Some(numeric_display::NumericDisplayCodec::MidSide),
        ),
    ]
    .map(|(parameter, label, codec)| {
        let is_mode_value = matches!(
            parameter,
            utility_panel::WIDTH | utility_panel::MID_SIDE_BALANCE
        );
        let selected_pending = pending_mode.is_some_and(|mid_side| {
            is_mode_value && mid_side == (parameter == utility_panel::MID_SIDE_BALANCE)
        });
        if is_mode_value && pending_mode.is_some() && !selected_pending {
            return None;
        }
        if !selected_pending
            && !hex_engine::project_source_utility_matrix_edit_supported(
                &state.project,
                id,
                parameter,
            )
        {
            return None;
        }
        metadata
            .parameters
            .iter()
            .find(|row| row.enabled && row.descriptor.id == parameter)
            .copied()
            .map(|mut row| {
                if selected_pending {
                    row.enabled = false;
                }
                (row, label, codec)
            })
    });
    metadata.parameters.retain(|row| {
        row.enabled
            && row
                .semantics
                .is_some_and(|semantics| semantics.modulation == ParameterModulation::SampleRate)
            && hex_engine::project_device_modulation_target_supported(
                &state.project,
                id,
                row.descriptor.id,
            )
    });
    let mut control_row = div().flex().flex_wrap().w_full().gap(px(2.0));
    for (row, label, codec) in controls.iter().flatten() {
        let identity = DeviceControlIdentity {
            class: NativeDeviceClass::Effect,
            type_id: metadata.type_id,
        };
        control_row = control_row.child(parameter_control_with_presentation(
            view,
            cx,
            id,
            identity,
            row,
            ControlPresentation::CompactDial,
            ControlText {
                label: Some(label),
                codec: *codec,
            },
        ));
    }
    if let Some(row) = mode_row {
        let identity = DeviceControlIdentity {
            class: NativeDeviceClass::Effect,
            type_id: metadata.type_id,
        };
        control_row = control_row.child(parameter_control_with_presentation(
            view,
            cx,
            id,
            identity,
            &row,
            ControlPresentation::CompactButton,
            ControlText {
                label: Some("mid/side"),
                codec: None,
            },
        ));
    }
    if let Some(row) = mono_row {
        control_row = control_row.child(parameter_control_with_presentation(
            view,
            cx,
            id,
            DeviceControlIdentity {
                class: NativeDeviceClass::Effect,
                type_id: metadata.type_id,
            },
            &row,
            ControlPresentation::CompactButton,
            ControlText {
                label: Some("mono"),
                codec: None,
            },
        ));
    }
    if let Some(row) = mute_row {
        control_row = control_row.child(parameter_control_with_presentation(
            view,
            cx,
            id,
            DeviceControlIdentity {
                class: NativeDeviceClass::Effect,
                type_id: metadata.type_id,
            },
            &row,
            ControlPresentation::CompactButton,
            ControlText {
                label: Some("mute"),
                codec: None,
            },
        ));
    }
    if let Some(row) = channel_row {
        control_row = control_row.child(parameter_control_with_presentation(
            view,
            cx,
            id,
            DeviceControlIdentity {
                class: NativeDeviceClass::Effect,
                type_id: metadata.type_id,
            },
            &row,
            ControlPresentation::CompactSelector,
            ControlText {
                label: None,
                codec: None,
            },
        ));
    }
    for (row, label) in polarity_rows.iter().zip(["ø l", "ø r"]) {
        if let Some(row) = row {
            control_row = control_row.child(parameter_control_with_presentation(
                view,
                cx,
                id,
                DeviceControlIdentity {
                    class: NativeDeviceClass::Effect,
                    type_id: metadata.type_id,
                },
                row,
                ControlPresentation::CompactButton,
                ControlText {
                    label: Some(label),
                    codec: None,
                },
            ));
        }
    }
    if controls.iter().any(Option::is_some)
        || mode_row.is_some()
        || mono_row.is_some()
        || mute_row.is_some()
        || channel_row.is_some()
        || polarity_rows.iter().any(Option::is_some)
    {
        body = body.child(control_row);
    }
    for row in &metadata.parameters {
        if controls.iter().all(Option::is_none) {
            body = body.child(
                div()
                    .relative()
                    .px(px(4.0))
                    .h(px(15.0))
                    .child(theme::text(row.descriptor.label))
                    .children(target_overlay(view, cx, id, row)),
            );
        }
    }
    let modulation = panel(view, cx, device, &metadata, true);
    let selected = view.device_panel.is_selected(id);
    let class = NativeDeviceClass::Effect;
    let focus = view.device_panel.chain_window.focus(id, cx);
    let rendered = hex_widgets::device_frame_with_modulators(
        device_comparison::source_title(id, metadata.name, cx),
        theme::device_frame_style(selected),
        bypass_action(view, device, class, cx),
        device_comparison::actions(view, device, class, selected, cx),
        body,
        modulation,
    )
    .h_full();
    let card = outer.track_focus(&focus).child(rendered);
    let card = keyboard::activation_card(view, cx, id, class, focus.clone(), card);
    device_comparison::card(view, cx, id, class, focus, card)
}
