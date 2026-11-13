//! Persistent host bindings shared by specialized and descriptor-driven panels.
//! Session gestures, canonical edits and app lifecycle remain with their owners.
use super::super::{
    device_action_owner::device_in_panel, native_automation_recording::automation_target,
};
use super::gesture_adapter::{
    NumericGestureLease, dispatch_bound_numeric_gesture, owns_native_automation,
    owns_numeric_gesture,
};
use super::*;
use std::{cell::Cell, rc::Rc};

pub(super) struct BindingPolicy {
    pub(super) tab_navigation: bool,
    pub(super) toggle_appearance: ToggleAppearance,
    pub(super) choice_order: Option<Vec<u32>>,
}

pub(super) type BindingPolicyFactory = fn(
    &EditorView,
    ParameterKey,
    DeviceControlIdentity,
    ParameterDescriptor,
    ControlPresentation,
    ControlText,
) -> BindingPolicy;
#[derive(Clone, Copy)]
pub(super) struct BindingPresentation {
    pub(super) kind: ControlPresentation,
    pub(super) text: ControlText,
    pub(super) policy: BindingPolicyFactory,
    pub(super) compact_toggle_width: f32,
}

#[derive(Clone, Copy)]
pub(super) struct CapturedBinding {
    generation: u64,
    owner: Option<DevicePanelOwner>,
    key: ParameterKey,
    identity: DeviceControlIdentity,
    descriptor: ParameterDescriptor,
}

impl CapturedBinding {
    pub(super) fn current(self, view: &EditorView, require_enabled: bool) -> bool {
        if self.generation != view.project_generation || self.owner != view.device_panel.owner {
            return false;
        }
        let Some(state) = view.state.as_ref() else {
            return false;
        };
        let Some(device) = self.owner.and_then(|owner| {
            device_in_panel(owner, &state.project, self.key.device, self.identity.class)
        }) else {
            return false;
        };
        if device.state().type_id() != self.identity.type_id {
            return false;
        }
        // Reserved trims are owned by the host and are absent from the
        // provider descriptor inventory.
        let kind = automation_target(self.key.parameter);
        if matches!(
            kind,
            hex_lattice_contract::DeviceAutomationTarget::HostInputGain
                | hex_lattice_contract::DeviceAutomationTarget::HostOutputGain
        ) {
            return hex_engine::native_parameter_input_target(
                &state.project,
                self.key.device,
                kind,
            )
            .is_ok();
        }
        if let Some((class, cached_state, cached_view)) =
            view.device_panel.views.get(&self.key.device)
            && *class == self.identity.class
            && cached_state == device.state()
        {
            return cached_view.as_ref().ok().is_some_and(|metadata| {
                metadata.parameters.iter().any(|parameter| {
                    parameter.descriptor == self.descriptor
                        && (!require_enabled || parameter.enabled)
                })
            });
        }
        hex_engine::native_device_view_for_class(self.identity.class, device.state()).is_ok_and(
            |metadata| {
                metadata.parameters.iter().any(|parameter| {
                    parameter.descriptor == self.descriptor
                        && (!require_enabled || parameter.enabled)
                })
            },
        )
    }
}

pub(super) enum ParameterControl {
    Float {
        lease: NumericGestureLease,
        delivery_fence: Rc<Cell<u64>>,
        entity: Entity<NumericControl>,
        _subscription: Subscription,
        binding: CapturedBinding,
        tab_navigation: bool,
    },
    Bool {
        entity: Entity<Toggle>,
        _subscription: Subscription,
        binding: CapturedBinding,
        tab_navigation: bool,
    },
    Choice {
        entity: Entity<Selector>,
        _subscription: Subscription,
        binding: CapturedBinding,
        tab_navigation: bool,
        values: Vec<u32>,
    },
}

pub(super) fn parameter_control_matches_identity(
    control: &ParameterControl,
    identity: DeviceControlIdentity,
) -> bool {
    let binding = control.binding();
    binding.identity.class == identity.class && binding.identity.type_id == identity.type_id
}

impl ParameterControl {
    pub(super) fn metadata(
        &self,
    ) -> (
        NativeDeviceClass,
        hex_lattice_contract::DeviceTypeId,
        ParameterDescriptor,
    ) {
        let binding = self.binding();
        (
            binding.identity.class,
            binding.identity.type_id,
            binding.descriptor,
        )
    }
    pub(super) fn binding(&self) -> CapturedBinding {
        match self {
            Self::Float { binding, .. }
            | Self::Bool { binding, .. }
            | Self::Choice { binding, .. } => *binding,
        }
    }
}

