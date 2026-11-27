//! Modulator parameter target overlays and destination helpers.
use super::*;

pub(in crate::editor) fn target_overlay(
    view: &EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    target: &NativeDeviceParameterView,
) -> Option<AnyElement> {
    let (_, source) = view
        .device_panel
        .mapping_source
        .filter(|(owner, _)| *owner == device)?;
    let state = view.state.as_ref()?;
    let bank = state.project.device(device)?.modulation();
    let parameter = target.descriptor.id;
    if !target.enabled
        || !target
            .semantics
            .is_some_and(|semantics| semantics.modulation == ParameterModulation::SampleRate)
        || !bank.sources().iter().any(|row| source.valid_for(row))
    {
        return None;
    }
    let assigned = bank.routes().iter().find(|route| {
        source.owns(route)
            && matches!(
                route.destination,
                ModulationDestination::Device { target: DeviceAutomationTarget::Parameter(id), .. }
                    if id == parameter
            )
    });
    if assigned.is_none()
        && (bank.routes().len() >= hex_project::MAX_MODULATION_ROUTES_PER_DEVICE
            || (is_pressure_source(bank.sources(), source.source)
                && (!hex_engine::fm4_note_pressure_parameter_supported(parameter)
                    || bank.routes().iter().any(|route| source.owns(route)))))
    {
        return None;
    }
    let accent_color = source_accent_color(bank, source.source);
    let target = target_drag::Target {
        device,
        source,
        mapping: MappingTarget::Device(parameter),
    };
    Some(mapped_target_overlay(
        view,
        cx,
        target,
        assigned,
        MappedTargetOverlaySpec {
            revision: state.revision,
            accent_color,
            selector: format!("map-target-{}-{}", device.value(), parameter.value()),
            depth_selector: format!("map-depth-{}-{}", device.value(), parameter.value()),
        },
    ))
}

pub(in crate::editor) fn vst3_target_overlay(
    view: &EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    row: &hex_session::service::PluginParameterRow,
    control: &'static str,
) -> Option<AnyElement> {
    let (_, source) = view
        .device_panel
        .mapping_source
        .filter(|(owner, _)| *owner == device)?;
    let mapping = vst3_parameters::mapping_target(view, device, row)?;
    let state = view.state.as_ref()?;
    let bank = state.project.device(device)?.modulation();
    if !bank.sources().iter().any(|slot| source.valid_for(slot))
        || is_pressure_source(bank.sources(), source.source)
    {
        return None;
    }
    let assigned = bank.routes().iter().find(|route| {
        source.owns(route)
            && matches!(
                route.destination,
                ModulationDestination::Device {
                    target: DeviceAutomationTarget::Vst3 { parameter }, vst3_class: Some(class), ..
                } if parameter == mapping.parameter && class == mapping.class
            )
    });
    if assigned.is_none() && bank.routes().len() >= hex_project::MAX_MODULATION_ROUTES_PER_DEVICE {
        return None;
    }
    let accent_color = source_accent_color(bank, source.source);
    Some(mapped_target_overlay(
        view,
        cx,
        target_drag::Target {
            device,
            source,
            mapping: MappingTarget::Vst3(mapping),
        },
        assigned,
        MappedTargetOverlaySpec {
            revision: state.revision,
            accent_color,
            selector: format!("map-vst3-{control}-{}-{}", device.value(), row.id),
            depth_selector: format!("map-vst3-{control}-depth-{}-{}", device.value(), row.id),
        },
    ))
}

pub(in crate::editor) struct MappedTargetOverlaySpec {
    pub(super) revision: u64,
    pub(super) accent_color: u32,
    pub(super) selector: String,
    pub(super) depth_selector: String,
}

pub(in crate::editor) fn mapped_target_overlay(
    view: &EditorView,
    cx: &mut Context<EditorView>,
    target: target_drag::Target,
    assigned: Option<&DeviceModulationRoute>,
    spec: MappedTargetOverlaySpec,
) -> AnyElement {
    let MappedTargetOverlaySpec {
        revision,
        accent_color,
        selector,
        depth_selector,
    } = spec;
    let display = target_drag::displayed_depth(view, target, assigned);
    let control = div()
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .absolute()
        .inset_0()
        .border_1()
        .border_color(rgb(accent_color))
        .children(display.map(|depth| {
            let label = depth_text(depth);
            let selector = format!("{depth_selector}-{label}");
            div()
                .absolute()
                .top_0()
                .right_0()
                .px(px(2.0))
                .text_size(px(9.0))
                .bg(rgb(theme::color(theme::PaletteRole::Background)))
                .text_color(rgb(accent_color))
                .debug_selector(move || selector.clone())
                .child(theme::text(label))
        }));
    target_drag::attach(view, cx, control, target, assigned, revision).into_any_element()
}

