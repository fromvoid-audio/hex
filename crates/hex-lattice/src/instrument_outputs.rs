//! Bounded typed native instrument multi-output port descriptors, routing, PDC,
//! atomic history, and zero-allocation callback router.
//!
//! Single-output native instruments continue to expose only `MAIN_STEREO` (never
//! decorative ports) and retain the existing shared-source receiver path.
//! Multi-output native instruments (up to `MAX_NATIVE_OUTPUT_PORTS == 2` stereo
//! outputs, e.g., Drum Rack main + aux buses) declare explicit port descriptors
//! and route each port to validated `Track`, `Return`, or `Unconnected`
//! destinations with bounded fanout, cycle refusal, exact PDC compensation,
//! atomic Undo/Redo history, strict binary snapshot encoding, and zero callback
//! allocations.

#[cfg(test)]
mod tests;

use crate::{
    DeviceResources, MAX_MAIN_PASS_DELAY_FRAMES, NativeInstrumentClock, NativeInstrumentEvent,
    NativeInstrumentRuntimeError, StereoFrame,
};
use std::{
    fmt,
    num::{NonZeroU32, NonZeroU64},
};

/// Maximum stereo output ports supported by one bounded native instrument.
pub const MAX_NATIVE_OUTPUT_PORTS: usize = 2;
/// Maximum simultaneous destinations one output port may fan out to.
pub const MAX_NATIVE_OUTPUT_FANOUT_PER_PORT: usize = 4;
/// Maximum total configured routes across all ports of one source.
pub const MAX_NATIVE_OUTPUT_ROUTES: usize =
    MAX_NATIVE_OUTPUT_PORTS * MAX_NATIVE_OUTPUT_FANOUT_PER_PORT;
/// Maximum registered graph destinations validated per routing plan.
pub const MAX_NATIVE_OUTPUT_DESTINATIONS: usize = 64;
/// Maximum graph edges traversed during cycle detection.
pub const MAX_NATIVE_OUTPUT_GRAPH_EDGES: usize = 256;
/// Maximum per-route PDC compensation delay in frames.
pub const MAX_NATIVE_OUTPUT_COMPENSATION_DELAY_FRAMES: u32 = MAX_MAIN_PASS_DELAY_FRAMES;
/// Maximum block size admitted by preallocated router scratch storage.
pub const MAX_NATIVE_OUTPUT_BLOCK_FRAMES: usize = 4_096;
/// Maximum linear route gain (`+12 dB`).
pub const MAX_NATIVE_OUTPUT_ROUTE_GAIN: f32 = 4.0;
/// Current strict binary persistence format version for multi-output plans.
pub const NATIVE_MULTI_OUTPUT_PLAN_VERSION: u32 = 1;
const PLAN_MAGIC: &[u8; 4] = b"HXMO";

/// Stable non-zero identifier for one native instrument output port.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NativeOutputPortId(NonZeroU32);

impl NativeOutputPortId {
    pub const MAIN_STEREO: Self = Self(NonZeroU32::MIN);
    pub const AUX_STEREO: Self = Self(NonZeroU32::MIN.saturating_add(1));

    pub const fn new(id: NonZeroU32) -> Self {
        Self(id)
    }

    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

/// Explicit channel layout for a native instrument output port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeOutputChannelLayout {
    Stereo,
}

/// Static descriptor for one native instrument output port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeOutputPortDescriptor {
    pub id: NativeOutputPortId,
    pub label: &'static str,
    pub layout: NativeOutputChannelLayout,
    pub algorithmic_latency_frames: u32,
}

/// Explicit routing destination for a native instrument output port.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NativeOutputDestination {
    Track(NonZeroU64),
    Return(NonZeroU64),
    Unconnected,
}

/// Validated graph destination metadata supplied on control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeOutputDestinationInfo {
    pub destination: NativeOutputDestination,
    pub latency_frames: u32,
}

/// Directed audio routing edge used for cycle detection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeGraphEdge {
    pub from: NativeOutputDestination,
    pub to: NativeOutputDestination,
}

/// Control-side graph snapshot against which a multi-output plan is validated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeMultiOutputTopology {
    /// Track hosting the multi-output source; routing back into this node
    /// directly or transitively is rejected as a cycle.
    pub source_host: NativeOutputDestination,
    pub destinations: Vec<NativeOutputDestinationInfo>,
    pub edges: Vec<NativeGraphEdge>,
}

/// One typed multi-output route from `(source_id, port_id)` to a destination.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeMultiOutputRoute {
    pub route_id: NonZeroU64,
    pub source_id: NonZeroU64,
    pub port_id: NativeOutputPortId,
    pub destination: NativeOutputDestination,
    pub gain: f32,
    pub enabled: bool,
}

