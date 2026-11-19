use super::*;

/// A stable graph owner and its actual prepared algorithmic latency.
#[derive(Clone, Copy, Debug)]
pub struct LatencyNode {
    pub key: u64,
    pub latency_frames: Option<u32>,
}

#[derive(Clone, Copy, Debug)]
pub struct LatencyEdge {
    pub key: u64,
    pub from: u64,
    pub to: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlannedNode {
    pub key: u64,
    pub latency_frames: u32,
    pub input_arrival_frames: u32,
    pub output_arrival_frames: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlannedEdge {
    pub key: u64,
    pub from_index: usize,
    pub to_index: usize,
    pub delay_frames: u32,
}

/// Immutable control-side result, sorted by stable keys independently of input
/// enumeration. Cycles (including self-feedback) require a separate causal policy.
#[derive(Debug, PartialEq, Eq)]
pub struct LatencyPlan {
    nodes: Vec<PlannedNode>,
    edges: Vec<PlannedEdge>,
    topological_order: Vec<usize>,
    max_latency_frames: u32,
}

impl LatencyPlan {
    /// Fallible control-side copy for an independently prepared runtime owner.
    /// Exact equality includes stable identities, topology and actual latencies.
    pub fn try_clone(&self) -> Result<Self, CompensationError> {
        let mut nodes = reserved(self.nodes.len())?;
        nodes.extend_from_slice(&self.nodes);
        let mut edges = reserved(self.edges.len())?;
        edges.extend_from_slice(&self.edges);
        let mut topological_order = reserved(self.topological_order.len())?;
        topological_order.extend_from_slice(&self.topological_order);
        Ok(Self {
            nodes,
            edges,
            topological_order,
            max_latency_frames: self.max_latency_frames,
        })
    }

    pub fn build(
        nodes: &[LatencyNode],
        edges: &[LatencyEdge],
        limits: CompensationLimits,
    ) -> Result<Self, CompensationError> {
        limits.validate()?;
        if nodes.len() > MAX_COMPENSATION_NODES || edges.len() > MAX_COMPENSATION_EDGES {
            return Err(CompensationError::GraphLimit);
        }
        let mut planned_nodes = reserved(nodes.len())?;
        for node in nodes {
            let latency_frames = node
                .latency_frames
                .ok_or(CompensationError::UnknownLatency)?;
            if latency_frames > limits.max_cumulative_frames {
                return Err(CompensationError::LatencyLimit);
            }
            planned_nodes.push(PlannedNode {
                key: node.key,
                latency_frames,
                input_arrival_frames: 0,
                output_arrival_frames: 0,
            });
        }
        planned_nodes.sort_unstable_by_key(|node| node.key);
        if planned_nodes
            .windows(2)
            .any(|pair| pair[0].key == pair[1].key)
        {
            return Err(CompensationError::DuplicateNode);
        }
        let mut planned_edges = reserved(edges.len())?;
        let mut indegree = reserved(nodes.len())?;
        indegree.resize(nodes.len(), 0_usize);
        for edge in edges {
            let from_index = planned_nodes
                .binary_search_by_key(&edge.from, |node| node.key)
                .map_err(|_| CompensationError::MissingNode)?;
            let to_index = planned_nodes
                .binary_search_by_key(&edge.to, |node| node.key)
                .map_err(|_| CompensationError::MissingNode)?;
            indegree[to_index] = indegree[to_index]
                .checked_add(1)
                .ok_or(CompensationError::ArithmeticOverflow)?;
            planned_edges.push(PlannedEdge {
                key: edge.key,
                from_index,
                to_index,
                delay_frames: 0,
            });
        }
        planned_edges.sort_unstable_by_key(|edge| edge.key);
        if planned_edges
            .windows(2)
            .any(|pair| pair[0].key == pair[1].key)
        {
            return Err(CompensationError::DuplicateEdge);
        }
        let mut order = reserved(nodes.len())?;
        while order.len() < planned_nodes.len() {
            // usize::MAX marks an already visited node. Stable-key scanning makes
            // ties deterministic without another heap or callback-side schedule.
            let next = indegree
                .iter()
                .position(|&count| count == 0)
                .ok_or(CompensationError::Cycle)?;
            indegree[next] = usize::MAX;
            let input_arrival_frames = planned_nodes[next].input_arrival_frames;
            let output_arrival_frames = input_arrival_frames
                .checked_add(planned_nodes[next].latency_frames)
                .ok_or(CompensationError::ArithmeticOverflow)?;
            if output_arrival_frames > limits.max_cumulative_frames {
                return Err(CompensationError::LatencyLimit);
            }
            planned_nodes[next].output_arrival_frames = output_arrival_frames;
            order.push(next);
            for edge in &planned_edges {
                if edge.from_index == next {
                    let node = &mut planned_nodes[edge.to_index];
                    node.input_arrival_frames =
                        node.input_arrival_frames.max(output_arrival_frames);
                    indegree[edge.to_index] = indegree[edge.to_index]
                        .checked_sub(1)
                        .ok_or(CompensationError::Cycle)?;
                }
            }
        }
        for edge in &mut planned_edges {
            edge.delay_frames = planned_nodes[edge.to_index]
                .input_arrival_frames
                .checked_sub(planned_nodes[edge.from_index].output_arrival_frames)
                .ok_or(CompensationError::ArithmeticOverflow)?;
        }
        let max_latency_frames = planned_nodes
            .iter()
            .map(|node| node.output_arrival_frames)
            .max()
            .unwrap_or(0);
        let plan = Self {
            nodes: planned_nodes,
            edges: planned_edges,
            topological_order: order,
            max_latency_frames,
        };
        if plan.storage_bytes()? > limits.max_storage_bytes {
            return Err(CompensationError::StorageLimit);
        }
        Ok(plan)
    }

    pub fn nodes(&self) -> &[PlannedNode] {
        &self.nodes
    }
    pub fn edges(&self) -> &[PlannedEdge] {
        &self.edges
    }
    pub fn topological_order(&self) -> &[usize] {
        &self.topological_order
    }
    pub fn node_index(&self, key: u64) -> Option<usize> {
        self.nodes.binary_search_by_key(&key, |node| node.key).ok()
    }
    pub fn edge_index(&self, key: u64) -> Option<usize> {
        self.edges.binary_search_by_key(&key, |edge| edge.key).ok()
    }
    pub const fn max_latency_frames(&self) -> u32 {
        self.max_latency_frames
    }
    pub fn storage_bytes(&self) -> Result<usize, CompensationError> {
        let Self {
            nodes,
            edges,
            topological_order,
            max_latency_frames: _,
        } = self;
        add_bytes(
            add_bytes(vector_bytes(nodes)?, vector_bytes(edges)?)?,
            vector_bytes(topological_order)?,
        )
    }
}
