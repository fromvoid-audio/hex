//! Vertical composition only: descriptors and previously created controls stay live.
use super::*;
use gpui::{Focusable, ScrollHandle, point};
use std::{cell::Cell, rc::Rc};

const CELL_WIDTH: f32 = 136.0;
const GAP: f32 = 2.0;
const PAD: f32 = hex_widgets::DEVICE_BODY_INSET;

#[derive(Clone, Debug, PartialEq)]
struct Row {
    first: usize,
    end: usize,
    top: f32,
    height: f32,
}

#[derive(Default)]
pub(super) struct RowWindow {
    pub(super) scroll: ScrollHandle,
    identity: Option<(NativeDeviceClass, hex_lattice_contract::DeviceTypeId)>,
    bounds: Rc<Cell<(f32, f32)>>,
    rows: Vec<Row>,
    ids: Vec<hex_lattice_contract::ParameterId>,
}

impl RowWindow {
    fn prepare(&mut self, parameters: &[NativeDeviceParameterView]) {
        let measured = self.bounds.get().0;
        let width = if measured.is_finite() && measured > 0.0 {
            measured
        } else {
            DEVICE_FRAME_WIDTH - 2.0 * hex_widgets::DEVICE_FRAME_BORDER_WIDTH
        };
        let columns = (((width - 2.0 * PAD + GAP) / (CELL_WIDTH + GAP)).floor() as usize).max(1);
        let mut top = PAD;
        let rows: Vec<_> = parameters
            .chunks(columns)
            .enumerate()
            .map(|(index, cells)| {
                let height = cells.iter().map(cell_height).fold(0.0_f32, f32::max);
                let row = Row {
                    first: index * columns,
                    end: index * columns + cells.len(),
                    top,
                    height,
                };
                top += height + GAP;
                row
            })
            .collect();
        let ids: Vec<_> = parameters.iter().map(|p| p.descriptor.id).collect();
        if self.rows != rows || self.ids != ids {
            let offset = f32::from(self.scroll.offset().y);
            let anchor = self
                .rows
                .iter()
                .filter(|row| row.top + row.height + offset > 0.0)
                .find_map(|row| {
                    self.ids[row.first..row.end].iter().find_map(|id| {
                        ids.iter()
                            .position(|candidate| candidate == id)
                            .map(|index| (row.top, index))
                    })
                });
            if let Some((old_top, index)) = anchor
                && let Some(new) = rows
                    .iter()
                    .find(|row| row.first <= index && index < row.end)
            {
                self.scroll
                    .set_offset(point(px(0.0), px(offset + old_top - new.top)));
            }
            self.rows = rows;
            self.ids = ids;
        }
    }

    fn visible(&self, row: &Row) -> bool {
        let measured = self.bounds.get().1;
        let height = if measured.is_finite() && measured > 0.0 {
            measured
        } else {
            DEVICE_FRAME_BODY_HEIGHT
        };
        let offset = f32::from(self.scroll.offset().y)
            .clamp(-f32::from(self.scroll.max_offset().height), 0.0);
        // One maximum-height row on each side of the explicit body viewport.
        row.top + row.height + offset >= -26.0 && row.top + offset <= height + 26.0
    }
}

fn compatible(parameter: &NativeDeviceParameterView) -> bool {
    matches!(
        (parameter.descriptor.kind, parameter.value),
        (ParameterKind::Float { .. }, ParameterValue::Float(_))
            | (ParameterKind::Bool, ParameterValue::Bool(_))
            | (ParameterKind::Choice { .. }, ParameterValue::Choice(_))
    )
}

fn cell_height(parameter: &NativeDeviceParameterView) -> f32 {
    if compatible(parameter) && matches!(parameter.descriptor.kind, ParameterKind::Float { .. }) {
        26.0
    } else {
        22.0
    }
}

