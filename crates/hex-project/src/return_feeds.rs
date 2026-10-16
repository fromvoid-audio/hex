//! Canonical bounded stereo Return feeds with atomic DAG admission.
use crate::{
    Created, Invalid, Project, ReturnId, RouteId, RoutingEdge, RoutingOwner, RoutingSignal,
    SessionCellContentSnapshot, Snapshot, TrackContentSnapshot,
};
use std::collections::BTreeSet;

/// A unique DAG on sixteen Returns has at most 16 * 15 / 2 edges.
pub const MAX_RETURN_FEEDS: usize = 120;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReturnFeed {
    pub(crate) id: RouteId,
    pub(crate) source: ReturnId,
    pub(crate) destination: ReturnId,
    pub(crate) gain: f32,
}
impl ReturnFeed {
    pub fn id(self) -> RouteId {
        self.id
    }
    pub fn source(self) -> ReturnId {
        self.source
    }
    pub fn destination(self) -> ReturnId {
        self.destination
    }
    pub fn gain(self) -> f32 {
        self.gain
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReturnFeedSnapshot {
    pub id: u64,
    pub source: u64,
    pub destination: u64,
    pub gain: f32,
}

impl Snapshot {
    /// Prepare a new metadata candidate without mutating the original. Gain
    /// edits keep identity; zero removes; recreation uses allocator high-water.
    pub fn with_return_feed(
        &self,
        source: ReturnId,
        destination: ReturnId,
        gain: f32,
    ) -> Result<Self, Invalid> {
        Project::validate_return_feed_snapshot(self)?;
        if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
            return Err(Invalid("return feed gain must be finite and within 0..2"));
        }
        if !self.return_tracks.iter().any(|r| r.id == source.value())
            || !self
                .return_tracks
                .iter()
                .any(|r| r.id == destination.value())
        {
            return Err(Invalid("return feed owner no longer exists"));
        }
        let mut candidate = self.clone();
        let existing = candidate.return_feeds.iter().position(|feed| {
            feed.source == source.value() && feed.destination == destination.value()
        });
        match (existing, gain == 0.0) {
            (Some(index), true) => {
                candidate.return_feeds.remove(index);
            }
            (Some(index), false) => candidate.return_feeds[index].gain = gain,
            (None, true) => {}
            (None, false) => {
                if candidate.return_feeds.len() == MAX_RETURN_FEEDS {
                    return Err(Invalid("return feed limit reached"));
                }
                let id = candidate.next_id;
                candidate.next_id = id
                    .checked_add(1)
                    .ok_or(Invalid("project id space exhausted"))?;
                candidate.return_feeds.push(ReturnFeedSnapshot {
                    id,
                    source: source.value(),
                    destination: destination.value(),
                    gain,
                });
            }
        }
        Project::validate_return_feed_snapshot(&candidate)?;
        Ok(candidate)
    }
}

impl Project {
    /// Validate global identities, endpoints, gain, pair uniqueness and the
    /// complete DAG. Returned records are metadata, not playback admission.
    pub fn validate_return_feed_snapshot(snapshot: &Snapshot) -> Result<Vec<ReturnFeed>, Invalid> {
        if snapshot.audio_feeds.len() > crate::MAX_AUDIO_FEEDS
            || snapshot.midi_routes.len() > crate::MAX_MIDI_ROUTES
        {
            return Err(Invalid("authored routing edge limit reached"));
        }
        if snapshot.return_feeds.len() > MAX_RETURN_FEEDS {
            return Err(Invalid("return feed limit reached"));
        }
        let mut legacy = snapshot.clone();
        legacy.return_feeds.clear();
        legacy.audio_feeds.clear();
        legacy.midi_routes.clear();
        let mut base = Self::from_snapshot(legacy)?;
        let mut seen = existing_ids(snapshot);
        seen.extend(snapshot.audio_feeds.iter().map(|r| r.id));
        seen.extend(snapshot.midi_routes.iter().map(|r| r.id));
        let mut pairs = BTreeSet::new();
        let mut feeds = Vec::with_capacity(snapshot.return_feeds.len());
        let mut edges = Vec::with_capacity(snapshot.return_feeds.len());
        for feed in &snapshot.return_feeds {
            if feed.id == 0 || feed.id >= snapshot.next_id || !seen.insert(feed.id) {
                return Err(Invalid(
                    "return feed id must be globally unique below allocator high-water",
                ));
            }
            if !feed.gain.is_finite() || !(0.0..=2.0).contains(&feed.gain) || feed.gain == 0.0 {
                return Err(Invalid(
                    "stored return feed gain must be positive and within 0..2",
                ));
            }
            let source = base
                .return_tracks()
                .iter()
                .find(|r| r.id().value() == feed.source)
                .ok_or(Invalid("return feed owner no longer exists"))?
                .id();
            let destination = base
                .return_tracks()
                .iter()
                .find(|r| r.id().value() == feed.destination)
                .ok_or(Invalid("return feed owner no longer exists"))?
                .id();
            if !pairs.insert((source, destination)) {
                return Err(Invalid("duplicate return feed"));
            }
            feeds.push(ReturnFeed {
                id: RouteId(feed.id),
                source,
                destination,
                gain: feed.gain,
            });
            edges.push(RoutingEdge {
                source: RoutingOwner::Return(source),
                target: RoutingOwner::Return(destination),
                signal: RoutingSignal::Audio { channels: 2 },
            });
        }
        base.validate_routing_proposal(&edges)?;
        base.routing = feeds.clone();
        base.adopt_playable_snapshot(snapshot)?;
        Ok(feeds)
    }

