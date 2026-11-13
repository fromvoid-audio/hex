//! Native modulator controls. The panel renders canonical assignments and emits
//! revision-bound edits. A selected mapping source is view state only.
mod target_drag;

pub(super) use target_drag::State as TargetDragState;

use super::*;
use crate::context_menu::{MenuItem, PopupMenu};
use crate::editor::context_menu::EditorContextAction;
use gpui::{AnyElement, Point, SharedString};
use hex_engine::{
    adsr_modulator as adsr, ahd_release_modulator as ahd_release,
    audio_rate_modulator as audio_rate, button_modulator as manual_button,
    buttons_modulator as buttons, follower_modulator as follower, lfo_modulator as lfo,
    macro_four_modulator as macro_four, macro_modulator as manual, math_modulator as math,
    mix_modulator as mix, note_counter_modulator as note_counter,
    note_pressure_modulator as pressure, ramp_modulator as ramp, random_modulator as random,
    steps_modulator as steps,
};
use hex_lattice_contract::{ParameterId, ParameterModulation};
use hex_project::{
    Device, DeviceAutomationTarget, DeviceModulation, DeviceModulationRoute, Invalid,
    ModulationAmount, ModulationDestination, ModulationOutputId, ModulationRange,
    ModulationRouteId, ModulationSourceId, Modulator,
};

const DEFAULT_MAPPING_DEPTH: f32 = 0.1;

#[derive(Clone, Copy)]
enum SourceKind {
    Lfo,
    Follower,
    Macro,
    MacroFour,
    Random,
    AudioRate,
    NotePressure,
    Button,
    Math,
    Buttons,
    Mix,
    NoteCounter,
    Ramp,
    AhdRelease,
    Adsr,
    Steps,
}