fn parameter_style(presentation: ControlPresentation) -> ControlStyle {
    theme::device_style(if presentation == ControlPresentation::InlineValue {
        theme::DeviceStyleRole::Readout
    } else {
        theme::DeviceStyleRole::Control
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ControlPresentation {
    Standard,
    CompactDial,
    MicroDial,
    FilledSlider,
    FilledVerticalSlider,
    InlineValue,
    MeterFader,
    ChoiceDial,
    CompactButton,
    CompactSelector,
    CompactConfirmSelector,
    CompactWideSelector,
    CompactSegments { columns: usize },
}

#[derive(Clone, Copy, Default)]
pub(super) struct ControlText {
    pub(super) label: Option<&'static str>,
    pub(super) codec: Option<numeric_display::NumericDisplayCodec>,
}

pub(crate) fn effective_parameter_value(
    view: &mut EditorView,
    key: ParameterKey,
    canonical_value: ParameterValue,
) -> ParameterValue {
    parameter_display_override(view, key).unwrap_or(canonical_value)
}

/// The in-flight gesture value, else the admitted manual value awaiting its
/// barrier. Bound controls and automation lanes share this display source.
pub(crate) fn parameter_display_override(
    view: &mut EditorView,
    key: ParameterKey,
) -> Option<ParameterValue> {
    if let Some(value) = view
        .native_automation_recorder
        .value(GestureTarget::DeviceParameter {
            device: key.device,
            parameter: key.parameter,
        })
    {
        return Some(value);
    }
    let gesture_value = view
        .device_gesture
        .snapshot()
        .and_then(|snapshot| match snapshot.target {
            GestureTarget::DeviceParameterSnapshot { device }
                if device == key.device && snapshot.key.generation == view.project_generation =>
            {
                snapshot.parameters.and_then(|parameters| {
                    parameters
                        .entries()
                        .iter()
                        .find(|(id, _)| *id == key.parameter)
                        .map(|(_, value)| *value)
                })
            }
            GestureTarget::DeviceParameter { device, parameter }
                if device == key.device && parameter == key.parameter =>
            {
                Some(snapshot.value)
            }
            GestureTarget::DeviceParameterPair { device, first, .. }
                if device == key.device && first == key.parameter =>
            {
                Some(snapshot.value)
            }
            GestureTarget::DeviceParameterPair { device, second, .. }
                if device == key.device && second == key.parameter =>
            {
                snapshot.secondary_value
            }
            _ => None,
        });
    let pending_value = view.device_panel.pending_values.get(&key).copied();
    if pending_value.is_some()
        && view.worker_requests_pending == 0
        && view.pending_requests.is_empty()
        && view.device_gesture_barriers.is_empty()
        && view.device_gesture.snapshot().is_none()
    {
        view.device_panel.pending_values.remove(&key);
    }
    gesture_value.or_else(|| view.device_panel.pending_values.get(&key).copied())
}

pub(super) fn render_parameter(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameter: &NativeDeviceParameterView,
    rendering: BindingPresentation,
) -> Div {
    let configuration = rendering;
    let BindingPresentation {
        kind: presentation,
        text: _,
        policy: _,
        compact_toggle_width,
    } = rendering;

    let style = parameter_style(presentation);
    let key = ParameterKey {
        device,
        parameter: parameter.descriptor.id,
    };
    let descriptor = parameter.descriptor;
    let value = if identity.type_id == hex_engine::eq::EQ_TYPE_ID
        || identity.type_id.as_str() == Ok("hex.eq8")
    {
        super::eq_automation::effective_value(view, key, identity.type_id, parameter)
    } else {
        effective_parameter_value(view, key, parameter.value)
    };
    let enabled = parameter.enabled;
    let compatible = matches!(
        (descriptor.kind, value),
        (ParameterKind::Float { .. }, ParameterValue::Float(_))
            | (ParameterKind::Bool, ParameterValue::Bool(_))
            | (ParameterKind::Choice { .. }, ParameterValue::Choice(_))
    );
    if !compatible {
        if let Some(mut control) = view.device_panel.controls.remove(&key) {
            cancel_control(view, cx, key, &mut control);
        }
        return unsupported_parameter(descriptor.label);
    }
    let matching = view.device_panel.controls.get(&key).is_some_and(|control| {
        let binding = control.binding();
        binding.generation == view.project_generation
            && binding.owner == view.device_panel.owner
            && binding.identity.class == identity.class
            && binding.identity.type_id == identity.type_id
            && binding.descriptor == descriptor
    });
    if !matching {
        if let Some(mut old) = view.device_panel.controls.remove(&key) {
            cancel_control(view, cx, key, &mut old);
        }
        if create_parameter_control(view, cx, key, identity, parameter, configuration).is_none() {
            return unsupported_parameter(descriptor.label);
        }
    }
    let tab_navigation = match view.device_panel.controls.get(&key) {
        Some(
            ParameterControl::Float { tab_navigation, .. }
            | ParameterControl::Bool { tab_navigation, .. }
            | ParameterControl::Choice { tab_navigation, .. },
        ) => *tab_navigation,
        None => false,
    };
    // Semantic queries describe acknowledged canonical values, not effective painted drafts.
    let revision = view.state.as_ref().map_or(0, |state| state.revision);
    let generation = view.project_generation;
    let owner = cx.entity().downgrade();
    let pending = view.edit_pending()
        || view.device_gesture.snapshot().is_some()
        || !view.device_gesture_barriers.is_empty()
        || view.device_panel.pending_values.contains_key(&key);
    let guard: std::rc::Rc<dyn Fn(&gpui::App) -> bool> = std::rc::Rc::new(move |app| {
        owner
            .read_with(app, |view, _| {
                view.project_generation == generation
                    && view
                        .state
                        .as_ref()
                        .is_some_and(|state| state.revision == revision)
                    && !view.edit_pending()
                    && view.device_gesture.snapshot().is_none()
                    && view.device_gesture_barriers.is_empty()
                    && !view.device_panel.pending_values.contains_key(&key)
                    && view.device_panel.controls.contains_key(&key)
            })
            .unwrap_or(false)
    });
    let rendered = match view.device_panel.controls.get(&key) {
        Some(ParameterControl::Float { entity, .. }) => {
            if let ParameterValue::Float(value) = value {
                entity.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                    if let ParameterValue::Float(canonical) = parameter.value
                        && let Ok(value) = control.accessibility_value(f64::from(canonical))
                    {
                        control.set_accessibility_binding(
                            gpui::AccessibilityBinding {
                                name: theme::text(descriptor.label),
                                revision,
                                value,
                                pending,
                                guard: guard.clone(),
                            },
                            cx,
                        );
                    }
                    control.set_enabled(enabled, cx);
                    control.set_tab_navigation(tab_navigation, cx);
                    let _ = control.set_external_value(f64::from(value), cx);
                });
            } else {
                entity.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                    if let ParameterValue::Float(canonical) = parameter.value
                        && let Ok(value) = control.accessibility_value(f64::from(canonical))
                    {
                        control.set_accessibility_binding(
                            gpui::AccessibilityBinding {
                                name: theme::text(descriptor.label),
                                revision,
                                value,
                                pending,
                                guard: guard.clone(),
                            },
                            cx,
                        );
                    }
                    control.set_enabled(enabled, cx);
                    control.set_tab_navigation(tab_navigation, cx);
                });
            }
            let (width, height) = match presentation {
                ControlPresentation::CompactDial => (
                    hex_widgets::COMPACT_DIAL_WIDTH,
                    hex_widgets::COMPACT_DIAL_HEIGHT,
                ),
                ControlPresentation::MicroDial => (
                    hex_widgets::MICRO_DIAL_WIDTH,
                    hex_widgets::MICRO_DIAL_HEIGHT,
                ),
                ControlPresentation::InlineValue => (
                    hex_widgets::INLINE_VALUE_WIDTH,
                    hex_widgets::INLINE_VALUE_HEIGHT,
                ),
                ControlPresentation::MeterFader => (60.0, 154.0),
                ControlPresentation::FilledSlider => (60.0, 18.0),
                ControlPresentation::FilledVerticalSlider => (12.0, 32.0),
                _ => (136.0, 26.0),
            };
            div().w(px(width)).h(px(height)).child(entity.clone())
        }
        Some(ParameterControl::Bool { entity, .. }) => {
            if let ParameterValue::Bool(value) = value {
                entity.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                    if let ParameterValue::Bool(canonical) = parameter.value {
                        control.set_accessibility_binding(
                            gpui::AccessibilityBinding {
                                name: theme::text(descriptor.label),
                                revision,
                                value: gpui::AccessibilityValue::Toggle(canonical),
                                pending,
                                guard: guard.clone(),
                            },
                            cx,
                        );
                    }
                    control.set_enabled(enabled, cx);
                    control.set_tab_navigation(tab_navigation, cx);
                    control.set_checked(value, cx);
                });
            } else {
                entity.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                    if let ParameterValue::Bool(canonical) = parameter.value {
                        control.set_accessibility_binding(
                            gpui::AccessibilityBinding {
                                name: theme::text(descriptor.label),
                                revision,
                                value: gpui::AccessibilityValue::Toggle(canonical),
                                pending,
                                guard: guard.clone(),
                            },
                            cx,
                        );
                    }
                    control.set_enabled(enabled, cx);
                    control.set_tab_navigation(tab_navigation, cx);
                });
            }
            let (width, height) = if presentation == ControlPresentation::CompactButton {
                (compact_toggle_width, 14.0)
            } else {
                (136.0, 22.0)
            };
            div().w(px(width)).h(px(height)).child(entity.clone())
        }
        Some(ParameterControl::Choice { entity, values, .. }) => {
            if let ParameterValue::Choice(value) = value {
                entity.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                    if let (ParameterValue::Choice(canonical), ParameterKind::Choice { labels }) =
                        (parameter.value, descriptor.kind)
                    {
                        control.set_accessibility_binding(
                            gpui::AccessibilityBinding {
                                name: theme::text(descriptor.label),
                                revision,
                                value: gpui::AccessibilityValue::Choice {
                                    selected: canonical as usize,
                                    options: labels.iter().map(theme::text).collect(),
                                },
                                pending,
                                guard: guard.clone(),
                            },
                            cx,
                        );
                    }
                    control.set_enabled(enabled, cx);
                    control.set_tab_navigation(tab_navigation, cx);
                    control.set_confirm_on_enter(
                        presentation == ControlPresentation::CompactConfirmSelector,
                        cx,
                    );
                    if let Some(index) = values.iter().position(|candidate| *candidate == value) {
                        let _ = control.set_selected(index, cx);
                    }
                });
            } else {
                entity.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                    if let (ParameterValue::Choice(canonical), ParameterKind::Choice { labels }) =
                        (parameter.value, descriptor.kind)
                    {
                        control.set_accessibility_binding(
                            gpui::AccessibilityBinding {
                                name: theme::text(descriptor.label),
                                revision,
                                value: gpui::AccessibilityValue::Choice {
                                    selected: canonical as usize,
                                    options: labels.iter().map(theme::text).collect(),
                                },
                                pending,
                                guard: guard.clone(),
                            },
                            cx,
                        );
                    }
                    control.set_enabled(enabled, cx);
                    control.set_tab_navigation(tab_navigation, cx);
                    control.set_confirm_on_enter(
                        presentation == ControlPresentation::CompactConfirmSelector,
                        cx,
                    );
                });
            }
            let (width, height) = match presentation {
                ControlPresentation::ChoiceDial => (64.0, 52.0),
                ControlPresentation::CompactSelector
                | ControlPresentation::CompactConfirmSelector => (64.0, 14.0),
                ControlPresentation::CompactWideSelector => (120.0, 14.0),
                ControlPresentation::CompactSegments { columns } => (
                    64.0,
                    values.len().div_ceil(columns.max(1)) as f32 * 16.0 - 2.0,
                ),
                _ => (136.0, 22.0),
            };
            div().w(px(width)).h(px(height)).child(entity.clone())
        }
        None => unsupported_parameter(descriptor.label),
    };
    let menu_binding = CapturedBinding {
        generation,
        owner: view.device_panel.owner,
        key,
        identity,
        descriptor,
    };
    rendered.capture_any_mouse_down(cx.listener(
        move |view, event: &gpui::MouseDownEvent, window, cx| {
            if event.button == gpui::MouseButton::Right
                && menu_binding.current(view, false)
                && view
                    .state
                    .as_ref()
                    .is_some_and(|state| state.revision == revision)
                && view.open_parameter_automation_menu(
                    event.position,
                    key.device,
                    automation_target(key.parameter),
                    window,
                    cx,
                )
            {
                cx.stop_propagation();
            }
        },
    ))
}

