//! View selection guarded by the canonical instrument owner, independent of DSP edits.
use super::super::{
    DevicePanelOwner, EditorView, ParameterControl, ParameterKey, finalize_controls_except,
};
use super::{OperatorPage, OperatorPanelState};
use gpui::{App, Bounds, Context, EntityId, FocusHandle, Focusable, Pixels, Window};
use hex_project::{DeviceId, TrackId};

#[derive(Clone, Copy, Debug)]
pub(in crate::editor::device_panel) struct Owner {
    generation: u64,
    track: TrackId,
    device: DeviceId,
}

impl Owner {
    pub(in crate::editor::device_panel) fn capture(
        view: &EditorView,
        device: DeviceId,
    ) -> Option<Self> {
        let state = view.state.as_ref()?;
        let track = state.project.tracks().iter().find(|track| {
            track.instrument().is_some_and(|instrument| {
                instrument.id() == device
                    && instrument.state().type_id().as_str() == Ok("hex.instrument.operator")
            })
        })?;
        Some(Self {
            generation: view.project_generation,
            track: track.id(),
            device,
        })
    }

    pub(super) fn is_alive(self, view: &EditorView) -> bool {
        self.generation == view.project_generation
            && view
                .state
                .as_ref()
                .and_then(|state| state.project.track(self.track))
                .and_then(hex_project::Track::instrument)
                .is_some_and(|instrument| {
                    instrument.id() == self.device
                        && instrument.state().type_id().as_str() == Ok("hex.instrument.operator")
                })
    }

    pub(super) fn is_current(self, view: &EditorView) -> bool {
        self.is_alive(view)
            && view.selected_track == Some(self.track)
            && view.focused_return.is_none()
            && view.device_panel.owner == Some(DevicePanelOwner::Track(self.track))
    }
}

pub(in crate::editor::device_panel) fn select_oscillator(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    owner: Owner,
    operator: usize,
    excluded: &[ParameterKey],
) {
    if operator >= 4 || !owner.is_current(view) {
        return;
    }
    let current = view
        .device_panel
        .operator_pages
        .get(&owner.device)
        .copied()
        .unwrap_or_default();
    let target = OperatorPanelState {
        selected_operator: operator,
        page: if current.page == OperatorPage::Envelope {
            OperatorPage::Envelope
        } else {
            OperatorPage::Oscillator
        },
    };
    if target != current && finalize_controls_except(view, cx, excluded) {
        view.device_panel
            .operator_pages
            .insert(owner.device, target);
        cx.notify();
    }
}

pub(super) fn select_routing(view: &mut EditorView, cx: &mut Context<EditorView>, owner: Owner) {
    if owner.is_current(view) && super::super::finalize_controls(view, cx) {
        view.device_panel
            .operator_pages
            .entry(owner.device)
            .or_default()
            .page = OperatorPage::Routing;
        cx.notify();
    }
}

// Subscribe at prepaint, when GPUI supplies the actual window. Subscriptions are
// retained per device and pruned to the live controls on every render. They never
// retain an audio/project owner or steal focus from the widget that activated.
pub(super) fn focus_listener(
    view: &EditorView,
    cx: &Context<EditorView>,
    device: DeviceId,
) -> impl Fn(Vec<Bounds<Pixels>>, &mut Window, &mut App) + 'static {
    let owner = Owner::capture(view, device);
    let entity = cx.weak_entity();
    move |_, window, cx| {
        let Some(owner) = owner else {
            return;
        };
        let _ = entity.update(cx, |view, cx| {
            let mut handles: Vec<(EntityId, FocusHandle, Option<usize>, Option<ParameterKey>)> =
                Vec::new();
            for (key, control) in &view.device_panel.controls {
                if key.device != device {
                    continue;
                }
                let operator = super::shell_operator_for_numeric_parameter(key.parameter).or_else(
                    || match key.parameter.value() {
                        62 | 65 | 68 | 71 => Some(((key.parameter.value() - 62) / 3) as usize),
                        _ => None,
                    },
                );
                if let Some(operator) = operator {
                    let (id, focus) = match control {
                        ParameterControl::Float { entity, .. } => {
                            (entity.entity_id(), entity.focus_handle(cx))
                        }
                        ParameterControl::Bool { entity, .. } => {
                            (entity.entity_id(), entity.focus_handle(cx))
                        }
                        ParameterControl::Choice { entity, .. } => {
                            (entity.entity_id(), entity.focus_handle(cx))
                        }
                    };
                    handles.push((id, focus, Some(operator), Some(*key)));
                }
            }
            if let Some(state) = view.device_panel.operator_previews.get(&device) {
                for (field, control) in &state.controls {
                    if field.is_global() {
                        handles.push((
                            control.entity.entity_id(),
                            control.entity.focus_handle(cx),
                            None,
                            None,
                        ));
                    }
                }
            }
            view.device_panel
                .operator_context_focus
                .entry(device)
                .or_default()
                .retain(|id, _| handles.iter().any(|(live, _, _, _)| id == live));
            for (id, handle, operator, key) in handles {
                if view
                    .device_panel
                    .operator_context_focus
                    .get(&device)
                    .is_some_and(|watches| watches.contains_key(&id))
                {
                    continue;
                }
                let subscription = cx.on_focus(&handle, window, move |view, _, cx| {
                    if let Some(operator) = operator {
                        select_oscillator(view, cx, owner, operator, key.as_slice());
                    } else {
                        select_routing(view, cx, owner);
                    }
                });
                view.device_panel
                    .operator_context_focus
                    .entry(device)
                    .or_default()
                    .insert(id, subscription);
            }
        });
    }
}