impl SourceKind {
    const ALL: [Self; 16] = [
        Self::Lfo,
        Self::Follower,
        Self::Macro,
        Self::MacroFour,
        Self::Random,
        Self::AudioRate,
        Self::NotePressure,
        Self::Button,
        Self::Math,
        Self::Buttons,
        Self::Mix,
        Self::NoteCounter,
        Self::Ramp,
        Self::AhdRelease,
        Self::Adsr,
        Self::Steps,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Lfo => "lfo",
            Self::Follower => "follower",
            Self::Macro => "macro",
            Self::MacroFour => "macro-4",
            Self::Random => "random",
            Self::AudioRate => "audio rate",
            Self::NotePressure => "note pressure",
            Self::Button => "button",
            Self::Math => "math",
            Self::Buttons => "buttons",
            Self::Mix => "mix",
            Self::NoteCounter => "note counter",
            Self::Ramp => "ramp",
            Self::AhdRelease => "ahd on release",
            Self::Adsr => "adsr",
            Self::Steps => "steps",
        }
    }

    fn required_outputs(self) -> usize {
        match self {
            Self::NotePressure => 0,
            Self::MacroFour => macro_four::VALUE_PARAMETERS.len(),
            Self::Buttons => buttons::OUTPUT_COUNT,
            _ => 1,
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
            Self::NotePressure => pressure::default_state(),
            Self::Button => manual_button::encode(false),
            Self::Math => math::Settings::default().encode(),
            Self::Buttons => buttons::Settings::default().encode(),
            Self::Mix => mix::Settings::default().encode(),
            Self::NoteCounter => note_counter::Settings::default().encode(),
            Self::Ramp => ramp::Settings::default().encode(),
            Self::AhdRelease => ahd_release::Settings::default().encode(),
            Self::Adsr => adsr::Settings::default().encode(),
            Self::Steps => steps::Settings::default().encode(),
            Self::Macro => manual::Settings::default().encode(),
            Self::MacroFour => macro_four::Settings::default().encode(),
            Self::Random => random::Settings {
                seed: identity,
                rate_hz: 1.0,
            }
            .encode(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::editor) struct MappingOutput {
    source: ModulationSourceId,
    output: ModulationOutputId,
}

impl MappingOutput {
    pub(super) const fn main(source: ModulationSourceId) -> Self {
        Self {
            source,
            output: ModulationOutputId::MAIN,
        }
    }

    fn owns(self, route: &DeviceModulationRoute) -> bool {
        route.source == self.source && route.output == self.output
    }

    fn valid_for(self, slot: &Modulator) -> bool {
        self.source == slot.id
            && match slot.state.type_id() {
                macro_four::TYPE_ID => macro_four::VALUE_PARAMETERS.iter().any(|&parameter| {
                    macro_four::output_for_parameter(parameter) == Some(self.output)
                }),
                buttons::TYPE_ID => buttons::index_for_output(self.output).is_some(),
                _ => self.output == ModulationOutputId::MAIN,
            }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MappingTarget {
    Device(ParameterId),
    Vst3(vst3_parameters::MappingTarget),
    HostGain(HostGainTarget),
    Macro(ModulationSourceId),
    MacroFour(ModulationSourceId, ParameterId),
    LfoRate(ModulationSourceId),
    CompositionInput(ModulationSourceId, ParameterId),
}

#[derive(Clone, Copy, PartialEq, Eq)]
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

fn macro_four_destination(
    source: ModulationSourceId,
    parameter: ParameterId,
) -> ModulationDestination {
    ModulationDestination::Modulator {
        source,
        module_type: macro_four::TYPE_ID,
        parameter,
    }
}

fn composition_destination(
    source: ModulationSourceId,
    module_type: hex_lattice_contract::DeviceTypeId,
    parameter: ParameterId,
) -> ModulationDestination {
    ModulationDestination::Modulator {
        source,
        module_type,
        parameter,
    }
}

fn composition_input_name(
    module_type: hex_lattice_contract::DeviceTypeId,
    parameter: ParameterId,
) -> Option<&'static str> {
    match (module_type, parameter) {
        (math::TYPE_ID, math::INPUT_A) | (mix::TYPE_ID, mix::INPUT_A) => Some("a"),
        (math::TYPE_ID, math::INPUT_B) | (mix::TYPE_ID, mix::INPUT_B) => Some("b"),
        (mix::TYPE_ID, mix::BLEND) => Some("blend"),
        _ => None,
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
enum CounterEdit {
    Steps(i16),
    Increment(i16),
    TogglePolarity,
}

#[derive(Clone, Copy)]
enum RampEdit {
    Duration { longer: bool },
    Curve(ramp::Curve),
    ToggleDirection,
    ToggleLoop,
}

#[derive(Clone, Copy)]
enum AhdEdit {
    Duration {
        segment: usize,
        longer: bool,
    },
    Curve {
        index: usize,
        curve: ahd_release::Curve,
    },
    ToggleSingleTrigger,
}

#[derive(Clone, Copy)]
enum AdsrEdit {
    Duration { segment: usize, longer: bool },
    Sustain(f32),
    ToggleSingleTrigger,
}

#[derive(Clone, Copy)]
enum StepsEdit {
    Time { longer: bool },
    ToggleTiming,
    Length { add: bool },
    Value { index: usize, delta: i16 },
    CycleDirection,
}

fn duration_text(micros: u32) -> String {
    if micros < 1000 {
        format!("{micros} µs")
    } else {
        format!("{:.3} ms", f64::from(micros) / 1000.0)
    }
}

fn curve_control(curve: ramp::Curve) -> (&'static str, ramp::Curve) {
    match curve {
        ramp::Curve::Linear => ("linear", ramp::Curve::EaseIn),
        ramp::Curve::EaseIn => ("ease in", ramp::Curve::EaseOut),
        ramp::Curve::EaseOut => ("ease out", ramp::Curve::Linear),
    }
}

fn duration_step(micros: u32, longer: bool) -> u32 {
    if longer {
        micros
            .saturating_mul(2)
            .clamp(ramp::MIN_MICROS, ramp::MAX_MICROS)
    } else if micros <= ramp::MIN_MICROS {
        0
    } else {
        (micros / 2).max(ramp::MIN_MICROS)
    }
}

#[derive(Clone, Copy)]
enum Edit {
    Add(SourceKind),
    ToggleSource(ModulationSourceId),
    ToggleButton(ModulationSourceId),
    ToggleButtons(MappingOutput),
    NoteCounter(ModulationSourceId, CounterEdit),
    Ramp(ModulationSourceId, RampEdit),
    AhdRelease(ModulationSourceId, AhdEdit),
    Adsr(ModulationSourceId, AdsrEdit),
    Steps(ModulationSourceId, StepsEdit),
    DuplicateSource(ModulationSourceId),
    RemoveSource(ModulationSourceId),
    Rate(ModulationSourceId, f64),
    Sync(ModulationSourceId),
    LfoShape(ModulationSourceId, lfo::Waveform),
    MacroValue(ModulationSourceId, f32),
    MacroFourValue(ModulationSourceId, ParameterId, f32),
    CompositionValue(ModulationSourceId, ParameterId, f32),
    MathOperation(ModulationSourceId, math::Operation),
    FollowerTime {
        source: ModulationSourceId,
        attack: bool,
        factor: f64,
    },
    RandomRate(ModulationSourceId, f64),
    Reseed(ModulationSourceId),
    Map(MappingOutput, MappingTarget, f32),
    SetDepth(ModulationRouteId, ModulationAmount),
    Adjust(ModulationRouteId, f32),
    Range(ModulationRouteId),
    ToggleRoute(ModulationRouteId),
    RemoveRoute(ModulationRouteId),
}

#[derive(Clone, Copy)]
struct AddModulatorTarget {
    generation: u64,
    revision: u64,
    owner: Option<DevicePanelOwner>,
    device: DeviceId,
}

impl AddModulatorTarget {
    fn current(self, view: &EditorView) -> bool {
        self.generation == view.project_generation
            && self.owner == view.device_panel.owner
            && view.state.as_ref().is_some_and(|state| {
                state.revision == self.revision && state.project.device(self.device).is_some()
            })
    }
}

#[derive(Clone, Copy)]
pub(in crate::editor) struct AddModulatorAction {
    target: AddModulatorTarget,
    kind: SourceKind,
}

impl AddModulatorAction {
    pub(in crate::editor) fn current(self, view: &EditorView) -> bool {
        self.target.current(view)
    }

    pub(in crate::editor) fn perform(self, view: &mut EditorView) {
        if self.current(view) {
            let AddModulatorTarget {
                generation,
                revision,
                device,
                ..
            } = self.target;
            submit(view, generation, revision, device, Edit::Add(self.kind));
        }
    }
}

fn open_add_menu(
    view: &mut EditorView,
    target: AddModulatorTarget,
    allow_audio_input: bool,
    position: Point<Pixels>,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    if !target.current(view) || !finalize_controls(view, cx) {
        return;
    }
    let previous_focus = view
        .context_menu
        .take()
        .and_then(|menu| menu.previous_focus)
        .or_else(|| window.focused(cx));
    let focus = cx.focus_handle();
    window.focus(&focus);
    let allow_note_pressure = view.state.as_ref().is_some_and(|state| {
        hex_engine::project_device_can_add_note_pressure(&state.project, target.device)
    });
    let allow_note_events = view.state.as_ref().is_some_and(|state| {
        hex_engine::project_device_has_modulation_note_events(&state.project, target.device)
    });
    let outputs_available = view
        .state
        .as_ref()
        .and_then(|state| state.project.device(target.device))
        .map_or(0, hex_engine::device_available_modulation_outputs);
    let items = SourceKind::ALL
        .into_iter()
        .filter(|kind| kind.required_outputs() <= outputs_available)
        .filter(|kind| match kind {
            SourceKind::Follower | SourceKind::AudioRate => allow_audio_input,
            SourceKind::NotePressure => allow_note_pressure,
            SourceKind::NoteCounter
            | SourceKind::Ramp
            | SourceKind::AhdRelease
            | SourceKind::Adsr => allow_note_events,
            _ => true,
        })
        .map(|kind| MenuItem {
            label: kind.label(),
            swatch: None,
            checked: false,
            action: EditorContextAction::AddModulator(AddModulatorAction { target, kind }),
        })
        .collect();
    view.context_menu = Some(PopupMenu {
        scroll: gpui::ScrollHandle::new(),
        icon_grid: None,
        position,
        items,
        selected: 0,
        focus,
        previous_focus,
    });
    cx.notify();
}

pub(super) struct HostGainMappingAvailability {
    device: DeviceId,
    generation: u64,
    revision: u64,
    stages: HostGainStages,
}

#[derive(Clone, Copy, Default)]
struct HostGainStages {
    input: bool,
    output: bool,
}

impl HostGainStages {
    fn supports(self, stage: HostGainTarget) -> bool {
        match stage {
            HostGainTarget::Input => self.input,
            HostGainTarget::Output => self.output,
        }
    }
}

pub(super) const SOURCE_EFFECT_CARD_WIDTH: f32 = 172.0;

/// Static Utility matrix controls share the verified source handoff.
pub(super) fn source_utility_gain_available(
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
        .gap(px(1.0));
    body = body.child(theme::text("source effect"));
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
    ]
    .map(|(parameter, label, codec)| {
        if !hex_engine::project_source_utility_matrix_edit_supported(&state.project, id, parameter)
        {
            return None;
        }
        metadata
            .parameters
            .iter()
            .find(|row| row.enabled && row.descriptor.id == parameter)
            .copied()
            .map(|row| (row, label, codec))
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
    if controls.iter().any(Option::is_some) {
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
    is_effect: bool,
) -> Option<AnyElement> {
    panel_with_targets(view, cx, device, Some(view_data), is_effect)
}

pub(super) fn vst3_panel(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: &Device,
) -> Option<AnyElement> {
    panel_with_targets(view, cx, device, None, true)
}

fn panel_with_targets(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: &Device,
    view_data: Option<&NativeDeviceView>,
    is_effect: bool,
) -> Option<AnyElement> {
    let state = view.state.as_ref()?;
    let id = device.id();
    let revision = state.revision;
    let generation = view.project_generation;
    let bank = device.modulation();
    let has_target = view_data.map_or_else(
        || vst3_parameters::has_mapping_target(view, id),
        |view_data| {
            view_data.parameters.iter().any(|row| {
                row.enabled
                    && row.semantics.is_some_and(|semantics| {
                        semantics.modulation == ParameterModulation::SampleRate
                    })
            })
        },
    );
    let selected = view
        .device_panel
        .mapping_source
        .filter(|(device, source)| {
            *device == id && bank.sources().iter().any(|slot| source.valid_for(slot))
        })
        .map(|(_, source)| source);
    let host_gains = if selected.is_some() || (!has_target && bank.is_empty()) {
        let cache = &mut view.device_panel.host_gain_mapping_availability;
        if let Some(cached) = cache.as_ref().filter(|entry| {
            entry.device == id && entry.generation == generation && entry.revision == revision
        }) {
            cached.stages
        } else {
            let effect =
                hex_engine::project_device_host_gain_modulation_supported(&state.project, id);
            let stages = HostGainStages {
                input: effect,
                output: effect
                    || hex_engine::project_device_instrument_output_modulation_supported(
                        &state.project,
                        id,
                    ),
            };
            *cache = Some(HostGainMappingAvailability {
                device: id,
                generation,
                revision,
                stages,
            });
            stages
        }
    } else {
        HostGainStages::default()
    };
    let host_gain_available = host_gains.input || host_gains.output;
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
    let mut body = div().flex().flex_col().flex_shrink_0().gap(px(3.0));
    if bank.sources().len() < hex_project::MAX_MODULATORS_PER_DEVICE
        && (hex_engine::device_available_modulation_outputs(device) > 0
            || hex_engine::project_device_can_add_note_pressure(&state.project, id))
    {
        let target = AddModulatorTarget {
            generation,
            revision,
            owner: view.device_panel.owner,
            device: id,
        };
        let add_button = action(
            format!("add-modulator-{}", id.value()),
            theme::text("+ modulator"),
        )
        .on_click(
            cx.listener(move |view, event: &gpui::ClickEvent, window, cx| {
                cx.stop_propagation();
                open_add_menu(view, target, is_effect, event.position(), window, cx);
            }),
        );
        body = body.child(add_button);
    }
    if let Some(source) = selected
        && host_gain_available
        && !is_pressure_source(bank.sources(), source.source)
    {
        let targets = [HostGainTarget::Input, HostGainTarget::Output]
            .into_iter()
            .filter(|&stage| host_gains.supports(stage))
            .filter_map(|stage| {
                let (parameter, key, label) = stage.mapping();
                let route = bank.routes().iter().find(|route| {
                    source.owns(route)
                        && matches!(route.destination, ModulationDestination::Device { target, .. } if target == parameter)
                });
                if route.is_none() && bank.routes().len() >= hex_project::MAX_MODULATION_ROUTES_PER_DEVICE {
                    return None;
                }
                let target = target_drag::Target { device: id, source, mapping: MappingTarget::HostGain(stage) };
                Some(mapping_button(
                    view, cx, format!("map-{key}-{}", id.value()), theme::text(label),
                    target, route, revision,
                ))
            });
        body = body.child(div().flex().flex_wrap().gap(px(4.0)).children(targets));
    }
    let blocked_targets: Vec<_> = selected
        .into_iter()
        .flat_map(|source| bank.source_route_ancestors(source.source))
        .collect();
    let pressure_selected =
        selected.is_some_and(|output| is_pressure_source(bank.sources(), output.source));
    let input_mapping_control = |cx: &mut Context<EditorView>,
                                 key: String,
                                 label: &str,
                                 target_id,
                                 mapping,
                                 destination| {
        let source = selected.filter(|_| !pressure_selected && !blocked_targets.is_empty())?;
        let route = bank
            .routes()
            .iter()
            .find(|route| source.owns(route) && route.destination == destination);
        if target_id == source.source
            || (route.is_none()
                && (blocked_targets.contains(&target_id)
                    || bank.routes().len() >= hex_project::MAX_MODULATION_ROUTES_PER_DEVICE))
        {
            return None;
        }
        let target = target_drag::Target {
            device: id,
            source,
            mapping,
        };
        Some(mapping_button(
            view,
            cx,
            key,
            theme::text(label),
            target,
            route,
            revision,
        ))
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
    let multiple_math = bank
        .sources()
        .iter()
        .filter(|source| source.state.type_id() == math::TYPE_ID)
        .take(2)
        .count()
        > 1;
    let multiple_mix = bank
        .sources()
        .iter()
        .filter(|source| source.state.type_id() == mix::TYPE_ID)
        .take(2)
        .count()
        > 1;
    for (slot, source) in bank.sources().iter().enumerate() {
        let source_id = source.id;
        let kind = match source.state.type_id() {
            lfo::TYPE_ID => SourceKind::Lfo,
            follower::TYPE_ID => SourceKind::Follower,
            manual::TYPE_ID => SourceKind::Macro,
            macro_four::TYPE_ID => SourceKind::MacroFour,
            random::TYPE_ID => SourceKind::Random,
            audio_rate::TYPE_ID => SourceKind::AudioRate,
            pressure::TYPE_ID => SourceKind::NotePressure,
            manual_button::TYPE_ID => SourceKind::Button,
            math::TYPE_ID => SourceKind::Math,
            buttons::TYPE_ID => SourceKind::Buttons,
            mix::TYPE_ID => SourceKind::Mix,
            note_counter::TYPE_ID => SourceKind::NoteCounter,
            ramp::TYPE_ID => SourceKind::Ramp,
            ahd_release::TYPE_ID => SourceKind::AhdRelease,
            adsr::TYPE_ID => SourceKind::Adsr,
            steps::TYPE_ID => SourceKind::Steps,
            _ => continue,
        };
        let label = kind.label();
        let title = if (multiple_macros && matches!(kind, SourceKind::Macro))
            || (multiple_lfos && matches!(kind, SourceKind::Lfo))
            || (multiple_math && matches!(kind, SourceKind::Math))
            || (multiple_mix && matches!(kind, SourceKind::Mix))
        {
            format!("{label} {}", slot + 1)
        } else {
            label.to_owned()
        };
        let source_key = |suffix: &str| format!("{label}-{}-{suffix}", source_id.value());
        let map_control = |cx: &mut Context<EditorView>, key, source| {
            let mapping = selected == Some(source);
            action(key, theme::text(if mapping { "done" } else { "map" })).on_click(cx.listener(
                move |view, _, _, cx| {
                    cx.stop_propagation();
                    if view.project_generation != generation {
                        return;
                    }
                    view.device_panel.mapping_source = (!mapping).then_some((id, source));
                    cx.notify();
                },
            ))
        };
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
            .children(
                (!matches!(kind, SourceKind::MacroFour | SourceKind::Buttons))
                    .then(|| map_control(cx, source_key("map"), MappingOutput::main(source_id))),
            )
            .children(
                (!matches!(kind, SourceKind::NotePressure)
                    && bank.sources().len() < hex_project::MAX_MODULATORS_PER_DEVICE)
                    .then(|| {
                        button(
                            cx,
                            source_key("duplicate"),
                            theme::text("duplicate"),
                            Edit::DuplicateSource(source_id),
                        )
                    }),
            )
            .child(button(
                cx,
                source_key("remove"),
                theme::text("×"),
                Edit::RemoveSource(source_id),
            ));
        if matches!(kind, SourceKind::Button)
            && let Ok(on) = manual_button::decode(&source.state)
        {
            row = row.child(button(
                cx,
                source_key("value"),
                theme::text(if on { "output on" } else { "output off" }),
                Edit::ToggleButton(source_id),
            ));
        }
        if let SourceKind::Buttons = kind
            && let Ok(settings) = buttons::Settings::decode(&source.state)
        {
            for (index, on) in settings.on.into_iter().enumerate() {
                let number = index as u32 + 1;
                let Ok(output) = ModulationOutputId::new(number) else {
                    continue;
                };
                let mapped = MappingOutput {
                    source: source_id,
                    output,
                };
                let label = if on { "on" } else { "off" };
                row = row.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.0))
                        .child(button(
                            cx,
                            source_key(&format!("{number}-toggle")),
                            theme::text(format!("{number} {label}")),
                            Edit::ToggleButtons(mapped),
                        ))
                        .children([map_control(
                            cx,
                            source_key(&format!("{number}-map")),
                            mapped,
                        )]),
                );
            }
        }
        if let SourceKind::NoteCounter = kind
            && let Ok(settings) = note_counter::Settings::decode(&source.state)
        {
            for (label, value, edit) in [
                (
                    "steps",
                    settings.steps,
                    CounterEdit::Steps as fn(i16) -> CounterEdit,
                ),
                ("increment", settings.increment, CounterEdit::Increment),
            ] {
                row = row.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.0))
                        .child(theme::text(format!("{label} {value}")))
                        .child(button(
                            cx,
                            source_key(&format!("{label}-less")),
                            theme::text("−"),
                            Edit::NoteCounter(source_id, edit(-1)),
                        ))
                        .child(button(
                            cx,
                            source_key(&format!("{label}-more")),
                            theme::text("+"),
                            Edit::NoteCounter(source_id, edit(1)),
                        )),
                );
            }
            row = row.child(button(
                cx,
                source_key("polarity"),
                theme::text(if settings.bipolar {
                    "bipolar"
                } else {
                    "unipolar"
                }),
                Edit::NoteCounter(source_id, CounterEdit::TogglePolarity),
            ));
        }
        if let SourceKind::Ramp = kind
            && let Ok(settings) = ramp::Settings::decode(&source.state)
        {
            let duration = duration_text(settings.duration_micros);
            let (curve, next) = curve_control(settings.curve);
            row = row
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.0))
                        .child(button(
                            cx,
                            source_key("shorter"),
                            theme::text("−"),
                            Edit::Ramp(source_id, RampEdit::Duration { longer: false }),
                        ))
                        .child(theme::text(duration))
                        .child(button(
                            cx,
                            source_key("longer"),
                            theme::text("+"),
                            Edit::Ramp(source_id, RampEdit::Duration { longer: true }),
                        )),
                )
                .child(button(
                    cx,
                    source_key("curve"),
                    theme::text(curve),
                    Edit::Ramp(source_id, RampEdit::Curve(next)),
                ))
                .child(button(
                    cx,
                    source_key("direction"),
                    theme::text(if settings.falling {
                        "falling"
                    } else {
                        "rising"
                    }),
                    Edit::Ramp(source_id, RampEdit::ToggleDirection),
                ))
                .child(button(
                    cx,
                    source_key("loop"),
                    theme::text(if settings.looping { "loop" } else { "one shot" }),
                    Edit::Ramp(source_id, RampEdit::ToggleLoop),
                ));
        }
        let envelope_time = |cx: &mut Context<EditorView>, name: &str, micros, edits: [Edit; 2]| {
            div()
                .flex()
                .items_center()
                .gap(px(2.0))
                .child(theme::text(format!("{name} {}", duration_text(micros))))
                .child(button(
                    cx,
                    source_key(&format!("{name}-shorter")),
                    theme::text("−"),
                    edits[0],
                ))
                .child(button(
                    cx,
                    source_key(&format!("{name}-longer")),
                    theme::text("+"),
                    edits[1],
                ))
        };
        if let SourceKind::AhdRelease = kind
            && let Ok(settings) = ahd_release::Settings::decode(&source.state)
        {
            for (index, (name, curve_index)) in
                [("attack", Some(0)), ("hold", None), ("decay", Some(1))]
                    .into_iter()
                    .enumerate()
            {
                let mut controls = envelope_time(
                    cx,
                    name,
                    settings.durations_micros[index],
                    [false, true].map(|longer| {
                        Edit::AhdRelease(
                            source_id,
                            AhdEdit::Duration {
                                segment: index,
                                longer,
                            },
                        )
                    }),
                );
                if let Some(curve_index) = curve_index {
                    let (curve, next) = curve_control(settings.curves[curve_index]);
                    controls = controls.child(button(
                        cx,
                        source_key(&format!("{name}-curve")),
                        theme::text(curve),
                        Edit::AhdRelease(
                            source_id,
                            AhdEdit::Curve {
                                index: curve_index,
                                curve: next,
                            },
                        ),
                    ));
                }
                row = row.child(controls);
            }
            row = row.child(button(
                cx,
                source_key("single-trigger"),
                theme::text(if settings.single_trigger {
                    "single trigger"
                } else {
                    "retrigger"
                }),
                Edit::AhdRelease(source_id, AhdEdit::ToggleSingleTrigger),
            ));
        }
        if let SourceKind::Adsr = kind
            && let Ok(settings) = adsr::Settings::decode(&source.state)
        {
            for (segment, name) in ["attack", "decay", "release"].into_iter().enumerate() {
                let controls = envelope_time(
                    cx,
                    name,
                    settings.durations_micros[segment],
                    [false, true].map(|longer| {
                        Edit::Adsr(source_id, AdsrEdit::Duration { segment, longer })
                    }),
                );
                row = row.child(controls);
            }
            row = row
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.0))
                        .child(theme::text(format!(
                            "sustain {:.0}%",
                            settings.sustain * 100.0
                        )))
                        .child(button(
                            cx,
                            source_key("sustain-less"),
                            theme::text("−"),
                            Edit::Adsr(source_id, AdsrEdit::Sustain(-0.05)),
                        ))
                        .child(button(
                            cx,
                            source_key("sustain-more"),
                            theme::text("+"),
                            Edit::Adsr(source_id, AdsrEdit::Sustain(0.05)),
                        )),
                )
                .child(button(
                    cx,
                    source_key("single-trigger"),
                    theme::text(if settings.single_trigger {
                        "single trigger"
                    } else {
                        "retrigger"
                    }),
                    Edit::Adsr(source_id, AdsrEdit::ToggleSingleTrigger),
                ));
        }
        if let SourceKind::Steps = kind
            && let Ok(settings) = steps::Settings::decode(&source.state)
        {
            let direction = match settings.direction {
                steps::Direction::Forward => "forward",
                steps::Direction::Backward => "backward",
                steps::Direction::PingPong => "ping pong",
            };
            if is_effect {
                let mode = match settings.timing {
                    steps::Timing::FreeMicros(_) => "free",
                    steps::Timing::Arrangement { .. } => "arrangement",
                };
                row = row.child(button(
                    cx,
                    source_key("timing-mode"),
                    theme::text(mode),
                    Edit::Steps(source_id, StepsEdit::ToggleTiming),
                ));
            }
            let time_edit = |longer| Edit::Steps(source_id, StepsEdit::Time { longer });
            let timing = match settings.timing {
                steps::Timing::FreeMicros(micros) => {
                    envelope_time(cx, "step time", micros, [false, true].map(time_edit))
                }
                steps::Timing::Arrangement { steps_per_beat } => {
                    let timing = source_key("arrangement-time");
                    div()
                        .debug_selector(move || timing.clone())
                        .flex()
                        .items_center()
                        .gap(px(2.0))
                        .child(button(
                            cx,
                            source_key("division-shorter"),
                            theme::text("−"),
                            time_edit(false),
                        ))
                        .child(theme::text(format!("1/{}", 4 * steps_per_beat)))
                        .child(button(
                            cx,
                            source_key("division-longer"),
                            theme::text("+"),
                            time_edit(true),
                        ))
                }
            };
            row = row
                .children([timing])
                .child(button(
                    cx,
                    source_key("direction"),
                    theme::text(direction),
                    Edit::Steps(source_id, StepsEdit::CycleDirection),
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.0))
                        .child(theme::text(format!("{} steps", settings.pattern.len())))
                        .child(button(
                            cx,
                            source_key("length-less"),
                            theme::text("−"),
                            Edit::Steps(source_id, StepsEdit::Length { add: false }),
                        ))
                        .child(button(
                            cx,
                            source_key("length-more"),
                            theme::text("+"),
                            Edit::Steps(source_id, StepsEdit::Length { add: true }),
                        )),
                );
            let mut pattern = div().w_full().flex().flex_wrap().gap(px(3.0));
            for (index, value) in settings.pattern.into_iter().enumerate() {
                let number = index + 1;
                let amount = f32::from(value) / f32::from(i16::MAX) * 100.0;
                pattern = pattern.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.0))
                        .child(theme::text(format!("{number} {amount:+.0}%")))
                        .child(button(
                            cx,
                            source_key(&format!("step-{number}-less")),
                            theme::text("−"),
                            Edit::Steps(
                                source_id,
                                StepsEdit::Value {
                                    index,
                                    delta: -3277,
                                },
                            ),
                        ))
                        .child(button(
                            cx,
                            source_key(&format!("step-{number}-more")),
                            theme::text("+"),
                            Edit::Steps(source_id, StepsEdit::Value { index, delta: 3277 }),
                        )),
                );
            }
            row = row.children([pattern]);
        }
        if matches!(kind, SourceKind::NotePressure) {
            row = row.child(theme::text("one carrier"));
        }
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
            let rate_control = if let Some(control) = input_mapping_control(
                cx,
                format!("map-lfo-rate-{}-{}", id.value(), source_id.value()),
                &rate_text,
                source_id,
                MappingTarget::LfoRate(source_id),
                destination,
            ) {
                control.into_any_element()
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
            if let Some(control) = input_mapping_control(
                cx,
                format!("map-macro-{}-{}", id.value(), source_id.value()),
                &value,
                source_id,
                MappingTarget::Macro(source_id),
                macro_destination(source_id),
            ) {
                row = row.children([control]);
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
        if let SourceKind::MacroFour = kind
            && let Ok(settings) = macro_four::Settings::decode(&source.state)
        {
            let mut controls = div().w_full().flex().flex_wrap().gap(px(3.0));
            for (parameter, value) in macro_four::VALUE_PARAMETERS
                .into_iter()
                .zip(settings.values)
            {
                let Some(output) = macro_four::output_for_parameter(parameter) else {
                    continue;
                };
                let number = output.value();
                let key = |suffix| source_key(&format!("{number}-{suffix}"));
                let value = format!("{number} · {:.0}%", value * 100.0);
                let destination = macro_four_destination(source_id, parameter);
                let value_control = if let Some(control) = input_mapping_control(
                    cx,
                    format!("map-macro-4-{}-{}-{number}", id.value(), source_id.value()),
                    &value,
                    source_id,
                    MappingTarget::MacroFour(source_id, parameter),
                    destination,
                ) {
                    control.into_any_element()
                } else {
                    theme::text(value).into_any_element()
                };
                controls = controls.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.0))
                        .children([value_control])
                        .child(button(
                            cx,
                            key("less"),
                            theme::text("−"),
                            Edit::MacroFourValue(source_id, parameter, -0.05),
                        ))
                        .child(button(
                            cx,
                            key("more"),
                            theme::text("+"),
                            Edit::MacroFourValue(source_id, parameter, 0.05),
                        ))
                        .children([map_control(
                            cx,
                            key("map"),
                            MappingOutput {
                                source: source_id,
                                output,
                            },
                        )]),
                );
            }
            row = row.child(controls);
        }
        let composition = if let SourceKind::Math = kind
            && let Ok(settings) = math::Settings::decode(&source.state)
        {
            use math::Operation;
            let (label, next) = match settings.operation {
                Operation::Multiply => ("multiply", Operation::Add),
                Operation::Add => ("add", Operation::Subtract),
                Operation::Subtract => ("subtract", Operation::Minimum),
                Operation::Minimum => ("min", Operation::Maximum),
                Operation::Maximum => ("max", Operation::Multiply),
            };
            row = row.child(button(
                cx,
                source_key("operation"),
                theme::text(label),
                Edit::MathOperation(source_id, next),
            ));
            [
                Some((math::INPUT_A, settings.inputs[0])),
                Some((math::INPUT_B, settings.inputs[1])),
                None,
            ]
        } else if let SourceKind::Mix = kind
            && let Ok(settings) = mix::Settings::decode(&source.state)
        {
            [
                Some((mix::INPUT_A, settings.inputs[0])),
                Some((mix::INPUT_B, settings.inputs[1])),
                Some((mix::BLEND, settings.blend)),
            ]
        } else {
            [None; 3]
        };
        let module_type = source.state.type_id();
        for (parameter, value) in composition.into_iter().flatten() {
            let Some(name) = composition_input_name(module_type, parameter) else {
                continue;
            };
            let value = if name == "blend" {
                format!("{name} {:.0}%", value * 100.0)
            } else {
                format!("{name} {:+.0}%", value * 100.0)
            };
            let destination = composition_destination(source_id, module_type, parameter);
            let value_control = if let Some(control) = input_mapping_control(
                cx,
                format!("map-{label}-{name}-{}-{}", id.value(), source_id.value()),
                &value,
                source_id,
                MappingTarget::CompositionInput(source_id, parameter),
                destination,
            ) {
                control.into_any_element()
            } else {
                theme::text(value).into_any_element()
            };
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .children([value_control])
                    .child(button(
                        cx,
                        source_key(&format!("{name}-less")),
                        theme::text("−"),
                        Edit::CompositionValue(source_id, parameter, -0.05),
                    ))
                    .child(button(
                        cx,
                        source_key(&format!("{name}-more")),
                        theme::text("+"),
                        Edit::CompositionValue(source_id, parameter, 0.05),
                    )),
            );
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
                    .and_then(|view_data| {
                        view_data
                            .parameters
                            .iter()
                            .find(|row| row.descriptor.id == parameter)
                    })
                    .map_or("unavailable", |row| row.descriptor.label)
                    .to_owned(),
                ModulationDestination::Device {
                    target: DeviceAutomationTarget::Vst3 { parameter },
                    ..
                } => vst3_parameters::mapping_label(view, id, parameter).map_or_else(
                    || format!("parameter {parameter}"),
                    hex_session::service::text::verbatim,
                ),
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
                ModulationDestination::Modulator {
                    source,
                    module_type: macro_four::TYPE_ID,
                    parameter,
                } => {
                    let Some(slot) = bank.sources().iter().position(|slot| slot.id == source)
                    else {
                        continue;
                    };
                    format!("macro-4 {} control {}", slot + 1, parameter.value())
                }
                ModulationDestination::Modulator {
                    source,
                    module_type,
                    parameter,
                } if matches!(module_type, math::TYPE_ID | mix::TYPE_ID) => {
                    let Some(slot) = bank.sources().iter().position(|slot| slot.id == source)
                    else {
                        continue;
                    };
                    let Some(input) = composition_input_name(module_type, parameter) else {
                        continue;
                    };
                    if module_type == math::TYPE_ID {
                        if multiple_math {
                            format!("math {} input {input}", slot + 1)
                        } else {
                            format!("math input {input}")
                        }
                    } else if multiple_mix {
                        format!("mix {} {input}", slot + 1)
                    } else {
                        format!("mix {input}")
                    }
                }
                _ => continue,
            };
            let label = if matches!(kind, SourceKind::MacroFour | SourceKind::Buttons) {
                format!("{} → {label}", route.output.value())
            } else {
                label
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
                    .children(
                        (!manual_button::uses_identity_range(source.state.type_id())).then(|| {
                            button(cx, key("range"), theme::text(range), Edit::Range(route.id))
                        }),
                    )
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
            .max_h(px(78.0))
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

pub(super) fn vst3_target_overlay(
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

fn mapped_target_overlay(
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

fn mapping_button(
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

fn depth_text(depth: f32) -> String {
    format!("{:+.0}%", depth * 100.0)
}

pub(super) fn vst3_destination(
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

fn is_pressure_source(sources: &[Modulator], id: ModulationSourceId) -> bool {
    sources
        .iter()
        .any(|source| source.id == id && source.state.type_id() == pressure::TYPE_ID)
}

fn is_button_source(sources: &[Modulator], id: ModulationSourceId) -> bool {
    sources
        .iter()
        .any(|source| source.id == id && manual_button::uses_identity_range(source.state.type_id()))
}

fn unipolar_range() -> Result<ModulationRange, Invalid> {
    Ok(ModulationRange {
        start: ModulationAmount::new(0.0)?,
        end: ModulationAmount::new(1.0)?,
    })
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
        Edit::Add(kind) => {
            if kind.required_outputs() > hex_engine::device_available_modulation_outputs(device) {
                return Err(Invalid("device modulator output capacity reached"));
            }
            if matches!(kind, SourceKind::NotePressure)
                && !hex_engine::project_device_can_add_note_pressure(project, id)
            {
                return Err(Invalid(
                    "one note pressure source is supported on an fm4 receiver",
                ));
            }
            if matches!(
                kind,
                SourceKind::NoteCounter
                    | SourceKind::Ramp
                    | SourceKind::AhdRelease
                    | SourceKind::Adsr
            ) && !hex_engine::project_device_has_modulation_note_events(project, id)
            {
                return Err(Invalid("this modulator requires instrument note events"));
            }
            sources.push(Modulator {
                color: Default::default(),
                id: ModulationSourceId::new(project.next_identity()).map_err(unsupported)?,
                enabled: true,
                state: kind
                    .default_state(project.next_identity())
                    .map_err(unsupported)?,
            });
        }
        Edit::ToggleSource(id) => {
            let source = find_source(&mut sources, id)?;
            source.enabled = !source.enabled;
        }
        Edit::ToggleButton(id) => {
            let source = find_source(&mut sources, id)?;
            let on = manual_button::decode(&source.state).map_err(unsupported)?;
            source.state = manual_button::encode(!on).map_err(unsupported)?;
        }
        Edit::ToggleButtons(output) => {
            let source = find_source(&mut sources, output.source)?;
            let mut settings = buttons::Settings::decode(&source.state).map_err(unsupported)?;
            let index = buttons::index_for_output(output.output)
                .ok_or(Invalid("button output is unsupported"))?;
            settings.on[index] = !settings.on[index];
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::NoteCounter(id, edit) => {
            let source = find_source(&mut sources, id)?;
            let mut settings =
                note_counter::Settings::decode(&source.state).map_err(unsupported)?;
            match edit {
                CounterEdit::Steps(delta) => {
                    settings.steps = settings.steps.saturating_add_signed(delta).clamp(2, 128);
                }
                CounterEdit::Increment(delta) => {
                    settings.increment = settings
                        .increment
                        .saturating_add_signed(delta)
                        .clamp(1, 128);
                }
                CounterEdit::TogglePolarity => settings.bipolar = !settings.bipolar,
            }
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::Ramp(id, edit) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = ramp::Settings::decode(&source.state).map_err(unsupported)?;
            match edit {
                RampEdit::Duration { longer } => {
                    settings.duration_micros =
                        duration_step(settings.duration_micros, longer).max(ramp::MIN_MICROS);
                }
                RampEdit::Curve(curve) => settings.curve = curve,
                RampEdit::ToggleDirection => settings.falling = !settings.falling,
                RampEdit::ToggleLoop => settings.looping = !settings.looping,
            }
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::AhdRelease(id, edit) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = ahd_release::Settings::decode(&source.state).map_err(unsupported)?;
            match edit {
                AhdEdit::Duration { segment, longer } => {
                    let micros = settings
                        .durations_micros
                        .get_mut(segment)
                        .ok_or(Invalid("envelope segment is unsupported"))?;
                    *micros = duration_step(*micros, longer);
                    if settings.durations_micros == [0; 3] {
                        settings.durations_micros[segment] = ramp::MIN_MICROS;
                    }
                }
                AhdEdit::Curve { index, curve } => {
                    let control = settings
                        .curves
                        .get_mut(index)
                        .ok_or(Invalid("envelope curve is unsupported"))?;
                    *control = curve;
                }
                AhdEdit::ToggleSingleTrigger => settings.single_trigger = !settings.single_trigger,
            }
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::Adsr(id, edit) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = adsr::Settings::decode(&source.state).map_err(unsupported)?;
            match edit {
                AdsrEdit::Duration { segment, longer } => {
                    let duration = settings
                        .durations_micros
                        .get_mut(segment)
                        .ok_or(Invalid("envelope segment is unsupported"))?;
                    *duration = duration_step(*duration, longer);
                }
                AdsrEdit::Sustain(delta) => {
                    settings.sustain = (settings.sustain + delta).clamp(0.0, 1.0);
                }
                AdsrEdit::ToggleSingleTrigger => settings.single_trigger = !settings.single_trigger,
            }
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::Steps(id, edit) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = steps::Settings::decode(&source.state).map_err(unsupported)?;
            match edit {
                StepsEdit::Time { longer } => {
                    settings.timing = match settings.timing {
                        steps::Timing::FreeMicros(micros) => steps::Timing::FreeMicros(
                            duration_step(micros, longer).max(ramp::MIN_MICROS),
                        ),
                        steps::Timing::Arrangement { steps_per_beat } => {
                            let steps_per_beat = if longer {
                                (steps_per_beat / 2).max(1)
                            } else {
                                (steps_per_beat * 2).min(steps::MAX_STEPS_PER_BEAT)
                            };
                            steps::Timing::Arrangement { steps_per_beat }
                        }
                    };
                }
                StepsEdit::ToggleTiming => {
                    settings.timing = match settings.timing {
                        steps::Timing::FreeMicros(_) => steps::Timing::DEFAULT_ARRANGEMENT,
                        steps::Timing::Arrangement { .. } => steps::Timing::DEFAULT_FREE,
                    };
                }
                StepsEdit::Length { add } => {
                    let count = settings.pattern.len();
                    let count = if add { count + 1 } else { count - 1 };
                    settings
                        .pattern
                        .resize(count.clamp(1, steps::MAX_MODULATION_STEPS), 0);
                }
                StepsEdit::Value { index, delta } => {
                    let value = settings
                        .pattern
                        .get_mut(index)
                        .ok_or(Invalid("step is unsupported"))?;
                    *value = value.saturating_add(delta).max(-i16::MAX);
                }
                StepsEdit::CycleDirection => {
                    settings.direction = match settings.direction {
                        steps::Direction::Forward => steps::Direction::Backward,
                        steps::Direction::Backward => steps::Direction::PingPong,
                        steps::Direction::PingPong => steps::Direction::Forward,
                    };
                }
            }
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::DuplicateSource(id) => {
            if is_pressure_source(&sources, id) {
                return Err(Invalid(
                    "one note pressure source is supported on an fm4 receiver",
                ));
            }
            return bank.duplicate_source(id, project.next_identity());
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
        Edit::MacroFourValue(id, parameter, change) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = macro_four::Settings::decode(&source.state).map_err(unsupported)?;
            let index = macro_four::VALUE_PARAMETERS
                .iter()
                .position(|&candidate| candidate == parameter)
                .ok_or(Invalid("macro-4 control is unsupported"))?;
            let value = settings
                .values
                .get_mut(index)
                .ok_or(Invalid("macro-4 control is unsupported"))?;
            *value = (*value + change).clamp(0.0, 1.0);
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::CompositionValue(id, parameter, change) => {
            let source = find_source(&mut sources, id)?;
            let unsupported_input = Invalid("modulator input is unsupported");
            source.state = match source.state.type_id() {
                math::TYPE_ID => {
                    let mut settings =
                        math::Settings::decode(&source.state).map_err(unsupported)?;
                    let value = match parameter {
                        math::INPUT_A => &mut settings.inputs[0],
                        math::INPUT_B => &mut settings.inputs[1],
                        _ => return Err(unsupported_input),
                    };
                    *value = (*value + change).clamp(-1.0, 1.0);
                    settings.encode()
                }
                mix::TYPE_ID => {
                    let mut settings = mix::Settings::decode(&source.state).map_err(unsupported)?;
                    let (value, minimum) = match parameter {
                        mix::INPUT_A => (&mut settings.inputs[0], -1.0),
                        mix::INPUT_B => (&mut settings.inputs[1], -1.0),
                        mix::BLEND => (&mut settings.blend, 0.0),
                        _ => return Err(unsupported_input),
                    };
                    *value = (*value + change).clamp(minimum, 1.0);
                    settings.encode()
                }
                _ => return Err(unsupported_input),
            }
            .map_err(unsupported)?;
        }
        Edit::MathOperation(id, operation) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = math::Settings::decode(&source.state).map_err(unsupported)?;
            settings.operation = operation;
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
        Edit::Map(source, target, depth) => {
            if !sources.iter().any(|slot| source.valid_for(slot)) {
                return Err(Invalid("modulator source output is unsupported"));
            }
            let pressure = is_pressure_source(&sources, source.source);
            if pressure
                && (!matches!(target, MappingTarget::Device(id) if hex_engine::fm4_note_pressure_parameter_supported(id))
                    || routes.iter().any(|route| source.owns(route)))
            {
                return Err(Invalid("note pressure supports one carrier assignment"));
            }
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
                MappingTarget::Vst3(target) => {
                    vst3_destination(project, id, target.parameter, target.class)?
                }
                MappingTarget::HostGain(stage) => {
                    let available = hex_engine::project_device_host_gain_modulation_supported(
                        project, id,
                    ) || (matches!(stage, HostGainTarget::Output)
                        && hex_engine::project_device_instrument_output_modulation_supported(
                            project, id,
                        ));
                    if !available {
                        return Err(Invalid("this device does not support this host gain stage"));
                    }
                    ModulationDestination::Device {
                        device_type: device.state().type_id(),
                        target: stage.mapping().0,
                        vst3_class: None,
                    }
                }
                MappingTarget::Macro(target) => macro_destination(target),
                MappingTarget::MacroFour(target, parameter) => {
                    let target = find_source(&mut sources, target)?;
                    macro_four::Settings::decode(&target.state).map_err(unsupported)?;
                    macro_four::output_for_parameter(parameter)
                        .ok_or(Invalid("macro-4 control is unsupported"))?;
                    macro_four_destination(target.id, parameter)
                }
                MappingTarget::CompositionInput(target, parameter) => {
                    let target = find_source(&mut sources, target)?;
                    let module_type = target.state.type_id();
                    match module_type {
                        math::TYPE_ID => {
                            math::Settings::decode(&target.state).map_err(unsupported)?;
                        }
                        mix::TYPE_ID => {
                            mix::Settings::decode(&target.state).map_err(unsupported)?;
                        }
                        _ => return Err(Invalid("modulator input is unsupported")),
                    }
                    composition_input_name(module_type, parameter)
                        .ok_or(Invalid("modulator input is unsupported"))?;
                    composition_destination(target.id, module_type, parameter)
                }
                MappingTarget::LfoRate(target) => {
                    let source = find_source(&mut sources, target)?;
                    lfo::Settings::decode(&source.state).map_err(unsupported)?;
                    lfo_rate_destination(target)
                }
            };
            if routes
                .iter()
                .any(|route| source.owns(route) && route.destination == destination)
            {
                return Err(Invalid("this source already modulates the parameter"));
            }
            routes.push(DeviceModulationRoute {
                id: ModulationRouteId::new(project.next_identity())?,
                source: source.source,
                output: source.output,
                destination,
                amount: ModulationAmount::new(depth)?,
                range: if pressure {
                    unipolar_range()?
                } else {
                    ModulationRange::default()
                },
                enabled: true,
            });
        }
        Edit::SetDepth(id, depth) => find_route(&mut routes, id)?.amount = depth,
        Edit::Adjust(id, change) => {
            let route = find_route(&mut routes, id)?;
            route.amount = ModulationAmount::new((route.amount.get() + change).clamp(-1.0, 1.0))?;
        }
        Edit::Range(id) => {
            let route = find_route(&mut routes, id)?;
            if is_button_source(&sources, route.source) {
                return Err(Invalid("button assignments use the original on/off values"));
            }
            route.range = if route.range == ModulationRange::default() {
                unipolar_range()?
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

#[cfg(all(test, not(feature = "ui-test-support")))]
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
            Edit::Map(
                MappingOutput::main(first),
                MappingTarget::Macro(second),
                DEFAULT_MAPPING_DEPTH,
            ),
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
