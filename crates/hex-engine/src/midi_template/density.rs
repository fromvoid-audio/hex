//! Control-side interval-density proofs for ongoing MIDI edge templates.
use super::{MIDI_TEMPLATE_MAX_EDGES, MIDI_TEMPLATE_MAX_ONSETS, MidiTemplateError};

#[derive(Clone, Copy, Debug)]
pub(super) struct DensityEvent {
    pub(super) phase: u64,
    pub(super) onset: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct DensityEnvelope {
    pub(super) max_edges: usize,
    pub(super) max_onsets: usize,
}

pub(super) fn admit_density(
    intro: &[DensityEvent],
    repeats: &[DensityEvent],
    first_pass: u64,
    period: Option<u64>,
    window: u64,
) -> Result<DensityEnvelope, MidiTemplateError> {
    let intro_edges = maximum_linear_count(intro, window, false);
    let intro_onsets = maximum_linear_count(intro, window, true);
    let mut periodic_edges = 0_u128;
    let mut periodic_onsets = 0_u128;
    let mut seam_edges = 0_u128;
    let mut seam_onsets = 0_u128;
    if let Some(period) = period {
        periodic_edges = maximum_periodic_count(repeats, period, window, false)?;
        periodic_onsets = maximum_periodic_count(repeats, period, window, true)?;
        seam_edges = seam_bound(intro, repeats, first_pass, period, window, false)?;
        seam_onsets = seam_bound(intro, repeats, first_pass, period, window, true)?;
    }
    let max_edges = (intro_edges as u128).max(periodic_edges).max(seam_edges);
    let max_onsets = (intro_onsets as u128).max(periodic_onsets).max(seam_onsets);
    if max_edges > MIDI_TEMPLATE_MAX_EDGES as u128 {
        return Err(MidiTemplateError::TooManyEdges);
    }
    if max_onsets > MIDI_TEMPLATE_MAX_ONSETS as u128 {
        return Err(MidiTemplateError::TooManyOnsets);
    }
    Ok(DensityEnvelope {
        max_edges: usize::try_from(max_edges).map_err(|_| MidiTemplateError::ArithmeticOverflow)?,
        max_onsets: usize::try_from(max_onsets)
            .map_err(|_| MidiTemplateError::ArithmeticOverflow)?,
    })
}

fn seam_bound(
    intro: &[DensityEvent],
    repeats: &[DensityEvent],
    first_pass: u64,
    period: u64,
    window: u64,
    onsets_only: bool,
) -> Result<u128, MidiTemplateError> {
    let tail_start = first_pass.saturating_sub(window);
    let intro_count = intro
        .iter()
        .filter(|event| event.phase >= tail_start && (!onsets_only || event.onset))
        .count() as u128;
    let repeat_count = periodic_count_from_zero(repeats, period, window, onsets_only)?;
    Ok(intro_count + repeat_count)
}

fn maximum_linear_count(events: &[DensityEvent], width: u64, onsets_only: bool) -> usize {
    let mut maximum = 0;
    let mut end = 0;
    let mut onset_count = 0_usize;
    for start in 0..events.len() {
        end = end.max(start);
        while end < events.len()
            && u128::from(events[end].phase) - u128::from(events[start].phase) <= u128::from(width)
        {
            onset_count += usize::from(events[end].onset);
            end += 1;
        }
        let count = if onsets_only {
            onset_count
        } else {
            end - start
        };
        maximum = maximum.max(count);
        onset_count -= usize::from(events[start].onset);
    }
    maximum
}

fn normalized_periodic_phases(
    events: &[DensityEvent],
    period: u64,
    onsets_only: bool,
) -> Result<Vec<u64>, MidiTemplateError> {
    let count = events
        .iter()
        .filter(|event| !onsets_only || event.onset)
        .count();
    let mut phases = Vec::new();
    phases
        .try_reserve_exact(count)
        .map_err(|_| MidiTemplateError::Allocation)?;
    phases.extend(
        events
            .iter()
            .filter(|event| !onsets_only || event.onset)
            .map(|event| {
                if event.phase == period {
                    0
                } else {
                    event.phase
                }
            }),
    );
    phases.sort_unstable();
    Ok(phases)
}

/// Maximum count in any closed circular interval of `width` raw beats.
///
/// `width = q * period + r` contributes `q * N` events plus a maximal closed
/// cyclic residual arc. When `r == 0`, that closed point deliberately includes
/// coincident phase events at both period endpoints.
fn maximum_periodic_count(
    events: &[DensityEvent],
    period: u64,
    width: u64,
    onsets_only: bool,
) -> Result<u128, MidiTemplateError> {
    if period == 0 {
        return Ok(0);
    }
    let phases = normalized_periodic_phases(events, period, onsets_only)?;
    if phases.is_empty() {
        return Ok(0);
    }
    let cycle_count = phases.len() as u128;
    let full_cycles = u128::from(width / period);
    let residual = width % period;
    let mut maximum_residual = 0_usize;
    let mut end = 0_usize;
    let count = phases.len();
    for start in 0..count {
        end = end.max(start);
        while end < start + count {
            let end_phase = if end < count {
                u128::from(phases[end])
            } else {
                u128::from(phases[end - count]) + u128::from(period)
            };
            if end_phase - u128::from(phases[start]) > u128::from(residual) {
                break;
            }
            end += 1;
        }
        maximum_residual = maximum_residual.max(end - start);
    }
    Ok(full_cycles * cycle_count + maximum_residual as u128)
}

/// Count repeated events in the closed seam tail `[0, width]`.
fn periodic_count_from_zero(
    events: &[DensityEvent],
    period: u64,
    width: u64,
    onsets_only: bool,
) -> Result<u128, MidiTemplateError> {
    if period == 0 {
        return Ok(0);
    }
    let phases = normalized_periodic_phases(events, period, onsets_only)?;
    let cycle_count = phases.len() as u128;
    let full_cycles = u128::from(width / period);
    let residual = width % period;
    let residual_count = phases.partition_point(|phase| *phase <= residual) as u128;
    Ok(full_cycles * cycle_count + residual_count)
}

#[cfg(test)]
mod tests {
    use super::{
        DensityEvent, maximum_linear_count, maximum_periodic_count, periodic_count_from_zero,
        seam_bound,
    };

