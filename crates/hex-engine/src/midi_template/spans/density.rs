//! Control-side source-span voice-density proof.
use super::super::MidiTemplateError;
use super::VoiceSpan;

#[derive(Clone, Copy)]
pub(crate) struct DensitySpan {
    pub(crate) start: u64,
    pub(crate) gate_end: u64,
    pub(crate) raw_end: u128,
}

trait DensityView {
    fn start(&self) -> u64;
    fn gate_end(&self) -> u64;
    fn raw_end(&self) -> u128;
}

impl DensityView for VoiceSpan {
    fn start(&self) -> u64 {
        self.start
    }
    fn gate_end(&self) -> u64 {
        self.gate_end
    }
    fn raw_end(&self) -> u128 {
        self.raw_end
    }
}

impl DensityView for DensitySpan {
    fn start(&self) -> u64 {
        self.start
    }
    fn gate_end(&self) -> u64 {
        self.gate_end
    }
    fn raw_end(&self) -> u128 {
        self.raw_end
    }
}

pub(super) fn admit_voice_density(
    intro: &[VoiceSpan],
    repeats: &[VoiceSpan],
    first_pass: u64,
    period: Option<u64>,
    release_raw: u64,
) -> Result<usize, MidiTemplateError> {
    admit_density(intro, repeats, first_pass, period, release_raw)
}

pub(super) fn admit_transport_loop_density(
    intro: &[DensitySpan],
    repeats: &[DensitySpan],
    first_pass: u64,
    period: u64,
    release_raw: u64,
) -> Result<usize, MidiTemplateError> {
    admit_density(intro, repeats, first_pass, Some(period), release_raw)
}

/// Held-only density of the same sources: every voice ends at its gate.
/// Instruments that steal releasing voices admit slots by this count, while
/// `admit_voice_density` (releases included) still sizes span buffers.
pub(super) fn admit_held_voice_density(
    intro: &[VoiceSpan],
    repeats: &[VoiceSpan],
    first_pass: u64,
    period: Option<u64>,
) -> Result<usize, MidiTemplateError> {
    admit_held_density(intro, repeats, first_pass, period)
}

/// Transport-loop counterpart of `admit_held_voice_density`.
pub(super) fn admit_held_transport_loop_density(
    intro: &[DensitySpan],
    repeats: &[DensitySpan],
    first_pass: u64,
    period: u64,
) -> Result<usize, MidiTemplateError> {
    admit_held_density(intro, repeats, first_pass, Some(period))
}

fn admit_held_density<S: DensityView>(
    intro: &[S],
    repeats: &[S],
    first_pass: u64,
    period: Option<u64>,
) -> Result<usize, MidiTemplateError> {
    let held = |spans: &[S]| -> Result<Vec<DensitySpan>, MidiTemplateError> {
        let mut held = Vec::new();
        held.try_reserve_exact(spans.len())
            .map_err(|_| MidiTemplateError::Allocation)?;
        held.extend(spans.iter().map(|span| DensitySpan {
            start: span.start(),
            gate_end: span.gate_end(),
            raw_end: u128::from(span.gate_end()),
        }));
        Ok(held)
    };
    admit_density(&held(intro)?, &held(repeats)?, first_pass, period, 0)
}

fn admit_density<S: DensityView>(
    intro: &[S],
    repeats: &[S],
    first_pass: u64,
    period: Option<u64>,
    release_raw: u64,
) -> Result<usize, MidiTemplateError> {
    let intro_max = maximum_linear_overlap(intro)?;
    let repeat_max = if let Some(period) = period {
        maximum_periodic_overlap(repeats, period)?
    } else {
        0
    };
    if intro_max > 64 || repeat_max > 64 {
        return Err(MidiTemplateError::TooManyVoices);
    }
    let first_tail_max = if let Some(period) = period {
        maximum_first_tail_overlap(intro, repeats, first_pass, period, release_raw)?
    } else {
        0
    };
    if first_tail_max > 64 {
        return Err(MidiTemplateError::TooManyVoices);
    }
    Ok(intro_max.max(repeat_max).max(first_tail_max))
}

#[derive(Clone, Copy)]
struct TailEvent {
    position: u128,
    is_start: bool,
}

