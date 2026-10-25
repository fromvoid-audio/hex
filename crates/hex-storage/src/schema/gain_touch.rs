//! Strict schema22 gain touch spans, also embedded by the current schema24 writer.
use super::gain_envelopes::{GainPointDto, InterpolationDto, bounded_gain_points};
use super::provenance::ProvenanceDto;
use super::return_feeds::{DocumentV20, FeedDto};
use super::*;
use hex_project::{GainInterpolation, GainPoint, MAX_GAIN_POINTS, MAX_GAIN_POINTS_PER_TRACK};
use hex_project::{
    GainTouchContext, GainTouchSpan, GainTouchStep, MAX_GAIN_TOUCH_SPANS_PER_TRACK,
    MAX_GAIN_TOUCH_STEPS_PER_SPAN,
};
use serde::de::{SeqAccess, Visitor};
use std::fmt;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV22 {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetV2>,
    #[serde(deserialize_with = "bounded_tracks")]
    pub(super) tracks: Vec<TrackV22>,
    pub(super) return_tracks: Vec<ReturnTrackDtoV17>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    pub(super) loop_region: Option<LoopRegionDto>,
    pub(super) master_gain: f32,
    pub(super) tempo_points: Vec<TempoPointDto>,
    pub(super) meter_points: Vec<MeterPointDto>,
    pub(super) session: SessionDtoV12,
    #[serde(deserialize_with = "provenance::required_provenance")]
    pub(super) import_provenance: Option<ProvenanceDto>,
    #[serde(deserialize_with = "return_feeds::bounded_feeds")]
    pub(super) routing: Vec<FeedDto>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TrackV22 {
    #[serde(deserialize_with = "bounded_gain_points")]
    pub(super) gain_envelope: Vec<GainPointDto>,
    gain_touch_epoch: u64,
    #[serde(deserialize_with = "bounded_spans")]
    gain_touch_spans: Vec<SpanDto>,
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) gain: f32,
    pub(super) balance: f32,
    pub(super) muted: bool,
    pub(super) soloed: bool,
    pub(super) audio_clips: Vec<ClipV7>,
    pub(super) midi_clips: Vec<MidiClipDtoV10>,
    pub(super) devices: Vec<DeviceDtoV14>,
    #[serde(deserialize_with = "deserialize_required_instrument")]
    pub(super) instrument: Option<DeviceDtoV14>,
    #[serde(deserialize_with = "deserialize_required_instrument_position")]
    pub(super) instrument_position: Option<usize>,
    pub(super) sends: Vec<SendDtoV17>,
}
pub(super) fn bounded_tracks<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<TrackV22>, D::Error> {
    struct Tracks;
    impl<'de> Visitor<'de> for Tracks {
        type Value = Vec<TrackV22>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("bounded tracks and project gain points")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut tracks = Vec::new();
            let mut count = 0_usize;
            while let Some(track) = seq.next_element::<TrackV22>()? {
                if tracks.len() == hex_project::MAX_TRACKS {
                    return Err(serde::de::Error::custom("track limit reached"));
                }
                count = count
                    .checked_add(
                        track.gain_envelope.len()
                            + track
                                .gain_touch_spans
                                .iter()
                                .map(|span| span.steps.len() + 2)
                                .sum::<usize>(),
                    )
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
impl DocumentV22 {
    pub(crate) fn from_project(project: &Project) -> Result<Self, StorageError> {
        Self::fields_for_new_envelope(project)
    }
    pub(super) fn fields_for_new_envelope(project: &Project) -> Result<Self, StorageError> {
        let base = DocumentV20::fields_for_new_envelope(project)?;
        let tracks = base
            .tracks
            .into_iter()
            .zip(project.tracks())
            .map(|(track, owner)| TrackV22 {
                gain_touch_epoch: owner.gain_lane().recording_epoch(),
                gain_touch_spans: owner
                    .gain_lane()
                    .spans()
                    .iter()
                    .map(SpanDto::from_model)
                    .collect(),
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
            version: 22,
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
        Project::from_legacy_snapshot(doc.into_snapshot()?).map_err(|e| StorageError::Invalid(e.0))
    }
    pub(super) fn into_snapshot(self) -> Result<hex_project::Snapshot, StorageError> {
        let doc = self;
        if doc.format != "hex-project" || doc.version != 22 {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        let mut touch_metadata = Vec::new();
        let mut envelopes = Vec::new();
        let mut count = 0_usize;
        let mut tracks = Vec::new();
        for track in doc.tracks {
            count = count
                .checked_add(
                    track.gain_envelope.len()
                        + track
                            .gain_touch_spans
                            .iter()
                            .map(|span| span.steps.len() + 2)
                            .sum::<usize>(),
                )
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
            let spans = track
                .gain_touch_spans
                .into_iter()
                .map(SpanDto::into_model)
                .collect::<Vec<_>>();
            hex_project::validate_gain_lane_parts(&points, &spans, track.gain_touch_epoch)
                .map_err(|e| StorageError::Invalid(e.0))?;
            touch_metadata.push((spans, track.gain_touch_epoch));
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
        for ((track, points), (spans, epoch)) in snapshot
            .tracks
            .iter_mut()
            .zip(envelopes)
            .zip(touch_metadata)
        {
            track.gain_envelope = points;
            track.gain_touch_spans = spans;
            track.gain_touch_epoch = epoch;
        }
        snapshot
            .validate_gain_lanes()
            .map_err(|e| StorageError::Invalid(e.0))?;
        Ok(snapshot)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextDto {
    recording_epoch: u64,
    project_generation: u64,
    canonical_revision: u64,
    audio_revision: u64,
    transport_epoch: u64,
    gesture_epoch: u64,
    gesture_serial: u64,
    sample_rate: u32,
    loop_pass: u64,
}
impl ContextDto {
    fn from_model(c: GainTouchContext) -> Self {
        Self {
            recording_epoch: c.recording_epoch,
            project_generation: c.project_generation,
            canonical_revision: c.canonical_revision,
            audio_revision: c.audio_revision,
            transport_epoch: c.transport_epoch,
            gesture_epoch: c.gesture_epoch,
            gesture_serial: c.gesture_serial,
            sample_rate: c.sample_rate,
            loop_pass: c.loop_pass,
        }
    }
    fn into_model(self) -> GainTouchContext {
        GainTouchContext {
            recording_epoch: self.recording_epoch,
            project_generation: self.project_generation,
            canonical_revision: self.canonical_revision,
            audio_revision: self.audio_revision,
            transport_epoch: self.transport_epoch,
            gesture_epoch: self.gesture_epoch,
            gesture_serial: self.gesture_serial,
            sample_rate: self.sample_rate,
            loop_pass: self.loop_pass,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StepDto {
    beat_raw: u64,
    gain: f32,
    origin_beat_raw: u64,
    source_sequence: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpanDto {
    source_start_raw: u64,
    start_raw: u64,
    end_raw: u64,
    context: ContextDto,
    #[serde(deserialize_with = "bounded_steps")]
    steps: Vec<StepDto>,
}
impl SpanDto {
    fn from_model(s: &GainTouchSpan) -> Self {
        Self {
            source_start_raw: s.source_start.raw(),
            start_raw: s.start.raw(),
            end_raw: s.end.raw(),
            context: ContextDto::from_model(s.context),
            steps: s
                .steps
                .iter()
                .map(|e| StepDto {
                    beat_raw: e.beat.raw(),
                    gain: e.gain,
                    origin_beat_raw: e.origin_beat.raw(),
                    source_sequence: e.source_sequence,
                })
                .collect(),
        }
    }
    fn into_model(self) -> GainTouchSpan {
        GainTouchSpan {
            source_start: BeatTime::from_raw(self.source_start_raw),
            start: BeatTime::from_raw(self.start_raw),
            end: BeatTime::from_raw(self.end_raw),
            context: self.context.into_model(),
            steps: self
                .steps
                .into_iter()
                .map(|e| GainTouchStep {
                    beat: BeatTime::from_raw(e.beat_raw),
                    gain: e.gain,
                    origin_beat: BeatTime::from_raw(e.origin_beat_raw),
                    source_sequence: e.source_sequence,
                })
                .collect(),
        }
    }
}
fn bounded_steps<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<StepDto>, D::Error> {
    struct Steps;
    impl<'de> Visitor<'de> for Steps {
        type Value = Vec<StepDto>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("bounded touch steps")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut steps = Vec::new();
            while let Some(step) = seq.next_element()? {
                if steps.len() == MAX_GAIN_TOUCH_STEPS_PER_SPAN {
                    return Err(serde::de::Error::custom("gain touch step limit reached"));
                }
                steps.push(step);
            }
            Ok(steps)
        }
    }
    d.deserialize_seq(Steps)
}
fn bounded_spans<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<SpanDto>, D::Error> {
    struct Spans;
    impl<'de> Visitor<'de> for Spans {
        type Value = Vec<SpanDto>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("bounded touch spans and endpoints")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut spans = Vec::new();
            let mut count = 0_usize;
            while let Some(span) = seq.next_element::<SpanDto>()? {
                if spans.len() == MAX_GAIN_TOUCH_SPANS_PER_TRACK {
                    return Err(serde::de::Error::custom("gain touch span limit reached"));
                }
                count = count
                    .checked_add(span.steps.len() + 2)
                    .ok_or_else(|| serde::de::Error::custom("gain lane count overflow"))?;
                if count > MAX_GAIN_POINTS_PER_TRACK {
                    return Err(serde::de::Error::custom("gain lane limit reached"));
                }
                spans.push(span);
            }
            Ok(spans)
        }
    }
    d.deserialize_seq(Spans)
}
#[cfg(test)]
mod tests;
