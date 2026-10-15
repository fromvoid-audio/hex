//! Strict schema19. Structurally checked proposals remain closed to playback.
use super::*;
use hex_project::{MAX_AUTHORED_ROUTES, RoutingEdge, RoutingOwner, RoutingSignal};
use provenance::{DocumentV18, ProvenanceDto};
use serde::de::{SeqAccess, Visitor};
use std::fmt;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV19 {
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
    #[serde(deserialize_with = "bounded_routes")]
    routing: Vec<EdgeDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "id",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum OwnerDto {
    Track(u64),
    Return(u64),
    Main,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SignalDto {
    Audio { channels: u8 },
    Midi,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EdgeDto {
    source: OwnerDto,
    target: OwnerDto,
    signal: SignalDto,
}

fn bounded_routes<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<EdgeDto>, D::Error> {
    struct Edges;
    impl<'de> Visitor<'de> for Edges {
        type Value = Vec<EdgeDto>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a bounded routing edge array")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<EdgeDto>, A::Error> {
            let mut edges = Vec::new();
            while let Some(edge) = seq.next_element()? {
                if edges.len() == MAX_AUTHORED_ROUTES {
                    return Err(serde::de::Error::custom(
                        "authored routing edge limit reached",
                    ));
                }
                edges.push(edge);
            }
            Ok(edges)
        }
    }
    d.deserialize_seq(Edges)
}
impl OwnerDto {
    fn resolve(self, project: &Project) -> Result<RoutingOwner, StorageError> {
        match self {
            Self::Track(id) => project
                .tracks()
                .iter()
                .find(|t| t.id().value() == id)
                .map(|t| RoutingOwner::Track(t.id())),
            Self::Return(id) => project
                .return_tracks()
                .iter()
                .find(|t| t.id().value() == id)
                .map(|t| RoutingOwner::Return(t.id())),
            Self::Main => Some(RoutingOwner::Main),
        }
        .ok_or(StorageError::Invalid("routing owner no longer exists"))
    }
}
impl DocumentV19 {
    #[cfg(test)]
    pub(crate) fn from_project(project: &Project) -> Result<Self, StorageError> {
        // The legacy codec also refuses any non-legacy authored topology.
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
            version: 19,
            routing: Vec::new(),
        })
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
        let doc: Self = serde_json::from_slice(bytes)?;
        if doc.format != "hex-project" || doc.version != 19 {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        let base = DocumentV18 {
            format: doc.format,
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
            version: 18,
        };
        let project = base.into_project()?;
        let routes = doc
            .routing
            .into_iter()
            .map(|edge| {
                Ok(RoutingEdge {
                    source: edge.source.resolve(&project)?,
                    target: edge.target.resolve(&project)?,
                    signal: match edge.signal {
                        SignalDto::Audio { channels } => RoutingSignal::Audio { channels },
                        SignalDto::Midi => RoutingSignal::Midi,
                    },
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        Project::from_snapshot_with_routing(project.snapshot(), routes)
            .map_err(|e| StorageError::Invalid(e.0))
    }
}
