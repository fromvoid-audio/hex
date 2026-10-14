//! Two strict schema18 lineages and a combined schema22 envelope.
//! Wire DTOs are frozen independently; only already-validated typed data is
//! normalized through existing canonical migration/validation machinery.
use super::*;
use std::collections::BTreeMap;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Gain18 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    tracks: Vec<GainTrack18>,
    return_tracks: Vec<GainReturn18>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    loop_region: Option<LoopRegionDto>,
    master_gain: f32,
    tempo_points: Vec<TempoPointDto>,
    meter_points: Vec<MeterPointDto>,
    session: SessionDtoV12,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GainTrack18 {
    id: u64,
    name: String,
    gain: f32,
    balance: f32,
    muted: bool,
    soloed: bool,
    audio_clips: Vec<ClipV7>,
    midi_clips: Vec<MidiClipDtoV10>,
    devices: Vec<GainDevice18>,
    #[serde(deserialize_with = "required_instrument")]
    instrument: Option<GainDevice18>,
    #[serde(deserialize_with = "deserialize_required_instrument_position")]
    instrument_position: Option<usize>,
    sends: Vec<SendDtoV17>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GainReturn18 {
    id: u64,
    name: String,
    gain: f32,
    muted: bool,
    devices: Vec<GainDevice18>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GainDevice18 {
    id: u64,
    bypassed: bool,
    type_id: String,
    state_version: u32,
    payload: Vec<u8>,
    #[serde(default)]
    output_gain_millidb: i32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document22 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    #[serde(deserialize_with = "bounded_tracks22")]
    tracks: Vec<GainTrack22>,
    return_tracks: Vec<GainReturn22>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    loop_region: Option<LoopRegionDto>,
    master_gain: f32,
    tempo_points: Vec<TempoPointDto>,
    meter_points: Vec<MeterPointDto>,
    session: SessionDtoV12,
    #[serde(deserialize_with = "provenance::required_provenance")]
    import_provenance: Option<provenance::ProvenanceDto>,
    #[serde(deserialize_with = "return_feeds::bounded_feeds")]
    routing: Vec<return_feeds::FeedDto>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GainTrack22 {
    id: u64,
    name: String,
    gain: f32,
    balance: f32,
    muted: bool,
    soloed: bool,
    audio_clips: Vec<ClipV7>,
    midi_clips: Vec<MidiClipDtoV10>,
    devices: Vec<GainDevice22>,
    #[serde(deserialize_with = "required_instrument")]
    instrument: Option<GainDevice22>,
    #[serde(deserialize_with = "deserialize_required_instrument_position")]
    instrument_position: Option<usize>,
    sends: Vec<SendDtoV17>,
    #[serde(deserialize_with = "gain_envelopes::bounded_gain_points")]
    gain_envelope: Vec<gain_envelopes::GainPointDto>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GainReturn22 {
    id: u64,
    name: String,
    gain: f32,
    muted: bool,
    devices: Vec<GainDevice22>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GainDevice22 {
    id: u64,
    bypassed: bool,
    type_id: String,
    state_version: u32,
    payload: Vec<u8>,
    output_gain_millidb: i32,
}

/// Presence (even null) is part of the provenance lineage. A removed key can
/// produce a valid gain18 shape; absent evidence cannot prove source lineage.
pub(super) fn decode18(bytes: &[u8]) -> Result<Project, StorageError> {
    let provenance = provenance::DocumentV18::decode(bytes);
    let gain = decode_gain18(bytes);
    match (provenance, gain) {
        (Ok(project), Err(_)) | (Err(_), Ok(project)) => Ok(project),
        _ => Err(StorageError::Invalid(
            "unsupported or ambiguous schema18 shape",
        )),
    }
}
fn decode_gain18(bytes: &[u8]) -> Result<Project, StorageError> {
    let doc: Gain18 = serde_json::from_slice(bytes)?;
    if doc.format != "hex-project" || doc.version != 18 {
        return Err(StorageError::Invalid(
            "unsupported project format or version",
        ));
    }
    let mut value = serde_json::to_value(doc)?;
    let gains = take_gains(&mut value)?;
    value["version"] = 17.into();
    install_gains(DocumentV17::decode(&serde_json::to_vec(&value)?)?, gains)
}
pub(super) fn decode22(bytes: &[u8]) -> Result<Project, StorageError> {
    let doc: Document22 = serde_json::from_slice(bytes)?;
    if doc.format != "hex-project" || doc.version != 22 {
        return Err(StorageError::Invalid(
            "unsupported project format or version",
        ));
    }
    let mut value = serde_json::to_value(doc)?;
    let gains = take_gains(&mut value)?;
    value["version"] = 21.into();
    install_gains(
        gain_envelopes::DocumentV21::decode(&serde_json::to_vec(&value)?)?,
        gains,
    )
}
// Only called with our typed wire DTO serialization or canonical encoder output;
// never with Value deserialized from foreign bytes (which would lose duplicates).
fn for_devices(
    value: &mut serde_json::Value,
    mut visit: impl FnMut(&mut serde_json::Value) -> Result<(), StorageError>,
) -> Result<(), StorageError> {
    for track in value["tracks"]
        .as_array_mut()
        .ok_or(StorageError::Invalid("invalid tracks"))?
    {
        for device in track["devices"]
            .as_array_mut()
            .ok_or(StorageError::Invalid("invalid devices"))?
        {
            visit(device)?;
        }
        if !track["instrument"].is_null() {
            visit(&mut track["instrument"])?;
        }
    }
    for bus in value["return_tracks"]
        .as_array_mut()
        .ok_or(StorageError::Invalid("invalid returns"))?
    {
        for device in bus["devices"]
            .as_array_mut()
            .ok_or(StorageError::Invalid("invalid devices"))?
        {
            visit(device)?;
        }
    }
    Ok(())
}
fn take_gains(value: &mut serde_json::Value) -> Result<BTreeMap<u64, i32>, StorageError> {
    let mut gains = BTreeMap::new();
    for_devices(value, |device| {
        let id = device["id"]
            .as_u64()
            .ok_or(StorageError::Invalid("invalid device id"))?;
        let gain = device
            .as_object_mut()
            .ok_or(StorageError::Invalid("invalid device"))?
            .remove("output_gain_millidb")
            .and_then(|v| v.as_i64())
            .ok_or(StorageError::Invalid("invalid host gain"))?;
        let gain = i32::try_from(gain).map_err(|_| StorageError::Invalid("invalid host gain"))?;
        if !(-60000..=24000).contains(&gain) || gains.insert(id, gain).is_some() {
            return Err(StorageError::Invalid(
                "invalid host gain or duplicate device id",
            ));
        }
        Ok(())
    })?;
    Ok(gains)
}
fn install_gains(project: Project, mut gains: BTreeMap<u64, i32>) -> Result<Project, StorageError> {
    let mut snapshot = project.snapshot();
    for device in snapshot
        .tracks
        .iter_mut()
        .flat_map(|t| t.devices.iter_mut().chain(t.instrument.iter_mut()))
        .chain(
            snapshot
                .return_tracks
                .iter_mut()
                .flat_map(|t| t.devices.iter_mut()),
        )
    {
        let gain = gains
            .remove(&device.id)
            .ok_or(StorageError::Invalid("missing host gain"))?;
        device.state = device
            .state
            .clone()
            .with_output_gain_millidb(gain)
            .map_err(|_| StorageError::Invalid("invalid host gain"))?;
    }
    if !gains.is_empty() {
        return Err(StorageError::Invalid("unknown host gain device"));
    }
    Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}
pub(super) fn has_gain(project: &Project) -> bool {
    project
        .tracks()
        .iter()
        .flat_map(|t| t.devices().iter().chain(t.instrument()))
        .chain(project.return_tracks().iter().flat_map(|t| t.devices()))
        .any(|d| d.state().output_gain_millidb() != 0)
}
pub(super) fn encode22(project: &Project) -> Result<Vec<u8>, StorageError> {
    let mut snapshot = project.snapshot();
    let mut gains = BTreeMap::new();
    for device in snapshot
        .tracks
        .iter_mut()
        .flat_map(|t| t.devices.iter_mut().chain(t.instrument.iter_mut()))
        .chain(
            snapshot
                .return_tracks
                .iter_mut()
                .flat_map(|t| t.devices.iter_mut()),
        )
    {
        gains.insert(device.id, device.state.output_gain_millidb());
        device.state = device
            .state
            .clone()
            .with_output_gain_millidb(0)
            .map_err(|_| StorageError::Invalid("invalid host gain"))?;
    }
    let unity = Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))?;
    let mut value = serde_json::to_value(gain_envelopes::DocumentV21::from_project(&unity)?)?;
    value["version"] = 22.into();
    for_devices(&mut value, |device| {
        let id = device["id"]
            .as_u64()
            .ok_or(StorageError::Invalid("invalid device id"))?;
        device["output_gain_millidb"] = (*gains
            .get(&id)
            .ok_or(StorageError::Invalid("missing host gain"))?)
        .into();
        Ok(())
    })?;
    let doc: Document22 = serde_json::from_value(value)?;
    crate::encode_document(&doc)
}

fn required_instrument<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

fn bounded_tracks22<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<GainTrack22>, D::Error> {
    struct Tracks;
    impl<'de> serde::de::Visitor<'de> for Tracks {
        type Value = Vec<GainTrack22>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded tracks and project gain points")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut tracks = Vec::new();
            let mut count = 0_usize;
            while let Some(track) = seq.next_element::<GainTrack22>()? {
                if tracks.len() == hex_project::MAX_TRACKS {
                    return Err(serde::de::Error::custom("track limit reached"));
                }
                count = count
                    .checked_add(track.gain_envelope.len())
                    .ok_or_else(|| serde::de::Error::custom("gain point count overflow"))?;
                if count > hex_project::MAX_GAIN_POINTS {
                    return Err(serde::de::Error::custom(
                        "project gain envelope point limit reached",
                    ));
                }
                tracks.push(track);
            }
            Ok(tracks)
        }
    }
    d.deserialize_seq(Tracks)
}
