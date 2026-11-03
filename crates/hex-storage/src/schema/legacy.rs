//! Historical saved envelopes, selected by evidenced version and field presence.
//!
//! Schema30 had separate tuning (`ac29a1ad`) and device-assets (`d34e5a13`)
//! writers;31 (`20cd1de3`) combined them,32 (`5165e305`) added comparison and33
//! (`7b647514`) added Main devices. Unpublished intermediate writers also
//! introduced sample tempo, placement and MIDI boundary fields under reused IDs.
//! The admitted feature combinations below come from their actual source DTOs.
//! Adapt absent-by-design metadata, then use the frozen strict34 decoder. Unknown
//! fields are never removed and an invalid branch never falls back to another.
//! Original31's explicit20/22 exports also retain MIDI boundary policy. Extract
//! that complete source-backed inventory and restore it after the frozen reader,
//! before validating the typed current state; late authored notes stay retained.
use super::{Project, StorageError};
use hex_project::{MidiOneShotBoundary, SessionCellContentSnapshot, TrackContentSnapshot};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) fn decode(bytes: &[u8], version: u32) -> Result<Project, StorageError> {
    if (1..=29).contains(&version) {
        return decode_early(bytes, version);
    }
    let mut value = super::audio_events::parse_unique(bytes)?;
    if value.get("version").and_then(Value::as_u64) != Some(u64::from(version)) {
        return Err(StorageError::Invalid("invalid historical project version"));
    }
    check_writer_shape(&value, version)?;
    let boundaries = upgrade_midi_boundary(&mut value)?;
    match version {
        30..=33 => upgrade_assets_tuning(&mut value, version)?,
        _ => {
            return Err(StorageError::Invalid(
                "unsupported historical project schema",
            ));
        }
    }
    upgrade_audio_layout(&mut value)?;
    value["version"] = 34.into();
    let project = super::tuning::decode_version_value(value, 34)?;
    restore_midi_boundaries(project, boundaries)
}

fn decode_early(bytes: &[u8], version: u32) -> Result<Project, StorageError> {
    // The original31 MIDI branch also exposed public20/22 writers using its
    // shared source DTOs. Their complete embedded policy inventory is evidenced
    // independently from ordinary older20/22 documents, which default SourceEnd.
    if matches!(version, 20 | 22) {
        let _: super::audio_events::UniqueKeys = serde_json::from_slice(bytes)?;
        let mut value: Value = serde_json::from_slice(bytes)?;
        let mut embedded = None;
        visit_midi(&value, |_, source| {
            uniform(&mut embedded, source.get("one_shot_boundary").is_some())
        })?;
        if embedded == Some(true) {
            validate_early_policy_layout(&value)?;
            let boundaries = upgrade_midi_boundary(&mut value)?;
            let project = super::legacy_early::decode(&serde_json::to_vec(&value)?, version)?;
            return restore_midi_boundaries(project, boundaries);
        }
    }
    super::legacy_early::decode(bytes, version)
}

fn validate_early_policy_layout(value: &Value) -> Result<(), StorageError> {
    if value.get("import_provenance").is_none() || value.get("routing").is_none() {
        return Err(StorageError::Invalid(
            "historical MIDI policy requires the foundation writer layout",
        ));
    }
    for owner in ["tracks", "return_tracks"] {
        let tracks = value
            .get(owner)
            .and_then(Value::as_array)
            .ok_or(StorageError::Invalid("missing historical track inventory"))?;
        for track in tracks {
            let devices = track
                .get("devices")
                .and_then(Value::as_array)
                .ok_or(StorageError::Invalid("missing historical device inventory"))?;
            let instrument = if owner == "tracks" {
                track.get("instrument").filter(|device| !device.is_null())
            } else {
                None
            };
            for device in devices.iter().chain(instrument) {
                if device.get("input_gain_millidb").is_none()
                    || device.get("output_gain_millidb").is_none()
                {
                    return Err(StorageError::Invalid(
                        "historical MIDI policy requires the two-trim writer layout",
                    ));
                }
            }
            // These explicit writers predate gain seams and relocated origins.
            // Never combine independently evidenced branches into a new layout.
            for (inventory, later_field) in [
                ("gain_envelope", "incoming_gain"),
                ("gain_touch_spans", "source_start_raw"),
            ] {
                if track
                    .get(inventory)
                    .and_then(Value::as_array)
                    .is_some_and(|rows| rows.iter().any(|row| row.get(later_field).is_some()))
                {
                    return Err(StorageError::Invalid("mixed historical MIDI/gain layouts"));
                }
            }
        }
    }
    Ok(())
}

