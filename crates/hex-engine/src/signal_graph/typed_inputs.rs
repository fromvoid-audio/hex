//! Stable scalar topology keys without tree nodes or hidden sort scratch.
use super::*;

pub(super) struct TypedInputs {
    pub(super) typed: Vec<NodeId>,
    pub(super) nodes: Vec<LatencyNode>,
    pub(super) edges: Vec<LatencyEdge>,
    pub(super) edge_keys: Vec<u64>,
}

pub(super) fn reserved<T>(count: usize) -> Result<Vec<T>, Invalid> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(count)
        .map_err(|_| Invalid("could not reserve signal graph scalar storage"))?;
    Ok(result)
}

/// Actual fixed node/edge cardinalities, before any builder backing is allocated.
/// Four track nodes/edges plus an enabled generator, every effect, each Return's
/// two nodes/edges, one Main node, and actual authored send/feed edges.
pub(super) fn counts(
    project: &Project,
    return_feeds: usize,
    audio_feeds: usize,
) -> Result<(usize, usize), Invalid> {
    let mut nodes = 1usize;
    let mut edges = return_feeds
        .checked_add(audio_feeds)
        .ok_or(Invalid("signal graph edge count overflow"))?;
    for track in &project.tracks {
        let count = 4usize
            .checked_add(usize::from(track.instrument_slot_enabled()))
            .and_then(|n| n.checked_add(track.device_chain.len()))
            .ok_or(Invalid("signal graph node count overflow"))?;
        nodes = nodes
            .checked_add(count)
            .ok_or(Invalid("signal graph node count overflow"))?;
        edges = edges
            .checked_add(count)
            .and_then(|n| n.checked_add(track.sends.len()))
            .ok_or(Invalid("signal graph edge count overflow"))?;
    }
    for bus in &project.return_buses {
        let count = 2usize
            .checked_add(bus.device_chain.len())
            .ok_or(Invalid("signal graph node count overflow"))?;
        nodes = nodes
            .checked_add(count)
            .ok_or(Invalid("signal graph node count overflow"))?;
        edges = edges
            .checked_add(count)
            .ok_or(Invalid("signal graph edge count overflow"))?;
    }
    nodes = nodes
        .checked_add(project.main_device_chain.len())
        .ok_or(Invalid("signal graph node count overflow"))?;
    edges = edges
        .checked_add(project.main_device_chain.len())
        .ok_or(Invalid("signal graph edge count overflow"))?;
    if nodes > MAX_COMPENSATION_NODES || edges > MAX_COMPENSATION_EDGES {
        return Err(error(CompensationError::GraphLimit));
    }
    Ok((nodes, edges))
}

pub(super) fn prepare(builder: &Builder) -> Result<TypedInputs, Invalid> {
    let mut typed = reserved(builder.nodes.len())?;
    typed.extend(builder.nodes.iter().map(|(id, _)| *id));
    typed.sort_unstable();
    if typed.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(Invalid("signal graph has duplicate typed nodes"));
    }
    let mut nodes = reserved(builder.nodes.len())?;
    for (id, latency_frames) in &builder.nodes {
        let index = typed
            .binary_search(id)
            .map_err(|_| Invalid("signal graph node mapping is missing"))?;
        let key = u64::try_from(index)
            .ok()
            .and_then(|n| n.checked_add(1))
            .ok_or(Invalid("signal graph node key overflow"))?;
        nodes.push(LatencyNode {
            key,
            latency_frames: *latency_frames,
        });
    }
    let mut edges = reserved(builder.edges.len())?;
    for (index, &(from, to)) in builder.edges.iter().enumerate() {
        let from = nodes
            .get(from)
            .ok_or(Invalid("signal graph edge source is missing"))?
            .key;
        let to = nodes
            .get(to)
            .ok_or(Invalid("signal graph edge destination is missing"))?
            .key;
        // This temporary key is the old edge ordinal. It breaks equal-endpoint
        // ties exactly like the previous stable sort; it is replaced below.
        let key = u64::try_from(index).map_err(|_| Invalid("signal graph edge key overflow"))?;
        edges.push(LatencyEdge { key, from, to });
    }
    edges.sort_unstable_by_key(|edge| (edge.from, edge.to, edge.key));
    let mut edge_keys = reserved(builder.edges.len())?;
    edge_keys.resize(builder.edges.len(), 0_u64);
    for (index, edge) in edges.iter_mut().enumerate() {
        let original =
            usize::try_from(edge.key).map_err(|_| Invalid("signal graph edge key overflow"))?;
        let key = u64::try_from(index)
            .ok()
            .and_then(|n| n.checked_add(1))
            .ok_or(Invalid("signal graph edge key overflow"))?;
        *edge_keys
            .get_mut(original)
            .ok_or(Invalid("signal graph edge mapping is missing"))? = key;
        edge.key = key;
    }
    Ok(TypedInputs {
        typed,
        nodes,
        edges,
        edge_keys,
    })
}

#[cfg(test)]
mod tests;