/// Measures the finite, one-sided repeat tail while intro releases remain
/// active. Repeat occurrences before the first pass are intentionally absent.
fn maximum_first_tail_overlap<S: DensityView>(
    intro: &[S],
    repeats: &[S],
    first_pass: u64,
    period: u64,
    release_raw: u64,
) -> Result<usize, MidiTemplateError> {
    let first_pass = u128::from(first_pass);
    let release_raw = u128::from(release_raw);
    if intro
        .iter()
        .any(|span| span.start() < first_pass as u64 && u128::from(span.gate_end()) > first_pass)
    {
        return Err(MidiTemplateError::SpanAdmissionWorkBoundExceeded);
    }
    let horizon = intro
        .iter()
        .filter(|span| span.start() < first_pass as u64)
        .filter_map(|span| span.raw_end().checked_sub(first_pass))
        .filter(|tail| *tail > 0)
        .max()
        .unwrap_or(0);
    if horizon == 0 {
        return Ok(0);
    }
    if horizon > release_raw {
        return Err(MidiTemplateError::SpanAdmissionWorkBoundExceeded);
    }
    if repeats.is_empty() {
        return Ok(intro
            .iter()
            .filter(|span| span.start() < first_pass as u64 && span.raw_end() > first_pass)
            .count());
    }

    let period = u128::from(period);
    let full_cycles = release_raw / period;
    // Every repeated voice lasts at least release_raw. If that spans one or
    // more periods, each source note contributes at least full_cycles to the
    // steady-state baseline, which is already capped at 64. Therefore the
    // finite tail has at most N*(full_cycles+1) starts, bounded by 128. When
    // release_raw < period, each note starts at most once and N is capped by
    // the canonical 65,536-note source limit.
    let occurrence_limit = if full_cycles == 0 {
        repeats.len()
    } else {
        let cycles = usize::try_from(full_cycles)
            .map_err(|_| MidiTemplateError::SpanAdmissionWorkBoundExceeded)?;
        let max_notes = 64 / cycles;
        repeats
            .len()
            .min(max_notes)
            .checked_mul(cycles + 1)
            .ok_or(MidiTemplateError::SpanAdmissionWorkBoundExceeded)?
    };

    let mut intro_active = 0_usize;
    let mut events = Vec::new();
    let event_capacity = intro
        .len()
        .checked_add(
            occurrence_limit
                .checked_mul(2)
                .ok_or(MidiTemplateError::SpanAdmissionWorkBoundExceeded)?,
        )
        .ok_or(MidiTemplateError::SpanAdmissionWorkBoundExceeded)?;
    events
        .try_reserve_exact(event_capacity)
        .map_err(|_| MidiTemplateError::Allocation)?;

    for span in intro {
        if span.start() < first_pass as u64 && span.raw_end() > first_pass {
            intro_active += 1;
            let end = span.raw_end() - first_pass;
            if end < horizon {
                events.push(TailEvent {
                    position: end,
                    is_start: false,
                });
            }
        }
    }
    let mut occurrence_count = 0_usize;
    for span in repeats {
        let phase = u128::from(span.start());
        if phase >= period {
            return Err(MidiTemplateError::SpanAdmissionWorkBoundExceeded);
        }
        if phase >= horizon {
            continue;
        }
        let duration = span
            .raw_end()
            .checked_sub(u128::from(span.start()))
            .ok_or(MidiTemplateError::ArithmeticOverflow)?;
        if duration < release_raw {
            return Err(MidiTemplateError::SpanAdmissionWorkBoundExceeded);
        }
        let last_iteration = (horizon - 1 - phase) / period;
        let count = usize::try_from(last_iteration + 1)
            .map_err(|_| MidiTemplateError::SpanAdmissionWorkBoundExceeded)?;
        occurrence_count = occurrence_count
            .checked_add(count)
            .ok_or(MidiTemplateError::SpanAdmissionWorkBoundExceeded)?;
        if occurrence_count > occurrence_limit {
            return Err(MidiTemplateError::SpanAdmissionWorkBoundExceeded);
        }
        for iteration in 0..count {
            let start = phase
                .checked_add(
                    u128::try_from(iteration)
                        .map_err(|_| MidiTemplateError::SpanAdmissionWorkBoundExceeded)?
                        .checked_mul(period)
                        .ok_or(MidiTemplateError::SpanAdmissionWorkBoundExceeded)?,
                )
                .ok_or(MidiTemplateError::SpanAdmissionWorkBoundExceeded)?;
            let end = start
                .checked_add(duration)
                .ok_or(MidiTemplateError::SpanAdmissionWorkBoundExceeded)?;
            events.push(TailEvent {
                position: start,
                is_start: true,
            });
            if end < horizon {
                events.push(TailEvent {
                    position: end,
                    is_start: false,
                });
            }
        }
    }
    events.sort_unstable_by_key(|event| (event.position, event.is_start));
    let mut maximum = intro_active;
    for event in events {
        if event.position >= horizon {
            break;
        }
        if event.is_start {
            intro_active = intro_active
                .checked_add(1)
                .ok_or(MidiTemplateError::ArithmeticOverflow)?;
            maximum = maximum.max(intro_active);
        } else {
            intro_active = intro_active
                .checked_sub(1)
                .ok_or(MidiTemplateError::ArithmeticOverflow)?;
        }
    }
    Ok(maximum)
}

