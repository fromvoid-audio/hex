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
    pub(super) clip_names: Vec<super::clip_names::ClipName>,
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
    #[serde(deserialize_with = "bounded_track_colors")]
    pub(super) track_colors: Vec<TrackColor>,
    #[serde(deserialize_with = "bounded_accents")]
    pub(super) accented_clips: Vec<u64>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TrackColor {
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
pub(super) fn bounded_track_colors<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<TrackColor>, D::Error> {
    bounded::<D, TrackColor, { hex_project::MAX_TRACKS }>(d)
}
pub(super) fn bounded_accents<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<u64>, D::Error> {
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
        let doc: Self = serde_json::from_slice(bytes)?;
        if doc.format != "hex-project" || doc.version != 27 {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        doc.into_project()
    }
    pub(super) fn into_project(self) -> Result<Project, StorageError> {
        let doc = self;
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
        Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
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