// Section containers track descendant focus, but are never keyboard targets.
pub(in crate::editor::device_panel) struct SectionFocus {
    pub(in crate::editor::device_panel) handle: FocusHandle,
    pub(in crate::editor::device_panel) focused: bool,
    subscriptions: Option<(gpui::Subscription, gpui::Subscription)>,
}

pub(super) fn section_focus(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    section: super::Section,
) -> (FocusHandle, bool) {
    let state = view
        .device_panel
        .operator_section_focus
        .entry((device, section))
        .or_insert_with(|| SectionFocus {
            handle: cx.focus_handle().tab_stop(false),
            focused: false,
            subscriptions: None,
        });
    (state.handle.clone(), state.focused)
}

pub(super) fn section_listener(
    view: &EditorView,
    cx: &Context<EditorView>,
    device: DeviceId,
    section: super::Section,
) -> impl Fn(Vec<Bounds<Pixels>>, &mut Window, &mut App) + 'static {
    let entity = cx.weak_entity();
    let owner = Owner::capture(view, device);
    move |_, window, cx| {
        let _ = entity.update(cx, |view, cx| {
            let key = (device, section);
            let Some(state) = view.device_panel.operator_section_focus.get(&key) else {
                return;
            };
            if state.subscriptions.is_some() {
                return;
            }
            let handle = state.handle.clone();
            let focus_in = cx.on_focus_in(&handle, window, move |view, window, cx| {
                if let Some(state) = view.device_panel.operator_section_focus.get_mut(&key) {
                    state.focused = true;
                }
                if let Some(owner) = owner {
                    let excluded: Vec<_> = view
                        .device_panel
                        .controls
                        .iter()
                        .filter_map(|(key, control)| {
                            let focus = match control {
                                ParameterControl::Float { entity, .. } => entity.focus_handle(cx),
                                ParameterControl::Bool { entity, .. } => entity.focus_handle(cx),
                                ParameterControl::Choice { entity, .. } => entity.focus_handle(cx),
                            };
                            (key.device == device && focus.is_focused(window)).then_some(*key)
                        })
                        .collect();
                    match section {
                        super::Section::Oscillator(operator)
                            if !excluded
                                .iter()
                                .any(|key| (609..=612).contains(&key.parameter.value())) =>
                        {
                            select_oscillator(view, cx, owner, operator, &excluded);
                        }
                        super::Section::Preview("lfo" | "filter")
                            if owner.is_current(view)
                                && super::super::finalize_controls_except(view, cx, &excluded) =>
                        {
                            view.device_panel
                                .operator_pages
                                .entry(device)
                                .or_default()
                                .page = if section == super::Section::Preview("filter") {
                                super::OperatorPage::Filter
                            } else {
                                super::OperatorPage::Lfo
                            };
                            cx.notify();
                        }
                        _ => {}
                    }
                }
                // GPUI delivers focus events after painting, when immediate
                // notifications cannot dirty the window. Schedule the changed
                // section/context for the next frame without requiring input.
                cx.defer_in(window, |_, _, cx| cx.notify());
            });
            let focus_out = cx.on_focus_out(&handle, window, move |view, _, window, cx| {
                if let Some(state) = view.device_panel.operator_section_focus.get_mut(&key) {
                    state.focused = false;
                    cx.defer_in(window, |_, _, cx| cx.notify());
                }
            });
            if let Some(state) = view.device_panel.operator_section_focus.get_mut(&key) {
                state.subscriptions = Some((focus_in, focus_out));
            }
        });
    }
}