pub(super) fn source_accent_color(bank: &DeviceModulation, source_id: ModulationSourceId) -> u32 {
    bank.sources()
        .iter()
        .find(|slot| slot.id == source_id)
        .map_or_else(
            || theme::color(theme::PaletteRole::Primary),
            |slot| theme::element_colors(slot.color).accent,
        )
}

/// Canonical effective-value modulation summary for one device parameter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct EffectiveParameterModulation {
    /// Net signed normalized delta contributed by enabled static sources (macros/buttons).
    pub(super) static_delta: f32,
    /// Total authored modulation span from enabled dynamic sources (LFO, envelope, etc.).
    pub(super) dynamic_span: f32,
    /// Accent color of the first enabled modulating source targeting this parameter.
    pub(super) accent_color: u32,
    /// Count of enabled routes from enabled sources targeting this parameter.
    pub(super) active_routes: usize,
}

pub(super) fn static_source_output(source: &Modulator, output: ModulationOutputId) -> Option<f32> {
    match source.state.type_id() {
        manual::TYPE_ID if output == ModulationOutputId::MAIN => {
            manual::Settings::decode(&source.state)
                .ok()
                .map(|settings| settings.value.clamp(0.0, 1.0))
        }
        macro_four::TYPE_ID => {
            let index = macro_four::VALUE_PARAMETERS.iter().position(|&parameter| {
                macro_four::output_for_parameter(parameter) == Some(output)
            })?;
            macro_four::Settings::decode(&source.state)
                .ok()
                .and_then(|settings| settings.values.get(index).copied())
                .map(|v| v.clamp(0.0, 1.0))
        }
        manual_button::TYPE_ID if output == ModulationOutputId::MAIN => {
            manual_button::decode(&source.state)
                .ok()
                .map(|on| if on { 1.0 } else { 0.0 })
        }
        buttons::TYPE_ID => {
            let index = buttons::index_for_output(output)?;
            buttons::Settings::decode(&source.state)
                .ok()
                .and_then(|settings| settings.on.get(index).copied())
                .map(|on| if on { 1.0 } else { 0.0 })
        }
        _ => None,
    }
}

pub(super) fn route_static_contribution(
    source: &Modulator,
    route: &DeviceModulationRoute,
) -> Option<f32> {
    let sample = static_source_output(source, route.output)?;
    let (start, end) = if manual_button::uses_identity_range(source.state.type_id()) {
        (-1.0_f64, 1.0_f64)
    } else {
        (
            f64::from(route.range.start.get()),
            f64::from(route.range.end.get()),
        )
    };
    let half_amount = f64::from(route.amount.get()) * 0.5;
    let scale = half_amount * (end - start);
    let offset = half_amount * (end + start);
    Some((f64::from(sample) * scale + offset) as f32)
}

/// Evaluate canonical effective modulation feedback for a native parameter target
/// using the exact range-contribution law shared with `hex-lattice`.
pub(super) fn effective_parameter_modulation(
    bank: &DeviceModulation,
    parameter: ParameterId,
) -> Option<EffectiveParameterModulation> {
    let mut static_delta = 0.0_f64;
    let mut dynamic_span = 0.0_f64;
    let mut accent_color = None;
    let mut active_routes = 0usize;

    for route in bank.routes() {
        if !route.enabled {
            continue;
        }
        let matches_target = matches!(
            route.destination,
            ModulationDestination::Device {
                target: DeviceAutomationTarget::Parameter(id),
                ..
            } if id == parameter
        );
        if !matches_target {
            continue;
        }
        let Some(source) = bank.sources().iter().find(|slot| slot.id == route.source) else {
            continue;
        };
        if !source.enabled {
            continue;
        }
        active_routes += 1;
        if accent_color.is_none() {
            accent_color = Some(theme::element_colors(source.color).accent);
        }
        if let Some(delta) = route_static_contribution(source, route) {
            static_delta += f64::from(delta);
        } else {
            dynamic_span += f64::from(route.amount.get().abs());
        }
    }

    if active_routes == 0 {
        return None;
    }
    Some(EffectiveParameterModulation {
        static_delta: static_delta.clamp(-1.0, 1.0) as f32,
        dynamic_span: dynamic_span.clamp(0.0, 1.0) as f32,
        accent_color: accent_color.unwrap_or_else(|| theme::color(theme::PaletteRole::Primary)),
        active_routes,
    })
}

