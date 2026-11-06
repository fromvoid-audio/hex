//! Bounded half-open overlays for delivered native parameter touches.
//! Uncovered time evaluates the original base envelope exactly. A newer take
//! replaces only its covered intervals; existing left/right fragments retain
//! their held values and original step times.
use super::{AutomationValue, MAX_DEVICE_AUTOMATION_POINTS_PER_LANE};
use crate::{BeatTime, Invalid};

pub const MAX_DEVICE_AUTOMATION_TOUCH_SPANS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceAutomationTouchStep {
    pub beat: BeatTime,
    pub value: AutomationValue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceAutomationTouchSpan {
    pub start: BeatTime,
    pub end: BeatTime,
    pub steps: Vec<DeviceAutomationTouchStep>,
}

impl DeviceAutomationTouchSpan {
    fn value_at(&self, beat: BeatTime) -> Option<AutomationValue> {
        if !(self.start..self.end).contains(&beat) {
            return None;
        }
        let index = self.steps.partition_point(|step| step.beat <= beat);
        self.steps.get(index.checked_sub(1)?).map(|step| step.value)
    }

    fn fragment(&self, start: BeatTime, end: BeatTime) -> Result<Self, Invalid> {
        let held = self.value_at(start).ok_or(Invalid(
            "device automation touch fragment has no held value",
        ))?;
        let first = self.steps.partition_point(|step| step.beat <= start);
        let last = self.steps.partition_point(|step| step.beat < end);
        let mut steps = Vec::new();
        steps
            .try_reserve_exact(1 + last.saturating_sub(first))
            .map_err(|_| Invalid("could not reserve device automation touch fragment"))?;
        steps.push(DeviceAutomationTouchStep {
            beat: start,
            value: held,
        });
        if let Some(retained) = self.steps.get(first..last) {
            steps.extend_from_slice(retained);
        }
        Ok(Self { start, end, steps })
    }
}

pub(super) fn validate(
    spans: &[DeviceAutomationTouchSpan],
    base_points: usize,
) -> Result<(), Invalid> {
    if spans.len() > MAX_DEVICE_AUTOMATION_TOUCH_SPANS
        || spans.windows(2).any(|pair| pair[0].end > pair[1].start)
    {
        return Err(Invalid(
            "device automation touch spans overlap or exceed capacity",
        ));
    }
    let mut count = base_points;
    for span in spans {
        if span.start >= span.end
            || span
                .steps
                .first()
                .is_none_or(|first| first.beat != span.start)
            || span.steps.last().is_some_and(|last| last.beat >= span.end)
            || span
                .steps
                .windows(2)
                .any(|pair| pair[0].beat >= pair[1].beat)
        {
            return Err(Invalid("device automation touch span is invalid"));
        }
        count = count
            .checked_add(span.steps.len())
            .ok_or(Invalid("device automation point count overflow"))?;
        if count > MAX_DEVICE_AUTOMATION_POINTS_PER_LANE {
            return Err(Invalid(
                "device automation envelope exceeds its point budget",
            ));
        }
    }
    Ok(())
}

pub(super) fn value_at(
    spans: &[DeviceAutomationTouchSpan],
    beat: BeatTime,
) -> Option<AutomationValue> {
    let next = spans.partition_point(|span| span.start <= beat);
    spans.get(next.checked_sub(1)?)?.value_at(beat)
}

pub(super) fn splice(
    old: &[DeviceAutomationTouchSpan],
    added: Vec<DeviceAutomationTouchSpan>,
    base_points: usize,
) -> Result<Vec<DeviceAutomationTouchSpan>, Invalid> {
    if added.is_empty() {
        return Err(Invalid("device automation take has no covered time"));
    }
    validate(&added, base_points)?;
    let mut result = Vec::new();
    // Each existing and incoming interval can contribute at most two
    // boundaries to the combined disjoint inventory.
    result
        .try_reserve_exact(old.len() + 2 * added.len())
        .map_err(|_| Invalid("could not reserve device automation touch splice"))?;
    for span in old {
        let mut start = span.start;
        let index = added.partition_point(|next| next.end <= start);
        for next in added[index..]
            .iter()
            .take_while(|next| next.start < span.end)
        {
            if start < next.start {
                result.push(span.fragment(start, next.start)?);
            }
            start = start.max(next.end);
        }
        if start < span.end {
            result.push(span.fragment(start, span.end)?);
        }
    }
    result.extend(added);
    result.sort_unstable_by_key(|span| span.start);
    validate(&result, base_points)?;
    Ok(result)
}
