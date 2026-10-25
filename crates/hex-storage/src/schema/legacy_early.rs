//! Frozen early saved-project branches, validated before the shared typed readers.
//!
//! Original reader definitions and tests for 1–13 are in 89674b36. Public writers
//! for 14–29 and their competing development branches are pinned by the fixtures
//! in tests/fixtures/early. Selection uses complete inventories, never parse-error
//! fallback. Only fields absent by design acquire defaults; current formats and the
//! later historical decoder do not pass through this adapter.
//!
//! The 26 event writer (309d41ae) precedes names; named events (d8922776)
//! and named fades (0298bf88) both used 27. Their appearance integrations used
//! 28 (819a4cc4/e50ffea3), then 29 (caed5306) combined them. The later 29
//! writer a0cedd1d adds required nullable sample tempo without changing the ID.
use super::{Project, StorageError};
use hex_project::{
    MAX_ASSETS, MAX_CLIPS, MAX_DEVICES, MAX_DEVICES_PER_TRACK, MAX_GAIN_POINTS,
    MAX_GAIN_POINTS_PER_TRACK, MAX_GAIN_TOUCH_SPANS_PER_TRACK, MAX_GAIN_TOUCH_STEPS_PER_SPAN,
    MAX_RETURN_TRACKS, MAX_SESSION_CELLS, MAX_TRACKS,
};
use serde_json::{Map, Value, json};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Branch {
    Ordinary,
    InlineAudio,
    Events,
    Fades,
}

pub(crate) fn decode(bytes: &[u8], version: u32) -> Result<Project, StorageError> {
    if !(1..=29).contains(&version) {
        return Err(StorageError::Invalid("invalid early project schema"));
    }
    let _: super::audio_events::UniqueKeys = serde_json::from_slice(bytes)?;
    let mut value: Value = serde_json::from_slice(bytes)?;
    if value.get("format").and_then(Value::as_str) != Some("hex-project")
        || value.get("version").and_then(Value::as_u64) != Some(u64::from(version))
    {
        return Err(StorageError::Invalid("invalid historical project header"));
    }
    rows(value.get("assets"), MAX_ASSETS)?;
    rows(value.get("tracks"), MAX_TRACKS)?;
    let branch = select_branch(&value, version)?;
    validate_host_trims(&value, version, branch)?;
    upgrade_gain_lanes(&mut value, version)?;
    upgrade_audio_edits(&mut value, version, branch)?;
    let adapted = serde_json::to_vec(&value)?;
    match branch {
        Branch::Events => super::audio_events::decode_historical(&adapted, version),
        Branch::Fades => super::fade_shapes::DocumentV29::decode_historical(&adapted, version),
        _ => super::DocumentV11::decode(&adapted),
    }
}

