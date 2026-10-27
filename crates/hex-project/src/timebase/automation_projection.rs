//! Musical placement of immutable captured sample-frame automation.
//! Version one converts captured frames through the captured map's exact beat
//! inverse, then the destination map's canonical ties-later forward conversion.
//! Project coverage boundaries forward too. A destination frame selects the
//! latest covered source frame whose FORWARD projection reaches it or earlier;
//! a naive inverse destination clock can select the wrong rounded boundary.
//! This pure control-side descriptor borrows maps, allocates nothing and owns
//! no file, captured events, project revision, callback or persisted reference.
use super::TempoMap;
use crate::Invalid;
use std::ops::Range;

pub const AUTOMATION_PROJECTION_VERSION: u32 = 1;

/// Emit the exact versioned destination-map identity framing, without allocation.
/// Hashing owners choose their digest; all consumers share this byte contract.
/// Captured-map identity is supplied separately by the immutable source.
pub fn visit_automation_projection_identity(map: &TempoMap, visit: &mut impl FnMut(&[u8])) {
    visit(b"hex.automation.projection\0");
    visit(&AUTOMATION_PROJECTION_VERSION.to_le_bytes());
    visit(&map.sample_rate().to_le_bytes());
    visit(&(map.points().len() as u64).to_le_bytes());
    for point in map.points() {
        visit(&point.beat().raw().to_le_bytes());
        visit(&point.tempo().micro_bpm().to_le_bytes());
    }
}

#[derive(Clone, Copy)]
pub struct CapturedAutomationProjection<'a> {
    captured: &'a TempoMap,
    destination: &'a TempoMap,
}
impl<'a> CapturedAutomationProjection<'a> {
    pub fn new(captured: &'a TempoMap, destination: &'a TempoMap) -> Self {
        Self {
            captured,
            destination,
        }
    }
    /// Preserve immutable event origins; return their new placement only.
    /// Unrepresentable source beats or destination frames explicitly refuse.
    pub fn project_frame(self, captured_frame: u64) -> Result<u64, Invalid> {
        self.destination
            .beat_to_frame(self.captured.frame_to_beat(captured_frame)?)
    }
    /// Both maps use nonnegative sample frames in their own declared rate.
    /// A valid nonempty source interval may project to an empty destination.
    pub fn project_range(self, captured: Range<u64>) -> Result<Range<u64>, Invalid> {
        if captured.start >= captured.end {
            return Err(Invalid("captured automation range must be nonempty"));
        }
        let start = self.project_frame(captured.start)?;
        let end = self.project_frame(captured.end)?;
        if start > end {
            return Err(Invalid("captured automation projection is not monotonic"));
        }
        Ok(start..end)
    }
    /// Locate the source frame with latest effective event precedence at this
    /// destination position. Half-open ends and collapsed intervals contribute
    /// nothing. Search takes at most 64 iterations plus endpoint validation,
    /// independent of take duration/event/pass count, using canonical conversion.
    pub fn source_frame_at(
        self,
        destination_frame: u64,
        captured: Range<u64>,
    ) -> Result<Option<u64>, Invalid> {
        if !self
            .project_range(captured.clone())?
            .contains(&destination_frame)
        {
            return Ok(None);
        }
        let mut left = captured.start;
        let mut right = captured.end;
        while left < right {
            let middle = left + (right - left) / 2;
            if self.project_frame(middle)? <= destination_frame {
                left = middle + 1;
            } else {
                right = middle;
            }
        }
        Ok(left.checked_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BeatTime, Tempo, TempoPoint};

    fn map(rate: u32, tempo: u32) -> TempoMap {
        TempoMap::constant(rate, Tempo::new(tempo).unwrap()).unwrap()
    }

    #[test]
    fn unchanged_clock_is_exact_and_projected_frame_collisions_choose_later_source() {
        let source = map(48_000, 120_000_000);
        let identity = CapturedAutomationProjection::new(&source, &source);
        for frame in [0, 1, 23_999, 24_000, 48_001, 1_000_000_000] {
            assert_eq!(identity.project_frame(frame).unwrap(), frame);
            assert_eq!(
                identity.source_frame_at(frame, frame..frame + 1).unwrap(),
                Some(frame)
            );
            assert_eq!(
                identity
                    .source_frame_at(frame + 1, frame..frame + 1)
                    .unwrap(),
                None
            );
        }
        let fast = map(48_000, 240_000_000);
        let projection = CapturedAutomationProjection::new(&source, &fast);
        assert_eq!(projection.project_frame(1).unwrap(), 1); // half ties later
        assert_eq!(projection.project_frame(2).unwrap(), 1);
        assert_eq!(projection.source_frame_at(1, 0..10).unwrap(), Some(2));
        assert_eq!(projection.project_range(1..2).unwrap(), 1..1);
        assert_eq!(projection.source_frame_at(1, 1..2).unwrap(), None);
    }

    #[test]
    fn bounded_forward_location_matches_an_independent_exhaustive_oracle() {
        let source = TempoMap::new(
            44_100,
            vec![
                TempoPoint::new(BeatTime::ZERO, Tempo::new(137_000_000).unwrap()),
                TempoPoint::new(
                    BeatTime::from_raw(9_000_000),
                    Tempo::new(321_000_000).unwrap(),
                ),
                TempoPoint::new(
                    BeatTime::from_raw(40_000_000),
                    Tempo::new(71_000_000).unwrap(),
                ),
            ],
        )
        .unwrap();
        for destination in [
            map(8_000, 1_000_000_000),
            map(192_000, 71_000_000),
            source.clone(),
        ] {
            let projection = CapturedAutomationProjection::new(&source, &destination);
            for range in [0..3, 1..128, 71..320] {
                let start = destination
                    .beat_to_frame(source.frame_to_beat(range.start).unwrap())
                    .unwrap();
                let end = destination
                    .beat_to_frame(source.frame_to_beat(range.end).unwrap())
                    .unwrap();
                let forward: Vec<_> = range
                    .clone()
                    .map(|frame| {
                        (
                            frame,
                            destination
                                .beat_to_frame(source.frame_to_beat(frame).unwrap())
                                .unwrap(),
                        )
                    })
                    .collect();
                assert!(forward.windows(2).all(|pair| pair[0].1 <= pair[1].1));
                for frame in 0..=end + 1 {
                    let expected = if (start..end).contains(&frame) {
                        forward
                            .iter()
                            .rev()
                            .find(|(_, placed)| *placed <= frame)
                            .map(|(source, _)| *source)
                    } else {
                        None
                    };
                    assert_eq!(
                        projection.source_frame_at(frame, range.clone()).unwrap(),
                        expected
                    );
                }
            }
        }
    }

    #[test]
    fn invalid_and_unrepresentable_ranges_refuse_without_saturation() {
        let map = map(48_000, 120_000_000);
        let projection = CapturedAutomationProjection::new(&map, &map);
        assert!(projection.project_range(1..1).is_err());
        assert!(
            projection
                .project_range(Range { start: 2, end: 1 })
                .is_err()
        );
        assert!(projection.project_frame(u64::MAX).is_err());
        assert!(projection.source_frame_at(0, 0..u64::MAX).is_err());
    }
}
