//! Schema54 adds Arrangement processing roles and persistent output suppression
//! over the complete schema53 document. Each authored payload appears once on
//! wire. Decode partitions its placements into complementary input/print views
//! for the strict predecessor decoder, then validates the combined canonical
//! snapshot. This keeps frozen occupancy admission intact without fake timing,
//! invented fades or an ambient validation bypass. Only known removed clip IDs
//! are filtered from clip inventories: malformed/unknown rows remain for strict
//! admission, and every removed row is validated in the complementary view.
use crate::{Project, StorageError};
use hex_project::{AudioClipRole, OutputSuppression};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Span {
    start: u64,
    end: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    track: u64,
    #[serde(deserialize_with = "bounded_clips")]
    printed: Vec<u64>,
    #[serde(deserialize_with = "bounded_spans")]
    suppressed: Vec<Span>,
}
fn bounded<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>, const MAX: usize>(
    d: D,
) -> Result<Vec<T>, D::Error> {
    struct Items<T, const MAX: usize>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const MAX: usize> serde::de::Visitor<'de> for Items<T, MAX> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded printed output inventory")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
            let mut rows = Vec::new();
            while let Some(row) = seq.next_element()? {
                if rows.len() == MAX {
                    return Err(serde::de::Error::custom(
                        "printed output inventory limit reached",
                    ));
                }
                rows.try_reserve(1).map_err(serde::de::Error::custom)?;
                rows.push(row);
            }
            Ok(rows)
        }
    }
    d.deserialize_seq(Items::<T, MAX>(std::marker::PhantomData))
}
fn bounded_clips<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u64>, D::Error> {
    bounded::<D, u64, { hex_project::MAX_CLIPS }>(d)
}
fn bounded_spans<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Span>, D::Error> {
    bounded::<D, Span, { hex_project::MAX_OUTPUT_SUPPRESSIONS }>(d)
}
#[derive(Deserialize)]
struct Rows(#[serde(deserialize_with = "bounded_rows")] Vec<Row>);
fn bounded_rows<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Row>, D::Error> {
    struct Inventory;
    impl<'de> serde::de::Visitor<'de> for Inventory {
        type Value = Vec<Row>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded track output inventory")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<Row>, A::Error> {
            let mut rows = Vec::new();
            let mut clips = 0usize;
            let mut spans = 0usize;
            while let Some(row) = seq.next_element::<Row>()? {
                clips = clips
                    .checked_add(row.printed.len())
                    .filter(|n| *n <= hex_project::MAX_CLIPS)
                    .ok_or_else(|| serde::de::Error::custom("printed clip limit reached"))?;
                spans = spans
                    .checked_add(row.suppressed.len())
                    .filter(|n| *n <= hex_project::MAX_OUTPUT_SUPPRESSIONS)
                    .ok_or_else(|| serde::de::Error::custom("output suppression limit reached"))?;
                if rows.len() == hex_project::MAX_TRACKS {
                    return Err(serde::de::Error::custom("printed track limit reached"));
                }
                rows.try_reserve(1).map_err(serde::de::Error::custom)?;
                rows.push(row);
            }
            Ok(rows)
        }
    }
    d.deserialize_seq(Inventory)
}
fn invalid(e: hex_project::Invalid) -> StorageError {
    StorageError::Invalid(e.0)
}
fn bad() -> StorageError {
    StorageError::Invalid("invalid printed output inventory")
}
fn array<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, StorageError> {
    value.get(key).and_then(Value::as_array).ok_or_else(bad)
}
fn id(value: &Value) -> Result<u64, StorageError> {
    value
        .get("id")
        .and_then(Value::as_u64)
        .filter(|id| *id != 0)
        .ok_or_else(bad)
}

pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    // The predecessor validates this same role/suppression-bearing model
    // before constructing its wire DTOs; avoid another complete model copy.
    let mut value = super::host_automation::encode(project)?;
    let rows: Vec<_> = project
        .tracks()
        .iter()
        .map(|track| Row {
            track: track.id().value(),
            printed: track
                .clips()
                .iter()
                .filter(|clip| clip.role() == AudioClipRole::PostDevice)
                .map(|clip| clip.id().value())
                .collect(),
            suppressed: track
                .output_suppression()
                .iter()
                .map(|span| Span {
                    start: span.start(),
                    end: span.end(),
                })
                .collect(),
        })
        .collect();
    value["version"] = 54.into();
    value["printed_output"] = serde_json::to_value(rows)?;
    Ok(value)
}

// This closed list belongs to the frozen53 wire contract, not to future schemas.
// Unknown top-level fields are retained for predecessor rejection.
const CLIP_ROWS: &[&str] = &[
    "clip_names",
    "audio_clip_state",
    "audio_event_state",
    "audio_event_fades",
    "grouped_audio",
    "content_views",
    "fade_shapes",
    "fade_curve_positions",
    "clip_activation",
    "midi_one_shot_boundaries",
];
fn project(mut value: Value, removed: &BTreeSet<u64>) -> Result<Project, StorageError> {
    super::clip_aliases::project_inventory(&mut value, removed)?;
    let tracks = value
        .get_mut("tracks")
        .and_then(Value::as_array_mut)
        .ok_or_else(bad)?;
    for track in tracks {
        for name in ["audio_clips", "midi_clips"] {
            let clips = track
                .get_mut(name)
                .and_then(Value::as_array_mut)
                .ok_or_else(bad)?;
            clips.retain(|clip| {
                !clip
                    .get("id")
                    .and_then(Value::as_u64)
                    .is_some_and(|id| removed.contains(&id))
            });
        }
    }
    for name in CLIP_ROWS {
        let rows = value
            .get_mut(*name)
            .and_then(Value::as_array_mut)
            .ok_or_else(bad)?;
        rows.retain(|row| {
            !row.get("clip")
                .and_then(Value::as_u64)
                .is_some_and(|id| removed.contains(&id))
        });
    }
    let accents = value
        .get_mut("accented_clips")
        .and_then(Value::as_array_mut)
        .ok_or_else(bad)?;
    accents.retain(|row| !row.as_u64().is_some_and(|id| removed.contains(&id)));
    let volumes = value
        .get_mut("midi_clip_volumes")
        .and_then(|v| v.get_mut("clips"))
        .and_then(Value::as_array_mut)
        .ok_or_else(bad)?;
    volumes.retain(|row| {
        !row.get("clip")
            .and_then(Value::as_u64)
            .is_some_and(|id| removed.contains(&id))
    });
    super::host_automation::decode(&serde_json::to_vec(&value)?)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    let _: super::audio_events::UniqueKeys = serde_json::from_slice(bytes)?;
    let mut value: Value = serde_json::from_slice(bytes)?;
    let object = value.as_object_mut().ok_or_else(bad)?;
    if object.get("version").and_then(Value::as_u64) != Some(54) {
        return Err(bad());
    }
    let Rows(rows) = serde_json::from_value(object.remove("printed_output").ok_or_else(bad)?)?;
    object.insert("version".into(), 53.into());
    let tracks = array(&value, "tracks")?;
    if rows.len() != tracks.len() {
        return Err(bad());
    }
    let mut printed = BTreeSet::new();
    let mut original_order = BTreeMap::new();
    let mut all = BTreeSet::new();
    let mut spans = BTreeMap::new();
    let mut span_count = 0usize;
    for (track, row) in tracks.iter().zip(rows) {
        let track_id = id(track)?;
        if row.track != track_id || spans.contains_key(&track_id) {
            return Err(bad());
        }
        let mut audio = BTreeSet::new();
        for (index, clip) in array(track, "audio_clips")?.iter().enumerate() {
            let clip_id = id(clip)?;
            if !all.insert(clip_id) {
                return Err(bad());
            }
            audio.insert(clip_id);
            original_order.insert(clip_id, index);
        }
        for clip in array(track, "midi_clips")? {
            if !all.insert(id(clip)?) {
                return Err(bad());
            }
        }
        for clip in row.printed {
            if !audio.contains(&clip) || !printed.insert(clip) {
                return Err(bad());
            }
        }
        span_count = span_count
            .checked_add(row.suppressed.len())
            .filter(|n| *n <= hex_project::MAX_OUTPUT_SUPPRESSIONS)
            .ok_or_else(bad)?;
        let checked = row
            .suppressed
            .into_iter()
            .map(|span| OutputSuppression::new(span.start, span.end).map_err(invalid))
            .collect::<Result<Vec<_>, _>>()?;
        spans.insert(track_id, checked);
    }
    if all.len() > hex_project::MAX_CLIPS {
        return Err(bad());
    }
    if printed.is_empty() && spans.values().all(Vec::is_empty) {
        // Ordinary projects keep the predecessor's canonical owner; no extra
        // snapshot/model copy is needed to install proven-empty metadata.
        return super::host_automation::decode(&serde_json::to_vec(&value)?);
    }
    let mut snapshot = if printed.is_empty() {
        super::host_automation::decode(&serde_json::to_vec(&value)?)?.snapshot()
    } else {
        let input = project(value.clone(), &printed)?;
        let removed = all.difference(&printed).copied().collect();
        let output = project(value, &removed)?;
        let mut snapshot = input.snapshot();
        let output = output.snapshot();
        for (track, mut post) in snapshot.tracks.iter_mut().zip(output.tracks) {
            if track.id != post.id {
                return Err(bad());
            }
            for mut clip in post.audio_clips_mut().map_err(invalid)?.drain(..) {
                clip.role = AudioClipRole::PostDevice;
                track.audio_clips_mut().map_err(invalid)?.push(clip);
            }
            track
                .audio_clips_mut()
                .map_err(invalid)?
                .sort_by_key(|clip| original_order.get(&clip.id).copied());
        }
        let mut sources: BTreeMap<_, _> = snapshot
            .aliases
            .sources
            .into_iter()
            .map(|source| (source.id, source))
            .collect();
        for source in output.aliases.sources {
            if let Some(existing) = sources.get(&source.id) {
                if existing != &source {
                    return Err(bad());
                }
            } else {
                sources.insert(source.id, source);
            }
        }
        let mut members: BTreeMap<_, _> = snapshot
            .aliases
            .members
            .into_iter()
            .map(|member| (member.clip, member))
            .collect();
        for member in output.aliases.members {
            if let Some(existing) = members.get(&member.clip) {
                if existing != &member {
                    return Err(bad());
                }
            } else {
                members.insert(member.clip, member);
            }
        }
        snapshot.aliases.sources = sources.into_values().collect();
        snapshot.aliases.members = members.into_values().collect();
        snapshot
    };
    for track in &mut snapshot.tracks {
        track.output_suppression = spans.remove(&track.id).ok_or_else(bad)?;
    }
    Project::from_snapshot(snapshot).map_err(invalid)
}