    pub(crate) fn set_return_feed(
        &mut self,
        source: ReturnId,
        destination: ReturnId,
        gain: f32,
    ) -> Result<Created, Invalid> {
        let candidate = self
            .snapshot()
            .with_return_feed(source, destination, gain)?;
        let feeds = Self::validate_return_feed_snapshot(&candidate)?;
        self.routing = feeds;
        self.next_id = candidate.next_id;
        Ok(Created::None)
    }
}

pub(crate) fn existing_ids(snapshot: &Snapshot) -> BTreeSet<u64> {
    let mut ids = BTreeSet::new();
    ids.extend(snapshot.assets.iter().map(|a| a.id));
    for track in &snapshot.tracks {
        ids.insert(track.id);
        ids.extend(track.devices.iter().map(|d| d.id));
        ids.extend(track.instrument.iter().map(|d| d.id));
        let (audio, midi) = match &track.content {
            TrackContentSnapshot::Audio(audio) => (audio.as_slice(), &[][..]),
            TrackContentSnapshot::Instrument(midi) => (&[][..], midi.as_slice()),
            TrackContentSnapshot::Hybrid {
                audio_clips,
                midi_clips,
            } => (audio_clips.as_slice(), midi_clips.as_slice()),
        };
        ids.extend(audio.iter().map(|c| c.id));
        for clip in midi {
            ids.insert(clip.id);
            ids.extend(clip.notes.iter().map(|n| n.id));
        }
    }
    for bus in &snapshot.return_tracks {
        ids.insert(bus.id);
        ids.extend(bus.devices.iter().map(|d| d.id));
    }
    ids.extend(snapshot.session.scenes.iter().map(|s| s.id));
    for cell in &snapshot.session.cells {
        match &cell.content {
            SessionCellContentSnapshot::Audio(clip) => {
                ids.insert(clip.id);
            }
            SessionCellContentSnapshot::Midi(clip) => {
                ids.insert(clip.id);
                ids.extend(clip.source.notes.iter().map(|n| n.id));
            }
            SessionCellContentSnapshot::Stop => {}
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Editor};

    fn fixture(count: usize) -> (Editor, Vec<ReturnId>) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let mut ids = Vec::new();
        for _ in 0..count {
            let Created::ReturnTrack(id) = editor
                .execute(Command::AddReturnTrack { name: "bus".into() })
                .unwrap()
            else {
                panic!("return")
            };
            ids.push(id);
        }
        (editor, ids)
    }

    #[test]
    fn candidates_keep_gain_identity_zero_removes_and_recreation_never_rewinds() {
        let (mut editor, ids) = fixture(2);
        editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
        assert!(editor.undo());
        let before = editor.project().clone();
        let legacy = before.snapshot();
        let first = legacy.with_return_feed(ids[0], ids[1], 0.25).unwrap();
        assert_eq!(first.return_feeds[0].id, legacy.next_id);
        let changed = first.with_return_feed(ids[0], ids[1], 2.0).unwrap();
        assert_eq!(first.return_feeds[0].id, changed.return_feeds[0].id);
        assert_eq!(first.next_id, changed.next_id);
        let removed = changed.with_return_feed(ids[0], ids[1], 0.0).unwrap();
        assert!(removed.return_feeds.is_empty());
        assert_eq!(removed.next_id, changed.next_id);
        let recreated = removed.with_return_feed(ids[0], ids[1], 1.0).unwrap();
        assert!(recreated.return_feeds[0].id > first.return_feeds[0].id);
        assert_eq!(
            Project::from_snapshot(first.clone()).unwrap().snapshot(),
            first
        );
        editor
            .execute(Command::SetReturnFeed {
                source: ids[0],
                destination: ids[1],
                gain: 0.25,
            })
            .unwrap();
        assert!(!editor.can_redo());
        let committed = editor.project().snapshot();
        let back = crate::RoutingEdge {
            source: crate::RoutingOwner::Return(ids[1]),
            target: crate::RoutingOwner::Return(ids[0]),
            signal: crate::RoutingSignal::Audio { channels: 2 },
        };
        assert_eq!(
            editor.execute(Command::SetRouting(vec![back])),
            Err(Invalid("routing cycles are unsupported"))
        );
        assert_eq!(editor.project().snapshot(), committed);
        let identity = editor.project().authored_routing()[0].id();
        editor
            .execute(Command::SetReturnFeed {
                source: ids[0],
                destination: ids[1],
                gain: 0.5,
            })
            .unwrap();
        assert_eq!(editor.project().authored_routing()[0].id(), identity);
        let committed = editor.project().snapshot();
        assert!(
            editor
                .execute(Command::SetReturnFeed {
                    source: ids[1],
                    destination: ids[0],
                    gain: 1.0,
                })
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), committed);
        editor
            .execute(Command::SetReturnFeed {
                source: ids[0],
                destination: ids[1],
                gain: 0.0,
            })
            .unwrap();
        assert!(editor.project().authored_routing().is_empty());
        assert!(editor.undo());
        assert_eq!(editor.project().authored_routing()[0].id(), identity);
        assert!(editor.redo());
        editor
            .execute(Command::SetReturnFeed {
                source: ids[0],
                destination: ids[1],
                gain: 1.0,
            })
            .unwrap();
        assert!(editor.project().authored_routing()[0].id().value() > identity.value());
    }

