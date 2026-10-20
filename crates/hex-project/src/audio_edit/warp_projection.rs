//! Canonical warp law over either retained or virtually retimed markers.
use super::{AudioClipDomain, WarpSettings};
use crate::{BeatTime, Invalid, WarpMarker};
fn partition_point(
    count: usize,
    marker: &impl Fn(usize) -> Result<WarpMarker, Invalid>,
    predicate: impl Fn(WarpMarker) -> bool,
) -> Result<usize, Invalid> {
    let (mut left, mut right) = (0, count);
    while left < right {
        let middle = left + (right - left) / 2;
        if predicate(marker(middle)?) {
            left = middle + 1;
        } else {
            right = middle;
        }
    }
    Ok(left)
}
pub(crate) fn beat_for_source_frame(
    count: usize,
    marker: impl Fn(usize) -> Result<WarpMarker, Invalid>,
    source: u64,
) -> Result<BeatTime, Invalid> {
    if count < 2 {
        return Err(Invalid("warp needs two endpoints"));
    }
    let index = partition_point(count, &marker, |m| m.source_frame <= source)?
        .saturating_sub(1)
        .min(count - 2);
    let a = marker(index)?;
    let b = marker(index + 1)?;
    let delta = source
        .checked_sub(a.source_frame)
        .ok_or(Invalid("source precedes warp marker"))?;
    let frames = b
        .source_frame
        .checked_sub(a.source_frame)
        .filter(|v| *v > 0)
        .ok_or(Invalid("warp source markers must increase"))?;
    let beats = b
        .beat
        .raw()
        .checked_sub(a.beat.raw())
        .filter(|v| *v > 0)
        .ok_or(Invalid("warp beat markers must increase"))?;
    let value =
        u128::from(delta) * u128::from(beats) / u128::from(frames) + u128::from(a.beat.raw());
    Ok(BeatTime::from_raw(
        u64::try_from(value).map_err(|_| Invalid("source beat projection overflow"))?,
    ))
}
pub(crate) fn source_position_q32(
    count: usize,
    marker: impl Fn(usize) -> Result<WarpMarker, Invalid>,
    source_offset: u64,
    source_length: u64,
    looping: bool,
    beat: u128,
) -> Result<i128, Invalid> {
    if count < 2 {
        return Err(Invalid("warp needs two endpoints"));
    }
    let unit = marker(count - 1)?.beat.raw();
    if unit == 0 {
        return Err(Invalid("warp beat span must be positive"));
    }
    let (cycle, local) = if looping {
        (beat / u128::from(unit), (beat % u128::from(unit)) as u64)
    } else {
        (
            0,
            u64::try_from(beat).map_err(|_| Invalid("warp beat position overflow"))?,
        )
    };
    let index = partition_point(count, &marker, |m| m.beat.raw() <= local)?
        .saturating_sub(1)
        .min(count - 2);
    let a = marker(index)?;
    let b = marker(index + 1)?;
    let marker_offset = a
        .source_frame
        .checked_sub(source_offset)
        .ok_or(Invalid("warp marker precedes source region"))?;
    let base = cycle
        .checked_mul(u128::from(source_length))
        .and_then(|n| n.checked_add(u128::from(marker_offset)))
        .and_then(|n| n.checked_mul(1_u128 << 32))
        .ok_or(Invalid("warp source cycle overflow"))?;
    let beat_delta = local
        .checked_sub(a.beat.raw())
        .ok_or(Invalid("warp beat precedes marker"))?;
    let source_delta = b
        .source_frame
        .checked_sub(a.source_frame)
        .filter(|v| *v > 0)
        .ok_or(Invalid("warp source markers must increase"))?;
    let marker_span = b
        .beat
        .raw()
        .checked_sub(a.beat.raw())
        .filter(|v| *v > 0)
        .ok_or(Invalid("warp beat markers must increase"))?;
    let delta = u128::from(beat_delta)
        .checked_mul(u128::from(source_delta))
        .and_then(|n| n.checked_mul(1_u128 << 32))
        .ok_or(Invalid("warp source projection overflow"))?
        / u128::from(marker_span);
    i128::try_from(
        base.checked_add(delta)
            .ok_or(Invalid("warp source position overflow"))?,
    )
    .map_err(|_| Invalid("warp source position overflow"))
}
impl WarpSettings {
    /// Local beat for an absolute project-rate source frame.
    pub fn beat_for_source_frame(&self, source: u64) -> Result<BeatTime, Invalid> {
        beat_for_source_frame(
            self.markers.len(),
            |i| {
                self.markers
                    .get(i)
                    .copied()
                    .ok_or(Invalid("warp endpoint missing"))
            },
            source,
        )
    }
}
impl AudioClipDomain {
    /// Source-relative, unwrapped Q32 frame position; consumers wrap only when
    /// indexing peaks or PCM. Shared with virtual preview interpolation.
    pub fn warp_source_position_q32(&self, beat: u128) -> Result<i128, Invalid> {
        let warp = self.warp.as_ref().ok_or(Invalid("warp settings missing"))?;
        source_position_q32(
            warp.markers.len(),
            |i| {
                warp.markers
                    .get(i)
                    .copied()
                    .ok_or(Invalid("warp endpoint missing"))
            },
            self.source_offset,
            self.source_length,
            self.looping,
            beat,
        )
    }
    pub fn warp_phase_beat(&self) -> Result<u64, Invalid> {
        self.warp
            .as_ref()
            .ok_or(Invalid("warp settings missing"))?
            .beat_for_source_frame(
                self.source_offset
                    .checked_add(self.source_phase)
                    .ok_or(Invalid("warp phase overflow"))?,
            )
            .map(BeatTime::raw)
    }
}
