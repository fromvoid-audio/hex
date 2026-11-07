//! Strict schema24 preserves playable typed inputs with all schema23 metadata.
use super::*;
use hex_project::{AudioFeedSnapshot, MidiRouteSnapshot, RoutingOwner};
use serde::de::{SeqAccess, Visitor};
use std::fmt;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV24 {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetV2>,
    #[serde(deserialize_with = "gain_touch::bounded_tracks")]
    pub(super) tracks: Vec<gain_touch::TrackV22>,
    pub(super) return_tracks: Vec<ReturnTrackDtoV17>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    pub(super) loop_region: Option<LoopRegionDto>,
    pub(super) master_gain: f32,
    pub(super) tempo_points: Vec<TempoPointDto>,
    pub(super) meter_points: Vec<MeterPointDto>,
    pub(super) session: SessionDtoV12,
    #[serde(deserialize_with = "provenance::required_provenance")]
    pub(super) import_provenance: Option<provenance::ProvenanceDto>,
    #[serde(deserialize_with = "return_feeds::bounded_feeds")]
    pub(super) routing: Vec<return_feeds::FeedDto>,
    #[serde(deserialize_with = "bounded_audio")]
    pub(super) audio_feeds: Vec<AudioDto>,
    #[serde(deserialize_with = "bounded_midi")]
    pub(super) midi_routes: Vec<MidiDto>,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SourceDto {
    Track { id: u64 },
    Return { id: u64 },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AudioDto {
    id: u64,
    source: SourceDto,
    destination: u64,
    gain: f32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MidiDto {
    id: u64,
    source: u64,
    destination: u64,
}
fn bounded<'de, D, T>(d: D, limit: usize) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Items<T> {
        limit: usize,
        marker: std::marker::PhantomData<T>,
    }
    impl<'de, T: Deserialize<'de>> Visitor<'de> for Items<T> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("bounded typed routes")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut items = Vec::new();
            while let Some(item) = seq.next_element()? {
                if items.len() == self.limit {
                    return Err(serde::de::Error::custom(
                        "authored routing edge limit reached",
                    ));
                }
                items.push(item);
            }
            Ok(items)
        }
    }
    d.deserialize_seq(Items {
        limit,
        marker: std::marker::PhantomData,
    })
}
pub(super) fn bounded_audio<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<AudioDto>, D::Error> {
    bounded(d, hex_project::MAX_AUDIO_FEEDS)
}
pub(super) fn bounded_midi<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<MidiDto>, D::Error> {
    bounded(d, hex_project::MAX_MIDI_ROUTES)
}
#[derive(Deserialize)]
struct MidiRoutes(#[serde(deserialize_with = "bounded_midi")] Vec<MidiDto>);

pub(super) fn decode_midi_routes(
    value: serde_json::Value,
) -> Result<Vec<MidiRouteSnapshot>, StorageError> {
    let MidiRoutes(routes) = serde_json::from_value(value)?;
    Ok(routes
        .into_iter()
        .map(|route| MidiRouteSnapshot {
            id: route.id,
            source: route.source,
            destination: route.destination,
        })
        .collect())
}

impl DocumentV24 {
    pub(crate) fn from_project(project: &Project) -> Result<Self, StorageError> {
        let base = gain_touch::DocumentV22::fields_for_new_envelope(project)?;
        let audio_feeds = project
            .audio_feeds()
            .iter()
            .map(|feed| {
                Ok(AudioDto {
                    id: feed.id().value(),
                    source: match feed.source() {
                        RoutingOwner::Track(id) => SourceDto::Track { id: id.value() },
                        RoutingOwner::Return(id) => SourceDto::Return { id: id.value() },
                        RoutingOwner::Main => {
                            return Err(StorageError::Invalid("main cannot feed a track input"));
                        }
                    },
                    destination: feed.destination().value(),
                    gain: feed.gain(),
                })
            })
            .collect::<Result<_, StorageError>>()?;
        Ok(Self {
            format: base.format,
            version: 24,
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
            routing: base.routing,
            audio_feeds,
            midi_routes: project
                .midi_routes()
                .iter()
                .map(|route| MidiDto {
                    id: route.id().value(),
                    source: route.source().value(),
                    destination: route.destination().value(),
                })
                .collect(),
        })
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
        let doc: Self = serde_json::from_slice(bytes)?;
        if doc.format != "hex-project" || doc.version != 24 {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        Self::into_snapshot(doc).and_then(|snapshot| {
            Project::from_legacy_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
        })
    }
    pub(super) fn into_snapshot(doc: Self) -> Result<Snapshot, StorageError> {
        let base = gain_touch::DocumentV22 {
            format: doc.format,
            version: 22,
            sample_rate: doc.sample_rate,
            next_id: doc.next_id,
            assets: doc.assets,
            tracks: doc.tracks,
            return_tracks: doc.return_tracks,
            loop_region: doc.loop_region,
            master_gain: doc.master_gain,
            tempo_points: doc.tempo_points,
            meter_points: doc.meter_points,
            session: doc.session,
            import_provenance: doc.import_provenance,
            routing: doc.routing,
        };
        let mut snapshot = base.into_snapshot()?;
        let owners = Project::from_legacy_snapshot(snapshot.clone())
            .map_err(|error| StorageError::Invalid(error.0))?;
        snapshot.audio_feeds = doc
            .audio_feeds
            .into_iter()
            .map(|feed| {
                Ok(AudioFeedSnapshot {
                    id: feed.id,
                    source: match feed.source {
                        SourceDto::Track { id } => RoutingOwner::Track(
                            owners
                                .tracks()
                                .iter()
                                .find(|owner| owner.id().value() == id)
                                .ok_or(StorageError::Invalid("routing owner no longer exists"))?
                                .id(),
                        ),
                        SourceDto::Return { id } => RoutingOwner::Return(
                            owners
                                .return_tracks()
                                .iter()
                                .find(|owner| owner.id().value() == id)
                                .ok_or(StorageError::Invalid("routing owner no longer exists"))?
                                .id(),
                        ),
                    },
                    destination: feed.destination,
                    gain: feed.gain,
                })
            })
            .collect::<Result<_, StorageError>>()?;
        snapshot.midi_routes = doc
            .midi_routes
            .into_iter()
            .map(|route| MidiRouteSnapshot {
                id: route.id,
                source: route.source,
                destination: route.destination,
            })
            .collect();
        Ok(snapshot)
    }
}
