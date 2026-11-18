//! Control-prepared DAG latency plans and allocation-free transactional delays.
//!
//! Device latency is supplied by the actual prepared processors. Compensation
//! aligns shorter incoming paths; it cannot remove physical or causal latency.
//! Construction and destruction belong on control. Rendering only borrows these
//! fixed buffers, and commit/rollback copy the circular intervals actually touched.

use crate::Frame;
use std::{fmt, mem::size_of};

mod block_origins;
pub(crate) use block_origins::OriginWindow;
mod delay;
mod plan;
pub use delay::DelayPair;
pub use plan::{LatencyEdge, LatencyNode, LatencyPlan, PlannedEdge, PlannedNode};

pub const MAX_COMPENSATION_FRAMES: u32 = 65_536;
pub const MAX_COMPENSATION_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_COMPENSATION_NODES: usize = 4096;
pub const MAX_COMPENSATION_EDGES: usize = 16_384;

/// One graph's policy, including both committed and speculative storage.
/// The owner must separately charge coexistence of active/candidate/retained graphs.
#[derive(Clone, Copy, Debug)]
pub struct CompensationLimits {
    pub max_cumulative_frames: u32,
    pub max_storage_bytes: usize,
}

impl Default for CompensationLimits {
    fn default() -> Self {
        Self {
            max_cumulative_frames: MAX_COMPENSATION_FRAMES,
            max_storage_bytes: MAX_COMPENSATION_BYTES,
        }
    }
}

impl CompensationLimits {
    fn validate(self) -> Result<(), CompensationError> {
        if self.max_cumulative_frames > MAX_COMPENSATION_FRAMES
            || self.max_storage_bytes > MAX_COMPENSATION_BYTES
        {
            return Err(CompensationError::PolicyLimit);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompensationError {
    PolicyLimit,
    GraphLimit,
    DuplicateNode,
    DuplicateEdge,
    MissingNode,
    UnknownLatency,
    Cycle,
    ArithmeticOverflow,
    LatencyLimit,
    StorageLimit,
    Allocation,
    InvalidIndex,
    BufferLength,
    TransactionActive,
    TransactionInactive,
}

impl fmt::Display for CompensationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::PolicyLimit => "compensation policy exceeds the engine limit",
            Self::GraphLimit => "compensation graph exceeds the engine limit",
            Self::DuplicateNode => "compensation graph has a duplicate node",
            Self::DuplicateEdge => "compensation graph has a duplicate edge",
            Self::MissingNode => "compensation edge references a missing node",
            Self::UnknownLatency => "prepared processor latency is unknown",
            Self::Cycle => "compensation graph contains a cycle",
            Self::ArithmeticOverflow => "compensation arithmetic overflow",
            Self::LatencyLimit => "cumulative latency exceeds the compensation limit",
            Self::StorageLimit => "compensation storage exceeds its budget",
            Self::Allocation => "could not reserve compensation storage",
            Self::InvalidIndex => "compensation index is outside its prepared table",
            Self::BufferLength => "compensation input and output lengths differ",
            Self::TransactionActive => "compensation transaction is already active",
            Self::TransactionInactive => "compensation transaction is not active",
        })
    }
}
impl std::error::Error for CompensationError {}

fn reserved<T>(count: usize) -> Result<Vec<T>, CompensationError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| CompensationError::Allocation)?;
    Ok(values)
}

fn vector_bytes<T>(values: &Vec<T>) -> Result<usize, CompensationError> {
    values
        .capacity()
        .checked_mul(size_of::<T>())
        .ok_or(CompensationError::ArithmeticOverflow)
}

fn add_bytes(a: usize, b: usize) -> Result<usize, CompensationError> {
    a.checked_add(b)
        .ok_or(CompensationError::ArithmeticOverflow)
}

