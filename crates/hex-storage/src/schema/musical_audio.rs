//! Strict schema25: the complete routed envelope and bijective musical audio state.
use super::playable_routing::{AudioDto, MidiDto, bounded_audio, bounded_midi};
use super::*;
use serde::de::{SeqAccess, Visitor};
use std::collections::BTreeMap;
use std::fmt;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV25 {
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
    #[serde(deserialize_with = "bounded_states")]
    pub(super) audio_clip_state: Vec<AudioState>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AudioState {
    pub(super) clip: u64,
    pub(super) gain_millidb: i32,
    #[serde(deserialize_with = "required_edit")]
    pub(super) audio_edit: Option<crate::audio_edit::AudioEditDto>,
}
fn required_edit<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<crate::audio_edit::AudioEditDto>, D::Error> {
    Option::deserialize(d)
}
pub(super) fn bounded_states<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<AudioState>, D::Error> {
    struct States;
    impl<'de> Visitor<'de> for States {
        type Value = Vec<AudioState>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("bounded complete audio clip state")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut states = Vec::new();
            while let Some(state) = seq.next_element()? {
                if states.len() == hex_project::MAX_CLIPS + hex_project::MAX_SESSION_CELLS {
                    return Err(serde::de::Error::custom("audio clip state limit reached"));
                }
                states.push(state);
            }
            Ok(states)
        }
    }
    d.deserialize_seq(States)
}
/// Exact audio-ID bijection prevents silently missing, duplicated or misbound state.
pub(super) fn overlay(
    mut snapshot: Snapshot,
    states: Vec<AudioState>,
) -> Result<Project, StorageError> {
    let mut by_id = BTreeMap::new();
    for state in states {
        if by_id.insert(state.clip, state).is_some() {
            return Err(StorageError::Invalid("duplicate audio clip state"));
        }
    }
    for track in &mut snapshot.tracks {
        let TrackContentSnapshot::Hybrid { audio_clips, .. } = &mut track.content else {
            continue;
        };
        for clip in audio_clips {
            let state = by_id
                .remove(&clip.id)
                .ok_or(StorageError::Invalid("audio clip state is missing"))?;
            clip.gain = hex_project::ClipGain::new(state.gain_millidb)
                .map_err(|e| StorageError::Invalid(e.0))?;
            clip.edit = state.audio_edit.map(TryInto::try_into).transpose()?;
        }
    }
    for cell in &mut snapshot.session.cells {
        if let SessionCellContentSnapshot::Audio(clip) = &mut cell.content {
            let state = by_id
                .remove(&clip.id)
                .ok_or(StorageError::Invalid("session audio clip state is missing"))?;
            if state.audio_edit.is_some() {
                return Err(StorageError::Invalid(
                    "session audio cannot retain arrangement transforms",
                ));
            }
            clip.gain = hex_project::ClipGain::new(state.gain_millidb)
                .map_err(|e| StorageError::Invalid(e.0))?;
        }
    }
    if !by_id.is_empty() {
        return Err(StorageError::Invalid(
            "audio clip state references a non-audio clip",
        ));
    }
    Project::from_legacy_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}
pub(crate) fn refuse_legacy_audio(project: &Project) -> Result<(), StorageError> {
    if project.tracks().iter().flat_map(|t|t.clips()).any(|c|c.gain()!=hex_project::ClipGain::UNITY || c.audio_edit().is_some() || c.fade().has_shape())
        || project.session_cells().iter().any(|c|matches!(c.content(),hex_project::SessionCellContent::Audio(c) if c.gain()!=hex_project::ClipGain::UNITY)) {
        return Err(StorageError::Invalid("older schema cannot save clip gain or musical audio state"));
    }
    Ok(())
}
impl DocumentV25 {
    pub(crate) fn from_project(project: &Project) -> Result<Self, StorageError> {
        let base = playable_routing::DocumentV24::from_project(project)?;
        let snapshot = project.snapshot();
        let mut audio_clip_state = Vec::new();
        for track in snapshot.tracks {
            let TrackContentSnapshot::Hybrid { audio_clips, .. } = track.content else {
                continue;
            };
            audio_clip_state.extend(audio_clips.into_iter().map(|clip| AudioState {
                clip: clip.id,
                gain_millidb: clip.gain.millidb(),
                audio_edit: clip.edit.map(Into::into),
            }));
        }
        audio_clip_state.extend(snapshot.session.cells.into_iter().filter_map(|cell| {
            if let SessionCellContentSnapshot::Audio(clip) = cell.content {
                Some(AudioState {
                    clip: clip.id,
                    gain_millidb: clip.gain.millidb(),
                    audio_edit: None,
                })
            } else {
                None
            }
        }));
        Ok(Self {
            format: base.format,
            version: 25,
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
            audio_clip_state,
        })
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
        Self::decode_document(serde_json::from_slice(bytes)?)
    }
    /// Decode the envelope an upper migration layer already parsed and adapted.
    pub(super) fn decode_value(value: serde_json::Value) -> Result<Project, StorageError> {
        Self::decode_document(serde_json::from_value(value)?)
    }
    fn decode_document(doc: Self) -> Result<Project, StorageError> {
        if doc.format != "hex-project" || doc.version != 25 {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        doc.into_project()
    }
    pub(super) fn into_project(self) -> Result<Project, StorageError> {
        let doc = self;
        let base = playable_routing::DocumentV24 {
            format: doc.format,
            version: 24,
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
        };
        overlay(
            playable_routing::DocumentV24::into_snapshot(base)?,
            doc.audio_clip_state,
        )
    }
}