/// Finite caller-supplied limits validated on control before preparing a router.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeMultiOutputLimits {
    pub max_pair_bytes: usize,
    pub max_fanout_per_port: usize,
    pub max_routes: usize,
    pub max_compensation_delay_frames: u32,
    pub max_block_frames: usize,
    pub max_work_units_per_block: usize,
}

impl Default for NativeMultiOutputLimits {
    fn default() -> Self {
        Self {
            max_pair_bytes: 2 * 1024 * 1024,
            max_fanout_per_port: MAX_NATIVE_OUTPUT_FANOUT_PER_PORT,
            max_routes: MAX_NATIVE_OUTPUT_ROUTES,
            max_compensation_delay_frames: MAX_NATIVE_OUTPUT_COMPENSATION_DELAY_FRAMES,
            max_block_frames: MAX_NATIVE_OUTPUT_BLOCK_FRAMES,
            max_work_units_per_block: MAX_NATIVE_OUTPUT_ROUTES * MAX_NATIVE_OUTPUT_BLOCK_FRAMES * 4,
        }
    }
}

/// Errors reported on invalid descriptors, stale/deleted/cyclic/saturated routes,
/// persistence corruption, or callback contract violations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultiOutputRoutingError {
    InvalidPortDescriptors,
    InvalidLimits,
    StaleSourceIdentity,
    UnknownPort,
    UnknownDestination,
    DuplicateRouteId,
    DuplicateRoute,
    CyclicRoute,
    FanoutBudgetExceeded,
    OutputBudgetExceeded,
    InvalidGain,
    CompensationDelayExceeded,
    RuntimeBudgetExceeded,
    WorkBudgetExceeded,
    AllocationFailed,
    BlockNotStarted,
    BlockTooLarge,
    PortBufferMismatch,
    MissingDestinationBuffer,
    RouteNotFound,
    MalformedPlanBytes,
    UnsupportedPlanVersion,
}

impl MultiOutputRoutingError {
    /// Allocation-free lowercase diagnostic message.
    pub const fn diagnostic_message(self) -> &'static str {
        match self {
            Self::InvalidPortDescriptors => "native output port descriptors are invalid",
            Self::InvalidLimits => "native multi-output routing limits are invalid",
            Self::StaleSourceIdentity => "route references a stale or mismatched source identity",
            Self::UnknownPort => "route references an unknown native output port",
            Self::UnknownDestination => "route references a missing or deleted destination",
            Self::DuplicateRouteId => "multi-output plan contains duplicate route identities",
            Self::DuplicateRoute => "multi-output plan contains duplicate port-destination routes",
            Self::CyclicRoute => "multi-output route would create an audio feedback cycle",
            Self::FanoutBudgetExceeded => "output port fanout exceeds configured limit",
            Self::OutputBudgetExceeded => "multi-output route count exceeds configured limit",
            Self::InvalidGain => "multi-output route gain must be finite and within range",
            Self::CompensationDelayExceeded => {
                "multi-output latency difference exceeds compensation delay limit"
            }
            Self::RuntimeBudgetExceeded => "multi-output router exceeds caller storage budget",
            Self::WorkBudgetExceeded => "multi-output router exceeds caller block work budget",
            Self::AllocationFailed => "multi-output router control allocation failed",
            Self::BlockNotStarted => "multi-output router transactional block was not started",
            Self::BlockTooLarge => {
                "multi-output block length exceeds preallocated scratch capacity"
            }
            Self::PortBufferMismatch => "multi-output port input slice count or length mismatches",
            Self::MissingDestinationBuffer => {
                "multi-output destination buffer is missing or mismatched"
            }
            Self::RouteNotFound => "multi-output route identity was not found",
            Self::MalformedPlanBytes => "multi-output routing plan bytes are malformed",
            Self::UnsupportedPlanVersion => "multi-output routing plan version is unsupported",
        }
    }
}

impl fmt::Display for MultiOutputRoutingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.diagnostic_message())
    }
}

impl std::error::Error for MultiOutputRoutingError {}

/// Validate native instrument output port descriptors (1..=2 sorted unique IDs,
/// lowercase labels, bounded latency, no decorative ports).
pub fn validate_output_port_descriptors(
    ports: &[NativeOutputPortDescriptor],
) -> Result<(), MultiOutputRoutingError> {
    if ports.is_empty() || ports.len() > MAX_NATIVE_OUTPUT_PORTS {
        return Err(MultiOutputRoutingError::InvalidPortDescriptors);
    }
    let mut prev_id = 0_u32;
    for port in ports {
        let raw = port.id.get();
        if raw <= prev_id
            || port.label.is_empty()
            || port.label.len() > 64
            || port.label.trim() != port.label
            || port
                .label
                .chars()
                .any(|c| c.is_control() || c.is_uppercase())
            || port.algorithmic_latency_frames > MAX_NATIVE_OUTPUT_COMPENSATION_DELAY_FRAMES
        {
            return Err(MultiOutputRoutingError::InvalidPortDescriptors);
        }
        prev_id = raw;
    }
    Ok(())
}

