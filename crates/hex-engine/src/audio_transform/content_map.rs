//! Fractional control-prepared read maps. Each occurrence retains its complete
//! finite outer PCM as filter context; tempo and occurrence seams never create
//! new isolated inputs. Only the actual finite base edges are zero padded.
use super::*;
use hex_project::{AudioClipEdit, AudioViewOccurrence, AudioViewProjection};
const Q32: i128 = 1_i128 << 32;
pub(super) struct Span {
    pub(super) first: u64,
    pub(super) last: u64,
    pub(super) map: hex_warp::TimeMap,
}
pub(super) fn prepare(
    projection: &AudioViewProjection<'_>,
    edit: &AudioClipEdit,
    occurrence: AudioViewOccurrence,
) -> Result<Option<Span>, Invalid> {
    let window = projection
        .content_window()
        .ok_or(Invalid("directed audio view missing"))?;
    let begin = occurrence.elapsed_start.max(window.start);
    let end = occurrence.elapsed_end.min(window.end);
    if begin >= end {
        return Ok(None);
    }
    let (a, b) = if window.reversed {
        (window.end - end, window.end - begin)
    } else {
        (begin - window.start, end - window.start)
    };
    let first = projection
        .frame_at_elapsed(a)?
        .max(projection.outer_frame_bounds().0);
    let last = projection
        .frame_at_elapsed(b)?
        .min(projection.outer_frame_bounds().1);
    if first >= last {
        return Ok(None);
    }
    let tempo = projection.placement_tempo();
    let current = projection.placement_reference();
    let mut boundaries = Vec::new();
    boundaries
        .try_reserve_exact(
            tempo
                .points()
                .len()
                .checked_mul(2)
                .and_then(|n| n.checked_add(2))
                .ok_or(Invalid("directed map boundary overflow"))?,
        )
        .map_err(|_| Invalid("directed map allocation failed"))?;
    boundaries.push(a);
    boundaries.push(b);
    for point in tempo.points() {
        let destination = i128::from(point.beat().raw()) - current;
        if destination > i128::from(a) && destination < i128::from(b) {
            boundaries.push(destination as u64);
        }
        let source = i128::from(point.beat().raw()) - projection.base_reference_start();
        if source > i128::from(begin) && source < i128::from(end) {
            let elapsed = if window.reversed {
                i128::from(window.end) - source
            } else {
                source - i128::from(window.start)
            };
            boundaries.push(
                u64::try_from(elapsed).map_err(|_| Invalid("directed reflected seam overflow"))?,
            );
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    let at_elapsed = |elapsed: u64| -> Result<i128, Invalid> {
        let read = if window.reversed {
            window.end - elapsed
        } else {
            window.start + elapsed
        };
        let frame = projection.base_frame_q32_at_elapsed(read)?;
        let (base_start, base_end) = base_bounds(projection, edit)?;
        if window.reversed {
            i128::from(base_end)
                .checked_mul(Q32)
                .and_then(|n| n.checked_sub(frame))
                .ok_or(Invalid("directed fractional reflection overflow"))
        } else {
            frame
                .checked_sub(i128::from(base_start) * Q32)
                .ok_or(Invalid("directed fractional source overflow"))
        }
    };
    // Refuse unsupported continuous speeds even if a very short tempo
    // segment later shares a rounded output boundary with its neighbor.
    for pair in boundaries.windows(2) {
        let output = projection
            .frame_q32_at_elapsed(pair[1])?
            .checked_sub(projection.frame_q32_at_elapsed(pair[0])?)
            .ok_or(Invalid("directed continuous output overflow"))?;
        let input = at_elapsed(pair[1])?
            .checked_sub(at_elapsed(pair[0])?)
            .ok_or(Invalid("directed continuous input overflow"))?;
        hex_warp::validate_stretch_ratio(output as f64 / input as f64, hex_warp::Mode::RePitch, 0)
            .map_err(warp_error)?;
    }
    for boundary in &mut boundaries {
        *boundary = projection.frame_at_elapsed(*boundary)?.clamp(first, last);
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    let source = |frame: u64| -> Result<i128, Invalid> {
        at_elapsed(projection.elapsed_at_frame(frame)?.clamp(a, b))
    };
    let mut segments = Vec::new();
    segments
        .try_reserve_exact(boundaries.len() - 1)
        .map_err(|_| Invalid("directed tempo segment allocation failed"))?;
    for pair in boundaries.windows(2) {
        segments.push(hex_warp::MapSegment {
            output_start: pair[0] - first,
            output_end: pair[1] - first,
            source_start_q32: source(pair[0])?,
            source_end_q32: source(pair[1])?,
        });
    }
    let map = hex_warp::TimeMap::new(
        segments,
        usize::try_from(last - first).map_err(|_| Invalid("directed output size overflow"))?,
    )
    .map_err(warp_error)?;
    map.validate_stretch(hex_warp::Mode::RePitch, 0)
        .map_err(warp_error)?;
    Ok(Some(Span { first, last, map }))
}

/// The retained finite base is admitted independently of the current crop. This
/// bounds filter context even when a small directed view reads a huge loop.
pub(super) fn base_bounds(
    projection: &AudioViewProjection<'_>,
    edit: &AudioClipEdit,
) -> Result<(u64, u64), Invalid> {
    let view = edit
        .content_view
        .ok_or(Invalid("directed audio view missing"))?;
    let length = view.base_placement.map_or(
        view.base_view_end
            .checked_sub(view.base_view_start)
            .ok_or(Invalid("directed base crop is reversed"))?,
        |p| p.outer_length.length(),
    );
    let start = projection.base_frame_at_elapsed(0)?;
    let end = projection.base_frame_at_elapsed(length)?;
    end.checked_sub(start)
        .filter(|n| *n > 0 && *n <= MAX_AUDIO_DOMAIN_FRAMES)
        .ok_or(Invalid(
            "directed filter context exceeds bounded base frames",
        ))?;
    Ok((start, end))
}
