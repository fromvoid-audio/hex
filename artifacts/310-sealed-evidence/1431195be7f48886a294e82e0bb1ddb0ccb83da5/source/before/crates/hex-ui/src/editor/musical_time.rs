//! UI-only snap selection and bounded musical timeline helpers.

#[path = "adaptive_grid_density.rs"]
mod density;
pub(super) use density::AdaptiveGridDensity;

use super::viewport::Viewport;
use crate::theme;
use gpui::{Context, Div, Window, div, prelude::*, px, rgb};
use hex_project::{BeatTime, Command, Meter, MeterPosition, Project};
use hex_session::service::Request;

const MAX_RULER_TICKS: usize = 64;
const MAX_METER_MAP_POINTS: usize = 1_024;
const MAX_GRID_LINES: usize = 256;
const MAX_GRID_BANDS: usize = 256;
const RULER_LABEL_SPACING_PX: f32 = 48.0;
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

impl super::EditorView {
    pub(super) fn toggle_snap_grid(&mut self) {
        self.snap_grid = self.snap_grid.toggled();
        self.status = format!("arranger · snap {}", self.snap_grid.label());
    }

    pub(super) fn cursor_beat(&self) -> Option<BeatTime> {
        let state = self.state.as_ref()?;
        let beat = state
            .project
            .tempo_map()
            .frame_to_beat(self.cursor_position())
            .ok()?;
        beat.snap_to(BeatTime::ONE).ok()
    }

    fn time_edit_admitted(&self) -> bool {
        !self.opening_project
            && !self.close_prompt_open
            && self.worker_requests_pending == 0
            && self.pending_requests.is_empty()
    }

    pub(super) fn cycle_meter(&mut self) {
        if !self.time_edit_admitted() {
            self.error = Some("meter edit waits for pending project work".into());
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(beat) = self.cursor_beat() else {
            self.error = Some("meter point unavailable at this cursor position".into());
            return;
        };
        let current = state.project.meter_map().meter_at(beat);
        let signature = match (current.numerator(), current.denominator()) {
            (4, 4) => (3, 4),
            (3, 4) => (6, 8),
            (6, 8) => (5, 4),
            _ => (4, 4),
        };
        let Ok(meter) = Meter::new(signature.0, signature.1) else {
            self.error = Some("meter point unavailable".into());
            return;
        };
        self.enqueue(Request::Edit(Command::SetMeterPoint { beat, meter }));
    }

    pub(super) fn time_controls(&self, cx: &mut Context<Self>) -> Div {
        let Some(state) = self.state.as_ref() else {
            return div().w(px(self.track_header_width())).flex_shrink_0();
        };
        let beat = self.cursor_beat();
        let meter_label = beat.map_or_else(
            || "—".into(),
            |beat| {
                let meter = state.project.meter_map().meter_at(beat);
                format!("{}/{}", meter.numerator(), meter.denominator())
            },
        );
        let enabled = self.time_edit_admitted() && beat.is_some();
        div()
            .w(px(self.track_header_width()))
            .h_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(2.0))
            .children([self.time_button(
                &meter_label,
                "cycle meter at cursor snapped quarter · 4/4, 3/4, 6/8, 5/4",
                enabled,
                |view, _, _| view.cycle_meter(),
                cx,
            )])
    }

