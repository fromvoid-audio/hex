//! Transient lane presentation over canonical device envelopes. Selecting or
//! revealing a parameter never authors points. The worker validates source
//! stamps and owns history; released curves retain their exact ordered receipt.
mod paint;
#[cfg(test)]
mod tests;
use super::pending_edit::{Disposition, Receipt};
use super::{EditorView, ViewState, device_panel::DevicePanelOwner as Owner};
use crate::context_menu::{MenuItem, PopupMenu};
use gpui::{Context, Pixels, Point, SharedString, Window};
use hex_engine::NativeDeviceClass;
use hex_lattice_contract::{
    DeviceTypeId, ParameterDescriptor, ParameterId, ParameterKind, ParameterSemantics,
    ParameterValue,
};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Device, DeviceId, Project,
};
use hex_session::service::{Event, Request};
pub(super) use paint::{overlay, rows};

pub(super) const LANE_HEIGHT: f32 = 64.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Target {
    pub(super) generation: u64,
    pub(super) owner: Owner,
    pub(super) device: DeviceId,
    pub(super) parameter: ParameterId,
    class: NativeDeviceClass,
    provider: DeviceTypeId,
}

#[derive(Clone)]
pub(super) struct Entry {
    pub(super) target: Target,
    pub(super) label: SharedString,
    descriptor: ParameterDescriptor,
    semantics: ParameterSemantics,
    pub(super) base: f32,
    end: Option<BeatTime>,
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
    Shape(Target, AutomationInterpolation),
}

#[derive(Clone)]
pub(super) struct Draft {
    target: Target,
    revision: u64,
    points: Vec<AutomationPoint>,
    initial: AutomationPoint,
    point: AutomationPoint,
    viewport: super::viewport::Viewport,
    rect: [f32; 4],
    origin: [f32; 2],
}
struct Pending {
    target: Target,
    revision: u64,
    points: Vec<AutomationPoint>,
    receipt: Receipt,
}

#[derive(Default)]
pub(super) struct Lanes {
    main: Vec<Entry>,
    extra: Vec<Entry>,
    known: Vec<Entry>,
    selected: Option<(Target, BeatTime)>,
    draft: Option<Draft>,
    pending: Option<Pending>,
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
    fn current(self, project: &Project, generation: u64) -> Option<&Device> {
        if self.generation != generation {
            return None;
        }
        let (device, class) = devices(project, self.owner)
            .into_iter()
            .find(|(device, _)| device.id() == self.device)?;
        (class == self.class
            && device.state().type_id() == self.provider
            && hex_engine::native_parameter_automation_supported(
                class,
                device.state(),
                self.parameter,
            ))
        .then_some(device)
    }
}

