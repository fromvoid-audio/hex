//! View-only Operator controls for sections without native DSP contracts yet.
use super::super::{EditorView, control_style};
use crate::diagnostics::ActionTraceContext as _;
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px, rgb};
use hex_widgets::{
    NumericControl, NumericControlConfig, NumericControlEvent, NumericControlKind,
    NumericControlPresentation, NumericScale, NumericSpec, Toggle, ToggleAppearance, ToggleChanged,
    ToggleConfig,
};
use std::collections::HashMap;
use std::sync::Arc;

// Preserve selector-column space while accepting the wider shared dial tile.
pub(super) const PANEL_WIDTH: f32 = crate::device_composition::VECTOR_PREVIEW_WIDTH;
const FIELD_WIDTH: f32 = hex_widgets::MICRO_DIAL_WIDTH;
const ROW_HEIGHT: f32 = 40.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(usize)]
pub(in crate::editor::device_panel) enum Field {
    PitchEnvelope,
    Spread,
    Transpose,
    Time,
    Tone,
    Volume,
}

impl Field {
    pub(super) const fn is_global(self) -> bool {
        matches!(self, Self::Time | Self::Tone | Self::Volume)
    }

    pub(in crate::editor::device_panel) const ALL: [Self; 6] = [
        Self::PitchEnvelope,
        Self::Spread,
        Self::Transpose,
        Self::Time,
        Self::Tone,
        Self::Volume,
    ];

    const fn spec(self) -> FieldSpec {
        match self {
            Self::PitchEnvelope => FieldSpec::new("pitch env", -100.0, 100.0, 11.0, "%", 0, false),
            Self::Spread => FieldSpec::new("spread", 0.0, 100.0, 0.0, "%", 0, false),
            Self::Transpose => FieldSpec::new("transpose", -48.0, 48.0, 0.0, "st", 0, false),
            Self::Time => FieldSpec::new("time", 0.0, 100.0, 23.0, "%", 0, false),
            Self::Tone => FieldSpec::new("tone", 0.0, 100.0, 100.0, "%", 0, false),
            Self::Volume => FieldSpec::new("volume", -60.0, 6.0, -16.0, "db", 0, false),
        }
    }
}

#[derive(Clone, Copy)]
struct FieldSpec {
    label: &'static str,
    min: f64,
    max: f64,
    default: f64,
    unit: &'static str,
    decimals: u8,
    logarithmic: bool,
}

