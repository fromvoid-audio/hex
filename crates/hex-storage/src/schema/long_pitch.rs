//! Schema117 stores exact, full-source V2 long plain pitch over a prior document.
//! Each row recreates the canonical pitch edit on an otherwise plain clip.
use crate::{LONG_PITCH_SCHEMA_VERSION, Project, StorageError};
use hex_project::{AudioClipEdit, MAX_AUDIO_DOMAIN_FRAMES, MAX_CLIPS, TrackContentSnapshot};
use serde::{Deserialize, Serialize, de};
use serde_json::value::RawValue;
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document<'a> {
    format: &'a str,
    version: u32,
    #[serde(borrow)]
    base: &'a RawValue,
    #[serde(deserialize_with = "bounded_clips")]
    clips: Vec<Pitch>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pitch {
    clip: u64,
    cents: i16,
}

fn bounded_clips<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Pitch>, D::Error> {
    struct Clips;
    impl<'de> de::Visitor<'de> for Clips {
        type Value = Vec<Pitch>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a bounded list of pitched clip identities")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut clips = Vec::new();
            while let Some(clip) = seq.next_element()? {
                if clips.len() == MAX_CLIPS {
                    return Err(de::Error::custom("long pitch clip limit reached"));
                }
                clips.push(clip);
            }
            Ok(clips)
        }
    }
    d.deserialize_seq(Clips)
}

fn pitch(clip: &hex_project::Clip) -> Option<i16> {
    let edit = clip.audio_edit()?;
    (clip.duration() > MAX_AUDIO_DOMAIN_FRAMES && edit.is_plain_pitch_capture(clip))
        .then_some(edit.domain.pitch_cents)
}

pub(crate) fn contains(project: &Project) -> bool {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .any(|clip| pitch(clip).is_some())
}

