//! Typed device-port graph validation, sidechain/MIDI/auxiliary-output routing
//! admission, and cycle/layout/PDC enforcement (`#2680`).
//!
//! Runs exclusively on control during graph compilation/editing. Callbacks
//! consume only pre-validated routing tables and compensation delays.
use hex_lattice_contract::{
    DevicePortDescriptor, DevicePortDirection, DevicePortId, DevicePortKind, validate_device_ports,
};
use std::collections::{BTreeMap, BTreeSet};

/// One directed connection from a source node's output port to a target node's input port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DevicePortConnection {
    pub source_node: u64,
    pub source_port: DevicePortId,
    pub target_node: u64,
    pub target_port: DevicePortId,
}

/// One node participating in port-level graph validation.
#[derive(Debug, Clone, Copy)]
pub struct DevicePortNode<'a> {
    pub node_id: u64,
    pub ports: &'a [DevicePortDescriptor],
    /// Declared algorithmic processing latency of this node in project frames.
    pub latency_frames: u32,
}

/// Deterministic refusal reasons for malformed or cyclic port connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevicePortRouteError {
    DuplicateNode,
    InvalidNodePorts,
    UnknownNode,
    UnknownPort,
    IncompatibleDirection,
    IncompatibleKind,
    ChannelLayoutMismatch,
    SelfCycle,
    GraphCycle,
    DuplicateTargetInput,
}

/// Validated topological execution order and per-node input alignment delays
/// (in frames) for an acyclic device-port graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedPortGraph {
    pub topological_order: Vec<u64>,
    /// Extra delay (in frames) each connection needs so converging inputs
    /// (including sidechains and parallel auxiliary outputs) align in time.
    pub connection_compensation_frames: Vec<u32>,
}

fn kinds_compatible(source: DevicePortKind, target: DevicePortKind) -> bool {
    matches!(
        (source, target),
        (
            DevicePortKind::MainAudio | DevicePortKind::AuxiliaryAudio,
            DevicePortKind::MainAudio | DevicePortKind::SidechainAudio,
        ) | (DevicePortKind::Midi, DevicePortKind::Midi)
    )
}