impl FieldSpec {
    const fn new(
        label: &'static str,
        min: f64,
        max: f64,
        default: f64,
        unit: &'static str,
        decimals: u8,
        logarithmic: bool,
    ) -> Self {
        Self {
            label,
            min,
            max,
            default,
            unit,
            decimals,
            logarithmic,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::editor::device_panel) enum Switch {
    Pitch,
}

impl Switch {
    pub(in crate::editor::device_panel) const fn index(self) -> usize {
        match self {
            Self::Pitch => 0,
        }
    }
}

pub(in crate::editor::device_panel) struct NumericPreview {
    pub(in crate::editor::device_panel) entity: gpui::Entity<NumericControl>,
    pub(in crate::editor::device_panel) _subscription: gpui::Subscription,
}

pub(in crate::editor::device_panel) struct TogglePreview {
    entity: gpui::Entity<Toggle>,
    _subscription: gpui::Subscription,
}

pub(in crate::editor::device_panel) struct State {
    pub(in crate::editor::device_panel) controls: HashMap<Field, NumericPreview>,
    pub(in crate::editor::device_panel) toggles: HashMap<Switch, TogglePreview>,
    pub(in crate::editor::device_panel) routing_focus: Option<gpui::FocusHandle>,
    pub(in crate::editor::device_panel) values: [f64; 6],
    pub(in crate::editor::device_panel) enabled: [bool; 1],
}

impl Default for State {
    fn default() -> Self {
        Self {
            controls: HashMap::new(),
            toggles: HashMap::new(),
            values: Field::ALL.map(|field| field.spec().default),
            enabled: [true; 1],
            routing_focus: None,
        }
    }
}

impl State {
    fn value(&self, field: Field) -> f64 {
        self.values[field as usize]
    }
}

pub(super) fn render(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: hex_project::DeviceId,
    device_view: &hex_engine::NativeDeviceView,
) -> Stateful<Div> {
    let rows = [
        super::lfo_summary(view, cx, device, device_view),
        super::filter_summary(view, cx, device, device_view),
        row(
            view,
            cx,
            device,
            "pitch env",
            Some(Switch::Pitch),
            &[Field::PitchEnvelope, Field::Spread, Field::Transpose],
        ),
        row(
            view,
            cx,
            device,
            "global",
            None,
            &[Field::Time, Field::Tone, Field::Volume],
        ),
    ];
    let mut content = hex_widgets::device_rail(super::super::device_layout_style())
        .w_full()
        .h_full();
    for row in rows {
        content = content.child(row);
    }
    hex_widgets::device_rail(super::super::device_layout_style())
        .id(ElementId::Name(
            format!("operator-preview-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-preview-{}", device.value()))
        .w(px(PANEL_WIDTH))
        .flex_shrink_0()
        .h_full()
        .pl(px(2.0))
        .child(content)
}

fn row(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: hex_project::DeviceId,
    title: &'static str,
    switch: Option<Switch>,
    fields: &[Field],
) -> Stateful<Div> {
    let owner = super::context::Owner::capture(view, device);
    let section = super::Section::Preview(title);
    let (section_focus, focused) = super::context::section_focus(view, cx, device, section);
    let mut row = hex_widgets::device_grouped_row(super::super::device_layout_style(), focused)
        .on_children_prepainted(super::context::section_listener(view, cx, device, section))
        .id(ElementId::Name(
            format!("operator-preview-row-{}-{title}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-preview-row-{}-{title}", device.value()))
        .track_focus(&section_focus)
        .w_full()
        .h(px(ROW_HEIGHT))
        .flex()
        .items_center()
        .gap(px(1.0));
    let mut left = div()
        .w(px(44.0))
        .flex_shrink_0()
        .h_full()
        .flex()
        .flex_col()
        .justify_center();
    if let Some(switch) = switch {
        left = left.child(toggle(view, cx, device, switch));
    } else {
        left = left.child(routing_link(view, cx, device));
    }
    row = row.child(left);
    let mut dials = div()
        .flex_1()
        .h_full()
        .flex()
        .items_center()
        .justify_between();
    for field in fields {
        dials = dials.child(control(view, cx, device, *field));
    }
    row = row.child(dials);
    if title == "global" {
        row = row.capture_any_mouse_down(cx.listener(
            move |view, event: &gpui::MouseDownEvent, _, cx| {
                if event.button == gpui::MouseButton::Left
                    && let Some(owner) = owner
                {
                    super::context::select_routing(view, cx, owner);
                }
            },
        ));
    }
    row
}

fn routing_link(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: hex_project::DeviceId,
) -> Stateful<Div> {
    let owner = super::context::Owner::capture(view, device);
    let focus = view
        .device_panel
        .operator_previews
        .entry(device)
        .or_default()
        .routing_focus
        .get_or_insert_with(|| cx.focus_handle().tab_index(0).tab_stop(true))
        .clone();
    let colors = (0..4).rev().filter_map(theme::oscillator_identity_color);
    let mut icon = div()
        .w(px(12.0))
        .h(px(30.0))
        .flex()
        .flex_col()
        .items_center()
        .justify_between();
    for (index, color) in colors.enumerate() {
        icon = icon.child(
            hex_widgets::device_identity_marker(color).id(ElementId::Name(
                format!("operator-route-icon-{}-{index}", device.value()).into(),
            )),
        );
    }
    hex_widgets::device_focus_target(super::super::device_layout_style())
        .id(ElementId::Name(
            format!("operator-preview-{}-route-link", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-preview-{}-route-link", device.value()))
        .track_focus(&focus)
        .key_context("hex_operator_routing")
        .tooltip(|_, cx| {
            cx.new(|_| super::super::super::transport::MiniTooltip {
                kind: crate::tooltips::Kind::control("routing and global context"),
                description: theme::text("routing and global context · enter or space"),
            })
            .into()
        })
        .w(px(18.0))
        .h(px(34.0))
        .flex()
        .items_center()
        .cursor(gpui::CursorStyle::PointingHand)
        .child(icon)
        .on_click(cx.listener(move |view, _, window, cx| {
            if owner.is_some_and(|owner| owner.is_current(view))
                && super::finalize_controls(view, cx)
            {
                focus.focus(window);
                view.device_panel
                    .operator_pages
                    .entry(device)
                    .or_default()
                    .page = super::OperatorPage::Routing;
                cx.notify();
            }
        }))
        .on_action(cx.trace(move |view, _: &super::ActivateRouting, _, cx| {
            if let Some(owner) = owner {
                super::context::select_routing(view, cx, owner);
                cx.stop_propagation();
            }
        }))
}

/// Refuse Tab traversal before blur can leave an invalid view-only numeric draft.
pub(super) fn finalize_controls(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: hex_project::DeviceId,
) -> bool {
    let controls: Vec<_> = view
        .device_panel
        .operator_previews
        .get(&device)
        .into_iter()
        .flat_map(|state| state.controls.values())
        .map(|control| control.entity.clone())
        .collect();
    for entity in controls {
        if let Err(error) = entity.update(cx, |control, cx| control.finish_edit(cx)) {
            view.error = Some(format!("finish the operator preview value edit: {error}").into());
            cx.notify();
            return false;
        }
    }
    true
}

pub(super) fn control(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: hex_project::DeviceId,
    field: Field,
) -> Stateful<Div> {
    let spec = field.spec();
    let key = (device, field);
    let needs_control = !view
        .device_panel
        .operator_previews
        .get(&device)
        .is_some_and(|state| state.controls.contains_key(&field));
    if needs_control {
        let scale = if spec.logarithmic {
            NumericScale::Logarithmic
        } else {
            NumericScale::Linear
        };
        let unit = Arc::<str>::from(theme::text(spec.unit).as_ref());
        let numeric_spec = match NumericSpec::new(
            spec.min,
            spec.max,
            spec.default,
            scale,
            spec.decimals,
            Some(unit),
        ) {
            Ok(spec) => spec,
            Err(error) => return control_error(error),
        };
        let initial = view
            .device_panel
            .operator_previews
            .get(&device)
            .map_or(spec.default, |state| state.value(field));
        let config = match NumericControlConfig::new(
            format!("operator-preview-{}-{}", device.value(), field as usize),
            theme::text(spec.label),
            NumericControlKind::Knob,
            numeric_spec,
            initial,
            control_style(),
        ) {
            Ok(config) => config.with_presentation(NumericControlPresentation::MicroDial),
            Err(error) => return control_error(error),
        };
        let entity = cx.new(|control_cx| NumericControl::from_config(control_cx, config));
        let owner = super::context::Owner::capture(view, device);
        let subscription =
            cx.subscribe(&entity, move |view, _, event: &NumericControlEvent, cx| {
                if let Some(owner) = owner {
                    let current = if event.phase == hex_widgets::NumericEventPhase::Cancel {
                        owner.is_alive(view)
                    } else {
                        owner.is_current(view)
                    };
                    if !current {
                        return;
                    }
                    if event.phase == hex_widgets::NumericEventPhase::Begin && field.is_global() {
                        super::context::select_routing(view, cx, owner);
                    }
                }
                if let Some(state) = view.device_panel.operator_previews.get_mut(&key.0) {
                    state.values[key.1 as usize] = event.value;
                }
                cx.notify();
            });
        view.device_panel
            .operator_previews
            .entry(device)
            .or_default()
            .controls
            .insert(
                field,
                NumericPreview {
                    entity,
                    _subscription: subscription,
                },
            );
    }
    let Some(control) = view
        .device_panel
        .operator_previews
        .get(&device)
        .and_then(|state| state.controls.get(&field))
    else {
        return control_error("numeric control unavailable");
    };
    let value = view
        .device_panel
        .operator_previews
        .get(&device)
        .map_or(spec.default, |state| state.value(field));
    control.entity.update(cx, |control, cx| {
        let _ = control.set_external_style(control_style(), cx);
        control.set_tab_navigation(true, cx);
        let _ = control.set_external_value(value, cx);
    });
    div()
        .id(ElementId::Name(
            format!("operator-preview-{}-{}", device.value(), field as usize).into(),
        ))
        .debug_selector(move || format!("operator-preview-{}-{}", device.value(), field as usize))
        .w(px(FIELD_WIDTH))
        .h(px(ROW_HEIGHT))
        .flex_shrink_0()
        .child(control.entity.clone())
}

fn toggle(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: hex_project::DeviceId,
    switch: Switch,
) -> Stateful<Div> {
    let needs_control = !view
        .device_panel
        .operator_previews
        .get(&device)
        .is_some_and(|state| state.toggles.contains_key(&switch));
    if needs_control {
        let enabled = view
            .device_panel
            .operator_previews
            .get(&device)
            .is_none_or(|state| state.enabled[switch.index()]);
        let config = match ToggleConfig::new(
            format!(
                "operator-preview-{}-toggle-{}",
                device.value(),
                switch.index()
            ),
            theme::text("on"),
            enabled,
            control_style(),
        ) {
            Ok(config) => config.with_appearance(ToggleAppearance::CompactSquare),
            Err(error) => return control_error(error),
        };
        let entity = cx.new(|control_cx| {
            let mut control = Toggle::from_config(control_cx, config);
            control.set_tab_navigation(true, control_cx);
            control
        });
        let key = (device, switch);
        let subscription = cx.subscribe(&entity, move |view, _, event: &ToggleChanged, cx| {
            if let Some(state) = view.device_panel.operator_previews.get_mut(&key.0) {
                state.enabled[key.1.index()] = event.checked;
            }
            cx.notify();
        });
        view.device_panel
            .operator_previews
            .entry(device)
            .or_default()
            .toggles
            .insert(
                switch,
                TogglePreview {
                    entity,
                    _subscription: subscription,
                },
            );
    }
    let Some(toggle) = view
        .device_panel
        .operator_previews
        .get(&device)
        .and_then(|state| state.toggles.get(&switch))
    else {
        return control_error("toggle unavailable");
    };
    toggle.entity.update(cx, |control, cx| {
        let _ = control.set_external_style(control_style(), cx);
    });
    div()
        .id(ElementId::Name(
            format!(
                "operator-preview-{}-toggle-{}",
                device.value(),
                switch.index()
            )
            .into(),
        ))
        .debug_selector(move || {
            format!(
                "operator-preview-{}-toggle-{}",
                device.value(),
                switch.index()
            )
        })
        .w(px(14.0))
        .h(px(14.0))
        .flex_shrink_0()
        .child(toggle.entity.clone())
}

fn control_error(error: impl std::fmt::Display) -> Stateful<Div> {
    div()
        .id("operator-preview-control-error")
        .h(px(ROW_HEIGHT))
        .text_size(px(7.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(format!("control unavailable · {error}")))
}

pub(in crate::editor::device_panel) fn retire_removed(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    alive: &std::collections::HashSet<hex_project::DeviceId>,
) {
    let stale: Vec<_> = view
        .device_panel
        .operator_previews
        .keys()
        .filter(|device| !alive.contains(device))
        .copied()
        .collect();
    for device in stale {
        if let Some(state) = view.device_panel.operator_previews.remove(&device) {
            for control in state.controls.into_values() {
                control.entity.update(cx, |control, cx| control.cancel(cx));
            }
        }
    }
}

/// Close any popup and cancel an in-flight pointer/text edit before this panel
/// owner is hidden. The local preview values and entities survive owner changes
/// so returning to an Operator keeps the UI where the user left it.
pub(in crate::editor::device_panel) fn cancel_interactions(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
) {
    let controls: Vec<_> = view
        .device_panel
        .operator_previews
        .values()
        .flat_map(|state| {
            state
                .controls
                .values()
                .map(|control| control.entity.clone())
        })
        .collect();
    for control in controls {
        control.update(cx, |control, cx| control.cancel(cx));
    }
}

pub(in crate::editor::device_panel) fn cancel_all(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
) {
    let previews = std::mem::take(&mut view.device_panel.operator_previews);
    for (_, state) in previews {
        for control in state.controls.into_values() {
            control.entity.update(cx, |control, cx| control.cancel(cx));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_fields_have_finite_bounded_defaults() {
        for field in Field::ALL {
            let spec = field.spec();
            assert!(spec.min.is_finite() && spec.max.is_finite() && spec.default.is_finite());
            assert!(spec.min < spec.max);
            assert!((spec.min..=spec.max).contains(&spec.default));
            if spec.logarithmic {
                assert!(spec.min > 0.0);
            }
        }
    }
}
