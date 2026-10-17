//! Typed production signal topology and control-prepared compensation ownership.
use crate::{Frame, Invalid, Project, Track, compensation::*};
mod legacy;
use hex_project::{ReturnId, RoutingOwner, TrackId};
use std::{collections::BTreeMap, mem::size_of, sync::Arc};

pub(crate) const MAX_COEXISTING_COMPENSATION_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum NodeId {
    Audio(u64),
    TrackInput(u64),
    Generator(u64),
    Merge(u64),
    Fader(u64),
    Device(u64),
    ReturnInput(u64),
    ReturnOutput(u64),
    Main,
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Send {
    pub target: usize,
    pub gain: f32,
    pub edge: usize,
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TrackPlan {
    pub id: Option<TrackId>,
    pub devices: Vec<usize>,
    pub generator: Option<usize>,
    pub merge: usize,
    pub fader: usize,
    pub source_input: usize,
    pub audio_merge: usize,
    pub generator_merge: Option<usize>,
    pub main: usize,
    pub sends: Vec<Send>,
    pub audio_feeds: Vec<Send>,
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReturnPlan {
    pub id: ReturnId,
    pub devices: Vec<usize>,
    pub output: usize,
    pub main: usize,
    pub feeds: Vec<Send>,
    pub audio_feeds: Vec<Send>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Owner {
    Track(usize),
    Return(usize),
}

#[derive(Debug, PartialEq)]
pub(crate) struct SignalGraph {
    pub plan: LatencyPlan,
    pub tracks: Vec<TrackPlan>,
    pub returns: Vec<ReturnPlan>,
    pub owner_order: Vec<Owner>,
    pub main: usize,
    typed: Vec<NodeId>,
}
fn error(error: CompensationError) -> Invalid {
    Invalid(match error {
        CompensationError::UnknownLatency => "enabled processor latency is unknown",
        CompensationError::LatencyLimit => {
            "cumulative latency exceeds the compensation frame limit"
        }
        CompensationError::StorageLimit => "signal compensation storage exceeds its bounded budget",
        CompensationError::GraphLimit => {
            "signal compensation graph exceeds its bounded node or edge limit"
        }
        CompensationError::Allocation => "could not reserve signal compensation storage",
        _ => "signal compensation plan is invalid or exceeds its bounded budget",
    })
}
#[derive(Default)]
struct Builder {
    nodes: Vec<(NodeId, Option<u32>)>,
    edges: Vec<(usize, usize)>,
}
impl Builder {
    fn node(&mut self, id: NodeId, latency: Option<u32>) -> usize {
        let index = self.nodes.len();
        self.nodes.push((id, latency));
        index
    }
    fn edge(&mut self, from: usize, to: usize) -> usize {
        let index = self.edges.len();
        self.edges.push((from, to));
        index
    }
}
impl SignalGraph {
    pub(crate) fn prepare(
        project: &Project,
        feeds: &[(usize, usize, f32)],
        canonical: &hex_project::Project,
    ) -> Result<Option<Arc<Self>>, Invalid> {
        let mut builder = Builder::default();
        let mut tracks = Vec::new();
        for (index, track) in project.tracks.iter().enumerate() {
            if track.instrument_runtime_latency != track.instrument_latency {
                return Err(Invalid(
                    "instrument and mutable runtime latency declarations disagree",
                ));
            }
            let id = track
                .canonical_track_id()
                .map_or(index as u64, TrackId::value);
            let audio = builder.node(NodeId::Audio(id), Some(0));
            let input = builder.node(NodeId::TrackInput(id), Some(0));
            let source_input = builder.edge(audio, input);
            let mut previous = input;
            let mut devices = Vec::new();
            for device in track.device_chain.iter().take(track.instrument_position) {
                let node = builder.node(
                    NodeId::Device(device.device_id),
                    device.algorithmic_latency_frames(),
                );
                builder.edge(previous, node);
                previous = node;
                devices.push(node);
            }
            let generator = track
                .instrument_slot_enabled()
                .then(|| builder.node(NodeId::Generator(id), track.instrument_latency));
            let merge = builder.node(NodeId::Merge(id), Some(0));
            let audio_merge = builder.edge(previous, merge);
            let generator_merge = generator.map(|node| builder.edge(node, merge));
            previous = merge;
            for device in track.device_chain.iter().skip(track.instrument_position) {
                let node = builder.node(
                    NodeId::Device(device.device_id),
                    device.algorithmic_latency_frames(),
                );
                builder.edge(previous, node);
                previous = node;
                devices.push(node);
            }
            let fader = builder.node(NodeId::Fader(id), Some(0));
            builder.edge(previous, fader);
            tracks.push(TrackPlan {
                id: track.canonical_track_id(),
                devices,
                generator,
                merge,
                fader,
                source_input,
                audio_merge,
                generator_merge,
                main: 0,
                sends: Vec::new(),
                audio_feeds: Vec::new(),
            });
        }
        let mut returns = Vec::new();
        let mut inputs = Vec::new();
        for bus in &project.return_buses {
            let input = builder.node(NodeId::ReturnInput(bus.id.value()), Some(0));
            inputs.push(input);
            let mut previous = input;
            let mut devices = Vec::new();
            for device in bus.device_chain.iter() {
                let node = builder.node(
                    NodeId::Device(device.device_id),
                    device.algorithmic_latency_frames(),
                );
                builder.edge(previous, node);
                previous = node;
                devices.push(node);
            }
            let output = builder.node(NodeId::ReturnOutput(bus.id.value()), Some(0));
            builder.edge(previous, output);
            returns.push(ReturnPlan {
                id: bus.id,
                devices,
                output,
                main: 0,
                feeds: Vec::new(),
                audio_feeds: Vec::new(),
            });
        }
        let main = builder.node(NodeId::Main, Some(0));
        for (index, track) in tracks.iter_mut().enumerate() {
            track.main = builder.edge(track.fader, main);
            for send in &project.tracks[index].sends {
                let input = *inputs
                    .get(send.return_index)
                    .ok_or(Invalid("compensation send target is missing"))?;
                track.sends.push(Send {
                    target: send.return_index,
                    gain: send.gain,
                    edge: builder.edge(track.fader, input),
                });
            }
        }
        for bus in &mut returns {
            bus.main = builder.edge(bus.output, main);
        }
        for &(source, target, gain) in feeds {
            let from = returns
                .get(source)
                .ok_or(Invalid("compensation feed source is missing"))?
                .output;
            let to = *inputs
                .get(target)
                .ok_or(Invalid("compensation feed target is missing"))?;
            returns[source].feeds.push(Send {
                target,
                gain,
                edge: builder.edge(from, to),
            });
        }
        for feed in canonical.audio_feeds() {
            let target = tracks
                .iter()
                .position(|track| track.id == Some(feed.destination()))
                .ok_or(Invalid("audio feed destination is missing"))?;
            let to = builder.edges[tracks[target].source_input].1;
            let (from, source) = match feed.source() {
                RoutingOwner::Track(id) => {
                    let index = tracks
                        .iter()
                        .position(|track| track.id == Some(id))
                        .ok_or(Invalid("audio feed source track is missing"))?;
                    (tracks[index].fader, Owner::Track(index))
                }
                RoutingOwner::Return(id) => {
                    let index = returns
                        .iter()
                        .position(|bus| bus.id == id)
                        .ok_or(Invalid("audio feed source return is missing"))?;
                    (returns[index].output, Owner::Return(index))
                }
                RoutingOwner::Main => return Err(Invalid("main cannot feed a track input")),
            };
            let send = Send {
                target,
                gain: feed.gain(),
                edge: builder.edge(from, to),
            };
            match source {
                Owner::Track(index) => tracks[index].audio_feeds.push(send),
                Owner::Return(index) => returns[index].audio_feeds.push(send),
            }
        }
        // Typed identities assign stable generic keys, independent of traversal order.
        let keys: BTreeMap<_, _> = builder
            .nodes
            .iter()
            .map(|(id, _)| *id)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .enumerate()
            .map(|(index, id)| (id, index as u64 + 1))
            .collect();
        if keys.len() != builder.nodes.len() {
            return Err(Invalid("signal graph has duplicate typed nodes"));
        }
        let nodes: Vec<_> = builder
            .nodes
            .iter()
            .map(|(id, latency)| LatencyNode {
                key: keys[id],
                latency_frames: *latency,
            })
            .collect();
        let mut edge_order: Vec<_> = (0..builder.edges.len()).collect();
        edge_order.sort_by_key(|&i| {
            let (from, to) = builder.edges[i];
            (keys[&builder.nodes[from].0], keys[&builder.nodes[to].0])
        });
        let mut edge_keys = vec![0; builder.edges.len()];
        for (key, &index) in edge_order.iter().enumerate() {
            edge_keys[index] = key as u64 + 1;
        }
        let edges: Vec<_> = builder
            .edges
            .iter()
            .enumerate()
            .map(|(i, &(from, to))| LatencyEdge {
                key: edge_keys[i],
                from: keys[&builder.nodes[from].0],
                to: keys[&builder.nodes[to].0],
            })
            .collect();
        let plan =
            LatencyPlan::build(&nodes, &edges, CompensationLimits::default()).map_err(error)?;
        if plan.max_latency_frames() == 0 && canonical.audio_feeds().is_empty() {
            return Ok(None);
        }
        let node_index = |old: usize| {
            plan.node_index(nodes[old].key)
                .ok_or(Invalid("signal graph node mapping is missing"))
        };
        let edge_index = |old: usize| {
            plan.edge_index(edge_keys[old])
                .ok_or(Invalid("signal graph edge mapping is missing"))
        };
        for track in &mut tracks {
            for device in &mut track.devices {
                *device = node_index(*device)?;
            }
            track.generator = track.generator.map(node_index).transpose()?;
            track.merge = node_index(track.merge)?;
            track.fader = node_index(track.fader)?;
            track.source_input = edge_index(track.source_input)?;
            track.audio_merge = edge_index(track.audio_merge)?;
            track.generator_merge = track.generator_merge.map(edge_index).transpose()?;
            track.main = edge_index(track.main)?;
            for send in track.sends.iter_mut().chain(&mut track.audio_feeds) {
                send.edge = edge_index(send.edge)?;
            }
        }
        for bus in &mut returns {
            for device in &mut bus.devices {
                *device = node_index(*device)?;
            }
            bus.output = node_index(bus.output)?;
            bus.main = edge_index(bus.main)?;
            for feed in bus.feeds.iter_mut().chain(&mut bus.audio_feeds) {
                feed.edge = edge_index(feed.edge)?;
            }
        }
        let mut order: Vec<_> = tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.fader, Owner::Track(i)))
            .chain(
                returns
                    .iter()
                    .enumerate()
                    .map(|(i, bus)| (bus.output, Owner::Return(i))),
            )
            .collect();
        let mut ranks = vec![0; plan.nodes().len()];
        for (rank, &node) in plan.topological_order().iter().enumerate() {
            ranks[node] = rank;
        }
        order.sort_by_key(|(node, _)| ranks[*node]);
        let owner_order = order.into_iter().map(|(_, owner)| owner).collect();
        let graph = Arc::new(Self {
            main: node_index(main)?,
            plan,
            tracks,
            returns,
            owner_order,
            typed: keys.into_keys().collect(),
        });
        // Charge actual prepared table capacity before admitting runtime owners.
        if graph
            .runtime_preflight_bytes()
            .ok_or(Invalid("signal graph storage count overflow"))?
            > MAX_COMPENSATION_BYTES
        {
            return Err(Invalid(
                "signal graph tables exceed the compensation budget",
            ));
        }
        Ok(Some(graph))
    }
    pub(crate) fn table_bytes(&self) -> Option<usize> {
        let mut total = size_of::<Self>()
            .checked_add(self.plan.storage_bytes().ok()?)?
            .checked_add(self.typed.capacity().checked_mul(size_of::<NodeId>())?)?
            .checked_add(self.tracks.capacity().checked_mul(size_of::<TrackPlan>())?)?
            .checked_add(
                self.returns
                    .capacity()
                    .checked_mul(size_of::<ReturnPlan>())?,
            )?
            .checked_add(
                self.owner_order
                    .capacity()
                    .checked_mul(size_of::<Owner>())?,
            )?;
        for track in &self.tracks {
            total = total
                .checked_add(track.devices.capacity().checked_mul(size_of::<usize>())?)?
                .checked_add(track.sends.capacity().checked_mul(size_of::<Send>())?)?
                .checked_add(
                    track
                        .audio_feeds
                        .capacity()
                        .checked_mul(size_of::<Send>())?,
                )?;
        }
        for bus in &self.returns {
            total = total
                .checked_add(bus.devices.capacity().checked_mul(size_of::<usize>())?)?
                .checked_add(bus.feeds.capacity().checked_mul(size_of::<Send>())?)?
                .checked_add(bus.audio_feeds.capacity().checked_mul(size_of::<Send>())?)?;
        }
        Some(total)
    }
    pub(crate) fn compatible(&self, other: &Self) -> bool {
        self.plan == other.plan
            && self.typed == other.typed
            && self
                .tracks
                .iter()
                .map(|track| track.id)
                .eq(other.tracks.iter().map(|track| track.id))
            && self
                .returns
                .iter()
                .map(|bus| bus.id)
                .eq(other.returns.iter().map(|bus| bus.id))
    }
    pub(crate) fn runtime_preflight_bytes(&self) -> Option<usize> {
        let stamp = (self.plan.max_latency_frames() as usize)
            .checked_add(1)?
            .checked_mul(size_of::<Option<TransportStamp>>())?
            .checked_mul(2)?;
        let rings = self.plan.edges().iter().try_fold(0_usize, |n, edge| {
            n.checked_add(
                (edge.delay_frames as usize)
                    .checked_mul(size_of::<Frame>())?
                    .checked_mul(2)?,
            )
        })?;
        self.table_bytes()?
            .checked_add(
                self.tracks
                    .len()
                    .checked_mul(size_of::<Frame>())?
                    .checked_mul(2)?,
            )?
            .checked_add(self.plan.storage_bytes().ok()?)?
            .checked_add(stamp)?
            .checked_add(rings)?
            .checked_add(
                self.plan
                    .edges()
                    .len()
                    .checked_mul(size_of::<DelayPair>())?,
            )
    }
    pub(crate) fn main_latency(&self) -> u32 {
        self.plan.nodes()[self.main].output_arrival_frames
    }
}

#[derive(Debug)]
pub(crate) struct SignalRuntime {
    pub graph: Arc<SignalGraph>,
    pub delays: CompensationRuntime,
    pub inputs: Vec<Frame>,
    pub main: Vec<Frame>,
}
impl SignalRuntime {
    pub(crate) fn new(graph: Arc<SignalGraph>) -> Result<Self, Invalid> {
        let scratch_bytes = graph
            .tracks
            .len()
            .checked_mul(size_of::<Frame>())
            .and_then(|n| n.checked_mul(2))
            .ok_or(Invalid("signal scratch count overflow"))?;
        let table_bytes = graph
            .table_bytes()
            .ok_or(Invalid("signal graph storage count overflow"))?;
        let limits = CompensationLimits {
            max_storage_bytes: MAX_COMPENSATION_BYTES
                .checked_sub(table_bytes)
                .and_then(|n| n.checked_sub(scratch_bytes))
                .ok_or(Invalid("signal graph storage exceeds its budget"))?,
            ..CompensationLimits::default()
        };
        let delays = CompensationRuntime::new(graph.plan.try_clone().map_err(error)?, limits)
            .map_err(error)?;
        let mut inputs = Vec::new();
        let mut main = Vec::new();
        inputs
            .try_reserve_exact(graph.tracks.len())
            .map_err(|_| Invalid("could not reserve signal inputs"))?;
        main.try_reserve_exact(graph.tracks.len())
            .map_err(|_| Invalid("could not reserve signal contributions"))?;
        inputs.resize(graph.tracks.len(), [0.0; 2]);
        main.resize(graph.tracks.len(), [0.0; 2]);
        let runtime = Self {
            graph,
            delays,
            inputs,
            main,
        };
        if runtime
            .storage_bytes()
            .is_none_or(|bytes| bytes > MAX_COMPENSATION_BYTES)
        {
            return Err(Invalid("signal runtime exceeds its storage budget"));
        }
        Ok(runtime)
    }
    pub(crate) fn storage_bytes(&self) -> Option<usize> {
        self.delays
            .storage_bytes()
            .checked_add(self.graph.table_bytes()?)?
            .checked_add(self.inputs.capacity().checked_mul(size_of::<Frame>())?)?
            .checked_add(self.main.capacity().checked_mul(size_of::<Frame>())?)
    }
    pub(crate) fn edge(&mut self, index: usize, frame: Frame) -> Result<Frame, Invalid> {
        let mut frames = [frame];
        self.delays
            .process_edge_in_place(index, &mut frames)
            .map_err(error)?;
        Ok(frames[0])
    }
    pub(crate) fn fader(
        &self,
        track: &Track,
        index: usize,
        override_gain: Option<f32>,
        frame: Frame,
    ) -> Result<Frame, Invalid> {
        let Some(origin) = self
            .delays
            .node_output_stamp(self.graph.tracks[index].fader)
            .map_err(error)?
        else {
            return Ok([0.0; 2]);
        };
        let mix = override_gain.map_or_else(
            || track.mix_at(origin.arrangement_frame),
            |gain| track.recording_mix(gain),
        );
        Ok(mix.apply_frame(frame))
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn process_track(
        &mut self,
        index: usize,
        track: &Track,
        chain: &mut crate::EffectRuntimeChain,
        mut audio: Frame,
        mut generator: Frame,
        offset: usize,
        metered: bool,
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
    ) -> Result<Frame, Invalid> {
        audio = self.edge(self.graph.tracks[index].source_input, audio)?;
        audio[0] += self.inputs[index][0];
        audio[1] += self.inputs[index][1];
        let slot = track.instrument_position();
        self.effects(
            &self.graph.tracks[index].devices[..slot],
            chain,
            0,
            &mut audio,
            offset,
            observer,
        )?;
        track.apply_instrument_input_gain(std::slice::from_mut(&mut audio));
        if metered {
            track.instrument_meter.observe_input(&[audio]);
        }
        audio = self.edge(self.graph.tracks[index].audio_merge, audio)?;
        if let Some(node) = self.graph.tracks[index].generator {
            if self
                .delays
                .node_output_stamp(node)
                .map_err(error)?
                .is_none()
            {
                generator = [0.0; 2];
            }
        }
        if let Some(edge) = self.graph.tracks[index].generator_merge {
            generator = self.edge(edge, generator)?;
        }
        let mut mixed = [audio[0] + generator[0], audio[1] + generator[1]];
        track.apply_instrument_output_gain(std::slice::from_mut(&mut mixed));
        if metered {
            track.instrument_meter.observe_output(&[mixed]);
        }
        self.effects(
            &self.graph.tracks[index].devices[slot..],
            chain,
            slot,
            &mut mixed,
            offset,
            observer,
        )?;
        Ok(mixed)
    }
    pub(crate) fn effects(
        &self,
        nodes: &[usize],
        chain: &mut crate::EffectRuntimeChain,
        start: usize,
        frame: &mut Frame,
        offset: usize,
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
    ) -> Result<(), Invalid> {
        for (local, &node) in nodes.iter().enumerate() {
            let clock = OriginClock(self.delays.node_input_stamp(node).map_err(error)?);
            let frames = std::slice::from_mut(frame);
            if let Some(observer) = observer.as_mut() {
                chain.process_range_tapped_with_clock(
                    start + local..start + local + 1,
                    frames,
                    offset,
                    Some(&clock),
                    &mut |owner, device, kind, phase, offset, frames| {
                        observer(
                            crate::PostNodeIdentity::new(owner, device, kind),
                            phase,
                            offset,
                            frames,
                        )
                    },
                )
            } else {
                chain.process_range_with_clock(
                    start + local..start + local + 1,
                    frames,
                    Some(&clock),
                )
            }
            .map_err(|_| Invalid("compensated native device range is invalid"))?;
            if self
                .delays
                .node_output_stamp(node)
                .map_err(error)?
                .is_none()
            {
                *frame = [0.0; 2];
            }
        }
        Ok(())
    }
}
struct OriginClock(Option<TransportStamp>);
impl hex_lattice::NativeInstrumentClock for OriginClock {
    fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
        if offset != 0 {
            return None;
        }
        self.0.and_then(|stamp| stamp.arrangement_beat_raw)
    }
    fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
        if offset != 0 {
            return None;
        }
        self.0
            .filter(|stamp| stamp.transport_is_running)
            .map(|stamp| stamp.performance_frame)
    }
    fn transport_is_running(&self) -> Option<bool> {
        self.0.map(|stamp| stamp.transport_is_running)
    }
    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        if offset != 0 {
            return None;
        }
        self.0.map(|stamp| stamp.performance_beat_raw)
    }
    fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
        if offset != 0 {
            return None;
        }
        self.0.map(|stamp| stamp.arrangement_frame)
    }
    fn loop_pass_at_offset(&self, offset: usize) -> Option<u64> {
        if offset != 0 {
            return None;
        }
        self.0.map(|stamp| stamp.loop_pass)
    }
}

#[cfg(test)]
pub(crate) mod tests;
