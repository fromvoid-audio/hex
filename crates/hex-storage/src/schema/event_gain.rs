//! Schema61 layers sparse per-event gain over the evidenced schema60 source
//! tree. Owners are (visible clip, bounded retained-source path, local event).
//! Schema1–60 had no event gain and decode to exact unity; originals stay intact.
use crate::{Project, StorageError};
use hex_project::{AudioEvent, ClipGain, ClipSnapshot, Snapshot, TrackContentSnapshot};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Gain {
    clip: u64,
    #[serde(deserialize_with = "bounded_path")]
    sources: Vec<u64>,
    event: u64,
    millidb: i32,
}
fn bounded_path<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u64>, D::Error> {
    struct Path;
    impl<'de> serde::de::Visitor<'de> for Path {
        type Value = Vec<u64>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a bounded retained audio source path")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut path = Vec::new();
            while let Some(id) = seq.next_element::<u64>()? {
                if id == 0 || path.len() == hex_project::MAX_RETAINED_AUDIO_DEPTH {
                    return Err(serde::de::Error::custom("invalid retained audio gain path"));
                }
                path.push(id);
            }
            Ok(path)
        }
    }
    d.deserialize_seq(Path)
}
fn invalid(error: hex_project::Invalid) -> StorageError {
    StorageError::Invalid(error.0)
}

