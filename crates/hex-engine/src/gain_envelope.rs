//! Control-prepared absolute track gains; render queries have no mutable cursor.
use crate::{Invalid, PreparedTrackMix};
use hex_project::{GainInterpolation, GainPoint, Project, TempoMap};

pub const MAX_PREPARED_GAIN_KNOTS: usize = 327_680;
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
#[derive(Clone, Debug)]
pub(crate) struct PreparedGainEnvelope {
    knots: Vec<GainEnvelopeKnot>,
    mix: PreparedTrackMix,
    authored_points: usize,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GainAutomationCapacity {
    pub authored_points: usize,
    pub prepared_knots: usize,
    pub allocated_knot_slots: usize,
    pub allocated_knot_bytes: usize,
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
pub(crate) fn validate_project(project: &Project) -> Result<(), Invalid> {
    let mut authored = 0_usize;
    let mut prepared = 0_usize;
    for track in project.tracks() {
        authored = authored
            .checked_add(track.gain_envelope().len())
            .ok_or(Invalid("gain point count overflow"))?;
        prepared = prepared
            .checked_add(knot_count(track.gain_envelope(), project.tempo_map())?)
            .ok_or(Invalid("prepared gain knot count overflow"))?;
    }
    if authored > hex_project::MAX_GAIN_POINTS || prepared > MAX_PREPARED_GAIN_KNOTS {
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
            *knots.last_mut().expect("existing knot") = knot;
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
    pub(crate) fn prepare(
        points: &[GainPoint],
        tempo: &TempoMap,
        mix: PreparedTrackMix,
    ) -> Result<Option<Self>, Invalid> {
        if points.is_empty() {
            return Ok(None);
        }
        let count = knot_count(points, tempo)?;
        if count > MAX_PREPARED_GAIN_KNOTS {
            return Err(Invalid("prepared gain knot limit reached"));
        }
        let mut knots = Vec::new();
        knots
            .try_reserve_exact(count)
            .map_err(|_| Invalid("could not reserve gain automation knots"))?;
        fill_knots(&mut knots, points, tempo, |_| Ok(()))?;
        Ok(Some(Self {
            knots,
            mix,
            authored_points: points.len(),
        }))
    }
    pub(crate) fn checked_backing_bytes(&self) -> Result<usize, Invalid> {
        let Self {
            knots,
            mix: _,
            authored_points: _,
        } = self;
        knots
            .capacity()
            .checked_mul(std::mem::size_of::<GainEnvelopeKnot>())
            .ok_or(Invalid("gain capacity overflow"))
    }
    pub(crate) fn knots(&self) -> &[GainEnvelopeKnot] {
        &self.knots
    }
    pub(crate) fn mix_at(&self, frame: u64) -> PreparedTrackMix {
        self.mix.with_absolute_gain(self.value_at(frame))
    }
    pub(crate) fn value_at(&self, frame: u64) -> f32 {
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
    pub(crate) fn capacity(&self) -> (usize, usize, usize, usize) {
        (
            self.authored_points,
            self.knots.len(),
            self.knots.capacity(),
            self.knots.capacity() * std::mem::size_of::<GainEnvelopeKnot>(),
        )
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
                track.gain_envelope(),
                project.tempo_map(),
                PreparedTrackMix::new(1.0, 0.0, false).unwrap(),
            )
            .unwrap()
            .unwrap();
            let (authored, count, slots, bytes) = prepared.capacity();
            totals.authored_points += authored;
            totals.prepared_knots += count;
            totals.allocated_knot_slots += slots;
            totals.allocated_knot_bytes += bytes;
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
