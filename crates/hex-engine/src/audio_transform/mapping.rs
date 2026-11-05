//! Control-side projection of validated canonical warp/tempo boundaries into a
//! bounded integer-frame map. Collapsed tempo/crop fragments share a sample;
//! collapsed authored intervals retain continuous stretch validation before
//! quantization. Root endpoints and surviving markers remain exact. No callback work.
use super::*;
use hex_project::{AudioDomainClock, BeatsPreserve, audio_projection_frame};
const Q32: i128 = 1_i128 << 32;
fn push(points: &mut Vec<(u64, bool)>, point: u64, marker: bool) -> Result<(), Invalid> {
    if points.len() >= hex_warp::MAX_MAP_SEGMENTS {
        return Err(Invalid("warp projection exceeds 8192 bounded segments"));
    }
    points
        .try_reserve(1)
        .map_err(|_| Invalid("warp projection allocation failed"))?;
    points.push((point, marker));
    Ok(())
}
pub(super) fn prepare_map(
    edit: &AudioClipEdit,
    tempo: &hex_project::TempoMap,
    cancel: &AtomicBool,
) -> Result<(hex_warp::TimeMap, Vec<u64>), Invalid> {
    let tempo = edit.processing_tempo(tempo);
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
    push(&mut boundaries, 0, false)?;
    push(&mut boundaries, span.raw(), false)?;
    for cycle in 0..cycles {
        if cancel.load(Ordering::Acquire) {
            return Err(Invalid("audio preparation cancelled"));
        }
        for marker in &warp.markers {
            let position = cycle * u128::from(unit) + u128::from(marker.beat.raw());
            if position >= u128::from(phase_beat) {
                let local = position - u128::from(phase_beat);
                if local == 0 {
                    boundaries[0].1 = true;
                } else if local == u128::from(span.raw()) {
                    boundaries[1].1 = true;
                } else if local < u128::from(span.raw()) {
                    push(&mut boundaries, local as u64, true)?;
                }
            }
        }
    }
    let origin = edit.processing_origin()?;
    for point in tempo.points() {
        let local = i128::from(point.beat().raw()) - origin;
        if local > 0 && local < i128::from(span.raw()) {
            push(&mut boundaries, local as u64, false)?;
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup_by(|later, earlier| {
        if later.0 == earlier.0 {
            earlier.1 |= later.1;
            true
        } else {
            false
        }
    });
    // Tempo crossings and partial root intervals can be arbitrarily close to
    // a marker or crop endpoint. They do not own separate output samples. Keep
    // exact root endpoints, then authored markers, then the latest tempo point
    // at each rounded frame. Validate collapsed intervals against their original
    // continuous timing law so rounding cannot hide unsupported source speeds.
    let mut projected: Vec<(u64, u64)> = Vec::new();
    projected
        .try_reserve_exact(boundaries.len())
        .map_err(|_| Invalid("warp projection allocation failed"))?;
    let source = |local: u64| {
        if local == 0 {
            Ok(i128::from(domain.source_phase) * Q32)
        } else {
            domain.warp_source_position_q32(u128::from(local) + u128::from(phase_beat))
        }
    };
    let mut previous_boundary = None;
    let mut retained_marker = false;
    for (local, marker) in boundaries {
        let frame =
            u64::try_from(audio_projection_frame(tempo, origin + i128::from(local))? - first)
                .map_err(|_| Invalid("warp output projection overflow"))?;
        if let Some((previous_local, previous_frame)) = previous_boundary
            && frame == previous_frame
        {
            // Every tempo point is a boundary, so this open interval has
            // one tempo. Signed invisible context uses tempo at zero.
            let beat = (origin + i128::from(previous_local)).max(0);
            let beat = hex_project::BeatTime::from_raw(
                u64::try_from(beat).map_err(|_| Invalid("warp beat projection overflow"))?,
            );
            let delta = source(local)? - source(previous_local)?;
            let ratio =
                (local - previous_local) as f64 * f64::from(tempo.sample_rate()) * 60_000_000.0
                    / f64::from(tempo.tempo_at(beat).micro_bpm())
                    / delta as f64;
            hex_warp::validate_stretch_ratio(ratio, render_mode(domain)?, domain.pitch_cents)
                .map_err(warp_error)?;
        }
        previous_boundary = Some((local, frame));
        if let Some((previous_local, previous_frame)) = projected.last_mut()
            && *previous_frame == frame
        {
            if local == span.raw() || (*previous_local != 0 && (marker || !retained_marker)) {
                *previous_local = local;
                retained_marker = marker;
            }
        } else {
            projected.push((local, frame));
            retained_marker = marker;
        }
    }
    let mut segments = Vec::new();
    segments
        .try_reserve_exact(projected.len().saturating_sub(1))
        .map_err(|_| Invalid("warp segment allocation failed"))?;
    for pair in projected.windows(2) {
        let (local_start, output_start) = pair[0];
        let (local_end, output_end) = pair[1];
        segments.push(hex_warp::MapSegment {
            output_start,
            output_end,
            source_start_q32: source(local_start)?,
            source_end_q32: source(local_end)?,
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
    first_root_frame: u64,
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
        let gain = projected.gain_at(first_root_frame + index as u64)?;
        frame[0] *= gain;
        frame[1] *= gain;
    }
    Ok(())
}
