//! Timeline musical ruler ticks, meter sections and landmark candidates.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RulerTick {
    pub(super) beat: BeatTime,
    pub frame: u64,
    pub label: String,
    pub meter_marker: bool,
    pub fragmentary_bar: bool,
    pub bar_start: bool,
}

pub fn ruler_ticks(project: &Project, viewport: Viewport, pixel_width: f32) -> Vec<RulerTick> {
    if viewport.length == 0 || !pixel_width.is_finite() || pixel_width <= 0.0 {
        return Vec::new();
    }
    let Some(viewport_end) = viewport.end() else {
        return Vec::new();
    };
    let last_frame = viewport_end.saturating_sub(1);
    let tempo = project.tempo_map();
    let Ok(start_beat) = tempo.frame_to_beat(viewport.start) else {
        return Vec::new();
    };
    // A frame viewport can extend beyond BeatTime's representable range. Clamp
    // its far edge so valid landmarks and markers earlier in the viewport remain.
    let last_beat = tempo
        .frame_to_beat(last_frame)
        .unwrap_or_else(|_| BeatTime::from_raw(u64::MAX));
    let target_count = ((pixel_width / RULER_LABEL_SPACING_PX).floor() as usize)
        .saturating_add(1)
        .clamp(1, MAX_RULER_TICKS);
    let resolution =
        AdaptiveGridResolution::new(project, viewport, pixel_width, RULER_LABEL_SPACING_PX, true);
    let ruler_candidates = ruler_candidates(
        project,
        viewport,
        viewport_end,
        start_beat,
        last_beat,
        target_count,
        LandmarkDensity::Ruler(&resolution),
    );
    let mut candidates = ruler_candidates.ticks;
    candidates.sort_by_key(|tick| tick.frame);
    candidates.dedup_by(|later, earlier| {
        if later.frame == earlier.frame {
            earlier.meter_marker |= later.meter_marker;
            earlier.fragmentary_bar |= later.fragmentary_bar;
            if later.meter_marker {
                earlier.label.clone_from(&later.label);
            }
            true
        } else {
            false
        }
    });

    let limit = target_count.min(MAX_RULER_TICKS);
    let mut bounded = Vec::with_capacity(limit);
    for tick in candidates.iter().filter(|tick| tick.meter_marker) {
        let x = (tick.frame - viewport.start) as f32 / viewport.length as f32 * pixel_width;
        if bounded.iter().any(|selected: &RulerTick| {
            let selected_x =
                (selected.frame - viewport.start) as f32 / viewport.length as f32 * pixel_width;
            (x - selected_x).abs() < RULER_LABEL_SPACING_PX
        }) {
            continue;
        }
        if bounded.len() >= limit {
            break;
        }
        bounded.push(tick.clone());
    }
    for tick in candidates.iter().filter(|tick| !tick.meter_marker) {
        let x = (tick.frame - viewport.start) as f32 / viewport.length as f32 * pixel_width;
        if bounded.iter().any(|selected: &RulerTick| {
            let selected_x =
                (selected.frame - viewport.start) as f32 / viewport.length as f32 * pixel_width;
            (x - selected_x).abs() < RULER_LABEL_SPACING_PX
        }) {
            continue;
        }
        if bounded.len() >= limit {
            break;
        }
        bounded.push(tick.clone());
    }
    bounded.sort_by_key(|tick| tick.frame);
    bounded
}

#[derive(Clone, Copy)]
pub(super) struct MeterSection {
    point_beat: BeatTime,
    beat_length: u64,
    first_landmark: u64,
    landmark_count: u64,
    regular_demand: u64,
}

pub(super) struct RulerCandidates {
    pub(super) ticks: Vec<RulerTick>,
    #[cfg(test)]
    pub(super) landmark_attempts: usize,
    #[cfg(test)]
    pub(super) marker_attempts: usize,
}

#[derive(Clone, Copy)]
pub(super) enum LandmarkDensity<'a> {
    Meter,
    Adaptive(&'a AdaptiveGridResolution<'a>),
    Ruler(&'a AdaptiveGridResolution<'a>),
    #[cfg(any(test, feature = "test-support"))]
    Fixed(u64),
}

