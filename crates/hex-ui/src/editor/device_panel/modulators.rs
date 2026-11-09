//! Native modulator controls. The panel renders canonical assignments and emits
//! revision-bound edits. A selected mapping source is view state only.
use super::*;
use gpui::{AnyElement, SharedString};
use hex_engine::{follower_modulator as follower, lfo_modulator as lfo};
use hex_lattice_contract::{ParameterId, ParameterModulation};
use hex_project::{
    Device, DeviceAutomationTarget, DeviceModulation, DeviceModulationRoute, Invalid,
    ModulationAmount, ModulationDestination, ModulationOutputId, ModulationRange,
    ModulationRouteId, ModulationSourceId, Modulator,
};

#[derive(Clone, Copy)]
enum Edit {
    AddLfo,
    AddFollower,
    ToggleSource(ModulationSourceId),
    RemoveSource(ModulationSourceId),
    Rate(ModulationSourceId, f64),
    Sync(ModulationSourceId),
    FollowerTime {
        source: ModulationSourceId,
        attack: bool,
        factor: f64,
    },
    Map(ModulationSourceId, ParameterId),
    Adjust(ModulationRouteId, f32),
    Range(ModulationRouteId),
    RemoveRoute(ModulationRouteId),
}

pub(super) fn panel(
    view: &EditorView,
    cx: &mut Context<EditorView>,
    device: &Device,
    view_data: &NativeDeviceView,
) -> Option<AnyElement> {
    let state = view.state.as_ref()?;
    let id = device.id();
    let revision = state.revision;
    let generation = view.project_generation;
    let bank = device.modulation();
    let has_target = view_data.parameters.iter().any(|row| {
        row.enabled
            && row
                .semantics
                .is_some_and(|semantics| semantics.modulation == ParameterModulation::SampleRate)
    });
    if !has_target && bank.is_empty() {
        return None;
    }
    let button = |cx: &mut Context<EditorView>, key: String, label: SharedString, edit| {
        action(key, label).on_click(cx.listener(move |view, _, _, cx| {
            cx.stop_propagation();
            submit(view, generation, revision, id, edit);
            cx.notify();
        }))
    };
    let mut body = div().flex().flex_col().gap(px(3.0)).child(
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(4.0))
            .child(theme::text("modulators"))
            .when(
                bank.sources().len() < hex_project::MAX_MODULATORS_PER_DEVICE,
                |row| {
                    row.child(button(
                        cx,
                        format!("add-lfo-{}", id.value()),
                        theme::text("+ lfo"),
                        Edit::AddLfo,
                    ))
                    .child(button(
                        cx,
                        format!("add-follower-{}", id.value()),
                        theme::text("+ follower"),
                        Edit::AddFollower,
                    ))
                },
            ),
    );
    let selected = view
        .device_panel
        .mapping_source
        .filter(|(device, _)| *device == id)
        .map(|(_, source)| source);
    for source in bank.sources() {
        let source_id = source.id;
        let lfo = lfo::Settings::decode(&source.state).ok();
        let follower = follower::Settings::decode(&source.state).ok();
        let (label, prefix) = match (lfo, follower) {
            (Some(_), _) => ("lfo", "lfo"),
            (_, Some(_)) => ("follower", "follower"),
            _ => continue,
        };
        let mapping = selected == Some(source_id);
        let source_key = |suffix: &str| format!("{prefix}-{}-{suffix}", source_id.value());
        let control = action(
            source_key("map"),
            theme::text(if mapping { "done" } else { "map" }),
        )
        .on_click(cx.listener(move |view, _, _, cx| {
            cx.stop_propagation();
            if view.project_generation != generation {
                return;
            }
            view.device_panel.mapping_source = (!mapping).then_some((id, source_id));
            cx.notify();
        }));
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(3.0))
            .child(theme::text(label))
            .child(button(
                cx,
                source_key("power"),
                theme::text(if source.enabled { "on" } else { "off" }),
                Edit::ToggleSource(source_id),
            ))
            .child(control)
            .child(button(
                cx,
                source_key("remove"),
                theme::text("×"),
                Edit::RemoveSource(source_id),
            ));
        if let Some(settings) = lfo {
            let (rate, units) = match settings.rate {
                lfo::Rate::Hertz(rate) => (rate, "hz"),
                lfo::Rate::BeatsPerCycle(rate) => (rate, "beats"),
            };
            row = row
                .child(button(
                    cx,
                    source_key("slower"),
                    theme::text("−"),
                    Edit::Rate(source_id, 0.5),
                ))
                .child(button(
                    cx,
                    source_key("sync"),
                    theme::text(format!("{rate:.2} {units}")),
                    Edit::Sync(source_id),
                ))
                .child(button(
                    cx,
                    source_key("faster"),
                    theme::text("+"),
                    Edit::Rate(source_id, 2.0),
                ));
        }
        if let Some(settings) = follower {
            for (attack, name, seconds) in [
                (true, "attack", settings.attack_seconds),
                (false, "release", settings.release_seconds),
            ] {
                row = row.child(
                    div()
                        .flex()
                        .gap(px(2.0))
                        .items_center()
                        .child(button(
                            cx,
                            source_key(&format!("{name}-less")),
                            theme::text("−"),
                            Edit::FollowerTime {
                                source: source_id,
                                attack,
                                factor: 0.5,
                            },
                        ))
                        .child(theme::text(format!("{name} {:.1} ms", seconds * 1000.0)))
                        .child(button(
                            cx,
                            source_key(&format!("{name}-more")),
                            theme::text("+"),
                            Edit::FollowerTime {
                                source: source_id,
                                attack,
                                factor: 2.0,
                            },
                        )),
                );
            }
        }
        body = body.child(row);
        for route in bank
            .routes()
            .iter()
            .filter(|route| route.source == source_id)
        {
            let ModulationDestination::Device {
                target: DeviceAutomationTarget::Parameter(parameter),
                ..
            } = route.destination
            else {
                continue;
            };
            let label = view_data
                .parameters
                .iter()
                .find(|row| row.descriptor.id == parameter)
                .map_or("unavailable", |row| row.descriptor.label);
            let key = |suffix| format!("modulation-route-{}-{suffix}", route.id.value());
            let range = if route.range == ModulationRange::default() {
                "bipolar"
            } else {
                "unipolar"
            };
            body = body.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(3.0))
                    .child(theme::text(label))
                    .child(button(
                        cx,
                        key("less"),
                        theme::text("−"),
                        Edit::Adjust(route.id, -0.05),
                    ))
                    .child(theme::text(format!("{:+.0}%", route.amount.get() * 100.0)))
                    .child(button(
                        cx,
                        key("more"),
                        theme::text("+"),
                        Edit::Adjust(route.id, 0.05),
                    ))
                    .child(button(
                        cx,
                        key("range"),
                        theme::text(range),
                        Edit::Range(route.id),
                    ))
                    .child(button(
                        cx,
                        key("remove"),
                        theme::text("×"),
                        Edit::RemoveRoute(route.id),
                    )),
            );
        }
    }
    Some(
        div()
            .id("device-modulator-scroll")
            .max_h(px(58.0))
            .overflow_y_scroll()
            .child(body)
            .into_any_element(),
    )
}

