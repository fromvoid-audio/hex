//! Schema116 stores exact plain long reversals over an unchanged prior document.
//! Each listed clip must be a plain long frame clip in the base; applying the
//! canonical reverse command reconstructs all of its authored metadata.
use crate::{LONG_REVERSE_SCHEMA_VERSION, Project, StorageError};
use hex_project::{Command, Editor, MAX_AUDIO_DOMAIN_FRAMES, MAX_CLIPS, TrackContentSnapshot};
use serde::{Deserialize, Serialize, de};
use serde_json::value::RawValue;
use std::collections::BTreeSet;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document<'a> {
    format: &'a str,
    version: u32,
    #[serde(borrow)]
    base: &'a RawValue,
    #[serde(deserialize_with = "bounded_clips")]
    clips: Vec<u64>,
}

fn bounded_clips<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u64>, D::Error> {
    struct Clips;
    impl<'de> de::Visitor<'de> for Clips {
        type Value = Vec<u64>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a bounded list of clip identities")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut clips = Vec::new();
            while let Some(clip) = seq.next_element()? {
                if clips.len() == MAX_CLIPS {
                    return Err(de::Error::custom("long reverse clip limit reached"));
                }
                clips.push(clip);
            }
            Ok(clips)
        }
    }
    d.deserialize_seq(Clips)
}

fn is_long_reverse(clip: &hex_project::Clip) -> bool {
    clip.duration() > MAX_AUDIO_DOMAIN_FRAMES
        && clip
            .audio_edit()
            .is_some_and(|edit| edit.is_plain_reversed_capture(clip))
}

pub(crate) fn contains(project: &Project) -> bool {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .any(is_long_reverse)
}

pub(crate) fn encode(project: &Project) -> Result<Vec<u8>, StorageError> {
    let ids: BTreeSet<u64> = project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .filter(|clip| is_long_reverse(clip))
        .map(|clip| clip.id().value())
        .collect();
    if ids.is_empty() || ids.len() > MAX_CLIPS {
        return Err(StorageError::Invalid("invalid long reverse inventory"));
    }
    let mut snapshot = project.snapshot();
    for track in &mut snapshot.tracks {
        let clips = match &mut track.content {
            TrackContentSnapshot::Audio(clips) => clips,
            TrackContentSnapshot::Hybrid { audio_clips, .. } => audio_clips,
            TrackContentSnapshot::Instrument(_) => continue,
        };
        for clip in clips {
            if ids.contains(&clip.id) {
                clip.edit = None;
            }
        }
    }
    let base = Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    let base = crate::encode(&base)?;
    crate::encode_document(&Document {
        format: "hex-project",
        version: LONG_REVERSE_SCHEMA_VERSION,
        base: serde_json::from_slice(&base)?,
        clips: ids.into_iter().collect(),
    })
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    if bytes.len() > crate::MAX_PROJECT_DOCUMENT_BYTES {
        return Err(StorageError::Invalid(
            "project exceeds the document size limit",
        ));
    }
    let document: Document<'_> = serde_json::from_slice(bytes)?;
    #[derive(Deserialize)]
    struct Header {
        version: u32,
    }
    let base = document.base.get().as_bytes();
    let header: Header = serde_json::from_slice(base)?;
    if header.version >= LONG_REVERSE_SCHEMA_VERSION
        || document.clips.is_empty()
        || document.clips.windows(2).any(|ids| ids[0] >= ids[1])
    {
        return Err(StorageError::Invalid("invalid long reverse envelope"));
    }
    let project = crate::decode(base)?;
    let wanted: BTreeSet<_> = document.clips.into_iter().collect();
    let mut selected = Vec::new();
    for clip in project.tracks().iter().flat_map(|track| track.clips()) {
        if wanted.contains(&clip.id().value()) {
            if clip.duration() <= MAX_AUDIO_DOMAIN_FRAMES || clip.audio_edit().is_some() {
                return Err(StorageError::Invalid("long reverse base clip is not plain"));
            }
            selected.push(clip.id());
        }
    }
    if selected.len() != wanted.len() {
        return Err(StorageError::Invalid("long reverse clip is missing"));
    }
    let mut editor = Editor::new(project, 1).map_err(|error| StorageError::Invalid(error.0))?;
    editor
        .execute(Command::ReverseClips(selected))
        .map_err(|error| StorageError::Invalid(error.0))?;
    Ok(editor.project().clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created};
    use serde_json::Value;

    fn fixture() -> (Project, Project, u64) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "long".into(),
                path: "long.wav".into(),
                sample_rate: 48_000,
                frames: 20_000_000,
            })
            .unwrap()
        else {
            panic!("audio")
        };
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        editor
            .execute(Command::AddArrangementLocator { frame: 100 })
            .unwrap();
        let base = editor.project().clone();
        editor.execute(Command::ReverseClips(vec![clip])).unwrap();
        (base, editor.project().clone(), clip.value())
    }

    #[test]
    fn long_reverse_round_trips_over_unchanged_older_document() {
        let (base, reversed, _) = fixture();
        let base_bytes = crate::encode(&base).unwrap();
        assert_eq!(crate::decode(&base_bytes).unwrap(), base);
        assert_eq!(
            crate::schema_version_for(&reversed),
            LONG_REVERSE_SCHEMA_VERSION
        );
        let bytes = crate::encode(&reversed).unwrap();
        let document: Document<'_> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(document.base.get().as_bytes(), base_bytes);
        assert_eq!(crate::decode(&bytes).unwrap(), reversed);
    }

    #[test]
    fn malformed_long_reverse_inventory_is_refused() {
        let (_, reversed, clip) = fixture();
        let bytes = crate::encode(&reversed).unwrap();
        let original: Value = serde_json::from_slice(&bytes).unwrap();
        for ids in [vec![], vec![clip, clip], vec![clip + 1]] {
            let mut invalid = original.clone();
            invalid["clips"] = serde_json::json!(ids);
            assert!(crate::decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
        let mut newer = original.clone();
        newer["base"]["version"] = LONG_REVERSE_SCHEMA_VERSION.into();
        assert!(crate::decode(&serde_json::to_vec(&newer).unwrap()).is_err());
        let mut unknown = original;
        unknown["unknown"] = true.into();
        assert!(crate::decode(&serde_json::to_vec(&unknown).unwrap()).is_err());

        // A lower-version document must not acquire the new acceptance rule.
        let mut snapshot = reversed.snapshot();
        snapshot.arrangement_locators.clear();
        let without_locators = Project::from_snapshot(snapshot).unwrap();
        assert!(super::super::track_name_origin::encode(&without_locators).is_err());
    }

    #[test]
    fn historical_projects_still_open_and_round_trip() {
        for bytes in [
            &include_bytes!("../../tests/fixtures/track-name-origin/schema62.json")[..],
            &include_bytes!("../../tests/fixtures/content-views/schema40-audio-4c66f735.json")[..],
        ] {
            let project = crate::decode(bytes).unwrap();
            assert_eq!(
                crate::decode(&crate::encode(&project).unwrap()).unwrap(),
                project
            );
        }
    }
}
