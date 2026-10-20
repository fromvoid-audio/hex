use super::*;
use hex_project::{AudioDomainClock, AudioWarpMapRead, BeatsPreserve};
const Q32: i128 = 1_i128 << 32;

/// Reserve on the caller's control/UI side BEFORE entering a retained read.
/// Private Copy-only scratch; no source, model, PCM or admission owner is kept.
pub struct WarpPreviewWorkspace {
    boundaries: Vec<u64>,
    segments: Vec<hex_warp::MapSegment>,
    attacks: Vec<u64>,
}
impl WarpPreviewWorkspace {
    fn empty() -> Self {
        Self {
            boundaries: Vec::new(),
            segments: Vec::new(),
            attacks: Vec::new(),
        }
    }
    pub fn reserve_on_control() -> Result<Self, Invalid> {
        let mut result = Self::empty();
        reserve(
            &mut result.boundaries,
            hex_warp::MAX_MAP_SEGMENTS,
            true,
            "warp projection allocation failed",
        )?;
        reserve(
            &mut result.segments,
            hex_warp::MAX_MAP_SEGMENTS,
            true,
            "warp segment allocation failed",
        )?;
        reserve(
            &mut result.attacks,
            hex_project::MAX_AUDIO_POINTS,
            true,
            "beat attack allocation failed",
        )?;
        Ok(result)
    }
}
fn reserve<T>(
    values: &mut Vec<T>,
    needed: usize,
    growing: bool,
    message: &'static str,
) -> Result<(), Invalid> {
    if needed > values.capacity() {
        if !growing {
            return Err(Invalid("warp preview scratch capacity is insufficient"));
        }
        values
            .try_reserve_exact(needed.saturating_sub(values.len()))
            .map_err(|_| Invalid(message))?;
    }
    Ok(())
}
fn push(points: &mut Vec<u64>, point: u64, growing: bool) -> Result<(), Invalid> {
    // Preserve the actual pre-dedup boundary limit and its error precedence.
    if points.len() >= hex_warp::MAX_MAP_SEGMENTS {
        return Err(Invalid("warp projection exceeds 8192 bounded segments"));
    }
    if growing {
        points
            .try_reserve(1)
            .map_err(|_| Invalid("warp projection allocation failed"))?;
    } else if points.len() == points.capacity() {
        return Err(Invalid("warp preview scratch capacity is insufficient"));
    }
    points.push(point);
    Ok(())
}
pub(super) fn prepare_map(
    edit: &AudioClipEdit,
    tempo: &hex_project::TempoMap,
    cancel: &dyn crate::CancellationQuery,
) -> Result<(hex_warp::TimeMap, Vec<u64>), Invalid> {
    let mut scratch = WarpPreviewWorkspace::empty();
    let frames = edit.with_warp_map_read(tempo, |read| {
        prepare_projection(&read, &mut scratch, cancel, true)
    })?;
    Ok((
        hex_warp::TimeMap::new(scratch.segments, frames).map_err(warp_error)?,
        scratch.attacks,
    ))
}
pub(super) fn validate_preview(
    preview: &hex_project::AudioClipPreviewRead<'_>,
    scratch: &mut WarpPreviewWorkspace,
) -> Result<(), Invalid> {
    preview.with_warp_map_read(|read| {
        let Some(read) = read else {
            return Ok(());
        };
        let frames = prepare_projection(&read, scratch, &AtomicBool::new(false), false)?;
        let map = hex_warp::TimeMapRead::new(&scratch.segments, frames).map_err(warp_error)?;
        let mode = if read.warp_enabled() {
            Some(read.warp_mode()?)
        } else {
            None
        };
        map.validate_stretch(render_warp_mode(mode), read.pitch_cents())
            .map_err(warp_error)
    })
}
fn prepare_projection(
    read: &AudioWarpMapRead<'_>,
    scratch: &mut WarpPreviewWorkspace,
    cancel: &dyn crate::CancellationQuery,
    growing: bool,
) -> Result<usize, Invalid> {
    // Copy elements only: clear/dedup cannot free or grow their backing.
    scratch.boundaries.clear();
    scratch.segments.clear();
    scratch.attacks.clear();
    let (first, length, _, _) = read.projected_domain()?;
    let length_usize = usize::try_from(length)
        .map_err(|_| Invalid("audio domain length exceeds addressable range"))?;
    if !read.warp_enabled() {
        reserve(
            &mut scratch.segments,
            1,
            growing,
            "warp segment allocation failed",
        )?;
        scratch.segments.push(hex_warp::MapSegment {
            output_start: 0,
            output_end: length,
            source_start_q32: 0,
            source_end_q32: i128::from(length) * Q32,
        });
        return Ok(length_usize);
    }
    let AudioDomainClock::Beats(span) = read.clock() else {
        return Err(Invalid("warp requires a musical domain"));
    };
    let marker_count = read.marker_count()?;
    let unit = read
        .marker(
            marker_count
                .checked_sub(1)
                .ok_or(Invalid("warp endpoint missing"))?,
        )?
        .beat
        .raw();
    if unit == 0 || marker_count < 2 {
        return Err(Invalid("invalid warp marker endpoints"));
    }
    let phase_beat = read.phase_beat()?;
    let total = u128::from(span.raw()) + u128::from(phase_beat);
    let cycles = if read.looping() {
        total / u128::from(unit) + 1
    } else {
        1
    };
    if cycles > hex_warp::MAX_MAP_SEGMENTS as u128 {
        return Err(Invalid("warp loop density exceeds bounded projection"));
    }
    let boundaries = &mut scratch.boundaries;
    push(boundaries, 0, growing)?;
    push(boundaries, span.raw(), growing)?;
    for cycle in 0..cycles {
        if cancel.is_cancelled() {
            return Err(Invalid("audio preparation cancelled"));
        }
        for index in 0..marker_count {
            let marker = read.marker(index)?;
            let position = cycle * u128::from(unit) + u128::from(marker.beat.raw());
            if position > u128::from(phase_beat) {
                let local = position - u128::from(phase_beat);
                if local < u128::from(span.raw()) {
                    push(boundaries, local as u64, growing)?;
                }
            }
        }
    }
    let origin = read.projection_origin()?;
    for point in read.tempo_points() {
        let local = i128::from(point.beat().raw()) - origin;
        if local > 0 && local < i128::from(span.raw()) {
            push(boundaries, local as u64, growing)?;
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    let segments = &mut scratch.segments;
    reserve(
        segments,
        boundaries.len().saturating_sub(1),
        growing,
        "warp segment allocation failed",
    )?;
    for pair in boundaries.windows(2) {
        let output = |local: u64| {
            u64::try_from(read.project_frame(origin + i128::from(local))? - first)
                .map_err(|_| Invalid("warp output projection overflow"))
        };
        let output_start = output(pair[0])?;
        let output_end = output(pair[1])?;
        if output_start == output_end {
            return Err(Invalid(
                "warp markers or tempo changes are closer than one prepared output frame",
            ));
        }
        let source =
            |local: u64| read.source_position_q32(u128::from(local) + u128::from(phase_beat));
        segments.push(hex_warp::MapSegment {
            output_start,
            output_end,
            source_start_q32: if pair[0] == 0 {
                i128::from(read.source_phase()) * Q32
            } else {
                source(pair[0])?
            },
            source_end_q32: source(pair[1])?,
        });
    }
    let attacks = &mut scratch.attacks;
    if let WarpMode::Beats {
        preserve: BeatsPreserve::Division(division),
        ..
    } = read.warp_mode()?
    {
        if division.raw() == 0 {
            return Err(Invalid("beat division must be positive"));
        }
        let count = unit / division.raw() + 1;
        if count > hex_project::MAX_AUDIO_POINTS as u64 {
            return Err(Invalid("beat division exceeds 1024 source attacks"));
        }
        reserve(
            attacks,
            count as usize,
            growing,
            "beat attack allocation failed",
        )?;
        let mut beat = 0_u64;
        while beat < unit {
            let frame = u64::try_from(read.source_position_q32(u128::from(beat))? / Q32)
                .map_err(|_| Invalid("beat attack source position overflow"))?;
            if frame < read.source_length() && attacks.last() != Some(&frame) {
                attacks.push(frame);
            }
            beat = beat
                .checked_add(division.raw())
                .ok_or(Invalid("beat division overflow"))?;
        }
    }
    Ok(length_usize)
}
pub(super) fn apply_fade(
    frames: &mut [crate::Frame],
    edit: &AudioClipEdit,
    tempo: &hex_project::TempoMap,
    cancel: &dyn crate::CancellationQuery,
) -> Result<(), Invalid> {
    let domain = &edit.domain;
    if domain.fade.is_neutral() {
        return Ok(());
    }
    let projected = edit.projected_fade(tempo)?;
    for (index, frame) in frames.iter_mut().enumerate() {
        if index % 1_024 == 0 && cancel.is_cancelled() {
            return Err(Invalid("audio preparation cancelled"));
        }
        let gain = projected.gain_at(index as u64)?;
        frame[0] *= gain;
        frame[1] *= gain;
    }
    Ok(())
}

#[cfg(test)]
mod trim_workspace_tests;
