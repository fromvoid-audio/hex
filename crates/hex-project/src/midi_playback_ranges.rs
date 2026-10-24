use super::*;
use std::ops::Range;

impl MidiSourcePlayback {
    /// Source-beat intervals visited by a finite half-open elapsed clip window.
    /// Retained playback offset, first pass, loop wrap and one-shot silence have
    /// the same law as `source_phase_at_elapsed`. Repeated visits merge into at
    /// most two source intervals; this query never enumerates notes/occurrences.
    /// No allocation. Query endpoints clamp to the finite arrangement duration.
    pub fn source_ranges_at_elapsed(
        self,
        range: Range<BeatTime>,
    ) -> Result<[Option<Range<BeatTime>>; 2], Invalid> {
        if range.start > range.end {
            return Err(Invalid("midi selection range is reversed"));
        }
        let first = range.start.raw().min(self.arrangement_duration.raw());
        let last = range.end.raw().min(self.arrangement_duration.raw());
        let (first, last) = if self.reversed {
            (
                self.arrangement_duration.raw() - last,
                self.arrangement_duration.raw() - first,
            )
        } else {
            (first, last)
        };
        let ranges = crate::source_ranges::source_ranges(
            u128::from(self.playback_offset.raw()) + u128::from(first),
            u128::from(self.playback_offset.raw()) + u128::from(last),
            self.source_start.raw(),
            self.initial_end.raw(),
            self.loop_range.map(|(start, end)| (start.raw(), end.raw())),
        )?;
        Ok(ranges.map(|slot| {
            slot.map(|range| BeatTime::from_raw(range.start)..BeatTime::from_raw(range.end))
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    fn b(raw: u64) -> BeatTime {
        BeatTime::from_raw(raw)
    }

    #[test]
    fn source_ranges_match_visited_phases_across_offsets_loops_and_silence() {
        for (source_start, looping) in [(0, None), (3, None), (0, Some((2, 6))), (4, Some((2, 6)))]
        {
            for offset in [0, 1, 5, 9] {
                let playback = MidiSourcePlayback::new(
                    b(8),
                    b(source_start),
                    looping.map(|(start, end)| (b(start), b(end))),
                    b(20),
                )
                .unwrap()
                .with_playback_offset(b(offset))
                .unwrap();
                for range in [0..0, 0..1, 1..4, 3..9, 0..20, 17..23, 24..30] {
                    let ranges = playback
                        .source_ranges_at_elapsed(b(range.start)..b(range.end))
                        .unwrap();
                    let actual = ranges
                        .into_iter()
                        .flatten()
                        .flat_map(|r| r.start.raw()..r.end.raw())
                        .collect::<BTreeSet<_>>();
                    let expected = (range.start.min(20)..range.end.min(20))
                        .filter_map(|elapsed| {
                            playback
                                .source_phase_at_elapsed(b(elapsed))
                                .map(BeatTime::raw)
                        })
                        .collect::<BTreeSet<_>>();
                    assert_eq!(
                        actual, expected,
                        "start={source_start} loop={looping:?} offset={offset} range={range:?}"
                    );
                }
            }
        }
    }
    #[test]
    fn source_ranges_refuse_reversal_and_bound_large_finite_loops() {
        let playback =
            MidiSourcePlayback::new(b(u64::MAX), b(1), Some((b(2), b(6))), b(u64::MAX)).unwrap();
        assert!(playback.source_ranges_at_elapsed(b(4)..b(2)).is_err());
        assert_eq!(
            playback
                .source_ranges_at_elapsed(b(0)..b(u64::MAX))
                .unwrap(),
            [Some(b(1)..b(6)), None]
        );
    }
}
