//! Validated, allocation-free projection from MIDI source beats to a finite
//! arrangement timeline. Canonical clips persist independent source/start/loop
//! metadata; ongoing Session templates require a separate scheduling contract.
use crate::{BeatTime, Invalid};
#[path = "midi_playback_ranges.rs"]
mod ranges;

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
    initial_end: BeatTime,
    loop_range: Option<(BeatTime, BeatTime)>,
    arrangement_duration: BeatTime,
    playback_offset: BeatTime,
    reversed: bool,
}

impl MidiSourcePlayback {
    pub fn new(
        source_length: BeatTime,
        source_start: BeatTime,
        loop_range: Option<(BeatTime, BeatTime)>,
        arrangement_duration: BeatTime,
    ) -> Result<Self, Invalid> {
        Self::new_with_initial_end(
            source_length,
            source_start,
            loop_range,
            arrangement_duration,
            loop_range.map_or(source_length, |(_, end)| end),
        )
    }

    /// Retain the full authored source for validation while limiting the first
    /// audible pass. With no loop this permits a captured cycle to play once
    /// then become silent, without deleting notes outside that cycle.
    /// Repeating mappings require the initial end to equal their loop end.
    pub fn new_with_initial_end(
        source_length: BeatTime,
        source_start: BeatTime,
        loop_range: Option<(BeatTime, BeatTime)>,
        arrangement_duration: BeatTime,
        initial_end: BeatTime,
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
        if initial_end > source_length || source_start >= initial_end {
            return Err(Invalid(
                "midi initial playback end must follow its start inside the source",
            ));
        }
        if loop_range.is_some_and(|(_, end)| end != initial_end) {
            return Err(Invalid(
                "midi repeating initial end must equal its loop end",
            ));
        }
        Ok(Self {
            source_length,
            source_start,
            initial_end,
            loop_range,
            arrangement_duration,
            playback_offset: BeatTime::ZERO,
            reversed: false,
        })
    }