fn maximum_linear_overlap<S: DensityView>(spans: &[S]) -> Result<usize, MidiTemplateError> {
    let capacity = spans
        .len()
        .checked_mul(2)
        .ok_or(MidiTemplateError::ArithmeticOverflow)?;
    let mut points = Vec::new();
    points
        .try_reserve_exact(capacity)
        .map_err(|_| MidiTemplateError::Allocation)?;
    for span in spans {
        points.push((u128::from(span.start()), true));
        points.push((span.raw_end(), false));
    }
    points.sort_unstable_by_key(|(position, is_start)| (*position, *is_start));
    let mut active = 0_usize;
    let mut maximum = 0;
    for (_, is_start) in points {
        if is_start {
            active += 1;
            maximum = maximum.max(active);
        } else {
            active -= 1;
        }
    }
    Ok(maximum)
}

fn maximum_periodic_overlap<S: DensityView>(
    spans: &[S],
    period: u64,
) -> Result<usize, MidiTemplateError> {
    let p = u128::from(period);
    let mut baseline = 0_u128;
    let mut arcs = Vec::new();
    arcs.try_reserve_exact(spans.len())
        .map_err(|_| MidiTemplateError::Allocation)?;
    for span in spans {
        let length = u128::from(span.gate_end() - span.start())
            .checked_add(span.raw_end() - u128::from(span.gate_end()))
            .ok_or(MidiTemplateError::ArithmeticOverflow)?;
        baseline = baseline
            .checked_add(length / p)
            .ok_or(MidiTemplateError::ArithmeticOverflow)?;
        let residual = length % p;
        if residual > 0 {
            arcs.push((u128::from(span.start()), residual));
        }
    }
    let residual_max = maximum_cyclic_arc_overlap(&arcs, p)?;
    usize::try_from(
        baseline
            .checked_add(residual_max as u128)
            .ok_or(MidiTemplateError::ArithmeticOverflow)?,
    )
    .map_err(|_| MidiTemplateError::ArithmeticOverflow)
}

fn maximum_cyclic_arc_overlap(
    arcs: &[(u128, u128)],
    period: u128,
) -> Result<usize, MidiTemplateError> {
    if arcs.is_empty() {
        return Ok(0);
    }
    let mut points = Vec::new();
    let capacity = arcs
        .len()
        .checked_mul(4)
        .ok_or(MidiTemplateError::ArithmeticOverflow)?;
    points
        .try_reserve_exact(capacity)
        .map_err(|_| MidiTemplateError::Allocation)?;
    for (start, length) in arcs {
        let end = start + length;
        if end <= period {
            points.push((*start, true));
            points.push((end, false));
        } else {
            points.push((*start, true));
            points.push((period, false));
            points.push((0, true));
            points.push((end - period, false));
        }
    }
    points.sort_unstable_by_key(|(position, is_start)| (*position, *is_start));
    let mut active = 0_usize;
    let mut maximum = 0;
    for (_, is_start) in points {
        if is_start {
            active += 1;
            maximum = maximum.max(active);
        } else {
            active -= 1;
        }
    }
    Ok(maximum)
}

