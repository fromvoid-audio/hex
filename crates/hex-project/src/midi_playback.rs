//! Validated, allocation-free projection from MIDI source beats to a finite
//! arrangement timeline. Canonical clips persist independent source/start/loop
//! metadata; ongoing Session templates require a separate scheduling contract.
use crate::{BeatTime, Invalid};

/// Beat-based source and arrangement mapping for one MIDI playback region.
///
/// The source interval is `[0, source_length)`. `source_start` selects where
/// the arrangement begins reading. When a loop exists, its source interval is
/// `[loop_start, loop_end)` and the first pass runs from `source_start` to the
/// loop end. Notes before `source_start` are not chased.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiSourcePlayback {
    source_length: BeatTime,
    source_start: BeatTime,
    loop_range: Option<(BeatTime, BeatTime)>,
    arrangement_duration: BeatTime,
}

impl MidiSourcePlayback {
    pub fn new(
        source_length: BeatTime,
        source_start: BeatTime,
        loop_range: Option<(BeatTime, BeatTime)>,
        arrangement_duration: BeatTime,
    ) -> Result<Self, Invalid> {
        if source_length == BeatTime::ZERO || arrangement_duration == BeatTime::ZERO {
            return Err(Invalid(
                "midi source and arrangement durations must be positive",
            ));
        }
        if let Some((loop_start, loop_end)) = loop_range {
            if loop_start >= loop_end || loop_end > source_length {
                return Err(Invalid("midi source loop must be inside its source"));
            }
            if source_start >= loop_end {
                return Err(Invalid("midi source start must precede its loop end"));
            }
        } else if source_start >= source_length {
            return Err(Invalid("midi source start must be inside its source"));
        }
        Ok(Self {
            source_length,
            source_start,
            loop_range,
            arrangement_duration,
        })
    }

    /// Construct the current canonical one-shot mapping for a clip.
    pub fn one_shot(length: BeatTime) -> Result<Self, Invalid> {
        Self::new(length, BeatTime::ZERO, None, length)
    }

    pub fn source_length(self) -> BeatTime {
        self.source_length
    }

    pub fn source_start(self) -> BeatTime {
        self.source_start
    }

    pub fn loop_range(self) -> Option<(BeatTime, BeatTime)> {
        self.loop_range
    }

    pub fn arrangement_duration(self) -> BeatTime {
        self.arrangement_duration
    }

    /// Build a constant-time plan for a validated source-local note.
    pub fn note_occurrences(
        self,
        note_start: BeatTime,
        note_duration: BeatTime,
    ) -> Result<MidiNoteOccurrencePlan, Invalid> {
        if note_duration == BeatTime::ZERO {
            return Err(Invalid("midi note duration must be positive"));
        }
        let note_end = note_start
            .raw()
            .checked_add(note_duration.raw())
            .ok_or(Invalid("midi note end overflow"))?;
        if note_end > self.source_length.raw() {
            return Err(Invalid("midi note exceeds its source"));
        }

        let timeline_limit = self.arrangement_duration.raw();
        let mut initial = None;
        let mut repeats = 0_u64;
        let mut first_repeat = 0_u64;
        let mut period = 0_u64;
        let mut repeat_source_gate = 0_u64;

        if let Some((loop_start, loop_end)) = self.loop_range {
            let loop_start_raw = loop_start.raw();
            let loop_end_raw = loop_end.raw();
            let first_pass = loop_end_raw - self.source_start.raw();
            period = loop_end_raw - loop_start_raw;

            if note_start.raw() >= self.source_start.raw() && note_start.raw() < loop_end_raw {
                let timeline_start = note_start.raw() - self.source_start.raw();
                if timeline_start < timeline_limit {
                    let gate = note_duration
                        .raw()
                        .min(loop_end_raw - note_start.raw())
                        .min(timeline_limit - timeline_start);
                    if gate > 0 {
                        initial = Some(MidiNoteOccurrence {
                            iteration: 0,
                            timeline_start: BeatTime::from_raw(timeline_start),
                            gate_duration: BeatTime::from_raw(gate),
                        });
                    }
                }
            }

            if note_start.raw() >= loop_start_raw && note_start.raw() < loop_end_raw {
                let source_delta = note_start.raw() - loop_start_raw;
                repeat_source_gate = note_duration.raw().min(loop_end_raw - note_start.raw());
                // Compare against the available timeline before adding. This
                // avoids overflowing a mathematically out-of-range onset.
                let available_after_first = timeline_limit.saturating_sub(first_pass);
                if source_delta < available_after_first {
                    first_repeat = first_pass + source_delta;
                    repeats = 1 + (timeline_limit - 1 - first_repeat) / period;
                }
            }
        } else if note_start.raw() >= self.source_start.raw()
            && note_start.raw() < self.source_length.raw()
        {
            let timeline_start = note_start.raw() - self.source_start.raw();
            if timeline_start < timeline_limit {
                let gate = note_duration
                    .raw()
                    .min(self.source_length.raw() - note_start.raw())
                    .min(timeline_limit - timeline_start);
                if gate > 0 {
                    initial = Some(MidiNoteOccurrence {
                        iteration: 0,
                        timeline_start: BeatTime::from_raw(timeline_start),
                        gate_duration: BeatTime::from_raw(gate),
                    });
                }
            }
        }

        Ok(MidiNoteOccurrencePlan {
            initial,
            first_repeat,
            period,
            repeats,
            repeat_source_gate,
            timeline_limit,
        })
    }
}

