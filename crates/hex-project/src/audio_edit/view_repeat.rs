//! Constant-space audible spans within a finite repeating window.
use crate::Invalid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AudibleSpan {
    pub cycle_index: u64,
    pub elapsed_start: u64,
    pub elapsed_end: u64,
    pub source_start: u64,
    pub source_end: u64,
}

/// A cycle starts with `audible` ticks of source followed by silence up to
/// `period`. The initial cursor may be inside either portion.
#[derive(Clone, Copy)]
pub(super) struct AudibleRepeats {
    length: u64,
    period: u64,
    audible: u64,
    first_start: u64,
    first_source: u64,
    first_cycle: u64,
    next_start: Option<u64>,
    count: u64,
}
impl AudibleRepeats {
    pub fn new(length: u64, phase: u64, period: u64, audible: u64) -> Result<Self, Invalid> {
        if length == 0 || period == 0 || audible == 0 || audible > period || phase >= period {
            return Err(Invalid("invalid audible repeat window"));
        }
        let in_audio = phase < audible;
        let first_start = if in_audio { 0 } else { period - phase };
        let first_source = if in_audio { phase } else { 0 };
        let first_cycle = u64::from(!in_audio);
        let next_start = (period - phase).checked_add(if in_audio { 0 } else { period });
        let count = if first_start >= length {
            0
        } else if let Some(next) = next_start.filter(|&next| next < length) {
            2_u64
                .checked_add((length - 1 - next) / period)
                .ok_or(Invalid("audible repeat count overflow"))?
        } else {
            1
        };
        Ok(Self {
            length,
            period,
            audible,
            first_start,
            first_source,
            first_cycle,
            next_start,
            count,
        })
    }
    pub fn count(self) -> u64 {
        self.count
    }
    pub fn span(self, index: u64) -> Option<AudibleSpan> {
        if index >= self.count {
            return None;
        }
        let (elapsed_start, source_start, cycle_index) = if index == 0 {
            (self.first_start, self.first_source, self.first_cycle)
        } else {
            (
                self.next_start?
                    .checked_add((index - 1).checked_mul(self.period)?)?,
                0,
                self.first_cycle.checked_add(index)?,
            )
        };
        let frames = (self.audible - source_start).min(self.length - elapsed_start);
        Some(AudibleSpan {
            cycle_index,
            elapsed_start,
            elapsed_end: elapsed_start.checked_add(frames)?,
            source_start,
            source_end: source_start.checked_add(frames)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finite_repeats_expose_source_and_silent_intervals() {
        let repeats = AudibleRepeats::new(23, 0, 10, 6).unwrap();
        assert_eq!(repeats.count(), 3);
        assert_eq!(
            repeats.span(0),
            Some(AudibleSpan {
                cycle_index: 0,
                elapsed_start: 0,
                elapsed_end: 6,
                source_start: 0,
                source_end: 6
            })
        );
        assert_eq!(
            repeats.span(1),
            Some(AudibleSpan {
                cycle_index: 1,
                elapsed_start: 10,
                elapsed_end: 16,
                source_start: 0,
                source_end: 6
            })
        );
        assert_eq!(
            repeats.span(2),
            Some(AudibleSpan {
                cycle_index: 2,
                elapsed_start: 20,
                elapsed_end: 23,
                source_start: 0,
                source_end: 3
            })
        );
        assert_eq!(repeats.span(3), None);
    }

    #[test]
    fn initial_phase_can_begin_in_source_or_silence() {
        let source = AudibleRepeats::new(18, 4, 10, 6).unwrap();
        assert_eq!(
            source.span(0),
            Some(AudibleSpan {
                cycle_index: 0,
                elapsed_start: 0,
                elapsed_end: 2,
                source_start: 4,
                source_end: 6
            })
        );
        assert_eq!(source.span(1).unwrap().elapsed_start, 6);
        assert_eq!(source.span(2).unwrap().elapsed_start, 16);
        let silence = AudibleRepeats::new(15, 8, 10, 6).unwrap();
        assert_eq!(silence.count(), 2);
        assert_eq!(
            silence.span(0),
            Some(AudibleSpan {
                cycle_index: 1,
                elapsed_start: 2,
                elapsed_end: 8,
                source_start: 0,
                source_end: 6
            })
        );
        assert_eq!(silence.span(1).unwrap().elapsed_start, 12);
        assert_eq!(AudibleRepeats::new(2, 8, 10, 6).unwrap().count(), 0);
    }

    #[test]
    fn spans_match_small_per_tick_oracle() {
        for period in 1..=8 {
            for audible in 1..=period {
                for phase in 0..period {
                    for length in 1..=24 {
                        let repeats = AudibleRepeats::new(length, phase, period, audible).unwrap();
                        let mut projected = vec![None; length as usize];
                        for index in 0..repeats.count() {
                            let span = repeats.span(index).unwrap();
                            for elapsed in span.elapsed_start..span.elapsed_end {
                                let source = span.source_start + elapsed - span.elapsed_start;
                                assert!(projected[elapsed as usize].replace(source).is_none());
                            }
                        }
                        for elapsed in 0..length {
                            let source = (phase + elapsed) % period;
                            assert_eq!(
                                projected[elapsed as usize],
                                (source < audible).then_some(source)
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn rejects_invalid_windows_and_bounds_extreme_coordinates() {
        for args in [
            (0, 0, 1, 1),
            (1, 0, 0, 0),
            (1, 1, 1, 1),
            (1, 0, 1, 2),
            (1, 0, 1, 0),
        ] {
            assert!(AudibleRepeats::new(args.0, args.1, args.2, args.3).is_err());
        }
        let huge = AudibleRepeats::new(u64::MAX, u64::MAX - 2, u64::MAX, 1).unwrap();
        assert_eq!(huge.count(), 1);
        assert_eq!(huge.span(0).unwrap().elapsed_start, 2);
        assert_eq!(huge.span(1), None);
    }
}
