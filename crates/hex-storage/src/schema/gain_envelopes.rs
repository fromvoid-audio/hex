//! Strict schema21: canonical track gain lanes plus the complete schema20 state.
use super::provenance::ProvenanceDto;
use super::return_feeds::{DocumentV20, FeedDto};
use super::*;
use hex_project::{GainInterpolation, GainPoint, MAX_GAIN_POINTS, MAX_GAIN_POINTS_PER_TRACK};
use serde::de::{SeqAccess, Visitor};
use std::fmt;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV21 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    #[serde(deserialize_with = "bounded_tracks")]
    tracks: Vec<TrackV21>,
    return_tracks: Vec<ReturnTrackDtoV17>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    loop_region: Option<LoopRegionDto>,
    master_gain: f32,
    tempo_points: Vec<TempoPointDto>,
    meter_points: Vec<MeterPointDto>,
    session: SessionDtoV12,
    #[serde(deserialize_with = "provenance::required_provenance")]
    import_provenance: Option<ProvenanceDto>,
    #[serde(deserialize_with = "return_feeds::bounded_feeds")]
    routing: Vec<FeedDto>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackV21 {
    #[serde(deserialize_with = "bounded_gain_points")]
    gain_envelope: Vec<GainPointDto>,
    id: u64,
    name: String,
    gain: f32,
    balance: f32,
    muted: bool,
    soloed: bool,
    audio_clips: Vec<ClipV7>,
    midi_clips: Vec<MidiClipDtoV10>,
    devices: Vec<DeviceDtoV14>,
    #[serde(deserialize_with = "deserialize_required_instrument")]
    instrument: Option<DeviceDtoV14>,
    #[serde(deserialize_with = "deserialize_required_instrument_position")]
    instrument_position: Option<usize>,
    sends: Vec<SendDtoV17>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GainPointDto {
    #[serde(deserialize_with = "required_incoming_gain")]
    pub(super) incoming_gain: Option<f32>,
    pub(super) beat_raw: u64,
    pub(super) gain: f32,
    pub(super) outgoing: InterpolationDto,
}
fn required_incoming_gain<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<f32>, D::Error> {
    Option::deserialize(d)
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum InterpolationDto {
    Step,
    Linear,
}
fn bounded_tracks<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<TrackV21>, D::Error> {
    struct Tracks;
    impl<'de> Visitor<'de> for Tracks {
        type Value = Vec<TrackV21>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("bounded tracks and project gain points")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut tracks = Vec::new();
            let mut count = 0_usize;
            while let Some(track) = seq.next_element::<TrackV21>()? {
                if tracks.len() == hex_project::MAX_TRACKS {
                    return Err(serde::de::Error::custom("track limit reached"));
                }
                count = count
                    .checked_add(track.gain_envelope.len())
                    .ok_or_else(|| serde::de::Error::custom("gain point count overflow"))?;
                if count > MAX_GAIN_POINTS {
                    return Err(serde::de::Error::custom(
                        "project gain envelope point limit reached",
                    ));
                }
                tracks.push(track);
            }
            Ok(tracks)
        }
    }
    d.deserialize_seq(Tracks)
}
pub(super) fn bounded_gain_points<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<GainPointDto>, D::Error> {
    struct Points;
    impl<'de> Visitor<'de> for Points {
        type Value = Vec<GainPointDto>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("at most1024 ordered gain points")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut points = Vec::new();
            while let Some(point) = seq.next_element()? {
                if points.len() == MAX_GAIN_POINTS_PER_TRACK {
                    return Err(serde::de::Error::custom(
                        "track gain envelope point limit reached",
                    ));
                }
                points.push(point);
            }
            Ok(points)
        }
    }
    d.deserialize_seq(Points)
}
impl DocumentV21 {
    #[cfg(test)]
    pub(crate) fn from_project(project: &Project) -> Result<Self, StorageError> {
        if project.has_gain_touch_metadata() {
            return Err(StorageError::Invalid(
                "older schema cannot save gain touch metadata",
            ));
        }
        let base = DocumentV20::fields_for_new_envelope(project)?;
        let tracks = base
            .tracks
            .into_iter()
            .zip(project.tracks())
            .map(|(track, owner)| TrackV21 {
                id: track.id,
                name: track.name,
                gain: track.gain,
                balance: track.balance,
                muted: track.muted,
                soloed: track.soloed,
                audio_clips: track.audio_clips,
                midi_clips: track.midi_clips,
                devices: track.devices,
                instrument: track.instrument,
                instrument_position: track.instrument_position,
                sends: track.sends,
                gain_envelope: owner
                    .gain_envelope()
                    .iter()
                    .map(|point| GainPointDto {
                        incoming_gain: point.incoming_gain,
                        beat_raw: point.beat.raw(),
                        gain: point.gain,
                        outgoing: match point.outgoing {
                            GainInterpolation::Step => InterpolationDto::Step,
                            GainInterpolation::Linear => InterpolationDto::Linear,
                        },
                    })
                    .collect(),
            })
            .collect();
        Ok(Self {
            version: 21,
            tracks,
            format: base.format,
            sample_rate: base.sample_rate,
            next_id: base.next_id,
            assets: base.assets,
            return_tracks: base.return_tracks,
            loop_region: base.loop_region,
            master_gain: base.master_gain,
            tempo_points: base.tempo_points,
            meter_points: base.meter_points,
            session: base.session,
            import_provenance: base.import_provenance,
            routing: base.routing,
        })
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
        let doc: Self = serde_json::from_slice(bytes)?;
        if doc.format != "hex-project" || doc.version != 21 {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        let mut envelopes = Vec::new();
        let mut count = 0_usize;
        let mut tracks = Vec::new();
        for track in doc.tracks {
            count = count
                .checked_add(track.gain_envelope.len())
                .ok_or(StorageError::Invalid("gain point count overflow"))?;
            if count > MAX_GAIN_POINTS {
                return Err(StorageError::Invalid(
                    "project gain envelope point limit reached",
                ));
            }
            let points = track
                .gain_envelope
                .into_iter()
                .map(|point| GainPoint {
                    incoming_gain: point.incoming_gain,
                    beat: BeatTime::from_raw(point.beat_raw),
                    gain: point.gain,
                    outgoing: match point.outgoing {
                        InterpolationDto::Step => GainInterpolation::Step,
                        InterpolationDto::Linear => GainInterpolation::Linear,
                    },
                })
                .collect::<Vec<_>>();
            hex_project::validate_gain_envelope(&points).map_err(|e| StorageError::Invalid(e.0))?;
            envelopes.push(points);
            tracks.push(TrackV17 {
                id: track.id,
                name: track.name,
                gain: track.gain,
                balance: track.balance,
                muted: track.muted,
                soloed: track.soloed,
                audio_clips: track.audio_clips,
                midi_clips: track.midi_clips,
                devices: track.devices,
                instrument: track.instrument,
                instrument_position: track.instrument_position,
                sends: track.sends,
            });
        }
        let base = DocumentV20 {
            version: 20,
            tracks,
            format: doc.format,
            sample_rate: doc.sample_rate,
            next_id: doc.next_id,
            assets: doc.assets,
            return_tracks: doc.return_tracks,
            loop_region: doc.loop_region,
            master_gain: doc.master_gain,
            tempo_points: doc.tempo_points,
            meter_points: doc.meter_points,
            session: doc.session,
            import_provenance: doc.import_provenance,
            routing: doc.routing,
        };
        let mut snapshot = base.into_snapshot()?;
        for (track, points) in snapshot.tracks.iter_mut().zip(envelopes) {
            track.gain_envelope = points;
        }
        Project::from_legacy_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
    }
}
