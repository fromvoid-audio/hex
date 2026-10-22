use super::*;
use hex_project::{AudioDomainClock, BeatsPreserve, audio_projection_frame};
const Q32: i128 = 1_i128 << 32;
fn push(points: &mut Vec<u64>, point: u64) -> Result<(), Invalid> {
    if points.len() >= hex_warp::MAX_MAP_SEGMENTS {
        return Err(Invalid("warp projection exceeds 8192 bounded segments"));
    }
    points
        .try_reserve(1)
        .map_err(|_| Invalid("warp projection allocation failed"))?;
    points.push(point);
    Ok(())
}
pub(super) fn prepare_map(
    edit: &AudioClipEdit,
    tempo: &hex_project::TempoMap,
    cancel: &AtomicBool,
) -> Result<(hex_warp::TimeMap, Vec<u64>), Invalid> {
    let (first, length, _, _) = edit.projected_domain(tempo)?;
    let length_usize = usize::try_from(length)
        .map_err(|_| Invalid("audio domain length exceeds addressable range"))?;
    let domain = &edit.domain;
    if !domain.warp_enabled {
        return Ok((
            hex_warp::TimeMap::new(
                vec![hex_warp::MapSegment {
                    output_start: 0,
                    output_end: length,
                    source_start_q32: 0,
                    source_end_q32: i128::from(length) * Q32,
                }],
                length_usize,
            )
            .map_err(warp_error)?,
            Vec::new(),
        ));
    }
    let AudioDomainClock::Beats(span) = domain.clock else {
        return Err(Invalid("warp requires a musical domain"));
    };
    let warp = domain
        .warp
        .as_ref()
        .ok_or(Invalid("warp settings missing"))?;
    let unit = warp
        .markers
        .last()
        .ok_or(Invalid("warp endpoint missing"))?
        .beat
        .raw();
    if unit == 0 || warp.markers.len() < 2 {
        return Err(Invalid("invalid warp marker endpoints"));
    }
    let phase_beat = domain.warp_phase_beat()?;
    let total = u128::from(span.raw()) + u128::from(phase_beat);
    let cycles = if domain.looping {
        total / u128::from(unit) + 1
    } else {
        1
    };
    if cycles > hex_warp::MAX_MAP_SEGMENTS as u128 {
        return Err(Invalid("warp loop density exceeds bounded projection"));
    }
    let mut boundaries = Vec::new();
    push(&mut boundaries, 0)?;
    push(&mut boundaries, span.raw())?;
    for cycle in 0..cycles {
        if cancel.load(Ordering::Acquire) {
            return Err(Invalid("audio preparation cancelled"));
        }
        for marker in &warp.markers {
            let position = cycle * u128::from(unit) + u128::from(marker.beat.raw());
            if position > u128::from(phase_beat) {
                let local = position - u128::from(phase_beat);
                if local < u128::from(span.raw()) {
                    push(&mut boundaries, local as u64)?;
                }
            }
        }
    }
    let origin = edit.projection_origin()?;
    for point in tempo.points() {
        let local = i128::from(point.beat().raw()) - origin;
        if local > 0 && local < i128::from(span.raw()) {
            push(&mut boundaries, local as u64)?;
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    let mut segments = Vec::new();
    segments
        .try_reserve_exact(boundaries.len().saturating_sub(1))
        .map_err(|_| Invalid("warp segment allocation failed"))?;
    for pair in boundaries.windows(2) {
        let output = |local: u64| {
            u64::try_from(audio_projection_frame(tempo, origin + i128::from(local))? - first)
                .map_err(|_| Invalid("warp output projection overflow"))
        };
        let output_start = output(pair[0])?;
        let output_end = output(pair[1])?;
        if output_start == output_end {
            return Err(Invalid(
                "warp markers or tempo changes are closer than one prepared output frame",
            ));
        }
        let source = |local: u64| {
            domain.warp_source_position_q32(u128::from(local) + u128::from(phase_beat))
        };
        segments.push(hex_warp::MapSegment {
            output_start,
            output_end,
            source_start_q32: if pair[0] == 0 {
                i128::from(domain.source_phase) * Q32
            } else {
                source(pair[0])?
            },
            source_end_q32: source(pair[1])?,
        });
    }
    let mut attacks = Vec::new();
    if let WarpMode::Beats {
        preserve: BeatsPreserve::Division(division),
        ..
    } = warp.mode
    {
        if division.raw() == 0 {
            return Err(Invalid("beat division must be positive"));
        }
        let count = unit / division.raw() + 1;
        if count > hex_project::MAX_AUDIO_POINTS as u64 {
            return Err(Invalid("beat division exceeds 1024 source attacks"));
        }
        attacks
            .try_reserve_exact(count as usize)
            .map_err(|_| Invalid("beat attack allocation failed"))?;
        let mut beat = 0_u64;
        while beat < unit {
            let frame = u64::try_from(domain.warp_source_position_q32(u128::from(beat))? / Q32)
                .map_err(|_| Invalid("beat attack source position overflow"))?;
            if frame < domain.source_length && attacks.last() != Some(&frame) {
                attacks.push(frame);
            }
            beat = beat
                .checked_add(division.raw())
                .ok_or(Invalid("beat division overflow"))?;
        }
    }
    Ok((
        hex_warp::TimeMap::new(segments, length_usize).map_err(warp_error)?,
        attacks,
    ))
}
pub(super) fn apply_fade(
    frames: &mut [crate::Frame],
    edit: &AudioClipEdit,
    tempo: &hex_project::TempoMap,
    cancel: &AtomicBool,
) -> Result<(), Invalid> {
    let domain = &edit.domain;
    if domain.fade.is_neutral() {
        return Ok(());
    }
    let projected = edit.projected_fade(tempo)?;
    for (index, frame) in frames.iter_mut().enumerate() {
        if index % 1_024 == 0 && cancel.load(Ordering::Acquire) {
            return Err(Invalid("audio preparation cancelled"));
        }
        let gain = projected.gain_at(index as u64)?;
        frame[0] *= gain;
        frame[1] *= gain;
    }
    Ok(())
}
