//! Bounded union of source intervals visited by an elapsed half-open window.
//! Shared by the canonical audio/MIDI projection owners, never a playback path.
use crate::Invalid;
use std::ops::Range;

pub(crate) fn source_ranges(
    start: u128,
    end: u128,
    source_start: u64,
    initial_end: u64,
    looping: Option<(u64, u64)>,
) -> Result<[Option<Range<u64>>; 2], Invalid> {
    let first = initial_end
        .checked_sub(source_start)
        .filter(|length| *length > 0)
        .ok_or(Invalid("source interval must have positive length"))?;
    if start >= end {
        return Ok([None, None]);
    }
    let mut pieces = [None; 3];
    if start < u128::from(first) {
        let a = u64::try_from(u128::from(source_start) + start)
            .map_err(|_| Invalid("source interval start overflow"))?;
        let b = u64::try_from(u128::from(source_start) + end.min(u128::from(first)))
            .map_err(|_| Invalid("source interval end overflow"))?;
        pieces[0] = Some((a, b));
    }
    if let Some((loop_start, loop_end)) = looping {
        let cycle = loop_end
            .checked_sub(loop_start)
            .filter(|length| *length > 0)
            .ok_or(Invalid("source loop must have positive length"))?;
        let a = start.saturating_sub(u128::from(first));
        let b = end.saturating_sub(u128::from(first));
        if b > a {
            let span = b - a;
            if span >= u128::from(cycle) {
                pieces[1] = Some((loop_start, loop_end));
            } else {
                let phase = u64::try_from(a % u128::from(cycle))
                    .map_err(|_| Invalid("source loop phase overflow"))?;
                let span = u64::try_from(span).map_err(|_| Invalid("source loop span overflow"))?;
                let at = loop_start
                    .checked_add(phase)
                    .ok_or(Invalid("source loop start overflow"))?;
                let remaining = loop_end - at;
                pieces[1] = Some((at, at + span.min(remaining)));
                if span > remaining {
                    pieces[2] = Some((loop_start, loop_start + (span - remaining)));
                }
            }
        }
    }
    pieces.sort_unstable_by_key(|piece| piece.unwrap_or((u64::MAX, u64::MAX)));
    let mut merged: [Option<(u64, u64)>; 2] = [None, None];
    for (start, end) in pieces.into_iter().flatten() {
        if let Some(last) = merged.iter_mut().rev().flatten().next()
            && start <= last.1
        {
            last.1 = last.1.max(end);
        } else {
            let slot = merged
                .iter_mut()
                .find(|slot| slot.is_none())
                .ok_or(Invalid("source interval union exceeds its bound"))?;
            *slot = Some((start, end));
        }
    }
    Ok(merged.map(|slot| slot.map(|(start, end)| start..end)))
}