    fn events(phases: &[u64], period: u64) -> Vec<DensityEvent> {
        let mut events: Vec<_> = phases
            .iter()
            .map(|phase| DensityEvent {
                phase: *phase,
                onset: true,
            })
            .collect();
        events.sort_unstable_by_key(|event| {
            if event.phase == period {
                0
            } else {
                event.phase
            }
        });
        events
    }

    fn brute_periodic(phases: &[u64], period: u64, width: u64) -> u128 {
        if phases.is_empty() {
            return 0;
        }
        let normalized: Vec<_> = phases
            .iter()
            .map(|phase| if *phase == period { 0 } else { *phase })
            .collect();
        let mut maximum = 0_u128;
        for start in 0..period {
            let mut count = 0_u128;
            let first_iteration = i128::from(start / period) - i128::from(width / period) - 1;
            let last_iteration = i128::from(start / period) + i128::from(width / period) + 2;
            for iteration in first_iteration..=last_iteration {
                for phase in &normalized {
                    let position = iteration * i128::from(period) + i128::from(*phase);
                    if position >= i128::from(start)
                        && position <= i128::from(start) + i128::from(width)
                    {
                        count += 1;
                    }
                }
            }
            maximum = maximum.max(count);
        }
        maximum
    }

    #[test]
    fn cyclic_closed_arc_formula_matches_tiny_period_oracle() {
        for period in 1..=8 {
            for mask in 0_u16..256 {
                let phases: Vec<_> = (0..period)
                    .filter(|phase| (mask >> (phase % 8)) & 1 != 0)
                    .chain((mask % 3 == 0).then_some(period))
                    .collect();
                let events = events(&phases, period);
                for width in 0..=period * 3 {
                    assert_eq!(
                        maximum_periodic_count(&events, period, width, false).unwrap(),
                        brute_periodic(&phases, period, width),
                        "period={period}, width={width}, phases={phases:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn linear_windows_and_seam_tail_bounds_match_closed_interval_oracles() {
        for phases in [vec![], vec![0], vec![0, 0, 3], vec![1, 2, 2, 7, 9]] {
            let period = 10;
            let mut intro: Vec<_> = phases
                .iter()
                .map(|phase| DensityEvent {
                    phase: *phase,
                    onset: true,
                })
                .collect();
            intro.sort_unstable_by_key(|event| event.phase);
            for width in 0..=10 {
                let max_end = phases.iter().copied().max().unwrap_or(0);
                let brute = (0..=max_end)
                    .map(|start| {
                        phases
                            .iter()
                            .filter(|phase| **phase >= start && **phase <= start + width)
                            .count()
                    })
                    .max()
                    .unwrap_or(0);
                assert_eq!(maximum_linear_count(&intro, width, false), brute);

                let events = events(&phases, period);
                let seam_bound = periodic_count_from_zero(&events, period, width, false).unwrap();
                let brute_tail = (0..=width)
                    .flat_map(|position| {
                        phases.iter().map(move |phase| {
                            let phase = if *phase == period { 0 } else { *phase };
                            position % period == phase
                        })
                    })
                    .filter(|present| *present)
                    .count();
                assert!(seam_bound >= brute_tail as u128);
            }
        }
    }

    #[test]
    fn seam_bound_covers_every_tiny_intro_to_repeat_window() {
        for period in 1..=7 {
            for width in 0..=period * 2 {
                let intro_phases = [0, 2, 4, 6, 8];
                let repeat_phases = [0, 1, period / 2, period];
                let intro: Vec<_> = intro_phases
                    .iter()
                    .map(|phase| DensityEvent {
                        phase: *phase,
                        onset: true,
                    })
                    .collect();
                let repeats = events(&repeat_phases, period);
                let first_pass = 10;
                let bound = seam_bound(&intro, &repeats, first_pass, period, width, false).unwrap();
                for before in 0..=width {
                    let intro_start = first_pass.saturating_sub(before);
                    let tail_end = width - before;
                    let intro_count = intro_phases
                        .iter()
                        .filter(|phase| **phase >= intro_start && **phase <= first_pass)
                        .count();
                    let repeat_count = (0..=tail_end)
                        .flat_map(|position| {
                            repeat_phases.iter().map(move |phase| {
                                let phase = if *phase == period { 0 } else { *phase };
                                position % period == phase
                            })
                        })
                        .filter(|present| *present)
                        .count();
                    assert!(bound >= (intro_count + repeat_count) as u128);
                }
            }
        }
    }
}
