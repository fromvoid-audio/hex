//! Typed production signal topology and control-prepared compensation ownership.
use crate::{Frame, Invalid, Project, Track, compensation::*};
mod latency_observation;
mod legacy;
#[cfg(feature = "instrument-test-support")]
mod printed_branch_probe;
use hex_project::{GroupOutputMuteRange, ReturnId, RoutingOwner, TrackId};
pub use latency_observation::OutputLatency;
#[cfg(feature = "instrument-test-support")]
pub use printed_branch_probe::PrintedBranchProbe;
use std::{collections::BTreeMap, mem::size_of, sync::Arc};

pub(crate) const MAX_COEXISTING_COMPENSATION_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum NodeId {
    Audio(u64),
    TrackInput(u64),
    Generator(u64),
    Merge(u64),
    PrintedMerge(u64),
    Fader(u64),
    Device(u64),
    ReturnInput(u64),
    ReturnOutput(u64),
    Main,
}
impl NodeId {
    fn belongs_to_track(self, track: TrackId) -> bool {
        matches!(self,
            Self::Audio(id) | Self::TrackInput(id) | Self::Generator(id)
            | Self::Merge(id) | Self::PrintedMerge(id) | Self::Fader(id)
            if id == track.value())
    }
}
/// One silent owner whose new compensation edges start empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FreshHistory {
    Track(TrackId),
    /// A muted, isolated existing track whose old and new local rings are silent.
    ChangedTrack(TrackId, u64),
    Return(ReturnId),
}

