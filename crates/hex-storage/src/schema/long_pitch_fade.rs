//! Schema119 stores exact long full-source V2 pitch with a retained frame-clock fade.
//! Each row recreates the canonical pitch edit on an otherwise plain clip.
use crate::{LONG_PITCH_FADE_SCHEMA_VERSION, Project, StorageError};
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
    (clip.duration() > MAX_AUDIO_DOMAIN_FRAMES
        && edit.is_plain_pitch_capture(clip)
        && edit.domain.fade != clip.fade())
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
                let edit = clip
                    .edit
                    .take()
                    .ok_or(StorageError::Invalid("long pitch edit missing"))?;
                clip.fade = edit.domain.fade;
            }
        }
    }
    let base = Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    let base = crate::encode(&base)?;
    crate::encode_document(&Document {
        format: "hex-project",
        version: LONG_PITCH_FADE_SCHEMA_VERSION,
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
    if header.version >= LONG_PITCH_FADE_SCHEMA_VERSION
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
        return Err(StorageError::Invalid("invalid long pitch fade envelope"));
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
            if clip.duration() <= MAX_AUDIO_DOMAIN_FRAMES
                || clip.audio_edit().is_some()
                || clip.fade()
                    == hex_project::ClipFade::neutral(clip.duration())
                        .map_err(|error| StorageError::Invalid(error.0))?
            {
                return Err(StorageError::Invalid("long pitch base clip is not plain"));
            }
            let edit = AudioClipEdit::reconstruct_faded_pitch(clip, cents)
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
                clip.fade = hex_project::ClipFade::neutral(clip.duration)
                    .map_err(|error| StorageError::Invalid(error.0))?;
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

    #[test]
    fn inactive_fade_curve_settings_are_not_lost() {
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
            panic!("fixture")
        };
        let fade = hex_project::ClipFade::neutral(20_000_000)
            .unwrap()
            .with_shape(0, 0, 200, -300)
            .unwrap();
        assert!(fade.is_neutral());
        editor
            .execute(Command::SetClipFadeEnvelope { clip, fade })
            .unwrap();
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 700 })
            .unwrap();
        let project = editor.project();
        assert_eq!(
            crate::schema_version_for(project),
            LONG_PITCH_FADE_SCHEMA_VERSION
        );
        assert_eq!(
            &crate::decode(&crate::encode(project).unwrap()).unwrap(),
            project
        );
    }

    #[test]
    fn long_pitch_fade_round_trips_full_state_and_rejects_malformed_rows() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "long".into(),
                path: "long.wav".into(),
                sample_rate: 48_000,
                frames: 20_000_000,
            })
            .unwrap()
        else {
            panic!("fixture")
        };
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 1234,
                fade_out: 5678,
            })
            .unwrap();
        let fade = editor
            .project()
            .clip(clip)
            .unwrap()
            .fade()
            .with_shape(0, 0, 300, -250)
            .unwrap()
            .with_curve_positions(330, 670)
            .unwrap()
            .with_curve_laws([hex_project::FadeCurveLaw::SmoothStep; 2]);
        editor
            .execute(Command::SetClipFadeEnvelope { clip, fade })
            .unwrap();
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 700 })
            .unwrap();
        let Created::Imported { clip: neutral, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "neutral".into(),
                path: "neutral.wav".into(),
                sample_rate: 48_000,
                frames: 20_000_000,
            })
            .unwrap()
        else {
            panic!("fixture")
        };
        editor
            .execute(Command::SetClipFades {
                clip: neutral,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        editor
            .execute(Command::SetAudioClipPitch {
                clip: neutral,
                cents: -700,
            })
            .unwrap();
        let project = editor.project();
        assert_eq!(
            crate::schema_version_for(project),
            LONG_PITCH_FADE_SCHEMA_VERSION
        );
        let bytes = crate::encode(project).unwrap();
        let document: Document<'_> = serde_json::from_slice(&bytes).unwrap();
        let base: Value = serde_json::from_str(document.base.get()).unwrap();
        assert_eq!(base["version"], crate::LONG_PITCH_SCHEMA_VERSION);
        assert!(crate::decode(document.base.get().as_bytes()).is_ok());
        assert_eq!(&crate::decode(&bytes).unwrap(), project);
        let mut malformed: Value = serde_json::from_slice(&bytes).unwrap();
        malformed["clips"][0]["cents"] = 0.into();
        assert!(crate::decode(&serde_json::to_vec(&malformed).unwrap()).is_err());
        let mut malformed: Value = serde_json::from_slice(&bytes).unwrap();
        malformed["base"]["version"] = LONG_PITCH_FADE_SCHEMA_VERSION.into();
        assert!(crate::decode(&serde_json::to_vec(&malformed).unwrap()).is_err());
    }
}
