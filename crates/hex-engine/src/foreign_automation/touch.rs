//! Control-prepared half-open overlays. The original knots are never split or
//! rewritten: uncovered samples retain the original interpolation anchors.
//! Cuts bound callback work and participate in the ordinary quantum admission.
use super::{Invalid, Knot, Lane, QuantumProfile, point_bound_frames};
use hex_lattice_contract::ForeignParameterPoint;
use hex_project::{DeviceAutomationTouchSpan, TempoMap};

#[derive(Debug)]
pub(super) struct Overlay {
    spans: Vec<Span>,
    cuts: Vec<u64>,
}

#[derive(Debug)]
struct Span {
    start: u64,
    end: u64,
    steps: Vec<Knot>,
}

impl Overlay {
    pub(super) fn prepare(
        source: &[DeviceAutomationTouchSpan],
        base: &[Knot],
        quantization: u32,
        tempo: &TempoMap,
    ) -> Result<Option<Self>, Invalid> {
        if source.is_empty() {
            return Ok(None);
        }
        if source.len() > hex_project::MAX_DEVICE_AUTOMATION_TOUCH_SPANS {
            return Err(Invalid("foreign touch span budget exceeded"));
        }
        let mut spans = Vec::<Span>::new();
        spans
            .try_reserve_exact(source.len())
            .map_err(|_| Invalid("could not reserve foreign touch spans"))?;
        let mut count = 0usize;
        for span in source {
            count = count
                .checked_add(span.steps.len())
                .filter(|&n| n <= hex_project::MAX_DEVICE_AUTOMATION_POINTS_PER_LANE)
                .ok_or(Invalid("foreign touch point budget exceeded"))?;
            let start = tempo
                .beat_to_frame(span.start)
                .map_err(|error| Invalid(error.0))?;
            let end = tempo
                .beat_to_frame(span.end)
                .map_err(|error| Invalid(error.0))?;
            if start >= end || spans.last().is_some_and(|previous| previous.end > start) {
                return Err(Invalid("foreign touch spans overlap or collapse in frames"));
            }
            let mut steps = Vec::<Knot>::new();
            steps
                .try_reserve_exact(span.steps.len())
                .map_err(|_| Invalid("could not reserve foreign touch steps"))?;
            for step in &span.steps {
                let frame = tempo
                    .beat_to_frame(step.beat)
                    .map_err(|error| Invalid(error.0))?;
                if !(start..end).contains(&frame)
                    || steps.last().is_some_and(|previous| previous.frame >= frame)
                {
                    return Err(Invalid(
                        "foreign touch steps do not advance within their span",
                    ));
                }
                let value = f64::from(step.value.get());
                let value = if quantization == 0 {
                    value
                } else {
                    let scale = f64::from(quantization);
                    (value * scale).round() / scale
                };
                steps.push(Knot {
                    frame,
                    value,
                    linear: false,
                });
            }
            if steps.first().is_none_or(|first| first.frame != start) {
                return Err(Invalid("foreign touch span has no opening value"));
            }
            spans.push(Span { start, end, steps });
        }
        let capacity = base
            .len()
            .checked_add(count)
            .and_then(|n| n.checked_add(spans.len()))
            .ok_or(Invalid("foreign touch cut budget overflow"))?;
        let mut overlay = Self {
            spans,
            cuts: Vec::new(),
        };
        overlay
            .cuts
            .try_reserve_exact(capacity)
            .map_err(|_| Invalid("could not reserve foreign touch cuts"))?;
        for knot in base {
            if overlay.value(knot.frame).is_none() {
                overlay.cuts.push(knot.frame);
            }
        }
        for span in &overlay.spans {
            overlay
                .cuts
                .extend(span.steps.iter().map(|step| step.frame));
            overlay.cuts.push(span.end);
        }
        overlay.cuts.sort_unstable();
        overlay.cuts.dedup();
        Ok(Some(overlay))
    }

    pub(super) fn backing_bytes(&self) -> Option<usize> {
        let bytes = self
            .spans
            .capacity()
            .checked_mul(std::mem::size_of::<Span>())?
            .checked_add(
                self.cuts
                    .capacity()
                    .checked_mul(std::mem::size_of::<u64>())?,
            )?;
        self.spans.iter().try_fold(bytes, |bytes, span| {
            bytes.checked_add(
                span.steps
                    .capacity()
                    .checked_mul(std::mem::size_of::<Knot>())?,
            )
        })
    }

    pub(super) fn point_bound(&self, profile: QuantumProfile) -> usize {
        point_bound_frames(self.cuts.iter().copied(), profile)
    }

    fn value(&self, frame: u64) -> Option<f64> {
        let index = self
            .spans
            .partition_point(|span| span.start <= frame)
            .checked_sub(1)?;
        let span = self.spans.get(index).filter(|span| frame < span.end)?;
        let index = span
            .steps
            .partition_point(|step| step.frame <= frame)
            .checked_sub(1)?;
        span.steps.get(index).map(|step| step.value)
    }

    fn sample(&self, lane: &Lane, frame: u64) -> f64 {
        self.value(frame).unwrap_or_else(|| {
            lane.value(
                frame,
                lane.knots.partition_point(|knot| knot.frame <= frame),
            )
        })
    }

    pub(super) fn render_run(
        &self,
        lane: &Lane,
        start: usize,
        end: usize,
        origin: u64,
        output: &mut [ForeignParameterPoint],
        count: &mut usize,
    ) -> Result<(), Invalid> {
        let mut offset = start;
        let mut frame = origin;
        let mut next = self.cuts.partition_point(|&cut| cut <= frame);
        while offset < end {
            let width = self.cuts.get(next).map_or(end - offset, |cut| {
                (end - offset).min(usize::try_from(*cut - frame).unwrap_or(usize::MAX))
            });
            let last = frame
                .checked_add((width - 1) as u64)
                .ok_or(Invalid("foreign touch origin exceeds frame range"))?;
            lane.emit(offset, self.sample(lane, frame), output, count)?;
            if width > 1 {
                lane.emit(offset + width - 1, self.sample(lane, last), output, count)?;
            }
            offset += width;
            if offset < end {
                frame = last + 1;
                next += 1;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
