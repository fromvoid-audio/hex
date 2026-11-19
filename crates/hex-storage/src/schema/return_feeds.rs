//! Strict schema20 canonical Return-feed envelope.
use super::*;
use hex_project::{MAX_RETURN_FEEDS, ReturnFeedSnapshot, Snapshot};
use provenance::{DocumentV18, ProvenanceDto};
use serde::de::{SeqAccess, Visitor};
use std::fmt;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV20 {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetV2>,
    pub(super) tracks: Vec<TrackV17>,
    pub(super) return_tracks: Vec<ReturnTrackDtoV17>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    pub(super) loop_region: Option<LoopRegionDto>,
    pub(super) master_gain: f32,
    pub(super) tempo_points: Vec<TempoPointDto>,
    pub(super) meter_points: Vec<MeterPointDto>,
    pub(super) session: SessionDtoV12,
    #[serde(deserialize_with = "provenance::required_provenance")]
    pub(super) import_provenance: Option<ProvenanceDto>,
    #[serde(deserialize_with = "bounded_feeds")]
    pub(super) routing: Vec<FeedDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum FeedDto {
    ReturnFeed {
        id: u64,
        source: u64,
        destination: u64,
        gain: f32,
    },
}
pub(super) fn bounded_feeds<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<FeedDto>, D::Error> {
    struct Feeds;
    impl<'de> Visitor<'de> for Feeds {
        type Value = Vec<FeedDto>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("at most120 typed Return feeds")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<FeedDto>, A::Error> {
            let mut feeds = Vec::new();
            while let Some(feed) = seq.next_element()? {
                if feeds.len() == MAX_RETURN_FEEDS {
                    return Err(serde::de::Error::custom("return feed limit reached"));
                }
                feeds.push(feed);
            }
            Ok(feeds)
        }
    }
    d.deserialize_seq(Feeds)
}
impl DocumentV20 {
    pub(crate) fn from_project(project: &Project) -> Result<Self, StorageError> {
        if project.has_gain_automation() || project.has_gain_touch_metadata() {
            return Err(StorageError::Invalid(
                "older schema cannot save gain automation",
            ));
        }
        Self::fields_for_new_envelope(project)
    }
    pub(super) fn fields_for_new_envelope(project: &Project) -> Result<Self, StorageError> {
        // This envelope owns canonical feeds; schema21 also owns every gain lane.
        let base = DocumentV18::legacy_fields(project)?;
        Ok(Self {
            format: base.format,
            sample_rate: base.sample_rate,
            next_id: base.next_id,
            assets: base.assets,
            tracks: base.tracks,
            return_tracks: base.return_tracks,
            loop_region: base.loop_region,
            master_gain: base.master_gain,
            tempo_points: base.tempo_points,
            meter_points: base.meter_points,
            session: base.session,
            import_provenance: base.import_provenance,
            version: 20,
            routing: project
                .authored_routing()
                .iter()
                .map(|feed| FeedDto::ReturnFeed {
                    id: feed.id().value(),
                    source: feed.source().value(),
                    destination: feed.destination().value(),
                    gain: feed.gain(),
                })
                .collect(),
        })
    }
    pub(super) fn into_snapshot(self) -> Result<Snapshot, StorageError> {
        if self.format != "hex-project" || self.version != 20 {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        let base = DocumentV18 {
            format: self.format,
            sample_rate: self.sample_rate,
            next_id: self.next_id,
            assets: self.assets,
            tracks: self.tracks,
            return_tracks: self.return_tracks,
            loop_region: self.loop_region,
            master_gain: self.master_gain,
            tempo_points: self.tempo_points,
            meter_points: self.meter_points,
            session: self.session,
            import_provenance: self.import_provenance,
            version: 18,
        };
        let project = base.into_project()?;
        let return_feeds = self
            .routing
            .into_iter()
            .map(|feed| match feed {
                FeedDto::ReturnFeed {
                    id,
                    source,
                    destination,
                    gain,
                } => ReturnFeedSnapshot {
                    id,
                    source,
                    destination,
                    gain,
                },
            })
            .collect();
        let mut snapshot = project.snapshot();
        snapshot.return_feeds = return_feeds;
        Project::validate_return_feed_snapshot(&snapshot)
            .map_err(|e| StorageError::Invalid(e.0))?;
        Ok(snapshot)
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
        let doc: Self = serde_json::from_slice(bytes)?;
        Project::from_legacy_snapshot(doc.into_snapshot()?).map_err(|e| StorageError::Invalid(e.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor};

    #[test]
    fn typed_candidate_serialization_roundtrips_into_canonical_state() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        let mut owners = Vec::new();
        for _ in 0..2 {
            let Created::ReturnTrack(id) = editor
                .execute(Command::AddReturnTrack { name: "bus".into() })
                .unwrap()
            else {
                panic!("return")
            };
            owners.push(id);
        }
        let proposed = editor
            .project()
            .snapshot()
            .with_return_feed(owners[0], owners[1], 0.375)
            .unwrap();
        let feed = proposed.return_feeds[0];
        let mut doc = DocumentV20::from_project(editor.project()).unwrap();
        // Construct a strict transport fixture independently of production encoding.
        doc.next_id = proposed.next_id;
        doc.routing.push(FeedDto::ReturnFeed {
            id: feed.id,
            source: feed.source,
            destination: feed.destination,
            gain: feed.gain,
        });
        let bytes = serde_json::to_vec(&doc).unwrap();
        let decoded: DocumentV20 = serde_json::from_slice(&bytes).unwrap();
        let snapshot = decoded.into_snapshot().unwrap();
        assert_eq!(snapshot, proposed);
        assert_eq!(
            Project::validate_return_feed_snapshot(&snapshot).unwrap()[0].gain(),
            0.375
        );
        let admitted = DocumentV20::decode(&bytes).unwrap();
        assert_eq!(admitted.snapshot(), proposed);
        let encoded = serde_json::to_vec(&DocumentV20::from_project(&admitted).unwrap()).unwrap();
        assert_eq!(DocumentV20::decode(&encoded).unwrap(), admitted);
        assert!(DocumentV18::from_project(&admitted).is_err());
        assert!(editor.project().authored_routing().is_empty());
    }
}
