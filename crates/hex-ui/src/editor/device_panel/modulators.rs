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

mod apply;
mod overlays;
mod panel;
mod source_card;

use apply::*;
use overlays::*;
pub(super) use overlays::{target_overlay, vst3_destination, vst3_target_overlay};
pub(super) use panel::{macro_panel, panel, vst3_macro_panel, vst3_panel};
#[cfg(test)]
pub(super) use source_card::source_utility_gain_available;
pub(super) use source_card::{SOURCE_EFFECT_CARD_WIDTH, source_effect_card};

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests;
