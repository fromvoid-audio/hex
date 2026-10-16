use hex_project::{DeviceId, Project, ReturnId, RouteId, TrackId};
use std::{
    collections::{BTreeMap, BTreeSet},
    mem::size_of,
};
type Frame = [f32; 2];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fixed {
    Known(u64),
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum NodeKey {
    Audio(TrackId),
    Device(DeviceId),
    Instrument(DeviceId),
    Merge(TrackId),
    TrackMix(TrackId),
    ReturnInput(ReturnId),
    ReturnMix(ReturnId),
    Tail(u64),
    Main,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum EdgeKey {
    Internal(NodeKey, NodeKey),
    TrackMain(TrackId),
    Send(TrackId, ReturnId),
    Feed(RouteId),
    ReturnMain(ReturnId),
    TailMain(u64),
}
#[derive(Clone, Debug)]
pub(super) struct Node {
    pub key: NodeKey,
    pub fixed: Fixed,
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Edge {
    pub key: EdgeKey,
    pub source: usize,
    pub target: usize,
}
#[derive(Clone, Debug)]
pub(super) struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}
impl Graph {
    fn node(&mut self, key: NodeKey, fixed: Fixed) -> usize {
        let index = self.nodes.len();
        self.nodes.push(Node { key, fixed });
        index
    }
    fn edge(&mut self, key: EdgeKey, source: usize, target: usize) {
        self.edges.push(Edge {
            key,
            source,
            target,
        });
    }
    fn chain(&mut self, source: usize, target: usize) {
        self.edge(
            EdgeKey::Internal(self.nodes[source].key, self.nodes[target].key),
            source,
            target,
        );
    }
    pub fn from_project(
        project: &Project,
        declared: &BTreeMap<NodeKey, Fixed>,
        tails: &[(u64, u64)],
    ) -> Result<Self, &'static str> {
        project
            .validate_routing_proposal(&[])
            .map_err(|_| "invalid canonical graph")?;
        let mut graph = Self {
            nodes: Vec::new(),
            edges: Vec::new(),
        };
        let mut tracks = Vec::new();
        let mut returns = Vec::new();
        let declaration = |key| declared.get(&key).copied().unwrap_or(Fixed::Unknown);
        for track in project.tracks() {
            let mut cursor = graph.node(
                NodeKey::Audio(track.id()),
                declaration(NodeKey::Audio(track.id())),
            );
            let split = track.instrument_position().unwrap_or(track.devices().len());
            for device in &track.devices()[..split] {
                let key = NodeKey::Device(device.id());
                let next = graph.node(
                    key,
                    if device.bypassed() {
                        Fixed::Known(0)
                    } else {
                        declaration(key)
                    },
                );
                graph.chain(cursor, next);
                cursor = next;
            }
            if let Some(instrument) = track.instrument() {
                let key = NodeKey::Instrument(instrument.id());
                let source = graph.node(
                    key,
                    if instrument.bypassed() {
                        Fixed::Known(0)
                    } else {
                        declaration(key)
                    },
                );
                let merge = graph.node(NodeKey::Merge(track.id()), Fixed::Known(0));
                graph.chain(cursor, merge);
                graph.chain(source, merge);
                cursor = merge;
            }
            for device in &track.devices()[split..] {
                let key = NodeKey::Device(device.id());
                let next = graph.node(
                    key,
                    if device.bypassed() {
                        Fixed::Known(0)
                    } else {
                        declaration(key)
                    },
                );
                graph.chain(cursor, next);
                cursor = next;
            }
            let mix = graph.node(NodeKey::TrackMix(track.id()), Fixed::Known(0));
            graph.chain(cursor, mix);
            tracks.push((track.id(), mix));
        }
        for bus in project.return_tracks() {
            let input = graph.node(NodeKey::ReturnInput(bus.id()), Fixed::Known(0));
            let mut cursor = input;
            for device in bus.devices() {
                let key = NodeKey::Device(device.id());
                let next = graph.node(
                    key,
                    if device.bypassed() {
                        Fixed::Known(0)
                    } else {
                        declaration(key)
                    },
                );
                graph.chain(cursor, next);
                cursor = next;
            }
            let mix = graph.node(NodeKey::ReturnMix(bus.id()), Fixed::Known(0));
            graph.chain(cursor, mix);
            returns.push((bus.id(), input, mix));
        }
        let mut tail_nodes = Vec::new();
        for &(id, arrival) in tails {
            tail_nodes.push((id, graph.node(NodeKey::Tail(id), Fixed::Known(arrival))));
        }
        let main = graph.node(NodeKey::Main, Fixed::Known(0));
        for (&(_, mix), track) in tracks.iter().zip(project.tracks()) {
            graph.edge(EdgeKey::TrackMain(track.id()), mix, main);
            for send in track.sends() {
                let &(_, input, _) = returns
                    .iter()
                    .find(|bus| bus.0 == send.return_track())
                    .ok_or("send owner missing")?;
                graph.edge(EdgeKey::Send(track.id(), send.return_track()), mix, input);
            }
        }
        for &(id, _, mix) in &returns {
            graph.edge(EdgeKey::ReturnMain(id), mix, main);
        }
        let mut feeds = project.authored_routing().to_vec();
        feeds.sort_by_key(|feed| {
            (
                returns.iter().position(|bus| bus.0 == feed.source()),
                returns.iter().position(|bus| bus.0 == feed.destination()),
                feed.id(),
            )
        });
        for feed in feeds {
            let &(_, _, source) = returns
                .iter()
                .find(|bus| bus.0 == feed.source())
                .ok_or("feed source missing")?;
            let &(_, target, _) = returns
                .iter()
                .find(|bus| bus.0 == feed.destination())
                .ok_or("feed target missing")?;
            graph.edge(EdgeKey::Feed(feed.id()), source, target);
        }
        for (id, source) in tail_nodes {
            graph.edge(EdgeKey::TailMain(id), source, main);
        }
        Ok(graph)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct NodeLayout {
    pub key: NodeKey,
    pub arrival: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EdgeLayout {
    pub key: EdgeKey,
    pub delay: u64,
    pub ring_capacity: usize,
}
#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub nodes: usize,
    pub edges: usize,
    pub delay: u64,
    pub coexist_bytes: usize,
    pub node_capacity: usize,
    pub edge_capacity: usize,
    pub ring_quantum: usize,
}
#[derive(Clone, Debug, Default)]
pub(super) struct Context {
    pub previous_main_floor: u64,
    pub requested_ceiling: Option<u64>,
    pub continuous: bool,
    pub histories: BTreeMap<EdgeKey, u64>,
    pub active_bytes: usize,
    pub retained_bytes: usize,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Demand {
    pub stereo_frame_bytes: usize,
    pub ring_frames_per_bank: usize,
    pub ring_twin_bytes: usize,
    pub ring_metadata_bytes: usize,
    pub layout_bytes: usize,
    pub candidate_bytes: usize,
    pub active_bytes: usize,
    pub retained_bytes: usize,
    pub coexist_bytes: usize,
}
#[derive(Debug)]
pub(super) struct Layout {
    pub nodes: Vec<NodeLayout>,
    pub edges: Vec<EdgeLayout>,
    pub order: Vec<NodeKey>,
    pub main_latency: u64,
    pub demand: Demand,
}
fn add(a: usize, b: usize) -> Result<usize, &'static str> {
    a.checked_add(b).ok_or("byte overflow")
}
fn mul(a: usize, b: usize) -> Result<usize, &'static str> {
    a.checked_mul(b).ok_or("byte overflow")
}
pub(super) fn prepare(
    graph: &Graph,
    limits: Limits,
    context: &Context,
) -> Result<Layout, &'static str> {
    if graph.nodes.len() > limits.nodes
        || graph.edges.len() > limits.edges
        || limits.node_capacity < graph.nodes.len()
        || limits.edge_capacity < graph.edges.len()
        || limits.ring_quantum == 0
    {
        return Err("layout capacity refused");
    }
    // Reject impossible requested metadata/coexistence capacities before reserving
    // candidate vectors. Working graph/sort scratch is control-only and separate.
    let proposed_layout_bytes = add(
        add(
            add(
                size_of::<Layout>(),
                mul(limits.node_capacity, size_of::<NodeLayout>())?,
            )?,
            mul(limits.edge_capacity, size_of::<EdgeLayout>())?,
        )?,
        mul(graph.nodes.len(), size_of::<NodeKey>())?,
    )?;
    if add(
        add(proposed_layout_bytes, context.active_bytes)?,
        context.retained_bytes,
    )? > limits.coexist_bytes
    {
        return Err("coexistence capacity refused");
    }
    let mut keys = BTreeSet::new();
    let mut edge_keys = BTreeSet::new();
    let mut pairs = BTreeSet::new();
    let mut incoming = vec![0_usize; graph.nodes.len()];
    for node in &graph.nodes {
        if !keys.insert(node.key) {
            return Err("duplicate node");
        }
    }
    if !keys.contains(&NodeKey::Main) {
        return Err("main missing");
    }
    for edge in &graph.edges {
        if edge.source >= graph.nodes.len() || edge.target >= graph.nodes.len() {
            return Err("edge endpoint missing");
        }
        if !edge_keys.insert(edge.key) || !pairs.insert((edge.source, edge.target)) {
            return Err("duplicate edge");
        }
        incoming[edge.target] = incoming[edge.target]
            .checked_add(1)
            .ok_or("edge count overflow")?;
    }
    let mut arrival = vec![0_u64; graph.nodes.len()];
    let mut visited = vec![false; graph.nodes.len()];
    let mut order = Vec::with_capacity(graph.nodes.len());
    while order.len() < graph.nodes.len() {
        let index = (0..graph.nodes.len())
            .find(|&i| !visited[i] && incoming[i] == 0)
            .ok_or("cycle refused")?;
        let node = &graph.nodes[index];
        let own = match node.fixed {
            Fixed::Known(frames) => frames,
            Fixed::Unknown => return Err("unknown latency"),
        };
        let mut input = graph
            .edges
            .iter()
            .filter(|e| e.target == index)
            .map(|e| arrival[e.source])
            .max()
            .unwrap_or(0);
        if node.key == NodeKey::Main {
            input = input.max(context.previous_main_floor);
            if let Some(ceiling) = context.requested_ceiling {
                if ceiling < input {
                    return Err("tail or path exceeds ceiling");
                }
                input = ceiling;
            }
        }
        arrival[index] = input.checked_add(own).ok_or("latency overflow")?;
        visited[index] = true;
        order.push(node.key);
        for edge in graph.edges.iter().filter(|e| e.source == index) {
            incoming[edge.target] -= 1;
        }
    }
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    nodes
        .try_reserve_exact(limits.node_capacity)
        .map_err(|_| "layout allocation refused")?;
    edges
        .try_reserve_exact(limits.edge_capacity)
        .map_err(|_| "layout allocation refused")?;
    for (index, node) in graph.nodes.iter().enumerate() {
        nodes.push(NodeLayout {
            key: node.key,
            arrival: arrival[index],
        });
    }
    let mut ring_frames = 0;
    let mut rings = 0;
    for edge in &graph.edges {
        let own = match graph.nodes[edge.target].fixed {
            Fixed::Known(n) => n,
            Fixed::Unknown => return Err("unknown latency"),
        };
        let delay = arrival[edge.target]
            .checked_sub(own)
            .and_then(|n| n.checked_sub(arrival[edge.source]))
            .ok_or("negative edge delay")?;
        if delay > limits.delay {
            return Err("delay capacity refused");
        }
        if context.continuous
            && delay > 0
            && context.histories.get(&edge.key).copied().unwrap_or(0) < delay
        {
            return Err("history unavailable");
        }
        let raw = usize::try_from(delay).map_err(|_| "ring frame overflow")?;
        let capacity = if raw == 0 {
            0
        } else {
            raw.checked_add(limits.ring_quantum - 1)
                .and_then(|n| (n / limits.ring_quantum).checked_mul(limits.ring_quantum))
                .ok_or("ring capacity overflow")?
        };
        ring_frames = add(ring_frames, capacity)?;
        if capacity > 0 {
            rings = add(rings, 1)?;
        }
        edges.push(EdgeLayout {
            key: edge.key,
            delay,
            ring_capacity: capacity,
        });
    }
    // Proposed rings are two Vec<stereo Frame> banks plus two (cursor,valid,epoch)
    // tuples. No rings are allocated/activated by this proof. Report actual
    // layout Vec capacities and measured Rust representation sizes, not CPU caps.
    let ring_twin_bytes = mul(mul(ring_frames, size_of::<Frame>())?, 2)?;
    let ring_metadata_bytes = mul(
        rings,
        add(
            size_of::<[Vec<Frame>; 2]>(),
            size_of::<[(usize, usize, u64); 2]>(),
        )?,
    )?;
    let layout_bytes = add(
        add(
            add(
                size_of::<Layout>(),
                mul(nodes.capacity(), size_of::<NodeLayout>())?,
            )?,
            mul(edges.capacity(), size_of::<EdgeLayout>())?,
        )?,
        mul(order.capacity(), size_of::<NodeKey>())?,
    )?;
    let candidate_bytes = add(add(ring_twin_bytes, ring_metadata_bytes)?, layout_bytes)?;
    let coexist_bytes = add(
        add(candidate_bytes, context.active_bytes)?,
        context.retained_bytes,
    )?;
    if coexist_bytes > limits.coexist_bytes {
        return Err("coexistence capacity refused");
    }
    let main_latency = nodes
        .iter()
        .find(|n| n.key == NodeKey::Main)
        .ok_or("main missing")?
        .arrival;
    Ok(Layout {
        nodes,
        edges,
        order,
        main_latency,
        demand: Demand {
            stereo_frame_bytes: size_of::<Frame>(),
            ring_frames_per_bank: ring_frames,
            ring_twin_bytes,
            ring_metadata_bytes,
            layout_bytes,
            candidate_bytes,
            active_bytes: context.active_bytes,
            retained_bytes: context.retained_bytes,
            coexist_bytes,
        },
    })
}