pub(super) fn target_overlay(
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
    if bank.routes().len() >= hex_project::MAX_MODULATION_ROUTES_PER_DEVICE
        || !target.enabled
        || !target
            .semantics
            .is_some_and(|semantics| semantics.modulation == ParameterModulation::SampleRate)
        || !bank.sources().iter().any(|row| row.id == source)
    {
        return None;
    }
    let assigned = bank.routes().iter().any(|route| {
        route.source == source
            && matches!(
                route.destination,
                ModulationDestination::Device { target: DeviceAutomationTarget::Parameter(id), .. }
                    if id == parameter
            )
    });
    let generation = view.project_generation;
    let revision = state.revision;
    let selector = format!("map-target-{}-{}", device.value(), parameter.value());
    Some(
        div()
            .id(ElementId::Name(selector.clone().into()))
            .debug_selector(move || selector.clone())
            .absolute()
            .inset_0()
            .border_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Primary)))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |view, _, _, cx| {
                cx.stop_propagation();
                if !assigned {
                    submit(
                        view,
                        generation,
                        revision,
                        device,
                        Edit::Map(source, parameter),
                    );
                    cx.notify();
                }
            }))
            .into_any_element(),
    )
}

fn action(key: String, label: SharedString) -> Stateful<Div> {
    hex_widgets::action(label, theme::action_style())
        .id(ElementId::Name(key.clone().into()))
        .debug_selector(move || key.clone())
        .px(px(3.0))
        .h(px(15.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

fn submit(view: &mut EditorView, generation: u64, revision: u64, device: DeviceId, edit: Edit) {
    let Some(state) = view
        .state
        .as_ref()
        .filter(|state| view.project_generation == generation && state.revision == revision)
    else {
        return;
    };
    match apply(&state.project, device, edit) {
        Ok(modulation) => {
            view.enqueue(Request::ReplaceDeviceModulation {
                project_generation: generation,
                revision,
                device,
                modulation,
            });
        }
        Err(error) => view.error = Some(error.0.into()),
    }
}

fn apply(
    project: &hex_project::Project,
    id: DeviceId,
    edit: Edit,
) -> Result<DeviceModulation, Invalid> {
    let device = project
        .device(id)
        .ok_or(Invalid("device no longer exists"))?;
    let bank = device.modulation();
    let mut sources = bank.sources().to_vec();
    let mut routes = bank.routes().to_vec();
    let unsupported = |_| Invalid("modulator settings are unsupported");
    let maximum_hz = f64::from(project.sample_rate()) * 0.5;
    match edit {
        Edit::AddLfo | Edit::AddFollower => {
            let state = match edit {
                Edit::AddLfo => lfo::Settings::default().encode(),
                _ => follower::Settings::default().encode(),
            }
            .map_err(unsupported)?;
            sources.push(Modulator {
                id: ModulationSourceId::new(project.next_identity()).map_err(unsupported)?,
                enabled: true,
                state,
            });
        }
        Edit::ToggleSource(id) => {
            let source = find_source(&mut sources, id)?;
            source.enabled = !source.enabled;
        }
        Edit::RemoveSource(id) => {
            sources.retain(|source| source.id != id);
            routes.retain(|route| {
                route.source != id
                    && !matches!(route.destination, ModulationDestination::Modulator { source, .. } if source == id)
            });
        }
        Edit::Rate(id, factor) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = lfo::Settings::decode(&source.state).map_err(unsupported)?;
            settings.rate = match settings.rate {
                lfo::Rate::Hertz(rate) => lfo::Rate::Hertz((rate * factor).clamp(0.01, maximum_hz)),
                lfo::Rate::BeatsPerCycle(rate) => {
                    lfo::Rate::BeatsPerCycle((rate / factor).clamp(0.0625, 64.0))
                }
            };
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::Sync(id) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = lfo::Settings::decode(&source.state).map_err(unsupported)?;
            let beats_per_second = f64::from(
                project
                    .tempo_map()
                    .tempo_at(hex_project::BeatTime::ZERO)
                    .micro_bpm(),
            ) / 60e6;
            settings.rate = match settings.rate {
                lfo::Rate::Hertz(rate) => {
                    lfo::Rate::BeatsPerCycle((beats_per_second / rate).clamp(0.0625, 64.0))
                }
                lfo::Rate::BeatsPerCycle(beats) => {
                    lfo::Rate::Hertz((beats_per_second / beats).clamp(0.01, maximum_hz))
                }
            };
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::FollowerTime {
            source,
            attack,
            factor,
        } => {
            let source = find_source(&mut sources, source)?;
            let mut settings = follower::Settings::decode(&source.state).map_err(unsupported)?;
            let time = if attack {
                &mut settings.attack_seconds
            } else {
                &mut settings.release_seconds
            };
            *time = (*time * factor).clamp(0.0001, 10.0);
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::Map(source, parameter) => {
            if !hex_engine::project_device_modulation_target_supported(project, id, parameter) {
                return Err(Invalid(
                    "this parameter does not support sample-rate modulation",
                ));
            }
            let destination = ModulationDestination::Device {
                device_type: device.state().type_id(),
                target: DeviceAutomationTarget::Parameter(parameter),
                vst3_class: None,
            };
            if routes
                .iter()
                .any(|route| route.source == source && route.destination == destination)
            {
                return Err(Invalid("this source already modulates the parameter"));
            }
            routes.push(DeviceModulationRoute {
                id: ModulationRouteId::new(project.next_identity())?,
                source,
                output: ModulationOutputId::MAIN,
                destination,
                amount: ModulationAmount::new(0.1)?,
                range: ModulationRange::default(),
                enabled: true,
            });
        }
        Edit::Adjust(id, change) => {
            let route = find_route(&mut routes, id)?;
            route.amount = ModulationAmount::new((route.amount.get() + change).clamp(-1.0, 1.0))?;
        }
        Edit::Range(id) => {
            let route = find_route(&mut routes, id)?;
            route.range = if route.range == ModulationRange::default() {
                ModulationRange {
                    start: ModulationAmount::new(0.0)?,
                    end: ModulationAmount::new(1.0)?,
                }
            } else {
                ModulationRange::default()
            };
        }
        Edit::RemoveRoute(id) => routes.retain(|route| route.id != id),
    }
    DeviceModulation::new(
        sources.into_boxed_slice().into_vec(),
        routes.into_boxed_slice().into_vec(),
    )
}

fn find_source(
    sources: &mut [Modulator],
    id: ModulationSourceId,
) -> Result<&mut Modulator, Invalid> {
    sources
        .iter_mut()
        .find(|source| source.id == id)
        .ok_or(Invalid("modulator no longer exists"))
}

fn find_route(
    routes: &mut [DeviceModulationRoute],
    id: ModulationRouteId,
) -> Result<&mut DeviceModulationRoute, Invalid> {
    routes
        .iter_mut()
        .find(|route| route.id == id)
        .ok_or(Invalid("modulation route no longer exists"))
}
