//! Transient lane presentation over canonical device envelopes. Selecting or
//! revealing a parameter never authors points. The worker validates source
//! stamps and owns history; released curves retain their exact ordered receipt.
//! Provider descriptors and typed host gains have separate value metadata;
//! reserved widget keys are translated only by the shared binding edge.
//! A press on the curve adds or moves a point. Empty space selects a time
//! range within one lane. Dragging a selected point moves the selected group,
//! and each edit replaces the lane in one undoable request.
#[cfg(all(test, feature = "ui-test-support"))]
pub(super) mod host_tests;
mod paint;
mod plot_geometry;
mod range_edit;
pub(super) use range_edit::RangeEdit;
mod read_selection;
#[cfg(test)]
mod tests;
#[cfg(all(test, not(feature = "ui-test-support")))]
#[path = "device_automation/unit_tests.rs"]
mod unit_tests;
mod vst3;
use super::pending_edit::{Disposition, Receipt};
use super::{EditorView, ViewState, device_panel::DevicePanelOwner as Owner};
use crate::context_menu::{MenuItem, PopupMenu};
use gpui::{Context, Pixels, Point, SharedString, Window};
use hex_engine::NativeDeviceClass;
use hex_lattice_contract::{
    DeviceAutomationTarget, DeviceTypeId, HostGain, ParameterDescriptor, ParameterKind,
    ParameterSemantics, ParameterValue,
};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Device, DeviceAutomation,
    DeviceId, Project,
};
use hex_session::service::{Event, PluginParameterRow, Request, vst3_automation_class};
pub(super) use paint::{overlay, rows};

pub(super) const LANE_HEIGHT: f32 = 64.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Target {
    pub(super) generation: u64,
    pub(super) owner: Owner,
    pub(super) device: DeviceId,
    pub(super) kind: DeviceAutomationTarget,
    pub(super) class: NativeDeviceClass,
    provider: Option<DeviceTypeId>,
    vst3_class: Option<[u8; 16]>,
}

#[derive(Clone)]
pub(super) struct Entry {
    pub(super) target: Target,
    pub(super) label: SharedString,
    metadata: EntryMetadata,
    pub(super) base: f32,
    end: Option<BeatTime>,
}

fn automation_extent(lane: &DeviceAutomation) -> Option<BeatTime> {
    lane.points
        .last()
        .map(|point| point.beat)
        .max(lane.touch.last().map(|span| span.end))
}

#[derive(Clone, Copy)]
enum EntryMetadata {
    Provider {
        descriptor: ParameterDescriptor,
        semantics: ParameterSemantics,
    },
    HostGain,
    Vst3 {
        steps: u32,
    },
}
#[derive(Clone, Copy)]
pub(super) struct TargetKey(DeviceAutomationTarget);
impl std::fmt::Display for TargetKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            DeviceAutomationTarget::Parameter(id) => write!(f, "{}", id.value()),
            DeviceAutomationTarget::HostInputGain => f.write_str("host-input"),
            DeviceAutomationTarget::HostOutputGain => f.write_str("host-output"),
            DeviceAutomationTarget::Vst3 { parameter, .. } => write!(f, "vst3-{parameter}"),
        }
    }
}

#[derive(Clone)]
pub(super) enum Action {
    Show(Entry, bool),
    Pick {
        generation: u64,
        owner: Owner,
    },
    Device {
        generation: u64,
        owner: Owner,
        device: DeviceId,
        class: NativeDeviceClass,
        provider: DeviceTypeId,
        label: SharedString,
    },
    Reveal {
        generation: u64,
        owner: Owner,
        used_only: bool,
    },
    Hide(Target),
    HidePreview(Target),
    Clear(Target),
    Reenable(Target),
    Override(Target),
    ToggleNativeArm(Target),
    ToggleVst3Arm {
        target: Target,
        revision: u64,
    },
    ToggleVst3EditorArm {
        target: Target,
        revision: u64,
    },
    ToggleVst3EditorGroupMember {
        target: Target,
        revision: u64,
    },
    ToggleVst3EditorGroupArm {
        target: Target,
        revision: u64,
    },
    Shape(Target, AutomationInterpolation),
}

/// Selected beats within one lane, sorted, unique and non-empty.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Selection {
    target: Target,
    beats: Vec<BeatTime>,
}

impl Selection {
    fn one(target: Target, beat: BeatTime) -> Self {
        Self {
            target,
            beats: vec![beat],
        }
    }
    /// `None` for an empty set, so an empty marquee clears the selection.
    fn new(target: Target, mut beats: Vec<BeatTime>) -> Option<Self> {
        beats.sort_unstable();
        beats.dedup();
        (!beats.is_empty()).then_some(Self { target, beats })
    }
    fn contains(&self, target: Target, beat: BeatTime) -> bool {
        self.target == target && self.beats.binary_search(&beat).is_ok()
    }
    fn beats_in(&self, target: Target) -> &[BeatTime] {
        if self.target == target {
            &self.beats
        } else {
            &[]
        }
    }
}

/// Held point press. `group` holds the original selected points moving with
/// the pressed `initial` point; `points` is the current candidate lane.
#[derive(Clone)]
pub(super) struct Draft {
    target: Target,
    revision: u64,
    points: Vec<AutomationPoint>,
    group: Vec<AutomationPoint>,
    rest: Vec<AutomationPoint>,
    /// A stationary click on a grouped point collapses the selection to it.
    collapse: bool,
    initial: AutomationPoint,
    point: AutomationPoint,
    viewport: super::viewport::Viewport,
    rect: [f32; 4],
    origin: [f32; 2],
}

