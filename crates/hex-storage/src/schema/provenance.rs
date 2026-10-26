//! Strict schema-18 provenance envelope; no source-format interpretation.
use super::*;
use hex_project::{ImportProvenance, MAX_IMPORT_DOCUMENT_BYTES};
use serde::de::{SeqAccess, Visitor};
use std::fmt;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV18 {
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
    #[serde(deserialize_with = "required_provenance")]
    pub(super) import_provenance: Option<ProvenanceDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProvenanceDto {
    kind: String,
    version: u32,
    adapter_version: u32,
    #[serde(deserialize_with = "bounded_bytes")]
    original_bytes: Vec<u8>,
}
pub(super) fn required_provenance<'de, D: serde::Deserializer<'de>>(
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
    #[cfg(test)]
    pub(crate) fn from_project(project: &Project) -> Result<Self, StorageError> {
        if project.has_gain_automation() || project.has_gain_touch_metadata() {
            return Err(StorageError::Invalid(
                "older schema cannot save gain automation",
            ));
        }
        if !project.authored_routing().is_empty() {
            return Err(StorageError::Invalid(hex_project::ROUTING_UNSUPPORTED));
        }
        Self::legacy_fields(project)
    }
    // Schema20 reuses these fields, but owns serialization of every feed.
    // All older envelope entry points retain their refusal above.
    pub(super) fn legacy_fields(project: &Project) -> Result<Self, StorageError> {
        let base = DocumentV19::from_project(project)?;
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
        doc.into_project()
    }
    pub(super) fn into_project(self) -> Result<Project, StorageError> {
        let doc = self;
        let provenance = doc
            .import_provenance
            .map(|p| {
                ImportProvenance::new(&p.kind, p.version, p.adapter_version, p.original_bytes)
                    .map_err(|e| StorageError::Invalid(e.0))
            })
            .transpose()?;
        let base = DocumentV19 {
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
            version: 19,
        };
        let mut snapshot = DocumentV19::decode(&serde_json::to_vec(&base)?)?.snapshot();
        snapshot.import_provenance = provenance;
        Project::from_legacy_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
    }
}