fn select_branch(value: &Value, version: u32) -> Result<Branch, StorageError> {
    let has = |name| value.get(name).is_some();
    if matches!(version, 18..=21) {
        let provenance = has("import_provenance");
        let routing = has("routing");
        let valid = match version {
            18 => !routing,
            19..=21 => provenance == routing,
            _ => false,
        };
        if !valid {
            return Err(StorageError::Invalid("mixed early project lineages"));
        }
        if matches!(version, 20 | 21) && !provenance {
            return Ok(Branch::InlineAudio);
        }
    }
    if version < 26 {
        return Ok(Branch::Ordinary);
    }
    let names = has("clip_names");
    let events = has("audio_event_state");
    let fades = has("fade_shapes");
    let colors = has("track_colors");
    let accents = has("accented_clips");
    let branch = match (version, names, events, fades, colors, accents) {
        (26, true, false, false, false, false) => Branch::Ordinary,
        (26, false, true, false, false, false) => Branch::Events,
        (27, true, false, false, true, true) => Branch::Ordinary,
        (27, true, true, false, false, false) => Branch::Events,
        (27, true, false, true, false, false) => Branch::Fades,
        (28, true, true, false, true, true) => Branch::Events,
        (28, true, false, true, true, true) => Branch::Fades,
        (29, true, true, true, true, true) => Branch::Fades,
        _ => return Err(StorageError::Invalid("mixed early project inventories")),
    };
    Ok(branch)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Trims {
    None,
    Output,
    Both,
}

fn validate_host_trims(value: &Value, version: u32, branch: Branch) -> Result<(), StorageError> {
    if version < 14 {
        return Ok(());
    }
    let tracks = rows(value.get("tracks"), MAX_TRACKS)?;
    let touch = tracks.iter().find_map(|track| {
        (track.get("gain_touch_spans").is_some() || track.get("gain_touch_epoch").is_some())
            .then_some(true)
    });
    let allowed: &[Trims] = match version {
        14..=17 => &[Trims::None],
        18 if value.get("import_provenance").is_some() => &[Trims::None],
        18 => &[Trims::Output],
        19 if value.get("import_provenance").is_some() => &[Trims::None],
        19 => &[Trims::Both],
        20 | 21 if branch == Branch::InlineAudio => &[Trims::Both],
        // The later explicit public schema20 writer also emitted the complete
        // shared two-trim DTO; it did not use the output-only22 DTO.
        20 => &[Trims::None, Trims::Both],
        21 => &[Trims::None],
        22 if touch.is_some() => &[Trims::None, Trims::Both],
        22 if tracks.is_empty() => &[Trims::None, Trims::Output, Trims::Both],
        22 => &[Trims::Output],
        _ => &[Trims::Both],
    };
    let mut observed = None;
    let mut count = 0_usize;
    let mut visit = |device: &Value| -> Result<(), StorageError> {
        count = count
            .checked_add(1)
            .ok_or(StorageError::Invalid("device count overflow"))?;
        if count > MAX_DEVICES {
            return Err(StorageError::Invalid("historical device limit reached"));
        }
        let object = device
            .as_object()
            .ok_or(StorageError::Invalid("invalid historical device"))?;
        let trims = match (
            object.contains_key("input_gain_millidb"),
            object.contains_key("output_gain_millidb"),
        ) {
            (false, false) => Trims::None,
            (false, true) => Trims::Output,
            (true, true) => Trims::Both,
            _ => return Err(StorageError::Invalid("incomplete historical host trims")),
        };
        if !allowed.contains(&trims) || observed.is_some_and(|old| old != trims) {
            return Err(StorageError::Invalid("mixed historical host trim layouts"));
        }
        let fields = 5 + match trims {
            Trims::None => 0,
            Trims::Output => 1,
            Trims::Both => 2,
        };
        if object.len() != fields
            || ["id", "bypassed", "type_id", "state_version", "payload"]
                .iter()
                .any(|field| !object.contains_key(*field))
        {
            return Err(StorageError::Invalid("invalid historical device fields"));
        }
        observed = Some(trims);
        Ok(())
    };
    for track in tracks {
        for device in rows(track.get("devices"), MAX_DEVICES_PER_TRACK)? {
            visit(device)?;
        }
        if version >= 15 {
            let instrument = track
                .get("instrument")
                .ok_or(StorageError::Invalid("historical instrument is missing"))?;
            if !instrument.is_null() {
                visit(instrument)?;
            }
        }
    }
    if version >= 17 {
        for bus in rows(value.get("return_tracks"), MAX_RETURN_TRACKS)? {
            for device in rows(bus.get("devices"), MAX_DEVICES_PER_TRACK)? {
                visit(device)?;
            }
        }
    }
    Ok(())
}

fn upgrade_gain_lanes(value: &mut Value, version: u32) -> Result<(), StorageError> {
    let mut total = 0_usize;
    let mut modern = None;
    let modern_allowed = version == 22
        && rows(value.get("tracks"), MAX_TRACKS)?.iter().all(|track| {
            track.get("gain_touch_epoch").is_some() && track.get("gain_touch_spans").is_some()
        });
    for track in rows_mut(value.get_mut("tracks"), MAX_TRACKS)? {
        let mut units = 0_usize;
        if let Some(points) = track.get_mut("gain_envelope") {
            let points = rows_mut(Some(points), MAX_GAIN_POINTS_PER_TRACK)?;
            units = points.len();
            for point in points {
                let current = point.get("incoming_gain").is_some();
                gain_layout(&mut modern, current, modern_allowed)?;
                let fields: &[&str] = if current {
                    &["beat_raw", "gain", "outgoing", "incoming_gain"]
                } else {
                    &["beat_raw", "gain", "outgoing"]
                };
                let point = exact_object(point, fields)?;
                if !current {
                    point.insert("incoming_gain".into(), Value::Null);
                }
            }
        }
        if let Some(spans) = track.get_mut("gain_touch_spans") {
            for span in rows_mut(Some(spans), MAX_GAIN_TOUCH_SPANS_PER_TRACK)? {
                let current = span.get("source_start_raw").is_some();
                gain_layout(&mut modern, current, modern_allowed)?;
                let fields: &[&str] = if current {
                    &[
                        "start_raw",
                        "end_raw",
                        "context",
                        "steps",
                        "source_start_raw",
                    ]
                } else {
                    &["start_raw", "end_raw", "context", "steps"]
                };
                let span = exact_object(span, fields)?;
                let steps = rows(span.get("steps"), MAX_GAIN_TOUCH_STEPS_PER_SPAN)?;
                units = units
                    .checked_add(steps.len())
                    .and_then(|n| n.checked_add(2))
                    .ok_or(StorageError::Invalid("historical gain count overflow"))?;
                let start =
                    span.get("start_raw")
                        .and_then(Value::as_u64)
                        .ok_or(StorageError::Invalid(
                            "invalid historical gain source origin",
                        ))?;
                if !current {
                    // Historical playback measured event origins from placement
                    // start; the current source origin must preserve that law.
                    span.insert("source_start_raw".into(), start.into());
                }
            }
        }
        total = total
            .checked_add(units)
            .ok_or(StorageError::Invalid("historical gain count overflow"))?;
        if units > MAX_GAIN_POINTS_PER_TRACK || total > MAX_GAIN_POINTS {
            return Err(StorageError::Invalid("historical gain limit reached"));
        }
    }
    Ok(())
}

// The retained explicit public schema22 writer also emits the current complete
// gain DTO. Select that evidenced shape uniformly; never discard seam/origin data.
fn gain_layout(
    observed: &mut Option<bool>,
    current: bool,
    modern_allowed: bool,
) -> Result<(), StorageError> {
    if (current && !modern_allowed) || observed.is_some_and(|old| old != current) {
        return Err(StorageError::Invalid("mixed historical gain layouts"));
    }
    *observed = Some(current);
    Ok(())
}

fn upgrade_audio_edits(
    value: &mut Value,
    version: u32,
    branch: Branch,
) -> Result<(), StorageError> {
    // a9da374e stored edits inline under 21; e9736fce moved the same
    // four-field edit into the complete top-level inventory under 25.
    let mut sample_bpm = None;
    if branch == Branch::InlineAudio && version == 21 {
        for track in rows_mut(value.get_mut("tracks"), MAX_TRACKS)? {
            for clip in rows_mut(track.get_mut("audio_clips"), MAX_CLIPS)? {
                let edit = clip
                    .get_mut("audio_edit")
                    .ok_or(StorageError::Invalid("historical audio edit is missing"))?;
                upgrade_edit(edit, version, &mut sample_bpm)?;
            }
        }
    } else if version >= 25 {
        for state in rows_mut(
            value.get_mut("audio_clip_state"),
            MAX_CLIPS + MAX_SESSION_CELLS,
        )? {
            let state = exact_object(state, &["clip", "gain_millidb", "audio_edit"])?;
            let edit = state
                .get_mut("audio_edit")
                .ok_or(StorageError::Invalid("historical audio edit is missing"))?;
            upgrade_edit(edit, version, &mut sample_bpm)?;
        }
    }
    Ok(())
}

fn upgrade_edit(
    edit: &mut Value,
    version: u32,
    sample_bpm: &mut Option<bool>,
) -> Result<(), StorageError> {
    if edit.is_null() {
        return Ok(());
    }
    let edit = exact_object(
        edit,
        &["domain", "view_start", "view_end", "musical_start_raw"],
    )?;
    let domain = edit
        .get_mut("domain")
        .ok_or(StorageError::Invalid("historical audio domain is missing"))?;
    let has_bpm = domain.get("sample_micro_bpm").is_some();
    if (has_bpm && version != 29) || sample_bpm.is_some_and(|old| old != has_bpm) {
        return Err(StorageError::Invalid(
            "mixed historical sample tempo layouts",
        ));
    }
    *sample_bpm = Some(has_bpm);
    let fields: &[&str] = if has_bpm {
        &[
            "source_offset",
            "source_length",
            "source_phase",
            "looping",
            "clock",
            "pitch_cents",
            "warp_enabled",
            "warp",
            "source_gain",
            "fade",
            "fade_start",
            "fade_projection",
            "sample_micro_bpm",
        ]
    } else {
        &[
            "source_offset",
            "source_length",
            "source_phase",
            "looping",
            "clock",
            "pitch_cents",
            "warp_enabled",
            "warp",
            "source_gain",
            "fade",
            "fade_start",
            "fade_projection",
        ]
    };
    let domain = exact_object(domain, fields)?;
    if !has_bpm {
        domain.insert("sample_micro_bpm".into(), Value::Null);
    }
    let start = edit
        .remove("musical_start_raw")
        .ok_or(StorageError::Invalid("historical musical start is missing"))?;
    let timing = if start.is_null() {
        json!({"kind":"frames"})
    } else {
        let start = start
            .as_u64()
            .ok_or(StorageError::Invalid("invalid historical musical start"))?;
        json!({"kind":"musical","start_raw":start})
    };
    edit.insert("timing".into(), timing);
    edit.insert("placement".into(), Value::Null);
    edit.insert("processing_projection".into(), Value::Null);
    edit.insert("placement_start_offset".into(), 0.into());
    edit.insert("placement_end_offset".into(), 0.into());
    Ok(())
}

fn rows(value: Option<&Value>, maximum: usize) -> Result<&Vec<Value>, StorageError> {
    let rows = value
        .and_then(Value::as_array)
        .ok_or(StorageError::Invalid("missing historical inventory"))?;
    if rows.len() > maximum {
        return Err(StorageError::Invalid("historical inventory limit reached"));
    }
    Ok(rows)
}
fn rows_mut(value: Option<&mut Value>, maximum: usize) -> Result<&mut Vec<Value>, StorageError> {
    let rows = value
        .and_then(Value::as_array_mut)
        .ok_or(StorageError::Invalid("missing historical inventory"))?;
    if rows.len() > maximum {
        return Err(StorageError::Invalid("historical inventory limit reached"));
    }
    Ok(rows)
}
fn exact_object<'a>(
    value: &'a mut Value,
    fields: &[&str],
) -> Result<&'a mut Map<String, Value>, StorageError> {
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid historical metadata row"))?;
    if object.len() != fields.len() || fields.iter().any(|field| !object.contains_key(*field)) {
        return Err(StorageError::Invalid("invalid historical metadata fields"));
    }
    Ok(object)
}
