//! Canonical half-open gain-lane overlays and bounded splice operations.
use crate::{BeatTime, GainPoint, Invalid, MAX_GAIN_POINTS_PER_TRACK, validate_gain_envelope};

pub const GAIN_TOUCH_UNSUPPORTED: &str = "clear recorded touch spans before editing the base curve";
pub const MAX_GAIN_TOUCH_SPANS_PER_TRACK: usize = 128;
pub const MAX_GAIN_TOUCH_STEPS_PER_SPAN: usize = MAX_GAIN_POINTS_PER_TRACK - 2;

/// Structurally checked provenance, not evidence that a runtime applied a value.
/// `recording_epoch` is durable lane metadata; runtime gesture counters may restart in a new context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GainTouchContext {
    pub recording_epoch: u64,
    pub project_generation: u64,
    pub canonical_revision: u64,
    pub audio_revision: u64,
    pub transport_epoch: u64,
    pub gesture_epoch: u64,
    pub gesture_serial: u64,
    pub sample_rate: u32,
    pub loop_pass: u64,
}
impl GainTouchContext {
    fn validate(self) -> Result<(), Invalid> {
        if self.recording_epoch == 0
            || self.canonical_revision == 0
            || self.audio_revision == 0
            || self.transport_epoch == 0
            || self.gesture_epoch == 0
            || self.gesture_serial == 0
        {
            return Err(Invalid("gain touch context identities must be nonzero"));
        }
        if !(8000..=192000).contains(&self.sample_rate) {
            return Err(Invalid(
                "gain touch context sample rate must be 8000..192000 hz",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GainTouchStep {
    pub beat: BeatTime,
    pub gain: f32,
    /// Original event anchor; a retained right fragment carries a held value to its new start.
    pub origin_beat: BeatTime,
    pub source_sequence: u64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct GainTouchSpan {
    /// Immutable recording beat corresponding to current placement `start`.
    /// Relocation changes placement/step beats; event origins/context remain original.
    pub source_start: BeatTime,
    pub start: BeatTime,
    pub end: BeatTime,
    pub context: GainTouchContext,
    pub steps: Vec<GainTouchStep>,
}
impl GainTouchSpan {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.context.validate()?;
        if self.start >= self.end
            || self
                .source_start
                .raw()
                .checked_add(self.end.raw().saturating_sub(self.start.raw()))
                .is_none()
        {
            return Err(Invalid("gain touch span must be nonempty and half-open"));
        }
        if self.steps.is_empty() || self.steps.len() > MAX_GAIN_TOUCH_STEPS_PER_SPAN {
            return Err(Invalid("gain touch step count is outside its bound"));
        }
        if self.steps[0].beat != self.start {
            return Err(Invalid("gain touch span must start with a held value"));
        }
        for (index, step) in self.steps.iter().enumerate() {
            if step.beat < self.start
                || step.beat >= self.end
                || self
                    .source_start
                    .raw()
                    .checked_add(step.beat.raw() - self.start.raw())
                    .is_none_or(|source_beat| step.origin_beat.raw() > source_beat)
                || step.source_sequence == 0
                || !step.gain.is_finite()
                || !(0.0..=2.0).contains(&step.gain)
            {
                return Err(Invalid("gain touch step is invalid"));
            }
            if index > 0 {
                let old = self.steps[index - 1];
                if old.beat >= step.beat
                    || old.source_sequence >= step.source_sequence
                    || old.origin_beat >= step.origin_beat
                {
                    return Err(Invalid(
                        "gain touch event anchors and sequences must strictly increase",
                    ));
                }
                if step.origin_beat.raw()
                    != self
                        .source_start
                        .raw()
                        .checked_add(step.beat.raw() - self.start.raw())
                        .ok_or(Invalid("gain touch source position overflow"))?
                {
                    return Err(Invalid(
                        "only a fragment's first step may carry an earlier event",
                    ));
                }
            }
        }
        Ok(())
    }
    pub fn gain_at(&self, beat: BeatTime) -> Option<f32> {
        if beat < self.start || beat >= self.end {
            return None;
        }
        let index = self
            .steps
            .partition_point(|step| step.beat <= beat)
            .checked_sub(1)?;
        self.steps.get(index).map(|step| step.gain)
    }
    pub(crate) fn fragment(&self, start: BeatTime, end: BeatTime) -> Result<Self, Invalid> {
        let index = self
            .steps
            .partition_point(|step| step.beat <= start)
            .checked_sub(1)
            .ok_or(Invalid("gain touch fragment has no held value"))?;
        let mut first = self.steps[index];
        first.beat = start;
        let mut steps = vec![first];
        steps.extend(
            self.steps
                .iter()
                .copied()
                .filter(|step| step.beat > start && step.beat < end),
        );
        let fragment = Self {
            source_start: BeatTime::from_raw(
                self.source_start
                    .raw()
                    .checked_add(start.raw() - self.start.raw())
                    .ok_or(Invalid("gain touch source position overflow"))?,
            ),
            start,
            end,
            context: self.context,
            steps,
        };
        fragment.validate()?;
        Ok(fragment)
    }
}
/// One Project-owned value. The original curve is never copied into nested spans.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GainLane {
    pub(crate) base: Vec<GainPoint>,
    pub(crate) spans: Vec<GainTouchSpan>,
    pub(crate) recording_epoch: u64,
}
impl GainLane {
    pub fn new(
        base: Vec<GainPoint>,
        spans: Vec<GainTouchSpan>,
        recording_epoch: u64,
    ) -> Result<Self, Invalid> {
        let lane = Self {
            base,
            spans,
            recording_epoch,
        };
        lane.validate()?;
        Ok(lane)
    }
    pub fn base(&self) -> &[GainPoint] {
        &self.base
    }
    pub fn spans(&self) -> &[GainTouchSpan] {
        &self.spans
    }
    pub fn recording_epoch(&self) -> u64 {
        self.recording_epoch
    }
    pub fn units(&self) -> Result<usize, Invalid> {
        lane_units(&self.base, &self.spans)
    }
    pub fn validate(&self) -> Result<(), Invalid> {
        validate_gain_lane_parts(&self.base, &self.spans, self.recording_epoch).map(|_| ())
    }
    /// `None` means the unchanged base curve/static fallback owns this position.
    pub fn touch_gain_at(&self, beat: BeatTime) -> Option<f32> {
        let index = self
            .spans
            .partition_point(|span| span.start <= beat)
            .checked_sub(1)?;
        self.spans.get(index)?.gain_at(beat)
    }
    /// Pure, bounded metadata construction; Project admission also checks the project-wide limit.
    pub fn with_touch(
        &self,
        expected_epoch: u64,
        proposed: GainTouchSpan,
    ) -> Result<Self, Invalid> {
        let (_, span_count) = self.touch_shape(expected_epoch, &proposed)?;
        // Reserve only the already checked normalized result.
        let mut spans = Vec::new();
        spans
            .try_reserve_exact(span_count)
            .map_err(|_| Invalid("could not reserve gain touch fragments"))?;
        for old in &self.spans {
            if old.end <= proposed.start || old.start >= proposed.end {
                spans.push(old.clone());
            } else {
                if old.start < proposed.start {
                    spans.push(old.fragment(old.start, proposed.start)?);
                }
                if old.end > proposed.end {
                    spans.push(old.fragment(proposed.end, old.end)?);
                }
            }
        }
        let epoch = proposed.context.recording_epoch;
        spans.push(proposed);
        spans.sort_by_key(|span| span.start);
        Self::new(self.base.clone(), spans, epoch)
    }
    /// Preflight the normalized weighted result before a Project allocates it.
    pub(crate) fn touch_units(
        &self,
        expected_epoch: u64,
        proposed: &GainTouchSpan,
    ) -> Result<usize, Invalid> {
        self.touch_shape(expected_epoch, proposed)
            .map(|(units, _)| units)
    }
    fn touch_shape(
        &self,
        expected_epoch: u64,
        proposed: &GainTouchSpan,
    ) -> Result<(usize, usize), Invalid> {
        self.validate()?;
        proposed.validate()?;
        if expected_epoch != self.recording_epoch
            || proposed.context.recording_epoch <= self.recording_epoch
        {
            return Err(Invalid("gain touch replacement epoch is stale"));
        }
        // Count the complete normalized result before cloning any event/base vector.
        let mut count = self
            .base
            .len()
            .checked_add(proposed.steps.len() + 2)
            .ok_or(Invalid("gain lane count overflow"))?;
        let mut span_count = 1_usize;
        for old in &self.spans {
            let fragments = if old.end <= proposed.start || old.start >= proposed.end {
                [Some((old.start, old.end)), None]
            } else {
                [
                    (old.start < proposed.start).then_some((old.start, proposed.start)),
                    (old.end > proposed.end).then_some((proposed.end, old.end)),
                ]
            };
            for (start, end) in fragments.into_iter().flatten() {
                span_count += 1;
                let events = 1 + old
                    .steps
                    .iter()
                    .filter(|step| step.beat > start && step.beat < end)
                    .count();
                count = count
                    .checked_add(events + 2)
                    .ok_or(Invalid("gain lane count overflow"))?;
            }
        }
        if span_count > MAX_GAIN_TOUCH_SPANS_PER_TRACK || count > MAX_GAIN_POINTS_PER_TRACK {
            return Err(Invalid("gain lane exceeds its bounded metadata"));
        }
        Ok((count, span_count))
    }
}

fn lane_units(base: &[GainPoint], spans: &[GainTouchSpan]) -> Result<usize, Invalid> {
    spans.iter().try_fold(base.len(), |count, span| {
        count
            .checked_add(span.steps.len())
            .and_then(|n| n.checked_add(2))
            .ok_or(Invalid("gain lane count overflow"))
    })
}
pub fn validate_gain_lane_parts(
    base: &[GainPoint],
    spans: &[GainTouchSpan],
    epoch: u64,
) -> Result<usize, Invalid> {
    validate_gain_envelope(base)?;
    if spans.len() > MAX_GAIN_TOUCH_SPANS_PER_TRACK {
        return Err(Invalid("gain touch span limit reached"));
    }
    let units = lane_units(base, spans)?;
    if units > MAX_GAIN_POINTS_PER_TRACK {
        return Err(Invalid("gain lane exceeds its bounded metadata"));
    }
    for (index, span) in spans.iter().enumerate() {
        span.validate()?;
        if span.context.recording_epoch > epoch {
            return Err(Invalid("gain touch context exceeds the lane epoch"));
        }
        if index > 0 && spans[index - 1].end > span.start {
            return Err(Invalid("gain touch spans must be ordered and disjoint"));
        }
        for old in &spans[..index] {
            if old.context.recording_epoch == span.context.recording_epoch {
                if old.context != span.context {
                    return Err(Invalid("gain touch epoch has conflicting context"));
                }
                for step in &span.steps {
                    for prior in &old.steps {
                        if (step.source_sequence < prior.source_sequence
                            && step.origin_beat >= prior.origin_beat)
                            || (step.source_sequence > prior.source_sequence
                                && step.origin_beat <= prior.origin_beat)
                            || (step.source_sequence == prior.source_sequence
                                && (step.origin_beat != prior.origin_beat
                                    || step.gain.to_bits() != prior.gain.to_bits()))
                        {
                            return Err(Invalid(
                                "gain touch fragments have inconsistent event provenance",
                            ));
                        }
                    }
                }
            }
        }
    }
    Ok(units)
}
impl crate::Project {
    pub(crate) fn retain_gain_touch_epochs_from(&mut self, newer: &Self) {
        for track in &mut self.tracks {
            if let Some(old) = newer.track(track.id) {
                track.gain_lane.recording_epoch = track
                    .gain_lane
                    .recording_epoch
                    .max(old.gain_lane.recording_epoch);
            }
        }
    }
}

#[cfg(test)]
mod tests;
