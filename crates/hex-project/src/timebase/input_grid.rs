//! Transient musical input quantization. UI resolves adaptive steps before async
//! work; recordings and imports retain this immutable control-side snapshot.
//! This is not saved project state, a launch scheduler or an audio-thread API.
use super::{BeatTime, MAX_METER_POINTS, MeterMap, TempoMap};
use crate::Invalid;

/// One shared input policy; adaptive density/layout remains a UI responsibility.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputQuantization {
    #[default]
    Off,
    Quarter,
    Eighth,
    Sixteenth,
    Adaptive,
}
impl InputQuantization {
    pub const CHOICES: [Self; 5] = [
        Self::Off,
        Self::Quarter,
        Self::Eighth,
        Self::Sixteenth,
        Self::Adaptive,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Quarter => "1/4",
            Self::Eighth => "1/8",
            Self::Sixteenth => "1/16",
            Self::Adaptive => "adaptive",
        }
    }
    pub fn fixed_step(self) -> Option<BeatTime> {
        match self {
            Self::Quarter => Some(BeatTime::ONE),
            Self::Eighth => Some(BeatTime::from_raw(BeatTime::ONE.raw() / 2)),
            Self::Sixteenth => Some(BeatTime::from_raw(BeatTime::ONE.raw() / 4)),
            Self::Off | Self::Adaptive => None,
        }
    }
}

/// Landmarks begin at a meter origin and stop strictly before its next marker.
/// A meter change is itself the next segment's first landmark. Ties move later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MusicalGridSegment {
    origin: BeatTime,
    end: Option<BeatTime>,
    step: BeatTime,
}
impl MusicalGridSegment {
    pub fn new(origin: BeatTime, end: Option<BeatTime>, step: BeatTime) -> Result<Self, Invalid> {
        if step.raw() == 0 || end.is_some_and(|end| end <= origin) {
            return Err(Invalid(
                "musical grid segment must have positive step and extent",
            ));
        }
        Ok(Self { origin, end, step })
    }
    pub fn nearest(self, beat: BeatTime) -> Option<BeatTime> {
        let lower = beat.raw().saturating_sub(self.origin.raw()) / self.step.raw();
        [Some(lower), lower.checked_add(1)]
            .into_iter()
            .flatten()
            .filter_map(|index| {
                self.origin
                    .raw()
                    .checked_add(index.checked_mul(self.step.raw())?)
            })
            .filter(|raw| self.end.is_none_or(|end| *raw < end.raw()))
            .min_by_key(|raw| (raw.abs_diff(beat.raw()), u64::MAX - *raw))
            .map(BeatTime::from_raw)
    }
}

