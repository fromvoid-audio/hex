//! Control-prepared absolute track gains; render queries have no mutable cursor.
use crate::{Invalid, PreparedTrackMix};
use hex_project::{GainInterpolation, GainLane, GainPoint, Project, TempoMap};

#[cfg(test)]
mod construction_tests;
mod touch;
use touch::PreparedTouch;

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
    outgoing: GainInterpolation,
}
impl GainEnvelopeKnot {
    pub fn frame(&self) -> u64 {
        self.frame
    }
    pub fn gain(&self) -> f64 {
        self.gain
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
/// Fresh preview values enter the existing paired facade once. Clones retain
/// pairing; no raw Arc adoption or mutable gain-preview owner is exposed.
/// ```compile_fail
/// let raw = std::sync::Arc::new(hex_engine::GainAutomationPreview::default());
/// let _: hex_engine::SharedGainAutomationPreview = raw.into();
/// ```
pub type SharedGainAutomationPreview = crate::SharedMetadata<GainAutomationPreview>;

/// Actual selected outer/track/knot/touch storage. Enclosing view, every other
/// owner and full construction/custody/publication obligations remain separate.
pub fn visit_shared_gain_preview_storage_on_control<'a>(
    owner: &'a SharedGainAutomationPreview,
    ledger: &mut crate::AudioRootStorageLedger<'a>,
) -> Result<(), crate::AudioStorageError> {
    ledger.borrow_shared_metadata_body_on_control(owner)?;
    ledger.mutate(|ledger| {
        ledger.vec(&owner.tracks)?;
        for (_, envelope) in &owner.tracks {
            envelope.visit_audio_storage(ledger)?;
        }
        Ok(())
    })
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
    /// Visit exact base-ramp and held-touch segments clipped to a frame range.
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
/// Constructor-local requested backing; not a union or admission token.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GainConstructionCost {
    pub(crate) units: usize,
    pub(crate) backing_bytes: usize,
}
pub(crate) fn validate_project(project: &Project) -> Result<(), Invalid> {
    let mut authored = 0_usize;
    let mut prepared = 0_usize;
    let mut bytes = 0_usize;
    for track in project.tracks() {
        authored = authored
            .checked_add(track.gain_lane().units().map_err(|e| Invalid(e.0))?)
            .ok_or(Invalid("gain point count overflow"))?;
        let cost = PreparedGainEnvelope::construction_cost(track.gain_lane(), project.tempo_map())?;
        let (count, size) = (cost.units, cost.backing_bytes);
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
                    outgoing: GainInterpolation|
     -> Result<(), Invalid> {
        let frame = tempo.beat_to_frame(beat).map_err(|e| Invalid(e.0))?;
        let knot = GainEnvelopeKnot {
            frame,
            gain,
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
        push(point.beat, f64::from(point.gain), point.outgoing)?;
        if point.outgoing == GainInterpolation::Linear
            && let Some(next) = points.get(index + 1)
        {
            for marker in markers_between(tempo, point.beat, next.beat) {
                let fraction = (marker.beat().raw() - point.beat.raw()) as f64
                    / (next.beat.raw() - point.beat.raw()) as f64;
                let gain = f64::from(point.gain)
                    + (f64::from(next.gain) - f64::from(point.gain)) * fraction;
                push(marker.beat(), gain, GainInterpolation::Linear)?;
            }
        }
    }
    Ok(())
}
impl PreparedGainEnvelope {
    /// Fixed control teardown of this closed scalar-Vec value only. No opaque
    /// provider, callback, shared owner, allocation or fallible operation.
    pub(crate) fn dispose_on_control(self) {
        let Self {
            knots,
            mix,
            authored_points,
            authored_units,
            touch,
            fallback,
        } = self;
        fn copy_leaf<T: Copy>() {}
        copy_leaf::<GainEnvelopeKnot>();
        fn copy<T: Copy>(_: T) {}
        copy((mix, authored_points, authored_units, fallback));
        drop(knots);
        touch.dispose_on_control();
    }
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
    /// Pure actual authored-lane/tempo sizing. Existing per-track/project
    /// validation remains required; prepared touch arrays and tempo-expanded
    /// base knots are both included without constructing either one.
    pub(crate) fn construction_cost(
        lane: &GainLane,
        tempo: &TempoMap,
    ) -> Result<GainConstructionCost, Invalid> {
        let (units, backing_bytes) = plan_counts(lane, tempo)?;
        Ok(GainConstructionCost {
            units,
            backing_bytes,
        })
    }
    pub(crate) fn prepare(
        lane: &GainLane,
        tempo: &TempoMap,
        mix: PreparedTrackMix,
        fallback: f32,
    ) -> Result<Option<Self>, Invalid> {
        let cost = Self::construction_cost(lane, tempo)?;
        let (units, bytes) = (cost.units, cost.backing_bytes);
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
        let prepared = Self {
            knots,
            mix,
            authored_points: points.len(),
            authored_units: lane.units().map_err(|e| Invalid(e.0))?,
            touch: PreparedTouch::prepare(lane, tempo)?,
            fallback,
        };
        if prepared.checked_backing_bytes()? > bytes {
            return Err(Invalid(
                "prepared gain constructor exceeded its backing bound",
            ));
        }
        Ok(Some(prepared))
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
    fn base_value_at(&self, frame: u64) -> f32 {
        if self.knots.is_empty() {
            return self.fallback;
        }
        let index = self
            .knots
            .partition_point(|knot| knot.frame <= frame)
            .saturating_sub(1);
        let point = self.knots[index];
        if frame <= point.frame || point.outgoing == GainInterpolation::Step {
            return point.gain as f32;
        }
        let Some(next) = self.knots.get(index + 1) else {
            return point.gain as f32;
        };
        let fraction = (frame - point.frame) as f64 / (next.frame - point.frame) as f64;
        (point.gain + (next.gain - point.gain) * fraction) as f32
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
            let held = self.touch.value_at(start).is_some()
                || self.knots.is_empty()
                || self.knots[self
                    .knots
                    .partition_point(|knot| knot.frame <= start)
                    .saturating_sub(1)]
                .outgoing
                    == GainInterpolation::Step
                || self.knots.first().is_some_and(|knot| start < knot.frame);
            let end_gain = if held {
                start_gain
            } else {
                self.base_value_at(next)
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

impl PreparedGainEnvelope {
    pub(crate) fn visit_audio_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        let Self {
            knots,
            mix,
            authored_points,
            authored_units,
            touch,
            fallback,
        } = self;
        let _: (&usize, &usize, &f32) = (authored_points, authored_units, fallback);
        let PreparedTrackMix {
            channel_gain,
            effects,
            muted,
        } = mix;
        let _: (&crate::Frame, &Option<hex_lattice::StereoMatrix>, &bool) =
            (channel_gain, effects, muted);
        let _: Option<hex_lattice::StereoMatrix> = *effects; // fixed Copy matrix
        for GainEnvelopeKnot {
            frame,
            gain,
            outgoing,
        } in knots
        {
            let _: (&u64, &f64) = (frame, gain);
            match outgoing {
                GainInterpolation::Step | GainInterpolation::Linear => {}
            }
        }
        ledger.vec(knots)?;
        touch.visit_audio_storage(ledger)
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

#[cfg(test)]
mod shared_preview_storage_tests {
    use super::*;
    use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation};
    use hex_project::{
        BeatTime, Command, Created, Editor, GainTouchContext, GainTouchSpan, GainTouchStep,
    };
    fn reservation() -> AudioStorageLedgerReservation {
        AudioStorageLedgerReservation {
            data_rows: 128,
            handle_rows: 128,
            root_rows: 8,
            max_retained_bytes: 2_000_000,
            max_construction_peak_bytes: 2_000_000,
        }
    }
    fn children(preview: &GainAutomationPreview) -> usize {
        preview.tracks.capacity()
            * std::mem::size_of::<(hex_project::TrackId, PreparedGainEnvelope)>()
            + preview
                .tracks
                .iter()
                .map(|(_, envelope)| {
                    envelope.knots.capacity() * std::mem::size_of::<GainEnvelopeKnot>()
                        + envelope.touch.allocated_bytes()
                })
                .sum::<usize>()
    }
    #[test]
    fn shared_gain_preview_actual_producer_aliases_and_distinct_children_match_exact_limits() {
        if !crate::audio_storage_constructor_layout_supported() {
            eprintln!("unsupported paired gain-preview constructor; no positive storage proof");
            return;
        }
        eprintln!("SUPPORTED actual prepared gain preview with knots and recorded touch storage");
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "gain".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let mut snapshot = editor.project().snapshot();
        snapshot.tracks[0].gain_envelope = vec![
            GainPoint {
                beat: BeatTime::ZERO,
                gain: 1.0,
                outgoing: GainInterpolation::Linear,
            },
            GainPoint {
                beat: BeatTime::from_quarters(4).unwrap(),
                gain: 0.25,
                outgoing: GainInterpolation::Step,
            },
        ];
        snapshot.tracks[0].gain_touch_epoch = 1;
        snapshot.tracks[0].gain_touch_spans = vec![GainTouchSpan {
            start: BeatTime::ONE,
            end: BeatTime::from_quarters(3).unwrap(),
            context: GainTouchContext {
                recording_epoch: 1,
                project_generation: 0,
                canonical_revision: 1,
                audio_revision: 1,
                transport_epoch: 1,
                gesture_epoch: 1,
                gesture_serial: 1,
                sample_rate: 48_000,
                loop_pass: 0,
            },
            steps: [(1, 0.5), (2, 1.5)]
                .into_iter()
                .enumerate()
                .map(|(index, (quarter, gain))| {
                    let beat = BeatTime::from_quarters(quarter).unwrap();
                    GainTouchStep {
                        beat,
                        gain,
                        origin_beat: beat,
                        source_sequence: index as u64 + 1,
                    }
                })
                .collect(),
        }];
        let project = Project::from_snapshot(snapshot).unwrap();
        let owner = SharedGainAutomationPreview::new_on_control(
            GainAutomationPreview::prepare(&project).unwrap(),
        );
        assert_eq!(owner.value_at(track, 24_000), Some(0.5));
        assert_eq!(owner.value_at(track, 48_000), Some(1.5));
        let alias = owner.clone();
        let separate = SharedGainAutomationPreview::new_on_control(owner.as_ref().clone());
        assert!(!owner.shares_owner_with(&separate));
        assert!(children(&owner) > 0);
        let collect = |reservation| -> Result<_, crate::AudioStorageError> {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation)?;
            ledger.borrow_shared_metadata_body_on_control(&owner)?;
            let body = ledger.summary()?.data_bytes;
            assert!(body > std::mem::size_of::<GainAutomationPreview>());
            visit_shared_gain_preview_storage_on_control(&owner, &mut ledger)?;
            assert_eq!(ledger.summary()?.data_bytes, body + children(&owner));
            assert!(
                ledger
                    .validate_selected_shared_handles_on_control()
                    .is_err()
            );
            visit_shared_gain_preview_storage_on_control(&alias, &mut ledger)?;
            ledger.validate_selected_shared_handles_on_control()?;
            assert_eq!(ledger.summary()?.data_bytes, body + children(&owner));
            // Selected closure cannot detect an entirely omitted independent
            // owner: compare the actual separate allocation/backings explicitly.
            visit_shared_gain_preview_storage_on_control(&separate, &mut ledger)?;
            ledger.validate_selected_shared_handles_on_control()?;
            assert_eq!(
                ledger.summary()?.data_bytes,
                body * 2 + children(&owner) + children(&separate)
            );
            assert!(ledger.summary()?.require_complete().is_err());
            ledger.summary()
        };
        let measured = collect(reservation()).unwrap();
        let mut exact = reservation();
        exact.max_retained_bytes = measured.retained_bytes;
        exact.max_construction_peak_bytes = measured.construction_peak_bytes;
        assert_eq!(collect(exact).unwrap(), measured);
        exact.max_retained_bytes -= 1;
        assert!(collect(exact).is_err());
        drop(owner);
        drop(separate);
        drop(project);
        assert_eq!(alias.value_at(track, 48_000), Some(1.5));
    }
    #[test]
    fn shared_gain_preview_empty_reserved_track_backing_is_charged_after_clear() {
        if !crate::audio_storage_constructor_layout_supported() {
            eprintln!("unsupported paired gain-preview constructor; no positive storage proof");
            return;
        }
        eprintln!("SUPPORTED gain preview empty reserved actual track backing");
        let mut preview = GainAutomationPreview::default();
        preview.tracks.try_reserve_exact(19).unwrap();
        preview.tracks.clear();
        let bytes = children(&preview);
        assert!(bytes >= 19 * std::mem::size_of::<(hex_project::TrackId, PreparedGainEnvelope)>());
        let owner = SharedGainAutomationPreview::new_on_control(preview);
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        ledger
            .borrow_shared_metadata_body_on_control(&owner)
            .unwrap();
        let body = ledger.summary().unwrap().data_bytes;
        visit_shared_gain_preview_storage_on_control(&owner, &mut ledger).unwrap();
        assert_eq!(ledger.summary().unwrap().data_bytes, body + bytes);
        assert_eq!(ledger.summary().unwrap().data_allocations, 2);
    }
}