// Mutate only a private snapshot. Each restored node keeps its original media,
// tempo and complete processing; depth and aggregate root work remain bounded.
fn visit_clip(
    project: &Project,
    clip: &mut ClipSnapshot,
    owner: u64,
    path: &mut Vec<u64>,
    roots: &mut usize,
    visit: &mut impl FnMut(u64, &[u64], &mut AudioEvent),
) -> Result<(), StorageError> {
    if path.len() > hex_project::MAX_RETAINED_AUDIO_DEPTH {
        return Err(StorageError::Invalid(
            "retained audio nesting limit reached",
        ));
    }
    let Some(sequence) = clip
        .edit
        .as_mut()
        .and_then(|edit| Arc::make_mut(&mut edit.domain).events.as_mut())
    else {
        return Ok(());
    };
    let sequence = Arc::make_mut(sequence);
    for event in &mut sequence.events {
        visit(owner, path, event);
    }
    for root in &mut sequence.sources {
        *roots = roots
            .checked_add(1)
            .filter(|n| *n <= hex_project::MAX_CLIPS)
            .ok_or(StorageError::Invalid(
                "project retained audio root limit reached",
            ))?;
        path.push(root.id);
        let mut retained = root.clip.retained_snapshot();
        visit_clip(project, &mut retained, owner, path, roots, visit)?;
        *root = project
            .restore_retained_audio_root(root.id, retained, Arc::clone(&root.tempo))
            .map_err(invalid)?;
        path.pop();
    }
    Ok(())
}
fn visit_project(
    project: &Project,
    snapshot: &mut Snapshot,
    mut visit: impl FnMut(u64, &[u64], &mut AudioEvent),
) -> Result<(), StorageError> {
    let mut roots = 0;
    let mut path = Vec::new();
    for track in &mut snapshot.tracks {
        let clips = match &mut track.content {
            TrackContentSnapshot::Audio(clips) => clips,
            TrackContentSnapshot::Hybrid { audio_clips, .. } => audio_clips,
            TrackContentSnapshot::Instrument(_) => continue,
        };
        for clip in clips {
            visit_clip(project, clip, clip.id, &mut path, &mut roots, &mut visit)?;
        }
    }
    Ok(())
}
// Alias members share event content. The lowest member id is the stable
// inventory owner; every member must receive the same projection.
fn alias_owners(snapshot: &Snapshot) -> Result<BTreeMap<u64, u64>, StorageError> {
    let mut owners = BTreeMap::new();
    for member in &snapshot.aliases.members {
        owners
            .entry(member.source)
            .and_modify(|id: &mut u64| *id = (*id).min(member.clip))
            .or_insert(member.clip);
    }
    snapshot
        .aliases
        .members
        .iter()
        .map(|member| {
            let owner = owners
                .get(&member.source)
                .copied()
                .ok_or(StorageError::Invalid("audio alias owner missing"))?;
            Ok((member.clip, owner))
        })
        .collect()
}
fn stored_event_count(project: &Project) -> Result<usize, StorageError> {
    let mut snapshot = project.snapshot();
    let mut count = 0usize;
    let mut overflow = false;
    visit_project(project, &mut snapshot, |_, _, _| {
        if let Some(next) = count.checked_add(1) {
            count = next;
        } else {
            overflow = true;
        }
    })?;
    if overflow {
        return Err(StorageError::Invalid("audio event gain count overflow"));
    }
    Ok(count)
}
pub(crate) fn require_unity(project: &Project) -> Result<(), StorageError> {
    for clip in project.tracks().iter().flat_map(|track| track.clips()) {
        for source in
            std::iter::once(clip).chain(clip.retained_audio_roots().map(|root| &root.clip))
        {
            if source
                .audio_edit()
                .and_then(|edit| edit.domain.events.as_ref())
                .is_some_and(|events| {
                    events
                        .events
                        .iter()
                        .any(|event| event.gain != ClipGain::UNITY)
                })
            {
                return Err(StorageError::Invalid(
                    "older schema cannot save audio event gain",
                ));
            }
        }
    }
    Ok(())
}
pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    super::midi_clip_gain::require_unity(project)?;
    let mut snapshot = project.snapshot();
    let mut gains = Vec::new();
    let owners = alias_owners(&snapshot)?;
    visit_project(project, &mut snapshot, |clip, path, event| {
        if event.gain != ClipGain::UNITY {
            if owners.get(&clip).is_none_or(|owner| *owner == clip) {
                gains.push(Gain {
                    clip,
                    sources: path.to_vec(),
                    event: event.id,
                    millidb: event.gain.millidb(),
                });
            }
            event.gain = ClipGain::UNITY;
        }
    })?;
    gains
        .sort_unstable_by(|a, b| (a.clip, &a.sources, a.event).cmp(&(b.clip, &b.sources, b.event)));
    super::audio_alias_projection::synchronize(&mut snapshot)?;
    let neutral = Project::from_snapshot(snapshot).map_err(invalid)?;
    let mut value = super::alias_projection_v60::encode(&neutral)?;
    value["audio_event_gains"] = serde_json::to_value(gains)?;
    value["version"] = 61.into();
    Ok(value)
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_value(super::audio_events::parse_unique(bytes)?)
}
/// Decode an envelope parsed once by [`super::audio_events::parse_unique`].
pub(crate) fn decode_value(mut value: Value) -> Result<Project, StorageError> {
    if value.get("version").and_then(Value::as_u64) != Some(61) {
        return Err(StorageError::Invalid("invalid audio event gain schema"));
    }
    let raw = value
        .as_object_mut()
        .and_then(|value| value.remove("audio_event_gains"))
        .ok_or(StorageError::Invalid("audio event gain inventory missing"))?;
    value["version"] = 60.into();
    let project = super::alias_projection_v60::decode_value(value)?;
    let length = raw
        .as_array()
        .ok_or(StorageError::Invalid("invalid audio event gain inventory"))?
        .len();
    // Bound the owned inventory by actual validated event custody, before
    // deserializing another vector. No unseen owner may extend the model limit.
    if length > stored_event_count(&project)? {
        return Err(StorageError::Invalid(
            "audio event gain inventory exceeds event count",
        ));
    }
    let records: Vec<Gain> = serde_json::from_value(raw)?;
    type ByPath = BTreeMap<Vec<u64>, BTreeMap<u64, ClipGain>>;
    let mut gains: BTreeMap<u64, ByPath> = BTreeMap::new();
    for row in records {
        let gain = ClipGain::new(row.millidb).map_err(invalid)?;
        if row.clip == 0 || row.event == 0 || gain == ClipGain::UNITY {
            return Err(StorageError::Invalid("invalid non-unity audio event gain"));
        }
        if gains
            .entry(row.clip)
            .or_default()
            .entry(row.sources)
            .or_default()
            .insert(row.event, gain)
            .is_some()
        {
            return Err(StorageError::Invalid("duplicate audio event gain"));
        }
    }
    let mut snapshot = project.snapshot();
    let owners = alias_owners(&snapshot)?;
    let mut used = BTreeSet::new();
    visit_project(&project, &mut snapshot, |clip, path, event| {
        let owner = owners.get(&clip).copied().unwrap_or(clip);
        if let Some(gain) = gains
            .get(&owner)
            .and_then(|paths| paths.get(path))
            .and_then(|events| events.get(&event.id))
        {
            event.gain = *gain;
            used.insert((owner, path.to_vec(), event.id));
        }
    })?;
    let expected = gains.iter().flat_map(|(clip, paths)| {
        paths.iter().flat_map(move |(path, events)| {
            events
                .keys()
                .map(move |event| (*clip, path.clone(), *event))
        })
    });
    if expected.into_iter().any(|key| !used.contains(&key)) {
        return Err(StorageError::Invalid("audio event gain owner missing"));
    }
    super::audio_alias_projection::synchronize(&mut snapshot)?;
    Project::from_snapshot(snapshot).map_err(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{AudioEventEdit, Command, Created, Editor};
    fn alias_fixture(gain: i32, active: bool) -> Project {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Imported { track, clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 100,
            })
            .unwrap()
        else {
            panic!("import");
        };
        editor
            .execute(Command::SetTrackColor {
                track,
                color: hex_project::ElementColor::Red,
            })
            .unwrap();
        for operation in [
            AudioEventEdit::SetGain {
                event: 1,
                gain: ClipGain::new(gain).unwrap(),
            },
            AudioEventEdit::SetActive {
                events: vec![1],
                active,
            },
        ] {
            editor
                .execute(Command::EditAudioEvents { clip, operation })
                .unwrap();
        }
        editor
            .execute(Command::DuplicateAliasClips(vec![clip]))
            .unwrap();
        editor.project().clone()
    }
    #[test]
    fn shared_gain_is_saved_once_and_roundtrips_the_complete_source() {
        let project = alias_fixture(-6_000, true);
        let value = encode(&project).unwrap();
        assert_eq!(value["audio_event_gains"].as_array().unwrap().len(), 1);
        assert_eq!(
            crate::decode(&serde_json::to_vec(&value).unwrap())
                .unwrap()
                .snapshot(),
            project.snapshot()
        );
    }
    #[test]
    fn inactive_aliases_coexist_with_gain_and_frozen_schema58() {
        let project = alias_fixture(0, false);
        let old = super::super::nested_audio::encode(&project).unwrap();
        assert_eq!(
            super::super::nested_audio::decode(&serde_json::to_vec(&old).unwrap())
                .unwrap()
                .snapshot(),
            project.snapshot()
        );
        assert_eq!(
            crate::decode(&crate::encode(&project).unwrap())
                .unwrap()
                .snapshot(),
            project.snapshot()
        );
        let project = alias_fixture(-6_000, false);
        assert_eq!(
            crate::decode(&crate::encode(&project).unwrap())
                .unwrap()
                .snapshot(),
            project.snapshot()
        );
    }
    #[test]
    fn schema60_migrates_at_unity_and_gain_inventory_is_strict() {
        let project = alias_fixture(0, false);
        let old = super::super::alias_projection_v60::encode(&project).unwrap();
        let frozen =
            include_bytes!("../../tests/fixtures/event-gain/schema60-aliased-inactive.json");
        // JSON object ordering varies with serde_json feature unification; the
        // frozen writer output must represent the same complete document.
        assert_eq!(old, serde_json::from_slice::<Value>(frozen).unwrap());
        assert_eq!(
            crate::decode(frozen).unwrap().snapshot(),
            project.snapshot()
        );
        let gained = alias_fixture(-6_000, false);
        let valid = encode(&gained).unwrap();
        let mut duplicate = valid.clone();
        let row = duplicate["audio_event_gains"][0].clone();
        duplicate["audio_event_gains"]
            .as_array_mut()
            .unwrap()
            .push(row.clone());
        assert!(crate::decode(&serde_json::to_vec(&duplicate).unwrap()).is_err());
        let mut unknown = valid.clone();
        unknown["audio_event_gains"][0]["event"] = u64::MAX.into();
        assert!(crate::decode(&serde_json::to_vec(&unknown).unwrap()).is_err());
        let mut unity = valid.clone();
        unity["audio_event_gains"][0]["millidb"] = 0.into();
        assert!(crate::decode(&serde_json::to_vec(&unity).unwrap()).is_err());
        let mut out_of_range = valid.clone();
        out_of_range["audio_event_gains"][0]["millidb"] = 24001.into();
        assert!(crate::decode(&serde_json::to_vec(&out_of_range).unwrap()).is_err());
        let mut bad_path = valid.clone();
        bad_path["audio_event_gains"][0]["sources"] = serde_json::json!([0]);
        assert!(crate::decode(&serde_json::to_vec(&bad_path).unwrap()).is_err());
        let mut missing = valid;
        missing.as_object_mut().unwrap().remove("audio_event_gains");
        assert!(crate::decode(&serde_json::to_vec(&missing).unwrap()).is_err());
    }
}
