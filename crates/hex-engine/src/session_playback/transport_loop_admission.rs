//! Control-side periodic density proof for mutable Arrangement transport loops.
use super::SessionPlaybackError;
use crate::Track;
use hex_project::LoopRegion;

const QUERY_FRAMES: u64 = crate::midi_compile::RENDER_QUERY_FRAMES;
const MAX_ONSETS: usize = crate::midi_compile::MAX_ONSETS_PER_WINDOW;
const MAX_VOICES: usize = crate::midi_compile::MAX_SIMULTANEOUS_VOICES_PER_TRACK;
const MAX_EVENTS: usize = crate::mutable_instrument::MAX_MUTABLE_EVENTS_PER_QUERY;

#[derive(Clone, Copy)]
struct PeriodicSpan {
    start: u64,
}

/// Admit one track's periodic stream. With `steals_releasing_voices` the
/// declared slots bound held density only. The returned envelope still counts
/// releases, because live input shares the per-track proof ledger with them.
pub(super) fn admit_track(
    track: &Track,
    region: LoopRegion,
    release_tail_frames: u64,
    declared_max_events: usize,
    declared_max_voices: usize,
    steals_releasing_voices: bool,
) -> Result<usize, SessionPlaybackError> {
    admit_streams(
        &[track],
        region,
        release_tail_frames,
        declared_max_events,
        declared_max_voices,
        steals_releasing_voices,
    )
}

/// Admit all contributors that feed one shared source runtime as a single
/// periodic voice/event stream. The returned voice envelope includes release
/// spans that cross transport seams.
pub(super) fn admit_tracks(
    tracks: &[&Track],
    region: LoopRegion,
    release_tail_frames: u64,
    declared_max_events: usize,
    declared_max_voices: usize,
) -> Result<usize, SessionPlaybackError> {
    // Shared sources keep the conservative releases-included envelope.
    admit_streams(
        tracks,
        region,
        release_tail_frames,
        declared_max_events,
        declared_max_voices,
        false,
    )
}

