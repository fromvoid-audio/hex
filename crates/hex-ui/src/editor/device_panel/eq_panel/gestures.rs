//! Pointer edit plans capture the stable node, parameter pair and worker key.
use super::*;
use hex_session::device_gesture::GestureKey;

#[derive(Clone, Copy)]
pub(super) struct PairDrag {
    node: Node,
    first: ParameterId,
    second: ParameterId,
    key: GestureKey,
    origin_y: f32,
    mode: PairMode,
}

#[derive(Clone, Copy)]
enum PairMode {
    Gain,
    Q,
    OptionQ,
}

pub(super) fn paired_event(
    view: &mut EditorView,
    device: DeviceId,
    node: Node,
    event: ResponseGraphEvent,
) {
    if event.phase == ResponseGraphPhase::Begin {
        // A refused new Begin must never reuse the previous physical stream's
        // plan, even while its terminal worker acknowledgment is pending.
        if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
            panel.pair_drag = None;
        }
        let mode = if node.kind == eq::NodeKind::Bell
            && event.modifiers.alt
            && !event.modifiers.platform
            && !event.modifiers.control
        {
            PairMode::OptionQ
        } else if matches!(
            node.kind,
            eq::NodeKind::LowCut | eq::NodeKind::HighCut | eq::NodeKind::Notch
        ) {
            PairMode::Q
        } else {
            PairMode::Gain
        };
        let Ok(first) = node.id.parameter(NodeParameter::Frequency) else {
            return;
        };
        let Ok(second) = node.id.parameter(if matches!(mode, PairMode::Gain) {
            NodeParameter::Gain
        } else {
            NodeParameter::Q
        }) else {
            return;
        };
        let initial = ParameterPairValue {
            first: ParameterValue::Float(node.frequency_hz),
            second: ParameterValue::Float(if matches!(mode, PairMode::Gain) {
                node.gain_db
            } else {
                node.q
            }),
        };
        if let Err(error) = view.begin_device_pair_gesture(device, first, second, initial) {
            view.device_panel.message = Some(error.into());
            return;
        }
        let Some(snapshot) = view.device_gesture.snapshot() else {
            return;
        };
        let plan = PairDrag {
            node,
            first,
            second,
            key: snapshot.key,
            origin_y: event.position[1],
            mode,
        };
        if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
            panel.pair_drag = Some(plan);
        }
        return;
    }
    let Some(plan) = view
        .device_panel
        .eq_panels
        .get(&device)
        .and_then(|panel| panel.pair_drag)
    else {
        return;
    };
    if node.id != plan.node.id || node.kind != plan.node.kind
        || !view.device_gesture.snapshot().is_some_and(|snapshot|
            snapshot.key == plan.key && snapshot.key.generation == view.project_generation
            && matches!(snapshot.target, GestureTarget::DeviceParameterPair {device:d,first:f,second:s}
                if d == device && f == plan.first && s == plan.second)) { return; }
    if event.phase == ResponseGraphPhase::Cancel {
        let _ = view.cancel_device_gesture();
        for parameter in [plan.first, plan.second] {
            view.device_panel
                .pending_values
                .remove(&ParameterKey { device, parameter });
        }
        if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
            panel.pair_drag = None;
        }
        return;
    }
    if !matches!(
        event.phase,
        ResponseGraphPhase::Change | ResponseGraphPhase::Commit
    ) {
        return;
    }
    let rate = view
        .state
        .as_ref()
        .map_or(48_000, |state| state.project.sample_rate());
    let values = ParameterPairValue {
        first: ParameterValue::Float(if matches!(plan.mode, PairMode::OptionQ) {
            plan.node.frequency_hz
        } else {
            frequency_from_x(event.position[0], rate)
        }),
        second: ParameterValue::Float(match plan.mode {
            PairMode::Gain => event.position[1] * 48.0 - 24.0,
            PairMode::Q => q_from_y(event.position[1]),
            // Use displacement from the pressed node, rather than mapping its
            // gain coordinate to an unrelated absolute Q at the drag threshold.
            PairMode::OptionQ => {
                group_move::option_q(plan.node.q, event.position[1] - plan.origin_y)
            }
        }),
    };
    let result = if event.phase == ResponseGraphPhase::Commit {
        if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
            panel.pair_drag = None;
        }
        view.finish_device_pair_gesture(values)
    } else {
        view.change_device_pair_gesture(values).map(|_| ())
    };
    if let Err(error) = result {
        view.device_panel.message = Some(error.into());
    } else {
        for (parameter, value) in [(plan.first, values.first), (plan.second, values.second)] {
            view.device_panel
                .pending_values
                .insert(ParameterKey { device, parameter }, value);
        }
    }
}

