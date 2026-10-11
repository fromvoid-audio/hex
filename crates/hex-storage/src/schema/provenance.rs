//! Strict schema-18 provenance envelope; no source-format interpretation.
use super::*;
use hex_project::{ImportProvenance, MAX_IMPORT_DOCUMENT_BYTES};
use serde::de::{SeqAccess, Visitor};
use std::fmt;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV18 {
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
    #[serde(deserialize_with = "required_provenance")]
    import_provenance: Option<ProvenanceDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProvenanceDto {
    kind: String,
    version: u32,
    adapter_version: u32,
    #[serde(deserialize_with = "bounded_bytes")]
    original_bytes: Vec<u8>,
}
fn required_provenance<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<ProvenanceDto>, D::Error> {
    Option::deserialize(d)
}
fn bounded_bytes<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    struct Bytes;
    impl<'de> Visitor<'de> for Bytes {
        type Value = Vec<u8>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a bounded original-document byte array")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<u8>, A::Error> {
            let mut bytes = Vec::new();
            while let Some(byte) = seq.next_element::<u8>()? {
                if bytes.len() == MAX_IMPORT_DOCUMENT_BYTES {
                    return Err(serde::de::Error::custom(
                        "import document exceeds byte limit",
                    ));
                }
                bytes.push(byte);
            }
            Ok(bytes)
        }
    }
    d.deserialize_seq(Bytes)
}
impl DocumentV18 {
    pub(crate) fn from_project(project: &Project) -> Result<Self, StorageError> {
        let base = DocumentV17::from_project(project)?;
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
            version: 18,
            import_provenance: project.import_provenance().map(|p| ProvenanceDto {
                kind: p.kind().to_owned(),
                version: p.version(),
                adapter_version: p.adapter_version(),
                original_bytes: p.original_bytes().to_vec(),
            }),
        })
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
        let doc: Self = serde_json::from_slice(bytes)?;
        if doc.format != "hex-project" || doc.version != 18 {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        let provenance = doc
            .import_provenance
            .map(|p| {
                ImportProvenance::new(&p.kind, p.version, p.adapter_version, p.original_bytes)
                    .map_err(|e| StorageError::Invalid(e.0))
            })
            .transpose()?;
        let base = DocumentV17 {
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
            version: 17,
        };
        let mut snapshot = DocumentV17::decode(&serde_json::to_vec(&base)?)?.snapshot();
        snapshot.import_provenance = provenance;
        Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
    }
}