/// Reconcile all cached descriptors, including horizontally/vertically hidden ones.
/// This never creates an entity for a parameter that has not mounted.
pub(super) fn reconcile(view: &mut EditorView, cx: &mut Context<EditorView>) {
    let descriptors: HashMap<_, _> = view
        .device_panel
        .views
        .iter()
        .flat_map(|(device, (class, _, result))| {
            result.as_ref().ok().into_iter().flat_map(move |metadata| {
                metadata.parameters.iter().map(move |parameter| {
                    (
                        ParameterKey {
                            device: *device,
                            parameter: parameter.descriptor.id,
                        },
                        (
                            *class,
                            metadata.type_id,
                            parameter.descriptor,
                            compatible(parameter),
                            parameter.enabled,
                        ),
                    )
                })
            })
        })
        .collect();
    // Host trim is bound to the selected Arrangement lane. Its reserved
    // descriptor never appears in the provider's parameter inventory.
    let trim = view.selected_automation_trim();
    let stale: Vec<_> =
        view.device_panel
            .controls
            .iter()
            .filter_map(|(key, control)| {
                if trim.is_some_and(|trim| {
                    trim.device == key.device
                        && trim.kind
                            == super::super::native_automation_recording::automation_target(
                                key.parameter,
                            )
                }) {
                    return (!control.binding().current(view, true)).then_some(*key);
                }
                let (old_class, old_type, old_descriptor) = control.metadata();
                let valid = descriptors.get(key).is_some_and(
                    |(class, type_id, descriptor, compatible, _)| {
                        *class == old_class
                            && *type_id == old_type
                            && *descriptor == old_descriptor
                            && *compatible
                    },
                );
                (!valid).then_some(*key)
            })
            .collect();
    for key in stale {
        if let Some(mut control) = view.device_panel.controls.remove(&key) {
            cancel_control(view, cx, key, &mut control);
        }
        view.device_panel.pending_values.remove(&key);
    }
    // Disabling a cached control must cancel capture/popups even when hidden.
    for (key, control) in &view.device_panel.controls {
        if let Some((_, _, _, _, enabled)) = descriptors.get(key) {
            match control {
                ParameterControl::Float { entity, .. } => {
                    entity.update(cx, |control, cx| control.set_enabled(*enabled, cx))
                }
                ParameterControl::Bool { entity, .. } => {
                    entity.update(cx, |control, cx| control.set_enabled(*enabled, cx))
                }
                ParameterControl::Choice { entity, .. } => {
                    entity.update(cx, |control, cx| control.set_enabled(*enabled, cx))
                }
            }
        }
    }
    view.device_panel.parameter_rows.retain(|device, rows| {
        view.device_panel
            .views
            .get(device)
            .is_some_and(|(class, state, _)| rows.identity == Some((*class, state.type_id())))
    });
}

pub(super) fn pinned(view: &EditorView, key: ParameterKey, window: &Window, cx: &App) -> bool {
    view.device_panel.pending_values.contains_key(&key)
        || view
            .device_gesture
            .snapshot()
            .is_some_and(|snapshot| match snapshot.target {
                GestureTarget::DeviceParameter { device, parameter } => {
                    device == key.device && parameter == key.parameter
                }
                GestureTarget::DeviceParameterPair {
                    device,
                    first,
                    second,
                } => device == key.device && (first == key.parameter || second == key.parameter),
                GestureTarget::DeviceParameterSnapshot { device }
                | GestureTarget::EqNodeCreation { device } => device == key.device,
                GestureTarget::ReturnSend { .. }
                | GestureTarget::PluginParameter { .. }
                | GestureTarget::TrackGain { .. }
                | GestureTarget::TrackGains { .. }
                | GestureTarget::TrackBalance { .. }
                | GestureTarget::MainGain
                | GestureTarget::ReturnGain { .. } => false,
            })
        || view
            .device_panel
            .controls
            .get(&key)
            .is_some_and(|control| match control {
                ParameterControl::Float { entity, .. } => entity.read_with(cx, |control, cx| {
                    control.is_editing()
                        || control.is_interacting()
                        || control.focus_handle(cx).is_focused(window)
                }),
                ParameterControl::Bool { entity, .. } => entity.read_with(cx, |control, cx| {
                    control.focus_handle(cx).is_focused(window)
                }),
                ParameterControl::Choice { entity, .. } => entity.read_with(cx, |control, cx| {
                    control.is_open() || control.focus_handle(cx).is_focused(window)
                }),
            })
}