/// One grouped-node drag: every node keeps its press-time position plus the
/// pointer delta. Frequency and the vertical field (gain, or Q for cut/notch)
/// travel in one keyed parameter-snapshot gesture, so one Undo reverts it.
#[derive(Clone)]
pub(in crate::editor::device_panel) struct GroupDrag {
    key: GestureKey,
    origin: [f32; 2],
    nodes: Vec<(Node, [f32; 2])>,
}

fn group_snapshot(
    nodes: &[(Node, [f32; 2])],
    delta: [f32; 2],
    rate: u32,
) -> Result<hex_lattice_contract::DeviceParameterSnapshot, &'static str> {
    if nodes.len() > group_move::MAX_GROUP_NODES {
        return Err("eq group move is limited to 32 nodes");
    }
    let mut entries = Vec::with_capacity(nodes.len() * 2);
    for (node, origin) in nodes {
        let (frequency_hz, vertical) = group_move::values(node.kind, *origin, delta, rate);
        let (field, value) = match vertical {
            group_move::Vertical::Q(value) => (NodeParameter::Q, value),
            group_move::Vertical::Gain(value) => (NodeParameter::Gain, value),
        };
        let frequency = node
            .id
            .parameter(NodeParameter::Frequency)
            .map_err(|_| "eq group node is invalid")?;
        let second = node
            .id
            .parameter(field)
            .map_err(|_| "eq group node is invalid")?;
        entries.push((frequency, ParameterValue::Float(frequency_hz)));
        entries.push((second, ParameterValue::Float(value)));
    }
    hex_lattice_contract::DeviceParameterSnapshot::new(&entries)
        .map_err(|_| "eq group move is limited to 32 nodes")
}

pub(super) fn group_event(
    view: &mut EditorView,
    device: DeviceId,
    group: &[NodeId],
    event: ResponseGraphEvent,
) {
    let rate = view
        .state
        .as_ref()
        .map_or(48_000, |state| state.project.sample_rate());
    if event.phase == ResponseGraphPhase::Begin {
        if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
            panel.group_drag = None;
        }
        if view.device_gesture.snapshot().is_some() {
            view.device_panel.message = Some("wait for the current device edit, then retry".into());
            return;
        }
        let Some(state) = current(view, device).and_then(|state| EqState::decode(&state).ok())
        else {
            return;
        };
        let nodes: Vec<_> = group
            .iter()
            .filter_map(|id| state.node(*id).ok().copied())
            .map(|node| (node, node_position(&node, rate)))
            .collect();
        let result = group_snapshot(&nodes, [0.0, 0.0], rate).and_then(|initial| {
            view.device_gesture
                .begin_snapshot(view.project_generation, device, initial)
        });
        match result {
            Ok(key) => {
                if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
                    panel.group_drag = Some(GroupDrag {
                        key,
                        origin: event.position,
                        nodes,
                    });
                }
            }
            Err(error) => view.device_panel.message = Some(error.into()),
        }
        return;
    }
    let Some(plan) = view
        .device_panel
        .eq_panels
        .get_mut(&device)
        .and_then(|panel| panel.group_drag.take())
    else {
        return;
    };
    let owned = view.device_gesture.snapshot().is_some_and(|snapshot| {
        snapshot.key == plan.key
            && snapshot.key.generation == view.project_generation
            && snapshot.target == GestureTarget::DeviceParameterSnapshot { device }
    }) && !view.device_gesture.has_pending_terminal();
    if !owned {
        return;
    }
    let delta = [
        event.position[0] - plan.origin[0],
        event.position[1] - plan.origin[1],
    ];
    let result = match event.phase {
        ResponseGraphPhase::Cancel => view.cancel_device_gesture(),
        ResponseGraphPhase::Commit => group_snapshot(&plan.nodes, delta, rate)
            .and_then(|values| view.device_gesture.finish_snapshot(values)),
        ResponseGraphPhase::Change => {
            let result = group_snapshot(&plan.nodes, delta, rate)
                .and_then(|values| view.device_gesture.change_snapshot(values).map(|_| ()));
            if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
                panel.group_drag = Some(plan);
            }
            result
        }
        _ => Ok(()),
    };
    if let Err(error) = result {
        view.device_panel.message = Some(error.into());
    }
}