/// A time range is lane-local and retains its bounds even if it contains no points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RangeSelection {
    target: Target,
    start: BeatTime,
    end: BeatTime,
}

impl RangeSelection {
    fn selection(self, points: &[AutomationPoint]) -> Option<Selection> {
        let first = points.partition_point(|point| point.beat < self.start);
        let last = points.partition_point(|point| point.beat < self.end);
        Selection::new(
            self.target,
            points[first..last].iter().map(|point| point.beat).collect(),
        )
    }
}

#[derive(Clone)]
pub(super) struct MarqueeDraft {
    target: Target,
    revision: u64,
    anchor: BeatTime,
    base: Option<Selection>,
    base_range: Option<RangeSelection>,
    viewport: super::viewport::Viewport,
    rect: [f32; 4],
}
struct Pending {
    target: Target,
    revision: u64,
    points: Vec<AutomationPoint>,
    receipt: Receipt,
    select_range: Option<RangeSelection>,
    cut_clipboard: Option<range_edit::Clipboard>,
}

#[derive(Default)]
pub(super) struct Lanes {
    main: Vec<Entry>,
    extra: Vec<Entry>,
    known: Vec<Entry>,
    selected: Option<Selection>,
    range: Option<RangeSelection>,
    clipboard: Option<range_edit::Clipboard>,
    draft: Option<Draft>,
    marquee: Option<MarqueeDraft>,
    pending: Option<Pending>,
    read_pending: Option<read_selection::PendingRead>,
    read_summary: read_selection::ReadSummary,
    stamp: Option<(u64, u64)>,
    windows: Vec<(Owner, std::ops::Range<usize>)>,
}

