//! Schema26 preserves independent Arrangement clip names; old files get clip labels.
use super::musical_audio::AudioState;
use super::musical_audio::DocumentV25;
use super::playable_routing::{AudioDto, MidiDto, bounded_audio, bounded_midi};
use super::*;
use serde::de::{SeqAccess, Visitor};
use std::{collections::BTreeMap, fmt};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV26 {
    #[serde(deserialize_with = "bounded_names")]
    pub(super) clip_names: Vec<ClipName>,
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetV2>,
    #[serde(deserialize_with = "gain_touch::bounded_tracks")]
    pub(super) tracks: Vec<gain_touch::TrackV22>,
    pub(super) return_tracks: Vec<ReturnTrackDtoV17>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    pub(super) loop_region: Option<LoopRegionDto>,
    pub(super) master_gain: f32,
    pub(super) tempo_points: Vec<TempoPointDto>,
    pub(super) meter_points: Vec<MeterPointDto>,
    pub(super) session: SessionDtoV12,
    #[serde(deserialize_with = "provenance::required_provenance")]
    pub(super) import_provenance: Option<provenance::ProvenanceDto>,
    #[serde(deserialize_with = "return_feeds::bounded_feeds")]
    pub(super) routing: Vec<return_feeds::FeedDto>,
    #[serde(deserialize_with = "bounded_audio")]
    pub(super) audio_feeds: Vec<AudioDto>,
    #[serde(deserialize_with = "bounded_midi")]
    pub(super) midi_routes: Vec<MidiDto>,
    #[serde(deserialize_with = "super::musical_audio::bounded_states")]
    pub(super) audio_clip_state: Vec<AudioState>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ClipName {
    pub(super) clip: u64,
    pub(super) name: String,
}
pub(super) fn bounded_names<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<ClipName>, D::Error> {
    struct Names;
    impl<'de> Visitor<'de> for Names {
        type Value = Vec<ClipName>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("bounded arrangement clip names")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut names = Vec::new();
            while let Some(name) = seq.next_element()? {
                if names.len() == hex_project::MAX_CLIPS {
                    return Err(serde::de::Error::custom("clip name limit reached"));
                }
                names.push(name);
            }
            Ok(names)
        }
    }
    d.deserialize_seq(Names)
}
impl DocumentV26 {
    pub(crate) fn from_project(project: &Project) -> Result<Self, StorageError> {
        let base = DocumentV25::from_project(project)?;
        let clip_names = project
            .tracks()
            .iter()
            .flat_map(|t| {
                t.clips()
                    .iter()
                    .map(|c| ClipName {
                        clip: c.id().value(),
                        name: c.name().into(),
                    })
                    .chain(t.midi_clips().iter().map(|c| ClipName {
                        clip: c.id().value(),
                        name: c.name().into(),
                    }))
            })
            .collect();
        Ok(Self {
            clip_names,
            format: base.format,
            version: 26,
            sample_rate: base.sample_rate,
            next_id: base.next_id,
            assets: base.assets,
            tracks: base.tracks,
            return_tracks: base.return_tracks,
            loop_region: base.loop_region,
            master_gain: base.master_gain,
            tempo_points: base.tempo_points,
            meter_points: base.meter_points,
            session: base.session,
            import_provenance: base.import_provenance,
            routing: base.routing,
            audio_feeds: base.audio_feeds,
            midi_routes: base.midi_routes,
            audio_clip_state: base.audio_clip_state,
        })
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
        Self::into_project(serde_json::from_slice(bytes)?)
    }
    /// Decode the envelope an upper migration layer already parsed and adapted.
    pub(super) fn decode_value(value: serde_json::Value) -> Result<Project, StorageError> {
        Self::into_project(serde_json::from_value(value)?)
    }
    pub(super) fn into_project(doc: Self) -> Result<Project, StorageError> {
        if doc.format != "hex-project" || doc.version != 26 {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        let base = DocumentV25 {
            format: doc.format,
            version: 25,
            sample_rate: doc.sample_rate,
            next_id: doc.next_id,
            assets: doc.assets,
            tracks: doc.tracks,
            return_tracks: doc.return_tracks,
            loop_region: doc.loop_region,
            master_gain: doc.master_gain,
            tempo_points: doc.tempo_points,
            meter_points: doc.meter_points,
            session: doc.session,
            import_provenance: doc.import_provenance,
            routing: doc.routing,
            audio_feeds: doc.audio_feeds,
            midi_routes: doc.midi_routes,
            audio_clip_state: doc.audio_clip_state,
        };
        let mut snapshot = base.into_project()?.snapshot();
        let mut names = BTreeMap::new();
        for entry in doc.clip_names {
            if names.insert(entry.clip, entry.name).is_some() {
                return Err(StorageError::Invalid("duplicate clip name"));
            }
        }
        for track in &mut snapshot.tracks {
            let TrackContentSnapshot::Hybrid {
                audio_clips,
                midi_clips,
            } = &mut track.content
            else {
                continue;
            };
            for clip in audio_clips {
                clip.name = names
                    .remove(&clip.id)
                    .ok_or(StorageError::Invalid("clip name is missing"))?;
            }
            for clip in midi_clips {
                clip.name = names
                    .remove(&clip.id)
                    .ok_or(StorageError::Invalid("clip name is missing"))?;
            }
        }
        if !names.is_empty() {
            return Err(StorageError::Invalid(
                "clip name references an unknown arrangement clip",
            ));
        }
        Project::from_legacy_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
    }
}