fn create_parameter_control(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    key: ParameterKey,
    identity: DeviceControlIdentity,
    parameter: &NativeDeviceParameterView,
    rendering: BindingPresentation,
) -> Option<()> {
    let BindingPresentation {
        kind: presentation,
        text: display,
        policy,
        ..
    } = rendering;

    let descriptor = parameter.descriptor;
    let binding = CapturedBinding {
        generation: view.project_generation,
        owner: view.device_panel.owner,
        key,
        identity,
        descriptor,
    };
    let value = parameter.value;
    let BindingPolicy {
        tab_navigation,
        toggle_appearance,
        choice_order,
    } = policy(view, key, identity, descriptor, presentation, display);
    let style = parameter_style(presentation);
    let ControlText {
        label: display_label,
        codec: display_codec,
    } = display;
    let id = format!(
        "device-{}-parameter-{}",
        key.device.value(),
        key.parameter.value()
    );
    match (descriptor.kind, value) {
        (ParameterKind::Float { min, max, unit }, ParameterValue::Float(value)) if min < max => {
            let default = match descriptor.default {
                ParameterValue::Float(default) => default,
                _ => return None,
            };
            let scale = match descriptor.presentation {
                ParameterPresentation::LinearAmplitudeDecibels => {
                    NumericScale::LinearAmplitudeDecibels
                }
                ParameterPresentation::Logarithmic => NumericScale::Logarithmic,
                ParameterPresentation::Linear
                    if min < 0.0
                        && max > 0.0
                        && presentation != ControlPresentation::FilledSlider =>
                {
                    NumericScale::Bipolar
                }
                ParameterPresentation::Linear => NumericScale::Linear,
            };
            let mut spec = NumericSpec::new_f32(
                min,
                max,
                default,
                scale,
                2,
                unit.map(|unit| std::sync::Arc::<str>::from(theme::text(unit).as_ref())),
            )
            .ok()?;
            if let Some(codec) = display_codec {
                spec = codec.apply(spec);
            }
            let appearance = match presentation {
                ControlPresentation::CompactDial => NumericControlPresentation::CompactDial,
                ControlPresentation::MicroDial => NumericControlPresentation::MicroDial,
                ControlPresentation::InlineValue => NumericControlPresentation::InlineValue,
                ControlPresentation::MeterFader => NumericControlPresentation::MeterFader,
                ControlPresentation::FilledSlider => NumericControlPresentation::FilledSlider,
                ControlPresentation::FilledVerticalSlider => {
                    NumericControlPresentation::FilledVerticalSlider
                }
                _ => NumericControlPresentation::HorizontalSlider,
            };
            let config = NumericControlConfig::new(
                id,
                theme::text(display_label.unwrap_or(descriptor.label)),
                appearance,
                spec,
                f64::from(value),
                style,
            )
            .ok()?
            .with_dial_layout(
                if display_codec == Some(numeric_display::NumericDisplayCodec::Balance) {
                    DialLayout::Pan
                } else {
                    DialLayout::ReadoutGap
                },
            );
            let entity = cx.new(|control_cx| NumericControl::from_config(control_cx, config));
            entity.update(cx, |control, _| control.set_commit_unchanged_text(true));
            let target = GestureTarget::DeviceParameter {
                device: key.device,
                parameter: key.parameter,
            };
            let lease = Rc::new(Cell::new(None));
            let event_lease = lease.clone();
            let delivery_fence = Rc::new(Cell::new(0));
            let event_fence = delivery_fence.clone();
            let subscription = cx.subscribe(
                &entity,
                move |view, emitter, event: &NumericControlEvent, cx| {
                    let is_current = |view: &EditorView| {
                        event_fence.get() == 0
                            && binding.current(view, event.phase != NumericEventPhase::Cancel)
                            && matches!(view.device_panel.controls.get(&key),
                                Some(ParameterControl::Float { entity, .. })
                                    if entity.entity_id() == emitter.entity_id())
                    };
                    if !is_current(view) {
                        return;
                    }
                    if event.phase == NumericEventPhase::Begin {
                        view.remember_automation_parameter(
                            key.device,
                            automation_target(key.parameter),
                        );
                    }
                    let value = ParameterValue::Float(event.value as f32);
                    if event.phase == NumericEventPhase::Commit
                        && event_lease.get().is_none()
                        && view.device_gesture.snapshot().is_none()
                    {
                        let result = view
                            .begin_device_gesture(target, value)
                            .and_then(|_| view.change_device_gesture(value))
                            .and_then(|_| view.finish_device_gesture(value));
                        if let Err(error) = result {
                            view.error = Some(error.into());
                        }
                        cx.notify();
                        return;
                    }
                    if let Err(error) = dispatch_bound_numeric_gesture(
                        view,
                        target,
                        &event_lease,
                        event.phase,
                        value,
                    ) && event.phase != NumericEventPhase::Cancel
                    {
                        view.error = Some(error.to_owned().into());
                    }
                    cx.notify();
                },
            );
            view.device_panel.controls.insert(
                key,
                ParameterControl::Float {
                    lease,
                    delivery_fence,
                    entity,
                    _subscription: subscription,
                    binding,
                    tab_navigation,
                },
            );
            Some(())
        }
        (ParameterKind::Bool, ParameterValue::Bool(value)) => {
            let appearance = toggle_appearance;
            let config = ToggleConfig::new(
                id,
                theme::text(display_label.unwrap_or(descriptor.label)),
                value,
                style,
            )
            .ok()?
            .with_appearance(appearance);
            let entity = cx.new(|control_cx| Toggle::from_config(control_cx, config));
            let target = key;
            let subscription = cx.subscribe(&entity, move |view, emitter, event: &ToggleChanged, cx| {
                if !binding.current(view, true) || !matches!(view.device_panel.controls.get(&key),
                    Some(ParameterControl::Bool { entity, .. }) if entity.entity_id() == emitter.entity_id()) { return; }
                // A discrete control may hide a numeric field (for example,
                // switching Operator tuning to fixed frequency). Do not let
                // that transition strand an invalid draft offscreen.
                if !finalize_controls(view, cx) {
                    let previous = view
                        .device_panel
                        .pending_values
                        .get(&target)
                        .copied()
                        .or_else(|| {
                            view.device_panel
                                .views
                                .get(&target.device)
                                .and_then(|(_, _, device_view)| device_view.as_ref().ok())
                                .and_then(|device_view| {
                                    device_view.parameters.iter().find(|parameter| {
                                        parameter.descriptor.id == target.parameter
                                    })
                                })
                                .map(|parameter| parameter.value)
                        })
                        .unwrap_or(ParameterValue::Bool(!event.checked));
                    if let (
                        ParameterValue::Bool(previous),
                        Some(ParameterControl::Bool { entity, .. }),
                    ) = (previous, view.device_panel.controls.get(&target))
                    {
                        let entity = entity.clone();
                        entity.update(cx, |control, cx| control.set_checked(previous, cx));
                    }
                    return;
                }
                view.remember_automation_parameter(key.device, automation_target(key.parameter));
                apply_discrete_parameter(view, target, ParameterValue::Bool(event.checked));
                cx.notify();
            });
            view.device_panel.controls.insert(
                key,
                ParameterControl::Bool {
                    entity,
                    _subscription: subscription,
                    binding,
                    tab_navigation,
                },
            );
            Some(())
        }
        (ParameterKind::Choice { labels }, ParameterValue::Choice(value)) => {
            let values = choice_order.unwrap_or_else(|| (0..labels.len() as u32).collect());
            if values.len() != labels.len()
                || values.iter().enumerate().any(|(index, value)| {
                    *value as usize >= labels.len() || values[..index].contains(value)
                })
            {
                return None;
            }

            let selected = values.iter().position(|candidate| *candidate == value)?;
            let choices = values
                .iter()
                .map(|index| theme::text(labels[*index as usize]));
            let appearance = if matches!(
                presentation,
                ControlPresentation::CompactSelector
                    | ControlPresentation::CompactConfirmSelector
                    | ControlPresentation::CompactWideSelector
            ) {
                SelectorAppearance::CompactSelector
            } else if presentation == ControlPresentation::ChoiceDial {
                SelectorAppearance::CompactDial
            } else {
                SelectorAppearance::Standard
            };
            let config = SelectorConfig::new(id, choices, selected, style)
                .ok()?
                .with_accessibility_name(theme::text(display_label.unwrap_or(descriptor.label)))
                .with_appearance(appearance);
            let config = if let ControlPresentation::CompactSegments { columns } = presentation {
                config.with_segment_columns(columns).ok()?
            } else {
                config
            };
            let entity = cx.new(|control_cx| Selector::from_config(control_cx, config));
            let target = key;
            let event_values = values.clone();
            let subscription =
                cx.subscribe(&entity, move |view, emitter, event: &SelectorChanged, cx| {
                    if !binding.current(view, true) || !matches!(view.device_panel.controls.get(&key),
                        Some(ParameterControl::Choice { entity, .. }) if entity.entity_id() == emitter.entity_id()) { return; }
                    let Some(&value) = event_values.get(event.index) else {
                        return;
                    };
                    if !finalize_controls(view, cx) {
                        return;
                    }
                    view.remember_automation_parameter(key.device, automation_target(key.parameter));
                    apply_discrete_parameter(view, target, ParameterValue::Choice(value));
                    cx.notify();
                });
            view.device_panel.controls.insert(
                key,
                ParameterControl::Choice {
                    entity,
                    _subscription: subscription,
                    binding,
                    tab_navigation,
                    values,
                },
            );
            Some(())
        }
        _ => None,
    }
}