    #[test]
    fn complete_dag_bound_cycles_invalid_gain_and_global_ids_are_atomic() {
        let (editor, ids) = fixture(16);
        let mut candidate = editor.project().snapshot();
        for (index, &source) in ids.iter().enumerate() {
            for &destination in &ids[index + 1..] {
                candidate = candidate
                    .with_return_feed(source, destination, 0.5)
                    .unwrap();
            }
        }
        assert_eq!(candidate.return_feeds.len(), MAX_RETURN_FEEDS);
        assert_eq!(
            Project::validate_return_feed_snapshot(&candidate)
                .unwrap()
                .len(),
            120
        );
        let before = candidate.clone();
        assert_eq!(
            candidate.with_return_feed(ids[15], ids[0], 1.0),
            Err(Invalid("return feed limit reached"))
        );
        assert_eq!(candidate, before);
        let empty = editor.project().snapshot();
        let one = empty.with_return_feed(ids[0], ids[1], 1.0).unwrap();
        assert_eq!(
            one.with_return_feed(ids[1], ids[0], 1.0),
            Err(Invalid("routing cycles are unsupported"))
        );
        assert_eq!(
            empty.with_return_feed(ids[0], ids[0], 1.0),
            Err(Invalid("routing cycles are unsupported"))
        );
        for gain in [-1.0, 2.1, f32::NAN, f32::INFINITY] {
            assert!(empty.with_return_feed(ids[0], ids[1], gain).is_err());
        }
        for id in [0, ids[0].value(), one.next_id] {
            let mut invalid = one.clone();
            invalid.return_feeds[0].id = id;
            assert!(Project::validate_return_feed_snapshot(&invalid).is_err());
        }
        let mut duplicate = one.clone();
        let mut second = duplicate.return_feeds[0];
        second.id = duplicate.next_id;
        duplicate.next_id += 1;
        duplicate.return_feeds.push(second);
        assert_eq!(
            Project::validate_return_feed_snapshot(&duplicate),
            Err(Invalid("duplicate return feed"))
        );
        let mut exhausted = empty.clone();
        exhausted.next_id = u64::MAX;
        assert_eq!(
            exhausted.with_return_feed(ids[0], ids[1], 1.0),
            Err(Invalid("project id space exhausted"))
        );
    }

    #[test]
    fn canonical_history_carries_identity_and_removal_cleans_both_ends() {
        let (editor, ids) = fixture(3);
        let candidate = editor
            .project()
            .snapshot()
            .with_return_feed(ids[0], ids[1], 0.5)
            .unwrap()
            .with_return_feed(ids[1], ids[2], 0.25)
            .unwrap();
        let metadata = Project::from_snapshot(candidate.clone()).unwrap();
        assert_eq!(metadata.snapshot(), candidate);
        let mut history = Editor::new(metadata.clone(), 8).unwrap();
        history.execute(Command::MasterGain { gain: 0.25 }).unwrap();
        assert_eq!(
            history.project().authored_routing(),
            metadata.authored_routing()
        );
        assert!(history.undo());
        assert_eq!(history.project(), &metadata);
        assert!(history.redo());
        history.execute(Command::RemoveReturnTrack(ids[1])).unwrap();
        assert!(history.project().authored_routing().is_empty());
        assert_eq!(history.project().snapshot().next_id, candidate.next_id);
        assert!(history.undo());
        assert_eq!(
            history.project().snapshot().return_feeds,
            candidate.return_feeds
        );
        assert!(history.redo());
        assert!(history.project().authored_routing().is_empty());
    }
}
