//! Native modulator controls. The panel renders canonical assignments and emits
//! revision-bound edits. A selected mapping source is view state only.
use super::*;
use gpui::{AnyElement, SharedString};
use hex_engine::{
    audio_rate_modulator as audio_rate, follower_modulator as follower, lfo_modulator as lfo,
    macro_modulator as manual, random_modulator as random,
};
use hex_lattice_contract::{ParameterId, ParameterModulation};
use hex_project::{
    Device, DeviceAutomationTarget, DeviceModulation, DeviceModulationRoute, Invalid,
    ModulationAmount, ModulationDestination, ModulationOutputId, ModulationRange,
    ModulationRouteId, ModulationSourceId, Modulator,
};

#[derive(Clone, Copy)]
enum SourceKind {
    Lfo,
    Follower,
    Macro,
    Random,
    AudioRate,
}

impl SourceKind {
    const ALL: [Self; 5] = [
        Self::Lfo,
        Self::Follower,
        Self::Macro,
        Self::Random,
        Self::AudioRate,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Lfo => "lfo",
            Self::Follower => "follower",
            Self::Macro => "macro",
            Self::Random => "random",
            Self::AudioRate => "audio rate",
        }
    }

    fn default_state(
        self,
        identity: u64,
    ) -> Result<NativeDeviceState, hex_lattice_contract::DeviceContractError> {
        match self {
            Self::Lfo => lfo::Settings::default().encode(),
            Self::Follower => follower::Settings::default().encode(),
            Self::AudioRate => audio_rate::default_state(),
            Self::Macro => manual::Settings::default().encode(),
            Self::Random => random::Settings {
                seed: identity,
                rate_hz: 1.0,
            }
            .encode(),
        }
    }
}

#[derive(Clone, Copy)]
enum MappingTarget {
    Device(ParameterId),
    HostGain(HostGainTarget),
    Macro(ModulationSourceId),
    LfoRate(ModulationSourceId),
}

#[derive(Clone, Copy)]
enum HostGainTarget {
    Input,
    Output,
}

impl HostGainTarget {
    fn mapping(self) -> (DeviceAutomationTarget, &'static str, &'static str) {
        match self {
            Self::Input => (
                DeviceAutomationTarget::HostInputGain,
                "input-gain",
                "input trim",
            ),
            Self::Output => (
                DeviceAutomationTarget::HostOutputGain,
                "output-gain",
                "output trim",
            ),
        }
    }
}

fn macro_destination(source: ModulationSourceId) -> ModulationDestination {
    ModulationDestination::Modulator {
        source,
        module_type: manual::TYPE_ID,
        parameter: manual::VALUE_PARAMETER,
    }
}

fn lfo_rate_destination(source: ModulationSourceId) -> ModulationDestination {
    ModulationDestination::Modulator {
        source,
        module_type: lfo::TYPE_ID,
        parameter: lfo::RATE_PARAMETER,
    }
}

#[derive(Clone, Copy)]
enum Edit {
    Add(SourceKind),
    ToggleSource(ModulationSourceId),
    RemoveSource(ModulationSourceId),
    Rate(ModulationSourceId, f64),
    Sync(ModulationSourceId),
    LfoShape(ModulationSourceId, lfo::Waveform),
    MacroValue(ModulationSourceId, f32),
    FollowerTime {
        source: ModulationSourceId,
        attack: bool,
        factor: f64,
    },
    RandomRate(ModulationSourceId, f64),
    Reseed(ModulationSourceId),
    Map(ModulationSourceId, MappingTarget),
    Adjust(ModulationRouteId, f32),
    Range(ModulationRouteId),
    ToggleRoute(ModulationRouteId),
    RemoveRoute(ModulationRouteId),
}

pub(super) struct HostGainMappingAvailability {
    device: DeviceId,
    generation: u64,
    revision: u64,
    eligible: bool,
}

pub(super) const SOURCE_EFFECT_CARD_WIDTH: f32 = 172.0;

/// The bound receiver owns these controls. The card reuses the ordinary
/// modulator housing and deliberately has no track-chain drop geometry.
pub(super) fn source_effect_card(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: &Device,
) -> Stateful<Div> {
    let id = device.id();
    let selector = format!("source-effect-modulation-{}", id.value());
    let outer = div()
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .w(px(SOURCE_EFFECT_CARD_WIDTH))
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
    let mut body = crate::device_composition::body()
        .flex()
        .flex_col()
        .gap(px(3.0));
    body = body.child(theme::text("source effect"));
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
    for row in &metadata.parameters {
        body = body.child(
            div()
                .relative()
                .px(px(4.0))
                .h(px(15.0))
                .child(theme::text(row.descriptor.label))
                .children(target_overlay(view, cx, id, row)),
        );
    }
    let modulation = panel(view, cx, device, &metadata);
    let rendered = hex_widgets::device_frame_with_modulators(
        theme::text(metadata.name),
        theme::device_frame_style(false),
        div(),
        div(),
        body,
        modulation,
    )
    .h_full();
    outer.child(rendered)
}

