//! Schema41 adds finite directed Arrangement read views over the exact schema40
//! envelope. Older supported files acquire identity views in memory. Original
//! bytes/media are never changed by decoding; older writers refuse these views.
use super::{Project, StorageError};
use hex_project::{
    AudioContentView, AudioDomainClock, AudioViewMode, AudioViewPlacement, BeatTime, ContentWindow,
    Snapshot, TrackContentSnapshot,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Window {
    start: u64,
    end: u64,
    reversed: bool,
}
impl From<ContentWindow> for Window {
    fn from(w: ContentWindow) -> Self {
        Self {
            start: w.start,
            end: w.end,
            reversed: w.reversed,
        }
    }
}
impl Window {
    fn checked(self) -> Result<ContentWindow, StorageError> {
        ContentWindow::new(self.start, self.end, self.reversed)
            .map_err(|e| StorageError::Invalid(e.0))
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioView {
    restore_plain: bool,
    base_view_start: u64,
    base_view_end: u64,
    #[serde(deserialize_with = "required_placement")]
    base_placement: Option<Placement>,
    base_start_delta: String,
    window: Window,
    /// Schema66 read law, present only when it differs from
    /// [`AudioContentView::implied_read_resampler`]; older layouts never
    /// contain it (see `schema::content_view_resampler`).
    #[serde(
        default,
        deserialize_with = "crate::audio_edit::present_warp_resampler",
        skip_serializing_if = "Option::is_none"
    )]
    read_resampler: Option<u8>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Placement {
    repeat: bool,
    beats: bool,
    length: u64,
    phase: u64,
}
fn required_placement<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Placement>, D::Error> {
    Option::deserialize(d)
}
impl AudioView {
    fn new(v: AudioContentView, clock: AudioDomainClock) -> Self {
        Self {
            restore_plain: v.restore_plain,
            base_view_start: v.base_view_start,
            base_view_end: v.base_view_end,
            base_placement: v.base_placement.map(|p| Placement {
                repeat: p.mode == AudioViewMode::Repeat,
                beats: matches!(p.outer_length, AudioDomainClock::Beats(_)),
                length: p.outer_length.length(),
                phase: p.cycle_phase,
            }),
            base_start_delta: v.base_start_delta.to_string(),
            window: v.window.into(),
            read_resampler: v
                .has_explicit_read_resampler(clock)
                .then_some(crate::audio_edit::warp_resampler_tag(v.read_resampler)),
        }
    }
    fn checked(self, clock: AudioDomainClock) -> Result<AudioContentView, StorageError> {
        let implied = AudioContentView::implied_read_resampler(clock);
        let read_resampler = self
            .read_resampler
            .map_or(Ok(implied), crate::audio_edit::warp_resampler_from_tag)?;
        if self.read_resampler.is_some() && read_resampler == implied {
            return Err(StorageError::Invalid(
                "redundant read resampler version in directed audio view",
            ));
        }
        Ok(AudioContentView {
            restore_plain: self.restore_plain,
            base_view_start: self.base_view_start,
            base_view_end: self.base_view_end,
            base_placement: self.base_placement.map(|p| AudioViewPlacement {
                mode: if p.repeat {
                    AudioViewMode::Repeat
                } else {
                    AudioViewMode::Once
                },
                outer_length: if p.beats {
                    AudioDomainClock::Beats(BeatTime::from_raw(p.length))
                } else {
                    AudioDomainClock::Frames(p.length)
                },
                cycle_phase: p.phase,
            }),
            base_start_delta: self
                .base_start_delta
                .parse()
                .map_err(|_| StorageError::Invalid("invalid audio content base displacement"))?,
            window: self.window.checked()?,
            read_resampler,
        })
    }
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum View {
    Audio {
        #[serde(deserialize_with = "required_audio")]
        view: Option<AudioView>,
    },
    Midi {
        #[serde(deserialize_with = "required_midi")]
        view: Option<Window>,
    },
}
fn required_audio<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<AudioView>, D::Error> {
    Option::deserialize(d)
}
fn required_midi<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Window>, D::Error> {
    Option::deserialize(d)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    clip: u64,
    view: View,
}
#[derive(Deserialize)]
struct Rows(#[serde(deserialize_with = "bounded")] Vec<Row>);
fn bounded<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Row>, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = Vec<Row>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded directed clip view inventory")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut rows = Vec::new();
            while let Some(row) = seq.next_element()? {
                if rows.len() == hex_project::MAX_CLIPS {
                    return Err(serde::de::Error::custom(
                        "directed view inventory limit reached",
                    ));
                }
                rows.push(row);
            }
            Ok(rows)
        }
    }
    d.deserialize_seq(Visitor)
}
fn visit(
    snapshot: &mut Snapshot,
    mut audio: impl FnMut(&mut hex_project::ClipSnapshot) -> Result<(), StorageError>,
    mut midi: impl FnMut(&mut hex_project::MidiClipSnapshot) -> Result<(), StorageError>,
) -> Result<(), StorageError> {
    for track in &mut snapshot.tracks {
        match &mut track.content {
            TrackContentSnapshot::Hybrid {
                audio_clips,
                midi_clips,
            } => {
                for c in audio_clips {
                    audio(c)?;
                }
                for c in midi_clips {
                    midi(c)?;
                }
            }
            TrackContentSnapshot::Audio(clips) => {
                for c in clips {
                    audio(c)?;
                }
            }
            TrackContentSnapshot::Instrument(clips) => {
                for c in clips {
                    midi(c)?;
                }
            }
        }
    }
    Ok(())
}
pub(crate) fn require_legacy_compatible(project: &Project) -> Result<(), StorageError> {
    if project.tracks().iter().any(|t| {
        t.clips()
            .iter()
            .any(|c| c.audio_edit().is_some_and(|e| e.content_view.is_some()))
            || t.midi_clips().iter().any(|c| c.content_view().is_some())
    }) {
        return Err(StorageError::Invalid(
            "older project formats cannot represent directed clip content; save in the current format",
        ));
    }
    Ok(())
}
pub(crate) fn encode(project: &Project) -> Result<serde_json::Value, StorageError> {
    let mut snapshot = project.snapshot();
    let mut audio_rows = Vec::new();
    let mut midi_rows = Vec::new();
    visit(
        &mut snapshot,
        |c| {
            let view = c.edit.as_mut().and_then(|edit| {
                let clock = edit.domain.clock;
                edit.content_view.take().map(|v| AudioView::new(v, clock))
            });
            audio_rows.push(Row {
                clip: c.id,
                view: View::Audio { view },
            });
            Ok(())
        },
        |c| {
            midi_rows.push(Row {
                clip: c.id,
                view: View::Midi {
                    view: c.content_view.take().map(Into::into),
                },
            });
            Ok(())
        },
    )?;
    audio_rows.extend(midi_rows);
    let legacy = Project::from_legacy_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))?;
    let mut value = super::clip_activation::encode(&legacy)?;
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid directed content envelope"))?;
    object.insert("version".into(), 41.into());
    object.insert("content_views".into(), serde_json::to_value(audio_rows)?);
    Ok(value)
}
#[cfg(test)]
pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_value(super::audio_events::parse_unique(bytes)?)
}
/// Decode an envelope parsed once by [`super::audio_events::parse_unique`].
pub(crate) fn decode_value(mut value: serde_json::Value) -> Result<Project, StorageError> {
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid directed content envelope"))?;
    if object.get("version").and_then(|v| v.as_u64()) != Some(41) {
        return Err(StorageError::Invalid("invalid directed content version"));
    }
    let inventory: Rows = serde_json::from_value(object.remove("content_views").ok_or(
        StorageError::Invalid("directed content inventory is missing"),
    )?)?;
    object.insert("version".into(), 40.into());
    let legacy = super::clip_activation::decode_value(value)?;
    let mut rows = BTreeMap::new();
    for row in inventory.0 {
        if rows.insert(row.clip, row.view).is_some() {
            return Err(StorageError::Invalid(
                "duplicate directed content clip identity",
            ));
        }
    }
    // Separate passes avoid sharing mutable closure custody.
    let mut snapshot = legacy.snapshot();
    for track in &mut snapshot.tracks {
        let (audio, midi) = match &mut track.content {
            TrackContentSnapshot::Hybrid {
                audio_clips,
                midi_clips,
            } => (audio_clips.as_mut_slice(), midi_clips.as_mut_slice()),
            TrackContentSnapshot::Audio(clips) => (clips.as_mut_slice(), &mut [][..]),
            TrackContentSnapshot::Instrument(clips) => (&mut [][..], clips.as_mut_slice()),
        };
        for c in audio {
            let view = rows
                .remove(&c.id)
                .ok_or(StorageError::Invalid("directed content row is missing"))?;
            let View::Audio { view } = view else {
                return Err(StorageError::Invalid(
                    "midi content view references an audio clip",
                ));
            };
            if let Some(view) = view {
                let edit = c.edit.as_mut().ok_or(StorageError::Invalid(
                    "directed audio view requires a retained domain",
                ))?;
                edit.content_view = Some(view.checked(edit.domain.clock)?);
            }
        }
        for c in midi {
            let view = rows
                .remove(&c.id)
                .ok_or(StorageError::Invalid("directed content row is missing"))?;
            let View::Midi { view } = view else {
                return Err(StorageError::Invalid(
                    "audio content view references a midi clip",
                ));
            };
            c.content_view = view.map(Window::checked).transpose()?;
        }
    }
    if !rows.is_empty() {
        return Err(StorageError::Invalid(
            "directed content references a missing arrangement clip",
        ));
    }
    Project::from_legacy_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}