pub(super) fn body(
    view: &mut EditorView,
    window: Option<&Window>,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    metadata: &NativeDeviceView,
) -> Stateful<Div> {
    if window.is_none() {
        let controls = metadata
            .parameters
            .iter()
            .map(|parameter| parameter_control(view, cx, device, identity, parameter));
        return crate::device_composition::wrapped_body(controls)
            .id(ElementId::Name(
                format!("device-body-{}", device.value()).into(),
            ))
            .debug_selector(move || format!("device-body-{}", device.value()))
            .overflow_y_scroll();
    }
    let state = view.device_panel.parameter_rows.entry(device).or_default();
    state.identity = Some((identity.class, identity.type_id));
    state.prepare(&metadata.parameters);
    let scroll = state.scroll.clone();
    let bounds = state.bounds.clone();
    let rows = state.rows.clone();
    let visible: Vec<_> = rows
        .iter()
        .map(|row| window.is_none() || state.visible(row))
        .collect();
    let mut content = div().flex().flex_col().w_full();
    if metadata.parameters.is_empty() {
        content = content.child(menu_message("no parameters"));
    }
    let mut skipped = 0.0;
    for (index, row) in rows.iter().enumerate() {
        let parameters = &metadata.parameters[row.first..row.end];
        let mounted = visible[index]
            || window.is_some_and(|window| {
                parameters.iter().any(|p| {
                    pinned(
                        view,
                        ParameterKey {
                            device,
                            parameter: p.descriptor.id,
                        },
                        window,
                        cx,
                    )
                })
            });
        let extent = row.height + if index + 1 < rows.len() { GAP } else { 0.0 };
        if !mounted {
            skipped += extent;
            continue;
        }
        if skipped > 0.0 {
            content = content.child(div().w_full().h(px(skipped)).flex_shrink_0());
            skipped = 0.0;
        }
        let mut cells = div()
            .debug_selector(move || format!("device-row-{}-{}", device.value(), index))
            .flex()
            .gap(px(GAP))
            .w_full()
            .h(px(extent))
            .flex_shrink_0();
        for parameter in parameters {
            cells = cells.child(parameter_control(view, cx, device, identity, parameter));
        }
        content = content.child(cells);
    }
    if skipped > 0.0 {
        content = content.child(div().w_full().h(px(skipped)).flex_shrink_0());
    }
    let entity = cx.entity();
    let expected_offset = scroll.offset();
    let observed_scroll = scroll.clone();
    crate::device_composition::windowed_body(content)
        .on_children_prepainted(move |_, _, cx| {
            let size = observed_scroll.bounds().size;
            let measured = (f32::from(size.width), f32::from(size.height));
            let resized = measured.0.is_finite()
                && measured.1.is_finite()
                && measured.0 > 0.0
                && measured.1 > 0.0
                && bounds.replace(measured) != measured;
            if resized || observed_scroll.offset() != expected_offset {
                let entity = entity.clone();
                cx.defer(move |cx| entity.update(cx, |_, cx| cx.notify()));
            }
        })
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
        .w_full()
        .h(px(DEVICE_FRAME_BODY_HEIGHT))
        .min_h_0()
        .overflow_y_scroll()
        .track_scroll(&scroll)
        .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    pub(in crate::editor::device_panel) fn parameters() -> Vec<NativeDeviceParameterView> {
        (1..=72)
            .map(|id| {
                let (kind, value) = match id % 6 {
                    0 | 1 => (
                        ParameterKind::Float {
                            min: 0.0,
                            max: 1.0,
                            unit: None,
                        },
                        ParameterValue::Float(0.5),
                    ),
                    2 | 3 => (ParameterKind::Bool, ParameterValue::Bool(false)),
                    _ => (
                        ParameterKind::Choice {
                            labels: &["first", "second"],
                        },
                        ParameterValue::Choice(0),
                    ),
                };
                let descriptor = ParameterDescriptor {
                    id: hex_lattice_contract::ParameterId::from_static(id),
                    label: "row value",
                    kind,
                    default: value,
                    presentation: ParameterPresentation::Linear,
                };
                NativeDeviceParameterView {
                    descriptor,
                    semantics: None,
                    value: if id == 10 {
                        ParameterValue::Float(0.5)
                    } else {
                        value
                    },
                    enabled: id != 10,
                }
            })
            .collect()
    }

    #[test]
    fn mixed_rows_preserve_order_extent_resize_and_surviving_scroll_anchor() {
        let mut parameters = parameters();
        let mut state = RowWindow::default();
        state.prepare(&parameters);
        assert_eq!(state.rows.len(), 36);
        assert_eq!(
            state.rows[0],
            Row {
                first: 0,
                end: 2,
                top: 3.0,
                height: 26.0
            }
        );
        assert_eq!(
            state.rows[1],
            Row {
                first: 2,
                end: 4,
                top: 31.0,
                height: 22.0
            }
        );
        assert_eq!(
            state.rows[2],
            Row {
                first: 4,
                end: 6,
                top: 55.0,
                height: 26.0
            }
        );
        let ids: Vec<_> = parameters.iter().map(|p| p.descriptor.id).collect();
        assert_eq!(state.ids, ids);
        state.scroll.set_offset(point(px(0.0), px(-300.0)));
        let old_anchor = state
            .rows
            .iter()
            .find(|row| row.top + row.height > 300.0)
            .unwrap()
            .clone();
        let anchor = state.ids[old_anchor.first];
        state.bounds.set((142.0, 100.0));
        state.prepare(&parameters);
        assert_eq!(state.rows.len(), 72);
        let new = state
            .rows
            .iter()
            .find(|row| state.ids[row.first] == anchor)
            .unwrap();
        assert_eq!(
            new.top + f32::from(state.scroll.offset().y),
            old_anchor.top - 300.0
        );
        parameters.remove(0);
        state.prepare(&parameters);
        assert_eq!(state.ids.len(), 71);
        assert!(!state.ids.contains(&ids[0]));
        parameters.insert(
            0,
            NativeDeviceParameterView {
                descriptor: ParameterDescriptor {
                    id: hex_lattice_contract::ParameterId::from_static(100),
                    label: "unsupported",
                    kind: ParameterKind::Bool,
                    default: ParameterValue::Bool(false),
                    presentation: ParameterPresentation::Linear,
                },
                semantics: None,
                value: ParameterValue::Float(0.5),
                enabled: false,
            },
        );
        state.prepare(&parameters);
        assert_eq!(
            state.rows[0].height, 22.0,
            "unsupported placeholder geometry preserved"
        );
        assert_eq!(state.ids[0].value(), 100);
    }
}