impl FreshHistory {
    fn contains(self, node: NodeId) -> bool {
        match self {
            Self::Track(id) => node.belongs_to_track(id),
            Self::ChangedTrack(id, device) => {
                node.belongs_to_track(id) || node == NodeId::Device(device)
            }
            Self::Return(id) => {
                matches!(node, NodeId::ReturnInput(value) | NodeId::ReturnOutput(value) if value == id.value())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Send {
    pub target: usize,
    pub gain: f32,
    pub edge: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrackOutput {
    Main { edge: usize },
    Group { target: usize, edge: usize },
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
    /// Edges from the retained processing and printed-audio paths to their
    /// latency-aligned merge before the track fader.
    pub printed_merge: Option<(usize, usize)>,
    pub output: TrackOutput,
    pub sends: Vec<Send>,
    pub audio_feeds: Vec<Send>,
    group_output_mutes: Vec<GroupOutputMuteRange>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct KeyStage {
    source_track: usize,
    key_edge: usize,
    main_edge: usize,
}

#[derive(Debug, PartialEq)]
pub(crate) struct SignalGraph {
    pub plan: LatencyPlan,
    pub tracks: Vec<TrackPlan>,
    pub returns: Vec<ReturnPlan>,
    pub owner_order: Vec<Owner>,
    track_indices: Vec<(TrackId, usize)>,
    has_group_outputs: bool,
    pub main: usize,
    pub main_devices: Vec<usize>,
    pre_main_edges: Vec<usize>,
    key_stages: Vec<Option<KeyStage>>,
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
    /// Cheap prefilter for the single locally inserted processor case.
    pub(crate) fn has_one_more_node_than(&self, old: &Self) -> bool {
        old.typed.len().checked_add(1) == Some(self.typed.len())
    }
    #[cfg(test)]
    pub(crate) fn test_printed_branch(
        project: &Project,
        canonical: &hex_project::Project,
        index: usize,
    ) -> Result<Arc<Self>, Invalid> {
        Ok(Arc::new(Self::prepare_value(
            project,
            &[],
            canonical,
            &[index],
        )?))
    }
    /// Resolve the prepared stage's origin in this actual graph. Project device
    /// identities are unique; no caller-provided latency becomes authority.
    pub(crate) fn parameter_arrival(
        &self,
        input: crate::NativeParameterInputTarget,
    ) -> Result<u32, Invalid> {
        let node = self
            .typed
            .binary_search(&NodeId::Device(input.identity().device_id()))
            .ok()
            .and_then(|index| self.plan.nodes().get(index))
            .ok_or(Invalid("parameter stage is absent from the signal graph"))?;
        Ok(match input.target() {
            hex_lattice_contract::DeviceAutomationTarget::HostOutputGain => {
                node.output_arrival_frames
            }
            _ => node.input_arrival_frames,
        })
    }
    pub(crate) fn prepare(
        project: &Project,
        feeds: &[(usize, usize, f32)],
        canonical: &hex_project::Project,
    ) -> Result<Option<Arc<Self>>, Invalid> {
        let printed = Self::retained_receivers(project, canonical)?;
        Ok(Some(Arc::new(Self::prepare_value(
            project, feeds, canonical, &printed,
        )?)))
    }
    /// The same private topology constructor, retained by closed preparation.
    /// No caller Arc or copied graph declaration can enter this path.
    pub(crate) fn prepare_owned(
        project: &Project,
        feeds: &[(usize, usize, f32)],
        canonical: &hex_project::Project,
    ) -> Result<crate::owned_shared::OwnedShared<Self>, Invalid> {
        let printed = Self::retained_receivers(project, canonical)?;
        Ok(crate::owned_shared::OwnedShared::new_owned(
            Self::prepare_value(project, feeds, canonical, &printed)?,
        ))
    }
    fn retained_receivers(
        project: &Project,
        canonical: &hex_project::Project,
    ) -> Result<Vec<usize>, Invalid> {
        let mut indices = Vec::new();
        for receiver in canonical
            .tracks()
            .iter()
            .filter(|track| track.retained_source_path())
        {
            let index = project
                .tracks
                .iter()
                .position(|track| track.canonical_track_id() == Some(receiver.id()))
                .ok_or(Invalid("retained receiver is missing from prepared graph"))?;
            indices
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve retained receiver graph"))?;
            indices.push(index);
        }
        indices.sort_unstable();
        Ok(indices)
    }
    fn prepare_value(
        project: &Project,
        feeds: &[(usize, usize, f32)],
        canonical: &hex_project::Project,
        printed_audio_tracks: &[usize],
    ) -> Result<Self, Invalid> {
        if printed_audio_tracks
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
            || printed_audio_tracks.iter().any(|&index| {
                project
                    .tracks
                    .get(index)
                    .is_none_or(|track| track.instrument_id.is_none())
            })
        {
            return Err(Invalid("invalid printed audio branch"));
        }
        // Bound transient builder/lookup contexts before their first allocation.
        // BTree nodes are control-only opaque construction contexts, never
        // retained DATA or callback owners. No allocator/RSS claim is made.
        let devices = project
            .tracks
            .iter()
            .map(|t| t.device_chain.len())
            .chain(project.return_buses.iter().map(|r| r.device_chain.len()))
            .try_fold(project.main_device_chain.len(), |n, count| {
                n.checked_add(count)
            })
            .ok_or(Invalid("signal graph cardinality overflow"))?;
        let generators = project
            .tracks
            .iter()
            .filter(|t| t.instrument_slot_enabled())
            .count();
        let nodes = project
            .tracks
            .len()
            .checked_mul(4)
            .and_then(|n| n.checked_add(project.return_buses.len().checked_mul(2)?))
            .and_then(|n| n.checked_add(devices))
            .and_then(|n| n.checked_add(generators))
            .and_then(|n| n.checked_add(printed_audio_tracks.len()))
            .and_then(|n| n.checked_add(1))
            .ok_or(Invalid("signal graph cardinality overflow"))?;
        let sends = project
            .tracks
            .iter()
            .try_fold(0usize, |n, t| n.checked_add(t.sends.len()))
            .ok_or(Invalid("signal graph cardinality overflow"))?;
        let edges = nodes
            .checked_sub(1)
            .and_then(|n| n.checked_add(sends))
            .and_then(|n| n.checked_add(feeds.len()))
            .and_then(|n| n.checked_add(canonical.audio_feeds().len()))
            .and_then(|n| n.checked_add(canonical.sidechains().len()))
            .and_then(|n| n.checked_add(printed_audio_tracks.len()))
            .ok_or(Invalid("signal graph cardinality overflow"))?;
        if nodes > MAX_COMPENSATION_NODES || edges > MAX_COMPENSATION_EDGES {
            return Err(error(CompensationError::GraphLimit));
        }
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
            let printed_merge = if printed_audio_tracks.binary_search(&index).is_ok() {
                let node = builder.node(NodeId::PrintedMerge(id), Some(0));
                let processed = builder.edge(previous, node);
                let printed = builder.edge(audio, node);
                previous = node;
                Some((processed, printed))
            } else {
                None
            };
            let fader = builder.node(NodeId::Fader(id), Some(0));
            builder.edge(previous, fader);
            let mut group_output_mutes = Vec::new();
            if let Some(owner) = track
                .canonical_track_id()
                .and_then(|id| canonical.track(id))
            {
                let ranges = owner.group_output_mutes();
                group_output_mutes
                    .try_reserve_exact(ranges.len())
                    .map_err(|_| Invalid("could not reserve prepared group output mute ranges"))?;
                group_output_mutes.extend_from_slice(ranges);
            }
            tracks.push(TrackPlan {
                id: track.canonical_track_id(),
                devices,
                generator,
                merge,
                fader,
                source_input,
                audio_merge,
                generator_merge,
                printed_merge,
                output: TrackOutput::Main { edge: 0 },
                sends: Vec::new(),
                audio_feeds: Vec::new(),
                group_output_mutes,
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
        // Resolve canonical parent IDs once during preparation. Each track owns
        // exactly one default output; authored feeds remain separate edges.
        for index in 0..tracks.len() {
            let parent = match tracks[index].id {
                Some(id) => canonical
                    .track(id)
                    .ok_or(Invalid("canonical track is missing"))?
                    .parent_group(),
                None => None,
            };
            let target = parent
                .map(|id| {
                    tracks
                        .iter()
                        .position(|candidate| candidate.id == Some(id))
                        .ok_or(Invalid("group output target is missing"))
                })
                .transpose()?;
            let from = tracks[index].fader;
            let output = if let Some(target) = target {
                let input = builder.edges[tracks[target].source_input].1;
                TrackOutput::Group {
                    target,
                    edge: builder.edge(from, input),
                }
            } else {
                TrackOutput::Main {
                    edge: builder.edge(from, main),
                }
            };
            let track = &mut tracks[index];
            track.output = output;
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
        // The existing Main node remains the pre-Main summing/capture boundary.
        // Serial final-bus nodes extend only the existing content-origin plan.
        let mut main_devices = Vec::new();
        let mut previous_main = main;
        for device in project.main_device_chain.iter() {
            let node = builder.node(
                NodeId::Device(device.device_id),
                device.algorithmic_latency_frames(),
            );
            builder.edge(previous_main, node);
            main_devices.push(node);
            previous_main = node;
        }
        // Key edges participate in the same owner order and arrival plan as
        // audible inputs, but their PCM is delivered only to the detector.
        let mut authored_keys = Vec::new();
        authored_keys
            .try_reserve_exact(canonical.sidechains().len())
            .map_err(|_| Invalid("could not reserve compressor sidechains"))?;
        for route in canonical.sidechains() {
            let source_track = tracks
                .iter()
                .position(|track| track.id == Some(route.source))
                .ok_or(Invalid("compressor sidechain source was not prepared"))?;
            let destination = builder
                .nodes
                .iter()
                .position(|(id, _)| *id == NodeId::Device(route.device.value()))
                .ok_or(Invalid("compressor sidechain destination was not prepared"))?;
            let main_edge = builder
                .edges
                .iter()
                .position(|&(_, to)| to == destination)
                .ok_or(Invalid("compressor sidechain main edge was not prepared"))?;
            let key_edge = builder.edge(tracks[source_track].fader, destination);
            authored_keys.push((
                destination,
                KeyStage {
                    source_track,
                    key_edge,
                    main_edge,
                },
            ));
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
        let node_index = |old: usize| {
            plan.node_index(nodes[old].key)
                .ok_or(Invalid("signal graph node mapping is missing"))
        };
        let is_main_node = |index: usize| {
            main_devices
                .iter()
                .any(|old| nodes[*old].key == plan.nodes()[index].key)
        };
        // A Main Compressor's two stage inputs also need their existing
        // transactional delay histories, unlike ordinary serial Main edges.
        let main_key_edges: std::collections::BTreeSet<_> = authored_keys
            .iter()
            .filter(|(node, _)| main_devices.contains(node))
            .flat_map(|(_, stage)| [edge_keys[stage.key_edge], edge_keys[stage.main_edge]])
            .collect();
        let pre_main_edges: Vec<_> = plan
            .edges()
            .iter()
            .enumerate()
            .filter(|(_, edge)| {
                (!is_main_node(edge.from_index) && !is_main_node(edge.to_index))
                    || main_key_edges.contains(&edge.key)
            })
            .map(|(index, _)| index)
            .collect();
        let edge_index = |old: usize| {
            plan.edge_index(edge_keys[old])
                .and_then(|index| pre_main_edges.iter().position(|edge| *edge == index))
                .ok_or(Invalid("signal graph edge mapping is missing"))
        };
        let mut key_stages = Vec::new();
        if !authored_keys.is_empty() {
            key_stages
                .try_reserve_exact(plan.nodes().len())
                .map_err(|_| Invalid("could not reserve sidechain stage indices"))?;
            key_stages.resize(plan.nodes().len(), None);
            for (destination, stage) in authored_keys {
                key_stages[node_index(destination)?] = Some(KeyStage {
                    source_track: stage.source_track,
                    key_edge: edge_index(stage.key_edge)?,
                    main_edge: edge_index(stage.main_edge)?,
                });
            }
        }
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
            if let Some((processed, printed)) = track.printed_merge {
                track.printed_merge = Some((edge_index(processed)?, edge_index(printed)?));
            }
            match &mut track.output {
                TrackOutput::Main { edge } | TrackOutput::Group { edge, .. } => {
                    *edge = edge_index(*edge)?;
                }
            }
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
        for device in &mut main_devices {
            *device = node_index(*device)?;
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
        let has_group_outputs = tracks
            .iter()
            .any(|track| matches!(track.output, TrackOutput::Group { .. }));
        let mut track_indices = Vec::new();
        track_indices
            .try_reserve_exact(tracks.len())
            .map_err(|_| Invalid("could not reserve signal track index"))?;
        track_indices.extend(
            tracks
                .iter()
                .enumerate()
                .filter_map(|(index, track)| track.id.map(|id| (id, index))),
        );
        track_indices.sort_unstable_by_key(|(id, _)| *id);
        let graph = Self {
            track_indices,
            has_group_outputs,
            main: node_index(main)?,
            main_devices,
            pre_main_edges,
            key_stages,
            plan,
            tracks,
            returns,
            owner_order,
            typed: keys.into_keys().collect(),
        };
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
        Ok(graph)
    }
    pub(crate) fn table_bytes(&self) -> Option<usize> {
        size_of::<Self>()
            .checked_add(2 * size_of::<usize>())?
            .checked_add(self.nested_bytes()?)
    }
    pub(crate) fn nested_bytes(&self) -> Option<usize> {
        let Self {
            plan,
            tracks,
            returns,
            owner_order,
            track_indices,
            has_group_outputs: _,
            main: _,
            main_devices,
            pre_main_edges,
            key_stages,
            typed,
        } = self;
        let mut total = plan
            .storage_bytes()
            .ok()?
            .checked_add(typed.capacity().checked_mul(size_of::<NodeId>())?)?
            .checked_add(tracks.capacity().checked_mul(size_of::<TrackPlan>())?)?
            .checked_add(returns.capacity().checked_mul(size_of::<ReturnPlan>())?)?
            .checked_add(owner_order.capacity().checked_mul(size_of::<Owner>())?)?
            .checked_add(
                track_indices
                    .capacity()
                    .checked_mul(size_of::<(TrackId, usize)>())?,
            )?;
        total = total.checked_add(main_devices.capacity().checked_mul(size_of::<usize>())?)?;
        total = total.checked_add(pre_main_edges.capacity().checked_mul(size_of::<usize>())?)?;
        total = total.checked_add(
            key_stages
                .capacity()
                .checked_mul(size_of::<Option<KeyStage>>())?,
        )?;
        for track in tracks {
            total = total
                .checked_add(track.devices.capacity().checked_mul(size_of::<usize>())?)?
                .checked_add(track.sends.capacity().checked_mul(size_of::<Send>())?)?
                .checked_add(
                    track
                        .group_output_mutes
                        .capacity()
                        .checked_mul(size_of::<GroupOutputMuteRange>())?,
                )?
                .checked_add(
                    track
                        .audio_feeds
                        .capacity()
                        .checked_mul(size_of::<Send>())?,
                )?;
        }
        for bus in returns {
            total = total
                .checked_add(bus.devices.capacity().checked_mul(size_of::<usize>())?)?
                .checked_add(bus.feeds.capacity().checked_mul(size_of::<Send>())?)?
                .checked_add(bus.audio_feeds.capacity().checked_mul(size_of::<Send>())?)?;
        }
        Some(total)
    }
    /// Retain immutable topology even at zero arrival; runtime allocation and
    /// compensated execution follow the actual latency/typed-routing requirement.
    pub(crate) fn needs_runtime(&self) -> bool {
        self.plan.max_latency_frames() != 0
            || !self.key_stages.is_empty()
            || self.has_group_outputs
            || self.tracks.iter().any(|track| {
                !track.audio_feeds.is_empty()
                    || track.printed_merge.is_some()
                    || !track.group_output_mutes.is_empty()
            })
            || self.returns.iter().any(|bus| !bus.audio_feeds.is_empty())
    }
    /// Main's input arrival: the frame at which every aligned path is summed.
    pub(crate) fn main_input_arrival(&self) -> u32 {
        self.plan.nodes()[self.main].input_arrival_frames
    }
    /// A detached copy of `track`'s instrument suffix, rendered from the
    /// source origin and summed straight into Main's input, lands on Main's
    /// clock (Pulse #4188). That holds when the instrument output arrives at
    /// the origin and the track's own edge to Main carries no compensation
    /// delay, so the suffix latency alone reaches Main and that edge buffers
    /// none of the track's PCM.
    pub(crate) fn detached_suffix_reaches_main_aligned(&self, track: usize) -> bool {
        let Some(plan) = self.tracks.get(track) else {
            return false;
        };
        let TrackOutput::Main { edge } = plan.output else {
            return false;
        };
        let generator_at_origin = plan
            .generator
            .and_then(|node| self.plan.nodes().get(node))
            .is_some_and(|node| node.output_arrival_frames == 0);
        let undelayed = self
            .pre_main_edges
            .get(edge)
            .and_then(|index| self.plan.edges().get(*index))
            .is_some_and(|edge| edge.delay_frames == 0);
        generator_at_origin && undelayed
    }
    pub(crate) fn has_group_outputs(&self) -> bool {
        self.has_group_outputs
    }
    /// Stable ID lookup is prepared off callback; the returned slot belongs to
    /// this exact immutable graph and its matching Arrangement track inventory.
    pub(crate) fn track_index(&self, id: TrackId) -> Option<usize> {
        self.track_indices
            .binary_search_by_key(&id, |(track, _)| *track)
            .ok()
            .and_then(|index| self.track_indices.get(index).map(|(_, slot)| *slot))
    }
    /// Retained post-fader tails may enter a successor graph only while every
    /// destination on their original default path still names the same owner.
    pub(crate) fn same_default_path(&self, other: &Self, mut id: TrackId) -> bool {
        for _ in 0..=self.tracks.len() {
            let Some(a) = self
                .track_index(id)
                .and_then(|index| self.tracks.get(index))
            else {
                return false;
            };
            let Some(b) = other
                .track_index(id)
                .and_then(|index| other.tracks.get(index))
            else {
                return false;
            };
            let parent = |graph: &Self, output: TrackOutput| match output {
                TrackOutput::Main { .. } => Some(None),
                TrackOutput::Group { target, .. } => graph
                    .tracks
                    .get(target)
                    .and_then(|track| track.id)
                    .map(Some),
            };
            let Some(next) = parent(self, a.output) else {
                return false;
            };
            if Some(next) != parent(other, b.output) {
                return false;
            }
            match next {
                Some(parent) => id = parent,
                None => return true,
            }
        }
        false
    }
    /// An in-flight child tail cannot be fed into a replacement ancestor
    /// processor chain after its predecessor has been detached. Require the
    /// actual prepared ancestor chains to remain identical before transfer.
    pub(crate) fn same_tail_path(
        &self,
        other: &Self,
        old: &Project,
        new: &Project,
        mut id: TrackId,
    ) -> bool {
        if !self.same_default_path(other, id) {
            return false;
        }
        for _ in 0..self.tracks.len() {
            let Some(plan) = self
                .track_index(id)
                .and_then(|index| self.tracks.get(index))
            else {
                return false;
            };
            let TrackOutput::Group { target, .. } = plan.output else {
                return true;
            };
            let Some(parent) = self.tracks.get(target).and_then(|track| track.id) else {
                return false;
            };
            let Some(a) = self
                .track_index(parent)
                .and_then(|index| old.tracks.get(index))
            else {
                return false;
            };
            let Some(b) = other
                .track_index(parent)
                .and_then(|index| new.tracks.get(index))
            else {
                return false;
            };
            if a.device_chain.len() != b.device_chain.len()
                || !a
                    .device_chain
                    .iter()
                    .zip(b.device_chain.iter())
                    .all(|(a, b)| {
                        // Foreign binding metadata alone does not prove an
                        // endpoint lease. Let the predecessor drain instead.
                        !matches!(
                            a.definition,
                            Some(hex_lattice::PreparedProcessorFactory::Foreign(_))
                        ) && !matches!(
                            b.definition,
                            Some(hex_lattice::PreparedProcessorFactory::Foreign(_))
                        ) && a.device_id == b.device_id
                            && a.device_type == b.device_type
                            && a.bypassed == b.bypassed
                            && a.matrix == b.matrix
                            && a.configuration == b.configuration
                            && a.automation == b.automation
                    })
            {
                return false;
            }
            id = parent;
        }
        false
    }
    /// The closed streamed constructor may enumerate zero-latency group routes
    /// using the same source frontier as its per-frame render. Authored feeds
    /// remain outside that constructor's admitted graph class.
    pub(crate) fn supports_closed_group_stream(&self) -> bool {
        self.plan.max_latency_frames() == 0
            && self.has_group_outputs
            && self
                .tracks
                .iter()
                .all(|track| track.audio_feeds.is_empty() && track.printed_merge.is_none())
            && self.returns.iter().all(|bus| bus.audio_feeds.is_empty())
    }
    /// Serial Main latency needs clock history, but does not require rendering
    /// all upstream sources and effects one sample at a time. Typed audio feeds
    /// and pre-Main latency still use the general compensated routing kernel.
    pub(crate) fn needs_upstream_frame_routing(&self) -> bool {
        self.main_latency() != 0
            || !self.key_stages.is_empty()
            || self.has_group_outputs
            || self
                .tracks
                .iter()
                .any(|track| !track.audio_feeds.is_empty() || track.printed_merge.is_some())
            || self.returns.iter().any(|bus| !bus.audio_feeds.is_empty())
    }
    /// A flat graph has no consumer of a track's unaligned fader output.
    /// Its independent Main alignment edges can advance in a different owner
    /// order as long as each edge preserves its own frame order.
    pub(crate) fn independent_main_routes(&self) -> bool {
        !self.has_group_outputs
            && self.key_stages.is_empty()
            && self.returns.is_empty()
            && self.tracks.iter().all(|track| track.audio_feeds.is_empty())
    }
    pub(crate) fn resident_main_alignment(&self, index: usize) -> Option<(usize, u32)> {
        let track = self.tracks.get(index)?;
        let TrackOutput::Main { edge } = track.output else {
            return None;
        };
        (track.generator.is_none()
            && track.printed_merge.is_none()
            && track.group_output_mutes.is_empty()
            && self.plan.edges()[self.pre_main_edges[track.source_input]].delay_frames == 0
            && self.plan.edges()[self.pre_main_edges[track.audio_merge]].delay_frames == 0)
            .then_some((edge, self.plan.nodes()[track.fader].output_arrival_frames))
    }
    /// With no pre-Main latency or typed feeds, these graphs retain transport
    /// stamps only: every pre-Main edge has zero delay and owns no pending PCM.
    /// Inserting/removing zero-latency effects may change node/edge indices, but
    /// cannot invalidate those stamps. DSP history still follows device owners
    /// in the Session handoff; this is only compensation-history compatibility.
    /// Independent owners may also acquire different topological ranks when a
    /// serial node is added. Compare semantic routing, not those generated ranks.
    fn clock_only_compatible(&self, other: &Self) -> bool {
        !self.needs_upstream_frame_routing()
            && !other.needs_upstream_frame_routing()
            && self.same_routing(other)
    }
    /// Identify exactly one removed send in an otherwise unchanged flat graph.
    /// Both graphs have zero-delay upstream edges, so the removed edge has no
    /// queued samples; its future input is deliberately disconnected.
    fn removed_flat_send_from(&self, old: &Self) -> Option<ReturnId> {
        if self.needs_upstream_frame_routing()
            || old.needs_upstream_frame_routing()
            || self.typed != old.typed
            || self.key_stages != old.key_stages
            || self.tracks.len() != old.tracks.len()
            || self.returns.len() != old.returns.len()
            || self
                .returns
                .iter()
                .zip(&old.returns)
                .any(|(a, b)| a.id != b.id || !a.feeds.is_empty() || !b.feeds.is_empty())
        {
            return None;
        }
        let mut removed = None;
        for (new, old_track) in self.tracks.iter().zip(&old.tracks) {
            if new.id != old_track.id
                || !matches!(new.output, TrackOutput::Main { .. })
                || !matches!(old_track.output, TrackOutput::Main { .. })
            {
                return None;
            }
            let mut old_sends = old_track.sends.iter();
            for send in &new.sends {
                let mut prior = old_sends.next()?;
                if prior.target != send.target {
                    if removed.replace((prior.target, prior.edge)).is_some() {
                        return None;
                    }
                    prior = old_sends.next()?;
                }
                if prior.target != send.target || prior.gain != send.gain {
                    return None;
                }
            }
            for prior in old_sends {
                if removed.replace((prior.target, prior.edge)).is_some() {
                    return None;
                }
            }
        }
        let (target, removed_edge) = removed?;
        if old.pre_main_edges.len() != self.pre_main_edges.len().checked_add(1)?
            || !self
                .pre_main_edges
                .iter()
                .enumerate()
                .map(|(i, _)| self.pre_main_endpoints(i))
                .eq(old
                    .pre_main_edges
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != removed_edge)
                    .map(|(i, _)| old.pre_main_endpoints(i)))
        {
            return None;
        }
        old.returns.get(target).map(|bus| bus.id)
    }
    /// A first compensated epoch may start with empty rings only when the old
    /// graph owns no delay history and semantic routes have not changed. DSP
    /// quiescence and source/clock continuity are independently checked by Session.
    pub(crate) fn can_start_compensation_from(&self, other: &Self) -> bool {
        !other.needs_runtime()
            && self.tracks.iter().all(|track| {
                matches!(track.output, TrackOutput::Main { .. }) && track.audio_feeds.is_empty()
            })
            && self.returns.iter().all(|bus| bus.audio_feeds.is_empty())
            && self.same_routing(other)
    }
    /// Existing output may enter fresh delay rings unchanged. Only processors
    /// moved behind a new latency (or removed from this owner) need to be quiet:
    /// their old residual output has no corresponding new origin during fill.
    pub(crate) fn can_start_chain_from(
        &self,
        nodes: &[usize],
        old: &Self,
        old_nodes: &[usize],
        chain: &hex_lattice::NativeRuntimeChain,
    ) -> bool {
        if chain.is_empty() {
            return true; // Fused stateless matrices own no residual output.
        }
        old_nodes.len() == chain.len()
            && old_nodes.iter().enumerate().all(|(index, &old_node)| {
                let unchanged_origin = old
                    .typed
                    .get(old_node)
                    .and_then(|id| self.typed.binary_search(id).ok())
                    .filter(|node| nodes.contains(node))
                    .and_then(|node| self.plan.nodes().get(node))
                    .is_some_and(|node| {
                        node.input_arrival_frames == 0 && node.output_arrival_frames == 0
                    });
                unchanged_origin || chain.committed_range_is_quiescent(index..index + 1)
            })
    }
    fn same_routing(&self, other: &Self) -> bool {
        self.key_stages == other.key_stages
            && self.tracks.len() == other.tracks.len()
            && self.returns.len() == other.returns.len()
            && self.tracks.iter().zip(&other.tracks).all(|(a, b)| {
                a.id == b.id
                    && std::mem::discriminant(&a.output) == std::mem::discriminant(&b.output)
                    && match (a.output, b.output) {
                        (
                            TrackOutput::Group { target: a, .. },
                            TrackOutput::Group { target: b, .. },
                        ) => a == b,
                        _ => true,
                    }
                    && a.sends
                        .iter()
                        .map(|send| send.target)
                        .eq(b.sends.iter().map(|send| send.target))
            })
            && self.returns.iter().zip(&other.returns).all(|(a, b)| {
                a.id == b.id
                    && a.feeds
                        .iter()
                        .map(|send| send.target)
                        .eq(b.feeds.iter().map(|send| send.target))
            })
    }
    fn history_compatible(&self, other: &Self) -> bool {
        self.clock_only_compatible(other) || self.pre_main_compatible(other)
    }
    pub(crate) fn pre_main_compatible(&self, other: &Self) -> bool {
        if self.key_stages != other.key_stages
            || self.tracks.len() != other.tracks.len()
            || self.returns.len() != other.returns.len()
            || self.owner_order != other.owner_order
            || self.pre_main_edges.len() != other.pre_main_edges.len()
        {
            return false;
        }
        // Gain changes affect newly entering samples, not topology or the
        // identity of pending PCM already held in an unchanged delay edge.
        let same_edges = |a: &[Send], b: &[Send]| {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|(a, b)| (a.target, a.edge) == (b.target, b.edge))
        };
        if !self.tracks.iter().zip(&other.tracks).all(|(a, b)| {
            a.id == b.id
                && a.output == b.output
                && same_edges(&a.sends, &b.sends)
                && same_edges(&a.audio_feeds, &b.audio_feeds)
        }) || !self.returns.iter().zip(&other.returns).all(|(a, b)| {
            a.id == b.id
                && same_edges(&a.feeds, &b.feeds)
                && same_edges(&a.audio_feeds, &b.audio_feeds)
        }) {
            return false;
        }
        let current = self
            .typed
            .iter()
            .enumerate()
            .filter(|(i, _)| !self.main_devices.contains(i));
        let previous = other
            .typed
            .iter()
            .enumerate()
            .filter(|(i, _)| !other.main_devices.contains(i));
        if !current
            .clone()
            .map(|(_, id)| id)
            .eq(previous.clone().map(|(_, id)| id))
        {
            return false;
        }
        if !current.zip(previous).all(|((i, _), (j, _))| {
            let (a, b) = (self.plan.nodes()[i], other.plan.nodes()[j]);
            (
                a.latency_frames,
                a.input_arrival_frames,
                a.output_arrival_frames,
            ) == (
                b.latency_frames,
                b.input_arrival_frames,
                b.output_arrival_frames,
            )
        }) {
            return false;
        }
        self.pre_main_edges
            .iter()
            .zip(&other.pre_main_edges)
            .all(|(i, j)| {
                let (a, b) = (self.plan.edges()[*i], other.plan.edges()[*j]);
                (
                    self.typed[a.from_index],
                    self.typed[a.to_index],
                    a.delay_frames,
                ) == (
                    other.typed[b.from_index],
                    other.typed[b.to_index],
                    b.delay_frames,
                )
            })
    }
    /// Typed endpoints of the pre-Main edge at `position`.
    fn pre_main_endpoints(&self, position: usize) -> Option<(NodeId, NodeId)> {
        let edge = self.plan.edges().get(*self.pre_main_edges.get(position)?)?;
        Some((
            *self.typed.get(edge.from_index)?,
            *self.typed.get(edge.to_index)?,
        ))
    }
    /// Visit the pre-Main edges of `self` and `other` that join the same typed
    /// endpoints, as `(self plan edge, other plan edge)`. Both lists are sorted
    /// by typed endpoints (stable keys follow `NodeId` order), so this is one
    /// bounded merge walk. An edge present on only one side must carry no
    /// delay, so it holds no in-flight PCM; a paired edge must keep its delay.
    /// Returns false on any other mismatch.
    fn pair_retained_pre_main_edges(
        &self,
        other: &Self,
        fresh: Option<FreshHistory>,
        mut visit: impl FnMut(usize, usize),
    ) -> bool {
        let key = |graph: &Self, position: usize| {
            graph
                .pre_main_endpoints(position)
                .zip(graph.pre_main_edges.get(position).copied())
        };
        let delay = |graph: &Self, index: usize| graph.plan.edges()[index].delay_frames;
        let (mut a, mut b) = (0, 0);
        loop {
            if fresh.is_some_and(|owner| self.pre_main_edge_touches_owner(a, owner)) {
                a += 1;
                continue;
            }
            if fresh.is_some_and(|owner| other.pre_main_edge_touches_owner(b, owner)) {
                b += 1;
                continue;
            }
            match (key(self, a), key(other, b)) {
                (None, None) => {
                    return a == self.pre_main_edges.len() && b == other.pre_main_edges.len();
                }
                (Some((ka, ia)), Some((kb, ib))) if ka == kb => {
                    if delay(self, ia) != delay(other, ib) {
                        return false;
                    }
                    visit(ia, ib);
                    a += 1;
                    b += 1;
                }
                (Some((ka, ia)), kb) if kb.is_none_or(|(kb, _)| ka < kb) => {
                    if delay(self, ia) != 0 {
                        return false;
                    }
                    a += 1;
                }
                (_, Some((_, ib))) => {
                    if delay(other, ib) != 0 {
                        return false;
                    }
                    b += 1;
                }
                (Some(_), None) => return false,
            }
        }
    }
    fn pre_main_edge_touches_owner(&self, position: usize, owner: FreshHistory) -> bool {
        self.pre_main_endpoints(position)
            .is_some_and(|(from, to)| owner.contains(from) || owner.contains(to))
    }

    /// Only local source/fader edges and this track's direct contribution to
    /// Main can start with empty history. A group, sidechain or feed would
    /// also change another owner's input and needs the ordinary proof.
    fn is_isolated_main_track(&self, id: TrackId) -> bool {
        if let Some(track) = self.tracks.iter().find(|track| track.id == Some(id))
            && (!track.devices.is_empty() || !matches!(track.output, TrackOutput::Main { .. }))
        {
            return false;
        }
        self.pre_main_edges.iter().enumerate().all(|(index, _)| {
            let Some((from, to)) = self.pre_main_endpoints(index) else {
                return false;
            };
            match (from.belongs_to_track(id), to.belongs_to_track(id)) {
                (true, false) => from == NodeId::Fader(id.value()) && to == NodeId::Main,
                (false, true) => false,
                _ => true,
            }
        })
    }

    /// A changed track may own local device nodes, but no edge may feed
    /// another owner except its fader's direct Main contribution.
    fn is_isolated_changed_track(&self, id: TrackId, device: u64) -> bool {
        let owner = FreshHistory::ChangedTrack(id, device);
        self.tracks
            .iter()
            .any(|track| track.id == Some(id) && matches!(track.output, TrackOutput::Main { .. }))
            && self.pre_main_edges.iter().enumerate().all(|(index, _)| {
                let Some((from, to)) = self.pre_main_endpoints(index) else {
                    return false;
                };
                match (owner.contains(from), owner.contains(to)) {
                    (true, false) => from == NodeId::Fader(id.value()) && to == NodeId::Main,
                    (false, true) => false,
                    _ => true,
                }
            })
    }
    /// One new empty Return may begin with silent history only when retained
    /// Returns keep their order and no edge feeds it or routes it beyond Main.
    pub(crate) fn added_isolated_return_from(&self, other: &Self) -> Option<ReturnId> {
        if self.returns.len() != other.returns.len().checked_add(1)? {
            return None;
        }
        let index = self
            .returns
            .iter()
            .zip(&other.returns)
            .position(|(candidate, old)| candidate.id != old.id)
            .unwrap_or(other.returns.len());
        if !self
            .returns
            .iter()
            .skip(index + 1)
            .map(|bus| bus.id)
            .eq(other.returns.iter().skip(index).map(|bus| bus.id))
        {
            return None;
        }
        let bus = self.returns.get(index)?;
        if !bus.devices.is_empty() || !bus.feeds.is_empty() || !bus.audio_feeds.is_empty() {
            return None;
        }
        let input = NodeId::ReturnInput(bus.id.value());
        let output = NodeId::ReturnOutput(bus.id.value());
        self.pre_main_edges
            .iter()
            .enumerate()
            .all(|(index, _)| {
                let Some((from, to)) = self.pre_main_endpoints(index) else {
                    return false;
                };
                match (from == input || from == output, to == input || to == output) {
                    (true, true) => from == input && to == output,
                    (true, false) => from == output && to == NodeId::Main,
                    (false, true) => false,
                    (false, false) => true,
                }
            })
            .then_some(bus.id)
    }

    /// Relaxed `pre_main_compatible` for copying delay history into a fresh
    /// graph (Pulse #4188). Owners, routing targets and every routed edge's
    /// typed endpoints are unchanged, and nodes on both sides keep their actual
    /// latency and arrivals. Edge positions may shift: a zero-latency node with
    /// only zero-delay edges (for example an unbound source's generator) may
    /// appear or disappear. This can also change the order of independent
    /// owners. Each graph uses its own topological order, while history follows
    /// the unchanged typed edges. History must be copied through
    /// `pair_retained_pre_main_edges`, never by position.
    fn pre_main_history_maps_from(&self, other: &Self) -> bool {
        self.retained_pre_main_history_maps_from(other, None)
    }
    /// A changed muted track is excluded only after the handoff owner proves
    /// its narrow source contract. Its isolated local edges still require
    /// silent history in both runtimes before copying any retained edges.
    fn retained_pre_main_history_maps_from(
        &self,
        other: &Self,
        fresh: Option<FreshHistory>,
    ) -> bool {
        let owner_key = |graph: &Self, owner| match owner {
            Owner::Track(index) => graph.tracks[index].id.map(|id| (false, id.value())),
            Owner::Return(index) => Some((true, graph.returns[index].id.value())),
        };
        let retained_order = |graph: &Self, owner| {
            let key = owner_key(graph, owner);
            fresh
                .is_none_or(|owner| {
                    key != Some(match owner {
                        FreshHistory::Track(id) | FreshHistory::ChangedTrack(id, _) => {
                            (false, id.value())
                        }
                        FreshHistory::Return(id) => (true, id.value()),
                    })
                })
                .then_some(key)
        };
        if self.key_stages != other.key_stages
            || match fresh {
                Some(FreshHistory::Track(_) | FreshHistory::ChangedTrack(_, _)) | None => {
                    self.returns.len() != other.returns.len()
                }
                Some(FreshHistory::Return(id)) => {
                    self.added_isolated_return_from(other) != Some(id)
                        && other.added_isolated_return_from(self) != Some(id)
                }
            }
            || if let Some(owner) = fresh {
                self.main_latency() != other.main_latency()
                    || match owner {
                        FreshHistory::Track(track) => {
                            !self.is_isolated_main_track(track)
                                || !other.is_isolated_main_track(track)
                        }
                        FreshHistory::ChangedTrack(track, device) => {
                            self.tracks.len() != other.tracks.len()
                                || !self.independent_main_routes()
                                || !other.independent_main_routes()
                                || self
                                    .tracks
                                    .iter()
                                    .any(|track| track.printed_merge.is_some())
                                || other
                                    .tracks
                                    .iter()
                                    .any(|track| track.printed_merge.is_some())
                                || !self.is_isolated_changed_track(track, device)
                                || !other.is_isolated_changed_track(track, device)
                        }
                        FreshHistory::Return(_) => self.tracks.len() != other.tracks.len(),
                    }
                    || !self
                        .owner_order
                        .iter()
                        .filter_map(|&owner| retained_order(self, owner))
                        .eq(other
                            .owner_order
                            .iter()
                            .filter_map(|&owner| retained_order(other, owner)))
            } else {
                self.tracks.len() != other.tracks.len()
            }
        {
            return false;
        }
        let same_edge = |a: usize, b: usize| {
            self.pre_main_endpoints(a)
                .is_some_and(|ends| other.pre_main_endpoints(b) == Some(ends))
        };
        let same_sends = |a: &[Send], b: &[Send]| {
            a.len() == b.len()
                && a.iter().zip(b).all(|(a, b)| {
                    // Return insertion/removal shifts runtime indices;
                    // the typed endpoints still identify the same route.
                    (matches!(fresh, Some(FreshHistory::Return(_))) || a.target == b.target)
                        && same_edge(a.edge, b.edge)
                })
        };
        let same_output = |a: TrackOutput, b: TrackOutput| match (a, b) {
            (TrackOutput::Main { edge: a }, TrackOutput::Main { edge: b }) => same_edge(a, b),
            (
                TrackOutput::Group {
                    target: ta,
                    edge: a,
                },
                TrackOutput::Group {
                    target: tb,
                    edge: b,
                },
            ) => ta == tb && same_edge(a, b),
            _ => false,
        };
        let retained = |track: &&TrackPlan| !matches!(fresh, Some(FreshHistory::Track(id) | FreshHistory::ChangedTrack(id, _)) if track.id == Some(id));
        let retained_return =
            |bus: &&ReturnPlan| !matches!(fresh, Some(FreshHistory::Return(id)) if bus.id == id);
        if !self
            .tracks
            .iter()
            .filter(retained)
            .zip(other.tracks.iter().filter(retained))
            .all(|(a, b)| {
                a.id == b.id
                    && same_output(a.output, b.output)
                    && same_sends(&a.sends, &b.sends)
                    && same_sends(&a.audio_feeds, &b.audio_feeds)
            })
            || !self
                .returns
                .iter()
                .filter(retained_return)
                .zip(other.returns.iter().filter(retained_return))
                .all(|(a, b)| {
                    a.id == b.id
                        && same_edge(a.main, b.main)
                        && same_sends(&a.feeds, &b.feeds)
                        && same_sends(&a.audio_feeds, &b.audio_feeds)
                })
        {
            return false;
        }
        // Non-Main nodes, merged by sorted typed identity.
        let node = |graph: &Self, index: usize| {
            (index < graph.typed.len() && !graph.main_devices.contains(&index))
                .then(|| (graph.typed[index], graph.plan.nodes()[index]))
        };
        let (mut i, mut j) = (0, 0);
        while i < self.typed.len() || j < other.typed.len() {
            if i < self.typed.len()
                && (self.main_devices.contains(&i)
                    || fresh.is_some_and(|owner| owner.contains(self.typed[i])))
            {
                i += 1;
                continue;
            }
            if j < other.typed.len()
                && (other.main_devices.contains(&j)
                    || fresh.is_some_and(|owner| owner.contains(other.typed[j])))
            {
                j += 1;
                continue;
            }
            match (node(self, i), node(other, j)) {
                (Some((a, na)), Some((b, nb))) if a == b => {
                    if (
                        na.latency_frames,
                        na.input_arrival_frames,
                        na.output_arrival_frames,
                    ) != (
                        nb.latency_frames,
                        nb.input_arrival_frames,
                        nb.output_arrival_frames,
                    ) {
                        return false;
                    }
                    i += 1;
                    j += 1;
                }
                (Some((a, na)), b) if b.is_none_or(|(b, _)| a < b) => {
                    if na.latency_frames != 0 {
                        return false;
                    }
                    i += 1;
                }
                (_, Some((_, nb))) => {
                    if nb.latency_frames != 0 {
                        return false;
                    }
                    j += 1;
                }
                (Some(_), None) | (None, None) => return false,
            }
        }
        self.pair_retained_pre_main_edges(other, fresh, |_, _| {})
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
                    .checked_mul(2 + usize::from(!self.key_stages.is_empty()))?,
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

    /// Longest declared residual tail feeding Main, or the selected pre-Main
    /// capture edges. This is a control-side query over the prepared DAG.
    pub(crate) fn output_tail_frames(
        &self,
        project: &Project,
        captures: Option<&[crate::OutputCaptureId]>,
    ) -> Result<u64, Invalid> {
        use hex_lattice::{OutputTail, PreparedProcessorFactory, PreparedRuntimeInstance};

        let mut tails = Vec::new();
        tails
            .try_reserve_exact(self.plan.nodes().len())
            .map_err(|_| Invalid("could not reserve output tail plan"))?;
        tails.resize(self.plan.nodes().len(), Some(0_u64));
        let declared_tail = |device: &PreparedRuntimeInstance| {
            if device.bypassed {
                return Some(0);
            }
            match device.definition.as_ref().map_or(
                OutputTail::Finite { max_tail_frames: 0 },
                PreparedProcessorFactory::output_tail,
            ) {
                OutputTail::Finite { max_tail_frames } => Some(max_tail_frames),
                OutputTail::Unbounded => None,
            }
        };
        let mut assign = |indices: &[usize], chain: &[PreparedRuntimeInstance]| {
            if indices.len() != chain.len() {
                return Err(Invalid("output tail graph does not match device chain"));
            }
            for (&index, device) in indices.iter().zip(chain) {
                *tails
                    .get_mut(index)
                    .ok_or(Invalid("output tail node is missing"))? = declared_tail(device);
            }
            Ok(())
        };
        if self.tracks.len() != project.tracks.len()
            || self.returns.len() != project.return_buses.len()
        {
            return Err(Invalid("output tail graph does not match project"));
        }
        for (plan, track) in self.tracks.iter().zip(&project.tracks) {
            assign(&plan.devices, &track.device_chain)?;
        }
        for (plan, bus) in self.returns.iter().zip(&project.return_buses) {
            assign(&plan.devices, &bus.device_chain)?;
        }
        assign(&self.main_devices, &project.main_device_chain)?;
        for recipe in &project.source_effects {
            let generator = self
                .tracks
                .get(recipe.receiver)
                .and_then(|track| track.generator)
                .ok_or(Invalid("source effect tail has no prepared generator"))?;
            let tail = recipe
                .instances
                .iter()
                .try_fold(0_u64, |sum, device| sum.checked_add(declared_tail(device)?));
            *tails
                .get_mut(generator)
                .ok_or(Invalid("source effect tail node is missing"))? = tail;
        }

        // None means an unbounded or overflowing path. Keeping it in the DAG
        // allows an unrelated selected capture to remain independently valid.
        for &node in self.plan.topological_order() {
            let mut incoming = Some(0_u64);
            for edge in self
                .plan
                .edges()
                .iter()
                .filter(|edge| edge.to_index == node)
            {
                incoming = match (incoming, tails[edge.from_index]) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    _ => None,
                };
            }
            tails[node] =
                incoming.and_then(|frames| tails[node].and_then(|tail| frames.checked_add(tail)));
        }
        let mut maximum = 0_u64;
        if let Some(captures) = captures {
            for capture in captures {
                let edge = match *capture {
                    crate::OutputCaptureId::Track(id) => {
                        let plan = self
                            .tracks
                            .iter()
                            .find(|track| track.id == Some(id))
                            .ok_or(Invalid("capture track is missing"))?;
                        match plan.output {
                            TrackOutput::Main { edge } | TrackOutput::Group { edge, .. } => edge,
                        }
                    }
                    crate::OutputCaptureId::Return(id) => {
                        self.returns
                            .iter()
                            .find(|bus| bus.id == id)
                            .ok_or(Invalid("capture return is missing"))?
                            .main
                    }
                };
                let planned = *self
                    .pre_main_edges
                    .get(edge)
                    .ok_or(Invalid("capture edge is missing"))?;
                let node = self
                    .plan
                    .edges()
                    .get(planned)
                    .ok_or(Invalid("capture edge is missing"))?
                    .from_index;
                maximum = maximum.max(
                    tails[node]
                        .ok_or(Invalid("automatic output tail is unbounded or overflows"))?,
                );
            }
        } else {
            let node = self.main_devices.last().copied().unwrap_or(self.main);
            maximum =
                tails[node].ok_or(Invalid("automatic output tail is unbounded or overflows"))?;
        }
        Ok(maximum)
    }
}

#[derive(Debug)]
pub(crate) struct SignalRuntime {
    pub graph: Arc<SignalGraph>,
    delay_graph: Arc<SignalGraph>,
    pub delays: CompensationRuntime,
    pub inputs: Vec<Frame>,
    pub main: Vec<Frame>,
    key_taps: Vec<Frame>,
}
impl SignalRuntime {
    /// With no graph-fed input or delay at this track, its resident source can
    /// pass straight to the fader. Main and sends still advance in graph order.
    pub(crate) fn resident_return_track_is_passthrough(&self, index: usize) -> bool {
        if self.graph.returns.is_empty()
            || self.graph.has_group_outputs
            || !self.graph.key_stages.is_empty()
            || self
                .graph
                .tracks
                .iter()
                .any(|track| !track.audio_feeds.is_empty())
            || self
                .graph
                .returns
                .iter()
                .any(|bus| !bus.audio_feeds.is_empty())
        {
            return false;
        }
        let Some(track) = self.graph.tracks.get(index) else {
            return false;
        };
        let zero_edge = |edge: usize| {
            self.delay_graph
                .pre_main_edges
                .get(edge)
                .and_then(|&mapped| self.delay_graph.plan.edges().get(mapped))
                .is_some_and(|edge| edge.delay_frames == 0)
        };
        matches!(track.output, TrackOutput::Main { .. })
            && track.devices.is_empty()
            && track.generator.is_none()
            && track.printed_merge.is_none()
            && track.group_output_mutes.is_empty()
            && self.graph.plan.nodes()[track.fader].output_arrival_frames == 0
            && zero_edge(track.source_input)
            && zero_edge(track.audio_merge)
    }
    pub(crate) fn new(graph: Arc<SignalGraph>) -> Result<Self, Invalid> {
        let scratch_bytes = graph
            .tracks
            .len()
            .checked_mul(size_of::<Frame>())
            .and_then(|n| n.checked_mul(2 + usize::from(!graph.key_stages.is_empty())))
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
        let mut key_taps = Vec::new();
        if !graph.key_stages.is_empty() {
            key_taps
                .try_reserve_exact(graph.tracks.len())
                .map_err(|_| Invalid("could not reserve sidechain tap scratch"))?;
            key_taps.resize(graph.tracks.len(), [0.0; 2]);
        }
        let runtime = Self {
            key_taps,
            delay_graph: Arc::clone(&graph),
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
    #[cfg(test)]
    pub(crate) fn test_input_address(&self) -> usize {
        self.inputs.as_ptr() as usize
    }
    pub(crate) fn storage_bytes(&self) -> Option<usize> {
        self.storage_bytes_with_graph(&self.graph)
    }
    pub(crate) fn visit_fresh_candidate_storage<'a>(
        &'a self,
        graph: &'a Arc<SignalGraph>,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        use crate::audio_storage::{Location, Owner};
        // A fresh runtime has both original fields pointing at this actual
        // constructor graph. Later history rebinds retain separate origins.
        if !Arc::ptr_eq(graph, &self.graph) || !Arc::ptr_eq(graph, &self.delay_graph) {
            return Err(crate::AudioStorageError::SourceMismatch);
        }
        ledger.existing_shared(
            &self.graph,
            Owner::exclusive(self),
            Location("compensation-graph", 0),
        )?;
        ledger.existing_shared(
            &self.delay_graph,
            Owner::exclusive(self),
            Location("compensation-delay-graph", 0),
        )?;
        ledger.vec(&self.inputs)?;
        ledger.vec(&self.main)?;
        ledger.vec(&self.key_taps)?;
        self.delays.visit_owned_candidate_storage(ledger)
    }
    pub(crate) fn can_continue_pre_main(&self, other: &Self) -> bool {
        (self.graph.history_compatible(&other.graph)
            || self.graph.pre_main_history_maps_from(&other.graph))
            && self.delays.is_closed()
            && other.delays.is_closed()
            && self.delay_graph.history_compatible(&self.graph)
            && other.delay_graph.history_compatible(&other.graph)
    }
    pub(crate) fn horizon(&self) -> u32 {
        self.delays.history_horizon()
    }
    pub(crate) fn can_copy_from(&self, other: &Self, fresh: Option<FreshHistory>) -> bool {
        let compatible = if let Some(owner) = fresh {
            let silent_owner =
                |runtime: &Self| {
                    runtime.delay_graph.pre_main_edges.iter().enumerate().all(
                        |(position, &edge)| {
                            !runtime
                                .delay_graph
                                .pre_main_edge_touches_owner(position, owner)
                                || runtime.delays.edge_history_is_silent(edge)
                        },
                    )
                };
            self.graph
                .retained_pre_main_history_maps_from(&other.graph, Some(owner))
                && self.delays.is_closed()
                && other.delays.is_closed()
                && self.delay_graph.history_compatible(&self.graph)
                && other.delay_graph.history_compatible(&other.graph)
                && silent_owner(self)
                && silent_owner(other)
        } else {
            self.can_continue_pre_main(other)
        };
        compatible
            && self.horizon() >= other.horizon()
            && ((fresh.is_none() && self.delay_graph.clock_only_compatible(&other.delay_graph))
                || {
                    let mut copyable = true;
                    let mapped = self.delay_graph.pair_retained_pre_main_edges(
                        &other.delay_graph,
                        fresh,
                        |to, from| {
                            copyable &=
                                self.delays
                                    .can_copy_edge_history_from(to, &other.delays, from);
                        },
                    );
                    mapped && copyable
                })
    }
    /// A fresh candidate can retain only the transport stamps when one
    /// zero-delay send disappears. No donor delay storage is rebound.
    pub(crate) fn removed_send_clock_from(&self, other: &Self) -> Option<ReturnId> {
        if !Arc::ptr_eq(&self.graph, &self.delay_graph)
            || !self.delays.is_closed()
            || !other.delays.is_closed()
            || !other.delay_graph.clock_only_compatible(&other.graph)
            || self.horizon() < other.horizon()
        {
            return None;
        }
        self.graph.removed_flat_send_from(&other.graph)
    }
    pub(crate) fn copy_removed_send_clock_from(&mut self, other: &Self) -> Result<(), Invalid> {
        if self.removed_send_clock_from(other).is_none() {
            return Err(Invalid("removed send clock is incompatible"));
        }
        self.delays
            .copy_stamp_history_from(&other.delays)
            .map_err(error)
    }
    pub(crate) fn copy_from(
        &mut self,
        other: &Self,
        fresh: Option<FreshHistory>,
    ) -> Result<(), Invalid> {
        if !self.can_copy_from(other, fresh) {
            return Err(Invalid("signal history mapping is incompatible"));
        }
        if fresh.is_some() || !self.delay_graph.clock_only_compatible(&other.delay_graph) {
            // Paired by typed endpoints; can_copy_from proved every pair.
            let (delay_graph, delays) = (&self.delay_graph, &mut self.delays);
            let mut result = Ok(());
            delay_graph.pair_retained_pre_main_edges(&other.delay_graph, fresh, |to, from| {
                if result.is_ok() {
                    result = delays.copy_edge_history_from(to, &other.delays, from);
                }
            });
            result.map_err(error)?;
        }
        self.delays
            .copy_stamp_history_from(&other.delays)
            .map_err(error)
    }
    pub(crate) fn can_rebind_graph(&self, graph: &Arc<SignalGraph>) -> bool {
        self.delays.is_closed()
            && self.graph.history_compatible(graph)
            && self.delay_graph.history_compatible(graph)
            && graph.plan.max_latency_frames() <= self.horizon()
    }
    pub(crate) fn storage_bytes_with_graph(&self, graph: &Arc<SignalGraph>) -> Option<usize> {
        // Exhaustive owning projection: both Arc fields live in Self even when
        // they share a body. A distinct retained delay plan owns its own tables.
        let Self {
            graph: _,
            delay_graph,
            delays,
            inputs,
            main,
            key_taps,
        } = self;
        delays
            .storage_bytes()
            .checked_add(graph.table_bytes()?)?
            .checked_add(if Arc::ptr_eq(graph, delay_graph) {
                0
            } else {
                delay_graph.table_bytes()?
            })?
            .checked_add(inputs.capacity().checked_mul(size_of::<Frame>())?)?
            .checked_add(main.capacity().checked_mul(size_of::<Frame>())?)?
            .checked_add(key_taps.capacity().checked_mul(size_of::<Frame>())?)
    }
    pub(crate) fn rebind_graph(&mut self, graph: Arc<SignalGraph>) -> Result<(), Invalid> {
        if !self.can_rebind_graph(&graph) {
            return Err(Invalid("signal history rebind is incompatible"));
        }
        self.graph = graph;
        Ok(())
    }
    pub(crate) fn route_track_output(
        &mut self,
        index: usize,
        frame: Frame,
    ) -> Result<Frame, Invalid> {
        if let Some(tap) = self.key_taps.get_mut(index) {
            *tap = frame;
        }
        let output = self.graph.tracks[index].output;
        let edge = match output {
            TrackOutput::Main { edge } | TrackOutput::Group { edge, .. } => edge,
        };
        let routed = self.edge(edge, frame)?;
        match output {
            TrackOutput::Main { .. } => self.main[index] = routed,
            TrackOutput::Group { target, .. } => {
                self.inputs[target][0] += routed[0];
                self.inputs[target][1] += routed[1];
            }
        }
        Ok(routed)
    }
    pub(crate) fn edge(&mut self, index: usize, frame: Frame) -> Result<Frame, Invalid> {
        let mut frames = [frame];
        self.edge_block(index, &mut frames)?;
        Ok(frames[0])
    }
    pub(crate) fn edge_block(&mut self, index: usize, frames: &mut [Frame]) -> Result<(), Invalid> {
        let index = *self
            .delay_graph
            .pre_main_edges
            .get(index)
            .ok_or(Invalid("mapped delay edge is missing"))?;
        self.delays
            .process_edge_in_place(index, frames)
            .map_err(error)
    }
    pub(crate) fn fader(
        &self,
        track: &Track,
        index: usize,
        override_gain: Option<f32>,
        frame: Frame,
    ) -> Result<Frame, Invalid> {
        Ok(self
            .fader_mix(track, index, override_gain)
            .map_or([0.0; 2], |mix| mix.apply_frame(frame)))
    }
    pub(crate) fn fader_mix(
        &self,
        track: &Track,
        index: usize,
        override_gain: Option<f32>,
    ) -> Option<crate::PreparedTrackMix> {
        let stamp = self.fader_content_stamp(index)?;
        let frame = stamp.arrangement_frame;
        let mut mix =
            override_gain.map_or_else(|| track.mix_at(frame), |gain| track.recording_mix(gain));
        // Apply after devices while retaining the mixer coefficients so live
        // smoothing and automation still advance through the muted interval.
        let ranges = &self.graph.tracks[index].group_output_mutes;
        if stamp.transport_is_running && !ranges.is_empty() {
            let next = ranges.partition_point(|range| range.end() <= frame);
            if ranges.get(next).is_some_and(|range| range.contains(frame)) {
                mix.muted = true;
            }
        }
        Some(mix)
    }
    pub(crate) fn fader_content_frame(&self, index: usize) -> Option<u64> {
        self.fader_content_stamp(index)
            .map(|origin| origin.arrangement_frame)
    }
    fn fader_content_stamp(&self, index: usize) -> Option<TransportStamp> {
        self.delays.stamp_at_delay(
            self.graph.plan.nodes()[self.graph.tracks[index].fader].output_arrival_frames,
        )
    }
    pub(crate) fn process_main(
        &mut self,
        chain: &mut hex_lattice::NativeRuntimeChain,
        frames: &mut [Frame],
        offset: usize,
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
    ) -> Result<(), Invalid> {
        self.process_main_with_foreign(
            chain,
            frames,
            offset,
            &mut hex_lattice::RefuseForeign,
            observer,
            None,
            None,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn process_main_with_foreign(
        &mut self,
        chain: &mut hex_lattice::NativeRuntimeChain,
        frames: &mut [Frame],
        offset: usize,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
        mut input: Option<&mut crate::native_parameter_input::Processing>,
        external: Option<&dyn hex_lattice::NativeExternalAutomationSet>,
    ) -> Result<(), Invalid> {
        if frames.len() != 1 || chain.len() != self.graph.main_devices.len() {
            return Err(Invalid("main content-origin plan does not match runtime"));
        }
        for index in 0..self.graph.main_devices.len() {
            let node = self.graph.main_devices[index];
            self.process_effect_stage(
                node,
                index,
                chain,
                &mut frames[0],
                offset,
                foreign,
                observer,
                input.as_deref_mut(),
                external,
            )?;
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn process_track(
        &mut self,
        index: usize,
        track: &Track,
        chain: &mut hex_lattice::NativeRuntimeChain,
        audio: Frame,
        generator: Frame,
        offset: usize,
        metered: bool,
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
    ) -> Result<Frame, Invalid> {
        self.process_track_with_foreign(
            index,
            track,
            chain,
            audio,
            generator,
            offset,
            metered,
            &mut hex_lattice::RefuseForeign,
            observer,
            None,
            None,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn process_track_with_foreign(
        &mut self,
        index: usize,
        track: &Track,
        chain: &mut hex_lattice::NativeRuntimeChain,
        mut audio: Frame,
        mut generator: Frame,
        offset: usize,
        metered: bool,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
        mut input: Option<&mut crate::native_parameter_input::Processing>,
        external: Option<&dyn hex_lattice::NativeExternalAutomationSet>,
    ) -> Result<Frame, Invalid> {
        let printed = if self.graph.tracks[index].printed_merge.is_some() {
            std::mem::replace(&mut audio, [0.0; 2])
        } else {
            [0.0; 2]
        };
        audio = self.edge(self.graph.tracks[index].source_input, audio)?;
        audio[0] += self.inputs[index][0];
        audio[1] += self.inputs[index][1];
        let slot = track.instrument_position();
        self.effects_with_foreign(
            Owner::Track(index),
            0..slot,
            chain,
            &mut audio,
            offset,
            foreign,
            observer,
            input.as_deref_mut(),
            external,
        )?;
        track.apply_instrument_input_gain(std::slice::from_mut(&mut audio));
        if metered {
            track.instrument_meter.observe_input(&[audio]);
        }
        audio = self.edge(self.graph.tracks[index].audio_merge, audio)?;
        if let Some(node) = self.graph.tracks[index].generator
            && self
                .delays
                .stamp_at_delay(self.graph.plan.nodes()[node].output_arrival_frames)
                .is_none()
        {
            generator = [0.0; 2];
        }
        if let Some(edge) = self.graph.tracks[index].generator_merge {
            generator = self.edge(edge, generator)?;
        }
        let mut mixed = [audio[0] + generator[0], audio[1] + generator[1]];
        track.apply_instrument_output_gain(std::slice::from_mut(&mut mixed));
        if metered {
            track.instrument_meter.observe_output(&[mixed]);
        }
        self.effects_with_foreign(
            Owner::Track(index),
            slot..self.graph.tracks[index].devices.len(),
            chain,
            &mut mixed,
            offset,
            foreign,
            observer,
            input,
            external,
        )?;
        if let Some((processed_edge, printed_edge)) = self.graph.tracks[index].printed_merge {
            let processed = self.edge(processed_edge, mixed)?;
            let printed = self.edge(printed_edge, printed)?;
            mixed = [processed[0] + printed[0], processed[1] + printed[1]];
        }
        Ok(mixed)
    }
    pub(crate) fn process_return(
        &mut self,
        index: usize,
        chain: &mut hex_lattice::NativeRuntimeChain,
        frame: &mut Frame,
        offset: usize,
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
    ) -> Result<(), Invalid> {
        self.effects_with_foreign(
            Owner::Return(index),
            0..self.graph.returns[index].devices.len(),
            chain,
            frame,
            offset,
            &mut hex_lattice::RefuseForeign,
            observer,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn effects_with_foreign(
        &mut self,
        owner: Owner,
        stages: std::ops::Range<usize>,
        chain: &mut hex_lattice::NativeRuntimeChain,
        frame: &mut Frame,
        offset: usize,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
        mut input: Option<&mut crate::native_parameter_input::Processing>,
        external: Option<&dyn hex_lattice::NativeExternalAutomationSet>,
    ) -> Result<(), Invalid> {
        for index in stages {
            let node = match owner {
                Owner::Track(track) => self.graph.tracks[track].devices[index],
                Owner::Return(bus) => self.graph.returns[bus].devices[index],
            };
            self.process_effect_stage(
                node,
                index,
                chain,
                frame,
                offset,
                foreign,
                observer,
                input.as_deref_mut(),
                external,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn process_effect_stage(
        &mut self,
        node: usize,
        index: usize,
        chain: &mut hex_lattice::NativeRuntimeChain,
        frame: &mut Frame,
        offset: usize,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
        input: Option<&mut crate::native_parameter_input::Processing>,
        external: Option<&dyn hex_lattice::NativeExternalAutomationSet>,
    ) -> Result<(), Invalid> {
        let key = if let Some(stage) = self.graph.key_stages.get(node).copied().flatten() {
            *frame = self.edge(stage.main_edge, *frame)?;
            Some(self.edge(stage.key_edge, self.key_taps[stage.source_track])?)
        } else {
            None
        };
        let planned = &self.graph.plan.nodes()[node];
        let clock = OriginClock(self.delays.stamp_at_delay(planned.input_arrival_frames));
        let output_stamp = self.delays.stamp_at_delay(planned.output_arrival_frames);
        let output_clock = OriginClock(output_stamp);
        crate::native_parameter_input::process_range_with_key(
            input,
            external,
            chain,
            index..index + 1,
            std::slice::from_mut(frame),
            offset,
            Some(&clock),
            Some(&output_clock),
            planned.input_arrival_frames,
            planned.output_arrival_frames,
            foreign,
            key,
            observer,
        )?;
        if output_stamp.is_none() {
            *frame = [0.0; 2];
        }
        Ok(())
    }
}

pub(crate) struct OriginClock(pub(crate) Option<TransportStamp>);
impl hex_lattice::NativeInstrumentClock for OriginClock {
    fn arrangement_meter_at_offset(&self, offset: usize) -> Option<(u8, u8)> {
        if offset != 0 {
            return None;
        }
        let meter = self.0?.meter?;
        Some((meter.numerator(), meter.denominator()))
    }
    fn arrangement_cycle_at_offset(&self, offset: usize) -> Option<(u64, u64)> {
        if offset != 0 {
            return None;
        }
        self.0?.cycle_beats_raw
    }
    fn tempo_micro_bpm_at_offset(&self, offset: usize) -> Option<u32> {
        if offset != 0 {
            return None;
        }
        self.0.and_then(|stamp| stamp.tempo_micro_bpm)
    }
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
    fn authored_arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.performance_frame_at_offset(offset)?;
        self.arrangement_frame_at_offset(offset)
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