#[cfg(test)]
mod tests {
    use hex_project::{Command, Editor};
    #[test]
    fn schema41_refuses_unbounded_negative_frame_base_before_projection_math() {
        use hex_project::{
            AudioDomainClock, AudioViewMode, AudioViewPlacement, Created, Project,
            TrackContentSnapshot,
        };
        let mut editor = Editor::new(Project::new(8_000).unwrap(), 16).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "original".into(),
                path: "Original.wav".into(),
                sample_rate: 8_000,
                frames: 8_000,
            })
            .unwrap()
        else {
            panic!("audio fixture")
        };
        editor.execute(Command::ReverseClips(vec![clip])).unwrap();
        let mut snapshot = editor.project().snapshot();
        let clips = match &mut snapshot.tracks[0].content {
            TrackContentSnapshot::Audio(clips)
            | TrackContentSnapshot::Hybrid {
                audio_clips: clips, ..
            } => clips,
            _ => panic!("audio fixture"),
        };
        let view = clips[0]
            .edit
            .as_mut()
            .unwrap()
            .content_view
            .as_mut()
            .unwrap();
        view.base_view_start = 1_000;
        view.base_view_end = 2_000;
        view.base_placement = Some(AudioViewPlacement {
            mode: AudioViewMode::Once,
            outer_length: AudioDomainClock::Frames(8_000),
            cycle_phase: 0,
        });
        let project = Project::from_legacy_snapshot(snapshot).unwrap();
        let original = crate::encode(&project).unwrap();
        let document: serde_json::Value = serde_json::from_slice(&original).unwrap();
        for delta in [i128::MIN + 1, -2 * i128::from(u64::MAX) - 1, i128::MAX] {
            let mut invalid = document.clone();
            invalid["content_views"][0]["view"]["view"]["base_start_delta"] =
                delta.to_string().into();
            assert!(crate::decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
        let mut negative = document;
        negative["content_views"][0]["view"]["view"]["base_start_delta"] = "-1000".into();
        assert!(
            crate::decode(&serde_json::to_vec(&negative).unwrap()).is_ok(),
            "a retained base before zero remains legal"
        );
        assert_eq!(
            crate::decode(&original).unwrap().snapshot(),
            project.snapshot()
        );
    }
    #[test]
    fn schema41_migrates_genuine40_and_roundtrips_complete_directed_state() {
        for bytes in [
            include_bytes!("../../tests/fixtures/content-views/schema40-audio-4c66f735.json")
                .as_slice(),
            include_bytes!("../../tests/fixtures/content-views/schema40-midi-4c66f735.json")
                .as_slice(),
            include_bytes!(
                "../../tests/fixtures/content-views/schema40-session-audio-4c66f735.json"
            )
            .as_slice(),
        ] {
            let original = crate::decode(bytes).unwrap();
            assert!(
                original
                    .tracks()
                    .iter()
                    .flat_map(|t| t.midi_clips())
                    .all(|c| c.content_view().is_none())
            );
            let expected = original.snapshot();
            assert_eq!(
                crate::decode(&crate::encode(&original).unwrap())
                    .unwrap()
                    .snapshot(),
                expected
            );
            let ids = original
                .tracks()
                .iter()
                .flat_map(|t| {
                    t.clips()
                        .iter()
                        .map(|c| c.id())
                        .chain(t.midi_clips().iter().map(|c| c.id()))
                })
                .collect::<Vec<_>>();
            let mut editor = Editor::new(original, 32).unwrap();
            if !ids.is_empty() {
                editor.execute(Command::ReverseClips(ids.clone())).unwrap();
            }
            let reversed = editor.project().snapshot();
            let encoded = crate::encode(editor.project()).unwrap();
            let mut reopened = Editor::new(crate::decode(&encoded).unwrap(), 32).unwrap();
            assert_eq!(reopened.project().snapshot(), reversed);
            if let Some(clip) = reopened
                .project()
                .tracks()
                .iter()
                .flat_map(|t| t.midi_clips())
                .next()
            {
                let id = clip.id();
                let end = clip
                    .start()
                    .raw()
                    .checked_add(clip.length().raw())
                    .unwrap()
                    .checked_add(hex_project::BeatTime::ONE.raw())
                    .unwrap();
                let mut resized = Editor::new(reopened.project().clone(), 32).unwrap();
                resized
                    .execute(Command::ResizeMidiClipView {
                        clip: id,
                        operation: hex_project::MidiViewResizeOperation::RepeatOuterEnd {
                            beat: hex_project::BeatTime::from_raw(end),
                        },
                    })
                    .unwrap();
                assert_eq!(
                    crate::decode(&crate::encode(resized.project()).unwrap())
                        .unwrap()
                        .snapshot(),
                    resized.project().snapshot(),
                    "directed loop authoring retains complete saved state"
                );
            }
            assert!(
                super::super::clip_activation::encode(editor.project()).is_err() || ids.is_empty()
            );
            if !ids.is_empty() {
                reopened.execute(Command::ReverseClips(ids)).unwrap();
                assert_eq!(reopened.project().snapshot(), expected);
            }
        }
    }
    #[test]
    fn schema41_requires_complete_unique_typed_explicit_views_and_checked_geometry() {
        let project = crate::decode(include_bytes!(
            "../../tests/fixtures/content-views/schema40-audio-4c66f735.json"
        ))
        .unwrap();
        let identity: serde_json::Value =
            serde_json::from_slice(&crate::encode(&project).unwrap()).unwrap();
        let mut cases = Vec::new();
        let mut value = identity.clone();
        value.as_object_mut().unwrap().remove("content_views");
        cases.push(value);
        let mut value = identity.clone();
        value["content_views"].as_array_mut().unwrap().remove(0);
        cases.push(value);
        let mut value = identity.clone();
        value["content_views"][0]["view"]
            .as_object_mut()
            .unwrap()
            .remove("view");
        cases.push(value);
        let mut value = identity.clone();
        let row = value["content_views"][0].clone();
        value["content_views"].as_array_mut().unwrap().push(row);
        cases.push(value);
        let mut value = identity.clone();
        value["content_views"][0]["clip"] = u64::MAX.into();
        cases.push(value);
        for value in cases {
            assert!(crate::decode(&serde_json::to_vec(&value).unwrap()).is_err());
        }
        let id = project
            .tracks()
            .iter()
            .flat_map(|t| t.clips())
            .next()
            .unwrap()
            .id();
        let mut editor = Editor::new(project, 32).unwrap();
        editor.execute(Command::ReverseClips(vec![id])).unwrap();
        let authored: serde_json::Value =
            serde_json::from_slice(&crate::encode(editor.project()).unwrap()).unwrap();
        let row = authored["content_views"]
            .as_array()
            .unwrap()
            .iter()
            .position(|r| r["clip"].as_u64() == Some(id.value()))
            .unwrap();
        for (field, value) in [
            ("base_start_delta", serde_json::json!(i128::MAX.to_string())),
            (
                "window",
                serde_json::json!({"start":0,"end":1,"reversed":true}),
            ),
        ] {
            let mut invalid = authored.clone();
            invalid["content_views"][row]["view"]["view"][field] = value;
            assert!(crate::decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
    }
}
