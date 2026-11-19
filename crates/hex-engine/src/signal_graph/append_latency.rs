//! Proposed end-of-chain timing derived from the existing prepared topology.
use super::*;
use crate::{PreparedRevision, ProcessorChainOwner, ProcessorGraphLatency};

impl PreparedRevision {
    /// Plan an effect appended to an existing Track, Return or Main chain on
    /// control. The caller must match this frozen baseline to the insertion's
    /// canonical context and supply the actual pending processor's total declared
    /// latency, including its bridge. New tracks and instruments need different
    /// topology changes and are not covered by this query.
    ///
    /// This is a proposal, not applied timing or source validation. Only scalars
    /// escape: no runnable graph, processor factory or publication authority is
    /// created. Hardware I/O and bridge placement at SDK ports remain separate.
    pub fn appended_effect_latency_on_control(
        &self,
        owner: ProcessorChainOwner,
        latency_frames: u32,
    ) -> Result<ProcessorGraphLatency, Invalid> {
        self.arrangement()
            .signal_graph
            .as_deref()
            .ok_or(Invalid("prepared signal graph is unavailable"))?
            .appended_effect_latency(owner, latency_frames)
    }
}

impl SignalGraph {
    fn appended_effect_latency(
        &self,
        owner: ProcessorChainOwner,
        latency_frames: u32,
    ) -> Result<ProcessorGraphLatency, Invalid> {
        let count = self
            .plan
            .nodes()
            .len()
            .checked_add(1)
            .filter(|n| *n <= MAX_COMPENSATION_NODES)
            .ok_or_else(|| error(CompensationError::GraphLimit))?;
        let edge_count = self
            .plan
            .edges()
            .len()
            .checked_add(1)
            .filter(|n| *n <= MAX_COMPENSATION_EDGES)
            .ok_or_else(|| error(CompensationError::GraphLimit))?;
        if latency_frames > MAX_COMPENSATION_FRAMES {
            return Err(error(CompensationError::LatencyLimit));
        }
        let original = self.plan.nodes();
        let key = |index: usize| {
            original
                .get(index)
                .map(|node| node.key)
                .ok_or(Invalid("append timing node is missing"))
        };
        let new_key = original
            .last()
            .and_then(|node| node.key.checked_add(1))
            .ok_or(Invalid("append timing node identity exhausted"))?;
        let edge_key = self
            .plan
            .edges()
            .last()
            .map_or(Some(1), |edge| edge.key.checked_add(1))
            .ok_or(Invalid("append timing edge identity exhausted"))?;
        let final_output = self.main_devices.last().copied().unwrap_or(self.main);
        let incoming = |target| {
            self.plan
                .edges()
                .iter()
                .position(|edge| edge.to_index == target)
                .ok_or(Invalid("append timing input edge is missing"))
        };
        let splice = match owner {
            ProcessorChainOwner::Track(id) => {
                let track = self
                    .tracks
                    .iter()
                    .find(|track| track.id.is_some_and(|track| track.value() == id))
                    .ok_or(Invalid("append timing track is missing"))?;
                Some(match track.printed_merge {
                    Some((processed, _)) => *self
                        .pre_main_edges
                        .get(processed)
                        .ok_or(Invalid("append timing processing edge is missing"))?,
                    None => incoming(track.fader)?,
                })
            }
            ProcessorChainOwner::Return(id) => {
                let bus = self
                    .returns
                    .iter()
                    .find(|bus| bus.id.value() == id)
                    .ok_or(Invalid("append timing return is missing"))?;
                Some(incoming(bus.output)?)
            }
            ProcessorChainOwner::Main => None,
            ProcessorChainOwner::Source(_) => {
                return Err(Invalid("source effect append timing is unsupported"));
            }
        };
        let mut nodes = Vec::new();
        nodes
            .try_reserve_exact(count)
            .map_err(|_| error(CompensationError::Allocation))?;
        nodes.extend(original.iter().map(|node| LatencyNode {
            key: node.key,
            latency_frames: Some(node.latency_frames),
        }));
        nodes.push(LatencyNode {
            key: new_key,
            latency_frames: Some(latency_frames),
        });
        let mut edges = Vec::new();
        edges
            .try_reserve_exact(edge_count)
            .map_err(|_| error(CompensationError::Allocation))?;
        for (index, edge) in self.plan.edges().iter().enumerate() {
            edges.push(LatencyEdge {
                key: edge.key,
                from: if splice == Some(index) {
                    new_key
                } else {
                    key(edge.from_index)?
                },
                to: key(edge.to_index)?,
            });
        }
        let from = match splice {
            Some(index) => {
                self.plan
                    .edges()
                    .get(index)
                    .ok_or(Invalid("append timing edge is missing"))?
                    .from_index
            }
            None => final_output,
        };
        edges.push(LatencyEdge {
            key: edge_key,
            from: key(from)?,
            to: new_key,
        });
        // Recompute reconvergence delays with the one existing planner. Inserting
        // a slow stage may consume an old alignment delay rather than add to it.
        let plan =
            LatencyPlan::build(&nodes, &edges, CompensationLimits::default()).map_err(error)?;
        let stage = plan
            .node_index(new_key)
            .and_then(|index| plan.nodes().get(index))
            .ok_or(Invalid("proposed append stage is missing"))?;
        let final_key = if splice.is_none() {
            new_key
        } else {
            key(final_output)?
        };
        let output = plan
            .node_index(final_key)
            .and_then(|index| plan.nodes().get(index))
            .ok_or(Invalid("proposed append output is missing"))?;
        Ok(ProcessorGraphLatency {
            upstream_frames: stage.input_arrival_frames,
            stage_frames: stage.latency_frames,
            downstream_frames: output
                .output_arrival_frames
                .checked_sub(stage.output_arrival_frames)
                .ok_or(Invalid("proposed append timing is inconsistent"))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        PreparedPlaybackObservation,
        signal_graph::tests::{fixture, prepare, state},
    };
    use hex_project::{Command, Created, TrackSelectionIds};

    #[test]
    fn proposed_append_matches_canonical_compile_across_reconverging_paths() {
        let mut f = fixture(&[&[3, 5], &[], &[]], 32);
        let Created::Track(group) = f
            .editor
            .execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&f.tracks[..2]).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group")
        };
        let Created::ReturnTrack(bus) = f
            .editor
            .execute(Command::AddReturnTrack {
                name: "return".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        for command in [
            Command::SetReturnSend {
                track: group,
                return_track: bus,
                gain: 1.0,
            },
            Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state: state(4).unwrap(),
            },
            Command::InsertMainDevice {
                before: None,
                state: state(7).unwrap(),
            },
        ] {
            f.editor.execute(command).unwrap();
        }
        let baseline = prepare(&f).unwrap();
        assert_eq!(baseline.arrangement().main_latency_frames(), 19);
        let track = |index: usize| ProcessorChainOwner::Track(f.tracks[index].value());
        // Independent baseline: source3+5, Return4, Main7. A dry append below
        // twelve frames consumes existing alignment; above it, Main must move.
        for (owner, delay, expected) in [
            (track(0), 6, (8, 6, 11)),
            (track(2), 6, (0, 6, 13)),
            (track(2), 20, (0, 20, 7)),
            (ProcessorChainOwner::Track(group.value()), 6, (8, 6, 11)),
            (ProcessorChainOwner::Return(bus.value()), 6, (12, 6, 7)),
            (ProcessorChainOwner::Main, 6, (19, 6, 0)),
        ] {
            let proposal = baseline
                .appended_effect_latency_on_control(owner, delay)
                .unwrap();
            assert_eq!(
                (
                    proposal.upstream_frames,
                    proposal.stage_frames,
                    proposal.downstream_frames
                ),
                expected
            );
            let state = state(delay).unwrap();
            let kind = state.type_id();
            let command = match owner {
                ProcessorChainOwner::Track(id) => Command::InsertDevice {
                    track: f
                        .tracks
                        .iter()
                        .copied()
                        .chain([group])
                        .find(|track| track.value() == id)
                        .unwrap(),
                    before: None,
                    state,
                },
                ProcessorChainOwner::Return(_) => Command::InsertReturnDevice {
                    return_track: bus,
                    before: None,
                    state,
                },
                ProcessorChainOwner::Main => Command::InsertMainDevice {
                    before: None,
                    state,
                },
                ProcessorChainOwner::Source(_) => unreachable!("fixture owner"),
            };
            let Created::Device(device) = f.editor.execute(command).unwrap() else {
                panic!("device")
            };
            let candidate = prepare(&f).unwrap();
            let actual = PreparedPlaybackObservation::prepare_on_control(&candidate).unwrap();
            assert_eq!(
                actual.processor_latency(device.value(), kind),
                Some(proposal),
                "{owner:?}"
            );
            assert!(f.editor.undo());
            assert_eq!(
                baseline.arrangement().main_latency_frames(),
                19,
                "proposal never mutates baseline"
            );
        }
    }

    #[test]
    fn append_refuses_invalid_owners_and_latency_without_changing_baseline() {
        let f = fixture(&[&[3]], 8);
        let baseline = prepare(&f).unwrap();
        for owner in [
            ProcessorChainOwner::Track(0),
            ProcessorChainOwner::Return(u64::MAX),
            ProcessorChainOwner::Source(1),
        ] {
            assert!(
                baseline
                    .appended_effect_latency_on_control(owner, 1)
                    .is_err()
            );
        }
        for latency in [u32::MAX, MAX_COMPENSATION_FRAMES] {
            assert!(
                baseline
                    .appended_effect_latency_on_control(ProcessorChainOwner::Main, latency)
                    .is_err()
            );
        }
        let zero = baseline
            .appended_effect_latency_on_control(ProcessorChainOwner::Main, 0)
            .unwrap();
        assert_eq!(
            (
                zero.upstream_frames,
                zero.stage_frames,
                zero.downstream_frames
            ),
            (3, 0, 0)
        );
        assert_eq!(baseline.arrangement().main_latency_frames(), 3);
    }

    #[test]
    fn append_follows_generator_merge_and_retained_printed_processing_branch() {
        let mut f = fixture(&[&[3]], 8);
        f.editor
            .execute(Command::InsertInstrument {
                track: f.tracks[0],
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
        let revision = prepare(&f).unwrap();
        let mut project = (**revision.arrangement()).clone();
        // Exercise the graph-only five-frame generator declaration, independently
        // of the three-frame audio prefix, as in the existing printed-path tests.
        project.tracks[0].instrument_position = 1;
        project.tracks[0].instrument_latency = Some(5);
        project.tracks[0].instrument_runtime_latency = Some(5);
        let graph = SignalGraph::prepare_value(&project, &[], f.editor.project(), &[0]).unwrap();
        let proposal = graph
            .appended_effect_latency(ProcessorChainOwner::Track(f.tracks[0].value()), 6)
            .unwrap();
        assert_eq!(
            (
                proposal.upstream_frames,
                proposal.stage_frames,
                proposal.downstream_frames
            ),
            (5, 6, 0)
        );
        assert_eq!(graph.main_output_latency().unwrap().arrival_frames, 5);
    }
}