/// Complete control-side multi-output routing plan for one retained native source.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeMultiOutputPlan {
    pub source_id: NonZeroU64,
    pub routes: Vec<NativeMultiOutputRoute>,
}

impl NativeMultiOutputPlan {
    pub fn validate(
        &self,
        ports: &[NativeOutputPortDescriptor],
        topology: &NativeMultiOutputTopology,
        limits: &NativeMultiOutputLimits,
    ) -> Result<(), MultiOutputRoutingError> {
        validate_output_port_descriptors(ports)?;
        if limits.max_pair_bytes == 0
            || limits.max_fanout_per_port == 0
            || limits.max_fanout_per_port > MAX_NATIVE_OUTPUT_FANOUT_PER_PORT
            || limits.max_routes == 0
            || limits.max_routes > MAX_NATIVE_OUTPUT_ROUTES
            || limits.max_compensation_delay_frames > MAX_NATIVE_OUTPUT_COMPENSATION_DELAY_FRAMES
            || limits.max_block_frames == 0
            || limits.max_block_frames > MAX_NATIVE_OUTPUT_BLOCK_FRAMES
            || limits.max_work_units_per_block == 0
            || topology.destinations.len() > MAX_NATIVE_OUTPUT_DESTINATIONS
            || topology.edges.len() > MAX_NATIVE_OUTPUT_GRAPH_EDGES
        {
            return Err(MultiOutputRoutingError::InvalidLimits);
        }
        if self.routes.len() > limits.max_routes {
            return Err(MultiOutputRoutingError::OutputBudgetExceeded);
        }

        let mut port_fanout = [0_usize; MAX_NATIVE_OUTPUT_PORTS];
        for (idx, route) in self.routes.iter().enumerate() {
            if route.source_id != self.source_id {
                return Err(MultiOutputRoutingError::StaleSourceIdentity);
            }
            if !route.gain.is_finite()
                || !(0.0..=MAX_NATIVE_OUTPUT_ROUTE_GAIN).contains(&route.gain)
            {
                return Err(MultiOutputRoutingError::InvalidGain);
            }
            let Some(port_index) = ports.iter().position(|p| p.id == route.port_id) else {
                return Err(MultiOutputRoutingError::UnknownPort);
            };
            if self.routes[..idx]
                .iter()
                .any(|prior| prior.route_id == route.route_id)
            {
                return Err(MultiOutputRoutingError::DuplicateRouteId);
            }
            if route.destination == NativeOutputDestination::Unconnected {
                continue;
            }
            if !topology
                .destinations
                .iter()
                .any(|d| d.destination == route.destination)
            {
                return Err(MultiOutputRoutingError::UnknownDestination);
            }
            if self.routes[..idx].iter().any(|prior| {
                prior.port_id == route.port_id && prior.destination == route.destination
            }) {
                return Err(MultiOutputRoutingError::DuplicateRoute);
            }
            port_fanout[port_index] += 1;
            if port_fanout[port_index] > limits.max_fanout_per_port {
                return Err(MultiOutputRoutingError::FanoutBudgetExceeded);
            }
            if creates_cycle(topology, route.destination) {
                return Err(MultiOutputRoutingError::CyclicRoute);
            }
        }
        Ok(())
    }

    /// Strict v1 binary serialization for save/reopen round-trip verification.
    pub fn encode_bytes(&self) -> Result<Vec<u8>, MultiOutputRoutingError> {
        if self.routes.len() > MAX_NATIVE_OUTPUT_ROUTES {
            return Err(MultiOutputRoutingError::OutputBudgetExceeded);
        }
        let mut out = Vec::with_capacity(20 + self.routes.len() * 34);
        out.extend_from_slice(PLAN_MAGIC);
        out.extend_from_slice(&NATIVE_MULTI_OUTPUT_PLAN_VERSION.to_le_bytes());
        out.extend_from_slice(&self.source_id.get().to_le_bytes());
        out.extend_from_slice(&(self.routes.len() as u32).to_le_bytes());
        for route in &self.routes {
            if !route.gain.is_finite()
                || !(0.0..=MAX_NATIVE_OUTPUT_ROUTE_GAIN).contains(&route.gain)
            {
                return Err(MultiOutputRoutingError::InvalidGain);
            }
            out.extend_from_slice(&route.route_id.get().to_le_bytes());
            out.extend_from_slice(&route.source_id.get().to_le_bytes());
            out.extend_from_slice(&route.port_id.get().to_le_bytes());
            match route.destination {
                NativeOutputDestination::Unconnected => {
                    out.push(0);
                    out.extend_from_slice(&0_u64.to_le_bytes());
                }
                NativeOutputDestination::Track(id) => {
                    out.push(1);
                    out.extend_from_slice(&id.get().to_le_bytes());
                }
                NativeOutputDestination::Return(id) => {
                    out.push(2);
                    out.extend_from_slice(&id.get().to_le_bytes());
                }
            }
            out.extend_from_slice(&route.gain.to_le_bytes());
            out.push(u8::from(route.enabled));
        }
        Ok(out)
    }

