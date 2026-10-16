//! Bounded control-side musical landmarks; no playback clock or audible click policy.
use super::{BeatTime, MeterMap, MeterPosition, TempoMap};
use crate::Invalid;
use std::ops::Range;

/// One authored grid landmark and its canonical independently derived position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BeatPulse {
    beat: BeatTime,
    frame: u64,
    position: MeterPosition,
}

impl BeatPulse {
    pub const fn beat(self) -> BeatTime {
        self.beat
    }
    pub const fn frame(self) -> u64 {
        self.frame
    }
    pub const fn position(self) -> MeterPosition {
        self.position
    }
}

/// Return grid landmarks `origin + n * step`, for nonnegative integer `n`, whose
/// canonical rounded frames fall in the half-open interval. `step` must be
/// positive; origin is an explicit nonnegative absolute beat, not a phase that
/// implies landmarks before it. Meter markers annotate the grid, never reset it.
///
/// Results retain ascending beat order, including distinct beats that round to
/// the same frame. The exact candidate count is admitted against `output_budget`
/// before allocation or iteration. Work is bounded by that count and immutable
/// map lookups; allocation belongs on the control side, not an audio callback.
/// Reversed ranges, arithmetic/capacity failures and allocation refusal return
/// no partial output. Empty ranges need no capacity. There is no schema, clock,
/// loop repetition, accent, transport or sound behavior in this query.
pub fn query_beat_pulses(
    tempo: &TempoMap,
    meter: &MeterMap,
    origin: BeatTime,
    step: BeatTime,
    frames: Range<u64>,
    output_budget: usize,
) -> Result<Vec<BeatPulse>, Invalid> {
    if step.raw() == 0 {
        return Err(Invalid("pulse step must be positive"));
    }
    if frames.start > frames.end {
        return Err(Invalid("pulse frame range is reversed"));
    }
    if frames.start == frames.end {
        return Ok(Vec::new());
    }
    let lower = tempo.beat_lower_bound_for_frame(frames.start)?;
    let upper = tempo.beat_lower_bound_for_frame(frames.end)?;
    let origin = u128::from(origin.raw());
    let step = u128::from(step.raw());
    let distance = lower.saturating_sub(origin);
    let index = (distance / step)
        .checked_add(u128::from(distance % step != 0))
        .ok_or(Invalid("pulse grid index overflow"))?;
    let first = origin
        .checked_add(
            index
                .checked_mul(step)
                .ok_or(Invalid("pulse grid offset overflow"))?,
        )
        .ok_or(Invalid("pulse grid position overflow"))?;
    if first >= upper {
        return Ok(Vec::new());
    }
    let count = ((upper - 1 - first) / step)
        .checked_add(1)
        .ok_or(Invalid("pulse count overflow"))?;
    if count > output_budget as u128 {
        return Err(Invalid("pulse output budget exceeded"));
    }
    let count =
        usize::try_from(count).map_err(|_| Invalid("pulse count exceeds platform capacity"))?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(count)
        .map_err(|_| Invalid("pulse output allocation refused"))?;
    let mut raw = first;
    for index in 0..count {
        let beat = BeatTime::from_raw(
            u64::try_from(raw).map_err(|_| Invalid("pulse beat exceeds representable range"))?,
        );
        let frame = tempo.beat_to_frame(beat)?;
        if frame < frames.start || frame >= frames.end {
            return Err(Invalid("pulse frame violates canonical lower bound"));
        }
        output.push(BeatPulse {
            beat,
            frame,
            position: meter.position(beat)?,
        });
        if index + 1 < count {
            raw = raw
                .checked_add(step)
                .ok_or(Invalid("pulse beat advance overflow"))?;
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests;
