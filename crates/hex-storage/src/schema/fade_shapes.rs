//! Schema28 retains named/colored clips and adds bounded, exact audio fade-shape metadata.
use super::musical_audio::AudioState;
use super::playable_routing::{AudioDto, MidiDto, bounded_audio, bounded_midi};
use super::*;
use serde::de::{SeqAccess, Visitor};
use std::{collections::BTreeMap, fmt};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV28 {
    #[serde(deserialize_with = "bounded_shapes")]
    fade_shapes: Vec<FadeShape>,
    #[serde(deserialize_with = "super::appearance::bounded_track_colors")]
    track_colors: Vec<super::appearance::TrackColor>,
    #[serde(deserialize_with = "super::appearance::bounded_accents")]
    accented_clips: Vec<u64>,
    #[serde(deserialize_with = "super::clip_names::bounded_names")]
    clip_names: Vec<super::clip_names::ClipName>,
    format: String,
    version: u32,
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
    #[serde(deserialize_with = "super::musical_audio::bounded_states")]
    audio_clip_state: Vec<AudioState>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FadeShape {
    clip: u64,
    in_start: u64,
    out_end_offset: u64,
    in_curve: i16,
    out_curve: i16,
}
fn bounded_shapes<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<FadeShape>, D::Error> {
    struct Shapes;
    impl<'de> Visitor<'de> for Shapes {
        type Value = Vec<FadeShape>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("bounded arrangement fade shapes")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut shapes = Vec::new();
            while let Some(shape) = seq.next_element()? {
                if shapes.len() == hex_project::MAX_CLIPS {
                    return Err(serde::de::Error::custom("fade shape limit reached"));
                }
                shapes.push(shape);
            }
            Ok(shapes)
        }
    }
    d.deserialize_seq(Shapes)
}
fn apply_shapes(project: Project, shapes: Vec<FadeShape>) -> Result<Project, StorageError> {
    if shapes.len() > hex_project::MAX_CLIPS {
        return Err(StorageError::Invalid("fade shape limit reached"));
    }
    let mut shapes_by_id = BTreeMap::new();
    for shape in shapes {
        if shapes_by_id.insert(shape.clip, shape).is_some() {
            return Err(StorageError::Invalid("duplicate fade shape"));
        }
    }
    let mut snapshot = project.snapshot();
    for track in &mut snapshot.tracks {
        let TrackContentSnapshot::Hybrid { audio_clips, .. } = &mut track.content else {
            continue;
        };
        for clip in audio_clips {
            let shape = shapes_by_id
                .remove(&clip.id)
                .ok_or(StorageError::Invalid("fade shape is missing"))?;
            let target = if let Some(edit) = &mut clip.edit {
                &mut std::sync::Arc::make_mut(&mut edit.domain).fade
            } else {
                &mut clip.fade
            };
            *target = target
                .with_shape(
                    shape.in_start,
                    shape.out_end_offset,
                    shape.in_curve,
                    shape.out_curve,
                )
                .map_err(|e| StorageError::Invalid(e.0))?;
        }
    }
    if !shapes_by_id.is_empty() {
        return Err(StorageError::Invalid(
            "fade shape references a non-arrangement audio clip",
        ));
    }
    Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}
impl DocumentV28 {
    pub(crate) fn from_project(project: &Project) -> Result<Self, StorageError> {
        let base = super::appearance::DocumentV27::from_project(project)?;
        let fade_shapes = project
            .tracks()
            .iter()
            .flat_map(|track| track.clips())
            .map(|clip| {
                let fade = clip.retained_fade();
                FadeShape {
                    clip: clip.id().value(),
                    in_start: fade.in_start(),
                    out_end_offset: fade.out_end_offset(),
                    in_curve: fade.in_curve(),
                    out_curve: fade.out_curve(),
                }
            })
            .collect();
        Ok(Self {
            fade_shapes,
            track_colors: base.track_colors,
            accented_clips: base.accented_clips,
            clip_names: base.clip_names,
            format: base.format,
            version: 28,
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
        let doc: Self = serde_json::from_slice(bytes)?;
        if doc.format != "hex-project" || doc.version != 28 {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        let base = super::appearance::DocumentV27 {
            track_colors: doc.track_colors,
            accented_clips: doc.accented_clips,
            clip_names: doc.clip_names,
            format: doc.format,
            version: 27,
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
        apply_shapes(base.into_project()?, doc.fade_shapes)
    }
}