impl Entry {
    fn read(
        project: &Project,
        generation: u64,
        owner: Owner,
        device: DeviceId,
        parameter: ParameterId,
    ) -> Option<Self> {
        let inventory = devices(project, owner);
        let (index, (source, class)) = inventory
            .iter()
            .enumerate()
            .find(|(_, (source, _))| source.id() == device)?;
        if !hex_engine::native_parameter_automation_supported(*class, source.state(), parameter) {
            return None;
        }
        let metadata = hex_engine::native_device_view_for_class(*class, source.state()).ok()?;
        let value = metadata
            .parameters
            .iter()
            .find(|value| value.descriptor.id == parameter)?;
        let semantics = value.semantics?;
        let base = semantics
            .normalization
            .normalize(value.descriptor, value.value)
            .ok()?;
        Some(Self {
            target: Target {
                generation,
                owner,
                device,
                parameter,
                class: *class,
                provider: source.state().type_id(),
            },
            label: format!(
                "{} {} · {}",
                metadata.name,
                index + 1,
                value.descriptor.label
            )
            .into(),
            descriptor: value.descriptor,
            semantics,
            base,
            end: source
                .automation_lane(parameter)
                .and_then(|lane| lane.points.last())
                .map(|point| point.beat),
        })
    }
    fn refreshed(&self, project: &Project, generation: u64) -> Option<Self> {
        self.target.current(project, generation)?;
        Self::read(
            project,
            generation,
            self.target.owner,
            self.target.device,
            self.target.parameter,
        )
    }
    fn quantize(&self, value: f32) -> Option<f32> {
        let mut canonical = self
            .semantics
            .normalization
            .denormalize(self.descriptor, value.clamp(0.0, 1.0))
            .ok()?;
        if let (
            hex_lattice_contract::ParameterStepping::Increment(step),
            ParameterKind::Float { min, max, .. },
            ParameterValue::Float(value),
        ) = (self.semantics.stepping, self.descriptor.kind, canonical)
        {
            canonical = ParameterValue::Float(
                (min + ((value - min) / step).round() * step).clamp(min, max),
            );
        }
        self.semantics
            .normalization
            .normalize(self.descriptor, canonical)
            .ok()
    }
    fn format(&self, value: f32) -> String {
        let Ok(value) = self
            .semantics
            .normalization
            .denormalize(self.descriptor, value)
        else {
            return "unavailable".into();
        };
        match (value, self.descriptor.kind) {
            (ParameterValue::Bool(value), _) => if value { "on" } else { "off" }.into(),
            (ParameterValue::Choice(index), ParameterKind::Choice { labels }) => labels
                .get(index as usize)
                .copied()
                .unwrap_or("unavailable")
                .into(),
            (ParameterValue::Float(value), ParameterKind::Float { unit, .. }) => {
                if self.descriptor.presentation
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
        if let Some(draft) = self.draft.as_ref().filter(|draft| draft.target == target) {
            return draft.points.clone();
        }
        if let Some(pending) = self
            .pending
            .as_ref()
            .filter(|pending| pending.target == target)
        {
            return pending.points.clone();
        }
        state
            .project
            .device(target.device)
            .and_then(|device| device.automation_lane(target.parameter))
            .map(|lane| lane.points.clone())
            .unwrap_or_default()
    }
}

impl EditorView {
    pub(super) fn remember_automation_parameter(
        &mut self,
        device: DeviceId,
        parameter: ParameterId,
    ) {
        let Some(owner) = self.device_panel.owner else {
            return;
        };
        let Some(state) = &self.state else {
            return;
        };
        let Some(entry) = Entry::read(
            &state.project,
            self.project_generation,
            owner,
            device,
            parameter,
        ) else {
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
        parameter: ParameterId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(owner) = self.device_panel.owner else {
            return false;
        };
        let Some(state) = &self.state else {
            return false;
        };
        let Some(entry) = Entry::read(
            &state.project,
            self.project_generation,
            owner,
            device,
            parameter,
        ) else {
            return false;
        };
        let utility_mode = entry.target.provider == DeviceTypeId::from_static("hex.utility")
            && matches!(parameter.value(), 3 | 10);
        self.open_automation_menu(
            position,
            vec![
                ("show automation", Action::Show(entry.clone(), false)),
                ("add new automation lane", Action::Show(entry, true)),
            ],
            window,
            cx,
        );
        if utility_mode {
            self.append_utility_mode_choice(device, parameter.value() == 10);
        }
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
                    checked: matches!(&action, Action::Show(entry, _) if self.state.as_ref().is_some_and(|state| state.project.device(entry.target.device).and_then(|device| device.automation_lane(entry.target.parameter)).is_some())),
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
                    .current(&state.project, self.project_generation)
                    .is_none()
            }),
            Action::Hide(target)
            | Action::HidePreview(target)
            | Action::Clear(target)
            | Action::Shape(target, _) => self.state.as_ref().is_none_or(|state| {
                target
                    .current(&state.project, self.project_generation)
                    .is_none()
            }),
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
                ..
            } => {
                *generation != self.project_generation
                    || self.state.as_ref().is_none_or(|state| {
                        !devices(&state.project, *owner)
                            .iter()
                            .any(|(source, kind)| source.id() == *device && kind == class)
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
                    if metadata.parameters.iter().any(|parameter| {
                        hex_engine::native_parameter_automation_supported(
                            class,
                            device.state(),
                            parameter.descriptor.id,
                        )
                    }) {
                        actions.push((
                            "device",
                            Action::Device {
                                generation: self.project_generation,
                                owner,
                                device: device.id(),
                                class,
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
                let actions = metadata
                    .parameters
                    .iter()
                    .filter_map(|parameter| {
                        Entry::read(
                            &state.project,
                            self.project_generation,
                            owner,
                            device,
                            parameter.descriptor.id,
                        )
                        .map(|entry| ("parameter", Action::Show(entry, true)))
                    })
                    .collect();
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
                                lane.parameter,
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
                                .and_then(|device| device.automation_lane(entry.target.parameter))
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
                self.automation_view.devices.clear_main(target.owner);
            }
            Action::Hide(target) => {
                if self
                    .automation_view
                    .devices
                    .selected
                    .is_some_and(|(selected, _)| selected == target)
                {
                    self.automation_view.devices.selected = None;
                }
                self.automation_view
                    .devices
                    .extra
                    .retain(|entry| entry.target != target);
                self.track_scroll.remeasure();
            }
            Action::Clear(target) => {
                self.commit_device_automation(target, Vec::new(), cx);
            }
            Action::Shape(target, shape) => {
                let Some((selected_target, beat)) = self.automation_view.devices.selected else {
                    return;
                };
                if selected_target != target {
                    return;
                }
                let Some(state) = &self.state else {
                    return;
                };
                let mut points = self.automation_view.devices.points(state, target);
                if let Some(point) = points.iter_mut().find(|point| point.beat == beat) {
                    point.outgoing = shape;
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
        if lanes.draft.as_ref().is_some_and(|draft| {
            draft.revision != state.revision
                || draft
                    .target
                    .current(&state.project, self.project_generation)
                    .is_none()
        }) {
            lanes.draft = None;
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
        if lanes.selected.is_some_and(|(target, beat)| {
            lanes
                .pending
                .as_ref()
                .is_none_or(|pending| pending.target != target)
                && state
                    .project
                    .device(target.device)
                    .and_then(|device| device.automation_lane(target.parameter))
                    .is_none_or(|lane| !lane.points.iter().any(|point| point.beat == beat))
        }) {
            lanes.selected = None;
        }
        self.track_scroll.remeasure();
    }

    pub(super) fn pending_device_automation_matches(&self, request: &Request) -> bool {
        let Some(pending) = &self.automation_view.devices.pending else {
            return false;
        };
        matches!(request, Request::DeviceAutomation { project_generation, revision, device, parameter, points } if *project_generation == pending.target.generation && *revision == pending.revision && *device == pending.target.device && *parameter == pending.target.parameter && *points == pending.points)
    }
    pub(super) fn device_automation_submitted(&mut self) {
        if let Some(pending) = &mut self.automation_view.devices.pending {
            pending.receipt.submitted(self.worker_requests_pending);
        }
    }
    pub(super) fn retire_pending_device_automation(&mut self) -> bool {
        self.automation_view.devices.pending.take().is_some()
    }
    pub(super) fn device_automation_complete(&mut self, event: &Event) {
        if let Some(pending) = &mut self.automation_view.devices.pending
            && pending.receipt.observe(
                self.project_generation,
                event.project_replaced,
                event.request_complete,
            ) != Disposition::Waiting
        {
            self.retire_pending_device_automation();
        }
    }
    pub(super) fn cancel_device_automation_draft(&mut self, cx: &mut Context<Self>) -> bool {
        let changed = self.automation_view.devices.draft.take().is_some();
        if changed {
            cx.notify();
        }
        changed
    }
    pub(super) fn delete_device_automation_point(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((target, beat)) = self.automation_view.devices.selected else {
            return false;
        };
        let Some(state) = &self.state else {
            return false;
        };
        let mut points = self.automation_view.devices.points(state, target);
        points.retain(|point| point.beat != beat);
        self.commit_device_automation(target, points, cx);
        true
    }
    fn commit_device_automation(
        &mut self,
        target: Target,
        points: Vec<AutomationPoint>,
        cx: &mut Context<Self>,
    ) {
        if self.edit_pending() || self.automation_view.devices.pending.is_some() {
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
            .and_then(|device| device.automation_lane(target.parameter))
            .map_or(&[][..], |lane| lane.points.as_slice());
        if canonical == points {
            return;
        }
        let revision = state.revision;
        self.automation_view.devices.pending = Some(Pending {
            target,
            revision,
            points: points.clone(),
            receipt: Receipt::new(target.generation),
        });
        if !self.enqueue(Request::DeviceAutomation {
            project_generation: target.generation,
            revision,
            device: target.device,
            parameter: target.parameter,
            points,
        }) {
            self.retire_pending_device_automation();
        }
        cx.notify();
    }
}
