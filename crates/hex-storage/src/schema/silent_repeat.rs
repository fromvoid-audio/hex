//! Schema118 retains independent frame-clock repeat periods over an older document.
use crate::{Project, SILENT_REPEAT_SCHEMA_VERSION, StorageError};
use hex_project::{MAX_CLIPS, TrackContentSnapshot};
use serde::{Deserialize, Serialize, de};
use serde_json::value::RawValue;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document<'a> {
    format: &'a str,
    version: u32,
    #[serde(borrow)]
    base: &'a RawValue,
    #[serde(deserialize_with = "bounded_rows")]
    clips: Vec<Row>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    clip: u64,
    period: u64,
    phase: u64,
}

fn bounded_rows<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Row>, D::Error> {
    struct Rows;
    impl<'de> de::Visitor<'de> for Rows {
        type Value = Vec<Row>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a bounded list of silent repeat clips")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut rows = Vec::new();
            while let Some(row) = seq.next_element()? {
                if rows.len() == MAX_CLIPS {
                    return Err(de::Error::custom("silent repeat clip limit reached"));
                }
                rows.push(row);
            }
            Ok(rows)
        }
    }
    d.deserialize_seq(Rows)
}

pub(crate) fn contains(project: &Project) -> bool {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .any(|clip| {
            clip.audio_edit()
                .is_some_and(|edit| edit.repeat_period.is_some())
        })
}

pub(crate) fn encode(project: &Project) -> Result<Vec<u8>, StorageError> {
    let mut snapshot = project.snapshot();
    let mut rows = Vec::new();
    for track in &mut snapshot.tracks {
        let clips = match &mut track.content {
            TrackContentSnapshot::Audio(clips) => clips,
            TrackContentSnapshot::Hybrid { audio_clips, .. } => audio_clips,
            TrackContentSnapshot::Instrument(_) => continue,
        };
        for clip in clips {
            let Some(edit) = clip.edit.as_mut() else {
                continue;
            };
            let Some(period) = edit.repeat_period.take() else {
                continue;
            };
            let placement = edit
                .placement
                .as_mut()
                .ok_or(StorageError::Invalid("silent repeat requires a placement"))?;
            rows.push(Row {
                clip: clip.id,
                period,
                phase: placement.cycle_phase,
            });
            let span = edit
                .view_end
                .checked_sub(edit.view_start)
                .filter(|n| *n > 0)
                .ok_or(StorageError::Invalid(
                    "silent repeat requires an audible span",
                ))?;
            placement.cycle_phase %= span;
        }
    }
    rows.sort_unstable_by_key(|row| row.clip);
    if rows.is_empty() || rows.len() > MAX_CLIPS {
        return Err(StorageError::Invalid("invalid silent repeat inventory"));
    }
    let prior = Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    let base = crate::encode(&prior)?;
    crate::encode_document(&Document {
        format: "hex-project",
        version: SILENT_REPEAT_SCHEMA_VERSION,
        base: serde_json::from_slice(&base)?,
        clips: rows,
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
    if header.version >= SILENT_REPEAT_SCHEMA_VERSION
        || document.clips.is_empty()
        || document
            .clips
            .windows(2)
            .any(|rows| rows[0].clip >= rows[1].clip)
    {
        return Err(StorageError::Invalid("invalid silent repeat envelope"));
    }
    let mut wanted: std::collections::BTreeMap<_, _> = document
        .clips
        .into_iter()
        .map(|row| (row.clip, (row.period, row.phase)))
        .collect();
    let mut snapshot = crate::decode(base)?.snapshot();
    for track in &mut snapshot.tracks {
        let clips = match &mut track.content {
            TrackContentSnapshot::Audio(clips) => clips,
            TrackContentSnapshot::Hybrid { audio_clips, .. } => audio_clips,
            TrackContentSnapshot::Instrument(_) => continue,
        };
        for clip in clips {
            let Some((period, phase)) = wanted.remove(&clip.id) else {
                continue;
            };
            let edit = clip
                .edit
                .as_mut()
                .ok_or(StorageError::Invalid("silent repeat clip has no edit"))?;
            let placement = edit
                .placement
                .as_mut()
                .ok_or(StorageError::Invalid("silent repeat requires a placement"))?;
            edit.repeat_period = Some(period);
            placement.cycle_phase = phase;
        }
    }
    if !wanted.is_empty() {
        return Err(StorageError::Invalid("silent repeat clip is missing"));
    }
    Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{AudioViewResizeOperation, Command, Created, Editor};

    fn project() -> Project {
        let mut editor = Editor::new(Project::new(16_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "source.wav".into(),
                sample_rate: 16_000,
                frames: 8_000,
            })
            .unwrap()
        else {
            panic!("audio fixture")
        };
        editor
            .execute(Command::ResizeAudioClipView {
                clip,
                operation: AudioViewResizeOperation::RepeatOuterEnd {
                    project_frame: 22_000,
                },
            })
            .unwrap();
        editor
            .execute(Command::ResizeAudioClipView {
                clip,
                operation: AudioViewResizeOperation::CycleEnd {
                    project_frame: 10_000,
                },
            })
            .unwrap();
        editor
            .execute(Command::AddArrangementLocator { frame: 17_000 })
            .unwrap();
        editor.project().clone()
    }

    #[test]
    fn round_trip_preserves_phase_in_silence_and_older_base() {
        let mut snapshot = project().snapshot();
        let clips = snapshot
            .tracks
            .iter_mut()
            .find_map(|track| match &mut track.content {
                TrackContentSnapshot::Audio(clips) if !clips.is_empty() => Some(clips),
                TrackContentSnapshot::Hybrid { audio_clips, .. } if !audio_clips.is_empty() => {
                    Some(audio_clips)
                }
                _ => None,
            })
            .unwrap();
        clips[0]
            .edit
            .as_mut()
            .unwrap()
            .placement
            .as_mut()
            .unwrap()
            .cycle_phase = 9_000;
        let project = Project::from_snapshot(snapshot).unwrap();
        let encoded = encode(&project).unwrap();
        assert_eq!(
            crate::schema_version_for(&project),
            SILENT_REPEAT_SCHEMA_VERSION
        );
        assert_eq!(
            crate::decode(&encoded).unwrap().snapshot(),
            project.snapshot()
        );
        let document: Document<'_> = serde_json::from_slice(&encoded).unwrap();
        let prior = crate::decode(document.base.get().as_bytes()).unwrap();
        assert!(!contains(&prior));
        assert_eq!(
            crate::schema_version_for(&prior),
            crate::ARRANGEMENT_LOCATOR_SCHEMA_VERSION
        );
    }

    #[test]
    fn malformed_rows_are_refused() {
        let encoded = encode(&project()).unwrap();
        let document: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        for mutation in 0..4 {
            let mut invalid = document.clone();
            match mutation {
                0 => invalid["clips"][0]["period"] = 0.into(),
                1 => invalid["clips"][0]["phase"] = 10_000.into(),
                2 => invalid["clips"][0]["clip"] = u64::MAX.into(),
                _ => {
                    let row = invalid["clips"][0].clone();
                    invalid["clips"].as_array_mut().unwrap().push(row);
                }
            }
            assert!(
                decode(&serde_json::to_vec(&invalid).unwrap()).is_err(),
                "case {mutation}"
            );
        }
    }
}
