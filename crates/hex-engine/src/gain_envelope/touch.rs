//! Immutable frame-domain overlay; authored base knots are never spliced.
use crate::Invalid;
use hex_project::{GainLane, TempoMap};
use std::ops::Range;

#[derive(Clone, Debug)]
struct Span {
    start: u64,
    end: u64,
    steps: Range<usize>,
    release_from: f32,
    release_target: f32,
    release_end: u64,
}
#[derive(Clone, Copy, Debug)]
struct Step {
    frame: u64,
    gain: f32,
    from: f32,
    anchor: u64,
}
impl Step {
    fn value_at(self, frame: u64, duration: u64) -> f32 {
        let elapsed = frame.saturating_sub(self.anchor).saturating_add(1);
        if elapsed >= duration {
            return self.gain;
        }
        (f64::from(self.from)
            + (f64::from(self.gain) - f64::from(self.from)) * elapsed as f64 / duration as f64)
            as f32
    }
}
#[derive(Clone, Debug, Default)]
pub(super) struct PreparedTouch {
    spans: Vec<Span>,
    steps: Vec<Step>,
    duration: u64,
}

/// An authored target, not the evaluated intermediate gain of its ramp.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TouchTarget {
    pub span: usize,
    pub event: usize,
    pub gain: f32,
}
impl PreparedTouch {
    pub(super) fn target_at(&self, frame: u64) -> Option<TouchTarget> {
        let span_index = self
            .spans
            .partition_point(|span| span.start <= frame)
            .checked_sub(1)?;
        let span = self.spans.get(span_index)?;
        if frame >= span.end {
            return None;
        }
        let steps = self.steps.get(span.steps.clone())?;
        let event = steps
            .partition_point(|step| step.frame <= frame)
            .checked_sub(1)?;
        Some(TouchTarget {
            span: span_index,
            event: span.steps.start + event,
            gain: steps[event].gain,
        })
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
    pub(super) fn prepare(
        lane: &GainLane,
        tempo: &TempoMap,
        base_at: impl Fn(u64) -> f32,
    ) -> Result<Self, Invalid> {
        let step_capacity = lane
            .spans()
            .iter()
            .try_fold(0_usize, |n, span| n.checked_add(span.steps.len()))
            .ok_or(Invalid("prepared gain touch step count overflow"))?;
        let mut prepared = Self {
            duration: u64::from(tempo.sample_rate().div_ceil(200).max(1)),
            ..Self::default()
        };
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
            let seed_frame = start.saturating_sub(1);
            let seed = prepared
                .value_at(seed_frame)
                .unwrap_or_else(|| base_at(seed_frame));
            let first = prepared.steps.len();
            for step in &span.steps {
                let frame = tempo.beat_to_frame(step.beat).map_err(|e| Invalid(e.0))?;
                if start == end || frame >= end {
                    continue;
                }
                let step = Step {
                    frame,
                    gain: step.gain,
                    from: 0.0,
                    anchor: frame,
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
                for index in first..prepared.steps.len() {
                    let frame = prepared.steps[index].frame;
                    let previous = index
                        .checked_sub(1)
                        .filter(|&prior| prior >= first)
                        .map(|prior| prepared.steps[prior]);
                    if let Some(previous) = previous {
                        if previous.gain == prepared.steps[index].gain {
                            prepared.steps[index].from = previous.from;
                            prepared.steps[index].anchor = previous.anchor;
                        } else {
                            prepared.steps[index].from =
                                previous.value_at(frame.saturating_sub(1), prepared.duration);
                        }
                    } else {
                        prepared.steps[index].from = seed;
                    }
                }
                let release_end = end
                    .checked_add(prepared.duration)
                    .ok_or(Invalid("gain touch release frame overflow"))?;
                let release_from = prepared
                    .steps
                    .last()
                    .filter(|_| prepared.steps.len() > first)
                    .map_or(seed, |step| step.value_at(end - 1, prepared.duration));
                prepared.spans.push(Span {
                    start,
                    end,
                    steps: first..prepared.steps.len(),
                    release_from,
                    release_target: base_at(release_end - 1),
                    release_end,
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
            if frame >= span.release_end {
                return None;
            }
            return Some(
                Step {
                    frame: span.end,
                    anchor: span.end,
                    from: span.release_from,
                    gain: span.release_target,
                }
                .value_at(frame, self.duration),
            );
        }
        let steps = self.steps.get(span.steps.clone())?;
        steps
            .get(
                steps
                    .partition_point(|step| step.frame <= frame)
                    .checked_sub(1)?,
            )
            .map(|step| step.value_at(frame, self.duration))
    }
    pub(super) fn next_boundary(&self, frame: u64) -> Option<u64> {
        let next_span = self.spans.partition_point(|span| span.start <= frame);
        let start = self.spans.get(next_span).map(|span| span.start);
        let end = next_span
            .checked_sub(1)
            .and_then(|index| self.spans.get(index))
            .and_then(|span| {
                [
                    span.end,
                    span.release_end.saturating_sub(1),
                    span.release_end,
                ]
                .into_iter()
                .filter(|&at| at > frame)
                .min()
            });
        let step = self
            .steps
            .get(self.steps.partition_point(|step| step.frame <= frame))
            .map(|step| step.frame);
        let ramp_end = self
            .steps
            .partition_point(|step| step.frame <= frame)
            .checked_sub(1)
            .and_then(|index| self.steps.get(index))
            .and_then(|step| step.anchor.checked_add(self.duration.saturating_sub(1)))
            .filter(|&at| at > frame);
        [start, end, step, ramp_end].into_iter().flatten().min()
    }

    /// Value of the current linear touch segment at its exclusive end,
    /// before an event at that boundary changes ownership.
    pub(super) fn segment_end(&self, frame: u64, end: u64) -> Option<f32> {
        let index = self
            .spans
            .partition_point(|span| span.start <= frame)
            .checked_sub(1)?;
        let span = self.spans.get(index)?;
        let step = if frame >= span.end {
            if frame >= span.release_end {
                return None;
            }
            Step {
                frame: span.end,
                anchor: span.end,
                from: span.release_from,
                gain: span.release_target,
            }
        } else {
            let steps = self.steps.get(span.steps.clone())?;
            *steps.get(
                steps
                    .partition_point(|step| step.frame <= frame)
                    .checked_sub(1)?,
            )?
        };
        Some(step.value_at(end, self.duration))
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