fn upgrade_audio_layout(value: &mut Value) -> Result<(), StorageError> {
    let rows = value
        .get_mut("audio_clip_state")
        .and_then(Value::as_array_mut)
        .ok_or(StorageError::Invalid(
            "missing historical audio state inventory",
        ))?;
    if rows.len() > hex_project::MAX_CLIPS + hex_project::MAX_SESSION_CELLS {
        return Err(StorageError::Invalid(
            "historical audio state inventory limit",
        ));
    }
    for row in rows {
        let edit = row
            .get_mut("audio_edit")
            .ok_or(StorageError::Invalid("missing historical audio edit"))?;
        if edit.is_null() {
            continue;
        }
        let edit = edit
            .as_object_mut()
            .ok_or(StorageError::Invalid("invalid historical audio edit"))?;
        if !edit.contains_key("placement") {
            edit.insert("placement".into(), Value::Null);
        }
        let domain = edit
            .get_mut("domain")
            .and_then(Value::as_object_mut)
            .ok_or(StorageError::Invalid("invalid historical audio domain"))?;
        if !domain.contains_key("sample_micro_bpm") {
            domain.insert("sample_micro_bpm".into(), Value::Null);
        }
    }
    Ok(())
}

fn upgrade_assets_tuning(value: &mut Value, version: u32) -> Result<(), StorageError> {
    let tuning = value.get("tuning").is_some();
    let assets = value.get("device_assets").is_some();
    if (version == 30 && tuning == assets) || (version >= 31 && !(tuning && assets)) {
        return Err(StorageError::Invalid(
            "inconsistent historical tuning/asset envelope",
        ));
    }
    // These inventories did not exist in any of these writers. Refuse supplied
    // data rather than overwriting it with a manufactured default.
    for field in ["asset_sources", "midi_clip_volumes", "arrangement_sections"] {
        if value.get(field).is_some() {
            return Err(StorageError::Invalid("unexpected later-format inventory"));
        }
    }
    if version < 33 {
        if value.get("main_devices").is_some() {
            return Err(StorageError::Invalid(
                "unexpected historical Main inventory",
            ));
        }
        value["main_devices"] = json!([]);
    }
    if !tuning {
        value["tuning"] = serde_json::to_value(super::tuning::Tuning::from(
            hex_tuning::ProjectTuning::default(),
        ))?;
    }
    if !assets {
        let mut rows = Vec::new();
        for owner in ["tracks", "return_tracks"] {
            let tracks = value
                .get(owner)
                .and_then(Value::as_array)
                .ok_or(StorageError::Invalid("missing historical track inventory"))?;
            for track in tracks {
                let devices = track
                    .get("devices")
                    .and_then(Value::as_array)
                    .ok_or(StorageError::Invalid("missing historical device inventory"))?;
                for device in devices {
                    add_unbound_device(&mut rows, device)?;
                }
                if owner == "tracks" {
                    let instrument = track
                        .get("instrument")
                        .ok_or(StorageError::Invalid("missing historical instrument"))?;
                    if !instrument.is_null() {
                        add_unbound_device(&mut rows, instrument)?;
                    }
                }
            }
        }
        value["device_assets"] = Value::Array(rows);
    } else {
        let rows = value
            .get_mut("device_assets")
            .and_then(Value::as_array_mut)
            .ok_or(StorageError::Invalid(
                "invalid historical device asset inventory",
            ))?;
        if rows.len() > hex_project::MAX_DEVICES {
            return Err(StorageError::Invalid(
                "historical device asset inventory limit",
            ));
        }
        for row in rows {
            let row = row
                .as_object_mut()
                .ok_or(StorageError::Invalid("invalid historical device asset row"))?;
            if !row.contains_key("comparison") {
                row.insert("comparison".into(), Value::Null);
            }
        }
    }
    let assets = value
        .get("assets")
        .and_then(Value::as_array)
        .ok_or(StorageError::Invalid("missing historical asset inventory"))?;
    if assets.len() > hex_project::MAX_ASSETS {
        return Err(StorageError::Invalid("historical asset inventory limit"));
    }
    let sources = assets
        .iter()
        .map(|asset| {
            let id = asset
                .get("id")
                .and_then(Value::as_u64)
                .ok_or(StorageError::Invalid("invalid historical asset identity"))?;
            Ok(json!({"asset":id,"original":null}))
        })
        .collect::<Result<Vec<_>, StorageError>>()?;
    value["asset_sources"] = Value::Array(sources);
    Ok(())
}