fn requested_ring_bytes<T>(capacity: usize) -> Result<usize, CompensationError> {
    capacity
        .checked_mul(size_of::<T>())
        .and_then(|bytes| bytes.checked_mul(2))
        .ok_or(CompensationError::ArithmeticOverflow)
}

/// Original timeline identity carried through latency, including across loops.
/// Beat preserves exact Q32.32 quarter-note ticks; no project or runtime owner is retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportStamp {
    /// Local source tempo; stays aligned with the same delayed content stamp.
    pub tempo_micro_bpm: Option<u32>,
    /// Exact source performance-frame identity, independent of loop position.
    pub performance_frame: u64,
    /// Actual host transport state when this source frame entered the graph.
    pub transport_is_running: bool,
    pub arrangement_frame: u64,
    pub loop_pass: u64,
    pub performance_beat_raw: u64,
    /// Loop-normalized Arrangement beat when representable in Q32.32.
    /// An unavailable beat does not invalidate the exact frame identity.
    pub arrangement_beat_raw: Option<u64>,
    /// Meter and active cycle at the same source origin as the audio.
    pub meter: Option<hex_project::Meter>,
    pub cycle_beats_raw: Option<(u64, u64)>,
}

/// One fixed prepared graph. Every edge uses the plan's stable-key sorted index.
/// A single transport history is sufficient because every branch shares the same
/// source clock. Append a stamp before rendering that frame, then query input or
/// output origins. `None` explicitly represents history before the current epoch.
#[derive(Debug)]
pub struct CompensationRuntime {
    plan: LatencyPlan,
    edges: Vec<DelayPair>,
    stamps: delay::RingPair<Option<TransportStamp>>,
    storage_bytes: usize,
    transaction_active: bool,
}