// Max-min fair sharing of the existing small paint budget. Every sum is
// capped at budget+1: enormous raw spans cannot overflow or affect the quota.
// The inclusive level domain 0..=256 needs at most NINE midpoint probes, plus
// one final bounded sum. No heap storage or historical-section walk is used.
pub(super) fn capped_fair_share(
    count: usize,
    budget: usize,
    demand: impl Fn(usize) -> usize,
) -> (usize, usize) {
    #[cfg(test)]
    record_grid_work(2);
    let budget = budget.min(MAX_GRID_LINES);
    if count == 0 || budget == 0 {
        return (0, 0);
    }
    let cost = |level| {
        (0..count).fold(0_usize, |sum, index| {
            sum.saturating_add(demand(index).min(level)).min(budget + 1)
        })
    };
    let (mut low, mut high) = (0, budget);
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if cost(middle) <= budget {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    (low, budget - cost(low))
}

pub(super) fn ruler_candidates(
    project: &Project,
    viewport: Viewport,
    viewport_end: u64,
    start_beat: BeatTime,
    last_beat: BeatTime,
    target_count: usize,
    target_grid: LandmarkDensity<'_>,
) -> RulerCandidates {
    let tempo = project.tempo_map();
    let meters = project.meter_map();
    let points = meters.points();
    let mut sections = Vec::with_capacity(points.len().min(MAX_METER_MAP_POINTS));
    let mut ticks = Vec::with_capacity(target_count.saturating_add(points.len()));
    for (index, point) in points.iter().take(MAX_METER_MAP_POINTS).enumerate() {
        let point_raw = point.beat().raw();
        let section_end = points.get(index + 1).map(|next| next.beat().raw());
        if point_raw <= last_beat.raw() && section_end.is_none_or(|end| end > start_beat.raw()) {
            let effective_start = start_beat.raw().max(point_raw);
            let effective_end = section_end
                .and_then(|end| end.checked_sub(1))
                .unwrap_or(last_beat.raw())
                .min(last_beat.raw());
            if effective_start <= effective_end {
                let meter_beat_length =
                    (4_u64 * BeatTime::ONE.raw()) / u64::from(point.meter().denominator());
                let beat_length = match target_grid {
                    LandmarkDensity::Adaptive(resolution) | LandmarkDensity::Ruler(resolution) => {
                        resolution.step(point.beat(), meter_beat_length)
                    }
                    #[cfg(any(test, feature = "test-support"))]
                    LandmarkDensity::Fixed(step) => step,
                    LandmarkDensity::Meter => meter_beat_length,
                };
                if beat_length > 0 {
                    let first_landmark = effective_start
                        .saturating_sub(point_raw)
                        .div_ceil(beat_length);
                    let last_landmark = effective_end.saturating_sub(point_raw) / beat_length;
                    if first_landmark <= last_landmark {
                        sections.push(MeterSection {
                            point_beat: point.beat(),
                            beat_length,
                            first_landmark,
                            landmark_count: last_landmark - first_landmark + 1,
                            // Count conservatively across any viewport phase so
                            // crossing a candidate edge does not change stride.
                            regular_demand: ((effective_end - effective_start) / beat_length)
                                .saturating_add(1),
                        });
                    }
                }
            }
        }
        // Markers are considered even when sub-quarter or quantized frame
        // positions make inverse lookup ambiguous.
        if index > 0
            && let Ok(frame) = tempo.beat_to_frame(point.beat())
            && (viewport.start..viewport_end).contains(&frame)
            && let Ok(position) = meters.position(point.beat())
        {
            let truncated_prior_bar = point
                .beat()
                .raw()
                .checked_sub(1)
                .map(BeatTime::from_raw)
                .and_then(|before| meters.position(before).ok())
                .is_some_and(|before| {
                    before.fragmentary() && before.bar_end() == Some(point.beat())
                });
            ticks.push(RulerTick {
                beat: point.beat(),
                frame,
                label: meter_label(position),
                meter_marker: true,
                fragmentary_bar: truncated_prior_bar,
                bar_start: position.beat() == 1 && position.beat_fraction().raw() == 0,
            });
        }
    }

    let section_count = sections.len();
    let sampled_sections = section_count.min(target_count);
    // If all coarsened landmarks fit, preserve them all. Otherwise unused
    // short-section slots belong to still-unsatisfied sections, in the same
    // deterministic order as adaptive coarsening. Over-budget section sampling
    // below retains its existing evenly spaced section identities.
    let demand = |index: usize| {
        let count = if matches!(target_grid, LandmarkDensity::Ruler(_)) {
            sections[index].regular_demand
        } else {
            sections[index].landmark_count
        };
        count.min((target_count.min(MAX_GRID_LINES) + 1) as u64) as usize
    };
    let (level, mut remainder) = if section_count <= target_count {
        capped_fair_share(section_count, target_count, demand)
    } else {
        (0, 0)
    };
    #[cfg(test)]
    let mut landmark_attempts = 0;
    if sampled_sections > 0 {
        for sampled_index in 0..sampled_sections {
            // Spread the quota evenly across visible meter sections. This keeps a
            // long, dense section from consuming the ruler's whole tick budget.
            let section_index = if sampled_sections == section_count {
                sampled_index
            } else if sampled_sections == 1 {
                section_count / 2
            } else {
                sampled_index * (section_count - 1) / (sampled_sections - 1)
            };
            let Some(section) = sections.get(section_index).copied() else {
                continue;
            };
            let quota = if section_count <= target_count {
                let wanted = demand(section_index);
                let extra = usize::from(wanted > level && remainder > 0);
                remainder -= extra;
                wanted.min(level) + extra
            } else {
                target_count / sampled_sections
                    + usize::from(sampled_index < target_count % sampled_sections)
            };
            let regular = matches!(target_grid, LandmarkDensity::Ruler(_));
            let quota = if regular {
                quota as u64
            } else {
                (quota as u64).min(section.landmark_count)
            };
            if quota == 0 {
                continue;
            }
            // Only the ruler thins to an origin-anchored stride. Grid and snap
            // consumers retain their existing visible-candidate sampling.
            let stride = section
                .regular_demand
                .div_ceil(quota)
                .checked_next_power_of_two()
                .unwrap_or(u64::MAX);
            let last = section
                .first_landmark
                .saturating_add(section.landmark_count - 1);
            let first = section
                .first_landmark
                .div_ceil(stride)
                .checked_mul(stride)
                .filter(|first| *first <= last)
                .unwrap_or(section.first_landmark);
            for sample in 0..quota {
                let landmark_index = if regular {
                    let Some(index) = sample
                        .checked_mul(stride)
                        .and_then(|offset| first.checked_add(offset))
                    else {
                        break;
                    };
                    if index > last {
                        break;
                    }
                    index
                } else {
                    let offset = if quota <= 1 {
                        0
                    } else {
                        let Ok(offset) = u64::try_from(
                            u128::from(sample) * u128::from(section.landmark_count - 1)
                                / u128::from(quota - 1),
                        ) else {
                            continue;
                        };
                        offset
                    };
                    let Some(index) = section.first_landmark.checked_add(offset) else {
                        continue;
                    };
                    index
                };
                let raw_beat = u128::from(section.point_beat.raw())
                    + u128::from(landmark_index) * u128::from(section.beat_length);
                let Ok(raw_beat) = u64::try_from(raw_beat) else {
                    continue;
                };
                let beat = BeatTime::from_raw(raw_beat);
                #[cfg(test)]
                {
                    landmark_attempts += 1;
                }
                if let (Ok(frame), Ok(position)) =
                    (tempo.beat_to_frame(beat), meters.position(beat))
                    && (viewport.start..viewport_end).contains(&frame)
                {
                    ticks.push(RulerTick {
                        beat,
                        frame,
                        label: meter_label(position),
                        meter_marker: false,
                        fragmentary_bar: position.fragmentary(),
                        bar_start: position.beat() == 1 && position.beat_fraction().raw() == 0,
                    });
                }
            }
        }
    }

    RulerCandidates {
        ticks,
        #[cfg(test)]
        landmark_attempts,
        #[cfg(test)]
        marker_attempts: points.len().min(MAX_METER_MAP_POINTS),
    }
}

// Only visible sections can overflow the painted landmark quota. Historical
// meter points outside the viewport do not justify allocating a paint list on
// each snap/nudge. Binary searches preserve the current map/tempo semantics.
pub(super) fn visible_meter_sections(project: &Project, viewport: Viewport) -> Option<usize> {
    if viewport.length == 0 {
        return None;
    }
    let tempo = project.tempo_map();
    let first = tempo.frame_to_beat(viewport.start).ok()?;
    let last = tempo
        .frame_to_beat(viewport.end()?.checked_sub(1)?)
        .unwrap_or_else(|_| BeatTime::from_raw(u64::MAX));
    let points = project.meter_map().points();
    let start = points
        .partition_point(|point| point.beat() <= first)
        .saturating_sub(1);
    let end = points.partition_point(|point| point.beat() <= last);
    Some(end.saturating_sub(start).max(1))
}

pub(super) fn meter_label(position: MeterPosition) -> String {
    let fraction = position.beat_fraction().raw();
    if fraction == 0 && position.beat() == 1 {
        position.bar().to_string()
    } else if fraction == 0 {
        format!("{}.{}", position.bar(), position.beat())
    } else {
        let tenths = (u128::from(fraction) * 10 / u128::from(BeatTime::ONE.raw())) as u8;
        format!("{}.{}.{}", position.bar(), position.beat(), tenths)
    }
}