#[derive(Clone, Copy)]
pub(super) struct SlopeScroll {
    pub(super) key: GestureKey,
    pub(super) stream: u64,
    pub(super) node: NodeId,
    pub(super) parameter: ParameterId,
    pub(super) value: u32,
    pub(super) last_steps: i32,
}

pub(super) fn scroll_nodes(
    view: &EditorView,
    device: DeviceId,
    state: &EqState,
    cx: &App,
) -> Vec<u32> {
    let Some(panel) = view.device_panel.eq_panels.get(&device) else {
        return Vec::new();
    };
    let editing = view.device_panel.controls.values().any(|control| matches!(control,
        ParameterControl::Float {entity,..} if entity.read(cx).is_editing() || entity.read(cx).is_interacting()));
    if editing || panel.creation.is_some() {
        return Vec::new();
    }
    state
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, eq::NodeKind::LowCut | eq::NodeKind::HighCut))
        .map(|node| node.id.value())
        .collect()
}

pub(super) fn scroll_event(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    generation: u64,
    event: hex_widgets::ResponseGraphScrollEvent,
) {
    if generation != view.project_generation || event.gesture_id == 0 {
        return;
    }
    if event.phase == hex_widgets::ResponseGraphScrollPhase::Begin
        && !current(view, device)
            .and_then(|s| EqState::decode(&s).ok())
            .is_some_and(|state| scroll_nodes(view, device, &state, cx).contains(&event.node_id))
    {
        return;
    }
    if !scroll_queue::intercept(view, device, event) {
        scroll_event_core(view, device, generation, event);
    }
    cx.notify();
}

