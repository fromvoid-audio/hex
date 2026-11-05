//! Control-prepared absolute track gains; render queries have no mutable cursor.
use crate::{Invalid, PreparedTrackMix};
use hex_project::{GainInterpolation, GainLane, GainPoint, Project, TempoMap};

mod touch;
use touch::PreparedTouch;
pub(crate) use touch::TouchTarget;

pub const MAX_PREPARED_GAIN_KNOTS: usize = 327_680;
/// Combined prepared base-knot and touch-array allocation ceiling.
pub const MAX_PREPARED_GAIN_BYTES: usize =
    MAX_PREPARED_GAIN_KNOTS * std::mem::size_of::<GainEnvelopeKnot>();

/// One exact continuous part of the effective read-only curve. Consecutive
/// segments can differ at their shared frame, denoting a step discontinuity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GainPreviewSegment {
    pub start_frame: u64,
    pub end_frame: u64,
    pub start_gain: f32,
    pub end_gain: f32,
}
#[derive(Clone, Copy, Debug)]
pub struct GainEnvelopeKnot {
    frame: u64,
    gain: f64,
    incoming_gain: f64,
    outgoing: GainInterpolation,
}
impl GainEnvelopeKnot {
    pub fn frame(&self) -> u64 {
        self.frame
    }
    pub fn gain(&self) -> f64 {
        self.gain
    }
    pub fn incoming_gain(&self) -> f64 {
        self.incoming_gain
    }
    pub fn outgoing(&self) -> GainInterpolation {
        self.outgoing
    }
}
/// Control-side display cache, prepared by the same law as playback. Never an audio acknowledgment.
#[derive(Clone, Debug, Default)]
pub struct GainAutomationPreview {
    tracks: Vec<(hex_project::TrackId, PreparedGainEnvelope)>,
}
impl GainAutomationPreview {
    pub fn prepare(project: &Project) -> Result<Self, Invalid> {
        validate_project(project)?;
        let mut tracks = Vec::new();
        tracks
            .try_reserve_exact(project.tracks().len())
            .map_err(|_| Invalid("could not reserve gain preview"))?;
        for track in project.tracks() {
            if let Some(envelope) = PreparedGainEnvelope::prepare(
                track.gain_lane(),
                project.tempo_map(),
                PreparedTrackMix::new(1.0, 0.0, false)?,
                track.gain(),
            )? {
                tracks.push((track.id(), envelope));
            }
        }
        Ok(Self { tracks })
    }
    pub fn knots(&self, track: hex_project::TrackId) -> &[GainEnvelopeKnot] {
        self.tracks
            .iter()
            .find(|(id, _)| *id == track)
            .map(|(_, envelope)| envelope.knots.as_slice())
            .unwrap_or(&[])
    }
    /// Visit exact base and touch transition segments clipped to a frame range.
    /// Base knots remain separately available as authored-curve metadata.
    pub fn visit_segments(
        &self,
        track: hex_project::TrackId,
        start: u64,
        end: u64,
        mut visit: impl FnMut(GainPreviewSegment),
    ) -> bool {
        let Some((_, envelope)) = self.tracks.iter().find(|(id, _)| *id == track) else {
            return false;
        };
        envelope.visit_segments(start, end, &mut visit);
        true
    }
    pub fn value_at(&self, track: hex_project::TrackId, frame: u64) -> Option<f32> {
        self.tracks
            .iter()
            .find(|(id, _)| *id == track)
            .map(|(_, envelope)| envelope.value_at(frame))
    }
}
#[derive(Clone, Debug)]
pub(crate) struct PreparedGainEnvelope {
    knots: Vec<GainEnvelopeKnot>,
    mix: PreparedTrackMix,
    authored_points: usize,
    authored_units: usize,
    touch: PreparedTouch,
    fallback: f32,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GainAutomationCapacity {
    pub authored_points: usize,
    pub prepared_knots: usize,
    pub allocated_knot_slots: usize,
    pub allocated_knot_bytes: usize,
    pub authored_units: usize,
    pub prepared_touch_spans: usize,
    pub prepared_touch_steps: usize,
    pub allocated_touch_bytes: usize,
}

fn markers_between(
    tempo: &TempoMap,
    from: hex_project::BeatTime,
    to: hex_project::BeatTime,
) -> &[hex_project::TempoPoint] {
    let points = tempo.points();
    let start = points.partition_point(|marker| marker.beat() <= from);
    let end = points.partition_point(|marker| marker.beat() < to);
    &points[start..end]
}
fn knot_count(points: &[GainPoint], tempo: &TempoMap) -> Result<usize, Invalid> {
    hex_project::validate_gain_envelope(points).map_err(|e| Invalid(e.0))?;
    let mut count = points.len();
    for pair in points.windows(2) {
        if pair[0].outgoing == GainInterpolation::Linear {
            count = count
                .checked_add(markers_between(tempo, pair[0].beat, pair[1].beat).len())
                .ok_or(Invalid("prepared gain knot count overflow"))?;
        }
    }
    Ok(count)
}
fn plan_counts(lane: &GainLane, tempo: &TempoMap) -> Result<(usize, usize), Invalid> {
    lane.validate().map_err(|error| Invalid(error.0))?;
    let knots = knot_count(lane.base(), tempo)?;
    let units = knots
        .checked_add(lane.units().map_err(|e| Invalid(e.0))? - lane.base().len())
        .ok_or(Invalid("prepared gain plan count overflow"))?;
    let bytes = knots
        .checked_mul(std::mem::size_of::<GainEnvelopeKnot>())
        .and_then(|n| n.checked_add(PreparedTouch::preflight_bytes(lane)?))
        .ok_or(Invalid("prepared gain plan byte count overflow"))?;
    Ok((units, bytes))
}
pub(crate) fn validate_project(project: &Project) -> Result<(), Invalid> {
    let mut authored = 0_usize;
    let mut prepared = 0_usize;
    let mut bytes = 0_usize;
    for track in project.tracks() {
        authored = authored
            .checked_add(track.gain_lane().units().map_err(|e| Invalid(e.0))?)
            .ok_or(Invalid("gain point count overflow"))?;
        let (count, size) = plan_counts(track.gain_lane(), project.tempo_map())?;
        prepared = prepared
            .checked_add(count)
            .ok_or(Invalid("prepared gain knot count overflow"))?;
        bytes = bytes
            .checked_add(size)
            .ok_or(Invalid("prepared gain plan byte count overflow"))?;
    }
    if authored > hex_project::MAX_GAIN_POINTS
        || prepared > MAX_PREPARED_GAIN_KNOTS
        || bytes > MAX_PREPARED_GAIN_BYTES
    {
        return Err(Invalid("gain automation exceeds its bounded plan"));
    }
    Ok(())
}
pub(crate) mod persistent;
fn fill_knots(
    knots: &mut Vec<GainEnvelopeKnot>,
    points: &[GainPoint],
    tempo: &TempoMap,
    mut fault: impl FnMut(usize) -> Result<(), Invalid>,
) -> Result<(), Invalid> {
    let mut attempts = 0;
    let mut push = |beat: hex_project::BeatTime,
                    gain: f64,
                    incoming_gain: f64,
                    outgoing: GainInterpolation|
     -> Result<(), Invalid> {
        let frame = tempo.beat_to_frame(beat).map_err(|e| Invalid(e.0))?;
        let knot = GainEnvelopeKnot {
            frame,
            gain,
            incoming_gain,
            outgoing,
        };
        if knots
            .last()
            .is_some_and(|old: &GainEnvelopeKnot| old.frame == frame)
        {
            if let Some(last) = knots.last_mut() {
                *last = knot;
            }
        } else {
            knots.push(knot);
        }
        fault(attempts)?;
        attempts += 1;
        Ok(())
    };
    for (index, point) in points.iter().enumerate() {
        push(
            point.beat,
            f64::from(point.gain),
            f64::from(point.incoming_gain.unwrap_or(point.gain)),
            point.outgoing,
        )?;
        if point.outgoing == GainInterpolation::Linear
            && let Some(next) = points.get(index + 1)
        {
            for marker in markers_between(tempo, point.beat, next.beat) {
                let fraction = (marker.beat().raw() - point.beat.raw()) as f64
                    / (next.beat.raw() - point.beat.raw()) as f64;
                let gain = f64::from(point.gain)
                    + (f64::from(next.incoming_gain.unwrap_or(next.gain)) - f64::from(point.gain))
                        * fraction;
                push(marker.beat(), gain, gain, GainInterpolation::Linear)?;
            }
        }
    }
    Ok(())
}
impl PreparedGainEnvelope {
    pub(crate) fn constant(mix: PreparedTrackMix, fallback: f32) -> Self {
        Self {
            knots: Vec::new(),
            mix,
            authored_points: 0,
            authored_units: 0,
            touch: PreparedTouch::default(),
            fallback,
        }
    }
    pub(crate) fn mix_with_gain(&self, gain: f32) -> PreparedTrackMix {
        self.mix.with_absolute_gain(gain)
    }
    pub(crate) fn prepare(
        lane: &GainLane,
        tempo: &TempoMap,
        mix: PreparedTrackMix,
        fallback: f32,
    ) -> Result<Option<Self>, Invalid> {
        let (units, bytes) = plan_counts(lane, tempo)?;
        if !fallback.is_finite() || !(0.0..=2.0).contains(&fallback) {
            return Err(Invalid("gain fallback must be finite and within 0..2"));
        }
        let points = lane.base();
        if points.is_empty() && lane.spans().is_empty() {
            return Ok(None);
        }
        let count = knot_count(points, tempo)?;
        if units > MAX_PREPARED_GAIN_KNOTS || bytes > MAX_PREPARED_GAIN_BYTES {
            return Err(Invalid("prepared gain knot limit reached"));
        }
        let mut knots = Vec::new();
        knots
            .try_reserve_exact(count)
            .map_err(|_| Invalid("could not reserve gain automation knots"))?;
        if knots.capacity() > count {
            return Err(Invalid("prepared gain knot allocation exceeds its bound"));
        }
        fill_knots(&mut knots, points, tempo, |_| Ok(()))?;
        let touch =
            PreparedTouch::prepare(lane, tempo, |frame| base_gain_at(&knots, fallback, frame))?;
        Ok(Some(Self {
            knots,
            mix,
            authored_points: points.len(),
            authored_units: lane.units().map_err(|e| Invalid(e.0))?,
            touch,
            fallback,
        }))
    }
    pub(crate) fn checked_backing_bytes(&self) -> Result<usize, Invalid> {
        let Self {
            knots,
            mix: _,
            authored_points: _,
            authored_units: _,
            touch,
            fallback: _,
        } = self;
        knots
            .capacity()
            .checked_mul(std::mem::size_of::<GainEnvelopeKnot>())
            .and_then(|bytes| bytes.checked_add(touch.allocated_bytes()))
            .ok_or(Invalid("gain capacity overflow"))
    }
    pub(crate) fn knots(&self) -> &[GainEnvelopeKnot] {
        &self.knots
    }
    pub(crate) fn mix_at(&self, frame: u64) -> PreparedTrackMix {
        self.mix.with_absolute_gain(self.value_at(frame))
    }
    pub(crate) fn value_at(&self, frame: u64) -> f32 {
        self.touch
            .value_at(frame)
            .unwrap_or_else(|| self.base_value_at(frame))
    }
    pub(crate) fn touch_target_at(&self, frame: u64) -> Option<TouchTarget> {
        self.touch.target_at(frame)
    }
    pub(crate) fn base_value_at(&self, frame: u64) -> f32 {
        base_gain_at(&self.knots, self.fallback, frame)
    }
    fn visit_segments(&self, mut start: u64, end: u64, visit: &mut impl FnMut(GainPreviewSegment)) {
        while start < end {
            let next_knot = self
                .knots
                .get(self.knots.partition_point(|knot| knot.frame <= start))
                .map_or(end, |knot| knot.frame.min(end));
            let next = self
                .touch
                .next_boundary(start)
                .map_or(next_knot, |frame| frame.min(next_knot));
            let start_gain = self.value_at(start);
            let touch_end = self.touch.segment_end(start, next);
            let held = self.knots.is_empty()
                || self.knots[self
                    .knots
                    .partition_point(|knot| knot.frame <= start)
                    .saturating_sub(1)]
                .outgoing
                    == GainInterpolation::Step
                || self.knots.first().is_some_and(|knot| start < knot.frame);
            let end_gain = if let Some(end_gain) = touch_end {
                end_gain
            } else if held {
                start_gain
            } else {
                self.knots
                    .get(self.knots.partition_point(|knot| knot.frame < next))
                    .filter(|knot| knot.frame == next)
                    .map_or_else(
                        || self.base_value_at(next),
                        |knot| knot.incoming_gain as f32,
                    )
            };
            visit(GainPreviewSegment {
                start_frame: start,
                end_frame: next,
                start_gain,
                end_gain,
            });
            start = next;
        }
    }
    pub(crate) fn capacity(&self) -> GainAutomationCapacity {
        GainAutomationCapacity {
            authored_points: self.authored_points,
            prepared_knots: self.knots.len(),
            allocated_knot_slots: self.knots.capacity(),
            allocated_knot_bytes: self.knots.capacity() * std::mem::size_of::<GainEnvelopeKnot>(),
            authored_units: self.authored_units,
            prepared_touch_spans: self.touch.span_count(),
            prepared_touch_steps: self.touch.step_count(),
            allocated_touch_bytes: self.touch.allocated_bytes(),
        }
    }
}

fn base_gain_at(knots: &[GainEnvelopeKnot], fallback: f32, frame: u64) -> f32 {
    if knots.is_empty() {
        return fallback;
    }
    let index = knots
        .partition_point(|knot| knot.frame <= frame)
        .saturating_sub(1);
    let point = knots[index];
    if frame <= point.frame || point.outgoing == GainInterpolation::Step {
        return point.gain as f32;
    }
    let Some(next) = knots.get(index + 1) else {
        return point.gain as f32;
    };
    let fraction = (frame - point.frame) as f64 / (next.frame - point.frame) as f64;
    (point.gain + (next.incoming_gain - point.gain) * fraction) as f32
}

impl PreparedGainEnvelope {
    pub(crate) fn visit_audio_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        let Self {
            knots,
            mix: _,
            authored_points: _,
            authored_units: _,
            touch,
            fallback: _,
        } = self;
        if touch.allocated_bytes() != 0 {
            return Err(crate::AudioStorageError::UnknownStorage);
        }
        ledger.vec(knots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{BeatTime, Command, Created, Editor, Tempo, TempoPoint};
    #[test]
    fn maximum_authored_lanes_report_actual_knot_slots_and_bytes_with_dense_tempo() {
        let mut editor = Editor::new(Project::new(48000).unwrap(), 1).unwrap();
        for _ in 0..256 {
            let Created::Track(_) = editor
                .execute(Command::AddTrack {
                    name: "capacity".into(),
                })
                .unwrap()
            else {
                panic!("track")
            };
        }
        let mut snapshot = editor.project().snapshot();
        snapshot.tempo_points = (0..1024)
            .map(|i| {
                TempoPoint::new(
                    BeatTime::from_raw(i * (BeatTime::ONE.raw() / 1024)),
                    Tempo::new(120_000_000).unwrap(),
                )
            })
            .collect();
        for track in &mut snapshot.tracks {
            track.gain_envelope = (0..256)
                .map(|i| GainPoint {
                    incoming_gain: None,
                    beat: BeatTime::from_quarters(i).unwrap(),
                    gain: 1.0,
                    outgoing: GainInterpolation::Linear,
                })
                .collect();
        }
        let project = Project::from_snapshot(snapshot).unwrap();
        validate_project(&project).unwrap();
        let mut totals = GainAutomationCapacity::default();
        for track in project.tracks() {
            let prepared = PreparedGainEnvelope::prepare(
                track.gain_lane(),
                project.tempo_map(),
                PreparedTrackMix::new(1.0, 0.0, false).unwrap(),
                track.gain(),
            )
            .unwrap()
            .unwrap();
            let capacity = prepared.capacity();
            totals.authored_points += capacity.authored_points;
            totals.prepared_knots += capacity.prepared_knots;
            totals.allocated_knot_slots += capacity.allocated_knot_slots;
            totals.allocated_knot_bytes += capacity.allocated_knot_bytes;
        }
        assert_eq!(totals.authored_points, 65536);
        assert_eq!(totals.prepared_knots, 327424);
        assert!(totals.allocated_knot_slots >= totals.prepared_knots);
        assert!(totals.allocated_knot_slots <= MAX_PREPARED_GAIN_KNOTS);
        assert_eq!(
            totals.allocated_knot_bytes,
            totals.allocated_knot_slots * std::mem::size_of::<GainEnvelopeKnot>()
        );
        eprintln!(
            "measured prepared gain plan capacity: {totals:?}; knot layout {} bytes; not a process-memory or hardware-deadline bound",
            std::mem::size_of::<GainEnvelopeKnot>()
        );
    }
}