    /// Strict v1 binary decoder rejecting unknown versions, malformed fields,
    /// invalid gains, or trailing bytes.
    pub fn decode_bytes(bytes: &[u8]) -> Result<Self, MultiOutputRoutingError> {
        let mut cursor = 0_usize;
        let read = |cursor: &mut usize, len: usize| -> Result<&[u8], MultiOutputRoutingError> {
            let end = cursor
                .checked_add(len)
                .ok_or(MultiOutputRoutingError::MalformedPlanBytes)?;
            let slice = bytes
                .get(*cursor..end)
                .ok_or(MultiOutputRoutingError::MalformedPlanBytes)?;
            *cursor = end;
            Ok(slice)
        };
        if read(&mut cursor, 4)? != PLAN_MAGIC {
            return Err(MultiOutputRoutingError::MalformedPlanBytes);
        }
        let version = u32::from_le_bytes(
            read(&mut cursor, 4)?
                .try_into()
                .map_err(|_| MultiOutputRoutingError::MalformedPlanBytes)?,
        );
        if version != NATIVE_MULTI_OUTPUT_PLAN_VERSION {
            return Err(MultiOutputRoutingError::UnsupportedPlanVersion);
        }
        let source_raw = u64::from_le_bytes(
            read(&mut cursor, 8)?
                .try_into()
                .map_err(|_| MultiOutputRoutingError::MalformedPlanBytes)?,
        );
        let source_id =
            NonZeroU64::new(source_raw).ok_or(MultiOutputRoutingError::MalformedPlanBytes)?;
        let count = usize::try_from(u32::from_le_bytes(
            read(&mut cursor, 4)?
                .try_into()
                .map_err(|_| MultiOutputRoutingError::MalformedPlanBytes)?,
        ))
        .map_err(|_| MultiOutputRoutingError::MalformedPlanBytes)?;
        if count > MAX_NATIVE_OUTPUT_ROUTES {
            return Err(MultiOutputRoutingError::OutputBudgetExceeded);
        }
        let mut routes = Vec::with_capacity(count);
        for _ in 0..count {
            let route_id = NonZeroU64::new(u64::from_le_bytes(
                read(&mut cursor, 8)?
                    .try_into()
                    .map_err(|_| MultiOutputRoutingError::MalformedPlanBytes)?,
            ))
            .ok_or(MultiOutputRoutingError::MalformedPlanBytes)?;
            let route_source = NonZeroU64::new(u64::from_le_bytes(
                read(&mut cursor, 8)?
                    .try_into()
                    .map_err(|_| MultiOutputRoutingError::MalformedPlanBytes)?,
            ))
            .ok_or(MultiOutputRoutingError::MalformedPlanBytes)?;
            let port_id = NativeOutputPortId::new(
                NonZeroU32::new(u32::from_le_bytes(
                    read(&mut cursor, 4)?
                        .try_into()
                        .map_err(|_| MultiOutputRoutingError::MalformedPlanBytes)?,
                ))
                .ok_or(MultiOutputRoutingError::MalformedPlanBytes)?,
            );
            let dest_tag = read(&mut cursor, 1)?[0];
            let dest_raw = u64::from_le_bytes(
                read(&mut cursor, 8)?
                    .try_into()
                    .map_err(|_| MultiOutputRoutingError::MalformedPlanBytes)?,
            );
            let destination = match dest_tag {
                0 => {
                    if dest_raw != 0 {
                        return Err(MultiOutputRoutingError::MalformedPlanBytes);
                    }
                    NativeOutputDestination::Unconnected
                }
                1 => NativeOutputDestination::Track(
                    NonZeroU64::new(dest_raw).ok_or(MultiOutputRoutingError::MalformedPlanBytes)?,
                ),
                2 => NativeOutputDestination::Return(
                    NonZeroU64::new(dest_raw).ok_or(MultiOutputRoutingError::MalformedPlanBytes)?,
                ),
                _ => return Err(MultiOutputRoutingError::MalformedPlanBytes),
            };
            let gain = f32::from_le_bytes(
                read(&mut cursor, 4)?
                    .try_into()
                    .map_err(|_| MultiOutputRoutingError::MalformedPlanBytes)?,
            );
            if !gain.is_finite() || !(0.0..=MAX_NATIVE_OUTPUT_ROUTE_GAIN).contains(&gain) {
                return Err(MultiOutputRoutingError::InvalidGain);
            }
            let enabled = match read(&mut cursor, 1)?[0] {
                0 => false,
                1 => true,
                _ => return Err(MultiOutputRoutingError::MalformedPlanBytes),
            };
            routes.push(NativeMultiOutputRoute {
                route_id,
                source_id: route_source,
                port_id,
                destination,
                gain,
                enabled,
            });
        }
        if cursor != bytes.len() {
            return Err(MultiOutputRoutingError::MalformedPlanBytes);
        }
        Ok(Self { source_id, routes })
    }
}