pub(crate) fn encode(project: &Project) -> Result<Vec<u8>, StorageError> {
    let clips: BTreeMap<u64, i16> = project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .filter_map(|clip| pitch(clip).map(|cents| (clip.id().value(), cents)))
        .collect();
    if clips.is_empty() || clips.len() > MAX_CLIPS {
        return Err(StorageError::Invalid("invalid long pitch inventory"));
    }
    let mut snapshot = project.snapshot();
    for track in &mut snapshot.tracks {
        let clips_in_track = match &mut track.content {
            TrackContentSnapshot::Audio(clips) => clips,
            TrackContentSnapshot::Hybrid { audio_clips, .. } => audio_clips,
            TrackContentSnapshot::Instrument(_) => continue,
        };
        for clip in clips_in_track {
            if clips.contains_key(&clip.id) {
                clip.edit = None;
            }
        }
    }
    let base = Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    let base = crate::encode(&base)?;
    crate::encode_document(&Document {
        format: "hex-project",
        version: LONG_PITCH_SCHEMA_VERSION,
        base: serde_json::from_slice(&base)?,
        clips: clips
            .into_iter()
            .map(|(clip, cents)| Pitch { clip, cents })
            .collect(),
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
    if header.version >= LONG_PITCH_SCHEMA_VERSION
        || document.clips.is_empty()
        || document
            .clips
            .windows(2)
            .any(|rows| rows[0].clip >= rows[1].clip)
        || document
            .clips
            .iter()
            .any(|row| row.cents == 0 || !(-4_800..=4_800).contains(&row.cents))
    {
        return Err(StorageError::Invalid("invalid long pitch envelope"));
    }
    let project = crate::decode(base)?;
    let mut wanted: BTreeMap<_, _> = document
        .clips
        .into_iter()
        .map(|row| (row.clip, row.cents))
        .collect();
    let mut selected = BTreeMap::new();
    for clip in project.tracks().iter().flat_map(|track| track.clips()) {
        if let Some(cents) = wanted.remove(&clip.id().value()) {
            if clip.duration() <= MAX_AUDIO_DOMAIN_FRAMES || clip.audio_edit().is_some() {
                return Err(StorageError::Invalid("long pitch base clip is not plain"));
            }
            let edit = AudioClipEdit::reconstruct_plain_pitch(clip, cents)
                .map_err(|error| StorageError::Invalid(error.0))?;
            selected.insert(clip.id().value(), edit);
        }
    }
    if !wanted.is_empty() {
        return Err(StorageError::Invalid("long pitch clip is missing"));
    }
    let mut snapshot = project.snapshot();
    for track in &mut snapshot.tracks {
        let clips = match &mut track.content {
            TrackContentSnapshot::Audio(clips) => clips,
            TrackContentSnapshot::Hybrid { audio_clips, .. } => audio_clips,
            TrackContentSnapshot::Instrument(_) => continue,
        };
        for clip in clips {
            if let Some(edit) = selected.remove(&clip.id) {
                clip.edit = Some(edit);
            }
        }
    }
    if !selected.is_empty() {
        return Err(StorageError::Invalid("long pitch clip is missing"));
    }
    Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor};
    use serde_json::Value;

    fn fixture() -> (Project, Project, u64) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let mut clips = Vec::new();
        for name in ["reverse", "pitch", "up"] {
            let Created::Imported { clip, .. } = editor
                .execute(Command::ImportAudioTrack {
                    name: name.into(),
                    path: format!("{name}.wav").into(),
                    sample_rate: 48_000,
                    frames: 20_000_000,
                })
                .unwrap()
            else {
                panic!("audio fixture")
            };
            editor
                .execute(Command::SetClipFades {
                    clip,
                    fade_in: 0,
                    fade_out: 0,
                })
                .unwrap();
            clips.push(clip);
        }
        editor
            .execute(Command::ReverseClips(vec![clips[0]]))
            .unwrap();
        let base = editor.project().clone();
        editor
            .execute(Command::SetAudioClipPitch {
                clip: clips[1],
                cents: -700,
            })
            .unwrap();
        editor
            .execute(Command::SetAudioClipPitch {
                clip: clips[2],
                cents: 700,
            })
            .unwrap();
        (base, editor.project().clone(), clips[1].value())
    }

    #[test]
    fn long_pitch_composes_with_long_reverse_and_round_trips_complete_state() {
        let (base, pitched, _) = fixture();
        let base_bytes = crate::encode(&base).unwrap();
        assert_eq!(
            crate::schema_version_for(&base),
            crate::LONG_REVERSE_SCHEMA_VERSION
        );
        assert_eq!(
            crate::schema_version_for(&pitched),
            LONG_PITCH_SCHEMA_VERSION
        );
        let encoded = crate::encode(&pitched).unwrap();
        let document: Document<'_> = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(document.base.get().as_bytes(), base_bytes);
        let reopened = crate::decode(&encoded).unwrap();
        assert_eq!(reopened, pitched);
        let mut pitch_count = 0;
        for clip in reopened.tracks().iter().flat_map(|track| track.clips()) {
            if let Some(edit) = clip
                .audio_edit()
                .filter(|edit| edit.is_plain_pitch_capture(clip))
            {
                pitch_count += 1;
                assert_eq!(edit.domain.warp_resampler, hex_project::WarpResampler::V2);
            }
        }
        assert_eq!(pitch_count, 2);
        assert_eq!(crate::decode(&base_bytes).unwrap(), base);
    }

    #[test]
    fn malformed_pitch_inventory_and_newer_base_are_refused() {
        let (_, pitched, clip) = fixture();
        let mut wrong_law = pitched.snapshot();
        for track in &mut wrong_law.tracks {
            let clips = match &mut track.content {
                TrackContentSnapshot::Audio(clips) => clips,
                TrackContentSnapshot::Hybrid { audio_clips, .. } => audio_clips,
                TrackContentSnapshot::Instrument(_) => continue,
            };
            for entry in clips {
                if entry.id == clip {
                    let edit = entry.edit.as_mut().unwrap();
                    std::sync::Arc::make_mut(&mut edit.domain).warp_resampler =
                        hex_project::WarpResampler::V1;
                }
            }
        }
        assert!(Project::from_snapshot(wrong_law).is_err());
        let original: Value = serde_json::from_slice(&crate::encode(&pitched).unwrap()).unwrap();
        for rows in [
            serde_json::json!([]),
            serde_json::json!([{ "clip": clip, "cents": 0 }]),
            serde_json::json!([{ "clip": clip, "cents": 4801 }]),
            serde_json::json!([{ "clip": clip + 1, "cents": 1 }]),
            serde_json::json!([{ "clip": clip, "cents": 1 }, { "clip": clip, "cents": 2 }]),
            serde_json::json!([{ "clip": clip, "cents": 1, "extra": true }]),
        ] {
            let mut invalid = original.clone();
            invalid["clips"] = rows;
            assert!(crate::decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
        let mut newer = original.clone();
        newer["base"]["version"] = LONG_PITCH_SCHEMA_VERSION.into();
        assert!(crate::decode(&serde_json::to_vec(&newer).unwrap()).is_err());
        let mut unknown = original;
        unknown["unknown"] = true.into();
        assert!(crate::decode(&serde_json::to_vec(&unknown).unwrap()).is_err());
    }
}
