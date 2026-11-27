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
        let custom_summary = view
            .state
            .as_ref()
            .and_then(|s| s.project.device(device))
            .and_then(|d| hex_engine::native_operator_fm_summary(d.state()).ok());
        let mut custom_tile = div()
            .debug_selector(|| "operator-routing-custom".into())
            .w(px(38.0))
            .h_full()
            .flex_shrink_0()
            .relative()
            .flex()
            .items_end()
            .justify_center()
            .child(detail_caption("custom"));
        if let Some(summary) = custom_summary {
            custom_tile = custom_tile.children([custom_diagram(summary, true)]);
        }
        row = row.children([custom_tile]);
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

const NODE_SIZE: f32 = 3.6;
const NODE_HALF: f32 = NODE_SIZE * 0.5;
const OUTPUT_BUS_INSET: f32 = 3.0;

fn diagram_node_centers(index: usize, width: f32, height: f32) -> [[f32; 2]; 4] {
    let positions = POSITIONS[index];
    std::array::from_fn(|operator| {
        [
            3.0 + (width - 6.0) * positions[operator][0],
            2.0 + (height - 10.0) * positions[operator][1],
        ]
    })
}

/// Topological rank-based node layout for arbitrary acyclic 4-operator FM DAGs.
/// Carriers (out-degree 0) sit on the lowest rank above the output bus, and
/// modulators sit at rank `1 + max(rank[target])` above them.
fn dag_node_centers(edges: &[(usize, usize)], width: f32, height: f32) -> [[f32; 2]; 4] {
    let mut rank = [0usize; 4];
    for _ in 0..4 {
        for &(src, dst) in edges {
            if src < 4 && dst < 4 && src != dst {
                rank[src] = rank[src].max(rank[dst] + 1);
            }
        }
    }
    let max_rank = rank.into_iter().max().unwrap_or(0).max(1);
    let mut per_rank_counts = [0usize; 4];
    let mut slot_in_rank = [0usize; 4];
    for op in 0..4 {
        let r = rank[op].min(3);
        slot_in_rank[op] = per_rank_counts[r];
        per_rank_counts[r] += 1;
    }
    let usable_w = (width - 6.0).max(NODE_SIZE * 4.0);
    let usable_h = (height - 10.0).max(NODE_SIZE * 2.0);
    std::array::from_fn(|op| {
        let r = rank[op].min(3);
        let count = per_rank_counts[r].max(1);
        let x_norm = if count == 1 {
            0.5
        } else {
            0.1 + 0.8 * (slot_in_rank[op] as f32 / (count - 1) as f32)
        };
        let y_norm = if max_rank == 0 {
            0.55
        } else {
            0.85 - 0.70 * (r as f32 / max_rank as f32)
        };
        [3.0 + usable_w * x_norm, 2.0 + usable_h * y_norm]
    })
}

pub(super) fn custom_diagram(
    summary: hex_engine::OperatorFmSummary,
    selected: bool,
) -> impl IntoElement {
    let edges = summary.edges();
    let carriers: Vec<usize> = (0..4).filter(|&op| summary.carriers[op]).collect();
    let colors = std::array::from_fn::<_, 4, _>(|operator| {
        theme::device_identity_style(operator, true, false).surface
    });
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let centers = dag_node_centers(
                &edges,
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
            );
            paint_topology_graph(
                bounds, &centers, &edges, &carriers, colors, selected, window,
            );
        },
    )
    .absolute()
    .size_full()
}

fn paint_topology_graph(
    bounds: gpui::Bounds<gpui::Pixels>,
    centers: &[[f32; 2]; 4],
    edges: &[(usize, usize)],
    carriers: &[usize],
    colors: [u32; 4],
    selected: bool,
    window: &mut gpui::Window,
) {
    let position = |operator: usize| {
        point(
            bounds.left() + px(centers[operator][0]),
            bounds.top() + px(centers[operator][1]),
        )
    };
    let mut path = PathBuilder::stroke(px(0.75));
    for &(source, target) in edges {
        let start = position(source);
        let end = position(target);
        path.move_to(start);
        path.line_to(end);
        let delta = end - start;
        let length = (f32::from(delta.x).powi(2) + f32::from(delta.y).powi(2)).sqrt();
        if length > 7.0 {
            let direction = delta / length;
            let middle = start + delta * 0.55;
            let side = point(-direction.y, direction.x) * 1.2;
            path.move_to(middle - direction * 1.5 + side);
            path.line_to(middle);
            path.line_to(middle - direction * 1.5 - side);
        }
    }
    let output_y = bounds.bottom() - px(OUTPUT_BUS_INSET);
    let mut left = bounds.right();
    let mut right = bounds.left();
    for &carrier in carriers {
        let node = position(carrier);
        path.move_to(node);
        path.line_to(point(node.x, output_y));
        left = left.min(node.x);
        right = right.max(node.x);
    }
    if !carriers.is_empty() {
        path.move_to(point(left, output_y));
        path.line_to(point(right, output_y));
        let output_x = (left + right) / 2.0;
        path.move_to(point(output_x, output_y));
        path.line_to(point(output_x, bounds.bottom() - px(0.5)));
    }
    if let Ok(built) = path.build() {
        window.paint_path(
            built,
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
                center - point(px(NODE_HALF), px(NODE_HALF)),
                gpui::size(px(NODE_SIZE), px(NODE_SIZE)),
            ),
            rgb(color),
        ));
    }
}