/// One note occurrence on the arrangement timeline. Its gate ends at the
/// source seam or finite arrangement end when either cuts the authored gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiNoteOccurrence {
    iteration: u64,
    timeline_start: BeatTime,
    gate_duration: BeatTime,
}

impl MidiNoteOccurrence {
    pub fn iteration(self) -> u64 {
        self.iteration
    }

    pub fn timeline_start(self) -> BeatTime {
        self.timeline_start
    }

    pub fn gate_duration(self) -> BeatTime {
        self.gate_duration
    }
}

/// O(1)-sized arithmetic plan for a single note's finite occurrences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiNoteOccurrencePlan {
    initial: Option<MidiNoteOccurrence>,
    first_repeat: u64,
    period: u64,
    repeats: u64,
    repeat_source_gate: u64,
    timeline_limit: u64,
}

impl MidiNoteOccurrencePlan {
    /// The constructor spaces all occurrences at distinct raw timeline onsets
    /// inside `[0, arrangement_duration)`, so this sum is at most `u64::MAX`.
    pub fn count(self) -> u64 {
        self.initial.map_or(0, |_| 1) + self.repeats
    }

    pub fn iter(self) -> MidiNoteOccurrenceIter {
        MidiNoteOccurrenceIter {
            plan: self,
            ordinal: 0,
        }
    }
}

/// Bounded-by-the-plan iterator. Each call performs constant work, so callers
/// must check aggregate occurrence counts before iterating untrusted regions.
pub struct MidiNoteOccurrenceIter {
    plan: MidiNoteOccurrencePlan,
    ordinal: u64,
}

impl Iterator for MidiNoteOccurrenceIter {
    type Item = MidiNoteOccurrence;

