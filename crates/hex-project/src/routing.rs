//! Structural proposals; only explicit canonical Return feeds have playback support.
use crate::{Created, Invalid, Project, ReturnId, TrackId};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Additional authored edges, excluding canonical sends and implicit Main paths.
pub const MAX_AUTHORED_ROUTES: usize = 4_096;
pub const ROUTING_UNSUPPORTED: &str = "authored routing requires playback support";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RoutingOwner {
    Track(TrackId),
    Return(ReturnId),
    Main,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RoutingSignal {
    Audio {
        channels: u8,
    },
    /// Representable for explicit refusal; no current role exposes a MIDI bus.
    Midi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RoutingEdge {
    pub source: RoutingOwner,
    pub target: RoutingOwner,
    pub signal: RoutingSignal,
}

impl Project {
    /// Structural validation is not permission to render or adopt this graph.
    /// Existing sends have one owner and are included alongside implicit Main
    /// paths. Return-to-Return proposals can be checked, but cannot be adopted.
    pub fn validate_routing_proposal(&self, proposed: &[RoutingEdge]) -> Result<(), Invalid> {
        if proposed.len() > MAX_AUTHORED_ROUTES {
            return Err(Invalid("authored routing edge limit reached"));
        }
        let mut nodes = BTreeMap::new();
        for track in self.tracks() {
            nodes.insert(RoutingOwner::Track(track.id()), nodes.len());
        }
        for bus in self.return_tracks() {
            nodes.insert(RoutingOwner::Return(bus.id()), nodes.len());
        }
        nodes.insert(RoutingOwner::Main, nodes.len());
        let mut edges = BTreeSet::new();
        for track in self.tracks() {
            edges.insert((RoutingOwner::Track(track.id()), RoutingOwner::Main));
            for send in track.sends() {
                edges.insert((
                    RoutingOwner::Track(track.id()),
                    RoutingOwner::Return(send.return_track()),
                ));
            }
        }
        for bus in self.return_tracks() {
            edges.insert((RoutingOwner::Return(bus.id()), RoutingOwner::Main));
        }
        for feed in self.authored_routing() {
            edges.insert((
                RoutingOwner::Return(feed.source()),
                RoutingOwner::Return(feed.destination()),
            ));
        }
        for edge in proposed {
            if !nodes.contains_key(&edge.source) || !nodes.contains_key(&edge.target) {
                return Err(Invalid("routing owner no longer exists"));
            }
            if edge.signal != (RoutingSignal::Audio { channels: 2 }) {
                return Err(Invalid("routing requires supported stereo audio ports"));
            }
            if edge.source == RoutingOwner::Main || matches!(edge.target, RoutingOwner::Track(_)) {
                return Err(Invalid("routing port direction is unsupported"));
            }
            if !edges.insert((edge.source, edge.target)) {
                return Err(Invalid("duplicate routing edge"));
            }
        }
        let mut outgoing = vec![Vec::new(); nodes.len()];
        let mut incoming = vec![0_usize; nodes.len()];
        for (source, target) in edges {
            let source = *nodes
                .get(&source)
                .ok_or(Invalid("routing owner no longer exists"))?;
            let target = *nodes
                .get(&target)
                .ok_or(Invalid("routing owner no longer exists"))?;
            outgoing[source].push(target);
            incoming[target] += 1;
        }
        let mut ready: VecDeque<_> = incoming
            .iter()
            .enumerate()
            .filter_map(|(index, &count)| (count == 0).then_some(index))
            .collect();
        let mut visited = 0;
        while let Some(node) = ready.pop_front() {
            visited += 1;
            for &next in &outgoing[node] {
                incoming[next] -= 1;
                if incoming[next] == 0 {
                    ready.push_back(next);
                }
            }
        }
        if visited != nodes.len() {
            return Err(Invalid("routing cycles are unsupported"));
        }
        Ok(())
    }

    pub(crate) fn adopt_routing(&mut self, proposed: Vec<RoutingEdge>) -> Result<Created, Invalid> {
        self.validate_routing_proposal(&proposed)?;
        if !proposed.is_empty() {
            return Err(Invalid(ROUTING_UNSUPPORTED));
        }
        Ok(Created::None)
    }

    /// Construct a canonical snapshot candidate with additional structural proposals.
    /// Generic proposals remain closed; typed Return feeds use their own command.
    pub fn from_snapshot_with_routing(
        snapshot: crate::Snapshot,
        proposed: Vec<RoutingEdge>,
    ) -> Result<Self, Invalid> {
        let mut project = Self::from_snapshot(snapshot)?;
        project.adopt_routing(proposed)?;
        Ok(project)
    }

    /// Canonical Return feeds. Existing sends remain on regular tracks; this
    /// list never duplicates their gains or identities.
    pub fn authored_routing(&self) -> &[crate::ReturnFeed] {
        &self.routing
    }
}