fn creates_cycle(
    topology: &NativeMultiOutputTopology,
    destination: NativeOutputDestination,
) -> bool {
    if destination == NativeOutputDestination::Unconnected
        || topology.source_host == NativeOutputDestination::Unconnected
    {
        return false;
    }
    if destination == topology.source_host {
        return true;
    }
    let mut visited = [NativeOutputDestination::Unconnected; MAX_NATIVE_OUTPUT_DESTINATIONS];
    let mut visited_len = 0_usize;
    let mut queue = [NativeOutputDestination::Unconnected; MAX_NATIVE_OUTPUT_DESTINATIONS];
    let mut head = 0_usize;
    let mut tail = 0_usize;

    queue[tail] = destination;
    tail += 1;
    visited[visited_len] = destination;
    visited_len += 1;

    while head < tail {
        let current = queue[head];
        head += 1;
        for edge in &topology.edges {
            if edge.from != current {
                continue;
            }
            if edge.to == topology.source_host {
                return true;
            }
            if edge.to != NativeOutputDestination::Unconnected
                && !visited[..visited_len].contains(&edge.to)
                && visited_len < visited.len()
            {
                visited[visited_len] = edge.to;
                visited_len += 1;
                queue[tail] = edge.to;
                tail += 1;
            }
        }
    }
    false
}

/// Atomic one-step edits over a `NativeMultiOutputPlan`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NativeMultiOutputEdit {
    AddRoute(NativeMultiOutputRoute),
    RemoveRoute(NonZeroU64),
    DisconnectPort(NativeOutputPortId),
    SetRouteGain {
        route_id: NonZeroU64,
        gain: f32,
    },
    SetRouteEnabled {
        route_id: NonZeroU64,
        enabled: bool,
    },
    SetRouteDestination {
        route_id: NonZeroU64,
        destination: NativeOutputDestination,
    },
}

/// Atomic one-step Undo/Redo history over a `NativeMultiOutputPlan`.
#[derive(Clone, Debug)]
pub struct NativeMultiOutputHistory {
    current: NativeMultiOutputPlan,
    undo_stack: Vec<NativeMultiOutputPlan>,
    redo_stack: Vec<NativeMultiOutputPlan>,
}

impl NativeMultiOutputHistory {
    pub fn new(
        initial: NativeMultiOutputPlan,
        ports: &[NativeOutputPortDescriptor],
        topology: &NativeMultiOutputTopology,
        limits: &NativeMultiOutputLimits,
    ) -> Result<Self, MultiOutputRoutingError> {
        initial.validate(ports, topology, limits)?;
        Ok(Self {
            current: initial,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        })
    }

    pub fn current(&self) -> &NativeMultiOutputPlan {
        &self.current
    }

    pub fn apply(
        &mut self,
        edit: NativeMultiOutputEdit,
        ports: &[NativeOutputPortDescriptor],
        topology: &NativeMultiOutputTopology,
        limits: &NativeMultiOutputLimits,
    ) -> Result<&NativeMultiOutputPlan, MultiOutputRoutingError> {
        let mut candidate = self.current.clone();
        match edit {
            NativeMultiOutputEdit::AddRoute(route) => candidate.routes.push(route),
            NativeMultiOutputEdit::RemoveRoute(id) => {
                let pos = candidate
                    .routes
                    .iter()
                    .position(|r| r.route_id == id)
                    .ok_or(MultiOutputRoutingError::RouteNotFound)?;
                candidate.routes.remove(pos);
            }
            NativeMultiOutputEdit::DisconnectPort(port_id) => {
                for route in &mut candidate.routes {
                    if route.port_id == port_id {
                        route.destination = NativeOutputDestination::Unconnected;
                    }
                }
            }
            NativeMultiOutputEdit::SetRouteGain { route_id, gain } => {
                let route = candidate
                    .routes
                    .iter_mut()
                    .find(|r| r.route_id == route_id)
                    .ok_or(MultiOutputRoutingError::RouteNotFound)?;
                route.gain = gain;
            }
            NativeMultiOutputEdit::SetRouteEnabled { route_id, enabled } => {
                let route = candidate
                    .routes
                    .iter_mut()
                    .find(|r| r.route_id == route_id)
                    .ok_or(MultiOutputRoutingError::RouteNotFound)?;
                route.enabled = enabled;
            }
            NativeMultiOutputEdit::SetRouteDestination {
                route_id,
                destination,
            } => {
                let route = candidate
                    .routes
                    .iter_mut()
                    .find(|r| r.route_id == route_id)
                    .ok_or(MultiOutputRoutingError::RouteNotFound)?;
                route.destination = destination;
            }
        }
        candidate.validate(ports, topology, limits)?;
        let prev = std::mem::replace(&mut self.current, candidate);
        self.undo_stack.push(prev);
        self.redo_stack.clear();
        Ok(&self.current)
    }