/// Non-interactive effective-value modulation indicator painted on mapped parameters.
pub(in crate::editor) fn modulation_feedback_overlay(
    view: &EditorView,
    device: DeviceId,
    parameter: &NativeDeviceParameterView,
) -> Option<AnyElement> {
    if !parameter.enabled
        || !parameter
            .semantics
            .is_some_and(|semantics| semantics.modulation == ParameterModulation::SampleRate)
    {
        return None;
    }
    let state = view.state.as_ref()?;
    let bank = state.project.device(device)?.modulation();
    let effective = effective_parameter_modulation(bank, parameter.descriptor.id)?;
    let selector = format!(
        "param-mod-effective-{}-{}-{}",
        device.value(),
        parameter.descriptor.id.value(),
        depth_text(effective.static_delta)
    );
    let fill = if effective.static_delta.abs() > 1e-4 {
        effective.static_delta.abs().clamp(0.15, 1.0)
    } else {
        effective.dynamic_span.clamp(0.15, 1.0)
    };
    Some(
        div()
            .debug_selector(move || selector.clone())
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(2.0))
            .bg(rgb(theme::color(theme::PaletteRole::Border)))
            .child(
                div()
                    .h_full()
                    .w(gpui::relative(fill))
                    .bg(rgb(effective.accent_color)),
            )
            .into_any_element(),
    )
}

pub(in crate::editor) fn mapping_button(
    view: &EditorView,
    cx: &mut Context<EditorView>,
    selector: String,
    label: SharedString,
    target: target_drag::Target,
    route: Option<&DeviceModulationRoute>,
    revision: u64,
) -> Stateful<Div> {
    let depth = target_drag::displayed_depth(view, target, route);
    let control = hex_widgets::action(label, theme::action_style())
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .relative()
        .px(px(3.0))
        .h(px(15.0))
        .gap(px(3.0))
        .border_color(rgb(theme::color(theme::PaletteRole::Primary)))
        .child(
            div()
                .min_w(px(30.0))
                .children(depth.map(|depth| theme::text(depth_text(depth)))),
        );
    target_drag::attach(view, cx, control, target, route, revision)
}

pub(in crate::editor) fn depth_text(depth: f32) -> String {
    format!("{:+.0}%", depth * 100.0)
}

pub(in crate::editor) fn vst3_destination(
    project: &hex_project::Project,
    id: DeviceId,
    parameter: u32,
    class: [u8; 16],
) -> Result<ModulationDestination, Invalid> {
    let unsupported = || Invalid("this vst3 parameter does not support sample-rate modulation");
    let location = project.device_location(id).ok_or_else(unsupported)?;
    let device = location.device;
    let target = DeviceAutomationTarget::Vst3 { parameter };
    if location.class != hex_project::DeviceUnitClass::Effect
        || matches!(location.owner, hex_project::DeviceOwner::Source(_))
        || device.state().type_id() != hex_lattice_contract::VST3_DEVICE_TYPE_ID
        || device.automation_target_class(target)? != Some(class)
        || device
            .reading_automation()
            .any(|lane| lane.target == target)
        || hex_engine::project_vst3_modulation_target(device)?
            .is_some_and(|selected| selected != (parameter, class))
    {
        return Err(unsupported());
    }
    Ok(ModulationDestination::Device {
        device_type: device.state().type_id(),
        target,
        vst3_class: Some(class),
    })
}

pub(in crate::editor) fn is_pressure_source(sources: &[Modulator], id: ModulationSourceId) -> bool {
    sources
        .iter()
        .any(|source| source.id == id && source.state.type_id() == pressure::TYPE_ID)
}

pub(in crate::editor) fn is_button_source(sources: &[Modulator], id: ModulationSourceId) -> bool {
    sources
        .iter()
        .any(|source| source.id == id && manual_button::uses_identity_range(source.state.type_id()))
}

pub(in crate::editor) fn unipolar_range() -> Result<ModulationRange, Invalid> {
    Ok(ModulationRange {
        start: ModulationAmount::new(0.0)?,
        end: ModulationAmount::new(1.0)?,
    })
}

pub(in crate::editor) fn action(key: String, label: SharedString) -> Stateful<Div> {
    hex_widgets::action(label, theme::action_style())
        .id(ElementId::Name(key.clone().into()))
        .debug_selector(move || key.clone())
        .px(px(3.0))
        .h(px(15.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}