#[cfg(test)]
pub(super) fn remove_empty_inventory(value: &mut Value) {
    let Rows(rows) = serde_json::from_value(
        value
            .get("printed_output")
            .cloned()
            .expect("current print inventory"),
    )
    .expect("typed print rows");
    let tracks = array(value, "tracks").expect("current tracks");
    assert_eq!(rows.len(), tracks.len());
    for (row, track) in rows.iter().zip(tracks) {
        assert_eq!(row.track, id(track).expect("track identity"));
        assert!(
            row.printed.is_empty() && row.suppressed.is_empty(),
            "historical fixture cannot discard printed state"
        );
    }
    value
        .as_object_mut()
        .expect("current document")
        .remove("printed_output");
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor};
    fn fixture() -> (Project, Project, u64) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "original.wav".into(),
                sample_rate: 48_000,
                frames: 100,
            })
            .unwrap()
        else {
            panic!("audio")
        };
        let Created::Clips { first: printed, .. } = editor
            .execute(Command::DuplicateAliasClips(vec![clip]))
            .unwrap()
        else {
            panic!("alias")
        };
        let older = editor.project().clone();
        let mut snapshot = older.snapshot();
        let print = snapshot.tracks[0]
            .audio_clips_mut()
            .unwrap()
            .iter_mut()
            .find(|clip| clip.id == printed.value())
            .unwrap();
        print.role = AudioClipRole::PostDevice;
        print.start = 20;
        snapshot.tracks[0].output_suppression = vec![OutputSuppression::new(20, 120).unwrap()];
        (
            Project::from_snapshot(snapshot).unwrap(),
            older,
            printed.value(),
        )
    }
    #[test]
    fn overlapping_print_and_input_share_one_alias_payload_and_round_trip_after_deletion() {
        let (project, older, printed) = fixture();
        let bytes = crate::encode(&project).unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["alias_sources"].as_array().unwrap().len(), 1);
        assert_eq!(crate::decode(&bytes).unwrap(), project);
        let mut editor = Editor::new(project, 8).unwrap();
        let clip = editor.project().tracks()[0]
            .clips()
            .iter()
            .find(|clip| clip.id().value() == printed)
            .unwrap()
            .id();
        editor.execute(Command::RemoveClip(clip)).unwrap();
        assert_eq!(
            crate::decode(&crate::encode(editor.project()).unwrap()).unwrap(),
            *editor.project()
        );
        assert!(!editor.project().tracks()[0].output_suppression().is_empty());
        let legacy = super::super::host_automation::encode(&older).unwrap();
        assert_eq!(
            crate::decode(&serde_json::to_vec(&legacy).unwrap()).unwrap(),
            older
        );
        assert!(
            older.tracks()[0]
                .clips()
                .iter()
                .all(|clip| clip.role() == AudioClipRole::Input)
        );
        assert!(crate::encode_schema22(editor.project()).is_err());
    }
    #[test]
    fn complementary_validation_never_hides_bad_input_print_or_alias_inventory() {
        let (project, _, _) = fixture();
        let good = encode(&project).unwrap();
        type Mutation = Box<dyn Fn(&mut Value)>;
        let mutations: Vec<Mutation> = vec![
            Box::new(|v| v["tracks"][0]["audio_clips"][0]["unknown"] = true.into()),
            Box::new(|v| v["tracks"][0]["audio_clips"][1]["unknown"] = true.into()),
            Box::new(|v| v["clip_activation"][0]["unknown"] = true.into()),
            Box::new(|v| v["clip_activation"][1]["unknown"] = true.into()),
            Box::new(|v| {
                v["clip_activation"].as_array_mut().unwrap().pop();
            }),
            Box::new(|v| v["printed_output"][0]["printed"] = serde_json::json!([u64::MAX])),
            Box::new(|v| v["printed_output"][0]["suppressed"] = serde_json::json!([])),
            Box::new(|v| {
                v["printed_output"][0]["suppressed"] =
                    serde_json::json!([{"start":20,"end":120},{"start":100,"end":140}])
            }),
            Box::new(|v| {
                v["printed_output"].as_array_mut().unwrap().clear();
            }),
            Box::new(|v| {
                let mut row = v["alias_sources"][0].clone();
                row["id"] = u64::MAX.into();
                v["alias_sources"].as_array_mut().unwrap().push(row);
            }),
            Box::new(|v| v["alias_members"][0]["clip"] = u64::MAX.into()),
            Box::new(|v| {
                let row = v["alias_members"][0].clone();
                v["alias_members"].as_array_mut().unwrap().push(row);
            }),
            Box::new(|v| v["unknown"] = true.into()),
        ];
        for (index, mutate) in mutations.into_iter().enumerate() {
            let mut bad = good.clone();
            mutate(&mut bad);
            assert!(
                crate::decode(&serde_json::to_vec(&bad).unwrap()).is_err(),
                "malformed case {index} must not disappear in a projection"
            );
        }
    }
}