pub(super) fn scroll_event_core(
    view: &mut EditorView,
    device: DeviceId,
    generation: u64,
    event: hex_widgets::ResponseGraphScrollEvent,
) {
    use hex_widgets::ResponseGraphScrollPhase as Phase;
    if generation != view.project_generation || event.gesture_id == 0 {
        return;
    }
    if event.phase == Phase::Begin {
        if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
            panel.slope_scroll = None;
        }
        let Some(expected) = current(view, device) else {
            return;
        };
        let Ok(state) = EqState::decode(&expected) else {
            return;
        };
        let Ok(id) = NodeId::new(u64::from(event.node_id)) else {
            return;
        };
        let Ok(node) = state.node(id) else { return };
        if !matches!(node.kind, eq::NodeKind::LowCut | eq::NodeKind::HighCut)
            || !view
                .device_panel
                .eq_panels
                .get(&device)
                .is_some_and(|panel| panel.authored_state == expected)
            || view.worker_requests_pending != 0
            || !view.pending_requests.is_empty()
            || !view.device_gesture_barriers.is_empty()
            || view.device_gesture.snapshot().is_some()
        {
            view.device_panel.message = Some("wait for the current device edit, then retry".into());
            return;
        }
        let Ok(parameter) = id.parameter(NodeParameter::Slope) else {
            return;
        };
        let value = u32::from(node.slope);
        if let Err(error) = view.begin_device_gesture(
            GestureTarget::DeviceParameter { device, parameter },
            ParameterValue::Choice(value),
        ) {
            view.device_panel.message = Some(error.into());
            return;
        }
        let Some(snapshot) = view.device_gesture.snapshot() else {
            return;
        };
        let scroll = SlopeScroll {
            key: snapshot.key,
            stream: event.gesture_id,
            node: id,
            parameter,
            value,
            last_steps: 0,
        };
        if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
            panel.slope_scroll = Some(scroll);
            panel.selected = Some(id);
        }
        return;
    }
    let Some(mut scroll) = view
        .device_panel
        .eq_panels
        .get(&device)
        .and_then(|panel| panel.slope_scroll)
    else {
        return;
    };
    let owns_key = view.device_gesture.snapshot().is_some_and(|snapshot| {
        snapshot.key == scroll.key
            && snapshot.key.generation == generation
            && matches!(snapshot.target, GestureTarget::DeviceParameter { device: d, parameter: p }
                if d == device && p == scroll.parameter)
    });
    if scroll.node.value() != event.node_id || scroll.stream != event.gesture_id || !owns_key {
        return;
    }
    if event.phase == Phase::Cancel {
        let _ = view.cancel_device_gesture();
        view.device_panel.pending_values.remove(&ParameterKey {
            device,
            parameter: scroll.parameter,
        });
        if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
            panel.slope_scroll = None;
        }
        return;
    }
    if !current(view, device)
        .and_then(|state| EqState::decode(&state).ok())
        .is_some_and(|state| {
            state
                .node(scroll.node)
                .is_ok_and(|node| matches!(node.kind, eq::NodeKind::LowCut | eq::NodeKind::HighCut))
        })
    {
        return;
    }
    let delta = event.steps.saturating_sub(scroll.last_steps);
    scroll.value = slope::advance(scroll.value, delta);
    scroll.last_steps = event.steps;
    let value = ParameterValue::Choice(scroll.value);
    let result = if event.phase == Phase::Commit {
        if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
            panel.slope_scroll = None;
        }
        view.finish_device_gesture(value)
    } else {
        if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
            panel.slope_scroll = Some(scroll);
        }
        view.change_device_gesture(value)
    };
    if let Err(error) = result {
        view.device_panel.message = Some(error.into());
    } else {
        if event.phase == Phase::Commit {
            scroll_queue::retiring(view, device, scroll);
        }
        view.device_panel.pending_values.insert(
            ParameterKey {
                device,
                parameter: scroll.parameter,
            },
            value,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grouped_node_limit_matches_the_host_snapshot_capacity() {
        assert_eq!(
            group_move::MAX_GROUP_NODES * 2,
            hex_lattice_contract::MAX_PARAMETER_SNAPSHOT_ENTRIES
        );
        let mut nodes: Vec<_> = (1..=32)
            .map(|raw| {
                (
                    Node::new(NodeId::new(raw).unwrap(), 1_000.0, 0.0, 0).unwrap(),
                    [0.5, 0.5],
                )
            })
            .collect();
        assert!(group_snapshot(&nodes, [0.0, 0.0], 48_000).is_ok());
        nodes.push((
            Node::new(NodeId::new(33).unwrap(), 1_000.0, 0.0, 0).unwrap(),
            [0.5, 0.5],
        ));
        assert_eq!(
            group_snapshot(&nodes, [0.0, 0.0], 48_000).unwrap_err(),
            "eq group move is limited to 32 nodes"
        );
    }
    #[test]
    fn option_displacement_keeps_initial_q_and_bounds_without_gain_coordinate_jump() {
        for initial in [0.1, 0.707, 1.0, 18.0] {
            assert_eq!(group_move::option_q(initial, 0.0), initial);
            assert_eq!(group_move::option_q(initial, -2.0), 0.1);
            assert_eq!(group_move::option_q(initial, 2.0), 18.0);
            assert!(group_move::option_q(initial, 0.1) >= initial);
        }
    }
}