// Upper bound for the existing linear, periodic and first-tail temporary arrays.
// If the tail is shorter than a period, reserve two edges per repeat plus
// every intro. Longer tails admit at most128 repeat occurrences (256 edges).
pub(super) fn current_sampler_workspace_quote(
    notes: usize,
) -> Result<usize, crate::AudioStorageError> {
    use std::mem::size_of;
    notes
        .checked_mul(
            2 * size_of::<(u128, bool)>()
                + size_of::<(u128, u128)>()
                + 4 * size_of::<(u128, bool)>(),
        )
        .and_then(|n| {
            n.checked_add(
                notes
                    .checked_add(256)?
                    .checked_mul(size_of::<TailEvent>())?,
            )
        })
        .ok_or(crate::AudioStorageError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::{
        DensitySpan, admit_transport_loop_density, maximum_cyclic_arc_overlap,
        maximum_first_tail_overlap,
    };
    use crate::midi_template::spans::VoiceSpan;
    use hex_lattice::PreparedAnalyticNote;
    use hex_project::{BeatTime, Command, Created, Editor, NoteId, Project};

    fn span(
        start: u64,
        gate_end: u64,
        release: u64,
        note: NoteId,
        _voice: PreparedAnalyticNote,
    ) -> VoiceSpan {
        VoiceSpan {
            start,
            gate_end,
            raw_end: u128::from(gate_end + release),
            note,
            pitch: 60,
            channel: 0,
            velocity: 100,
            sampler_index: 0,
        }
    }

    fn test_voice() -> (NoteId, PreparedAnalyticNote) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let track = match editor
            .execute(Command::AddMidiTrack {
                name: "keys".into(),
            })
            .unwrap()
        {
            Created::Track(id) => id,
            _ => unreachable!(),
        };
        let clip = match editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        {
            Created::Clip(id) => id,
            _ => unreachable!(),
        };
        let note = match editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(1).unwrap(),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap()
        {
            Created::Note(id) => id,
            _ => unreachable!(),
        };
        let instrument = hex_devices::prepare_legacy_sine_instrument(48_000).unwrap();
        let voice = instrument.prepare_note(60, 100).unwrap();
        (note, voice)
    }

    fn brute(arcs: &[(u128, u128)], period: u128) -> usize {
        (0..period)
            .map(|point| {
                arcs.iter()
                    .filter(|(start, length)| (point + period - start) % period < *length)
                    .count()
            })
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn half_open_periodic_residual_arcs_match_tiny_domain_oracle() {
        for period in 2..=9_u128 {
            for mask in 0_u16..512 {
                let mut arcs = Vec::new();
                for start in 0..period {
                    for length in 1..period {
                        let bit = ((start * 11 + length * 7) % 9) as u32;
                        if (mask >> bit) & 1 != 0 {
                            arcs.push((start, length));
                        }
                    }
                }
                assert_eq!(
                    maximum_cyclic_arc_overlap(&arcs, period).unwrap(),
                    brute(&arcs, period)
                );
            }
        }
    }

    #[test]
    fn zero_residual_does_not_add_a_phantom_endpoint_voice() {
        assert_eq!(maximum_cyclic_arc_overlap(&[], 8).unwrap(), 0);
        assert_eq!(maximum_cyclic_arc_overlap(&[(7, 1)], 8).unwrap(), 1);
        assert_eq!(maximum_cyclic_arc_overlap(&[(0, 1), (0, 1)], 8).unwrap(), 2);
    }

    #[test]
    fn finite_first_tail_overlap_matches_independent_tiny_sweep() {
        let (note, voice) = test_voice();
        for period in 2_u64..=7 {
            for release in 1_u64..=8 {
                for intro_count in 0_u64..=3 {
                    for repeat_count in 0_u64..=3 {
                        let first_pass = 16;
                        let intro = (0..intro_count)
                            .map(|index| {
                                let gate_end = first_pass - (index % release);
                                span(gate_end - 1, gate_end, release, note, voice)
                            })
                            .collect::<Vec<_>>();
                        let repeats = (0..repeat_count)
                            .map(|index| {
                                let phase = index % period;
                                span(phase, phase + 1, release, note, voice)
                            })
                            .collect::<Vec<_>>();
                        let horizon = intro
                            .iter()
                            .map(|item| item.raw_end - u128::from(first_pass))
                            .max()
                            .unwrap_or(0);
                        let expected = (0..horizon)
                            .map(|time| {
                                let active_intro = intro
                                    .iter()
                                    .filter(|item| {
                                        item.start < first_pass
                                            && u128::from(first_pass) + time < item.raw_end
                                    })
                                    .count();
                                let active_repeats = repeats
                                    .iter()
                                    .flat_map(|item| {
                                        let phase = u128::from(item.start);
                                        let duration = item.raw_end - u128::from(item.start);
                                        (0..=time / u128::from(period)).map(move |iteration| {
                                            let start = phase + iteration * u128::from(period);
                                            (start <= time && time < start + duration) as usize
                                        })
                                    })
                                    .sum::<usize>();
                                active_intro + active_repeats
                            })
                            .max()
                            .unwrap_or(0);
                        assert_eq!(
                            maximum_first_tail_overlap(
                                &intro, &repeats, first_pass, period, release,
                            )
                            .unwrap(),
                            expected,
                            "period={period}, release={release}, intro={intro_count}, repeats={repeat_count}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn first_tail_sweep_refuses_more_occurrences_than_the_proved_bound() {
        let (note, voice) = test_voice();
        let intro = [span(0, 1, 100, note, voice)];
        let repeats = [span(0, 1, 100, note, voice)];
        assert_eq!(
            maximum_first_tail_overlap(&intro, &repeats, 1, 1, 100),
            Err(crate::midi_template::MidiTemplateError::SpanAdmissionWorkBoundExceeded)
        );
    }

    #[test]
    fn transport_loop_first_seam_includes_intro_release_voices() {
        let intro = [DensitySpan {
            start: 0,
            gate_end: 20,
            raw_end: 25,
        }];
        let repeats = [DensitySpan {
            start: 0,
            gate_end: 10,
            raw_end: 20,
        }];
        assert_eq!(
            admit_transport_loop_density(&intro, &repeats, 20, 100, 10).unwrap(),
            2
        );
    }
}