    fn next(&mut self) -> Option<Self::Item> {
        let initial_offset = u64::from(self.plan.initial.is_some());
        if let Some(initial) = self.plan.initial {
            if self.ordinal == 0 {
                self.ordinal = 1;
                return Some(initial);
            }
        }
        let repeat_index = self.ordinal.checked_sub(initial_offset)?;
        if repeat_index >= self.plan.repeats {
            return None;
        }
        let timeline_start = self
            .plan
            .first_repeat
            .checked_add(repeat_index.checked_mul(self.plan.period)?)?;
        let gate_duration = self
            .plan
            .repeat_source_gate
            .min(self.plan.timeline_limit - timeline_start);
        self.ordinal = self.ordinal.checked_add(1)?;
        Some(MidiNoteOccurrence {
            iteration: repeat_index.checked_add(1)?,
            timeline_start: BeatTime::from_raw(timeline_start),
            gate_duration: BeatTime::from_raw(gate_duration),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beat(raw: u64) -> BeatTime {
        BeatTime::from_raw(raw)
    }

    fn plan(
        source_length: u64,
        source_start: u64,
        loop_range: Option<(u64, u64)>,
        arrangement_duration: u64,
        note_start: u64,
        note_duration: u64,
    ) -> MidiNoteOccurrencePlan {
        MidiSourcePlayback::new(
            beat(source_length),
            beat(source_start),
            loop_range.map(|(start, end)| (beat(start), beat(end))),
            beat(arrangement_duration),
        )
        .unwrap()
        .note_occurrences(beat(note_start), beat(note_duration))
        .unwrap()
    }

    fn tuples(plan: MidiNoteOccurrencePlan) -> Vec<(u64, u64, u64)> {
        plan.iter()
            .map(|occurrence| {
                (
                    occurrence.iteration(),
                    occurrence.timeline_start().raw(),
                    occurrence.gate_duration().raw(),
                )
            })
            .collect()
    }

    #[test]
    fn bounded_once_keeps_authored_notes_but_stops_onsets_and_gates_at_selected_end() {
        // A captured 2..6 cycle was switched off. The retained 0..12 source
        // includes hidden notes; the selected remaining cycle must end at 6.
        let bounded = MidiSourcePlayback::new(beat(12), beat(2), None, beat(20)).unwrap();
        assert_eq!(bounded.source_length(), beat(12));
        assert_eq!(tuples(bounded.note_occurrences(beat(4), beat(5)).unwrap()), vec![(0, 2, 2)]);
        assert_eq!(bounded.note_occurrences(beat(6), beat(1)).unwrap().count(), 0);
        assert_eq!(bounded.note_occurrences(beat(10), beat(2)).unwrap().count(), 0);
        // Full authored bounds still validate the retained note, never the
        // smaller audible window; true source overflow must still refuse.
        assert!(bounded.note_occurrences(beat(11), beat(2)).is_err());
        let ordinary = MidiSourcePlayback::new(beat(12), beat(2), None, beat(20)).unwrap();
        assert_eq!(tuples(ordinary.note_occurrences(beat(10), beat(2)).unwrap()), vec![(0, 8, 2)]);
    }

    #[test]
    fn bounded_once_tail_and_finite_outer_end_do_not_create_repeats_or_chase() {
        let bounded = MidiSourcePlayback::new(beat(12), beat(5), None, beat(20)).unwrap();
        assert_eq!(tuples(bounded.note_occurrences(beat(5), beat(6)).unwrap()), vec![(0, 0, 1)]);
        assert_eq!(bounded.note_occurrences(beat(4), beat(6)).unwrap().count(), 0);
        assert_eq!(bounded.note_occurrences(beat(7), beat(1)).unwrap().count(), 0);
        let shorter = MidiSourcePlayback::new(beat(12), beat(2), None, beat(1)).unwrap();
        assert_eq!(tuples(shorter.note_occurrences(beat(2), beat(8)).unwrap()), vec![(0, 0, 1)]);
    }

    struct Segment {
        source_start: u64,
        source_end: u64,
        timeline_start: u64,
        iteration: u64,
    }

    fn source_segment_oracle(
        segment: Segment,
        note_start: u64,
        note_duration: u64,
        arrangement_duration: u64,
        output: &mut Vec<(u64, u64, u64)>,
    ) {
        if note_start < segment.source_start
            || note_start >= segment.source_end
            || segment.timeline_start >= arrangement_duration
        {
            return;
        }
        let occurrence_start = segment.timeline_start + (note_start - segment.source_start);
        if occurrence_start >= arrangement_duration {
            return;
        }
        let gate = note_duration
            .min(segment.source_end - note_start)
            .min(arrangement_duration - occurrence_start);
        if gate > 0 {
            output.push((segment.iteration, occurrence_start, gate));
        }
    }

    fn walk_oracle(
        source_length: u64,
        source_start: u64,
        loop_range: Option<(u64, u64)>,
        arrangement_duration: u64,
        note_start: u64,
        note_duration: u64,
    ) -> Vec<(u64, u64, u64)> {
        let mut output = Vec::new();
        if let Some((loop_start, loop_end)) = loop_range {
            let first_pass = loop_end - source_start;
            source_segment_oracle(
                Segment {
                    source_start,
                    source_end: loop_end,
                    timeline_start: 0,
                    iteration: 0,
                },
                note_start,
                note_duration,
                arrangement_duration,
                &mut output,
            );
            let period = loop_end - loop_start;
            let mut timeline_start = first_pass;
            let mut iteration = 1;
            while timeline_start < arrangement_duration {
                source_segment_oracle(
                    Segment {
                        source_start: loop_start,
                        source_end: loop_end,
                        timeline_start,
                        iteration,
                    },
                    note_start,
                    note_duration,
                    arrangement_duration,
                    &mut output,
                );
                timeline_start += period;
                iteration += 1;
            }
        } else {
            source_segment_oracle(
                Segment {
                    source_start,
                    source_end: source_length,
                    timeline_start: 0,
                    iteration: 0,
                },
                note_start,
                note_duration,
                arrangement_duration,
                &mut output,
            );
        }
        output
    }

    #[test]
    fn one_shot_projection_preserves_offsets_and_truncates_finite_end() {
        assert_eq!(tuples(plan(10, 2, None, 5, 4, 4)), [(0, 2, 3)]);
        assert_eq!(tuples(plan(10, 2, None, 5, 1, 2)), []);
        assert_eq!(tuples(plan(10, 2, None, 1, 4, 4)), []);
    }

    #[test]
    fn intro_plays_once_and_notes_crossing_the_loop_seam_are_cut() {
        assert_eq!(tuples(plan(8, 0, Some((2, 6)), 11, 1, 3)), [(0, 1, 3)]);
        assert_eq!(
            tuples(plan(8, 0, Some((2, 6)), 11, 3, 5)),
            [(0, 3, 3), (1, 7, 3)]
        );
    }

    #[test]
    fn source_start_after_loop_start_skips_chase_but_keeps_repeats() {
        assert_eq!(
            tuples(plan(9, 4, Some((2, 6)), 8, 3, 2)),
            [(1, 3, 2), (2, 7, 1)]
        );
    }

    #[test]
    fn repeated_gate_is_cut_at_finite_arrangement_end() {
        assert_eq!(
            tuples(plan(8, 0, Some((2, 6)), 9, 2, 4)),
            [(0, 2, 4), (1, 6, 3)]
        );
    }

    #[test]
    fn huge_loop_count_is_constant_time_and_unrepresentable_onsets_are_skipped() {
        let huge = plan(u64::MAX, 0, Some((0, 1)), u64::MAX, 0, 1);
        assert_eq!(huge.count(), u64::MAX);
        let mut iter = huge.iter();
        assert_eq!(iter.next().map(MidiNoteOccurrence::iteration), Some(0));
        assert_eq!(iter.next().map(MidiNoteOccurrence::iteration), Some(1));

        assert_eq!(
            tuples(plan(
                u64::MAX,
                0,
                Some((0, u64::MAX - 1)),
                u64::MAX,
                u64::MAX - 2,
                1,
            )),
            [(0, u64::MAX - 2, 1)]
        );
    }

    #[test]
    fn descriptor_and_note_inputs_reject_invalid_ranges_and_overflow() {
        assert_eq!(
            MidiSourcePlayback::new(beat(8), beat(6), Some((beat(2), beat(6))), beat(4)),
            Err(Invalid("midi source start must precede its loop end"))
        );
        assert_eq!(
            MidiSourcePlayback::new(beat(8), beat(0), Some((beat(3), beat(3))), beat(4)),
            Err(Invalid("midi source loop must be inside its source"))
        );
        assert_eq!(
            MidiSourcePlayback::one_shot(beat(8))
                .unwrap()
                .note_occurrences(beat(u64::MAX), beat(1)),
            Err(Invalid("midi note end overflow"))
        );
    }

    #[test]
    fn constant_time_plans_match_independent_small_segment_walks() {
        for source_length in 1..=8 {
            for source_start in 0..source_length {
                let mut loop_ranges = vec![None];
                for loop_start in 0..source_length {
                    for loop_end in (loop_start + 1)..=source_length {
                        if source_start < loop_end {
                            loop_ranges.push(Some((loop_start, loop_end)));
                        }
                    }
                }
                for arrangement_duration in 1..=10 {
                    for note_start in 0..source_length {
                        for note_duration in 1..=(source_length - note_start) {
                            for loop_range in &loop_ranges {
                                let actual = plan(
                                    source_length,
                                    source_start,
                                    *loop_range,
                                    arrangement_duration,
                                    note_start,
                                    note_duration,
                                );
                                let expected = walk_oracle(
                                    source_length,
                                    source_start,
                                    *loop_range,
                                    arrangement_duration,
                                    note_start,
                                    note_duration,
                                );
                                assert_eq!(tuples(actual), expected);
                                assert_eq!(actual.count(), actual.iter().count() as u64);
                            }
                        }
                    }
                }
            }
        }
    }
}
