//! Bounded clock transfer for a retained worker between two bare MIDI routes.
//! The physical bridge already owns generated PCM. Changed compensated edges
//! must be silent, and only unchanged typed edges and stamps are copied.
use super::{NodeId, SignalGraph, SignalRuntime, error};
use crate::Invalid;
use hex_project::TrackId;
use std::sync::Arc;

const BRIDGE: u32 = 512;

impl SignalRuntime {
    pub(crate) fn can_copy_hosted_move_from(
        &self,
        previous: &Self,
        donor: TrackId,
        receiver: TrackId,
    ) -> bool {
        self.hosted_move_edges_from(previous, donor, receiver)
            .is_some()
    }

    pub(crate) fn copy_hosted_move_from(
        &mut self,
        previous: &Self,
        donor: TrackId,
        receiver: TrackId,
    ) -> Result<(), Invalid> {
        let retained = self
            .hosted_move_edges_from(previous, donor, receiver)
            .ok_or(Invalid("hosted move compensation history changed"))?;
        for (next, old) in retained {
            self.delays
                .copy_edge_history_from(next, &previous.delays, old)
                .map_err(error)?;
        }
        self.delays
            .copy_stamp_history_from(&previous.delays)
            .map_err(error)
    }

    fn hosted_move_edges_from(
        &self,
        previous: &Self,
        donor: TrackId,
        receiver: TrackId,
    ) -> Option<[(usize, usize); 4]> {
        if donor == receiver
            || !previous.delays.history_ready()
            || [self, previous].into_iter().any(|runtime| {
                runtime.horizon() != BRIDGE
                    || runtime.graph.main_input_arrival() != BRIDGE
                    || runtime.graph.main_latency() != BRIDGE
                    || runtime.graph.tracks.len() != 2
                    || runtime.graph.pre_main_edges.len() != 9
                    || !runtime.graph.main_devices.is_empty()
                    || !runtime.graph.returns.is_empty()
                    || !Arc::ptr_eq(&runtime.graph, &runtime.delay_graph)
                    || !runtime.delays.is_closed()
            })
        {
            return None;
        }
        let donor = donor.value();
        let receiver = receiver.value();
        let dry = |id| (NodeId::TrackInput(id), NodeId::Merge(id));
        let output = |id| (NodeId::Fader(id), NodeId::Main);
        let generator = |id| (NodeId::Generator(id), NodeId::Merge(id));
        for (key, old_delay, new_delay) in [
            (dry(donor), Some(BRIDGE), Some(0)),
            (dry(receiver), Some(0), Some(BRIDGE)),
            (output(donor), Some(0), Some(BRIDGE)),
            (output(receiver), Some(BRIDGE), Some(0)),
            (generator(donor), Some(0), None),
            (generator(receiver), None, Some(0)),
        ] {
            for (runtime, expected) in [(previous, old_delay), (self, new_delay)] {
                match (plan_edge(&runtime.graph, key), expected) {
                    (None, None) => {}
                    (Some(edge), Some(delay))
                        if runtime.graph.plan.edges()[edge].delay_frames == delay
                            && runtime.delays.edge_history_is_silent(edge) => {}
                    _ => return None,
                }
            }
        }
        let source = |id| (NodeId::Audio(id), NodeId::TrackInput(id));
        let fader = |id| (NodeId::Merge(id), NodeId::Fader(id));
        let mut retained = [(0, 0); 4];
        for (pair, key) in retained.iter_mut().zip([
            source(donor),
            source(receiver),
            fader(donor),
            fader(receiver),
        ]) {
            let next = plan_edge(&self.graph, key)?;
            let old = plan_edge(&previous.graph, key)?;
            if !self
                .delays
                .can_copy_edge_history_from(next, &previous.delays, old)
            {
                return None;
            }
            *pair = (next, old);
        }
        Some(retained)
    }
}

fn plan_edge(graph: &SignalGraph, key: (NodeId, NodeId)) -> Option<usize> {
    graph
        .pre_main_edges
        .iter()
        .copied()
        .enumerate()
        .find_map(|(position, edge)| {
            (graph.pre_main_endpoints(position) == Some(key)).then_some(edge)
        })
}
