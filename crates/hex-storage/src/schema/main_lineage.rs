//! Frozen main17–19 gain envelope, distinct from provenance/routing lineages.
//! Required top-level fields disambiguate shapes; typed decoding retains strict
//! duplicate/unknown-field refusal before any normalization.
use super::*;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MainDocument {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    tracks: Vec<MainTrack>,
    return_tracks: Vec<MainReturn>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    loop_region: Option<LoopRegionDto>,
    master_gain: f32,
    tempo_points: Vec<TempoPointDto>,
    meter_points: Vec<MeterPointDto>,
    session: SessionDtoV12,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MainTrack {
    id: u64,
    name: String,
    gain: f32,
    balance: f32,
    muted: bool,
    soloed: bool,
    audio_clips: Vec<ClipV7>,
    midi_clips: Vec<MidiClipDtoV10>,
    devices: Vec<MainDevice>,
    #[serde(deserialize_with = "required_instrument")]
    instrument: Option<MainDevice>,
    #[serde(deserialize_with = "deserialize_required_instrument_position")]
    instrument_position: Option<usize>,
    sends: Vec<SendDtoV17>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MainReturn {
    id: u64,
    name: String,
    gain: f32,
    muted: bool,
    devices: Vec<MainDevice>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MainDevice {
    id: u64,
    bypassed: bool,
    type_id: String,
    state_version: u32,
    payload: Vec<u8>,
    #[serde(default)]
    input_gain_millidb: i32,
    #[serde(default)]
    output_gain_millidb: i32,
}
fn required_instrument<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<MainDevice>, D::Error> {
    Option::deserialize(d)
}
pub(super) fn decode_gain(bytes: &[u8]) -> Result<Project, StorageError> {
    let doc: MainDocument = serde_json::from_slice(bytes)?;
    if doc.format != "hex-project" || !matches!(doc.version, 17..=19) {
        return Err(StorageError::Invalid(
            "unsupported project format or version",
        ));
    }
    // Only typed DTO output is normalized: foreign duplicate keys have already
    // been refused, and no unknown fields or hybrid lineage are discarded.
    let mut value = serde_json::to_value(doc)?;
    let gains = host_gain::take_both_gains(&mut value)?;
    value["version"] = 17.into();
    host_gain::install_both_gains(DocumentV17::decode(&serde_json::to_vec(&value)?)?, gains)
}
pub(super) fn decode18(bytes: &[u8]) -> Result<Project, StorageError> {
    disjoint(provenance::DocumentV18::decode(bytes), decode_gain(bytes))
}
pub(super) fn decode19(bytes: &[u8]) -> Result<Project, StorageError> {
    disjoint(routing::DocumentV19::decode(bytes), decode_gain(bytes))
}
fn disjoint(
    first: Result<Project, StorageError>,
    second: Result<Project, StorageError>,
) -> Result<Project, StorageError> {
    match (first, second) {
        (Ok(project), Err(_)) | (Err(_), Ok(project)) => Ok(project),
        _ => Err(StorageError::Invalid(
            "unsupported or ambiguous project schema shape",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_envelope_writers_refuse_both_nondefault_gain_domains() {
        let project =
            decode_gain(include_bytes!("../../tests/fixtures/lineages/main19.json")).unwrap();
        assert!(provenance::DocumentV18::from_project(&project).is_err());
        assert!(routing::DocumentV19::from_project(&project).is_err());
    }
}
