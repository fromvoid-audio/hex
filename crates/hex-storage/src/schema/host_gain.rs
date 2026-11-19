//! Frozen output-only22 and the strict two-gain23 union envelope.
//! Wire DTOs are frozen independently; only already-validated typed data is
//! normalized through existing canonical migration/validation machinery.
use super::*;
use std::collections::BTreeMap;
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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document23 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    #[serde(deserialize_with = "bounded_tracks23")]
    tracks: Vec<GainTrack23>,
    return_tracks: Vec<GainReturn23>,
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
struct GainTrack23 {
    id: u64,
    name: String,
    gain: f32,
    balance: f32,
    muted: bool,
    soloed: bool,
    audio_clips: Vec<ClipV7>,
    midi_clips: Vec<MidiClipDtoV10>,
    devices: Vec<GainDevice23>,
    #[serde(deserialize_with = "required_instrument")]
    instrument: Option<GainDevice23>,
    #[serde(deserialize_with = "deserialize_required_instrument_position")]
    instrument_position: Option<usize>,
    sends: Vec<SendDtoV17>,
    #[serde(deserialize_with = "gain_envelopes::bounded_gain_points")]
    gain_envelope: Vec<gain_envelopes::GainPointDto>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GainReturn23 {
    id: u64,
    name: String,
    gain: f32,
    muted: bool,
    devices: Vec<GainDevice23>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GainDevice23 {
    id: u64,
    bypassed: bool,
    type_id: String,
    state_version: u32,
    payload: Vec<u8>,
    input_gain_millidb: i32,
    output_gain_millidb: i32,
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
    Project::from_legacy_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}

pub(super) fn take_both_gains(
    value: &mut serde_json::Value,
) -> Result<BTreeMap<u64, (i32, i32)>, StorageError> {
    let mut gains = BTreeMap::new();
    for_devices(value, |device| {
        let id = device["id"]
            .as_u64()
            .ok_or(StorageError::Invalid("invalid device id"))?;
        let fields = device
            .as_object_mut()
            .ok_or(StorageError::Invalid("invalid device"))?;
        let mut read = |field| {
            let gain = fields
                .remove(field)
                .and_then(|v| v.as_i64())
                .and_then(|v| i32::try_from(v).ok())
                .filter(|v| (-60000..=24000).contains(v))
                .ok_or(StorageError::Invalid("invalid host gain"))?;
            Ok::<_, StorageError>(gain)
        };
        let input = read("input_gain_millidb")?;
        let output = read("output_gain_millidb")?;
        if gains.insert(id, (input, output)).is_some() {
            return Err(StorageError::Invalid("duplicate device id"));
        }
        Ok(())
    })?;
    Ok(gains)
}
pub(super) fn install_both_gains(
    project: Project,
    mut gains: BTreeMap<u64, (i32, i32)>,
) -> Result<Project, StorageError> {
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
        let (input, output) = gains
            .remove(&device.id)
            .ok_or(StorageError::Invalid("missing host gain"))?;
        device.state = device
            .state
            .clone()
            .with_input_gain_millidb(input)
            .and_then(|s| s.with_output_gain_millidb(output))
            .map_err(|_| StorageError::Invalid("invalid host gain"))?;
    }
    if !gains.is_empty() {
        return Err(StorageError::Invalid("unknown host gain device"));
    }
    Project::from_legacy_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}
pub(super) fn decode23(bytes: &[u8]) -> Result<Project, StorageError> {
    let doc: Document23 = serde_json::from_slice(bytes)?;
    if doc.format != "hex-project" || doc.version != 23 {
        return Err(StorageError::Invalid(
            "unsupported project format or version",
        ));
    }
    let mut value = serde_json::to_value(doc)?;
    let gains = take_both_gains(&mut value)?;
    value["version"] = 21.into();
    install_both_gains(
        gain_envelopes::DocumentV21::decode(&serde_json::to_vec(&value)?)?,
        gains,
    )
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

fn bounded_tracks23<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<GainTrack23>, D::Error> {
    struct Tracks;
    impl<'de> serde::de::Visitor<'de> for Tracks {
        type Value = Vec<GainTrack23>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded tracks and project gain points")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut tracks = Vec::new();
            let mut count = 0_usize;
            while let Some(track) = seq.next_element::<GainTrack23>()? {
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

// Shape inspection selects one complete frozen DTO before decoding original
// bytes. Missing/duplicate/unknown fields are never stripped or retried.
#[derive(Deserialize)]
struct Shape {
    tracks: Vec<TrackShape>,
    return_tracks: Vec<ReturnShape>,
}
#[derive(Deserialize)]
struct TrackShape {
    #[serde(default, deserialize_with = "present_field")]
    gain_touch_epoch: Option<serde::de::IgnoredAny>,
    #[serde(default, deserialize_with = "present_field")]
    gain_touch_spans: Option<serde::de::IgnoredAny>,
}
#[derive(Deserialize)]
struct ReturnShape {
    devices: Vec<DeviceShape>,
}
#[derive(Deserialize)]
struct DeviceShape {
    #[serde(default, deserialize_with = "present_field")]
    input_gain_millidb: Option<serde::de::IgnoredAny>,
}
pub(super) fn decode_historical(bytes: &[u8], version: u32) -> Result<Project, StorageError> {
    let shape: Shape = serde_json::from_slice(bytes)?;
    let mut touch = None;
    for track in shape.tracks {
        let has_touch = match (
            track.gain_touch_epoch.is_some(),
            track.gain_touch_spans.is_some(),
        ) {
            (true, true) => true,
            (false, false) => false,
            _ => return Err(StorageError::Invalid("incomplete gain touch lineage")),
        };
        if touch.is_some_and(|previous| previous != has_touch) {
            return Err(StorageError::Invalid("mixed gain touch lineages"));
        }
        touch = Some(has_touch);
    }
    // With no tracks there is no gain-lane history to distinguish. For22 the
    // Return device's input-gain field identifies main's complete gain DTO.
    let main = touch.unwrap_or_else(|| {
        version == 23
            || shape
                .return_tracks
                .iter()
                .flat_map(|bus| &bus.devices)
                .any(|device| device.input_gain_millidb.is_some())
    });
    match (version, main) {
        (22, true) => gain_touch::DocumentV22::decode(bytes),
        (23, true) => combined::decode(bytes),
        (22, false) => decode22(bytes),
        (23, false) => decode23(bytes),
        _ => Err(StorageError::Invalid("unsupported host-gain lineage")),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_touch_fields_must_be_complete_and_unambiguous() {
        let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        editor
            .execute(hex_project::Command::AddTrack {
                name: "touch".into(),
            })
            .unwrap();
        let original = editor.project();
        let main = crate::encode(original).unwrap();
        assert_eq!(crate::decode(&main).unwrap(), *original);
        let base: serde_json::Value = serde_json::from_slice(&main).unwrap();
        for key in ["gain_touch_epoch", "gain_touch_spans"] {
            let mut missing = base.clone();
            missing["tracks"][0].as_object_mut().unwrap().remove(key);
            assert!(crate::decode(&serde_json::to_vec(&missing).unwrap()).is_err());
        }
        let mut mixed = base.clone();
        let mut track = mixed["tracks"][0].clone();
        track["id"] = 9999.into();
        track["devices"] = serde_json::json!([]);
        track["instrument"] = serde_json::Value::Null;
        track.as_object_mut().unwrap().remove("gain_touch_epoch");
        track.as_object_mut().unwrap().remove("gain_touch_spans");
        mixed["tracks"].as_array_mut().unwrap().push(track);
        mixed["next_id"] = 10000.into();
        assert!(crate::decode(&serde_json::to_vec(&mixed).unwrap()).is_err());
        let text = String::from_utf8(main).unwrap();
        let needle = format!(
            "\"gain_touch_epoch\":{}",
            base["tracks"][0]["gain_touch_epoch"]
        );
        let duplicate = text.replace(&needle, &format!("{needle},{needle}"));
        assert_ne!(text, duplicate);
        assert!(crate::decode(duplicate.as_bytes()).is_err());
        let mut future = base;
        future["unrecognized"] = true.into();
        assert!(crate::decode(&serde_json::to_vec(&future).unwrap()).is_err());
    }
}