fn devices(project: &Project, owner: Owner) -> Vec<(&Device, NativeDeviceClass)> {
    match owner {
        Owner::Track(id) => project
            .track(id)
            .map(|track| {
                track
                    .devices()
                    .iter()
                    .map(|device| (device, NativeDeviceClass::Effect))
                    .chain(
                        track
                            .instrument()
                            .map(|device| (device, NativeDeviceClass::Instrument)),
                    )
                    .collect()
            })
            .unwrap_or_default(),
        Owner::Main => project
            .main_devices()
            .iter()
            .map(|device| (device, NativeDeviceClass::Effect))
            .collect(),
        Owner::Return(id) => project
            .return_tracks()
            .iter()
            .find(|track| track.id() == id)
            .map(|track| {
                track
                    .devices()
                    .iter()
                    .map(|device| (device, NativeDeviceClass::Effect))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn owner_exists(project: &Project, owner: Owner) -> bool {
    match owner {
        Owner::Main => true,
        Owner::Track(id) => project.track(id).is_some(),
        Owner::Return(id) => project.return_tracks().iter().any(|track| track.id() == id),
    }
}

impl Target {
    fn identity_current(self, project: &Project, generation: u64) -> Option<&Device> {
        if self.generation != generation {
            return None;
        }
        let (device, class) = match self.owner {
            Owner::Track(id) => {
                let track = project.track(id)?;
                if let Some(device) = track
                    .devices()
                    .iter()
                    .find(|device| device.id() == self.device)
                {
                    (device, NativeDeviceClass::Effect)
                } else {
                    (
                        track
                            .instrument()
                            .filter(|device| device.id() == self.device)?,
                        NativeDeviceClass::Instrument,
                    )
                }
            }
            Owner::Main => (
                project
                    .main_devices()
                    .iter()
                    .find(|device| device.id() == self.device)?,
                NativeDeviceClass::Effect,
            ),
            Owner::Return(id) => (
                project
                    .return_tracks()
                    .iter()
                    .find(|track| track.id() == id)?
                    .devices()
                    .iter()
                    .find(|device| device.id() == self.device)?,
                NativeDeviceClass::Effect,
            ),
        };
        (class == self.class
            && self
                .provider
                .is_none_or(|provider| device.state().type_id() == provider)
            && self
                .vst3_class
                .is_none_or(|class| vst3_automation_class(device.state()) == Some(class)))
        .then_some(device)
    }
    pub(super) fn current(self, project: &Project, generation: u64) -> Option<&Device> {
        let device = self.identity_current(project, generation)?;
        match self.kind {
            DeviceAutomationTarget::Vst3 { .. } => self.vst3_class.is_some().then_some(device),
            _ => hex_engine::project_device_automation_target_supported(
                project,
                self.device,
                self.kind,
            )
            .then_some(device),
        }
    }
    pub(super) fn key(self) -> TargetKey {
        TargetKey(self.kind)
    }
}

impl Entry {
    fn read(
        project: &Project,
        generation: u64,
        owner: Owner,
        device: DeviceId,
        kind: DeviceAutomationTarget,
    ) -> Option<Self> {
        let inventory = devices(project, owner);
        let (index, (source, class)) = inventory
            .iter()
            .enumerate()
            .find(|(_, (source, _))| source.id() == device)?;
        if !hex_engine::project_device_automation_target_supported(project, device, kind) {
            return None;
        }
        let metadata = hex_engine::native_device_view_for_class(*class, source.state()).ok()?;
        let (entry_metadata, base, label) = match kind {
            DeviceAutomationTarget::Vst3 { .. } => return None,
            DeviceAutomationTarget::Parameter(parameter) => {
                let value = metadata
                    .parameters
                    .iter()
                    .find(|value| value.descriptor.id == parameter)?;
                let semantics = value.semantics?;
                (
                    EntryMetadata::Provider {
                        descriptor: value.descriptor,
                        semantics,
                    },
                    semantics
                        .normalization
                        .normalize(value.descriptor, value.value)
                        .ok()?,
                    value.descriptor.label,
                )
            }
            DeviceAutomationTarget::HostInputGain | DeviceAutomationTarget::HostOutputGain => {
                let (millidb, label) = if kind == DeviceAutomationTarget::HostInputGain {
                    (source.state().input_gain_millidb(), "input gain")
                } else {
                    (source.state().output_gain_millidb(), "output gain")
                };
                (
                    EntryMetadata::HostGain,
                    HostGain::from_millidb(millidb).ok()?.normalized(),
                    label,
                )
            }
        };
        Some(Self {
            target: Target {
                generation,
                owner,
                device,
                kind,
                class: *class,
                provider: kind.provider_parameter().map(|_| source.state().type_id()),
                vst3_class: None,
            },
            label: format!("{} {} · {}", metadata.name, index + 1, label).into(),
            metadata: entry_metadata,
            base,
            end: source.automation_target(kind).and_then(automation_extent),
        })
    }

    fn refreshed(&self, project: &Project, generation: u64) -> Option<Self> {
        let source = self.target.identity_current(project, generation)?;
        if matches!(self.metadata, EntryMetadata::Vst3 { .. }) {
            let mut entry = self.clone();
            entry.end = source
                .automation_target(self.target.kind)
                .and_then(automation_extent);
            return Some(entry);
        }
        Self::read(
            project,
            generation,
            self.target.owner,
            self.target.device,
            self.target.kind,
        )
    }
    /// Lane position of a control-space value; host gains travel in decibels.
    fn normalized(&self, value: ParameterValue) -> Option<f32> {
        match (self.metadata, value) {
            (EntryMetadata::HostGain, ParameterValue::Float(db)) => {
                HostGain::from_db(db).ok().map(HostGain::normalized)
            }
            (EntryMetadata::HostGain | EntryMetadata::Vst3 { .. }, _) => None,
            (
                EntryMetadata::Provider {
                    descriptor,
                    semantics,
                },
                value,
            ) => semantics.normalization.normalize(descriptor, value).ok(),
        }
    }
    fn is_discrete(&self) -> bool {
        if let EntryMetadata::Vst3 { steps } = self.metadata {
            return steps != 0;
        }
        matches!(
            self.metadata,
            EntryMetadata::Provider {
                descriptor: ParameterDescriptor {
                    kind: ParameterKind::Bool | ParameterKind::Choice { .. },
                    ..
                },
                ..
            }
        )
    }
    fn quantize(&self, value: f32) -> Option<f32> {
        match self.metadata {
            EntryMetadata::HostGain => HostGain::from_normalized(value)
                .ok()
                .map(HostGain::normalized),
            EntryMetadata::Vst3 { steps: 0 } => Some(value.clamp(0.0, 1.0)),
            EntryMetadata::Vst3 { steps } => {
                let steps = f64::from(steps);
                Some(((f64::from(value).clamp(0.0, 1.0) * steps).round() / steps) as f32)
            }
            EntryMetadata::Provider {
                descriptor,
                semantics,
            } => {
                let mut canonical = semantics
                    .normalization
                    .denormalize(descriptor, value.clamp(0.0, 1.0))
                    .ok()?;
                if let (
                    hex_lattice_contract::ParameterStepping::Increment(step),
                    ParameterKind::Float { min, max, .. },
                    ParameterValue::Float(value),
                ) = (semantics.stepping, descriptor.kind, canonical)
                {
                    canonical = ParameterValue::Float(
                        (min + ((value - min) / step).round() * step).clamp(min, max),
                    );
                }
                semantics
                    .normalization
                    .normalize(descriptor, canonical)
                    .ok()
            }
        }
    }
    fn format(&self, value: f32) -> String {
        if matches!(self.metadata, EntryMetadata::Vst3 { .. }) {
            return format!("{:.1}%", value * 100.0);
        }
        let EntryMetadata::Provider {
            descriptor,
            semantics,
        } = self.metadata
        else {
            return HostGain::from_normalized(value)
                .map(|gain| format!("{:.3} db", gain.db()))
                .unwrap_or_else(|_| "unavailable".into());
        };
        let Ok(value) = semantics.normalization.denormalize(descriptor, value) else {
            return "unavailable".into();
        };
        match (value, descriptor.kind) {
            (ParameterValue::Bool(value), _) => if value { "on" } else { "off" }.into(),
            (ParameterValue::Choice(index), ParameterKind::Choice { labels }) => labels
                .get(index as usize)
                .copied()
                .unwrap_or("unavailable")
                .into(),
            (ParameterValue::Float(value), ParameterKind::Float { unit, .. }) => {
                if descriptor.presentation
                    == hex_lattice_contract::ParameterPresentation::LinearAmplitudeDecibels
                {
                    if value == 0.0 {
                        "-inf db".into()
                    } else {
                        format!("{:.1} db", 20.0 * value.log10())
                    }
                } else {
                    format!(
                        "{value:.3}{}",
                        unit.map(|unit| format!(" {unit}")).unwrap_or_default()
                    )
                }
            }
            _ => "unavailable".into(),
        }
    }
}

impl Lanes {
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(in crate::editor) fn main_target_for_test(&self) -> Option<Target> {
        self.main.last().map(|entry| entry.target)
    }

    pub(super) fn display_end(&self, project: &Project) -> Option<u64> {
        self.main
            .iter()
            .chain(&self.extra)
            .filter_map(|entry| entry.end)
            .chain(
                self.draft
                    .as_ref()
                    .and_then(|draft| draft.points.last())
                    .map(|point| point.beat),
            )
            .chain(
                self.pending
                    .as_ref()
                    .and_then(|pending| pending.points.last())
                    .map(|point| point.beat),
            )
            .max()
            .and_then(|beat| project.tempo_map().beat_to_frame(beat).ok())
    }
    pub(super) fn pointer_active(&self) -> bool {
        self.draft.is_some() || self.marquee.is_some()
    }
    pub(super) fn point_draft_active(&self) -> bool {
        self.draft.is_some()
    }
    pub(super) fn extra_height(&self, owner: Owner) -> f32 {
        self.extra
            .iter()
            .filter(|entry| entry.target.owner == owner)
            .count() as f32
            * LANE_HEIGHT
    }
    pub(super) fn clear_main(&mut self, owner: Owner) {
        self.main.retain(|entry| entry.target.owner != owner);
        self.selected = None;
        self.range = None;
    }
    pub(super) fn has_main(&self, owner: Owner) -> bool {
        self.main.iter().any(|entry| entry.target.owner == owner)
    }
    fn entry(&self, target: Target) -> Option<&Entry> {
        self.main
            .iter()
            .chain(&self.extra)
            .find(|entry| entry.target == target)
    }
    fn points(&self, state: &ViewState, target: Target) -> Vec<AutomationPoint> {
        self.points_ref(&state.project, target).to_vec()
    }
    /// Displayed points (draft, then pending, then canonical) without copying.
    fn points_ref<'a>(&'a self, project: &'a Project, target: Target) -> &'a [AutomationPoint] {
        if let Some(draft) = self.draft.as_ref().filter(|draft| draft.target == target) {
            return &draft.points;
        }
        if let Some(pending) = self
            .pending
            .as_ref()
            .filter(|pending| pending.target == target)
        {
            return &pending.points;
        }
        project
            .device(target.device)
            .and_then(|device| device.automation_target(target.kind))
            .map_or(&[], |lane| lane.points.as_slice())
    }
}

impl EditorView {
    /// Unautomated lanes follow the bound control frame by frame: the in-flight
    /// gesture, then the admitted manual value, then canonical `base`. Released
    /// gestures keep their snapshot until the exact terminal installs canonical
    /// state, so the line never snaps back. Authored envelopes ignore this value.
    pub(super) fn live_device_automation_base(&mut self, entry: &Entry) -> f32 {
        let parameter = match entry.target.kind {
            DeviceAutomationTarget::Parameter(parameter) => parameter,
            DeviceAutomationTarget::HostInputGain => hex_engine::DEVICE_INPUT_GAIN.id,
            DeviceAutomationTarget::HostOutputGain => hex_engine::DEVICE_OUTPUT_GAIN.id,
            DeviceAutomationTarget::Vst3 { parameter } => {
                let Some(snapshot) = self.device_gesture.snapshot() else {
                    return entry.base;
                };
                if !self.state.as_ref().is_some_and(|state| {
                    entry
                        .target
                        .current(&state.project, self.project_generation)
                        .is_some()
                }) || snapshot.key.generation != entry.target.generation
                    || entry.target.generation != self.project_generation
                    || snapshot.target
                        != (hex_session::device_gesture::GestureTarget::PluginParameter {
                            device: entry.target.device,
                            parameter,
                        })
                {
                    return entry.base;
                }
                return snapshot
                    .plugin_value
                    .map(|value| value.normalized() as f32)
                    .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
                    .unwrap_or(entry.base);
            }
        };
        let key = super::device_panel::ParameterKey {
            device: entry.target.device,
            parameter,
        };
        super::device_panel::parameter_display_override(self, key)
            .and_then(|value| entry.normalized(value))
            .unwrap_or(entry.base)
    }

    /// The device toolbar follows the exact native host lane selected in
    /// Arrangement. The toolbar uses the device panel's existing input owner.
    pub(super) fn selected_automation_trim(&self) -> Option<Target> {
        if !self.automation_view.enabled || self.mode != super::EditorMode::Arrangement {
            return None;
        }
        let owner = self.device_panel.owner?;
        self.automation_view
            .devices
            .main
            .iter()
            .find(|entry| {
                entry.target.owner == owner && matches!(entry.metadata, EntryMetadata::HostGain)
            })
            .map(|entry| entry.target)
    }

    pub(super) fn remember_automation_parameter(
        &mut self,
        device: DeviceId,
        kind: DeviceAutomationTarget,
    ) {
        let Some(owner) = self.device_panel.owner else {
            return;
        };
        let Some(state) = &self.state else {
            return;
        };
        let Some(entry) = Entry::read(&state.project, self.project_generation, owner, device, kind)
        else {
            return;
        };
        let main = &mut self.automation_view.devices.main;
        main.retain(|old| old.target.owner != owner);
        main.push(entry);
    }

    pub(super) fn open_parameter_automation_menu(
        &mut self,
        position: Point<Pixels>,
        device: DeviceId,
        kind: DeviceAutomationTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(owner) = self.device_panel.owner else {
            return false;
        };
        let Some(state) = &self.state else {
            return false;
        };
        let Some(entry) = Entry::read(&state.project, self.project_generation, owner, device, kind)
        else {
            return false;
        };
        if !super::device_panel::finalize_controls(self, cx) {
            return false;
        }
        let utility_mode = entry.target.provider == Some(DeviceTypeId::from_static("hex.utility"))
            && kind
                .provider_parameter()
                .is_some_and(|parameter| matches!(parameter.value(), 3 | 10));
        let mut actions = vec![
            ("show automation", Action::Show(entry.clone(), false)),
            ("add new automation lane", Action::Show(entry.clone(), true)),
        ];
        if self.device_automation_read_selection(entry.target)
            == Some(hex_project::AutomationReadSelection::Base)
        {
            actions.push(("re-enable automation", Action::Reenable(entry.target)));
        }
        actions.extend(self.native_automation_arm_action(&entry));
        self.open_automation_menu(position, actions, window, cx);
        if utility_mode {
            self.append_utility_mode_choice(
                device,
                kind.provider_parameter()
                    .is_some_and(|parameter| parameter.value() == 10),
            );
        }
        true
    }

    fn native_automation_arm_action(&self, entry: &Entry) -> Option<(&'static str, Action)> {
        let target = entry.target;
        let supported = matches!(
            entry.metadata,
            EntryMetadata::HostGain | EntryMetadata::Provider { .. }
        );
        (supported
            && self.state.as_ref().is_some_and(|state| {
                hex_engine::native_parameter_input_target(
                    &state.project,
                    target.device,
                    target.kind,
                )
                .is_ok()
            }))
        .then(|| {
            let label = if self.native_automation_arm_selected(target.device, target.kind) {
                "disarm automation recording"
            } else {
                "arm automation recording"
            };
            (label, Action::ToggleNativeArm(target))
        })
    }

    pub(super) fn open_vst3_automation_menu(
        &mut self,
        position: Point<Pixels>,
        device: DeviceId,
        row: &PluginParameterRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(owner) = self.device_panel.owner else {
            return false;
        };
        let Some((entry, revision)) = self.state.as_ref().and_then(|state| {
            Entry::vst3(&state.project, self.project_generation, owner, device, row)
                .map(|entry| (entry, state.revision))
        }) else {
            return false;
        };
        if !super::device_panel::finalize_controls(self, cx) {
            return false;
        }
        let target = entry.target;
        let mut actions = vec![
            ("show automation", Action::Show(entry.clone(), false)),
            ("add new automation lane", Action::Show(entry, true)),
        ];
        actions.extend(self.device_automation_read_action(target));
        if let (Some(class), DeviceAutomationTarget::Vst3 { parameter }) =
            (target.vst3_class, target.kind)
        {
            let label = if self.vst3_automation_arm_selected(device, class, parameter) {
                "disarm automation recording"
            } else {
                "arm automation recording"
            };
            actions.push((label, Action::ToggleVst3Arm { target, revision }));
            let editor_label = if self.vst3_editor_arm_selected(device, class, parameter) {
                "disarm plug-in editor recording"
            } else {
                "arm plug-in editor recording"
            };
            actions.push((
                editor_label,
                Action::ToggleVst3EditorArm { target, revision },
            ));
            if let Some((member, arm)) = self.vst3_editor_group_menu(device, class, parameter) {
                actions.push((
                    member,
                    Action::ToggleVst3EditorGroupMember { target, revision },
                ));
                if let Some(label) = arm {
                    actions.push((label, Action::ToggleVst3EditorGroupArm { target, revision }));
                }
            }
        }
        self.open_automation_menu(position, actions, window, cx);
        true
    }

    pub(super) fn open_automation_owner_menu(
        &mut self,
        owner: Owner,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_automation_menu(position, Vec::new(), window, cx);
        self.append_automation_menu(owner);
    }

    fn open_automation_menu(
        &mut self,
        position: Point<Pixels>,
        actions: Vec<(&'static str, Action)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous_focus = self
            .context_menu
            .take()
            .and_then(|menu| menu.previous_focus)
            .or_else(|| window.focused(cx));
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.context_menu = Some(PopupMenu {
            scroll: Default::default(),
            position,
            icon_grid: None,
            selected: 0,
            focus,
            previous_focus,
            items: actions
                .into_iter()
                .map(|(label, action)| MenuItem {
                    label,
                    checked: matches!(&action, Action::Show(entry, _) if self.state.as_ref().is_some_and(|state| state.project.device(entry.target.device).and_then(|device| device.automation_target(entry.target.kind)).is_some())),
                    swatch: None,
                    action: super::context_menu::EditorContextAction::Automation(action),
                })
                .collect(),
        });
        cx.notify();
    }

    pub(super) fn append_automation_menu(&mut self, owner: Owner) {
        let generation = self.project_generation;
        if let Some(menu) = &mut self.context_menu {
            for (label, action) in [
                (
                    "automatable parameters…",
                    Action::Pick { generation, owner },
                ),
                (
                    "show used automation lanes",
                    Action::Reveal {
                        generation,
                        owner,
                        used_only: true,
                    },
                ),
                (
                    "show all automation lanes",
                    Action::Reveal {
                        generation,
                        owner,
                        used_only: false,
                    },
                ),
            ] {
                menu.items.push(MenuItem {
                    label,
                    checked: false,
                    swatch: None,
                    action: super::context_menu::EditorContextAction::Automation(action),
                });
            }
        }
    }

    pub(super) fn automation_action_stale(&self, action: &Action) -> bool {
        match action {
            Action::Show(entry, _) => self.state.as_ref().is_none_or(|state| {
                entry
                    .target
                    .identity_current(&state.project, self.project_generation)
                    .is_none()
            }),
            Action::Hide(target)
            | Action::HidePreview(target)
            | Action::Clear(target)
            | Action::Reenable(target)
            | Action::Override(target)
            | Action::ToggleNativeArm(target)
            | Action::Shape(target, _) => self.state.as_ref().is_none_or(|state| {
                target
                    .identity_current(&state.project, self.project_generation)
                    .is_none()
            }),
            Action::ToggleVst3Arm { target, revision }
            | Action::ToggleVst3EditorArm { target, revision }
            | Action::ToggleVst3EditorGroupMember { target, revision }
            | Action::ToggleVst3EditorGroupArm { target, revision } => {
                self.state.as_ref().is_none_or(|state| {
                    state.revision != *revision
                        || target
                            .identity_current(&state.project, self.project_generation)
                            .is_none()
                })
            }
            Action::Pick { generation, owner }
            | Action::Reveal {
                generation, owner, ..
            } => {
                *generation != self.project_generation
                    || self
                        .state
                        .as_ref()
                        .is_none_or(|state| !owner_exists(&state.project, *owner))
            }
            Action::Device {
                generation,
                owner,
                device,
                class,
                provider,
                ..
            } => {
                *generation != self.project_generation
                    || self.state.as_ref().is_none_or(|state| {
                        !devices(&state.project, *owner)
                            .iter()
                            .any(|(source, kind)| {
                                source.id() == *device
                                    && kind == class
                                    && source.state().type_id() == *provider
                            })
                    })
            }
        }
    }

    pub(super) fn automation_menu_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.automation_action_stale(&action) {
            return;
        }
        let position = window.mouse_position();
        match action {
            Action::Show(entry, extra) => {
                if !super::device_panel::finalize_controls(self, cx) {
                    return;
                }
                let Some(state) = &self.state else {
                    return;
                };
                let Some(entry) = entry.refreshed(&state.project, self.project_generation) else {
                    return;
                };
                let target = entry.target;
                self.set_mode(super::EditorMode::Arrangement);
                self.automation_view.enabled = true;
                self.gain_lane.selected = None;
                let lanes = &mut self.automation_view.devices;
                lanes.selected = None;
                lanes.main.retain(|old| old.target.owner != target.owner);
                lanes.main.push(entry.clone());
                if !lanes.known.iter().any(|old| old.target == target) {
                    lanes.known.push(entry.clone());
                }
                if extra && !lanes.extra.iter().any(|old| old.target == target) {
                    lanes.extra.push(entry);
                }
                self.focus.focus(window);
                self.track_scroll.remeasure();
            }
            Action::Pick { owner, .. } => {
                let Some(state) = &self.state else {
                    return;
                };
                let mut actions = Vec::new();
                for (index, (device, class)) in
                    devices(&state.project, owner).into_iter().enumerate()
                {
                    let Ok(metadata) =
                        hex_engine::native_device_view_for_class(class, device.state())
                    else {
                        continue;
                    };
                    if [
                        DeviceAutomationTarget::HostInputGain,
                        DeviceAutomationTarget::HostOutputGain,
                    ]
                    .into_iter()
                    .any(|target| {
                        hex_engine::project_device_automation_target_supported(
                            &state.project,
                            device.id(),
                            target,
                        )
                    }) || metadata.parameters.iter().any(|parameter| {
                        hex_engine::project_device_automation_target_supported(
                            &state.project,
                            device.id(),
                            DeviceAutomationTarget::Parameter(parameter.descriptor.id),
                        )
                    }) {
                        actions.push((
                            "device",
                            Action::Device {
                                generation: self.project_generation,
                                owner,
                                device: device.id(),
                                class,
                                provider: device.state().type_id(),
                                label: format!("{} {}", metadata.name, index + 1).into(),
                            },
                        ));
                    }
                }
                if actions.is_empty() {
                    self.error = Some("no implemented automatable parameters on this track".into());
                } else {
                    self.open_automation_menu(position, actions, window, cx);
                }
            }
            Action::Device {
                owner,
                device,
                class,
                ..
            } => {
                let Some(state) = &self.state else {
                    return;
                };
                let Some((source, _)) = devices(&state.project, owner)
                    .into_iter()
                    .find(|(source, kind)| source.id() == device && *kind == class)
                else {
                    return;
                };
                let Ok(metadata) = hex_engine::native_device_view_for_class(class, source.state())
                else {
                    return;
                };
                let mut actions: Vec<_> = metadata
                    .parameters
                    .iter()
                    .filter_map(|parameter| {
                        Entry::read(
                            &state.project,
                            self.project_generation,
                            owner,
                            device,
                            DeviceAutomationTarget::Parameter(parameter.descriptor.id),
                        )
                        .map(|entry| ("parameter", Action::Show(entry, true)))
                    })
                    .collect();
                actions.extend(
                    [
                        DeviceAutomationTarget::HostInputGain,
                        DeviceAutomationTarget::HostOutputGain,
                    ]
                    .into_iter()
                    .filter_map(|target| {
                        Entry::read(
                            &state.project,
                            self.project_generation,
                            owner,
                            device,
                            target,
                        )
                        .map(|entry| ("parameter", Action::Show(entry, true)))
                    }),
                );
                self.open_automation_menu(position, actions, window, cx);
            }
            Action::Reveal {
                owner, used_only, ..
            } => {
                let Some(state) = &self.state else {
                    return;
                };
                let entries: Vec<_> = devices(&state.project, owner)
                    .into_iter()
                    .flat_map(|(device, _)| {
                        device.automation().iter().filter_map(|lane| {
                            Entry::read(
                                &state.project,
                                self.project_generation,
                                owner,
                                device.id(),
                                lane.target,
                            )
                        })
                    })
                    .collect();
                let lanes = &mut self.automation_view.devices;
                if used_only {
                    lanes.extra.retain(|entry| {
                        entry.target.owner != owner
                            || state
                                .project
                                .device(entry.target.device)
                                .and_then(|device| device.automation_target(entry.target.kind))
                                .is_some()
                    });
                }
                for entry in entries {
                    if !lanes.extra.iter().any(|old| old.target == entry.target) {
                        lanes.extra.push(entry);
                    }
                }
                if !used_only {
                    for entry in lanes
                        .known
                        .iter()
                        .filter(|entry| entry.target.owner == owner)
                    {
                        if !lanes.extra.iter().any(|old| old.target == entry.target) {
                            lanes.extra.push(entry.clone());
                        }
                    }
                }
                if !used_only
                    && let Some(main) = lanes
                        .main
                        .iter()
                        .find(|entry| entry.target.owner == owner)
                        .cloned()
                    && !lanes.extra.iter().any(|entry| entry.target == main.target)
                {
                    lanes.extra.push(main);
                }
                self.set_mode(super::EditorMode::Arrangement);
                self.automation_view.enabled = true;
                self.track_scroll.remeasure();
            }
            Action::HidePreview(target) => {
                if self.selected_automation_trim() == Some(target)
                    && !super::device_panel::finalize_controls(self, cx)
                {
                    return;
                }
                self.automation_view.devices.clear_main(target.owner);
            }
            Action::Hide(target) => {
                if self
                    .automation_view
                    .devices
                    .selected
                    .as_ref()
                    .is_some_and(|selected| selected.target == target)
                {
                    self.automation_view.devices.selected = None;
                }
                self.automation_view
                    .devices
                    .extra
                    .retain(|entry| entry.target != target);
                self.track_scroll.remeasure();
            }
            Action::Reenable(target) => self.reenable_device_automation(Some(target), cx),
            Action::Override(target) => self.select_device_automation_read(
                Some(target),
                hex_project::AutomationReadSelection::Base,
                cx,
            ),
            Action::ToggleNativeArm(target) => {
                self.toggle_native_automation_arm(target.device, target.kind);
            }
            Action::ToggleVst3Arm { target, .. } => {
                if let (Some(class), DeviceAutomationTarget::Vst3 { parameter }) =
                    (target.vst3_class, target.kind)
                {
                    self.toggle_vst3_automation_arm(target.device, class, parameter);
                }
            }
            Action::ToggleVst3EditorArm { target, .. } => {
                if let (Some(class), DeviceAutomationTarget::Vst3 { parameter }) =
                    (target.vst3_class, target.kind)
                {
                    self.toggle_vst3_editor_arm(target.device, class, parameter);
                }
            }
            Action::ToggleVst3EditorGroupMember { target, .. } => {
                if let (Some(class), DeviceAutomationTarget::Vst3 { parameter }) =
                    (target.vst3_class, target.kind)
                {
                    self.toggle_vst3_editor_group_member(target.device, class, parameter);
                }
            }
            Action::ToggleVst3EditorGroupArm { target, .. } => {
                if let Some(class) = target.vst3_class {
                    self.toggle_vst3_editor_group_arm(target.device, class);
                }
            }
            Action::Clear(target) => {
                self.commit_device_automation(target, Vec::new(), cx);
            }
            Action::Shape(target, shape) => {
                let Some(selected) = self.automation_view.devices.selected.clone() else {
                    return;
                };
                let Some(state) = &self.state else {
                    return;
                };
                let mut points = self.automation_view.devices.points(state, target);
                for point in &mut points {
                    if selected.contains(target, point.beat) {
                        point.outgoing = shape;
                    }
                }
                self.commit_device_automation(target, points, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn device_automation_state_installed(&mut self, replaced: bool) {
        let Some(state) = &self.state else {
            return;
        };
        let stamp = (self.project_generation, state.revision);
        let lanes = &mut self.automation_view.devices;
        if replaced {
            self.automation_view.gain_preview = None;
            *lanes = Lanes::default();
            self.track_scroll.remeasure();
            return;
        }
        if lanes.stamp == Some(stamp) {
            return;
        }
        lanes.stamp = Some(stamp);
        let previous_extra_count = lanes.extra.len();
        if lanes.draft.as_ref().is_some_and(|draft| {
            draft.revision != state.revision
                || draft
                    .target
                    .current(&state.project, self.project_generation)
                    .is_none()
        }) {
            lanes.draft = None;
        }
        if lanes.marquee.as_ref().is_some_and(|marquee| {
            marquee.revision != state.revision
                || marquee
                    .target
                    .current(&state.project, self.project_generation)
                    .is_none()
        }) && let Some(marquee) = lanes.marquee.take()
        {
            lanes.selected = marquee.base;
            lanes.range = marquee.base_range;
        }
        for entries in [&mut lanes.main, &mut lanes.extra, &mut lanes.known] {
            entries.retain_mut(|entry| {
                if let Some(current) = entry.refreshed(&state.project, self.project_generation) {
                    *entry = current;
                    true
                } else {
                    false
                }
            });
        }
        if lanes
            .range
            .is_some_and(|range| lanes.entry(range.target).is_none())
        {
            lanes.range = None;
        }
        if let Some(selected) = lanes.selected.take() {
            let target = selected.target;
            lanes.selected = if lanes
                .pending
                .as_ref()
                .is_some_and(|pending| pending.target == target)
            {
                Some(selected)
            } else {
                let canonical = state
                    .project
                    .device(target.device)
                    .and_then(|device| device.automation_target(target.kind))
                    .map_or(&[][..], |lane| lane.points.as_slice());
                Selection::new(
                    target,
                    selected
                        .beats
                        .into_iter()
                        .filter(|beat| canonical.iter().any(|point| point.beat == *beat))
                        .collect(),
                )
            };
        }
        if lanes.extra.len() != previous_extra_count {
            self.track_scroll.remeasure();
        }
    }

    pub(super) fn pending_device_automation_matches(&self, request: &Request) -> bool {
        if let Some(pending) = &self.automation_view.devices.read_pending
            && matches!(request, Request::DeviceAutomationControl { project_generation, revision, edit }
                if *project_generation == pending.generation && *revision == pending.revision && *edit == pending.edit)
        {
            return true;
        }
        let Some(pending) = &self.automation_view.devices.pending else {
            return false;
        };
        matches!(request, Request::DeviceAutomation { project_generation, revision, device, target, vst3_class, points } if *project_generation == pending.target.generation && *revision == pending.revision && *device == pending.target.device && *target == pending.target.kind && *vst3_class == pending.target.vst3_class && *points == pending.points)
    }
    pub(super) fn device_automation_submitted(&mut self) {
        if let Some(pending) = &mut self.automation_view.devices.read_pending {
            pending.receipt.submitted(self.worker_requests_pending);
        }
        if let Some(pending) = &mut self.automation_view.devices.pending {
            pending.receipt.submitted(self.worker_requests_pending);
        }
    }
    pub(super) fn retire_pending_device_automation(&mut self) -> bool {
        let points = self.automation_view.devices.pending.take().is_some();
        self.automation_view.devices.read_pending.take().is_some() || points
    }
    pub(super) fn device_automation_complete(&mut self, event: &Event) {
        if let Some(pending) = &mut self.automation_view.devices.read_pending
            && pending.receipt.observe(
                self.project_generation,
                event.project_replaced,
                event.request_complete,
            ) != Disposition::Waiting
        {
            self.automation_view.devices.read_pending = None;
        }
        let lanes = &mut self.automation_view.devices;
        let Some(pending) = &mut lanes.pending else {
            return;
        };
        let disposition = pending.receipt.observe(
            self.project_generation,
            event.project_replaced,
            event.request_complete,
        );
        if disposition != Disposition::Waiting
            && let Some(pending) = lanes.pending.take()
            && disposition == Disposition::Terminal
            && event.error.is_none()
        {
            if let Some(copy) = pending.cut_clipboard {
                lanes.clipboard = Some(copy);
            }
            if let Some(range) = pending.select_range
                && lanes
                    .range
                    .is_some_and(|active| active.target == range.target)
            {
                lanes.range = Some(range);
                lanes.selected = range.selection(&pending.points);
            }
        }
    }
    pub(super) fn cancel_device_automation_draft(&mut self, cx: &mut Context<Self>) -> bool {
        let lanes = &mut self.automation_view.devices;
        let mut changed = lanes.draft.take().is_some();
        if let Some(marquee) = lanes.marquee.take() {
            lanes.selected = marquee.base;
            lanes.range = marquee.base_range;
            changed = true;
        }
        if changed {
            cx.notify();
        }
        changed
    }
    /// Removes every selected point of the selected lane in one request.
    pub(super) fn delete_device_automation_point(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(selected) = self.automation_view.devices.selected.clone() else {
            return false;
        };
        let Some(state) = &self.state else {
            return false;
        };
        let target = selected.target;
        let mut points = self.automation_view.devices.points(state, target);
        points.retain(|point| !selected.contains(target, point.beat));
        self.automation_view.devices.selected = None;
        self.commit_device_automation(target, points, cx);
        true
    }
    fn commit_device_automation(
        &mut self,
        target: Target,
        points: Vec<AutomationPoint>,
        cx: &mut Context<Self>,
    ) {
        if self.edit_pending()
            || self.automation_view.devices.pending.is_some()
            || self.automation_view.devices.read_pending.is_some()
        {
            return;
        }
        let Some(state) = &self.state else {
            return;
        };
        if target
            .current(&state.project, self.project_generation)
            .is_none()
        {
            return;
        }
        let canonical = state
            .project
            .device(target.device)
            .and_then(|device| device.automation_target(target.kind));
        let unchanged = canonical.map_or(points.is_empty(), |lane| {
            lane.points == points && (!points.is_empty() || lane.touch.is_empty())
        });
        if unchanged {
            return;
        }
        let revision = state.revision;
        self.automation_view.devices.pending = Some(Pending {
            target,
            revision,
            points: points.clone(),
            receipt: Receipt::new(target.generation),
            select_range: None,
            cut_clipboard: None,
        });
        if !self.enqueue(Request::DeviceAutomation {
            project_generation: target.generation,
            revision,
            device: target.device,
            target: target.kind,
            vst3_class: target.vst3_class,
            points,
        }) {
            self.retire_pending_device_automation();
        }
        cx.notify();
    }
}