pub(super) fn panel(
    view: &mut EditorView,
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
    let selected = view
        .device_panel
        .mapping_source
        .filter(|(device, source)| {
            *device == id && bank.sources().iter().any(|slot| slot.id == *source)
        })
        .map(|(_, source)| source);
    let host_gain_available = if selected.is_some() || (!has_target && bank.is_empty()) {
        let cache = &mut view.device_panel.host_gain_mapping_availability;
        if let Some(cached) = cache.as_ref().filter(|entry| {
            entry.device == id && entry.generation == generation && entry.revision == revision
        }) {
            cached.eligible
        } else {
            let eligible =
                hex_engine::project_device_host_gain_modulation_supported(&state.project, id);
            *cache = Some(HostGainMappingAvailability {
                device: id,
                generation,
                revision,
                eligible,
            });
            eligible
        }
    } else {
        false
    };
    if !has_target && !host_gain_available && bank.is_empty() {
        return None;
    }
    let button = |cx: &mut Context<EditorView>, key: String, label: SharedString, edit| {
        action(key, label).on_click(cx.listener(move |view, _, _, cx| {
            cx.stop_propagation();
            submit(view, generation, revision, id, edit);
            cx.notify();
        }))
    };
    let mut header = div().flex().flex_wrap().items_center().gap(px(4.0));
    if bank.sources().len() < hex_project::MAX_MODULATORS_PER_DEVICE {
        for kind in SourceKind::ALL {
            let name = kind.label();
            header = header.child(button(
                cx,
                format!("add-{name}-{}", id.value()),
                theme::text(format!("+ {name}")),
                Edit::Add(kind),
            ));
        }
    }
    let mut body = div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap(px(3.0))
        .child(header);
    if let Some(source) = selected
        && host_gain_available
        && bank.routes().len() < hex_project::MAX_MODULATION_ROUTES_PER_DEVICE
    {
        let targets = [HostGainTarget::Input, HostGainTarget::Output]
            .into_iter()
            .filter_map(|stage| {
                let (target, key, label) = stage.mapping();
                let assigned = bank.routes().iter().any(|route| {
                    route.source == source
                        && matches!(route.destination, ModulationDestination::Device { target: existing, .. } if existing == target)
                });
                (!assigned).then(|| button(
                    cx,
                    format!("map-{key}-{}", id.value()),
                    theme::text(label),
                    Edit::Map(source, MappingTarget::HostGain(stage)),
                ))
            });
        body = body.child(div().flex().flex_wrap().gap(px(4.0)).children(targets));
    }
    let blocked_targets: Vec<_> = selected
        .into_iter()
        .flat_map(|source| bank.source_route_ancestors(source))
        .collect();
    let mapping_source_for = |target, destination| {
        selected.filter(|source| {
            !blocked_targets.is_empty()
                && !blocked_targets.contains(&target)
                && bank.routes().len() < hex_project::MAX_MODULATION_ROUTES_PER_DEVICE
                && !bank
                    .routes()
                    .iter()
                    .any(|route| route.source == *source && route.destination == destination)
        })
    };
    let multiple_macros = bank
        .sources()
        .iter()
        .filter(|source| source.state.type_id() == manual::TYPE_ID)
        .take(2)
        .count()
        > 1;
    let multiple_lfos = bank
        .sources()
        .iter()
        .filter(|source| source.state.type_id() == lfo::TYPE_ID)
        .take(2)
        .count()
        > 1;
    for (slot, source) in bank.sources().iter().enumerate() {
        let source_id = source.id;
        let kind = match source.state.type_id() {
            lfo::TYPE_ID => SourceKind::Lfo,
            follower::TYPE_ID => SourceKind::Follower,
            manual::TYPE_ID => SourceKind::Macro,
            random::TYPE_ID => SourceKind::Random,
            audio_rate::TYPE_ID => SourceKind::AudioRate,
            _ => continue,
        };
        let label = kind.label();
        let title = if (multiple_macros && matches!(kind, SourceKind::Macro))
            || (multiple_lfos && matches!(kind, SourceKind::Lfo))
        {
            format!("{label} {}", slot + 1)
        } else {
            label.to_owned()
        };
        let mapping = selected == Some(source_id);
        let source_key = |suffix: &str| format!("{label}-{}-{suffix}", source_id.value());
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
            .child(theme::text(title))
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
        if let SourceKind::Lfo = kind
            && let Ok(settings) = lfo::Settings::decode(&source.state)
        {
            let (rate, units) = match settings.rate {
                lfo::Rate::Hertz(rate) => (rate, "hz"),
                lfo::Rate::BeatsPerCycle(rate) => (rate, "beats"),
            };
            let (shape, next) = match settings.waveform {
                lfo::Waveform::Sine => ("sine", lfo::Waveform::Triangle),
                lfo::Waveform::Triangle => ("triangle", lfo::Waveform::RisingRamp),
                lfo::Waveform::RisingRamp => ("ramp", lfo::Waveform::Square),
                lfo::Waveform::Square => ("square", lfo::Waveform::Sine),
            };
            let destination = lfo_rate_destination(source_id);
            let rate_text = format!("{rate:.2} {units}");
            let rate_control =
                if let Some(mapping_source) = mapping_source_for(source_id, destination) {
                    button(
                        cx,
                        format!("map-lfo-rate-{}-{}", id.value(), source_id.value()),
                        theme::text(rate_text),
                        Edit::Map(mapping_source, MappingTarget::LfoRate(source_id)),
                    )
                    .border_color(rgb(theme::color(theme::PaletteRole::Primary)))
                    .into_any_element()
                } else if selected.is_some() {
                    div()
                        .px(px(4.0))
                        .h(px(15.0))
                        .child(theme::text(rate_text))
                        .into_any_element()
                } else {
                    button(
                        cx,
                        source_key("sync"),
                        theme::text(rate_text),
                        Edit::Sync(source_id),
                    )
                    .into_any_element()
                };
            row = row
                .child(button(
                    cx,
                    source_key("slower"),
                    theme::text("−"),
                    Edit::Rate(source_id, 0.5),
                ))
                .child(div().children([rate_control]))
                .child(button(
                    cx,
                    source_key("faster"),
                    theme::text("+"),
                    Edit::Rate(source_id, 2.0),
                ))
                .child(
                    button(
                        cx,
                        source_key("waveform"),
                        theme::text(shape),
                        Edit::LfoShape(source_id, next),
                    )
                    .w(px(55.0)),
                );
        }
        if let SourceKind::Follower = kind
            && let Ok(settings) = follower::Settings::decode(&source.state)
        {
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
        if let SourceKind::Macro = kind
            && let Ok(settings) = manual::Settings::decode(&source.state)
        {
            let value = format!("{:.0}%", settings.value * 100.0);
            if let Some(mapping_source) =
                mapping_source_for(source_id, macro_destination(source_id))
            {
                row = row.child(
                    button(
                        cx,
                        format!("map-macro-{}-{}", id.value(), source_id.value()),
                        theme::text(value),
                        Edit::Map(mapping_source, MappingTarget::Macro(source_id)),
                    )
                    .border_color(rgb(theme::color(theme::PaletteRole::Primary))),
                );
            } else {
                row = row
                    .child(button(
                        cx,
                        source_key("less"),
                        theme::text("−"),
                        Edit::MacroValue(source_id, -0.05),
                    ))
                    .child(theme::text(value))
                    .child(button(
                        cx,
                        source_key("more"),
                        theme::text("+"),
                        Edit::MacroValue(source_id, 0.05),
                    ));
            }
        }
        if let SourceKind::Random = kind
            && let Ok(settings) = random::Settings::decode(&source.state)
        {
            row = row.child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(3.0))
                    .child(button(
                        cx,
                        source_key("slower"),
                        theme::text("−"),
                        Edit::RandomRate(source_id, 0.5),
                    ))
                    .child(theme::text(format!("{:.2} hz", settings.rate_hz)))
                    .child(button(
                        cx,
                        source_key("faster"),
                        theme::text("+"),
                        Edit::RandomRate(source_id, 2.0),
                    ))
                    .child(button(
                        cx,
                        source_key("reseed"),
                        theme::text("new seed"),
                        Edit::Reseed(source_id),
                    )),
            );
        }
        body = body.child(row);
        for route in bank
            .routes()
            .iter()
            .filter(|route| route.source == source_id)
        {
            let label = match route.destination {
                ModulationDestination::Device {
                    target: DeviceAutomationTarget::Parameter(parameter),
                    ..
                } => view_data
                    .parameters
                    .iter()
                    .find(|row| row.descriptor.id == parameter)
                    .map_or("unavailable", |row| row.descriptor.label)
                    .to_owned(),
                ModulationDestination::Device {
                    target: DeviceAutomationTarget::HostInputGain,
                    ..
                } => HostGainTarget::Input.mapping().2.to_owned(),
                ModulationDestination::Device {
                    target: DeviceAutomationTarget::HostOutputGain,
                    ..
                } => HostGainTarget::Output.mapping().2.to_owned(),
                ModulationDestination::Modulator {
                    source,
                    module_type: lfo::TYPE_ID,
                    parameter: lfo::RATE_PARAMETER,
                } => {
                    let Some(slot) = bank.sources().iter().position(|slot| slot.id == source)
                    else {
                        continue;
                    };
                    if multiple_lfos {
                        format!("lfo {} rate", slot + 1)
                    } else {
                        "lfo rate".to_owned()
                    }
                }
                ModulationDestination::Modulator {
                    source,
                    module_type: manual::TYPE_ID,
                    parameter: manual::VALUE_PARAMETER,
                } => {
                    let Some(slot) = bank.sources().iter().position(|slot| slot.id == source)
                    else {
                        continue;
                    };
                    if multiple_macros {
                        format!("macro {} value", slot + 1)
                    } else {
                        "macro value".to_owned()
                    }
                }
                _ => continue,
            };
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
                        key("toggle"),
                        theme::text(if route.enabled { "on" } else { "off" }),
                        Edit::ToggleRoute(route.id),
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
            .debug_selector(|| "device-modulator-scroll".into())
            .flex()
            .flex_col()
            .max_h(px(58.0))
            .occlude()
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
                        Edit::Map(source, MappingTarget::Device(parameter)),
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
        Edit::Add(kind) => sources.push(Modulator {
            color: Default::default(),
            id: ModulationSourceId::new(project.next_identity()).map_err(unsupported)?,
            enabled: true,
            state: kind
                .default_state(project.next_identity())
                .map_err(unsupported)?,
        }),
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
        Edit::LfoShape(id, waveform) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = lfo::Settings::decode(&source.state).map_err(unsupported)?;
            settings.waveform = waveform;
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::MacroValue(id, change) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = manual::Settings::decode(&source.state).map_err(unsupported)?;
            settings.value = (settings.value + change).clamp(0.0, 1.0);
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
        Edit::RandomRate(id, factor) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = random::Settings::decode(&source.state).map_err(unsupported)?;
            settings.rate_hz = (settings.rate_hz * factor).clamp(0.01, maximum_hz);
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::Reseed(id) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = random::Settings::decode(&source.state).map_err(unsupported)?;
            settings.seed = settings.seed.wrapping_add(0xd1b5_4a32_d192_ed03);
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::Map(source, target) => {
            let destination = match target {
                MappingTarget::Device(parameter) => {
                    if !hex_engine::project_device_modulation_target_supported(
                        project, id, parameter,
                    ) {
                        return Err(Invalid(
                            "this parameter does not support sample-rate modulation",
                        ));
                    }
                    ModulationDestination::Device {
                        device_type: device.state().type_id(),
                        target: DeviceAutomationTarget::Parameter(parameter),
                        vst3_class: None,
                    }
                }
                MappingTarget::HostGain(stage) => {
                    if !hex_engine::project_device_host_gain_modulation_supported(project, id) {
                        return Err(Invalid("this device does not support host gain modulation"));
                    }
                    ModulationDestination::Device {
                        device_type: device.state().type_id(),
                        target: stage.mapping().0,
                        vst3_class: None,
                    }
                }
                MappingTarget::Macro(target) => macro_destination(target),
                MappingTarget::LfoRate(target) => {
                    let source = find_source(&mut sources, target)?;
                    lfo::Settings::decode(&source.state).map_err(unsupported)?;
                    lfo_rate_destination(target)
                }
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
        Edit::ToggleRoute(id) => {
            let route = find_route(&mut routes, id)?;
            route.enabled = !route.enabled;
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

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project};

    #[test]
    fn enabling_a_route_that_closes_a_cycle_preserves_the_existing_bank() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Device(device) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_devices::utility::UTILITY_TYPE_ID,
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("utility fixture");
        };
        for _ in 0..2 {
            let modulation = apply(editor.project(), device, Edit::Add(SourceKind::Macro)).unwrap();
            editor
                .execute(Command::ReplaceDeviceModulation { device, modulation })
                .unwrap();
        }
        let bank = editor.project().device(device).unwrap().modulation();
        let [first, second] = [bank.sources()[0].id, bank.sources()[1].id];
        let mapped = apply(
            editor.project(),
            device,
            Edit::Map(first, MappingTarget::Macro(second)),
        )
        .unwrap();
        let mut routes = mapped.routes().to_vec();
        let id = ModulationRouteId::new(editor.project().next_identity() + 1).unwrap();
        routes.push(DeviceModulationRoute {
            id,
            source: second,
            destination: macro_destination(first),
            enabled: false,
            ..routes[0]
        });
        let modulation = DeviceModulation::new(mapped.sources().to_vec(), routes).unwrap();
        editor
            .execute(Command::ReplaceDeviceModulation {
                device,
                modulation: modulation.clone(),
            })
            .unwrap();

        assert_eq!(
            apply(editor.project(), device, Edit::ToggleRoute(id)),
            Err(Invalid("modulation routes form a cycle")),
        );
        assert_eq!(
            editor.project().device(device).unwrap().modulation(),
            &modulation
        );
    }
}