    pub fn undo(&mut self) -> Option<&NativeMultiOutputPlan> {
        let prev = self.undo_stack.pop()?;
        let cur = std::mem::replace(&mut self.current, prev);
        self.redo_stack.push(cur);
        Some(&self.current)
    }

    pub fn redo(&mut self) -> Option<&NativeMultiOutputPlan> {
        let next = self.redo_stack.pop()?;
        let cur = std::mem::replace(&mut self.current, next);
        self.undo_stack.push(cur);
        Some(&self.current)
    }
}

/// Caller-owned stereo output buffer tagged with its `NativeOutputDestination`.
pub struct MultiOutputDestinationBuffer<'a> {
    pub destination: NativeOutputDestination,
    pub frames: &'a mut [StereoFrame],
}

/// Optional multi-output native instrument runtime writing up to two stereo
/// output ports into caller-owned preallocated buffers without callback allocation.
pub trait MultiOutputInstrumentRuntime: Send {
    fn output_port_descriptors(&self) -> &[NativeOutputPortDescriptor];

    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
        resources: &dyn DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError>;

    fn process_block_multi_output(
        &mut self,
        events: &[NativeInstrumentEvent],
        outputs: &mut [&mut [StereoFrame]],
        clock: Option<&dyn NativeInstrumentClock>,
        resources: &dyn DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError>;

    fn reset(&mut self);

    fn is_quiescent(&self) -> bool;

    fn fixed_storage_bytes(&self) -> usize;
}

struct RouteDelayRing {
    frames: Vec<StereoFrame>,
    next: usize,
}

impl RouteDelayRing {
    fn new_on_control(len: u32) -> Result<Self, MultiOutputRoutingError> {
        let count =
            usize::try_from(len).map_err(|_| MultiOutputRoutingError::CompensationDelayExceeded)?;
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(count)
            .map_err(|_| MultiOutputRoutingError::AllocationFailed)?;
        frames.resize(count, [0.0; 2]);
        Ok(Self { frames, next: 0 })
    }

    fn process(&mut self, samples: &mut [StereoFrame]) {
        let len = self.frames.len();
        if len == 0 {
            return;
        }
        for sample in samples {
            let out = self.frames[self.next];
            self.frames[self.next] = *sample;
            self.next = (self.next + 1) % len;
            *sample = out;
        }
    }

    fn copy_from(&mut self, other: &Self) {
        if self.frames.len() == other.frames.len() {
            self.frames.copy_from_slice(&other.frames);
            self.next = other.next;
        }
    }

    fn reset(&mut self) {
        self.frames.fill([0.0; 2]);
        self.next = 0;
    }