/// Validate all declared node ports, port connection direction/kind/channel
/// compatibility, single-writer sidechain/MIDI/audio input rules, and DAG
/// acyclicity; compute topological order and per-connection PDC alignment delays.
pub fn validate_port_graph(
    nodes: &[DevicePortNode<'_>],
    connections: &[DevicePortConnection],
) -> Result<ValidatedPortGraph, DevicePortRouteError> {
    let mut node_map: BTreeMap<u64, DevicePortNode<'_>> = BTreeMap::new();
    for &node in nodes {
        validate_device_ports(node.ports).map_err(|_| DevicePortRouteError::InvalidNodePorts)?;
        if node_map.insert(node.node_id, node).is_some() {
            return Err(DevicePortRouteError::DuplicateNode);
        }
    }

    let mut target_ports_seen: BTreeSet<(u64, DevicePortId)> = BTreeSet::new();
    let mut adjacency: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    let mut in_degree: BTreeMap<u64, usize> = node_map.keys().map(|&id| (id, 0usize)).collect();

    for &conn in connections {
        if conn.source_node == conn.target_node {
            return Err(DevicePortRouteError::SelfCycle);
        }
        let src_node = node_map
            .get(&conn.source_node)
            .ok_or(DevicePortRouteError::UnknownNode)?;
        let dst_node = node_map
            .get(&conn.target_node)
            .ok_or(DevicePortRouteError::UnknownNode)?;
        let src_port = src_node
            .ports
            .iter()
            .find(|p| p.id == conn.source_port)
            .ok_or(DevicePortRouteError::UnknownPort)?;
        let dst_port = dst_node
            .ports
            .iter()
            .find(|p| p.id == conn.target_port)
            .ok_or(DevicePortRouteError::UnknownPort)?;

        if src_port.direction != DevicePortDirection::Output
            || dst_port.direction != DevicePortDirection::Input
        {
            return Err(DevicePortRouteError::IncompatibleDirection);
        }
        if !kinds_compatible(src_port.kind, dst_port.kind) {
            return Err(DevicePortRouteError::IncompatibleKind);
        }
        if src_port.channels != dst_port.channels {
            return Err(DevicePortRouteError::ChannelLayoutMismatch);
        }
        if !target_ports_seen.insert((conn.target_node, conn.target_port)) {
            return Err(DevicePortRouteError::DuplicateTargetInput);
        }
        adjacency
            .entry(conn.source_node)
            .or_default()
            .push(conn.target_node);
        if let Some(deg) = in_degree.get_mut(&conn.target_node) {
            *deg += 1;
        }
    }

    // Kahn's topological sort with deterministic BTreeSet ordering.
    let mut ready: BTreeSet<u64> = in_degree
        .iter()
        .filter_map(|(&id, &deg)| (deg == 0).then_some(id))
        .collect();
    let mut topological_order = Vec::with_capacity(node_map.len());
    while let Some(node_id) = ready.pop_first() {
        topological_order.push(node_id);
        if let Some(targets) = adjacency.get(&node_id) {
            for &target in targets {
                if let Some(deg) = in_degree.get_mut(&target) {
                    *deg -= 1;
                    if *deg == 0 {
                        ready.insert(target);
                    }
                }
            }
        }
    }
    if topological_order.len() != node_map.len() {
        return Err(DevicePortRouteError::GraphCycle);
    }

    // Compute cumulative arrival latency at each node's inputs and exact
    // per-connection compensation delay so converging sidechain/aux branches align.
    let mut node_output_latency: BTreeMap<u64, u32> = BTreeMap::new();
    let mut node_input_arrival: BTreeMap<u64, u32> = BTreeMap::new();
    for &node_id in &topological_order {
        let max_in = connections
            .iter()
            .filter(|c| c.target_node == node_id)
            .filter_map(|c| node_output_latency.get(&c.source_node).copied())
            .max()
            .unwrap_or(0);
        node_input_arrival.insert(node_id, max_in);
        let own = node_map[&node_id].latency_frames;
        node_output_latency.insert(node_id, max_in.saturating_add(own));
    }
    let connection_compensation_frames = connections
        .iter()
        .map(|c| {
            let target_arrival = node_input_arrival[&c.target_node];
            let source_out = node_output_latency[&c.source_node];
            target_arrival.saturating_sub(source_out)
        })
        .collect();

    Ok(ValidatedPortGraph {
        topological_order,
        connection_compensation_frames,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{
        MIDI_CONTROLLED_STEREO_EFFECT_PORTS, MIDI_EFFECT_PORTS, PORT_MAIN_AUDIO_IN,
        PORT_MAIN_AUDIO_OUT, PORT_MIDI_IN, PORT_MIDI_OUT, PORT_SIDECHAIN_AUDIO_IN,
        SIDECHAIN_STEREO_EFFECT_PORTS, STEREO_AUDIO_EFFECT_PORTS, STEREO_INSTRUMENT_PORTS,
    };

    const AUX_PORT_OUT: DevicePortId = DevicePortId::from_static(10);
    const MULTI_OUT_INSTRUMENT_PORTS: &[DevicePortDescriptor] = &[
        DevicePortDescriptor {
            id: PORT_MIDI_IN,
            name: "midi in",
            direction: DevicePortDirection::Input,
            kind: DevicePortKind::Midi,
            channels: 0,
            optional: false,
        },
        DevicePortDescriptor {
            id: PORT_MAIN_AUDIO_OUT,
            name: "main out",
            direction: DevicePortDirection::Output,
            kind: DevicePortKind::MainAudio,
            channels: 2,
            optional: false,
        },
        DevicePortDescriptor {
            id: AUX_PORT_OUT,
            name: "kick aux",
            direction: DevicePortDirection::Output,
            kind: DevicePortKind::AuxiliaryAudio,
            channels: 2,
            optional: true,
        },
    ];

    const MONO_EFFECT_PORTS: &[DevicePortDescriptor] = &[
        DevicePortDescriptor {
            id: PORT_MAIN_AUDIO_IN,
            name: "mono in",
            direction: DevicePortDirection::Input,
            kind: DevicePortKind::MainAudio,
            channels: 1,
            optional: false,
        },
        DevicePortDescriptor {
            id: PORT_MAIN_AUDIO_OUT,
            name: "mono out",
            direction: DevicePortDirection::Output,
            kind: DevicePortKind::MainAudio,
            channels: 1,
            optional: false,
        },
    ];

    #[test]
    fn validates_multi_output_sidechain_and_midi_controlled_effect_with_exact_pdc() {
        // Node 1: MIDI effect (0 latency) -> feeds Node 2 (multi-out inst, 64f latency)
        //         and Node 4 (MIDI-controlled Shifter, 128f latency).
        // Node 2: main out -> Node 3 (delayed EQ, 256f latency) -> Node 5 (sidechain Compressor main in).
        // Node 2: kick aux -> Node 5 (sidechain Compressor sidechain in).
        // Node 5 receives main at 64+256=320f and aux at 64f, so aux needs 256f compensation!
        let nodes = [
            DevicePortNode {
                node_id: 1,
                ports: MIDI_EFFECT_PORTS,
                latency_frames: 0,
            },
            DevicePortNode {
                node_id: 2,
                ports: MULTI_OUT_INSTRUMENT_PORTS,
                latency_frames: 64,
            },
            DevicePortNode {
                node_id: 3,
                ports: STEREO_AUDIO_EFFECT_PORTS,
                latency_frames: 256,
            },
            DevicePortNode {
                node_id: 4,
                ports: MIDI_CONTROLLED_STEREO_EFFECT_PORTS,
                latency_frames: 128,
            },
            DevicePortNode {
                node_id: 5,
                ports: SIDECHAIN_STEREO_EFFECT_PORTS,
                latency_frames: 0,
            },
        ];
        let connections = [
            DevicePortConnection {
                source_node: 1,
                source_port: PORT_MIDI_OUT,
                target_node: 2,
                target_port: PORT_MIDI_IN,
            },
            DevicePortConnection {
                source_node: 1,
                source_port: PORT_MIDI_OUT,
                target_node: 4,
                target_port: PORT_MIDI_IN,
            },
            DevicePortConnection {
                source_node: 2,
                source_port: PORT_MAIN_AUDIO_OUT,
                target_node: 3,
                target_port: PORT_MAIN_AUDIO_IN,
            },
            DevicePortConnection {
                source_node: 3,
                source_port: PORT_MAIN_AUDIO_OUT,
                target_node: 5,
                target_port: PORT_MAIN_AUDIO_IN,
            },
            DevicePortConnection {
                source_node: 2,
                source_port: AUX_PORT_OUT,
                target_node: 5,
                target_port: PORT_SIDECHAIN_AUDIO_IN,
            },
        ];
        let validated = validate_port_graph(&nodes, &connections).expect("valid acyclic graph");
        assert_eq!(validated.topological_order, vec![1, 2, 3, 4, 5]);
        assert_eq!(
            validated.connection_compensation_frames,
            vec![0, 0, 0, 0, 256]
        );
    }

    #[test]
    fn refuses_cycles_wrong_kinds_channel_mismatches_and_duplicate_inputs() {
        let nodes = [
            DevicePortNode {
                node_id: 1,
                ports: STEREO_INSTRUMENT_PORTS,
                latency_frames: 0,
            },
            DevicePortNode {
                node_id: 2,
                ports: SIDECHAIN_STEREO_EFFECT_PORTS,
                latency_frames: 0,
            },
            DevicePortNode {
                node_id: 3,
                ports: STEREO_AUDIO_EFFECT_PORTS,
                latency_frames: 0,
            },
            DevicePortNode {
                node_id: 4,
                ports: MONO_EFFECT_PORTS,
                latency_frames: 0,
            },
            DevicePortNode {
                node_id: 5,
                ports: MIDI_EFFECT_PORTS,
                latency_frames: 0,
            },
        ];

        // Self-cycle refused.
        assert_eq!(
            validate_port_graph(
                &nodes,
                &[DevicePortConnection {
                    source_node: 2,
                    source_port: PORT_MAIN_AUDIO_OUT,
                    target_node: 2,
                    target_port: PORT_SIDECHAIN_AUDIO_IN,
                }]
            ),
            Err(DevicePortRouteError::SelfCycle)
        );

        // 2-hop audio/sidechain feedback cycle refused.
        assert_eq!(
            validate_port_graph(
                &nodes,
                &[
                    DevicePortConnection {
                        source_node: 2,
                        source_port: PORT_MAIN_AUDIO_OUT,
                        target_node: 3,
                        target_port: PORT_MAIN_AUDIO_IN,
                    },
                    DevicePortConnection {
                        source_node: 3,
                        source_port: PORT_MAIN_AUDIO_OUT,
                        target_node: 2,
                        target_port: PORT_SIDECHAIN_AUDIO_IN,
                    },
                ]
            ),
            Err(DevicePortRouteError::GraphCycle)
        );

        // MIDI -> Audio kind mismatch refused.
        assert_eq!(
            validate_port_graph(
                &nodes,
                &[DevicePortConnection {
                    source_node: 5,
                    source_port: PORT_MIDI_OUT,
                    target_node: 2,
                    target_port: PORT_SIDECHAIN_AUDIO_IN,
                }]
            ),
            Err(DevicePortRouteError::IncompatibleKind)
        );

        // Stereo -> Mono channel mismatch refused.
        assert_eq!(
            validate_port_graph(
                &nodes,
                &[DevicePortConnection {
                    source_node: 1,
                    source_port: PORT_MAIN_AUDIO_OUT,
                    target_node: 4,
                    target_port: PORT_MAIN_AUDIO_IN,
                }]
            ),
            Err(DevicePortRouteError::ChannelLayoutMismatch)
        );

        // Two writers into one sidechain port refused.
        assert_eq!(
            validate_port_graph(
                &nodes,
                &[
                    DevicePortConnection {
                        source_node: 1,
                        source_port: PORT_MAIN_AUDIO_OUT,
                        target_node: 2,
                        target_port: PORT_SIDECHAIN_AUDIO_IN,
                    },
                    DevicePortConnection {
                        source_node: 3,
                        source_port: PORT_MAIN_AUDIO_OUT,
                        target_node: 2,
                        target_port: PORT_SIDECHAIN_AUDIO_IN,
                    },
                ]
            ),
            Err(DevicePortRouteError::DuplicateTargetInput)
        );

        // Target without SidechainAudio port refused with UnknownPort.
        assert_eq!(
            validate_port_graph(
                &nodes,
                &[DevicePortConnection {
                    source_node: 1,
                    source_port: PORT_MAIN_AUDIO_OUT,
                    target_node: 3,
                    target_port: PORT_SIDECHAIN_AUDIO_IN,
                }]
            ),
            Err(DevicePortRouteError::UnknownPort)
        );
    }
}