    /// Select a finite elapsed window without rewriting the authored source law.
    /// Crossing gates are chased at local zero with only their remaining duration.
    pub fn with_playback_offset(mut self, offset: BeatTime) -> Result<Self, Invalid> {
        offset
            .raw()
            .checked_add(self.arrangement_duration.raw())
            .ok_or(Invalid("midi playback window end overflow"))?;
        self.playback_offset = offset;
        Ok(self)
    }
    /// Read a directed finite window of the unchanged first-pass/loop law.
    /// Ongoing Session templates do not use this finite-only operation.
    pub fn with_content_window(mut self, window: crate::ContentWindow) -> Result<Self, Invalid> {
        if window.length()? != self.arrangement_duration.raw() {
            return Err(Invalid(
                "midi content window differs from arrangement duration",
            ));
        }
        self.playback_offset = BeatTime::from_raw(window.start);
        self.reversed = window.reversed;
        Ok(self)
    }
    /// Internal normalization for owners materializing a finite directed view.
    /// Preserves source identity/clock geometry; no notes are rewritten here.
    pub(crate) fn unreversed(mut self) -> Option<Self> {
        if !self.reversed {
            return None;
        }
        self.reversed = false;
        Some(self)
    }
    pub fn playback_offset(self) -> BeatTime {
        self.playback_offset
    }
    /// Direction of the finite retained read window; ongoing Console playback
    /// remains forward. This query does not rewrite or materialize note data.
    pub fn playback_reversed(self) -> bool {
        self.reversed
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

    pub fn initial_end(self) -> BeatTime {
        self.initial_end
    }

    pub fn loop_range(self) -> Option<(BeatTime, BeatTime)> {
        self.loop_range
    }

    pub fn arrangement_duration(self) -> BeatTime {
        self.arrangement_duration
    }

    /// Internal loop seams in the finite audible order; the first-pass
    /// introduction is not repeated when it becomes a reversed tail.
    pub fn repeat_boundaries(self) -> Result<Option<crate::RepeatBoundaryPlan>, Invalid> {
        let Some((start, end)) = self.loop_range else {
            return Ok(None);
        };
        let end_of_window = self
            .playback_offset
            .raw()
            .checked_add(self.arrangement_duration.raw())
            .ok_or(Invalid("midi repeat window overflow"))?;
        crate::ContentWindow::new(self.playback_offset.raw(), end_of_window, self.reversed)?
            .repeat_boundaries(end.raw() - self.source_start.raw(), end.raw() - start.raw())
    }
    /// Source phase at an elapsed clip beat, independent of note content. An
    /// ongoing Session loop can repeat beyond `arrangement_duration`; finite
    /// placement consumers must check their placement duration separately.
    /// Ongoing loops reduce retained offsets and elapsed time without overflowing.
    /// One-shot sources return `None` at their exclusive end. No note is chased.
    pub fn source_phase_at_elapsed(self, elapsed: BeatTime) -> Option<BeatTime> {
        let elapsed = if self.reversed {
            BeatTime::from_raw(
                self.arrangement_duration
                    .raw()
                    .checked_sub(elapsed.raw())?
                    .checked_sub(1)?,
            )
        } else {
            elapsed
        };
        let raw = match self.playback_offset.raw().checked_add(elapsed.raw()) {
            Some(raw) => raw,
            None => {
                // Only ongoing queries can exceed the validated finite window.
                // Retain the ordinary 64-bit path; normalize this larger loop
                // clock before narrowing back to a representable source phase.
                let (start, end) = self.loop_range?;
                let raw = u128::from(self.playback_offset.raw()) + u128::from(elapsed.raw());
                let first_pass = u128::from(end.raw() - self.source_start.raw());
                let phase = u128::from(start.raw())
                    + (raw - first_pass) % u128::from(end.raw() - start.raw());
                return u64::try_from(phase).ok().map(BeatTime::from_raw);
            }
        };
        if let Some((start, end)) = self.loop_range {
            let first_pass = end.raw() - self.source_start.raw();
            let phase = if raw < first_pass {
                self.source_start.raw().checked_add(raw)?
            } else {
                start
                    .raw()
                    .checked_add((raw - first_pass) % (end.raw() - start.raw()))?
            };
            Some(BeatTime::from_raw(phase))
        } else {
            self.source_start
                .raw()
                .checked_add(raw)
                .filter(|phase| *phase < self.initial_end.raw())
                .map(BeatTime::from_raw)
        }
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

        let window_start = self.playback_offset.raw();
        let timeline_limit = window_start
            .checked_add(self.arrangement_duration.raw())
            .ok_or(Invalid("midi playback window end overflow"))?;
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
            && note_start.raw() < self.initial_end.raw()
        {
            let timeline_start = note_start.raw() - self.source_start.raw();
            if timeline_start < timeline_limit {
                let gate = note_duration
                    .raw()
                    .min(self.initial_end.raw() - note_start.raw())
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

        initial = initial.and_then(|occurrence| {
            let start = occurrence.timeline_start.raw();
            let end = start.checked_add(occurrence.gate_duration.raw())?;
            (end > window_start).then(|| MidiNoteOccurrence {
                iteration: 0,
                timeline_start: BeatTime::from_raw(start.saturating_sub(window_start)),
                gate_duration: BeatTime::from_raw(end - start.max(window_start)),
            })
        });
        let mut repeat_iteration_offset = 0;
        if repeats != 0
            && first_repeat <= window_start
            && repeat_source_gate <= window_start - first_repeat
        {
            let skipped = (window_start - first_repeat - repeat_source_gate) / period + 1;
            if skipped >= repeats {
                repeats = 0;
            } else {
                first_repeat = first_repeat
                    .checked_add(
                        skipped
                            .checked_mul(period)
                            .ok_or(Invalid("midi playback repeat overflow"))?,
                    )
                    .ok_or(Invalid("midi playback repeat overflow"))?;
                repeats -= skipped;
                repeat_iteration_offset = skipped;
            }
        }
        Ok(MidiNoteOccurrencePlan {
            reversed: self.reversed,
            duration: self.arrangement_duration.raw(),
            window_start,
            repeat_iteration_offset,
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
    reversed: bool,
    duration: u64,
    window_start: u64,
    repeat_iteration_offset: u64,
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

    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        self.ordinal = self.ordinal.checked_add(u64::try_from(n).ok()?)?;
        self.next()
    }

    fn next(&mut self) -> Option<Self::Item> {
        if self.ordinal >= self.plan.count() {
            return None;
        }
        let ordinal = if self.plan.reversed {
            self.plan.count() - 1 - self.ordinal
        } else {
            self.ordinal
        };
        self.ordinal = self.ordinal.checked_add(1)?;
        let initial_offset = u64::from(self.plan.initial.is_some());
        let mut occurrence = if ordinal < initial_offset {
            self.plan.initial?
        } else {
            let repeat_index = ordinal.checked_sub(initial_offset)?;
            let start = self
                .plan
                .first_repeat
                .checked_add(repeat_index.checked_mul(self.plan.period)?)?;
            let end = start.checked_add(
                self.plan
                    .repeat_source_gate
                    .min(self.plan.timeline_limit - start),
            )?;
            MidiNoteOccurrence {
                iteration: self
                    .plan
                    .repeat_iteration_offset
                    .checked_add(repeat_index)?
                    .checked_add(1)?,
                timeline_start: BeatTime::from_raw(start.saturating_sub(self.plan.window_start)),
                gate_duration: BeatTime::from_raw(
                    end.checked_sub(start.max(self.plan.window_start))?,
                ),
            }
        };
        if self.plan.reversed {
            occurrence.timeline_start = BeatTime::from_raw(
                self.plan.duration.checked_sub(
                    occurrence
                        .timeline_start
                        .raw()
                        .checked_add(occurrence.gate_duration.raw())?,
                )?,
            );
        }
        Some(occurrence)
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
    fn directed_finite_midi_reverses_intro_repeats_and_partial_gates_without_new_ids() {
        let forward =
            MidiSourcePlayback::new(beat(12), beat(1), Some((beat(3), beat(9))), beat(17))
                .unwrap()
                .with_playback_offset(beat(4))
                .unwrap();
        let reversed = forward
            .with_content_window(crate::ContentWindow::new(4, 21, true).unwrap())
            .unwrap();
        for (start, gate) in [(1, 2), (3, 4), (7, 4), (10, 2)] {
            let original = forward.note_occurrences(beat(start), beat(gate)).unwrap();
            let actual = reversed.note_occurrences(beat(start), beat(gate)).unwrap();
            assert_eq!(original.count(), actual.count());
            let mut oracle = tuples(original)
                .into_iter()
                .map(|(iteration, start, gate)| (iteration, 17 - start - gate, gate))
                .collect::<Vec<_>>();
            oracle.reverse();
            assert_eq!(tuples(actual), oracle);
        }
        for elapsed in 0..17 {
            assert_eq!(
                reversed.source_phase_at_elapsed(beat(elapsed)),
                forward.source_phase_at_elapsed(beat(16 - elapsed))
            );
        }
        assert_eq!(
            reversed.source_ranges_at_elapsed(beat(2)..beat(8)).unwrap(),
            forward.source_ranges_at_elapsed(beat(9)..beat(15)).unwrap()
        );
        assert!(
            reversed
                .with_content_window(crate::ContentWindow::new(0, 1, true).unwrap())
                .is_err()
        );
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
    fn source_phase_preserves_intro_loop_seam_and_exclusive_one_shot_end() {
        let b = BeatTime::from_raw;
        let one = MidiSourcePlayback::new(b(10), b(3), None, b(20)).unwrap();
        assert_eq!(one.source_phase_at_elapsed(b(0)), Some(b(3)));
        assert_eq!(one.source_phase_at_elapsed(b(6)), Some(b(9)));
        assert_eq!(one.source_phase_at_elapsed(b(7)), None);
        assert_eq!(one.source_phase_at_elapsed(b(u64::MAX)), None);
        let looped = MidiSourcePlayback::new(b(10), b(1), Some((b(3), b(7))), b(2)).unwrap();
        let actual: Vec<_> = (0..=11)
            .map(|e| looped.source_phase_at_elapsed(b(e)).unwrap().raw())
            .collect();
        assert_eq!(actual, [1, 2, 3, 4, 5, 6, 3, 4, 5, 6, 3, 4]);
        assert_eq!(looped.source_phase_at_elapsed(b(u64::MAX)), Some(b(4)));
        let inside = MidiSourcePlayback::new(b(10), b(5), Some((b(3), b(7))), b(2)).unwrap();
        assert_eq!(inside.source_phase_at_elapsed(b(1)), Some(b(6)));
        assert_eq!(inside.source_phase_at_elapsed(b(2)), Some(b(3)));
    }

    #[test]
    fn ongoing_loop_phase_retains_large_playback_windows_without_sum_overflow() {
        let b = BeatTime::from_raw;
        let playback = MidiSourcePlayback::new(b(10), b(0), Some((b(2), b(5))), b(1))
            .unwrap()
            .with_playback_offset(b(u64::MAX - 1))
            .unwrap();
        assert_eq!(playback.source_phase_at_elapsed(b(0)), Some(b(2)));
        assert_eq!(playback.source_phase_at_elapsed(b(5)), Some(b(4)));
        assert_eq!(playback.source_phase_at_elapsed(b(u64::MAX)), Some(b(2)));
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
