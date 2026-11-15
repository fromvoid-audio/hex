//! Topology diagrams dispatch one ordinary canonical parameter edit.
use super::*;
use gpui::{PathBuilder, canvas, point};

// a, b, c, d node positions in each left-to-right diagram. Coordinates are
// presentation only; the engine catalog supplies the actual edges and carriers.
const POSITIONS: [[[f32; 2]; 4]; 11] = [
    [[0.5, 0.9], [0.5, 0.65], [0.5, 0.4], [0.5, 0.15]],
    [[0.5, 0.85], [0.5, 0.55], [0.25, 0.25], [0.75, 0.25]],
    [[0.5, 0.85], [0.25, 0.55], [0.25, 0.25], [0.75, 0.55]],
    [[0.5, 0.85], [0.25, 0.55], [0.75, 0.55], [0.5, 0.25]],
    [[0.75, 0.85], [0.25, 0.85], [0.5, 0.55], [0.5, 0.25]],
    [[0.25, 0.85], [0.75, 0.85], [0.75, 0.55], [0.75, 0.25]],
    [[0.5, 0.7], [0.15, 0.4], [0.5, 0.4], [0.85, 0.4]],
    [[0.25, 0.7], [0.25, 0.4], [0.75, 0.7], [0.75, 0.4]],
    [[0.15, 0.7], [0.5, 0.7], [0.85, 0.7], [0.5, 0.4]],
    [[0.15, 0.7], [0.5, 0.7], [0.85, 0.7], [0.85, 0.4]],
    [[0.1, 0.55], [0.37, 0.55], [0.64, 0.55], [0.91, 0.55]],
];

pub(super) fn selector(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    selected: Option<ParameterValue>,
) -> Div {
    let owner = context::Owner::capture(view, device);
    let mut row = div()
        .w_full()
        .h(px(30.0))
        .flex_shrink_0()
        .flex()
        .gap(px(1.0));
    if selected == Some(ParameterValue::Choice(0)) {
        row = row.child(
            detail_caption("custom")
                .debug_selector(|| "operator-routing-custom".into())
                .w(px(38.0))
                .h_full()
                .flex_shrink_0()
                .flex()
                .items_center(),
        );
    }
    for index in 0..11 {
        let choice = index as u32 + 1;
        let active = selected == Some(ParameterValue::Choice(choice));
        let focus = navigation_focus(view, cx, device, NavigationTarget::Algorithm(choice));
        let pointer_focus = focus.clone();
        row = row.child(
            hex_widgets::device_segment(theme::text(""), theme::device_segment_style(active))
                .id(ElementId::Name(
                    format!("operator-algorithm-{}-{choice}", device.value()).into(),
                ))
                .debug_selector(move || format!("operator-algorithm-{}-{choice}", device.value()))
                .flex_1()
                .min_w_0()
                .h_full()
                .relative()
                .track_focus(&focus)
                .cursor(gpui::CursorStyle::PointingHand)
                .tooltip(move |_, cx| {
                    cx.new(|_| crate::editor::transport::MiniTooltip {
                        kind: crate::tooltips::Kind::Identification,
                        description: theme::text(format!("algorithm {choice}")),
                    })
                    .into()
                })
                .child(diagram(index, active))
                .on_click(cx.listener(move |view, _, window, cx| {
                    if owner.is_some_and(|owner| owner.is_current(view)) {
                        pointer_focus.focus(window);
                        select(view, cx, device, choice);
                    }
                }))
                .on_key_down(cx.listener(move |view, event: &gpui::KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space")
                        && owner.is_some_and(|owner| owner.is_current(view))
                    {
                        select(view, cx, device, choice);
                        cx.stop_propagation();
                    }
                })),
        );
    }
    row
}