fn add_unbound_device(rows: &mut Vec<Value>, device: &Value) -> Result<(), StorageError> {
    if rows.len() == hex_project::MAX_DEVICES {
        return Err(StorageError::Invalid(
            "historical device asset inventory limit",
        ));
    }
    let id = device
        .get("id")
        .and_then(Value::as_u64)
        .ok_or(StorageError::Invalid("invalid historical device identity"))?;
    rows.push(json!({"device":id,"references":[],"comparison":null}));
    Ok(())
}

// Feature tuples are (sample tempo, placement, comparison, MIDI boundary).
// ac29/d34, a0cedd,20cd,1bab,5d6c,4043,a927,5165,e09a,7b64 respectively.
fn check_writer_shape(value: &Value, version: u32) -> Result<(), StorageError> {
    let mut audio = None;
    let rows = value
        .get("audio_clip_state")
        .and_then(Value::as_array)
        .ok_or(StorageError::Invalid(
            "missing historical audio state inventory",
        ))?;
    if rows.len() > hex_project::MAX_CLIPS + hex_project::MAX_SESSION_CELLS {
        return Err(StorageError::Invalid(
            "historical audio state inventory limit",
        ));
    }
    for row in rows {
        let edit = row
            .get("audio_edit")
            .ok_or(StorageError::Invalid("missing historical audio edit"))?;
        if edit.is_null() {
            continue;
        }
        let edit = edit
            .as_object()
            .ok_or(StorageError::Invalid("invalid historical audio edit"))?;
        let placement = edit.contains_key("placement");
        if edit.len() != 4 + usize::from(placement)
            || !["domain", "view_start", "view_end", "musical_start_raw"]
                .iter()
                .all(|key| edit.contains_key(*key))
        {
            return Err(StorageError::Invalid(
                "invalid historical audio edit fields",
            ));
        }
        let domain = edit
            .get("domain")
            .and_then(Value::as_object)
            .ok_or(StorageError::Invalid("invalid historical audio domain"))?;
        uniform(
            &mut audio,
            (domain.contains_key("sample_micro_bpm"), placement),
        )?;
    }
    let mut comparison = None;
    if let Some(rows) = value.get("device_assets") {
        let rows = rows.as_array().ok_or(StorageError::Invalid(
            "invalid historical device asset inventory",
        ))?;
        if rows.len() > hex_project::MAX_DEVICES {
            return Err(StorageError::Invalid(
                "historical device asset inventory limit",
            ));
        }
        for row in rows {
            let row = row
                .as_object()
                .ok_or(StorageError::Invalid("invalid historical device asset row"))?;
            let present = row.contains_key("comparison");
            if row.len() != 2 + usize::from(present)
                || !row.contains_key("device")
                || !row.contains_key("references")
            {
                return Err(StorageError::Invalid(
                    "invalid historical device asset fields",
                ));
            }
            uniform(&mut comparison, present)?;
        }
    }
    let mut midi = None;
    visit_midi(value, |_, source| {
        uniform(&mut midi, source.get("one_shot_boundary").is_some())
    })?;
    let variants: &[(bool, bool, bool, bool)] = match version {
        30 if value.get("tuning").is_some() => &[(false, false, false, false)],
        30 => &[(false, false, false, false), (true, false, false, false)],
        31 => &[
            (false, false, false, false),
            (true, false, false, false),
            (true, false, false, true),
            (true, true, false, true),
        ],
        32 => &[
            (true, true, false, false),
            (true, false, true, false),
            (true, true, true, false),
        ],
        33 => &[(true, true, true, false)],
        _ => {
            return Err(StorageError::Invalid(
                "unsupported historical project schema",
            ));
        }
    };
    if !variants.iter().any(|&(sample, placement, cmp, boundary)| {
        audio.is_none_or(|a| a == (sample, placement))
            && comparison.is_none_or(|c| c == cmp)
            && midi.is_none_or(|m| m == boundary)
    }) {
        return Err(StorageError::Invalid(
            "inconsistent historical writer layout",
        ));
    }
    Ok(())
}

fn uniform<T: Copy + Eq>(selected: &mut Option<T>, value: T) -> Result<(), StorageError> {
    if selected.is_some_and(|prior| prior != value) {
        return Err(StorageError::Invalid("mixed historical inventory layouts"));
    }
    *selected = Some(value);
    Ok(())
}

