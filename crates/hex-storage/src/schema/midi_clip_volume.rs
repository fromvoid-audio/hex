//! Required current-format MIDI clip-volume inventory. Private older-shaped
//! DTOs initialize unity only as staging; every MIDI owner must have a row here
//! before the public current decoder can return a project.
use super::StorageError;
use hex_project::{MidiClipGain, MidiClipVolume, MidiClipVolumePoint};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    #[serde(deserialize_with = "bounded_clips")]
    clips: Vec<Row>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    clip: u64,
    #[serde(deserialize_with = "bounded_points")]
    points: Vec<Point>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Point {
    beat_raw: u64,
    incoming: f64,
    value: f64,
}
fn bounded_points<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Point>, D::Error> {
    struct Points;
    impl<'de> serde::de::Visitor<'de> for Points {
        type Value = Vec<Point>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded clip volume points")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut points = Vec::new();
            while let Some(point) = seq.next_element()? {
                if points.len() == hex_project::MAX_MIDI_VOLUME_POINTS_PER_CLIP {
                    return Err(serde::de::Error::custom("clip volume point limit reached"));
                }
                points.push(point);
            }
            Ok(points)
        }
    }
    d.deserialize_seq(Points)
}
fn bounded_clips<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Row>, D::Error> {
    struct Clips;
    impl<'de> serde::de::Visitor<'de> for Clips {
        type Value = Vec<Row>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded complete midi clip volume inventory")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut clips = Vec::new();
            let mut points = 0_usize;
            while let Some(row) = seq.next_element::<Row>()? {
                if clips.len() == hex_project::MAX_CLIPS {
                    return Err(serde::de::Error::custom(
                        "clip volume inventory limit reached",
                    ));
                }
                points = points
                    .checked_add(row.points.len())
                    .filter(|n| *n <= hex_project::MAX_MIDI_VOLUME_POINTS)
                    .ok_or_else(|| {
                        serde::de::Error::custom("project clip volume point limit reached")
                    })?;
                clips.push(row);
            }
            Ok(clips)
        }
    }
    d.deserialize_seq(Clips)
}
impl Row {
    fn from_volume(clip: u64, volume: &MidiClipVolume) -> Self {
        Self {
            clip,
            points: volume
                .points()
                .iter()
                .map(|point| Point {
                    beat_raw: point.beat.raw(),
                    incoming: point.incoming.linear(),
                    value: point.value.linear(),
                })
                .collect(),
        }
    }
    fn into_volume(self, length: u64) -> Result<MidiClipVolume, StorageError> {
        let points = self
            .points
            .into_iter()
            .map(|point| {
                Ok(MidiClipVolumePoint {
                    beat: hex_project::BeatTime::from_raw(point.beat_raw),
                    incoming: MidiClipGain::new(point.incoming)
                        .map_err(|e| StorageError::Invalid(e.0))?,
                    value: MidiClipGain::new(point.value)
                        .map_err(|e| StorageError::Invalid(e.0))?,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        MidiClipVolume::new(points, hex_project::BeatTime::from_raw(length))
            .map_err(|e| StorageError::Invalid(e.0))
    }
}
impl Manifest {
    pub(super) fn from_project(project: &hex_project::Project) -> Result<Self, StorageError> {
        let mut clips = Vec::new();
        let arrangement = project
            .tracks()
            .iter()
            .flat_map(|track| track.midi_clips())
            .map(|clip| (clip.id(), clip.volume()));
        let console = project
            .session_cells()
            .iter()
            .filter_map(|cell| match cell.content() {
                hex_project::SessionCellContent::Midi(clip) => Some((clip.id(), clip.volume())),
                _ => None,
            });
        let mut count = 0_usize;
        for (id, volume) in arrangement.chain(console) {
            if volume.clip_gain() != hex_project::ClipGain::UNITY {
                return Err(StorageError::Invalid(
                    "older schema cannot save midi clip gain",
                ));
            }
            count = count
                .checked_add(volume.points().len())
                .filter(|n| *n <= hex_project::MAX_MIDI_VOLUME_POINTS)
                .ok_or(StorageError::Invalid(
                    "project clip volume point limit reached",
                ))?;
            clips.push(Row::from_volume(id.value(), volume));
        }
        Ok(Self { clips })
    }
    pub(super) fn apply(self, snapshot: &mut hex_project::Snapshot) -> Result<(), StorageError> {
        let mut rows = BTreeMap::new();
        for row in self.clips {
            if row.clip == 0 || rows.insert(row.clip, row).is_some() {
                return Err(StorageError::Invalid(
                    "clip volume inventory has duplicate or zero ids",
                ));
            }
        }
        for track in &mut snapshot.tracks {
            let clips = match &mut track.content {
                hex_project::TrackContentSnapshot::Audio(_) => continue,
                hex_project::TrackContentSnapshot::Instrument(clips) => clips,
                hex_project::TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
            };
            for clip in clips {
                clip.volume = rows
                    .remove(&clip.id)
                    .ok_or(StorageError::Invalid("midi clip volume row is missing"))?
                    .into_volume(clip.source_length_raw)?;
            }
        }
        for cell in &mut snapshot.session.cells {
            if let hex_project::SessionCellContentSnapshot::Midi(clip) = &mut cell.content {
                clip.volume = rows
                    .remove(&clip.id)
                    .ok_or(StorageError::Invalid("midi clip volume row is missing"))?
                    .into_volume(clip.source.source_length_raw)?;
            }
        }
        if !rows.is_empty() {
            return Err(StorageError::Invalid(
                "clip volume inventory contains unknown or non-midi ids",
            ));
        }
        Ok(())
    }
}
