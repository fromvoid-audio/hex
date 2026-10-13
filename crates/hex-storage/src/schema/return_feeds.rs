//! Reserved schema20 Return feeds; nonempty load/adoption remains closed.
use super::*;
use hex_project::{MAX_RETURN_FEEDS, ReturnFeedSnapshot, Snapshot};
use provenance::{DocumentV18, ProvenanceDto};
use serde::de::{SeqAccess, Visitor};
use std::fmt;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV20 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    tracks: Vec<TrackV17>,
    return_tracks: Vec<ReturnTrackDtoV17>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    loop_region: Option<LoopRegionDto>,
    master_gain: f32,
    tempo_points: Vec<TempoPointDto>,
    meter_points: Vec<MeterPointDto>,
    session: SessionDtoV12,
    #[serde(deserialize_with = "provenance::required_provenance")]
    import_provenance: Option<ProvenanceDto>,
    #[serde(deserialize_with = "bounded_feeds")]
    routing: Vec<FeedDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum FeedDto {
    ReturnFeed {
        id: u64,
        source: u64,
        destination: u64,
        gain: f32,
    },
}
fn bounded_feeds<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<FeedDto>, D::Error> {
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
        // Older envelope construction refuses any nonempty canonical routes.
        let base = DocumentV18::from_project(project)?;
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
            routing: Vec::new(),
        })
    }
    fn into_snapshot(self) -> Result<Snapshot, StorageError> {
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
        Project::from_snapshot(doc.into_snapshot()?).map_err(|e| StorageError::Invalid(e.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor};

    #[test]
    fn typed_candidate_serialization_roundtrips_without_playback_admission() {
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
        // Transport-only fixture: production encoding/loading never admits it.
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
        assert!(matches!(
            DocumentV20::decode(&bytes),
            Err(StorageError::Invalid(hex_project::ROUTING_UNSUPPORTED))
        ));
        assert!(editor.project().authored_routing().is_empty());
    }
}