fn admit_streams(
    tracks: &[&Track],
    region: LoopRegion,
    release_tail_frames: u64,
    declared_max_events: usize,
    declared_max_voices: usize,
    steals_releasing_voices: bool,
) -> Result<usize, SessionPlaybackError> {
    let period = region
        .end()
        .checked_sub(region.start())
        .filter(|period| *period > 0)
        .ok_or(SessionPlaybackError::MutableInstrumentLoopDensity)?;
    let first_pass = region.end();
    let mut onsets = Vec::new();
    let mut span_overflow = false;
    let mut allocation_failure = false;
    let mut intro = Vec::new();
    let mut repeats = Vec::new();
    let mut arithmetic_failure = false;
    for track in tracks {
        track
            .for_each_mutable_midi_span(0, region.end(), |span| {
                if span.start >= region.end() {
                    return;
                }
                let gate_end = span.gate_end.min(region.end());
                if span.start > gate_end {
                    arithmetic_failure = true;
                    return;
                }
                let Some(end) = gate_end.checked_add(release_tail_frames) else {
                    arithmetic_failure = true;
                    return;
                };
                push_bounded(
                    &mut intro,
                    crate::midi_template::TransportDensitySpan {
                        start: span.start,
                        gate_end,
                        raw_end: u128::from(end),
                    },
                    &mut span_overflow,
                    &mut allocation_failure,
                );
                let chased_at_start = span.start < region.start() && span.gate_end > region.start();
                if span.start < region.start() && !chased_at_start {
                    return;
                }
                let repeat_start = if chased_at_start {
                    region.start()
                } else {
                    span.start
                };
                let Some(gate_end_relative) = gate_end.checked_sub(region.start()) else {
                    arithmetic_failure = true;
                    return;
                };
                let repeat_start = repeat_start - region.start();
                if repeat_start > gate_end_relative || gate_end_relative > period {
                    arithmetic_failure = true;
                    return;
                }
                let Some(repeat_end) = gate_end_relative.checked_add(release_tail_frames) else {
                    arithmetic_failure = true;
                    return;
                };
                push_bounded(
                    &mut onsets,
                    PeriodicSpan {
                        start: repeat_start,
                    },
                    &mut span_overflow,
                    &mut allocation_failure,
                );
                push_bounded(
                    &mut repeats,
                    crate::midi_template::TransportDensitySpan {
                        start: repeat_start,
                        gate_end: gate_end_relative,
                        raw_end: u128::from(repeat_end),
                    },
                    &mut span_overflow,
                    &mut allocation_failure,
                );
            })
            .map_err(|_| SessionPlaybackError::MutableInstrumentEvents)?;
    }
    if arithmetic_failure {
        return Err(SessionPlaybackError::ArithmeticOverflow);
    }
    if allocation_failure {
        return Err(SessionPlaybackError::Allocation);
    }
    if span_overflow {
        return Err(SessionPlaybackError::MutableInstrumentLoopDensity);
    }
    let voice_limit = declared_max_voices.min(MAX_VOICES);
    let event_limit = declared_max_events.min(MAX_EVENTS);
    if voice_limit == 0 || event_limit == 0 {
        return Err(SessionPlaybackError::MutableInstrumentLoopDensity);
    }

    let mut repeated_edges = Vec::new();
    repeated_edges
        .try_reserve_exact(
            repeats
                .len()
                .checked_mul(2)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?,
        )
        .map_err(|_| SessionPlaybackError::Allocation)?;
    for span in &repeats {
        repeated_edges.push(PeriodicSpan { start: span.start });
        repeated_edges.push(PeriodicSpan {
            start: span.gate_end % period,
        });
    }
    let intro_edge_max = maximum_linear_event_edges(&intro)?;
    let periodic_edge_max = maximum_periodic_onsets(&repeated_edges, period)?;
    if maximum_periodic_onsets(&onsets, period)? > MAX_ONSETS
        || intro_edge_max > event_limit
        || periodic_edge_max > event_limit
    {
        return Err(SessionPlaybackError::MutableInstrumentLoopDensity);
    }
    let first_seam_edge_max =
        maximum_first_seam_event_edges(&intro, &repeated_edges, first_pass, period, event_limit)?;
    let max_voices = crate::midi_template::admit_transport_loop_density(
        &intro,
        &repeats,
        first_pass,
        period,
        release_tail_frames,
    )
    .map_err(|_| SessionPlaybackError::MutableInstrumentLoopDensity)?;
    let admitted_voices = if steals_releasing_voices {
        crate::midi_template::admit_held_transport_loop_density(
            &intro, &repeats, first_pass, period,
        )
        .map_err(|_| SessionPlaybackError::MutableInstrumentLoopDensity)?
    } else {
        max_voices
    };
    if first_seam_edge_max > event_limit || admitted_voices > voice_limit {
        return Err(SessionPlaybackError::MutableInstrumentLoopDensity);
    }
    // The envelope keeps releases: live input shares the proof ledger.
    Ok(max_voices)
}

fn maximum_linear_event_edges(
    spans: &[crate::midi_template::TransportDensitySpan],
) -> Result<usize, SessionPlaybackError> {
    let capacity = spans
        .len()
        .checked_mul(2)
        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
    let mut positions = Vec::new();
    positions
        .try_reserve_exact(capacity)
        .map_err(|_| SessionPlaybackError::Allocation)?;
    for span in spans {
        positions.push(span.start);
        positions.push(span.gate_end);
    }
    maximum_event_window(&mut positions)
}

fn maximum_first_seam_event_edges(
    intro: &[crate::midi_template::TransportDensitySpan],
    repeated_edges: &[PeriodicSpan],
    first_pass: u64,
    period: u64,
    event_limit: usize,
) -> Result<usize, SessionPlaybackError> {
    let earliest = first_pass.saturating_sub(QUERY_FRAMES.saturating_sub(1));
    let after_seam = first_pass
        .checked_add(QUERY_FRAMES)
        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
    let mut positions = Vec::new();
    let position_capacity = event_limit
        .checked_mul(2)
        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
    positions
        .try_reserve_exact(position_capacity)
        .map_err(|_| SessionPlaybackError::Allocation)?;
    for span in intro {
        if (earliest..=first_pass).contains(&span.start) {
            push_seam_edge(&mut positions, span.start, position_capacity)?;
        }
        if (earliest..=first_pass).contains(&span.gate_end) {
            push_seam_edge(&mut positions, span.gate_end, position_capacity)?;
        }
    }
    for edge in repeated_edges {
        let mut position = first_pass
            .checked_add(edge.start)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        while position < after_seam {
            push_seam_edge(&mut positions, position, position_capacity)?;
            position = position
                .checked_add(period)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        }
    }
    maximum_event_window(&mut positions)
}