    fn is_quiet(&self) -> bool {
        self.frames.iter().all(|f| f[0] == 0.0 && f[1] == 0.0)
    }
}

struct PreparedRouteEntry {
    route_id: NonZeroU64,
    port_index: usize,
    port_id: NativeOutputPortId,
    destination: NativeOutputDestination,
    gain: f32,
    enabled: bool,
    compensation_delay_frames: u32,
    committed_delay: Option<RouteDelayRing>,
    scratch_delay: Option<RouteDelayRing>,
}

/// Control-prepared, real-time-safe PDC multi-output router for one native source.
pub struct PreparedMultiOutputRouter {
    source_id: NonZeroU64,
    port_count: usize,
    target_arrival_frames: u32,
    max_block_frames: usize,
    declared_storage_bytes: usize,
    routes: Vec<PreparedRouteEntry>,
    route_scratch: Vec<StereoFrame>,
    block_started: bool,
}

impl PreparedMultiOutputRouter {
    /// Validate `plan` against `ports`, `topology`, and `limits`, compute exact
    /// per-route PDC compensation delays, and preallocate all rings and scratch
    /// storage on the control thread.
    pub fn prepare_on_control(
        plan: &NativeMultiOutputPlan,
        ports: &[NativeOutputPortDescriptor],
        topology: &NativeMultiOutputTopology,
        limits: NativeMultiOutputLimits,
    ) -> Result<Self, MultiOutputRoutingError> {
        plan.validate(ports, topology, &limits)?;

        let mut target_arrival = 0_u32;
        for route in &plan.routes {
            if !route.enabled || route.destination == NativeOutputDestination::Unconnected {
                continue;
            }
            let port = ports
                .iter()
                .find(|p| p.id == route.port_id)
                .ok_or(MultiOutputRoutingError::UnknownPort)?;
            let dest_info = topology
                .destinations
                .iter()
                .find(|d| d.destination == route.destination)
                .ok_or(MultiOutputRoutingError::UnknownDestination)?;
            let path_latency = port
                .algorithmic_latency_frames
                .checked_add(dest_info.latency_frames)
                .ok_or(MultiOutputRoutingError::CompensationDelayExceeded)?;
            target_arrival = target_arrival.max(path_latency);
        }

        let work_units = plan
            .routes
            .len()
            .max(1)
            .checked_mul(limits.max_block_frames)
            .and_then(|n| n.checked_mul(2))
            .ok_or(MultiOutputRoutingError::WorkBudgetExceeded)?;
        if work_units > limits.max_work_units_per_block {
            return Err(MultiOutputRoutingError::WorkBudgetExceeded);
        }

        let mut entries = Vec::new();
        entries
            .try_reserve_exact(plan.routes.len())
            .map_err(|_| MultiOutputRoutingError::AllocationFailed)?;

        let mut total_bytes = std::mem::size_of::<Self>()
            .checked_add(
                limits
                    .max_block_frames
                    .checked_mul(std::mem::size_of::<StereoFrame>())
                    .ok_or(MultiOutputRoutingError::RuntimeBudgetExceeded)?,
            )
            .ok_or(MultiOutputRoutingError::RuntimeBudgetExceeded)?;

        for route in &plan.routes {
            let port_index = ports
                .iter()
                .position(|p| p.id == route.port_id)
                .ok_or(MultiOutputRoutingError::UnknownPort)?;
            let comp_delay =
                if route.enabled && route.destination != NativeOutputDestination::Unconnected {
                    let port_lat = ports[port_index].algorithmic_latency_frames;
                    let dest_lat = topology
                        .destinations
                        .iter()
                        .find(|d| d.destination == route.destination)
                        .ok_or(MultiOutputRoutingError::UnknownDestination)?
                        .latency_frames;
                    let path = port_lat
                        .checked_add(dest_lat)
                        .ok_or(MultiOutputRoutingError::CompensationDelayExceeded)?;
                    let delay = target_arrival.saturating_sub(path);
                    if delay > limits.max_compensation_delay_frames {
                        return Err(MultiOutputRoutingError::CompensationDelayExceeded);
                    }
                    delay
                } else {
                    0
                };

            let delay_bytes = usize::try_from(comp_delay)
                .map_err(|_| MultiOutputRoutingError::RuntimeBudgetExceeded)?
                .checked_mul(2)
                .and_then(|n| n.checked_mul(std::mem::size_of::<StereoFrame>()))
                .ok_or(MultiOutputRoutingError::RuntimeBudgetExceeded)?;
            total_bytes = total_bytes
                .checked_add(std::mem::size_of::<PreparedRouteEntry>())
                .and_then(|n| n.checked_add(delay_bytes))
                .ok_or(MultiOutputRoutingError::RuntimeBudgetExceeded)?;
            if total_bytes > limits.max_pair_bytes {
                return Err(MultiOutputRoutingError::RuntimeBudgetExceeded);
            }

            let (committed_delay, scratch_delay) = if comp_delay > 0 {
                (
                    Some(RouteDelayRing::new_on_control(comp_delay)?),
                    Some(RouteDelayRing::new_on_control(comp_delay)?),
                )
            } else {
                (None, None)
            };

            entries.push(PreparedRouteEntry {
                route_id: route.route_id,
                port_index,
                port_id: route.port_id,
                destination: route.destination,
                gain: route.gain,
                enabled: route.enabled,
                compensation_delay_frames: comp_delay,
                committed_delay,
                scratch_delay,
            });
        }

        let mut route_scratch = Vec::new();
        route_scratch
            .try_reserve_exact(limits.max_block_frames)
            .map_err(|_| MultiOutputRoutingError::AllocationFailed)?;
        route_scratch.resize(limits.max_block_frames, [0.0; 2]);

        Ok(Self {
            source_id: plan.source_id,
            port_count: ports.len(),
            target_arrival_frames: target_arrival,
            max_block_frames: limits.max_block_frames,
            declared_storage_bytes: total_bytes,
            routes: entries,
            route_scratch,
            block_started: false,
        })
    }

    pub const fn source_id(&self) -> NonZeroU64 {
        self.source_id
    }

    pub const fn port_count(&self) -> usize {
        self.port_count
    }

    pub const fn target_arrival_frames(&self) -> u32 {
        self.target_arrival_frames
    }

    pub const fn declared_storage_bytes(&self) -> usize {
        self.declared_storage_bytes
    }

    pub fn route_compensation_delay_frames(&self, route_id: NonZeroU64) -> Option<u32> {
        self.routes
            .iter()
            .find(|r| r.route_id == route_id)
            .map(|r| r.compensation_delay_frames)
    }

    pub fn begin_block(&mut self) {
        for route in &mut self.routes {
            if let (Some(scratch), Some(committed)) =
                (&mut route.scratch_delay, &route.committed_delay)
            {
                scratch.copy_from(committed);
            }
        }
        self.block_started = true;
    }