pub(super) fn diagram(index: usize, selected: bool) -> impl IntoElement {
    let topology = hex_engine::native_operator_algorithms[index];
    let colors = std::array::from_fn::<_, 4, _>(|operator| {
        theme::device_identity_style(operator, true, false).surface
    });
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let centers = diagram_node_centers(
                index,
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
            );
            paint_topology_graph(
                bounds,
                &centers,
                topology.edges,
                topology.carriers,
                colors,
                selected,
                window,
            );
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

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::*;

    #[test]
    fn all_eleven_routing_diagrams_fit_compact_tiles_without_overlap() {
        assert_eq!(
            POSITIONS.len(),
            hex_engine::native_operator_algorithms.len()
        );
        for (width, height) in [(38.0_f32, 34.0_f32), (24.0_f32, 28.0_f32)] {
            let output_y = height - OUTPUT_BUS_INSET;
            for (index, topology) in hex_engine::native_operator_algorithms.iter().enumerate() {
                assert!(!topology.carriers.is_empty());
                let centers = diagram_node_centers(index, width, height);
                for (op, [cx, cy]) in centers.iter().enumerate() {
                    let left = cx - NODE_HALF;
                    let right = cx + NODE_HALF;
                    let top = cy - NODE_HALF;
                    let bottom = cy + NODE_HALF;
                    assert!(
                        left >= 0.0 && right <= width && top >= 0.0 && bottom <= output_y,
                        "preset {index} op {op} rect [{left}, {top}, {right}, {bottom}] exceeds ({width}x{height}, bus {output_y})"
                    );
                }
                for a in 0..4 {
                    for b in (a + 1)..4 {
                        let overlap_x = (centers[a][0] - centers[b][0]).abs() < NODE_SIZE;
                        let overlap_y = (centers[a][1] - centers[b][1]).abs() < NODE_SIZE;
                        assert!(
                            !(overlap_x && overlap_y),
                            "preset {index} nodes {a} and {b} overlap at ({width}x{height})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn arbitrary_custom_dags_lay_out_topologically_without_node_overlap() {
        let cases: &[&[(usize, usize)]] = &[
            &[(0, 1), (1, 2), (2, 3)],         // reverse serial chain
            &[(0, 2), (1, 2), (2, 3), (0, 3)], // custom diamond + skip edge
            &[(3, 2), (1, 0), (0, 2)],         // custom non-catalog 3-deep merge
            &[(0, 3), (1, 3)],                 // two modulators into op d, op c parallel
            &[],                               // all parallel carriers
        ];
        for (width, height) in [(38.0_f32, 34.0_f32), (24.0_f32, 28.0_f32)] {
            let output_y = height - OUTPUT_BUS_INSET;
            for edges in cases {
                let centers = dag_node_centers(edges, width, height);
                for (op, [cx, cy]) in centers.iter().enumerate() {
                    assert!(
                        cx - NODE_HALF >= 0.0
                            && cx + NODE_HALF <= width
                            && cy - NODE_HALF >= 0.0
                            && cy + NODE_HALF <= output_y,
                        "custom DAG {edges:?} op {op} outside ({width}x{height})"
                    );
                }
                for &(src, dst) in *edges {
                    assert!(
                        centers[src][1] < centers[dst][1],
                        "modulator {src} must sit strictly above modulated target {dst} in {edges:?}"
                    );
                }
                for a in 0..4 {
                    for b in (a + 1)..4 {
                        let overlap_x = (centers[a][0] - centers[b][0]).abs() < NODE_SIZE;
                        let overlap_y = (centers[a][1] - centers[b][1]).abs() < NODE_SIZE;
                        assert!(
                            !(overlap_x && overlap_y),
                            "custom DAG {edges:?} nodes {a} and {b} overlap"
                        );
                    }
                }
            }
        }
    }
}
