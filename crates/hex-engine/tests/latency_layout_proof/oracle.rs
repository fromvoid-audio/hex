//! Independent path enumeration and actual test-owned storage; never runtime DSP.
use super::model::{Context, EdgeKey, Fixed, Graph, Layout, Limits, NodeKey};
use std::{collections::BTreeMap, mem::size_of};
type Frame = [f32; 2];
type Schedule = (BTreeMap<NodeKey, u64>, BTreeMap<EdgeKey, u64>);

pub(super) fn paths(graph: &Graph) -> Result<Schedule, &'static str> {
    fn visit(
        graph: &Graph,
        at: usize,
        path: &mut Vec<usize>,
        values: &mut Vec<u64>,
        sum: u64,
    ) -> Result<(), &'static str> {
        if at >= graph.nodes.len() {
            return Err("endpoint missing");
        }
        if path.contains(&at) {
            return Err("cycle");
        }
        let own = match graph.nodes[at].fixed {
            Fixed::Known(n) => n,
            Fixed::Unknown => return Err("unknown"),
        };
        let sum = sum.checked_add(own).ok_or("latency overflow")?;
        values[at] = values[at].max(sum);
        path.push(at);
        for edge in graph.edges.iter().filter(|e| e.source == at) {
            visit(graph, edge.target, path, values, sum)?;
        }
        path.pop();
        Ok(())
    }
    let mut arrivals = vec![0; graph.nodes.len()];
    let mut path = Vec::new();
    // Enumerate paths from every node: nonnegative declarations make the
    // longest complete source path dominate its suffixes. No Kahn traversal,
    // planner-arrival table or planner checked-arithmetic helper is reused.
    for node in 0..graph.nodes.len() {
        visit(graph, node, &mut path, &mut arrivals, 0)?;
    }
    let mut keyed = BTreeMap::new();
    for (node, value) in graph.nodes.iter().zip(&arrivals) {
        if keyed.insert(node.key, *value).is_some() {
            return Err("duplicate node");
        }
    }
    let mut edges = BTreeMap::new();
    for edge in &graph.edges {
        let source = *arrivals.get(edge.source).ok_or("endpoint missing")?;
        let target = *arrivals.get(edge.target).ok_or("endpoint missing")?;
        let own = match graph.nodes[edge.target].fixed {
            Fixed::Known(n) => n,
            Fixed::Unknown => return Err("unknown"),
        };
        let delay = target
            .checked_sub(own)
            .and_then(|n| n.checked_sub(source))
            .ok_or("negative delay")?;
        if edges.insert(edge.key, delay).is_some() {
            return Err("duplicate edge");
        }
    }
    Ok((keyed, edges))
}

#[derive(Default)]
pub(super) struct State {
    pub cursor: usize,
    pub valid: usize,
    pub epoch: u64,
}
pub(super) struct RingSlot {
    pub edge_index: usize,
    pub banks: [Vec<Frame>; 2],
    pub state: [State; 2],
}
pub(super) struct Witness {
    pub slots: Vec<RingSlot>,
}
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Charge {
    pub container: usize,
    pub descriptors: usize,
    pub samples: usize,
    pub candidate: usize,
    pub coexist: usize,
}
fn checked_capacity<T>(n: usize) -> Result<usize, &'static str> {
    n.checked_mul(size_of::<T>())
        .filter(|b| *b <= isize::MAX as usize)
        .ok_or("capacity overflow")
}
fn total(
    layout: &Layout,
    descriptors: usize,
    samples: usize,
    context: &Context,
) -> Result<Charge, &'static str> {
    let container = size_of::<Witness>();
    // Replace the old conceptual ring metadata with the actual owning
    // container/descriptor charges, retaining the unchanged schedule charge.
    let candidate = layout
        .demand
        .layout_bytes
        .checked_add(container)
        .and_then(|n| n.checked_add(descriptors))
        .and_then(|n| n.checked_add(samples))
        .ok_or("byte overflow")?;
    let coexist = candidate
        .checked_add(context.active_bytes)
        .and_then(|n| n.checked_add(context.retained_bytes))
        .ok_or("byte overflow")?;
    Ok(Charge {
        container,
        descriptors,
        samples,
        candidate,
        coexist,
    })
}

pub(super) fn allocate(
    layout: &Layout,
    slot_capacity: usize,
    banks: &[[usize; 2]],
    context: &Context,
    limits: Limits,
) -> Result<(Witness, Charge), &'static str> {
    let edges: Vec<_> = layout
        .edges
        .iter()
        .enumerate()
        .filter(|(_, e)| e.ring_capacity > 0)
        .collect();
    if banks.len() != edges.len() || slot_capacity < edges.len() {
        return Err("descriptor capacity refused");
    }
    let descriptor_bytes = checked_capacity::<RingSlot>(slot_capacity)?;
    let mut sample_bytes = 0usize;
    for ((_, edge), capacities) in edges.iter().zip(banks) {
        for &capacity in capacities {
            if capacity < edge.ring_capacity {
                return Err("sample capacity refused");
            }
            sample_bytes = sample_bytes
                .checked_add(checked_capacity::<Frame>(capacity)?)
                .ok_or("byte overflow")?;
        }
    }
    if total(layout, descriptor_bytes, sample_bytes, context)?.coexist > limits.coexist_bytes {
        return Err("coexistence refused");
    }
    let mut slots = Vec::new();
    slots
        .try_reserve_exact(slot_capacity)
        .map_err(|_| "allocation refused")?;
    for ((index, edge), capacities) in edges.iter().zip(banks) {
        let mut pair = [Vec::new(), Vec::new()];
        for (bank, &capacity) in pair.iter_mut().zip(capacities) {
            bank.try_reserve_exact(capacity)
                .map_err(|_| "allocation refused")?;
            bank.resize(edge.ring_capacity, [0.0; 2]);
        }
        slots.push(RingSlot {
            edge_index: *index,
            banks: pair,
            state: [State::default(), State::default()],
        });
    }
    let samples = slots
        .iter()
        .flat_map(|s| &s.banks)
        .try_fold(0usize, |sum, bank| {
            sum.checked_add(checked_capacity::<Frame>(bank.capacity())?)
                .ok_or("byte overflow")
        })?;
    let charge = total(
        layout,
        checked_capacity::<RingSlot>(slots.capacity())?,
        samples,
        context,
    )?;
    if charge.coexist > limits.coexist_bytes {
        return Err("coexistence refused");
    }
    Ok((Witness { slots }, charge))
}