    /// Route pre-rendered port buffers to caller-owned destination buffers with
    /// exact PDC alignment and gain weighting. Performs zero callback allocations.
    pub fn route_ports(
        &mut self,
        port_inputs: &[&[StereoFrame]],
        destinations: &mut [MultiOutputDestinationBuffer<'_>],
    ) -> Result<(), MultiOutputRoutingError> {
        if !self.block_started {
            return Err(MultiOutputRoutingError::BlockNotStarted);
        }
        if port_inputs.len() != self.port_count {
            return Err(MultiOutputRoutingError::PortBufferMismatch);
        }
        let len = port_inputs[0].len();
        if len > self.max_block_frames {
            return Err(MultiOutputRoutingError::BlockTooLarge);
        }
        if port_inputs.iter().any(|buf| buf.len() != len) {
            return Err(MultiOutputRoutingError::PortBufferMismatch);
        }
        // Read-only preflight: verify every active connected route has a matching destination buffer.
        for route in &self.routes {
            if !route.enabled || route.destination == NativeOutputDestination::Unconnected {
                continue;
            }
            let Some(dest_buf) = destinations
                .iter()
                .find(|d| d.destination == route.destination)
            else {
                return Err(MultiOutputRoutingError::MissingDestinationBuffer);
            };
            if dest_buf.frames.len() != len {
                return Err(MultiOutputRoutingError::MissingDestinationBuffer);
            }
        }
        if len == 0 {
            return Ok(());
        }

        for route in &mut self.routes {
            if !route.enabled || route.destination == NativeOutputDestination::Unconnected {
                continue;
            }
            let scratch = &mut self.route_scratch[..len];
            scratch.copy_from_slice(port_inputs[route.port_index]);
            if let Some(delay) = &mut route.scratch_delay {
                delay.process(scratch);
            }
            if let Some(dest_buf) = destinations
                .iter_mut()
                .find(|d| d.destination == route.destination)
            {
                let gain = route.gain;
                for (dst, &src) in dest_buf.frames.iter_mut().zip(scratch.iter()) {
                    dst[0] += src[0] * gain;
                    dst[1] += src[1] * gain;
                }
            }
        }
        Ok(())
    }

    pub fn commit_block(&mut self) {
        if self.block_started {
            for route in &mut self.routes {
                std::mem::swap(&mut route.committed_delay, &mut route.scratch_delay);
            }
            self.block_started = false;
        }
    }

    pub fn rollback_block(&mut self) {
        if self.block_started {
            for route in &mut self.routes {
                if let (Some(scratch), Some(committed)) =
                    (&mut route.scratch_delay, &route.committed_delay)
                {
                    scratch.copy_from(committed);
                }
            }
            self.block_started = false;
        }
    }

    pub fn reset(&mut self) {
        for route in &mut self.routes {
            if let Some(delay) = &mut route.committed_delay {
                delay.reset();
            }
            if let Some(delay) = &mut route.scratch_delay {
                delay.reset();
            }
        }
        self.block_started = false;
    }

    pub fn committed_is_quiescent(&self) -> bool {
        !self.block_started
            && self.routes.iter().all(|route| {
                route
                    .committed_delay
                    .as_ref()
                    .is_none_or(RouteDelayRing::is_quiet)
            })
    }

    pub fn can_inherit_route_from(&self, route_id: NonZeroU64, previous: &Self) -> bool {
        if self.block_started || previous.block_started || self.source_id != previous.source_id {
            return false;
        }
        let (Some(dst), Some(src)) = (
            self.routes.iter().find(|r| r.route_id == route_id),
            previous.routes.iter().find(|r| r.route_id == route_id),
        ) else {
            return false;
        };
        dst.port_id == src.port_id
            && dst.destination == src.destination
            && dst.enabled == src.enabled
            && dst.compensation_delay_frames == src.compensation_delay_frames
    }

    pub fn inherit_compatible_routes_from(
        &mut self,
        previous: &Self,
    ) -> Result<usize, MultiOutputRoutingError> {
        if self.block_started || previous.block_started {
            return Err(MultiOutputRoutingError::BlockNotStarted);
        }
        let mut count = 0_usize;
        for dst in &mut self.routes {
            if let Some(src) = previous.routes.iter().find(|r| {
                r.route_id == dst.route_id
                    && r.port_id == dst.port_id
                    && r.destination == dst.destination
                    && r.enabled == dst.enabled
                    && r.compensation_delay_frames == dst.compensation_delay_frames
            }) {
                if let (Some(dst_c), Some(dst_s), Some(src_c)) = (
                    &mut dst.committed_delay,
                    &mut dst.scratch_delay,
                    &src.committed_delay,
                ) {
                    dst_c.copy_from(src_c);
                    dst_s.copy_from(src_c);
                }
                count += 1;
            }
        }
        Ok(count)
    }
}