    fn time_button(
        &self,
        label: &str,
        description: &'static str,
        enabled: bool,
        handler: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        div()
            .id(description)
            .w(px(20.0))
            .h(px(19.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_xs()
            .bg(rgb(theme::color(theme::PaletteRole::Raised)))
            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
            .opacity(if enabled { 1.0 } else { 0.45 })
            .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Hover))))
            .tooltip(move |_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    description: description.into(),
                })
                .into()
            })
            .on_click(cx.listener(move |view, _, window, cx| {
                handler(view, window, cx);
                cx.notify();
            }))
            .child(theme::text(label))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LandmarkDensity, MAX_GRID_BANDS, MAX_GRID_LINES, MAX_METER_MAP_POINTS, MAX_RULER_TICKS,
        SnapGrid, adaptive_grid_bands, grid_lines, ruler_candidates, ruler_ticks,
    };
    use crate::editor::viewport::Viewport;
    use hex_project::{BeatTime, Command, Editor, Meter, Project, Tempo};

    #[test]
    fn directional_nudge_landmarks_are_distinct_off_grid_and_across_meter_tempo_seams() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::from_quarters(2).unwrap(),
                tempo: Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        editor
            .execute(Command::SetMeterPoint {
                beat: BeatTime::from_ratio(5, 2).unwrap(),
                meter: Meter::new(3, 4).unwrap(),
            })
            .unwrap();
        let project = editor.project();
        let viewport = Viewport::fit(384_000);
        assert_eq!(
            SnapGrid::Eighth.adjacent_frame(project, 7200, viewport, 640.0, false),
            Some(0)
        );
        assert_eq!(
            SnapGrid::Eighth.adjacent_frame(project, 7200, viewport, 640.0, true),
            Some(12000)
        );
        assert_eq!(
            SnapGrid::Eighth.adjacent_frame(project, 12000, viewport, 640.0, true),
            Some(24000)
        );
        assert_eq!(
            SnapGrid::Quarter.adjacent_frame(project, 67200, viewport, 640.0, true),
            Some(72000)
        );
        assert_eq!(
            SnapGrid::Quarter.adjacent_frame(project, 72000, viewport, 640.0, false),
            Some(48000)
        );
        assert_eq!(
            SnapGrid::Quarter.adjacent_frame(project, 72000, viewport, 640.0, true),
            Some(120000)
        );
        for width in [160.0, 640.0, 1920.0] {
            let lines = grid_lines(project, viewport, width, SnapGrid::Adaptive);
            for pair in lines.windows(2) {
                assert_eq!(
                    SnapGrid::Adaptive.adjacent_frame(
                        project,
                        pair[0].frame,
                        viewport,
                        width,
                        true
                    ),
                    Some(pair[1].frame)
                );
                assert_eq!(
                    SnapGrid::Adaptive.adjacent_frame(
                        project,
                        pair[1].frame,
                        viewport,
                        width,
                        false
                    ),
                    Some(pair[0].frame)
                );
            }
        }
        assert_eq!(
            SnapGrid::Quarter.adjacent_beat(project, BeatTime::ZERO, viewport, 640.0, false),
            None
        );
        assert_eq!(
            SnapGrid::Quarter.adjacent_beat(
                project,
                BeatTime::from_raw(u64::MAX),
                viewport,
                640.0,
                true
            ),
            None
        );
        assert_eq!(
            SnapGrid::Quarter.adjacent_frame(project, u64::MAX, viewport, 640.0, true),
            None
        );
        assert_eq!(
            SnapGrid::Quarter.adjacent_frame(project, 0, viewport, f32::NAN, true),
            None
        );
    }

    #[test]
    fn arranger_snap_toggles_between_adaptive_and_off() {
        assert_eq!(SnapGrid::default(), SnapGrid::Adaptive);
        assert_eq!(SnapGrid::Adaptive.toggled(), SnapGrid::Off);
        assert_eq!(SnapGrid::Off.toggled(), SnapGrid::Adaptive);
        assert_eq!(SnapGrid::Off.step_raw(), None);
        assert_eq!(SnapGrid::Adaptive.step_raw(), None);
        assert_eq!(SnapGrid::Quarter.step_raw(), Some(1_u64 << 32));
        assert_eq!(SnapGrid::Eighth.step_raw(), Some(1_u64 << 31));
        assert_eq!(SnapGrid::Sixteenth.step_raw(), Some(1_u64 << 30));
        assert_eq!(
            SnapGrid::Adaptive.with_platform_modifier(true),
            SnapGrid::Off
        );
        assert_eq!(
            SnapGrid::Off.with_platform_modifier(true),
            SnapGrid::Adaptive
        );
        assert_eq!(
            SnapGrid::Quarter.with_platform_modifier(true),
            SnapGrid::Off
        );
        assert_eq!(
            SnapGrid::Adaptive.with_platform_modifier(false),
            SnapGrid::Adaptive
        );
    }

    #[test]
    fn snap_resolution_uses_canonical_map_and_rounds_exact_ties_later() {
        let project = Project::new(48_000).expect("project");
        let viewport = Viewport {
            start: 0,
            length: 288_000,
        };
        let tie_frame = project
            .tempo_map()
            .beat_to_frame(BeatTime::from_ratio(1, 2).expect("half beat"))
            .expect("frame");
        assert_eq!(
            SnapGrid::Off.snap_frame(&project, tie_frame, viewport, 640.0),
            Some(tie_frame)
        );
        assert_eq!(
            SnapGrid::Off.snap_frame(&project, u64::MAX, viewport, 640.0),
            Some(u64::MAX)
        );
        assert_eq!(
            SnapGrid::Quarter.snap_frame(&project, tie_frame, viewport, 640.0),
            Some(24_000)
        );

        let eighth_tie_frame = project
            .tempo_map()
            .beat_to_frame(BeatTime::from_ratio(3, 4).expect("eighth tie"))
            .expect("frame");
        assert_eq!(
            SnapGrid::Eighth.snap_frame(&project, eighth_tie_frame, viewport, 640.0),
            Some(24_000)
        );
        assert_eq!(
            SnapGrid::Quarter.snap_frame(&project, u64::MAX, viewport, 640.0),
            None
        );
    }

    #[test]
    fn bounded_ruler_marks_quarter_positions_and_fragmentary_meter_changes() {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
        let beat = BeatTime::from_ratio(3, 2).expect("meter beat");
        let meter = Meter::new(3, 4).expect("meter");
        let _ = editor
            .execute(Command::SetMeterPoint { beat, meter })
            .expect("meter point");
        assert_eq!(editor.project().meter_map().points().len(), 2);

        let ticks = ruler_ticks(
            editor.project(),
            Viewport {
                start: 0,
                length: 120_000,
            },
            600.0,
        );
        assert!(ticks.len() <= 13);
        let marker = ticks
            .iter()
            .find(|tick| tick.meter_marker)
            .expect("visible meter marker");
        assert_eq!(marker.frame, 36_000);
        assert!(marker.fragmentary_bar);
        assert_eq!(marker.label, "2.1");
    }

    #[test]
    fn ruler_keeps_only_visible_landmarks_across_section_end() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        editor
            .execute(Command::SetMeterPoint {
                beat: BeatTime::from_quarters(4).unwrap(),
                meter: Meter::new(3, 4).unwrap(),
            })
            .unwrap();
        // At 120 bpm the second, open-ended section begins at frame 96_000.
        for (start, expected_frames) in [
            (72_000, vec![72_000, 96_000, 120_000]),
            (96_000, vec![96_000, 120_000, 144_000]),
            (120_000, vec![120_000, 144_000, 168_000]),
        ] {
            let viewport = Viewport {
                start,
                length: 72_000,
            };
            let ticks = ruler_candidates(
                editor.project(),
                viewport,
                viewport.end().unwrap(),
                editor.project().tempo_map().frame_to_beat(start).unwrap(),
                editor
                    .project()
                    .tempo_map()
                    .frame_to_beat(viewport.end().unwrap() - 1)
                    .unwrap(),
                8,
                LandmarkDensity::Meter,
            )
            .ticks;
            let mut frames = ticks.iter().map(|tick| tick.frame).collect::<Vec<_>>();
            frames.sort_unstable();
            frames.dedup();
            assert_eq!(frames, expected_frames);
            assert_eq!(
                ticks.iter().filter(|tick| tick.meter_marker).count(),
                usize::from(start <= 96_000)
            );
        }
    }

    #[test]
    fn six_eight_ruler_uses_denominator_beats() {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
        let _ = editor
            .execute(Command::SetMeterPoint {
                beat: BeatTime::ZERO,
                meter: Meter::new(6, 8).expect("meter"),
            })
            .expect("meter point");

        let ticks = ruler_ticks(
            editor.project(),
            Viewport {
                start: 0,
                length: 80_000,
            },
            600.0,
        );
        assert!(
            ticks
                .iter()
                .any(|tick| tick.frame == 12_000 && tick.label == "1.2")
        );
        assert!(
            ticks
                .iter()
                .any(|tick| tick.frame == 24_000 && tick.label == "1.3")
        );
    }

    #[test]
    fn fractional_meter_section_starts_its_denominator_beat_landmarks() {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
        let _ = editor
            .execute(Command::SetMeterPoint {
                beat: BeatTime::from_ratio(1, 2).expect("fractional point"),
                meter: Meter::new(3, 4).expect("meter"),
            })
            .expect("meter point");

        let ticks = ruler_ticks(
            editor.project(),
            Viewport {
                start: 0,
                length: 100_000,
            },
            600.0,
        );
        assert!(
            ticks
                .iter()
                .any(|tick| tick.frame == 12_000 && tick.label == "2.1")
        );
        assert!(
            ticks
                .iter()
                .any(|tick| tick.frame == 36_000 && tick.label == "2.2")
        );
    }

    #[test]
    fn huge_fractional_meter_viewport_keeps_tick_attempts_and_markers_bounded() {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
        let beat = BeatTime::from_ratio(1, 16).expect("fractional meter point");
        let meter = Meter::new(1, 1).expect("meter");
        let _ = editor
            .execute(Command::SetMeterPoint { beat, meter })
            .expect("meter point");

        let viewport = Viewport {
            start: 0,
            length: 1_000_000_000_000,
        };
        let viewport_end = viewport.end().expect("viewport end");
        let start_beat = editor
            .project()
            .tempo_map()
            .frame_to_beat(viewport.start)
            .expect("start beat");
        let last_beat = editor
            .project()
            .tempo_map()
            .frame_to_beat(viewport_end - 1)
            .unwrap_or_else(|_| BeatTime::from_raw(u64::MAX));
        let candidates = ruler_candidates(
            editor.project(),
            viewport,
            viewport_end,
            start_beat,
            last_beat,
            13,
            LandmarkDensity::Meter,
        );
        assert!(candidates.landmark_attempts <= 13);
        assert!(candidates.marker_attempts <= 1_024);
        let ticks = ruler_ticks(editor.project(), viewport, 600.0);
        assert!(ticks.len() <= 13);
        assert!(
            ticks
                .iter()
                .any(|tick| tick.meter_marker && tick.frame == 1_500)
        );

        let sub_quarter_ticks = ruler_ticks(
            editor.project(),
            Viewport {
                start: 1_400,
                length: 200,
            },
            200.0,
        );
        assert!(
            sub_quarter_ticks
                .iter()
                .any(|tick| tick.meter_marker && tick.frame == 1_500)
        );
    }

    #[test]
    fn ruler_landmarks_follow_tempo_boundaries_and_deduplicate_quantized_markers() {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
        let _ = editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::from_quarters(2).expect("tempo beat"),
                tempo: Tempo::new(60_000_000).expect("tempo"),
            })
            .expect("tempo point");
        let ticks = ruler_ticks(
            editor.project(),
            Viewport {
                start: 0,
                length: 150_000,
            },
            600.0,
        );
        assert!(
            ticks
                .iter()
                .any(|tick| tick.frame == 48_000 && tick.label == "1.3")
        );
        assert!(
            ticks
                .iter()
                .any(|tick| tick.frame == 96_000 && tick.label == "1.4")
        );

        let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
        for (raw, meter) in [
            (1, Meter::new(3, 4).expect("first meter")),
            (2, Meter::new(6, 8).expect("second meter")),
        ] {
            let _ = editor
                .execute(Command::SetMeterPoint {
                    beat: BeatTime::from_raw(raw),
                    meter,
                })
                .expect("meter point");
        }
        let ticks = ruler_ticks(
            editor.project(),
            Viewport {
                start: 0,
                length: 200,
            },
            200.0,
        );
        let at_zero: Vec<_> = ticks.iter().filter(|tick| tick.frame == 0).collect();
        assert_eq!(at_zero.len(), 1);
        assert!(at_zero[0].meter_marker);
        assert_eq!(at_zero[0].label, "3.1");
    }

    #[test]
    fn full_meter_map_keeps_marker_priority_spacing_and_tick_cap() {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
        let meter = Meter::new(3, 4).expect("meter");
        for index in 1..MAX_METER_MAP_POINTS {
            let beat = BeatTime::from_ratio(index as u64, 64).expect("meter beat");
            let _ = editor
                .execute(Command::SetMeterPoint { beat, meter })
                .expect("meter point");
        }
        let viewport = Viewport {
            start: 0,
            length: 384_000,
        };
        let viewport_end = viewport.end().expect("viewport end");
        let start_beat = BeatTime::ZERO;
        let last_beat = editor
            .project()
            .tempo_map()
            .frame_to_beat(viewport_end - 1)
            .expect("last beat");
        let candidates = ruler_candidates(
            editor.project(),
            viewport,
            viewport_end,
            start_beat,
            last_beat,
            MAX_RULER_TICKS,
            LandmarkDensity::Meter,
        );
        assert_eq!(candidates.marker_attempts, MAX_METER_MAP_POINTS);
        assert!(candidates.landmark_attempts <= MAX_RULER_TICKS);

        let ticks = ruler_ticks(editor.project(), viewport, 6_400.0);
        assert_eq!(ticks.len(), MAX_RULER_TICKS);
        assert!(ticks.iter().all(|tick| tick.meter_marker));
        assert!(ticks.windows(2).all(|pair| {
            let left = (pair[0].frame as f64 / viewport.length as f64) * 6_400.0;
            let right = (pair[1].frame as f64 / viewport.length as f64) * 6_400.0;
            right - left >= 48.0
        }));
    }

    #[test]
    fn adaptive_grid_stays_visible_and_bounded_at_zoom_extremes() {
        let project = Project::new(48_000).expect("project");
        for viewport in [
            Viewport {
                start: 0,
                length: 38_400,
            },
            Viewport {
                start: 0,
                length: 384_000,
            },
            Viewport {
                start: 0,
                length: 3_840_000,
            },
        ] {
            let lines = grid_lines(&project, viewport, 640.0, SnapGrid::Adaptive);
            assert!(!lines.is_empty(), "a valid viewport keeps grid landmarks");
            assert!(lines.len() <= MAX_GRID_LINES);
            assert!(lines.windows(2).all(|pair| pair[0].frame < pair[1].frame));
            assert!(lines.iter().all(|line| {
                line.frame >= viewport.start && line.frame < viewport.end().unwrap()
            }));
            let gaps = lines
                .windows(2)
                .map(|pair| (pair[1].frame - pair[0].frame) as f64 / viewport.length as f64 * 640.0)
                .collect::<Vec<_>>();
            if !gaps.is_empty() {
                let mut sorted_gaps = gaps;
                sorted_gaps.sort_by(f64::total_cmp);
                let median_gap = sorted_gaps[sorted_gaps.len() / 2];
                assert!(
                    (8.0..=40.0).contains(&median_gap),
                    "adaptive grid spacing is readable at {viewport:?}: {median_gap}px"
                );
            }
        }
        let zoomed_out = grid_lines(
            &project,
            Viewport {
                start: 0,
                length: 3_840_000,
            },
            640.0,
            SnapGrid::Adaptive,
        );
        assert!(zoomed_out.iter().any(|line| line.bar_start));
    }

    #[test]
    fn adaptive_background_bands_are_bounded_and_pan_anchored() {
        let project = Project::new(48_000).expect("project");
        let first_view = Viewport {
            start: 0,
            length: 384_000,
        };
        let panned_view = Viewport {
            start: 24_000,
            length: 384_000,
        };
        let first = adaptive_grid_bands(&project, first_view, 640.0);
        let panned = adaptive_grid_bands(&project, panned_view, 640.0);
        assert!(!first.is_empty());
        assert!(first.len() <= MAX_GRID_BANDS);
        assert!(first.iter().any(|band| band.start_frame == 0));
        assert!(first.iter().any(|band| band.alternate));
        assert!(first.iter().all(|band| band.start_frame < band.end_frame));
        assert!(panned.iter().all(|band| band.start_frame < band.end_frame));
        let shared = panned
            .iter()
            .find_map(|band| {
                first
                    .iter()
                    .find(|original| original.start_frame == band.start_frame)
                    .map(|original| (original, band))
            })
            .expect("panning retains at least one shared anchored band");
        assert_eq!(shared.0.alternate, shared.1.alternate);
        let zoomed = adaptive_grid_bands(
            &project,
            Viewport {
                start: 0,
                length: 192_000,
            },
            640.0,
        );
        assert_eq!(zoomed.first().map(|band| band.start_frame), Some(0));
        assert_ne!(
            zoomed.first().map(|band| band.end_frame),
            first.first().map(|band| band.end_frame),
            "zoom changes the four adaptive-grid-unit band width"
        );
    }

    #[test]
    fn adaptive_background_bands_restart_at_meter_origins() {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 2).expect("editor");
        editor
            .execute(Command::SetMeterPoint {
                beat: BeatTime::from_quarters(8).expect("meter beat"),
                meter: Meter::new(3, 4).expect("meter"),
            })
            .expect("meter point");
        let project = editor.project();
        let bands = adaptive_grid_bands(
            project,
            Viewport {
                start: 0,
                length: 384_000,
            },
            640.0,
        );
        let meter_origin = project
            .tempo_map()
            .beat_to_frame(BeatTime::from_quarters(8).expect("meter beat"))
            .expect("meter origin frame");
        let band = bands
            .iter()
            .find(|band| band.start_frame == meter_origin)
            .expect("new meter section begins a band at its musical origin");
        assert!(!band.alternate, "band parity restarts at the meter origin");
    }

    #[test]
    fn conversion_failures_cannot_make_band_candidate_enumeration_unbounded() {
        let attempts = std::cell::Cell::new(0);
        let produced = super::bounded_band_indices(0, u64::MAX, MAX_GRID_BANDS)
            .filter_map(|_| {
                attempts.set(attempts.get() + 1);
                None::<super::GridBand>
            })
            .count();
        assert_eq!(produced, 0, "simulate every frame conversion failing");
        let attempted = attempts.get();
        assert_eq!(attempted, MAX_GRID_BANDS);
    }

    #[test]
    fn grid_returns_empty_when_time_conversion_and_viewport_bounds_are_invalid() {
        let project = Project::new(48_000).expect("project");
        assert!(
            grid_lines(
                &project,
                Viewport {
                    start: u64::MAX - 1,
                    length: 1,
                },
                640.0,
                SnapGrid::Adaptive,
            )
            .is_empty()
        );
        assert!(grid_lines(&project, Viewport::fit(100), f32::NAN, SnapGrid::Adaptive).is_empty());
    }

    #[test]
    fn adaptive_snap_matches_painted_landmarks_across_tempo_meter_and_zoom_seams() {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
        let meter_beat = BeatTime::from_ratio(5, 2).expect("meter beat");
        let _ = editor
            .execute(Command::SetMeterPoint {
                beat: meter_beat,
                meter: Meter::new(3, 4).expect("meter"),
            })
            .expect("meter point");
        let _ = editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::from_quarters(2).expect("tempo beat"),
                tempo: Tempo::new(60_000_000).expect("tempo"),
            })
            .expect("tempo point");
        let project = editor.project();
        for viewport in [
            Viewport {
                start: 0,
                length: 72_000,
            },
            Viewport {
                start: 0,
                length: 288_000,
            },
            Viewport {
                start: 0,
                length: 1_152_000,
            },
        ] {
            let lines = grid_lines(project, viewport, 640.0, SnapGrid::Adaptive);
            assert!(!lines.is_empty());
            for pair in lines.windows(2) {
                let candidate = pair[0].frame + (pair[1].frame - pair[0].frame) / 2;
                let snapped = SnapGrid::Adaptive
                    .snap_frame(project, candidate, viewport, 640.0)
                    .expect("adaptive landmark");
                assert!(
                    snapped == pair[0].frame || snapped == pair[1].frame,
                    "snap {snapped} should use a painted neighboring landmark in {viewport:?}, pair={pair:?}, lines={lines:?}"
                );
            }
            assert!(lines.len() <= MAX_GRID_LINES);
        }

        let seam = project
            .tempo_map()
            .beat_to_frame(meter_beat)
            .expect("meter seam");
        let viewport = Viewport {
            start: 0,
            length: 288_000,
        };
        let lines = grid_lines(project, viewport, 640.0, SnapGrid::Adaptive);
        assert!(
            lines
                .iter()
                .any(|line| line.frame == seam && line.meter_marker)
        );
        for candidate in [seam.saturating_sub(1), seam, seam.saturating_add(1)] {
            let snapped = SnapGrid::Adaptive
                .snap_frame(project, candidate, viewport, 640.0)
                .expect("landmark around meter seam");
            assert!(lines.iter().any(|line| line.frame == snapped));
        }
    }
}

#[cfg(test)]
#[path = "adaptive_grid_density_tests.rs"]
mod density_tests;
