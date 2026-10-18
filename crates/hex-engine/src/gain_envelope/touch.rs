//! Immutable frame-domain overlay; authored base knots are never spliced.
use crate::Invalid;
use hex_project::{GainLane, TempoMap};
use std::ops::Range;

#[derive(Clone, Debug)]
struct Span {
    start: u64,
    end: u64,
    steps: Range<usize>,
}
#[derive(Clone, Copy, Debug)]
struct Step {
    frame: u64,
    gain: f32,
}
#[derive(Clone, Debug, Default)]
pub(super) struct PreparedTouch {
    spans: Vec<Span>,
    steps: Vec<Step>,
}
impl PreparedTouch {
    pub(super) fn visit_audio_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        let Self { spans, steps } = self;
        // Exhaustive fixed-leaf audit; future owned fields require traversal.
        for Span {
            start,
            end,
            steps: range,
        } in spans
        {
            let _: (&u64, &u64, &Range<usize>) = (start, end, range);
        }
        for Step { frame, gain } in steps {
            let _: (&u64, &f32) = (frame, gain);
        }
        ledger.vec(spans)?;
        ledger.vec(steps)
    }
    pub(super) fn preflight_bytes(lane: &GainLane) -> Option<usize> {
        let steps = lane
            .spans()
            .iter()
            .try_fold(0_usize, |n, span| n.checked_add(span.steps.len()))?;
        lane.spans()
            .len()
            .checked_mul(std::mem::size_of::<Span>())?
            .checked_add(steps.checked_mul(std::mem::size_of::<Step>())?)
    }
    pub(super) fn prepare(lane: &GainLane, tempo: &TempoMap) -> Result<Self, Invalid> {
        let step_capacity = lane
            .spans()
            .iter()
            .try_fold(0_usize, |n, span| n.checked_add(span.steps.len()))
            .ok_or(Invalid("prepared gain touch step count overflow"))?;
        let mut prepared = Self::default();
        prepared
            .spans
            .try_reserve_exact(lane.spans().len())
            .map_err(|_| Invalid("could not reserve gain touch spans"))?;
        prepared
            .steps
            .try_reserve_exact(step_capacity)
            .map_err(|_| Invalid("could not reserve gain touch steps"))?;
        if prepared.spans.capacity() > lane.spans().len()
            || prepared.steps.capacity() > step_capacity
        {
            return Err(Invalid("prepared gain touch allocation exceeds its bound"));
        }
        for span in lane.spans() {
            let start = tempo.beat_to_frame(span.start).map_err(|e| Invalid(e.0))?;
            let end = tempo.beat_to_frame(span.end).map_err(|e| Invalid(e.0))?;
            let first = prepared.steps.len();
            for step in &span.steps {
                let frame = tempo.beat_to_frame(step.beat).map_err(|e| Invalid(e.0))?;
                if start == end || frame >= end {
                    continue;
                }
                let step = Step {
                    frame,
                    gain: step.gain,
                };
                if prepared.steps.len() > first
                    && prepared.steps.last().is_some_and(|old| old.frame == frame)
                {
                    if let Some(last) = prepared.steps.last_mut() {
                        *last = step;
                    }
                } else {
                    prepared.steps.push(step);
                }
            }
            if start < end {
                prepared.spans.push(Span {
                    start,
                    end,
                    steps: first..prepared.steps.len(),
                });
            }
        }
        Ok(prepared)
    }
    pub(super) fn value_at(&self, frame: u64) -> Option<f32> {
        let index = self
            .spans
            .partition_point(|span| span.start <= frame)
            .checked_sub(1)?;
        let span = self.spans.get(index)?;
        if frame >= span.end {
            return None;
        }
        let steps = self.steps.get(span.steps.clone())?;
        steps
            .get(
                steps
                    .partition_point(|step| step.frame <= frame)
                    .checked_sub(1)?,
            )
            .map(|step| step.gain)
    }
    pub(super) fn next_boundary(&self, frame: u64) -> Option<u64> {
        let next_span = self.spans.partition_point(|span| span.start <= frame);
        let start = self.spans.get(next_span).map(|span| span.start);
        let end = next_span
            .checked_sub(1)
            .and_then(|index| self.spans.get(index))
            .filter(|span| span.end > frame)
            .map(|span| span.end);
        let step = self
            .steps
            .get(self.steps.partition_point(|step| step.frame <= frame))
            .map(|step| step.frame);
        [start, end, step].into_iter().flatten().min()
    }
    pub(super) fn span_count(&self) -> usize {
        self.spans.len()
    }
    pub(super) fn step_count(&self) -> usize {
        self.steps.len()
    }
    pub(super) fn allocated_bytes(&self) -> usize {
        self.spans.capacity() * std::mem::size_of::<Span>()
            + self.steps.capacity() * std::mem::size_of::<Step>()
    }
}

#[cfg(test)]
mod storage_tests {
    use super::*;
    #[test]
    fn prepared_gain_touch_empty_reserved_spans_and_steps_keep_their_actual_backings() {
        let mut touch = PreparedTouch::default();
        touch.spans.try_reserve_exact(7).unwrap();
        touch.steps.try_reserve_exact(13).unwrap();
        touch.spans.clear();
        touch.steps.clear();
        let bytes = touch.spans.capacity() * std::mem::size_of::<Span>()
            + touch.steps.capacity() * std::mem::size_of::<Step>();
        let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(
            crate::AudioStorageLedgerReservation {
                data_rows: 16,
                handle_rows: 16,
                root_rows: 4,
                max_retained_bytes: 1_000_000,
                max_construction_peak_bytes: 1_000_000,
            },
        )
        .unwrap();
        touch.visit_audio_storage(&mut ledger).unwrap();
        assert_eq!(ledger.summary().unwrap().data_bytes, bytes);
        assert_eq!(ledger.summary().unwrap().data_allocations, 2);
    }
}