fn apply_discrete_parameter(view: &mut EditorView, target: ParameterKey, value: ParameterValue) {
    let gesture = GestureTarget::DeviceParameter {
        device: target.device,
        parameter: target.parameter,
    };
    match view.begin_native_automation_step(gesture, value) {
        Ok(true) => return,
        Err(reason) => {
            view.error = Some(reason.into());
            return;
        }
        Ok(false) => {}
    }
    view.device_panel.pending_values.insert(target, value);
    if !view
        .manual_device_parameter_request(target.device, target.parameter, value)
        .is_ok_and(|request| view.enqueue_device_barrier(request))
    {
        view.device_panel.pending_values.remove(&target);
    }
}

/// Owner finalization can precede queued widget effects in the same App update.
/// Suppress only that completed delivery cycle, reopening this same binding
/// after the already-enqueued effects. Refused finalization never enters here.
pub(super) fn fence_finished_delivery(
    view: &EditorView,
    cx: &mut Context<EditorView>,
    key: ParameterKey,
    emitter: gpui::EntityId,
) {
    let Some(ParameterControl::Float { delivery_fence, .. }) = view.device_panel.controls.get(&key)
    else {
        return;
    };
    let fence = delivery_fence.clone();
    let epoch = fence.get().wrapping_add(1).max(1);
    fence.set(epoch);
    let owner = cx.weak_entity();
    cx.defer(move |cx| {
        let _ = owner.update(cx, |view, _| {
            if fence.get() == epoch
                && let Some(ParameterControl::Float {
                    entity,
                    binding,
                    delivery_fence,
                    ..
                }) = view.device_panel.controls.get(&key)
                && entity.entity_id() == emitter
                && Rc::ptr_eq(delivery_fence, &fence)
                && binding.generation == view.project_generation
                && binding.owner == view.device_panel.owner
            {
                fence.set(0);
            }
        });
    });
}

pub(super) fn cancel_control(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    key: ParameterKey,
    control: &mut ParameterControl,
) {
    if let ParameterControl::Float {
        entity,
        lease,
        delivery_fence,
        ..
    } = control
    {
        let target = GestureTarget::DeviceParameter {
            device: key.device,
            parameter: key.parameter,
        };
        let owns_native = owns_native_automation(view, target, lease);
        let owns = owns_numeric_gesture(view, target, lease);
        // Revoke before local cancellation can enqueue a subscription event.
        lease.set(None);
        delivery_fence.set(delivery_fence.get().wrapping_add(1).max(1));
        entity.update(cx, |control, cx| control.cancel(cx));
        if owns_native {
            view.cancel_native_automation_touch();
        } else if owns {
            let _ = view.cancel_device_gesture();
        }
    }
}

fn unsupported_parameter(label: &str) -> Div {
    div()
        .w(px(136.0))
        .h(px(22.0))
        .flex()
        .items_center()
        .text_xs()
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(format!("{label} · unsupported")))
}