impl CompensationRuntime {
    pub(crate) fn visit_owned_candidate_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        use crate::audio_storage::{Location, Owner};
        let Self {
            plan,
            edges,
            stamps,
            storage_bytes: _,
            transaction_active,
        } = self;
        if *transaction_active {
            return Err(crate::AudioStorageError::SourceMismatch);
        }
        let count = || -> Result<usize, CompensationError> {
            let mut bytes = add_bytes(plan.storage_bytes()?, vector_bytes(edges)?)?;
            bytes = add_bytes(bytes, stamps.storage_bytes()?)?;
            for edge in edges {
                bytes = add_bytes(bytes, edge.owned_data_bytes()?)?;
            }
            Ok(bytes)
        };
        let bytes = count().map_err(|_| crate::AudioStorageError::Overflow)?;
        // All DATA belongs to this private constructor: immutable plan Vecs,
        // edge rows, each concrete ring pair and the one transport stamp pair.
        // No owning handle or borrowed device/provider is present.
        ledger.bytes(
            Owner::exclusive(self),
            Location("compensation-owned-data", 0),
            bytes,
        )
    }
    /// Fallible control-only construction. Budgets charge actual Vec capacities,
    /// including the edge table and immutable plan, rather than requested lengths.
    pub fn new(plan: LatencyPlan, limits: CompensationLimits) -> Result<Self, CompensationError> {
        limits.validate()?;
        if plan.max_latency_frames() > limits.max_cumulative_frames {
            return Err(CompensationError::LatencyLimit);
        }
        let mut edges = reserved(plan.edges().len())?;
        let mut storage_bytes = add_bytes(plan.storage_bytes()?, vector_bytes(&edges)?)?;
        let stamp_capacity = usize::try_from(plan.max_latency_frames())
            .map_err(|_| CompensationError::ArithmeticOverflow)?
            .checked_add(1)
            .ok_or(CompensationError::ArithmeticOverflow)?;
        let mut requested_bytes = add_bytes(
            storage_bytes,
            requested_ring_bytes::<Option<TransportStamp>>(stamp_capacity)?,
        )?;
        for edge in plan.edges() {
            requested_bytes = add_bytes(
                requested_bytes,
                requested_ring_bytes::<Frame>(edge.delay_frames as usize)?,
            )?;
        }
        if requested_bytes > limits.max_storage_bytes {
            return Err(CompensationError::StorageLimit);
        }
        let stamps = delay::RingPair::new(stamp_capacity)?;
        storage_bytes = add_bytes(storage_bytes, stamps.storage_bytes()?)?;
        if storage_bytes > limits.max_storage_bytes {
            return Err(CompensationError::StorageLimit);
        }
        for edge in plan.edges() {
            let pair = DelayPair::new(
                edge.delay_frames,
                CompensationLimits {
                    max_storage_bytes: limits.max_storage_bytes - storage_bytes,
                    ..limits
                },
            )?;
            storage_bytes = add_bytes(storage_bytes, pair.storage_bytes())?;
            if storage_bytes > limits.max_storage_bytes {
                return Err(CompensationError::StorageLimit);
            }
            edges.push(pair);
        }
        Ok(Self {
            plan,
            edges,
            stamps,
            storage_bytes,
            transaction_active: false,
        })
    }

    pub fn plan(&self) -> &LatencyPlan {
        &self.plan
    }

    pub const fn storage_bytes(&self) -> usize {
        self.storage_bytes
    }

    pub const fn transaction_active(&self) -> bool {
        self.transaction_active
    }

    pub fn begin_transaction(&mut self) -> Result<(), CompensationError> {
        if self.transaction_active {
            return Err(CompensationError::TransactionActive);
        }
        for edge in &mut self.edges {
            edge.begin_transaction()?;
        }
        self.stamps.begin();
        self.transaction_active = true;
        Ok(())
    }

    pub fn process_edge(
        &mut self,
        edge_index: usize,
        input: &[Frame],
        output: &mut [Frame],
    ) -> Result<(), CompensationError> {
        self.edges
            .get_mut(edge_index)
            .ok_or(CompensationError::InvalidIndex)?
            .process(input, output)
    }

    pub fn process_edge_in_place(
        &mut self,
        edge_index: usize,
        frames: &mut [Frame],
    ) -> Result<(), CompensationError> {
        self.edges
            .get_mut(edge_index)
            .ok_or(CompensationError::InvalidIndex)?
            .process_in_place(frames)
    }

    /// Silence is ordinary input and still advances pending ring samples.
    pub fn process_edge_silence(
        &mut self,
        edge_index: usize,
        output: &mut [Frame],
    ) -> Result<(), CompensationError> {
        output.fill([0.0; 2]);
        self.process_edge_in_place(edge_index, output)
    }

    pub fn push_transport_stamp(&mut self, stamp: TransportStamp) -> Result<(), CompensationError> {
        if !self.transaction_active {
            return Err(CompensationError::TransactionInactive);
        }
        self.stamps.push(Some(stamp));
        Ok(())
    }

    pub fn stamp_at_delay(&self, delay_frames: u32) -> Option<TransportStamp> {
        self.stamps.at_delay(delay_frames as usize).flatten()
    }

    /// Preview an upcoming stage's Arrangement frame before its stamp is pushed.
    /// The supplied incoming projection belongs to the same planned clock as
    /// the caller's eventual render. Historical stamps are read verbatim from
    /// this owner's current transactional history; no clock is rebased here.
    pub(crate) fn preview_stage_frame<E>(
        history: Option<&Self>,
        delay_frames: u32,
        upcoming_offset: u64,
        incoming: impl FnOnce(u64) -> Result<Option<u64>, E>,
    ) -> Result<Option<u64>, E> {
        let delay = u64::from(delay_frames);
        if upcoming_offset >= delay {
            incoming(upcoming_offset - delay)
        } else {
            Ok(history
                .and_then(|history| history.stamp_at_delay((delay - upcoming_offset - 1) as u32))
                .map(|stamp| stamp.arrangement_frame))
        }
    }

    pub fn node_input_stamp(
        &self,
        node_index: usize,
    ) -> Result<Option<TransportStamp>, CompensationError> {
        let node = self
            .plan
            .nodes()
            .get(node_index)
            .ok_or(CompensationError::InvalidIndex)?;
        Ok(self.stamp_at_delay(node.input_arrival_frames))
    }

    pub fn node_output_stamp(
        &self,
        node_index: usize,
    ) -> Result<Option<TransportStamp>, CompensationError> {
        let node = self
            .plan
            .nodes()
            .get(node_index)
            .ok_or(CompensationError::InvalidIndex)?;
        Ok(self.stamp_at_delay(node.output_arrival_frames))
    }

    /// True once timeline history covers every output's cumulative latency.
    pub fn history_ready(&self) -> bool {
        self.stamp_at_delay(self.plan.max_latency_frames())
            .is_some()
    }

    pub fn commit_transaction(&mut self) -> Result<(), CompensationError> {
        if !self.transaction_active {
            return Err(CompensationError::TransactionInactive);
        }
        for edge in &mut self.edges {
            edge.commit_transaction()?;
        }
        self.stamps.commit();
        self.transaction_active = false;
        Ok(())
    }

    pub fn rollback_transaction(&mut self) -> Result<(), CompensationError> {
        if !self.transaction_active {
            return Err(CompensationError::TransactionInactive);
        }
        for edge in &mut self.edges {
            edge.rollback_transaction()?;
        }
        self.stamps.rollback();
        self.transaction_active = false;
        Ok(())
    }

    pub(crate) fn latest_stamp(&self) -> Option<TransportStamp> {
        self.stamp_at_delay(0)
    }
    pub(crate) fn is_closed(&self) -> bool {
        !self.transaction_active
    }
    pub(crate) fn history_horizon(&self) -> u32 {
        self.plan.max_latency_frames()
    }
    pub(crate) fn edge_history_is_silent(&self, index: usize) -> bool {
        self.is_closed()
            && self
                .edges
                .get(index)
                .is_some_and(DelayPair::history_is_silent)
    }
    /// True when no committed routing edge holds PCM. Transport stamps do not
    /// affect signal quiescence. This visits only the bounded edge table.
    pub(crate) fn pcm_history_is_silent(&self) -> bool {
        self.is_closed() && self.edges.iter().all(DelayPair::history_is_silent)
    }
    pub(crate) fn can_copy_edge_history_from(
        &self,
        target: usize,
        source: &Self,
        origin: usize,
    ) -> bool {
        self.is_closed()
            && source.is_closed()
            && self
                .edges
                .get(target)
                .zip(source.edges.get(origin))
                .is_some_and(|(target, origin)| target.can_copy_history_from(origin))
    }
    pub(crate) fn copy_edge_history_from(
        &mut self,
        target: usize,
        source: &Self,
        origin: usize,
    ) -> Result<(), CompensationError> {
        if !self.can_copy_edge_history_from(target, source, origin) {
            return Err(CompensationError::InvalidIndex);
        }
        self.edges[target].copy_history_from(&source.edges[origin]);
        Ok(())
    }
    pub(crate) fn copy_stamp_history_from(
        &mut self,
        source: &Self,
    ) -> Result<(), CompensationError> {
        if !self.is_closed()
            || !source.is_closed()
            || self.stamps.capacity() < source.stamps.capacity()
        {
            return Err(CompensationError::StorageLimit);
        }
        self.stamps.copy_history_from(&source.stamps);
        Ok(())
    }
    /// Discard both speculative and committed history at a transport epoch change.
    /// This bounded clear must be coordinated with the owner's DSP/clock reset.
    pub fn reset_epoch(&mut self) {
        for edge in &mut self.edges {
            edge.reset_epoch();
        }
        self.stamps.reset();
        self.transaction_active = false;
    }
}

#[cfg(test)]
mod tests;
