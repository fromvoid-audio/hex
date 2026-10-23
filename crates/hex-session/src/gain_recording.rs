//! Control-side construction of one atomic gain take from delivered sample stamps.
//! Wall-clock pointer events are deliberately not accepted as recording evidence.
use hex_project::{GainTouchContext, GainTouchSpan, GainTouchStep, Project, TrackId};

#[derive(Clone, Copy, Debug)]
pub struct AppliedGain {
    pub sequence: u64,
    pub performance_frame: u64,
    pub content_frame: u64,
    pub loop_pass: u64,
    pub gain: f32,
}
#[derive(Clone, Copy, Debug)]
pub struct GainTakeEnd {
    pub performance_frame: u64,
    pub content_frame: u64,
    pub loop_pass: u64,
}
/// Normalize successful sink receipts into disjoint, half-open loop-pass
/// proposals. The actor applies the entire ordered batch as one history edit.
pub fn finish_take(
    project: &Project,
    track: TrackId,
    expected_epoch: u64,
    context: GainTouchContext,
    events: &[AppliedGain],
    end: GainTakeEnd,
) -> Result<Vec<GainTouchSpan>, &'static str> {
    let owner = project
        .track(track)
        .ok_or("recorded track no longer exists")?;
    if owner.gain_lane().recording_epoch() != expected_epoch
        || context.recording_epoch
            != expected_epoch
                .checked_add(1)
                .ok_or("recording epoch exhausted")?
        || context.sample_rate != project.sample_rate()
    {
        return Err("gain take source or epoch changed");
    }
    if events.is_empty() {
        return Ok(Vec::new());
    }
    if events.len() > hex_project::MAX_GAIN_TOUCH_STEPS_PER_SPAN {
        return Err("gain take exceeds its event bound");
    }
    let first = events[0];
    if end.performance_frame <= first.performance_frame {
        return Ok(Vec::new());
    }
    let loop_region = project.loop_region().filter(|region| region.enabled());
    let position = |delta: u64| -> Result<(u64, u64), &'static str> {
        advance(first.content_frame, first.loop_pass, delta, loop_region)
    };
    let mut previous = None;
    for event in events {
        if event.sequence == 0
            || !event.gain.is_finite()
            || !(0.0..=2.0).contains(&event.gain)
            || event.performance_frame < first.performance_frame
            || event.performance_frame >= end.performance_frame
            || previous.is_some_and(|old: AppliedGain| {
                old.sequence >= event.sequence || old.performance_frame >= event.performance_frame
            })
        {
            return Err("gain take events are invalid or unordered");
        }
        if position(event.performance_frame - first.performance_frame)?
            != (event.content_frame, event.loop_pass)
        {
            return Err("gain take transport history is discontinuous");
        }
        previous = Some(*event);
    }
    if position(end.performance_frame - first.performance_frame)?
        != (end.content_frame, end.loop_pass)
    {
        return Err("gain take release history is discontinuous");
    }
    // Count loop pieces and weighted storage before constructing any span/step vectors.
    let duration = end.performance_frame - first.performance_frame;
    let pieces = if let Some(region) = loop_region {
        if first.content_frame >= region.end() {
            return Err("gain take loop origin is invalid");
        }
        let initial = region.end() - first.content_frame;
        if duration <= initial {
            1
        } else {
            let width = region.end() - region.start();
            1_u64
                .checked_add((duration - initial - 1) / width + 1)
                .ok_or("gain take loop count overflow")?
        }
    } else {
        1
    };
    let pieces = usize::try_from(pieces).map_err(|_| "gain take loop count overflow")?;
    if pieces > hex_project::MAX_GAIN_TOUCH_SPANS_PER_TRACK {
        return Err("gain take exceeds its bounded loop capacity");
    }
    let mut carry = 0_usize;
    let mut count_cursor = first.performance_frame;
    for index in 0..pieces {
        if index != 0
            && !events
                .iter()
                .any(|event| event.performance_frame == count_cursor)
        {
            carry += 1;
        }
        let (frame, _) = position(count_cursor - first.performance_frame)?;
        let count = loop_region.map_or(end.performance_frame - count_cursor, |region| {
            (region.end() - frame).min(end.performance_frame - count_cursor)
        });
        count_cursor = count_cursor
            .checked_add(count)
            .ok_or("gain take frame overflow")?;
    }
    if events
        .len()
        .checked_add(carry)
        .and_then(|n| n.checked_add(pieces * 2))
        .is_none_or(|n| n > hex_project::MAX_GAIN_POINTS_PER_TRACK)
    {
        return Err("gain take exceeds its bounded point capacity");
    }
    let exact_beat = |frame| {
        let beat = project
            .tempo_map()
            .frame_to_beat(frame)
            .map_err(|error| error.0)?;
        if project
            .tempo_map()
            .beat_to_frame(beat)
            .map_err(|error| error.0)?
            != frame
        {
            return Err("gain take sample position cannot be represented exactly");
        }
        Ok(beat)
    };
    let mut spans = Vec::new();
    spans
        .try_reserve_exact(pieces)
        .map_err(|_| "could not reserve gain take spans")?;
    let mut cursor = first.performance_frame;
    let mut event_index = 0;
    let mut held = first.gain;
    for index in 0..pieces {
        let (start_frame, pass) = position(cursor - first.performance_frame)?;
        let until = loop_region.map_or(end.performance_frame - cursor, |region| {
            (region.end() - start_frame).min(end.performance_frame - cursor)
        });
        let finish = cursor
            .checked_add(until)
            .ok_or("gain take frame overflow")?;
        let end_frame = start_frame
            .checked_add(until)
            .ok_or("gain take frame overflow")?;
        while event_index < events.len() && events[event_index].performance_frame <= cursor {
            held = events[event_index].gain;
            event_index += 1;
        }
        let start = exact_beat(start_frame)?;
        let mut steps = Vec::new();
        let remaining =
            events[event_index..].partition_point(|event| event.performance_frame < finish);
        steps
            .try_reserve_exact(remaining + 1)
            .map_err(|_| "could not reserve gain take events")?;
        steps.push(GainTouchStep {
            beat: start,
            gain: held,
            origin_beat: start,
            source_sequence: cursor
                .checked_add(1)
                .ok_or("gain sample identity exhausted")?,
        });
        for event in &events[event_index..event_index + remaining] {
            let beat = exact_beat(start_frame + (event.performance_frame - cursor))?;
            steps.push(GainTouchStep {
                beat,
                gain: event.gain,
                origin_beat: beat,
                source_sequence: event
                    .performance_frame
                    .checked_add(1)
                    .ok_or("gain sample identity exhausted")?,
            });
            held = event.gain;
        }
        event_index += remaining;
        let span = GainTouchSpan {
            source_start: start,
            start,
            end: exact_beat(end_frame)?,
            steps,
            context: GainTouchContext {
                recording_epoch: context
                    .recording_epoch
                    .checked_add(index as u64)
                    .ok_or("recording epoch exhausted")?,
                loop_pass: pass,
                ..context
            },
        };
        span.validate().map_err(|error| error.0)?;
        spans.push(span);
        cursor = finish;
    }
    Ok(spans)
}

fn advance(
    frame: u64,
    pass: u64,
    delta: u64,
    region: Option<hex_project::LoopRegion>,
) -> Result<(u64, u64), &'static str> {
    let Some(region) = region else {
        return Ok((
            frame.checked_add(delta).ok_or("gain take frame overflow")?,
            pass,
        ));
    };
    if frame >= region.end() {
        return Err("gain take loop origin is invalid");
    }
    let initial = region.end() - frame;
    if delta < initial {
        return Ok((frame + delta, pass));
    }
    let remaining = delta - initial;
    let width = region.end() - region.start();
    let pass = pass
        .checked_add(1)
        .and_then(|n| n.checked_add(remaining / width))
        .ok_or("gain take loop pass exhausted")?;
    Ok((region.start() + remaining % width, pass))
}
