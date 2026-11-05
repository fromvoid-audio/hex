//! Adjacent tempo boundaries in the existing effective beat coordinate. Curve
//! preparation uses stable neighbors rather than restarting ramps at callbacks.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TempoBreaks {
    /// Last boundary at or before the query, absent outside the effective domain.
    pub(crate) previous: Option<BeatTime>,
    /// First strictly later boundary, absent beyond the representable domain.
    pub(crate) next: Option<BeatTime>,
}
/// Existing clock replacement boundary in effective performance coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TempoRebaseBoundary {
    pub(crate) beat: BeatTime,
    pub(crate) frame: u64,
}
pub(crate) trait MidiTempoBreaks: MidiTiming {
    /// A replacement clock only owns ramps from this stable cut onward. The old
    /// clock remains the owner of outgoing tails; this does not change clip origin.
    fn rebase_boundary(&self) -> Result<Option<TempoRebaseBoundary>, PreparedTempoError>;

    fn tempo_breaks_around(&self, beat: BeatTime) -> Result<TempoBreaks, PreparedTempoError>;
    /// Conservative distinct-frame boundary count. Callers must skip equal-frame
    /// buckets with indexed queries and make positive frame progress, not scan
    /// collapsed markers. The frame cap does not bound such a scan's work.
    fn max_tempo_breaks_per_window(&self, frames: u64) -> Result<usize, PreparedTempoError>;
}
impl MidiTempoBreaks for PreparedTempoMap {
    fn rebase_boundary(&self) -> Result<Option<TempoRebaseBoundary>, PreparedTempoError> {
        Ok(None)
    }

