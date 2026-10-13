//! Return-feed candidate metadata. Nonempty canonical adoption remains closed.
use crate::{
    Created, Invalid, Project, ROUTING_UNSUPPORTED, ReturnId, RouteId, RoutingEdge, RoutingOwner,
    RoutingSignal, SessionCellContentSnapshot, Snapshot, TrackContentSnapshot,
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

/// Transport-only metadata, not a playable Project or another history owner.
#[derive(Clone, Debug, PartialEq)]
pub struct RoutingSnapshot {
    pub project: Snapshot,
    pub return_feeds: Vec<ReturnFeedSnapshot>,
}
impl RoutingSnapshot {
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
        if !self
            .project
            .return_tracks
            .iter()
            .any(|r| r.id == source.value())
            || !self
                .project
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
                let id = candidate.project.next_id;
                candidate.project.next_id = id
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
    pub fn snapshot_with_return_feeds(&self) -> RoutingSnapshot {
        RoutingSnapshot {
            project: self.snapshot(),
            return_feeds: self
                .routing
                .iter()
                .map(|feed| ReturnFeedSnapshot {
                    id: feed.id.value(),
                    source: feed.source.value(),
                    destination: feed.destination.value(),
                    gain: feed.gain,
                })
                .collect(),
        }
    }

    /// Validate global identities, endpoints, gain, pair uniqueness and the
    /// complete DAG. Returned records are metadata, not playback admission.
    pub fn validate_return_feed_snapshot(
        snapshot: &RoutingSnapshot,
    ) -> Result<Vec<ReturnFeed>, Invalid> {
        if snapshot.return_feeds.len() > MAX_RETURN_FEEDS {
            return Err(Invalid("return feed limit reached"));
        }
        let base = Self::from_snapshot(snapshot.project.clone())?;
        let mut seen = existing_ids(&snapshot.project);
        let mut pairs = BTreeSet::new();
        let mut feeds = Vec::with_capacity(snapshot.return_feeds.len());
        let mut edges = Vec::with_capacity(snapshot.return_feeds.len());
        for feed in &snapshot.return_feeds {
            if feed.id == 0 || feed.id >= snapshot.project.next_id || !seen.insert(feed.id) {
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
        Ok(feeds)
    }

    pub fn from_routing_snapshot(snapshot: RoutingSnapshot) -> Result<Self, Invalid> {
        let feeds = Self::validate_return_feed_snapshot(&snapshot)?;
        if !feeds.is_empty() {
            return Err(Invalid(ROUTING_UNSUPPORTED));
        }
        Self::from_snapshot(snapshot.project)
    }

    pub(crate) fn set_return_feed_closed(
        &mut self,
        source: ReturnId,
        destination: ReturnId,
        gain: f32,
    ) -> Result<Created, Invalid> {
        let candidate =
            self.snapshot_with_return_feeds()
                .with_return_feed(source, destination, gain)?;
        let feeds = Self::validate_return_feed_snapshot(&candidate)?;
        if !feeds.is_empty() {
            return Err(Invalid(ROUTING_UNSUPPORTED));
        }
        self.routing = feeds;
        self.next_id = candidate.project.next_id;
        Ok(Created::None)
    }
}

fn existing_ids(snapshot: &Snapshot) -> BTreeSet<u64> {
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
        let legacy = before.snapshot_with_return_feeds();
        let first = legacy.with_return_feed(ids[0], ids[1], 0.25).unwrap();
        assert_eq!(first.return_feeds[0].id, legacy.project.next_id);
        let changed = first.with_return_feed(ids[0], ids[1], 2.0).unwrap();
        assert_eq!(first.return_feeds[0].id, changed.return_feeds[0].id);
        assert_eq!(first.project.next_id, changed.project.next_id);
        let removed = changed.with_return_feed(ids[0], ids[1], 0.0).unwrap();
        assert!(removed.return_feeds.is_empty());
        assert_eq!(removed.project.next_id, changed.project.next_id);
        let recreated = removed.with_return_feed(ids[0], ids[1], 1.0).unwrap();
        assert!(recreated.return_feeds[0].id > first.return_feeds[0].id);
        assert_eq!(
            Project::from_routing_snapshot(first),
            Err(Invalid(ROUTING_UNSUPPORTED))
        );
        assert_eq!(
            editor.execute(Command::SetReturnFeed {
                source: ids[0],
                destination: ids[1],
                gain: 0.25
            }),
            Err(Invalid(ROUTING_UNSUPPORTED))
        );
        assert_eq!(editor.project(), &before);
        assert!(editor.can_redo());
        editor
            .execute(Command::SetReturnFeed {
                source: ids[0],
                destination: ids[1],
                gain: 0.0,
            })
            .unwrap();
        assert!(editor.can_redo());
        assert!(editor.redo());
    }

    #[test]
    fn complete_dag_bound_cycles_invalid_gain_and_global_ids_are_atomic() {
        let (editor, ids) = fixture(16);
        let mut candidate = editor.project().snapshot_with_return_feeds();
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
        let empty = editor.project().snapshot_with_return_feeds();
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
        for id in [0, ids[0].value(), one.project.next_id] {
            let mut invalid = one.clone();
            invalid.return_feeds[0].id = id;
            assert!(Project::validate_return_feed_snapshot(&invalid).is_err());
        }
        let mut duplicate = one.clone();
        let mut second = duplicate.return_feeds[0];
        second.id = duplicate.project.next_id;
        duplicate.project.next_id += 1;
        duplicate.return_feeds.push(second);
        assert_eq!(
            Project::validate_return_feed_snapshot(&duplicate),
            Err(Invalid("duplicate return feed"))
        );
        let mut exhausted = empty.clone();
        exhausted.project.next_id = u64::MAX;
        assert_eq!(
            exhausted.with_return_feed(ids[0], ids[1], 1.0),
            Err(Invalid("project id space exhausted"))
        );
    }

    #[test]
    fn unadmitted_metadata_history_carries_identity_and_removal_cleans_both_ends() {
        let (editor, ids) = fixture(3);
        let candidate = editor
            .project()
            .snapshot_with_return_feeds()
            .with_return_feed(ids[0], ids[1], 0.5)
            .unwrap()
            .with_return_feed(ids[1], ids[2], 0.25)
            .unwrap();
        // Test-only metadata fixture; no production constructor admits these feeds.
        let feeds = Project::validate_return_feed_snapshot(&candidate).unwrap();
        let mut metadata = Project::from_snapshot(candidate.project.clone()).unwrap();
        metadata.routing = feeds;
        assert_eq!(metadata.snapshot_with_return_feeds(), candidate);
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
        assert_eq!(
            history.project().snapshot().next_id,
            candidate.project.next_id
        );
        assert!(history.undo());
        assert_eq!(
            history.project().snapshot_with_return_feeds().return_feeds,
            candidate.return_feeds
        );
        assert!(history.redo());
        assert!(history.project().authored_routing().is_empty());
    }
}