fn push_seam_edge(
    positions: &mut Vec<u64>,
    position: u64,
    maximum: usize,
) -> Result<(), SessionPlaybackError> {
    if positions.len() >= maximum {
        return Err(SessionPlaybackError::MutableInstrumentLoopDensity);
    }
    positions.push(position);
    Ok(())
}

fn maximum_event_window(positions: &mut [u64]) -> Result<usize, SessionPlaybackError> {
    positions.sort_unstable();
    let mut maximum = 0;
    let mut right = 0;
    for left in 0..positions.len() {
        right = right.max(left);
        let end = positions[left]
            .checked_add(QUERY_FRAMES)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        while right < positions.len() && positions[right] < end {
            right += 1;
        }
        maximum = maximum.max(right - left);
    }
    Ok(maximum)
}

fn push_bounded<T>(
    target: &mut Vec<T>,
    value: T,
    overflow: &mut bool,
    allocation_failure: &mut bool,
) {
    if target.len() >= crate::midi_compile::MAX_COMPILED_MIDI_OCCURRENCES {
        *overflow = true;
        return;
    }
    if target.len() == target.capacity() {
        let additional = 64.min(crate::midi_compile::MAX_COMPILED_MIDI_OCCURRENCES - target.len());
        if target.try_reserve_exact(additional).is_err() {
            *allocation_failure = true;
            return;
        }
    }
    target.push(value);
}

fn maximum_periodic_onsets(
    spans: &[PeriodicSpan],
    period: u64,
) -> Result<usize, SessionPlaybackError> {
    let mut phases = Vec::new();
    phases
        .try_reserve_exact(spans.len())
        .map_err(|_| SessionPlaybackError::Allocation)?;
    phases.extend(spans.iter().map(|span| span.start % period));
    phases.sort_unstable();
    let full_cycles = QUERY_FRAMES / period;
    let baseline = usize::try_from(full_cycles)
        .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?
        .checked_mul(phases.len())
        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
    let remainder = QUERY_FRAMES % period;
    if remainder == 0 || phases.is_empty() {
        return Ok(baseline);
    }
    let mut starts = Vec::new();
    starts
        .try_reserve_exact(
            phases
                .len()
                .checked_mul(2)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?,
        )
        .map_err(|_| SessionPlaybackError::Allocation)?;
    for phase in phases {
        starts.push(phase);
        starts.push(
            phase
                .checked_add(period)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?,
        );
    }
    starts.sort_unstable();
    let mut residual_max = 0;
    let mut right = 0;
    for left in 0..starts.len() / 2 {
        right = right.max(left);
        let window_end = starts[left]
            .checked_add(remainder)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        while right < starts.len() && starts[right] < window_end {
            right += 1;
        }
        residual_max = residual_max.max(right - left);
    }
    baseline
        .checked_add(residual_max)
        .ok_or(SessionPlaybackError::ArithmeticOverflow)
}

#[cfg(test)]
mod tests {
    use super::{PeriodicSpan, maximum_periodic_onsets};

    fn brute(phases: &[u64], period: u64, width: u64) -> usize {
        (0..period)
            .map(|cursor| {
                let cycles = width / period;
                let remainder = width % period;
                let baseline = cycles as usize * phases.len();
                baseline
                    + phases
                        .iter()
                        .filter(|phase| (*phase + period - cursor) % period < remainder)
                        .count()
            })
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn periodic_onset_bound_matches_integer_frame_oracle_for_short_and_long_periods() {
        for period in 1..=19 {
            for mask in 0_u32..128 {
                let phases = (0..period)
                    .filter(|phase| (mask >> (phase % 7)) & 1 != 0)
                    .collect::<Vec<_>>();
                let spans = phases
                    .iter()
                    .map(|phase| PeriodicSpan { start: *phase })
                    .collect::<Vec<_>>();
                assert_eq!(
                    maximum_periodic_onsets(&spans, period).unwrap(),
                    brute(&phases, period, super::QUERY_FRAMES),
                    "period={period}, mask={mask}"
                );
            }
        }
    }
}