pub(super) fn diagram(index: usize, selected: bool) -> impl IntoElement {
    let topology = hex_engine::native_operator_algorithms[index];
    let positions = POSITIONS[index];
    let colors = std::array::from_fn::<_, 4, _>(|operator| {
        theme::device_identity_style(operator, true, false).surface
    });
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let position = |operator: usize| {
                point(
                    bounds.left()
                        + px(3.0)
                        + (bounds.size.width - px(6.0)) * positions[operator][0],
                    bounds.top()
                        + px(2.0)
                        + (bounds.size.height - px(10.0)) * positions[operator][1],
                )
            };
            let mut edges = PathBuilder::stroke(px(0.75));
            for &(source, target) in topology.edges {
                let start = position(source);
                let end = position(target);
                edges.move_to(start);
                edges.line_to(end);
                let delta = end - start;
                let length = (f32::from(delta.x).powi(2) + f32::from(delta.y).powi(2)).sqrt();
                if length > 7.0 {
                    let direction = delta / length;
                    let middle = start + delta * 0.55;
                    let side = point(-direction.y, direction.x) * 1.2;
                    edges.move_to(middle - direction * 1.5 + side);
                    edges.line_to(middle);
                    edges.line_to(middle - direction * 1.5 - side);
                }
            }
            let output_y = bounds.bottom() - px(3.0);
            let mut left = bounds.right();
            let mut right = bounds.left();
            for &carrier in topology.carriers {
                let node = position(carrier);
                edges.move_to(node);
                edges.line_to(point(node.x, output_y));
                left = left.min(node.x);
                right = right.max(node.x);
            }
            edges.move_to(point(left, output_y));
            edges.line_to(point(right, output_y));
            let output_x = (left + right) / 2.0;
            edges.move_to(point(output_x, output_y));
            edges.line_to(point(output_x, bounds.bottom() - px(0.5)));
            if let Ok(path) = edges.build() {
                window.paint_path(
                    path,
                    rgb(theme::color(if selected {
                        theme::PaletteRole::Text
                    } else {
                        theme::PaletteRole::Muted
                    })),
                );
            }
            for (operator, color) in colors.into_iter().enumerate() {
                let center = position(operator);
                window.paint_quad(gpui::fill(
                    gpui::Bounds::new(
                        center - point(px(1.8), px(1.8)),
                        gpui::size(px(3.6), px(3.6)),
                    ),
                    rgb(color),
                ));
            }
        },
    )
    .absolute()
    .size_full()
}

pub(in crate::editor::device_panel) fn select(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    choice: u32,
) {
    if !(1..=11).contains(&choice) || !finalize_controls(view, cx) {
        return;
    }
    let key = ParameterKey {
        device,
        parameter: id(600),
    };
    let canonical = view
        .device_panel
        .views
        .get(&device)
        .and_then(|(_, _, view)| view.as_ref().ok())
        .and_then(|view| {
            view.parameters
                .iter()
                .find(|parameter| parameter.descriptor.id == key.parameter)
        })
        .map(|parameter| parameter.value);
    let previous = view.device_panel.pending_values.get(&key).copied();
    let value = ParameterValue::Choice(choice);
    let mode_key = ParameterKey {
        device,
        parameter: id(608),
    };
    let legacy = view
        .device_panel
        .pending_values
        .get(&mode_key)
        .copied()
        .or_else(|| {
            view.device_panel
                .views
                .get(&device)
                .and_then(|(_, _, view)| view.as_ref().ok())
                .and_then(|view| {
                    view.parameters
                        .iter()
                        .find(|parameter| parameter.descriptor.id == mode_key.parameter)
                })
                .map(|parameter| parameter.value)
        })
        == Some(ParameterValue::Choice(0));
    if canonical.is_none()
        || previous == Some(value)
        || (!legacy && previous.or(canonical) == Some(value))
    {
        return;
    }
    view.device_panel.pending_values.insert(key, value);
    if !view.enqueue_device_barrier(hex_session::service::Request::SetDeviceParameter {
        device,
        parameter: key.parameter,
        value,
    }) {
        if let Some(previous) = previous {
            view.device_panel.pending_values.insert(key, previous);
        } else {
            view.device_panel.pending_values.remove(&key);
        }
    }
    cx.notify();
}
