//! Compensation history at the staged cut of one Resident audio leaf.
//! SessionPlayback proves the exact authored route and unchanged DSP owners.
//! This module matches retained histories by typed edges. The departing edge
//! must drain and the replacement edge must start with empty PCM history.

use super::{NodeId, SignalGraph, SignalRuntime, TrackOutput, error};
use crate::Invalid;
use hex_project::TrackId;
use std::sync::Arc;

impl SignalGraph {
    /// An upstream-aligned cut swaps a delayed direct-to-Main edge and an
    /// unbuffered Group input in either direction. It can also move an
    /// undelayed leaf to the immediate enclosing Group.
    pub(crate) fn aligned_group_reroute_edges_from(
        &self,
        previous: &Self,
        track: TrackId,
    ) -> Option<(usize, usize)> {
        let next = self.tracks.get(self.track_index(track)?)?;
        let old = previous.tracks.get(previous.track_index(track)?)?;
        let (next, old, expected_delays) = match (next.output, old.output) {
            (TrackOutput::Group { edge: next, .. }, TrackOutput::Main { edge: old }) => {
                (next, old, (false, true))
            }
            (TrackOutput::Main { edge: next }, TrackOutput::Group { edge: old, .. }) => {
                (next, old, (true, false))
            }
            (
                TrackOutput::Group {
                    edge: next,
                    target: next_parent,
                },
                TrackOutput::Group {
                    edge: old,
                    target: old_parent,
                },
            ) => {
                let TrackOutput::Group {
                    target: enclosing, ..
                } = previous.tracks.get(old_parent)?.output
                else {
                    return None;
                };
                if self.tracks.get(next_parent)?.id? != previous.tracks.get(enclosing)?.id? {
                    return None;
                }
                (next, old, (false, false))
            }
            _ => return None,
        };
        let has_delay = |graph: &Self, output| {
            let index = *graph.pre_main_edges.get(output)?;
            Some(graph.plan.edges().get(index)?.delay_frames != 0)
        };
        ((has_delay(self, next)?, has_delay(previous, old)?) == expected_delays)
            .then_some((next, old))
    }

    fn retained_group_route_edges(
        &self,
        output: usize,
    ) -> impl Iterator<Item = ((NodeId, NodeId), usize)> + '_ {
        self.pre_main_edges
            .iter()
            .copied()
            .enumerate()
            .filter(move |(position, _)| *position != output)
            .map(|(_, index)| {
                let edge = &self.plan.edges()[index];
                (
                    (self.typed[edge.from_index], self.typed[edge.to_index]),
                    index,
                )
            })
    }
}

enum GroupRouteHistory {
    Stamps,
    Aligned {
        next_output: usize,
        old_output: usize,
    },
}

impl SignalRuntime {
    fn group_route_clock_layout(&self) -> bool {
        let output = self
            .graph
            .main_devices
            .last()
            .copied()
            .unwrap_or(self.graph.main);
        let output_latency = self.graph.plan.nodes()[output].output_arrival_frames;
        output_latency != 0
            && self.horizon() == output_latency
            && self.graph.supports_closed_compensated_audio_groups()
            && self
                .graph
                .tracks
                .iter()
                .all(|track| track.audio_feeds.is_empty())
            && Arc::ptr_eq(&self.graph, &self.delay_graph)
            && self.delays.is_closed()
    }

    pub(crate) fn group_route_main_delay_only(&self) -> bool {
        self.graph.main_input_arrival() == 0
            && self.group_route_clock_layout()
            && self
                .graph
                .pre_main_edges
                .iter()
                .all(|&edge| self.graph.plan.edges()[edge].delay_frames == 0)
    }

    pub(crate) fn group_route_upstream_delay(&self) -> bool {
        self.graph.main_input_arrival() != 0 && self.group_route_clock_layout()
    }

    fn group_route_history_from(
        &self,
        previous: &Self,
        track: TrackId,
    ) -> Option<GroupRouteHistory> {
        if self.horizon() != previous.horizon() || !previous.delays.history_ready() {
            return None;
        }
        if self.group_route_main_delay_only() && previous.group_route_main_delay_only() {
            return Some(GroupRouteHistory::Stamps);
        }
        if !self.group_route_upstream_delay()
            || !previous.group_route_upstream_delay()
            || self.graph.typed != previous.graph.typed
            || self.graph.pre_main_edges.len() != previous.graph.pre_main_edges.len()
            || !self
                .graph
                .plan
                .nodes()
                .iter()
                .zip(previous.graph.plan.nodes())
                .all(|(next, old)| {
                    next.latency_frames == old.latency_frames
                        && next.input_arrival_frames == old.input_arrival_frames
                        && next.output_arrival_frames == old.output_arrival_frames
                })
        {
            return None;
        }
        let (next_output, old_output) = self
            .graph
            .aligned_group_reroute_edges_from(&previous.graph, track)?;
        // A new direct edge may introduce an alignment ring. Only the
        // successor's post-cut frames belong in that ring.
        if !self
            .delays
            .edge_history_is_silent(self.graph.pre_main_edges[next_output])
        {
            return None;
        }
        self.graph
            .retained_group_route_edges(next_output)
            .zip(previous.graph.retained_group_route_edges(old_output))
            .all(|((next_key, next), (old_key, old))| {
                next_key == old_key
                    && self
                        .delays
                        .can_copy_edge_history_from(next, &previous.delays, old)
            })
            .then_some(GroupRouteHistory::Aligned {
                next_output,
                old_output,
            })
    }

    pub(crate) fn can_copy_group_route_history_from(
        &self,
        previous: &Self,
        track: TrackId,
    ) -> bool {
        self.group_route_history_from(previous, track).is_some()
    }

    pub(crate) fn copy_group_route_history_from(
        &mut self,
        previous: &Self,
        track: TrackId,
    ) -> Result<(), Invalid> {
        let history = self
            .group_route_history_from(previous, track)
            .ok_or(Invalid("group route cannot carry transport history"))?;
        if let GroupRouteHistory::Aligned {
            next_output,
            old_output,
        } = history
        {
            let departing = previous.graph.pre_main_edges[old_output];
            if !previous.delays.edge_history_is_silent(departing) {
                return Err(Invalid("departing group route still holds aligned audio"));
            }
            for ((_, next), (_, old)) in self
                .graph
                .retained_group_route_edges(next_output)
                .zip(previous.graph.retained_group_route_edges(old_output))
            {
                self.delays
                    .copy_edge_history_from(next, &previous.delays, old)
                    .map_err(error)?;
            }
        }
        self.delays
            .copy_stamp_history_from(&previous.delays)
            .map_err(error)
    }
}