    fn max_tempo_breaks_per_window(&self, frames: u64) -> Result<usize, PreparedTempoError> {
        let frames = usize::try_from(frames)
            .map_err(|_| PreparedTempoError::BeatToFrame("tempo boundary work budget overflow"))?;
        Ok(self.canonical.points().len().min(frames))
    }
    fn tempo_breaks_around(&self, beat: BeatTime) -> Result<TempoBreaks, PreparedTempoError> {
        let points = self.canonical.points();
        let index = points.partition_point(|point| point.beat() <= beat);
        Ok(TempoBreaks {
            previous: index
                .checked_sub(1)
                .and_then(|i| points.get(i))
                .map(|point| point.beat()),
            next: points.get(index).map(|point| point.beat()),
        })
    }
}
impl MidiTempoBreaks for PeriodicSessionTempoView<'_> {
    fn rebase_boundary(&self) -> Result<Option<TempoRebaseBoundary>, PreparedTempoError> {
        Ok(None)
    }

    fn max_tempo_breaks_per_window(&self, frames: u64) -> Result<usize, PreparedTempoError> {
        if frames == 0 {
            return Ok(0);
        }
        let cycles = frames
            .checked_div(self.cycle.frame_period)
            .and_then(|n| n.checked_add(2))
            .ok_or(PreparedTempoError::BeatToFrame(
                "tempo boundary work budget overflow",
            ))?;
        let count = u64::try_from(self.canonical.canonical.points().len())
            .ok()
            .and_then(|n| n.checked_add(1))
            .and_then(|n| n.checked_mul(cycles))
            .ok_or(PreparedTempoError::BeatToFrame(
                "tempo boundary work budget overflow",
            ))?;
        usize::try_from(count.min(frames))
            .map_err(|_| PreparedTempoError::BeatToFrame("tempo boundary work budget overflow"))
    }
    fn tempo_breaks_around(&self, beat: BeatTime) -> Result<TempoBreaks, PreparedTempoError> {
        if beat < self.cycle.start_beat {
            let mut breaks = self.canonical.tempo_breaks_around(beat)?;
            breaks.next = Some(breaks.next.map_or(self.cycle.start_beat, |next| {
                next.min(self.cycle.start_beat)
            }));
            return Ok(breaks);
        }
        let elapsed = beat.raw() - self.cycle.start_beat.raw();
        let cycles = elapsed / self.cycle.beat_period;
        let local =
            BeatTime::from_raw(self.cycle.start_beat.raw() + elapsed % self.cycle.beat_period);
        let breaks = self.canonical.tempo_breaks_around(local)?;
        let previous = breaks
            .previous
            .unwrap_or(self.cycle.start_beat)
            .max(self.cycle.start_beat);
        let next = breaks
            .next
            .unwrap_or(self.cycle.end_beat)
            .min(self.cycle.end_beat);
        // A periodic seam is a boundary even with equal BPM: the admitted clock
        // repeats an integer frame period rather than a new canonical conversion.
        let previous = self.lift_raw(cycles, previous.raw())?;
        let next = u128::from(cycles)
            .checked_mul(u128::from(self.cycle.beat_period))
            .and_then(|offset| offset.checked_add(u128::from(next.raw())))
            .ok_or(PreparedTempoError::BeatToFrame(
                "periodic tempo boundary overflow",
            ))?;
        Ok(TempoBreaks {
            previous: Some(previous),
            next: u64::try_from(next).ok().map(BeatTime::from_raw),
        })
    }
}
impl MidiTempoBreaks for AnchoredSessionTempoView<'_> {
    fn rebase_boundary(&self) -> Result<Option<TempoRebaseBoundary>, PreparedTempoError> {
        if self.beat_to_frame(self.anchor.old_beat)? != self.anchor.cut_frame {
            return Err(PreparedTempoError::BeatToFrame(
                "anchored tempo cut is inconsistent",
            ));
        }
        Ok(Some(TempoRebaseBoundary {
            beat: self.anchor.old_beat,
            frame: self.anchor.cut_frame,
        }))
    }

    fn max_tempo_breaks_per_window(&self, frames: u64) -> Result<usize, PreparedTempoError> {
        self.base.max_tempo_breaks_per_window(frames)
    }
    fn tempo_breaks_around(&self, beat: BeatTime) -> Result<TempoBreaks, PreparedTempoError> {
        let offset =
            i128::from(self.anchor.old_beat.raw()) - i128::from(self.anchor.new_beat.raw());
        let base = u64::try_from(i128::from(beat.raw()) - offset).map_err(|_| {
            PreparedTempoError::BeatToFrame("anchored beat exceeds the beat domain")
        })?;
        let breaks = self.base.tempo_breaks_around(BeatTime::from_raw(base))?;
        // An out-of-domain translated marker is absent rather than an error:
        // the queried beat is valid, and clamping would invent a tempo change
        // at beat zero.
        let translate = |point: Option<BeatTime>| {
            point.and_then(|point| {
                u64::try_from(i128::from(point.raw()) + offset)
                    .ok()
                    .map(BeatTime::from_raw)
            })
        };
        Ok(TempoBreaks {
            previous: translate(breaks.previous),
            next: translate(breaks.next),
        })
    }
}
impl MidiTempoBreaks for SessionBaseTimingView<'_> {
    fn rebase_boundary(&self) -> Result<Option<TempoRebaseBoundary>, PreparedTempoError> {
        match self {
            Self::Canonical(map) => map.rebase_boundary(),
            Self::Periodic(map) => map.rebase_boundary(),
        }
    }

    fn max_tempo_breaks_per_window(&self, frames: u64) -> Result<usize, PreparedTempoError> {
        match self {
            Self::Canonical(map) => map.max_tempo_breaks_per_window(frames),
            Self::Periodic(map) => map.max_tempo_breaks_per_window(frames),
        }
    }
    fn tempo_breaks_around(&self, beat: BeatTime) -> Result<TempoBreaks, PreparedTempoError> {
        match self {
            Self::Canonical(map) => map.tempo_breaks_around(beat),
            Self::Periodic(map) => map.tempo_breaks_around(beat),
        }
    }
}
impl MidiTempoBreaks for SessionTimingView<'_> {
    fn rebase_boundary(&self) -> Result<Option<TempoRebaseBoundary>, PreparedTempoError> {
        match self {
            Self::Canonical(map) => map.rebase_boundary(),
            Self::Periodic(map) => map.rebase_boundary(),
            Self::Anchored(map) => map.rebase_boundary(),
        }
    }

    fn max_tempo_breaks_per_window(&self, frames: u64) -> Result<usize, PreparedTempoError> {
        match self {
            Self::Canonical(map) => map.max_tempo_breaks_per_window(frames),
            Self::Periodic(map) => map.max_tempo_breaks_per_window(frames),
            Self::Anchored(map) => map.max_tempo_breaks_per_window(frames),
        }
    }
    fn tempo_breaks_around(&self, beat: BeatTime) -> Result<TempoBreaks, PreparedTempoError> {
        match self {
            Self::Canonical(map) => map.tempo_breaks_around(beat),
            Self::Periodic(map) => map.tempo_breaks_around(beat),
            Self::Anchored(map) => map.tempo_breaks_around(beat),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn beat(n: u64) -> BeatTime {
        BeatTime::from_quarters(n).unwrap()
    }
    fn map() -> PreparedTempoMap {
        PreparedTempoMap::prepare(
            TempoMap::new(
                48_000,
                vec![
                    hex_project::TempoPoint::new(
                        beat(0),
                        hex_project::Tempo::new(120_000_000).unwrap(),
                    ),
                    hex_project::TempoPoint::new(
                        beat(3),
                        hex_project::Tempo::new(90_000_000).unwrap(),
                    ),
                    hex_project::TempoPoint::new(
                        beat(5),
                        hex_project::Tempo::new(60_000_000).unwrap(),
                    ),
                ],
            )
            .unwrap(),
            48_000,
        )
        .unwrap()
    }
    #[test]
    fn canonical_neighbors_include_equal_boundary_and_hold_the_last_tempo() {
        let map = map();
        assert_eq!(
            map.tempo_breaks_around(beat(0)).unwrap(),
            TempoBreaks {
                previous: Some(beat(0)),
                next: Some(beat(3))
            }
        );
        assert_eq!(
            map.tempo_breaks_around(beat(3)).unwrap(),
            TempoBreaks {
                previous: Some(beat(3)),
                next: Some(beat(5))
            }
        );
        assert_eq!(
            map.tempo_breaks_around(BeatTime::from_raw(u64::MAX))
                .unwrap(),
            TempoBreaks {
                previous: Some(beat(5)),
                next: None
            }
        );
    }
    #[test]
    fn periodic_neighbors_use_direct_cycle_lookup_and_preserve_seams() {
        let map = map();
        let cycle = PreparedPeriodicSessionTempo::prepare(
            &map,
            LoopRegion::new(
                map.beat_to_frame(beat(2)).unwrap(),
                map.beat_to_frame(beat(4)).unwrap(),
                true,
            )
            .unwrap(),
        )
        .unwrap();
        let view = cycle.view(&map);
        assert_eq!(
            view.tempo_breaks_around(beat(1)).unwrap(),
            TempoBreaks {
                previous: Some(beat(0)),
                next: Some(beat(2))
            }
        );
        assert_eq!(
            view.tempo_breaks_around(beat(4)).unwrap(),
            TempoBreaks {
                previous: Some(beat(4)),
                next: Some(beat(5))
            }
        );
        assert_eq!(
            view.tempo_breaks_around(beat(5)).unwrap(),
            TempoBreaks {
                previous: Some(beat(5)),
                next: Some(beat(6))
            }
        );
        let late = view
            .tempo_breaks_around(BeatTime::from_raw(u64::MAX))
            .unwrap();
        assert_eq!(
            late.previous,
            Some(BeatTime::from_raw(u64::MAX - BeatTime::ONE.raw() + 1))
        );
        assert_eq!(late.next, None);
    }
    #[test]
    fn rebase_boundary_is_the_existing_checked_clock_cut() {
        let map = map();
        assert_eq!(map.rebase_boundary().unwrap(), None);
        let anchored = AnchoredSessionTempoView::new(
            SessionBaseTimingView::Canonical(&map),
            SessionClockAnchor {
                cut_frame: 24000,
                old_beat: beat(7),
                new_beat: beat(1),
            },
        );
        assert_eq!(
            anchored.rebase_boundary().unwrap(),
            Some(TempoRebaseBoundary {
                beat: beat(7),
                frame: 24000
            })
        );
        assert_eq!(
            SessionTimingView::Anchored(anchored)
                .rebase_boundary()
                .unwrap(),
            anchored.rebase_boundary().unwrap()
        );
        assert_eq!(anchored.beat_to_frame(beat(7)).unwrap(), 24000);
    }
    #[test]
    fn anchored_neighbors_translate_without_clamping_absent_markers_to_zero() {
        let map = map();
        let view = AnchoredSessionTempoView::new(
            SessionBaseTimingView::Canonical(&map),
            SessionClockAnchor {
                cut_frame: 0,
                old_beat: beat(0),
                new_beat: beat(2),
            },
        );
        assert_eq!(
            view.tempo_breaks_around(beat(0)).unwrap(),
            TempoBreaks {
                previous: None,
                next: Some(beat(1))
            }
        );
        assert_eq!(
            view.tempo_breaks_around(beat(1)).unwrap(),
            TempoBreaks {
                previous: Some(beat(1)),
                next: Some(beat(3))
            }
        );
    }
}
