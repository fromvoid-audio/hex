//! In-memory nested wire upgrades for the supported schema34–36 branches.
//!
//! The original writers are schema34 at `85e6dfe2`, schema35 at `8d2e61ab`,
//! tempo-schema36 at `aea1275c` and section-schema36 at `97e885eb`. The two
//! schema36 branches are selected only by the presence of `arrangement_sections`, before
//! that inventory is removed. No parse-failure fallback or unknown-field removal
//! is permitted. The current strict DTOs still validate all adapted leaf values.
use crate::StorageError;
use hex_project::{
    MAX_CLIPS, MAX_GAIN_POINTS, MAX_GAIN_POINTS_PER_TRACK, MAX_GAIN_TOUCH_SPANS_PER_TRACK,
    MAX_GAIN_TOUCH_STEPS_PER_SPAN, MAX_SESSION_CELLS, MAX_TRACKS,
};
use serde_json::{Map, Value};

/// Adapt only historical metadata, after duplicate-key validation and before
/// the current DTOs parse it. All work stays on the caller's control-side value;
/// neither original file bytes nor media are modified.
pub(super) fn upgrade_nested(value: &mut Value, version: u32) -> Result<(), StorageError> {
    if !(34..=36).contains(&version) {
        return Err(StorageError::Invalid(
            "unsupported nested migration version",
        ));
    }
    let sections = version == 36 && value.get("arrangement_sections").is_some();
    let tagged_timing = version == 36 && !sections;
    upgrade_audio_edits(value, tagged_timing, sections)?;
    upgrade_gain_lanes(value, sections)
}

fn upgrade_audio_edits(
    value: &mut Value,
    tagged_timing: bool,
    sections: bool,
) -> Result<(), StorageError> {
    let states = bounded_rows(
        value.get_mut("audio_clip_state"),
        MAX_CLIPS + MAX_SESSION_CELLS,
        "invalid or oversized historical audio clip inventory",
    )?;
    for state in states {
        let state = exact_object(
            state,
            &["clip", "gain_millidb", "audio_edit"],
            "invalid historical audio clip state fields",
        )?;
        let edit = state
            .get_mut("audio_edit")
            .ok_or(StorageError::Invalid("historical audio edit is missing"))?;
        if edit.is_null() {
            continue;
        }
        let fields: &[&str] = if sections {
            &[
                "domain",
                "view_start",
                "view_end",
                "musical_start_raw",
                "placement",
                "processing_projection",
                "placement_start_offset",
                "placement_end_offset",
            ]
        } else if tagged_timing {
            &["domain", "view_start", "view_end", "timing", "placement"]
        } else {
            &[
                "domain",
                "view_start",
                "view_end",
                "musical_start_raw",
                "placement",
            ]
        };
        let edit = exact_object(edit, fields, "invalid historical audio edit fields")?;
        if !tagged_timing {
            // Historical None means frame placement; Some is the exact raw
            // musical start. Domain, view and placement data retain their DTOs.
            let start = edit
                .remove("musical_start_raw")
                .ok_or(StorageError::Invalid("historical musical start is missing"))?;
            let timing = if start.is_null() {
                serde_json::json!({ "kind": "frames" })
            } else {
                let start = start
                    .as_u64()
                    .ok_or(StorageError::Invalid("invalid historical musical start"))?;
                serde_json::json!({ "kind": "musical", "start_raw": start })
            };
            edit.insert("timing".into(), timing);
        }
        if !sections {
            // Earlier branches had neither a retained processing context nor
            // boundary offsets: null and zero preserve their rendering law.
            edit.insert("processing_projection".into(), Value::Null);
            edit.insert("placement_start_offset".into(), 0.into());
            edit.insert("placement_end_offset".into(), 0.into());
        }
    }
    Ok(())
}

fn upgrade_gain_lanes(value: &mut Value, sections: bool) -> Result<(), StorageError> {
    let tracks = bounded_rows(
        value.get_mut("tracks"),
        MAX_TRACKS,
        "invalid or oversized historical track inventory",
    )?;
    let mut project_units = 0_usize;
    for track in tracks {
        let points = bounded_rows(
            track.get_mut("gain_envelope"),
            MAX_GAIN_POINTS_PER_TRACK,
            "invalid or oversized historical gain envelope",
        )?;
        let mut track_units = points.len();
        for point in points {
            let fields: &[&str] = if sections {
                &["beat_raw", "gain", "outgoing", "incoming_gain"]
            } else {
                &["beat_raw", "gain", "outgoing"]
            };
            let point = exact_object(point, fields, "invalid historical gain point fields")?;
            if !sections {
                // An old point has no independent incoming side at a seam.
                point.insert("incoming_gain".into(), Value::Null);
            }
        }
        let spans = bounded_rows(
            track.get_mut("gain_touch_spans"),
            MAX_GAIN_TOUCH_SPANS_PER_TRACK,
            "invalid or oversized historical gain touch inventory",
        )?;
        for span in spans {
            let fields: &[&str] = if sections {
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
            let span = exact_object(span, fields, "invalid historical gain touch span fields")?;
            let steps = bounded_rows(
                span.get_mut("steps"),
                MAX_GAIN_TOUCH_STEPS_PER_SPAN,
                "invalid or oversized historical gain touch steps",
            )?;
            track_units = track_units
                .checked_add(steps.len())
                .and_then(|n| n.checked_add(2))
                .ok_or(StorageError::Invalid("historical gain lane count overflow"))?;
            if track_units > MAX_GAIN_POINTS_PER_TRACK {
                return Err(StorageError::Invalid("historical gain lane limit reached"));
            }
            if !sections {
                let start = span
                    .get("start_raw")
                    .and_then(Value::as_u64)
                    .ok_or(StorageError::Invalid("invalid historical gain touch start"))?;
                // Before section editing, span evaluation used its saved start
                // as the source origin; preserve that law rather than inventing
                // an origin from the first possibly-fragmented held step.
                span.insert("source_start_raw".into(), start.into());
            }
        }
        project_units = project_units
            .checked_add(track_units)
            .ok_or(StorageError::Invalid("historical gain lane count overflow"))?;
        if project_units > MAX_GAIN_POINTS {
            return Err(StorageError::Invalid(
                "historical project gain point limit reached",
            ));
        }
    }
    Ok(())
}

fn bounded_rows<'a>(
    value: Option<&'a mut Value>,
    maximum: usize,
    error: &'static str,
) -> Result<&'a mut Vec<Value>, StorageError> {
    let rows = value
        .and_then(Value::as_array_mut)
        .ok_or(StorageError::Invalid(error))?;
    if rows.len() > maximum {
        return Err(StorageError::Invalid(error));
    }
    Ok(rows)
}

fn exact_object<'a>(
    value: &'a mut Value,
    fields: &[&str],
    error: &'static str,
) -> Result<&'a mut Map<String, Value>, StorageError> {
    let object = value.as_object_mut().ok_or(StorageError::Invalid(error))?;
    if object.len() != fields.len() || fields.iter().any(|field| !object.contains_key(*field)) {
        return Err(StorageError::Invalid(error));
    }
    Ok(object)
}
