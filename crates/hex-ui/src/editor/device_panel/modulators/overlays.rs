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
        state.revision,
        format!("map-target-{}-{}", device.value(), parameter.value()),
        format!("map-depth-{}-{}", device.value(), parameter.value()),
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
    Some(mapped_target_overlay(
        view,
        cx,
        target_drag::Target {
            device,
            source,
            mapping: MappingTarget::Vst3(mapping),
        },
        assigned,
        state.revision,
        format!("map-vst3-{control}-{}-{}", device.value(), row.id),
        format!("map-vst3-{control}-depth-{}-{}", device.value(), row.id),
    ))
}

pub(in crate::editor) fn mapped_target_overlay(
    view: &EditorView,
    cx: &mut Context<EditorView>,
    target: target_drag::Target,
    assigned: Option<&DeviceModulationRoute>,
    revision: u64,
    selector: String,
    depth_selector: String,
) -> AnyElement {
    let display = target_drag::displayed_depth(view, target, assigned);
    let control = div()
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .absolute()
        .inset_0()
        .border_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Primary)))
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
                .text_color(rgb(theme::color(theme::PaletteRole::Primary)))
                .debug_selector(move || selector.clone())
                .child(theme::text(label))
        }));
    target_drag::attach(view, cx, control, target, assigned, revision).into_any_element()
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
