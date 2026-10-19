//! UI-only snap selection and bounded musical timeline helpers.

#[path = "adaptive_grid_density.rs"]
mod density;
pub(super) use density::AdaptiveGridDensity;

use super::viewport::Viewport;
use hex_project::{BeatTime, Command, Meter, MeterPosition, Project};

const MAX_RULER_TICKS: usize = 64;
const MAX_METER_MAP_POINTS: usize = 1_024;
const MAX_GRID_LINES: usize = 256;
const MAX_GRID_BANDS: usize = 256;
const RULER_LABEL_SPACING_PX: f32 = 48.0;

#[cfg(test)]
thread_local! {
    // Actual resolver work per test thread: step/base/share/natural demand.
    static GRID_RESOLUTION_WORK: std::cell::Cell<[usize; 4]> = const { std::cell::Cell::new([0; 4]) };
}

#[cfg(test)]
fn record_grid_work(kind: usize) {
    GRID_RESOLUTION_WORK.with(|work| {
        let mut counts = work.get();
        counts[kind] += 1;
        work.set(counts);
    });
}

fn adaptive_grid_spacing(pixel_width: f32, density: AdaptiveGridDensity) -> f32 {
    density
        .minimum_spacing()
        .max(pixel_width / (MAX_GRID_LINES.saturating_sub(1) as f32))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RulerTick {
    beat: BeatTime,
    pub(super) frame: u64,
    pub(super) label: String,
    pub(super) meter_marker: bool,
    pub(super) fragmentary_bar: bool,
    pub(super) bar_start: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GridLine {
    beat: BeatTime,
    pub(super) frame: u64,
    pub(super) bar_start: bool,
    pub(super) meter_marker: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GridBand {
    pub(super) start_frame: u64,
    pub(super) end_frame: u64,
    pub(super) alternate: bool,
}

/// Build alternating background bands spanning four adaptive grid units.
/// Band indices are anchored to each meter segment's musical origin, so panning
/// changes the visible subset without moving the pattern.
#[cfg(test)]
pub(super) fn adaptive_grid_bands(
    project: &Project,
    viewport: Viewport,
    pixel_width: f32,
) -> Vec<GridBand> {
    adaptive_grid_bands_for_grid(project, viewport, pixel_width, SnapGrid::Adaptive)
}

pub(super) fn adaptive_grid_bands_for_grid(
    project: &Project,
    viewport: Viewport,
    pixel_width: f32,
    grid: SnapGrid,
) -> Vec<GridBand> {
    if viewport.length == 0 || !pixel_width.is_finite() || pixel_width <= 0.0 {
        return Vec::new();
    }
    let Some(viewport_end) = viewport.end() else {
        return Vec::new();
    };
    let tempo = project.tempo_map();
    let Ok(first_visible_beat) = tempo.frame_to_beat(viewport.start) else {
        return Vec::new();
    };
    let last_visible_beat = tempo
        .frame_to_beat(viewport_end.saturating_sub(1))
        .unwrap_or_else(|_| BeatTime::from_raw(u64::MAX));
    let points = project.meter_map().points();
    let first_segment = points.partition_point(|point| point.beat() <= first_visible_beat);
    let first_segment = first_segment.saturating_sub(1);
    let mut bands = Vec::with_capacity(32);
    let mut candidates_considered = 0;
    'sections: for (index, point) in points
        .iter()
        .enumerate()
        .skip(first_segment)
        .take(MAX_METER_MAP_POINTS)
    {
        if candidates_considered >= MAX_GRID_BANDS || point.beat() > last_visible_beat {
            break;
        }
        let origin = point.beat().raw();
        let section_end = points.get(index + 1).map(|next| next.beat().raw());
        if section_end.is_some_and(|end| end <= first_visible_beat.raw()) {
            continue;
        }
        let beat_length = (4_u64 * BeatTime::ONE.raw()) / u64::from(point.meter().denominator());
        let step = adaptive_grid_step(
            project,
            viewport,
            point.beat(),
            beat_length,
            pixel_width,
            adaptive_grid_spacing(pixel_width, grid.density()),
        );
        let Some(band_span) = step.checked_mul(4).filter(|span| *span > 0) else {
            continue;
        };
        let first = first_visible_beat.raw().max(origin).saturating_sub(origin) / band_span;
        let visible_end_raw = last_visible_beat.raw().min(
            section_end
                .and_then(|end| end.checked_sub(1))
                .unwrap_or(u64::MAX),
        );
        let last = visible_end_raw.saturating_sub(origin) / band_span;
        if last < first {
            continue;
        }
        for band_index in bounded_band_indices(
            first,
            last,
            MAX_GRID_BANDS.saturating_sub(candidates_considered),
        ) {
            candidates_considered += 1;
            if bands.len() >= MAX_GRID_BANDS {
                break 'sections;
            }
            let Some(offset) = band_index.checked_mul(band_span) else {
                continue;
            };
            let Some(start_raw) = origin.checked_add(offset) else {
                continue;
            };
            let Some(end_raw) = start_raw.checked_add(band_span) else {
                continue;
            };
            let end_raw = section_end.map_or(end_raw, |section| end_raw.min(section));
            let (Ok(start_frame), Ok(end_frame)) = (
                tempo.beat_to_frame(BeatTime::from_raw(start_raw)),
                tempo.beat_to_frame(BeatTime::from_raw(end_raw)),
            ) else {
                continue;
            };
            if start_frame < viewport_end && end_frame > viewport.start && start_frame < end_frame {
                bands.push(GridBand {
                    start_frame,
                    end_frame,
                    alternate: band_index % 2 == 1,
                });
            }
        }
    }
    bands
}

fn bounded_band_indices(first: u64, last: u64, budget: usize) -> impl Iterator<Item = u64> {
    (first..=last).take(budget)
}

#[cfg(test)]
thread_local! {
    // Per-test-thread observation of actual paint-list construction, not a
    // process-wide setting or replacement resolver.
    static GRID_LINE_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(super) fn grid_lines(
    project: &Project,
    viewport: Viewport,
    pixel_width: f32,
    grid: SnapGrid,
) -> Vec<GridLine> {
    #[cfg(test)]
    GRID_LINE_BUILDS.with(|count| count.set(count.get() + 1));
    if grid.is_off() {
        return Vec::new();
    }
    if viewport.length == 0 || !pixel_width.is_finite() || pixel_width <= 0.0 {
        return Vec::new();
    }
    let Some(viewport_end) = viewport.end() else {
        return Vec::new();
    };
    let tempo = project.tempo_map();
    let Ok(start_beat) = tempo.frame_to_beat(viewport.start) else {
        return Vec::new();
    };
    let last_beat = tempo
        .frame_to_beat(viewport_end.saturating_sub(1))
        .unwrap_or_else(|_| BeatTime::from_raw(u64::MAX));
    // Adaptive spacing sets a readable minimum, while this larger candidate
    // quota preserves every visible landmark instead of sampling away lines
    // that snapping may legitimately choose.
    let target_count = MAX_GRID_LINES;
    let mut candidates = ruler_candidates(
        project,
        viewport,
        viewport_end,
        start_beat,
        last_beat,
        target_count,
        grid.landmark_density(pixel_width),
    )
    .ticks;
    candidates.sort_by_key(|tick| tick.frame);
    candidates.dedup_by(|later, earlier| {
        if later.frame == earlier.frame {
            earlier.meter_marker |= later.meter_marker;
            earlier.fragmentary_bar |= later.fragmentary_bar;
            earlier.bar_start |= later.bar_start;
            true
        } else {
            false
        }
    });
    // Fill the available quota across the complete visible candidate set.
    // Ceil-striding 257 candidates by two would unnecessarily paint only129.
    let count = candidates.len().min(MAX_GRID_LINES);
    (0..count)
        .map(|sample| {
            let index = if count <= 1 {
                0
            } else {
                sample * (candidates.len() - 1) / (count - 1)
            };
            &candidates[index]
        })
        .map(|tick| GridLine {
            beat: tick.beat,
            frame: tick.frame,
            bar_start: tick.bar_start,
            meter_marker: tick.meter_marker,
        })
        .collect()
}

pub(super) fn ruler_ticks(
    project: &Project,
    viewport: Viewport,
    pixel_width: f32,
) -> Vec<RulerTick> {
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
    let ruler_candidates = ruler_candidates(
        project,
        viewport,
        viewport_end,
        start_beat,
        last_beat,
        target_count,
        LandmarkDensity::Adaptive(pixel_width, RULER_LABEL_SPACING_PX),
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
struct MeterSection {
    point_beat: BeatTime,
    beat_length: u64,
    first_landmark: u64,
    landmark_count: u64,
}

struct RulerCandidates {
    ticks: Vec<RulerTick>,
    #[cfg(test)]
    landmark_attempts: usize,
    #[cfg(test)]
    marker_attempts: usize,
}

#[derive(Clone, Copy)]
enum LandmarkDensity {
    Meter,
    Adaptive(f32, f32),
    #[cfg(test)]
    Fixed(u64),
}

// Max-min fair sharing of the existing small paint budget. Every sum is
// capped at budget+1: enormous raw spans cannot overflow or affect the quota.
// The inclusive level domain 0..=256 needs at most NINE midpoint probes, plus
// one final bounded sum. No heap storage or historical-section walk is used.
fn capped_fair_share(
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

fn ruler_candidates(
    project: &Project,
    viewport: Viewport,
    viewport_end: u64,
    start_beat: BeatTime,
    last_beat: BeatTime,
    target_count: usize,
    target_grid: LandmarkDensity,
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
                    LandmarkDensity::Adaptive(pixel_width, spacing) => adaptive_grid_step(
                        project,
                        viewport,
                        point.beat(),
                        meter_beat_length,
                        pixel_width,
                        spacing,
                    ),
                    #[cfg(test)]
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
        sections[index]
            .landmark_count
            .min((target_count.min(MAX_GRID_LINES) + 1) as u64) as usize
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
            let quota = (quota as u64).min(section.landmark_count);
            for sample in 0..quota {
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
                let Some(landmark_index) = section.first_landmark.checked_add(offset) else {
                    continue;
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
fn visible_meter_sections(project: &Project, viewport: Viewport) -> Option<usize> {
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

fn adaptive_base_step(
    project: &Project,
    viewport: Viewport,
    beat: BeatTime,
    meter_beat_length: u64,
    pixel_width: f32,
    target_spacing: f32,
) -> u64 {
    #[cfg(test)]
    record_grid_work(1);
    if meter_beat_length == 0
        || viewport.length == 0
        || !pixel_width.is_finite()
        || pixel_width <= 0.0
        || !target_spacing.is_finite()
    {
        return meter_beat_length.max(1);
    }
    let tempo = project.tempo_map().tempo_at(beat).micro_bpm();
    let frames_per_quarter = f64::from(project.sample_rate()) * 60_000_000.0 / f64::from(tempo);
    let pixels_per_quarter = f64::from(pixel_width) * frames_per_quarter / viewport.length as f64;
    let mut step = meter_beat_length;
    let mut pixels = pixels_per_quarter * meter_beat_length as f64 / BeatTime::ONE.raw() as f64;
    let target = f64::from(target_spacing.max(1.0));
    let mut divisions = 1;
    while pixels > target * 1.5 && divisions < 16 {
        step = (step / 2).max(1);
        pixels *= 0.5;
        divisions *= 2;
    }
    let mut multiplier = 1_u64;
    while pixels < target / 1.5 && multiplier < 64 {
        step = step.saturating_mul(2);
        pixels *= 2.0;
        multiplier *= 2;
    }
    step.max(1)
}

fn adaptive_grid_step(
    project: &Project,
    viewport: Viewport,
    beat: BeatTime,
    meter_beat_length: u64,
    pixel_width: f32,
    target_spacing: f32,
) -> u64 {
    #[cfg(test)]
    record_grid_work(0);
    if meter_beat_length == 0
        || viewport.length == 0
        || !pixel_width.is_finite()
        || pixel_width <= 0.0
        || !target_spacing.is_finite()
    {
        return meter_beat_length.max(1);
    }
    let mut step = adaptive_base_step(
        project,
        viewport,
        beat,
        meter_beat_length,
        pixel_width,
        target_spacing,
    );
    // Coarsen the SAME step for drawing and snapping before any paint quota
    // could sample away regular landmarks. Use exact visible beat spans, which
    // also cover tempo changes within this meter section.
    let tempo_map = project.tempo_map();
    if let (Ok(first), Some(last)) = (
        tempo_map.frame_to_beat(viewport.start),
        viewport.end().map(|end| {
            tempo_map
                .frame_to_beat(end.saturating_sub(1))
                .unwrap_or_else(|_| BeatTime::from_raw(u64::MAX))
        }),
    ) {
        let points = project.meter_map().points();
        let first_section = points
            .partition_point(|point| point.beat() <= first)
            .saturating_sub(1);
        let last_section = points.partition_point(|point| point.beat() <= last);
        let sections = last_section.saturating_sub(first_section).max(1);
        // An internal meter marker is the section's zero landmark: reserving
        // it twice can discard half the usable subdivision budget. Only frame
        // quantization at either viewport edge can admit a marker whose beat
        // lies outside the regular landmark span. Binary searches above bound
        // those exceptions to the first/next origins (one frame at each edge).
        let extra_markers = usize::from(points.get(first_section).is_some_and(|point| {
            point.beat() < first
                && tempo_map
                    .beat_to_frame(point.beat())
                    .is_ok_and(|frame| frame >= viewport.start)
        })) + usize::from(points.get(last_section).is_some_and(|point| {
            tempo_map
                .beat_to_frame(point.beat())
                .is_ok_and(|frame| viewport.end().is_some_and(|end| frame < end))
        }));
        // Larger-than-paint-budget maps still share the final bounded painted
        // identities for snapping; no hidden extra subdivision is introduced.
        let section = points
            .partition_point(|point| point.beat() <= beat)
            .saturating_sub(1);
        let available = MAX_GRID_LINES.saturating_sub(extra_markers);
        let quota = if sections <= available && (first_section..last_section).contains(&section) {
            // Compute natural demand BEFORE coarsening. Only binary-bounded
            // visible spans participate; short sections cannot strand slots.
            let demand = |relative: usize| {
                #[cfg(test)]
                record_grid_work(3);
                let index = first_section + relative;
                let Some(point) = points.get(index) else {
                    return 0;
                };
                let start = first.raw().max(point.beat().raw());
                let end = points
                    .get(index + 1)
                    .and_then(|next| next.beat().raw().checked_sub(1))
                    .unwrap_or(last.raw())
                    .min(last.raw());
                if start > end {
                    return 0;
                }
                let meter_length =
                    (4_u64 * BeatTime::ONE.raw()) / u64::from(point.meter().denominator());
                let base = adaptive_base_step(
                    project,
                    viewport,
                    point.beat(),
                    meter_length,
                    pixel_width,
                    target_spacing,
                );
                ((u128::from(end - start) / u128::from(base) + 1).min((available + 1) as u128))
                    as usize
            };
            let (level, remainder) = capped_fair_share(sections, available, demand);
            let relative = section - first_section;
            let earlier_unsatisfied = (0..relative).filter(|index| demand(*index) > level).count();
            demand(relative).min(level)
                + usize::from(demand(relative) > level && earlier_unsatisfied < remainder)
        } else {
            // Preserve dense-map fallback and out-of-viewport neighbor behavior.
            available / sections
                + usize::from(section.saturating_sub(first_section) < available % sections)
        }
        .max(1) as u64;
        let start = first.raw().max(beat.raw());
        let end = points
            .get(section + 1)
            .and_then(|point| point.beat().raw().checked_sub(1))
            .unwrap_or(last.raw())
            .min(last.raw());
        if start <= end {
            loop {
                // Bound the maximum number across ANY alignment of this same
                // visible span. Counting the current phase makes a one-frame
                // pan flip between255 and256 points and halve the subdivision.
                let count = u128::from(end - start) / u128::from(step.max(1)) + 1;
                if count <= u128::from(quota) || step == u64::MAX {
                    break;
                }
                step = step.saturating_mul(2);
            }
        }
    }
    step.max(1)
}

fn snap_to_meter_landmark(
    project: &Project,
    beat: BeatTime,
    viewport: Viewport,
    pixel_width: f32,
    grid: SnapGrid,
) -> Option<BeatTime> {
    if grid.is_off() {
        return Some(beat);
    }
    let points = project.meter_map().points();
    let insertion = points.partition_point(|point| point.beat() <= beat);
    if insertion == 0 {
        return None;
    }
    let current = insertion - 1;
    let first = current.saturating_sub(1);
    let last = current
        .saturating_add(1)
        .min(points.len().saturating_sub(1));
    let mut best: Option<(u64, u64)> = None;
    for index in first..=last {
        let point = points.get(index)?;
        let origin = point.beat().raw();
        let section_end = points.get(index + 1).map(|next| next.beat().raw());
        let meter_beat_length =
            (4_u64 * BeatTime::ONE.raw()) / u64::from(point.meter().denominator());
        let step = match grid {
            SnapGrid::Adaptive | SnapGrid::AdaptiveDensity(_) => adaptive_grid_step(
                project,
                viewport,
                point.beat(),
                meter_beat_length,
                pixel_width,
                adaptive_grid_spacing(pixel_width, grid.density()),
            ),
            SnapGrid::Off | SnapGrid::OffDensity(_) => return Some(beat),
            #[cfg(test)]
            fixed => fixed.step_raw()?,
        }
        .max(1);
        let lower_index = beat.raw().saturating_sub(origin) / step;
        for landmark_index in [lower_index, lower_index.saturating_add(1)] {
            let Some(raw) = origin.checked_add(landmark_index.checked_mul(step)?) else {
                continue;
            };
            if section_end.is_some_and(|end| raw >= end) {
                continue;
            }
            let distance = raw.abs_diff(beat.raw());
            if best.is_none_or(|(best_distance, best_raw)| {
                distance < best_distance || (distance == best_distance && raw > best_raw)
            }) {
                best = Some((distance, raw));
            }
        }
    }
    best.map(|(_, raw)| BeatTime::from_raw(raw))
}

/// Directional grid lookup shares the painted meter origins and adaptive density.
/// Audio compares converted frames, so rounded landmarks must still be distinct.
fn adjacent_landmark(
    project: &Project,
    beat: BeatTime,
    frame: Option<u64>,
    viewport: Viewport,
    width: f32,
    grid: SnapGrid,
    right: bool,
) -> Option<BeatTime> {
    if grid.is_off() || !width.is_finite() || width <= 0.0 || viewport.length == 0 {
        return None;
    }
    let mut best: Option<(u64, BeatTime)> = None;
    let points = project.meter_map().points();
    for (index, point) in points.iter().take(MAX_METER_MAP_POINTS).enumerate() {
        let origin = point.beat().raw();
        let end = points.get(index + 1).map(|point| point.beat().raw());
        let step = match grid {
            SnapGrid::Adaptive | SnapGrid::AdaptiveDensity(_) => adaptive_grid_step(
                project,
                viewport,
                point.beat(),
                4 * BeatTime::ONE.raw() / u64::from(point.meter().denominator()),
                width,
                adaptive_grid_spacing(width, grid.density()),
            ),
            SnapGrid::Off | SnapGrid::OffDensity(_) => return None,
            #[cfg(test)]
            fixed => fixed.step_raw()?,
        }
        .max(1);
        let reference = end.map_or(beat.raw(), |end| beat.raw().min(end.saturating_sub(1)));
        let lower = reference.saturating_sub(origin) / step;
        for index in [
            lower.saturating_sub(1),
            lower,
            lower.saturating_add(1),
            lower.saturating_add(2),
        ] {
            let Some(raw) = index
                .checked_mul(step)
                .and_then(|offset| origin.checked_add(offset))
            else {
                continue;
            };
            if end.is_some_and(|end| raw >= end) {
                continue;
            }
            let candidate = BeatTime::from_raw(raw);
            let value = if frame.is_some() {
                let Ok(value) = project.tempo_map().beat_to_frame(candidate) else {
                    continue;
                };
                value
            } else {
                raw
            };
            let current = frame.unwrap_or(beat.raw());
            if (right && value <= current) || (!right && value >= current) {
                continue;
            }
            if best.is_none_or(|(previous, _)| {
                if right {
                    value < previous
                } else {
                    value > previous
                }
            }) {
                best = Some((value, candidate));
            }
        }
    }
    let candidate = best.map(|(_, beat)| beat)?;
    if grid.is_adaptive()
        && visible_meter_sections(project, viewport).is_some_and(|count| count > MAX_GRID_LINES / 2)
    {
        let candidate_frame = project.tempo_map().beat_to_frame(candidate).ok()?;
        if viewport
            .end()
            .is_some_and(|end| (viewport.start..end).contains(&candidate_frame))
        {
            let lines = grid_lines(project, viewport, width, grid);
            return lines
                .into_iter()
                .filter(|line| {
                    let value = frame.map_or(line.beat.raw(), |_| line.frame);
                    let reference = frame.unwrap_or(beat.raw());
                    if right {
                        value > reference
                    } else {
                        value < reference
                    }
                })
                .min_by_key(|line| {
                    let value = frame.map_or(line.beat.raw(), |_| line.frame);
                    if right { value } else { u64::MAX - value }
                })
                .map(|line| line.beat);
        }
    }
    Some(candidate)
}

fn meter_label(position: MeterPosition) -> String {
    let fraction = position.beat_fraction().raw();
    if fraction == 0 {
        format!("{}.{}", position.bar(), position.beat())
    } else {
        let tenths = (u128::from(fraction) * 10 / u128::from(BeatTime::ONE.raw())) as u8;
        format!("{}.{}+{}", position.bar(), position.beat(), tenths)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SnapGrid {
    Off,
    #[default]
    Adaptive,
    AdaptiveDensity(AdaptiveGridDensity),
    OffDensity(AdaptiveGridDensity),
    // Fixed resolutions are retained only as musical-time regression fixtures.
    #[cfg(test)]
    Quarter,
    #[cfg(test)]
    Eighth,
    #[cfg(test)]
    Sixteenth,
}

impl SnapGrid {
    const fn is_adaptive(self) -> bool {
        matches!(self, Self::Adaptive | Self::AdaptiveDensity(_))
    }

    pub(super) const fn density(self) -> AdaptiveGridDensity {
        match self {
            Self::AdaptiveDensity(density) | Self::OffDensity(density) => density,
            _ => AdaptiveGridDensity::Medium,
        }
    }

    pub(super) const fn is_off(self) -> bool {
        matches!(self, Self::Off | Self::OffDensity(_))
    }

    pub(super) const fn with_density(self, density: AdaptiveGridDensity) -> Self {
        match (self.is_off(), density) {
            (true, AdaptiveGridDensity::Medium) => Self::Off,
            (false, AdaptiveGridDensity::Medium) => Self::Adaptive,
            (true, density) => Self::OffDensity(density),
            (false, density) => Self::AdaptiveDensity(density),
        }
    }

    pub(super) const fn toggled(self) -> Self {
        let density = self.density();
        if self.is_off() {
            Self::Adaptive.with_density(density)
        } else {
            Self::Off.with_density(density)
        }
    }

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Off | Self::OffDensity(_) => "off",
            Self::Adaptive | Self::AdaptiveDensity(_) => "adaptive",
            #[cfg(test)]
            Self::Quarter => "1/4",
            #[cfg(test)]
            Self::Eighth => "1/8",
            #[cfg(test)]
            Self::Sixteenth => "1/16",
        }
    }

    /// Q32.32 quarter-note interval; the canonical project map performs all
    /// frame conversion and nearest-grid tie handling.
    #[cfg(test)]
    pub(super) const fn step_raw(self) -> Option<u64> {
        const QUARTER: u64 = 1_u64 << 32;
        match self {
            Self::Off | Self::OffDensity(_) | Self::Adaptive | Self::AdaptiveDensity(_) => None,
            Self::Quarter => Some(QUARTER),
            Self::Eighth => Some(QUARTER / 2),
            Self::Sixteenth => Some(QUARTER / 4),
        }
    }

    fn landmark_density(self, pixel_width: f32) -> LandmarkDensity {
        match self {
            Self::Off | Self::OffDensity(_) => LandmarkDensity::Meter,
            Self::Adaptive | Self::AdaptiveDensity(_) => LandmarkDensity::Adaptive(
                pixel_width,
                adaptive_grid_spacing(pixel_width, self.density()),
            ),
            #[cfg(test)]
            fixed => LandmarkDensity::Fixed(fixed.step_raw().unwrap_or(1)),
        }
    }

    pub(super) const fn with_platform_modifier(self, held: bool) -> Self {
        if held { self.toggled() } else { self }
    }

    pub(super) fn adjacent_beat(
        self,
        project: &Project,
        beat: BeatTime,
        viewport: Viewport,
        width: f32,
        right: bool,
    ) -> Option<BeatTime> {
        adjacent_landmark(project, beat, None, viewport, width, self, right)
    }
    pub(super) fn adjacent_frame(
        self,
        project: &Project,
        frame: u64,
        viewport: Viewport,
        width: f32,
        right: bool,
    ) -> Option<u64> {
        let beat = project.tempo_map().frame_to_beat(frame).ok()?;
        let next = adjacent_landmark(project, beat, Some(frame), viewport, width, self, right)?;
        project.tempo_map().beat_to_frame(next).ok()
    }

    pub(super) fn snap_frame(
        self,
        project: &Project,
        candidate_frame: u64,
        viewport: Viewport,
        pixel_width: f32,
    ) -> Option<u64> {
        if self.is_off() {
            return Some(candidate_frame);
        }
        let beat = project.tempo_map().frame_to_beat(candidate_frame).ok()?;
        let snapped = self.snap_beat(project, beat, viewport, pixel_width)?;
        project.tempo_map().beat_to_frame(snapped).ok()
    }

    pub(super) fn snap_beat(
        self,
        project: &Project,
        beat: BeatTime,
        viewport: Viewport,
        pixel_width: f32,
    ) -> Option<BeatTime> {
        if self.is_off() {
            return Some(beat);
        }
        let candidate = snap_to_meter_landmark(project, beat, viewport, pixel_width, self)?;
        if self.is_adaptive()
            && visible_meter_sections(project, viewport)
                .is_some_and(|count| count > MAX_GRID_LINES / 2)
        {
            let frame = project.tempo_map().beat_to_frame(candidate).ok()?;
            if viewport
                .end()
                .is_some_and(|end| (viewport.start..end).contains(&frame))
            {
                return grid_lines(project, viewport, pixel_width, self)
                    .into_iter()
                    .min_by_key(|line| {
                        (
                            line.beat.raw().abs_diff(beat.raw()),
                            u64::MAX - line.beat.raw(),
                        )
                    })
                    .map(|line| line.beat);
            }
        }
        Some(candidate)
    }
}


#[cfg(test)]
pub(super) fn differential_output(project: &Project, start: u64, length: u64, width: f32, density: usize) -> Vec<String> {
 let viewport = Viewport { start, length };
 let grid = SnapGrid::Adaptive.with_density(AdaptiveGridDensity::ALL[density]);
 let mut result = vec![format!("{:?}", grid_lines(project, viewport, width, grid)), format!("{:?}", ruler_ticks(project, viewport, width)), format!("{:?}", adaptive_grid_bands_for_grid(project, viewport, width, grid))];
 for candidate in [start.saturating_sub(1), start, start.saturating_add(length / 2), start.saturating_add(length)] {
  result.push(format!("{:?}", (grid.snap_frame(project, candidate, viewport, width), grid.adjacent_frame(project, candidate, viewport, width, true), grid.adjacent_frame(project, candidate, viewport, width, false))));
 }
 result
}
