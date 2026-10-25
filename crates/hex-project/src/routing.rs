//! Legacy structural proposals; playable inputs use typed AudioFeed/MidiRoute commands.
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
    /// Legacy proposal metadata only; typed MIDI routes have their own validation.
    Midi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RoutingEdge {
    pub source: RoutingOwner,
    pub target: RoutingOwner,
    pub signal: RoutingSignal,
}

impl Project {
    /// Recheck combined audio and MIDI graph constraints on a canonical input.
    ///
    /// Control-side preparation may borrow this query without cloning a public
    /// snapshot or reconstructing unrelated clips, devices and media metadata.
    /// Numeric values and global identities remain established by canonical
    /// constructors/typed edits; untrusted snapshots still require `from_snapshot`.
    /// Sealed fields alone do not prove combined graph validity: an intermediate
    /// prepared batch can introduce a cycle or remove a MIDI destination's
    /// instrument. Recheck both graph owners, including sends and Return feeds.
    /// This does not admit playback resources or supported device capabilities.
    pub fn validate_playback_routing(&self) -> Result<(), Invalid> {
        self.validate_routing_proposal(&[])?;
        self.validate_playable_routes()
    }

    /// Structural validation is not permission to render or adopt this graph.
    /// Existing sends have one owner and are included alongside implicit Main
    /// paths. This legacy proposal does not authorize adoption; use the canonical
    /// typed route commands to edit supported audio inputs, MIDI sources or feeds.
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
        for feed in self.audio_feeds() {
            edges.insert((feed.source(), RoutingOwner::Track(feed.destination())));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Editor, TrackDeviceSlot};

    fn instrument(editor: &mut Editor) -> TrackId {
        let Created::Track(id) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "track".into(),
                before_track: None,
                slot: TrackDeviceSlot::Instrument,
                state: crate::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("fixture track")
        };
        id
    }

    fn fixture() -> (Editor, TrackId, ReturnId) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let track = instrument(&mut editor);
        let mut returns = Vec::new();
        for name in ["a", "b"] {
            let Created::ReturnTrack(id) = editor
                .execute(Command::AddReturnTrack { name: name.into() })
                .unwrap()
            else {
                panic!("fixture return")
            };
            returns.push(id);
        }
        editor
            .execute(Command::SetReturnFeed {
                source: returns[0],
                destination: returns[1],
                gain: 0.5,
            })
            .unwrap();
        editor
            .execute(Command::SetAudioFeed {
                source: RoutingOwner::Return(returns[1]),
                destination: track,
                gain: 0.5,
            })
            .unwrap();
        (editor, track, returns[0])
    }

    fn matches_snapshot_guard(project: &Project, expected: Result<(), Invalid>) {
        let before = project.clone();
        assert_eq!(project.validate_playback_routing(), expected);
        assert_eq!(
            Project::validate_return_feed_snapshot(&project.snapshot()).map(|_| ()),
            expected,
        );
        assert_eq!(project, &before, "validation must only borrow");
    }

    #[test]
    fn borrowed_routing_guard_rejects_return_only_cycle_without_playable_routes() {
        let (editor, _, _) = fixture();
        let mut candidate = editor.project().clone();
        candidate.audio_feeds.clear();
        assert!(candidate.midi_routes.is_empty());
        matches_snapshot_guard(&candidate, Ok(()));
        let mut reverse = candidate.routing[0];
        std::mem::swap(&mut reverse.source, &mut reverse.destination);
        reverse.id = crate::RouteId(candidate.next_id);
        candidate.next_id += 1;
        candidate.routing.push(reverse);
        matches_snapshot_guard(&candidate, Err(Invalid("routing cycles are unsupported")));
    }

    #[test]
    fn borrowed_routing_guard_rejects_combined_cycle_and_accepts_repaired_candidate() {
        let (editor, track, return_track) = fixture();
        let mut candidate = editor.project().clone();
        matches_snapshot_guard(&candidate, Ok(()));
        // Exercise the precommit candidate directly, rather than relying on
        // Editor's final batch validation to reject before reaching this query.
        candidate
            .apply(Command::SetReturnSend {
                track,
                return_track,
                gain: 1.0,
            })
            .unwrap();
        matches_snapshot_guard(&candidate, Err(Invalid("routing cycles are unsupported")));
        candidate
            .apply(Command::SetReturnSend {
                track,
                return_track,
                gain: 0.0,
            })
            .unwrap();
        matches_snapshot_guard(&candidate, Ok(()));
    }

    #[test]
    fn borrowed_routing_guard_rejects_midi_destination_loss_and_accepts_route_removal() {
        let (mut editor, source, _) = fixture();
        let destination = instrument(&mut editor);
        editor
            .execute(Command::SetMidiSource {
                destination,
                source: Some(source),
            })
            .unwrap();
        let mut candidate = editor.project().clone();
        matches_snapshot_guard(&candidate, Ok(()));
        candidate
            .apply(Command::RemoveInstrument { track: destination })
            .unwrap();
        matches_snapshot_guard(
            &candidate,
            Err(Invalid("midi destination requires an instrument")),
        );
        candidate
            .apply(Command::SetMidiSource {
                destination,
                source: None,
            })
            .unwrap();
        matches_snapshot_guard(&candidate, Ok(()));
    }
}
