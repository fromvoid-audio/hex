//! Schema27: inherited track colors and per-clip flavors, with strict legacy defaults.
use super::musical_audio::{AudioState, bounded_states};
use super::playable_routing::{AudioDto, MidiDto, bounded_audio, bounded_midi};
use super::*;
use hex_project::{ClipFlavor, ElementColor};
use serde::de::{SeqAccess, Visitor};
use std::{collections::BTreeSet, fmt};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV27 {
    #[serde(deserialize_with = "super::clip_names::bounded_names")]
    clip_names: Vec<super::clip_names::ClipName>,
    format: String,
    pub(super) version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    #[serde(deserialize_with = "gain_touch::bounded_tracks")]
    tracks: Vec<gain_touch::TrackV22>,
    return_tracks: Vec<ReturnTrackDtoV17>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    loop_region: Option<LoopRegionDto>,
    master_gain: f32,
    tempo_points: Vec<TempoPointDto>,
    meter_points: Vec<MeterPointDto>,
    session: SessionDtoV12,
    #[serde(deserialize_with = "provenance::required_provenance")]
    import_provenance: Option<provenance::ProvenanceDto>,
    #[serde(deserialize_with = "return_feeds::bounded_feeds")]
    routing: Vec<return_feeds::FeedDto>,
    #[serde(deserialize_with = "bounded_audio")]
    audio_feeds: Vec<AudioDto>,
    #[serde(deserialize_with = "bounded_midi")]
    midi_routes: Vec<MidiDto>,
    #[serde(deserialize_with = "bounded_states")]
    audio_clip_state: Vec<AudioState>,
    #[serde(deserialize_with = "bounded_track_colors")]
    track_colors: Vec<TrackColor>,
    #[serde(deserialize_with = "bounded_accents")]
    accented_clips: Vec<u64>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackColor {
    track: u64,
    color: String,
}
fn bounded<'de, D, T, const MAX: usize>(d: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Items<T, const MAX: usize>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const MAX: usize> Visitor<'de> for Items<T, MAX> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("bounded appearance metadata")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut result = Vec::new();
            while let Some(item) = seq.next_element()? {
                if result.len() == MAX {
                    return Err(serde::de::Error::custom(
                        "appearance metadata limit reached",
                    ));
                }
                result.push(item);
            }
            Ok(result)
        }
    }
    d.deserialize_seq(Items::<T, MAX>(std::marker::PhantomData))
}
fn bounded_track_colors<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<TrackColor>, D::Error> {
    bounded::<D, TrackColor, { hex_project::MAX_TRACKS }>(d)
}
fn bounded_accents<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u64>, D::Error> {
    bounded::<D, u64, { hex_project::MAX_CLIPS + hex_project::MAX_SESSION_CELLS }>(d)
}
impl DocumentV27 {
    pub(crate) fn from_project(project: &Project) -> Result<Self, StorageError> {
        let base = clip_names::DocumentV26::from_project(project)?;
        let mut accented_clips = Vec::new();
        for track in project.tracks() {
            accented_clips.extend(
                track
                    .clips()
                    .iter()
                    .filter(|c| c.flavor() == ClipFlavor::Accent)
                    .map(|c| c.id().value()),
            );
            accented_clips.extend(
                track
                    .midi_clips()
                    .iter()
                    .filter(|c| c.flavor() == ClipFlavor::Accent)
                    .map(|c| c.id().value()),
            );
        }
        accented_clips.extend(project.session_cells().iter().filter_map(
            |cell| match cell.content() {
                hex_project::SessionCellContent::Audio(c) if c.flavor() == ClipFlavor::Accent => {
                    Some(c.id().value())
                }
                hex_project::SessionCellContent::Midi(c) if c.flavor() == ClipFlavor::Accent => {
                    Some(c.id().value())
                }
                _ => None,
            },
        ));
        let track_colors = project
            .tracks()
            .iter()
            .map(|track| TrackColor {
                track: track.id().value(),
                color: track.color().key().into(),
            })
            .collect();
        Ok(Self {
            clip_names: base.clip_names,
            format: base.format,
            version: 27,
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
            track_colors,
            accented_clips,
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
        if doc.format != "hex-project" || doc.version != 27 {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        let base = clip_names::DocumentV26 {
            clip_names: doc.clip_names,
            format: doc.format,
            version: 26,
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
        let mut snapshot = clip_names::DocumentV26::into_project(base)?.snapshot();
        let mut colors = std::collections::BTreeMap::new();
        for item in doc.track_colors {
            let color =
                ElementColor::from_key(&item.color).map_err(|e| StorageError::Invalid(e.0))?;
            if colors.insert(item.track, color).is_some() {
                return Err(StorageError::Invalid("duplicate track color"));
            }
        }
        let mut accents = BTreeSet::new();
        for id in doc.accented_clips {
            if !accents.insert(id) {
                return Err(StorageError::Invalid("duplicate accented clip"));
            }
        }
        let flavor = |id, accents: &mut BTreeSet<u64>| {
            if accents.remove(&id) {
                ClipFlavor::Accent
            } else {
                ClipFlavor::Muted
            }
        };
        for track in &mut snapshot.tracks {
            track.color = colors
                .remove(&track.id)
                .ok_or(StorageError::Invalid("track color is missing"))?;
            if let TrackContentSnapshot::Hybrid {
                audio_clips,
                midi_clips,
            } = &mut track.content
            {
                for clip in audio_clips {
                    clip.flavor = flavor(clip.id, &mut accents);
                }
                for clip in midi_clips {
                    clip.flavor = flavor(clip.id, &mut accents);
                }
            }
        }
        for cell in &mut snapshot.session.cells {
            match &mut cell.content {
                SessionCellContentSnapshot::Audio(c) => c.flavor = flavor(c.id, &mut accents),
                SessionCellContentSnapshot::Midi(c) => c.flavor = flavor(c.id, &mut accents),
                _ => {}
            }
        }
        if !colors.is_empty() || !accents.is_empty() {
            return Err(StorageError::Invalid(
                "appearance references missing track or clip",
            ));
        }
        Project::from_legacy_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
    }
}
pub(crate) fn refuse_legacy(project: &Project) -> Result<(), StorageError> {
    if project.tracks().iter().any(|t| {
        t.color() != ElementColor::default()
            || t.clips().iter().any(|c| c.flavor() == ClipFlavor::Accent)
            || t.midi_clips()
                .iter()
                .any(|c| c.flavor() == ClipFlavor::Accent)
    }) || project
        .session_cells()
        .iter()
        .any(|cell| match cell.content() {
            hex_project::SessionCellContent::Audio(c) => c.flavor() == ClipFlavor::Accent,
            hex_project::SessionCellContent::Midi(c) => c.flavor() == ClipFlavor::Accent,
            _ => false,
        })
    {
        return Err(StorageError::Invalid(
            "older schema cannot save element colors or clip flavors",
        ));
    }
    Ok(())
}