/// A bounded resolved grid, independent of later UI zoom, density and mode.
/// Empty segments mean off and preserve frame timing without a beat round trip.
/// Optional painted landmarks preserve the arranger's dense-meter fallback.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolvedInputGrid {
    segments: Vec<MusicalGridSegment>,
    visible: Option<VisibleLandmarks>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct VisibleLandmarks {
    start: u64,
    end: u64,
    beats: Vec<BeatTime>,
}
impl ResolvedInputGrid {
    pub fn off() -> Self {
        Self::default()
    }
    pub fn is_off(&self) -> bool {
        self.segments.is_empty()
    }
    pub fn new(segments: Vec<MusicalGridSegment>) -> Result<Self, Invalid> {
        if segments.is_empty()
            || segments.len() > MAX_METER_POINTS
            || segments[0].origin != BeatTime::ZERO
            || segments.last().is_some_and(|last| last.end.is_some())
            || segments
                .windows(2)
                .any(|pair| pair[0].end != Some(pair[1].origin))
        {
            return Err(Invalid(
                "resolved input grid requires a bounded complete meter partition",
            ));
        }
        Ok(Self {
            segments,
            visible: None,
        })
    }
    pub fn fixed(meter: &MeterMap, step: BeatTime) -> Result<Self, Invalid> {
        let points = meter.points();
        let mut segments = Vec::new();
        segments
            .try_reserve_exact(points.len())
            .map_err(|_| Invalid("could not reserve input grid"))?;
        for (index, point) in points.iter().enumerate() {
            segments.push(MusicalGridSegment::new(
                point.beat(),
                points.get(index + 1).map(|p| p.beat()),
                step,
            )?);
        }
        Self::new(segments)
    }
    pub fn with_visible_landmarks(
        mut self,
        start: u64,
        end: u64,
        beats: Vec<BeatTime>,
    ) -> Result<Self, Invalid> {
        if self.is_off()
            || start >= end
            || beats.is_empty()
            || beats.len() > 256
            || beats.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(Invalid(
                "resolved visible input grid has invalid bounds or landmarks",
            ));
        }
        self.visible = Some(VisibleLandmarks { start, end, beats });
        Ok(self)
    }
    pub fn snap_beat(&self, tempo: &TempoMap, beat: BeatTime) -> Result<BeatTime, Invalid> {
        if self.is_off() {
            return Ok(beat);
        }
        let current = self
            .segments
            .partition_point(|segment| segment.origin <= beat)
            .saturating_sub(1);
        let first = current.saturating_sub(1);
        let last = current.saturating_add(2).min(self.segments.len());
        let candidate = self.segments[first..last]
            .iter()
            .filter_map(|segment| segment.nearest(beat))
            .min_by_key(|candidate| {
                (
                    candidate.raw().abs_diff(beat.raw()),
                    u64::MAX - candidate.raw(),
                )
            })
            .ok_or(Invalid("input grid has no representable landmark"))?;
        if let Some(visible) = &self.visible
            && (visible.start..visible.end).contains(&tempo.beat_to_frame(candidate)?)
        {
            return visible
                .beats
                .iter()
                .copied()
                .min_by_key(|candidate| {
                    (
                        candidate.raw().abs_diff(beat.raw()),
                        u64::MAX - candidate.raw(),
                    )
                })
                .ok_or(Invalid("visible input grid is empty"));
        }
        Ok(candidate)
    }
    pub fn snap_frame(&self, tempo: &TempoMap, frame: u64) -> Result<u64, Invalid> {
        if self.is_off() {
            return Ok(frame);
        }
        tempo.beat_to_frame(self.snap_beat(tempo, tempo.frame_to_beat(frame)?)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Meter, MeterPoint};
    #[test]
    fn nearest_meter_landmarks_include_fragmentary_seams_and_forward_ties() {
        let meter = MeterMap::new(vec![
            MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME),
            MeterPoint::new(
                BeatTime::from_ratio(5, 8).unwrap(),
                Meter::new(7, 8).unwrap(),
            ),
        ])
        .unwrap();
        let grid = ResolvedInputGrid::fixed(&meter, BeatTime::from_ratio(1, 2).unwrap()).unwrap();
        let tempo = TempoMap::constant(48_000, super::super::Tempo::DEFAULT).unwrap();
        for (input, expected) in [(3, 4), (4, 4), (5, 5), (6, 5), (7, 9), (8, 9)] {
            assert_eq!(
                grid.snap_beat(&tempo, BeatTime::from_ratio(input, 8).unwrap())
                    .unwrap(),
                BeatTime::from_ratio(expected, 8).unwrap()
            );
        }
        assert_eq!(
            ResolvedInputGrid::off().snap_frame(&tempo, 12_345).unwrap(),
            12_345
        );
    }
    #[test]
    fn invalid_partitions_and_overflow_are_explicit() {
        assert!(MusicalGridSegment::new(BeatTime::ZERO, None, BeatTime::ZERO).is_err());
        assert!(ResolvedInputGrid::new(vec![]).is_err());
        assert!(
            ResolvedInputGrid::new(vec![
                MusicalGridSegment::new(BeatTime::ONE, None, BeatTime::ONE).unwrap()
            ])
            .is_err()
        );
        let segment =
            MusicalGridSegment::new(BeatTime::ZERO, None, BeatTime::from_raw(u64::MAX)).unwrap();
        assert_eq!(
            segment.nearest(BeatTime::from_raw(u64::MAX)),
            Some(BeatTime::from_raw(u64::MAX))
        );
        assert!(
            ResolvedInputGrid::off()
                .with_visible_landmarks(0, 1, vec![BeatTime::ZERO])
                .is_err()
        );
    }
}