fn visit_midi(
    value: &Value,
    mut visit: impl FnMut(u64, &Value) -> Result<(), StorageError>,
) -> Result<(), StorageError> {
    let tracks = value
        .get("tracks")
        .and_then(Value::as_array)
        .ok_or(StorageError::Invalid("missing historical tracks"))?;
    for track in tracks {
        let clips = track
            .get("midi_clips")
            .and_then(Value::as_array)
            .ok_or(StorageError::Invalid("missing historical MIDI clips"))?;
        for clip in clips {
            visit(midi_id(clip)?, clip)?;
        }
    }
    let cells = value
        .pointer("/session/cells")
        .and_then(Value::as_array)
        .ok_or(StorageError::Invalid("missing historical Session cells"))?;
    for cell in cells {
        if cell.pointer("/content/kind").and_then(Value::as_str) == Some("midi") {
            visit(
                midi_id(
                    cell.pointer("/content/clip")
                        .ok_or(StorageError::Invalid("missing historical MIDI clip"))?,
                )?,
                cell.pointer("/content/clip/source")
                    .ok_or(StorageError::Invalid("missing historical MIDI source"))?,
            )?;
        }
    }
    Ok(())
}

fn midi_id(clip: &Value) -> Result<u64, StorageError> {
    clip.get("id")
        .and_then(Value::as_u64)
        .filter(|id| *id != 0)
        .ok_or(StorageError::Invalid("invalid historical MIDI identity"))
}

fn upgrade_midi_boundary(
    value: &mut Value,
) -> Result<BTreeMap<u64, MidiOneShotBoundary>, StorageError> {
    let mut boundaries = BTreeMap::new();
    visit_midi(value, |id, source| {
        if let Some(boundary) = source.get("one_shot_boundary") {
            let boundary = match boundary.as_str() {
                Some("source_end") => MidiOneShotBoundary::SourceEnd,
                Some("loop_end") => MidiOneShotBoundary::LoopEnd,
                _ => {
                    return Err(StorageError::Invalid(
                        "invalid historical MIDI one-shot boundary",
                    ));
                }
            };
            if boundaries.len() == hex_project::MAX_CLIPS {
                return Err(StorageError::Invalid("historical MIDI policy limit"));
            }
            if boundaries.insert(id, boundary).is_some() {
                return Err(StorageError::Invalid("duplicate historical MIDI policy"));
            }
        }
        Ok(())
    })?;
    for track in value
        .get_mut("tracks")
        .and_then(Value::as_array_mut)
        .ok_or(StorageError::Invalid("missing historical tracks"))?
    {
        for clip in track
            .get_mut("midi_clips")
            .and_then(Value::as_array_mut)
            .ok_or(StorageError::Invalid("missing historical MIDI clips"))?
        {
            clip.as_object_mut()
                .ok_or(StorageError::Invalid("invalid historical MIDI clip"))?
                .remove("one_shot_boundary");
        }
    }
    for cell in value
        .pointer_mut("/session/cells")
        .and_then(Value::as_array_mut)
        .ok_or(StorageError::Invalid("missing historical Session cells"))?
    {
        if cell.pointer("/content/kind").and_then(Value::as_str) == Some("midi") {
            cell.pointer_mut("/content/clip/source")
                .and_then(Value::as_object_mut)
                .ok_or(StorageError::Invalid("invalid historical MIDI source"))?
                .remove("one_shot_boundary");
        }
    }
    Ok(boundaries)
}

// The frozen readers validate the original notes and source extent without
// knowing31's policy field. Restore that independent metadata by exact clip ID,
// then validate the complete typed state; never truncate retained source notes.
fn restore_midi_boundaries(
    project: Project,
    mut boundaries: BTreeMap<u64, MidiOneShotBoundary>,
) -> Result<Project, StorageError> {
    if boundaries.is_empty() {
        return Ok(project);
    }
    let mut snapshot = project.snapshot();
    for track in &mut snapshot.tracks {
        let clips = match &mut track.content {
            TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
            TrackContentSnapshot::Instrument(clips) => clips,
            TrackContentSnapshot::Audio(_) => continue,
        };
        for clip in clips {
            clip.one_shot_boundary = boundaries
                .remove(&clip.id)
                .ok_or(StorageError::Invalid("missing historical MIDI policy"))?;
        }
    }
    for cell in &mut snapshot.session.cells {
        if let SessionCellContentSnapshot::Midi(clip) = &mut cell.content {
            clip.source.one_shot_boundary = boundaries
                .remove(&clip.id)
                .ok_or(StorageError::Invalid("missing historical MIDI policy"))?;
        }
    }
    if !boundaries.is_empty() {
        return Err(StorageError::Invalid("unmatched historical MIDI policy"));
    }
    Project::from_legacy_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}
