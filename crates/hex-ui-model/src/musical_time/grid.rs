//! Musical grid line generation and adaptive step resolution.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridLine {
    pub(super) beat: BeatTime,
    pub frame: u64,
    pub bar_start: bool,
    pub meter_marker: bool,
}

#[cfg(test)]
thread_local! {
    // Per-test-thread observation of actual paint-list construction, not a
    // process-wide setting or replacement resolver.
    pub(super) static GRID_LINE_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub fn grid_lines(
    project: &Project,
    viewport: Viewport,
    pixel_width: f32,
    grid: SnapGrid,
) -> Vec<GridLine> {
    let resolution = AdaptiveGridResolution::for_grid(project, viewport, pixel_width, grid);
    grid_lines_resolved(project, viewport, pixel_width, grid, &resolution)
}

pub(super) fn grid_lines_resolved(
    project: &Project,
    viewport: Viewport,
    pixel_width: f32,
    grid: SnapGrid,
    resolution: &AdaptiveGridResolution<'_>,
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
        grid.landmark_density(resolution),
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

pub(super) fn adaptive_base_step(
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

// Ephemeral metadata for ONE paint/snap/nudge/band operation. Visible natural
// demands are evaluated once, then the existing max-min allocation is resolved
// once. This fixed scratch never outlives the immutable operation or changes
// the dense-map/outside-neighbor quotient/remainder fallback.
pub(super) struct AdaptiveGridResolution<'a> {
    project: &'a Project,
    viewport: Viewport,
    pixel_width: f32,
    target_spacing: f32,
    visible: Option<VisibleGridQuota>,
}

pub(super) struct VisibleGridQuota {
    first: BeatTime,
    last: BeatTime,
    first_section: usize,
    last_section: usize,
    sections: usize,
    available: usize,
    quotas: [u16; MAX_GRID_LINES],
}

impl<'a> AdaptiveGridResolution<'a> {
    pub(super) fn for_grid(
        project: &'a Project,
        viewport: Viewport,
        width: f32,
        grid: SnapGrid,
    ) -> Self {
        Self::new(
            project,
            viewport,
            width,
            adaptive_grid_spacing(width, grid.density()),
            grid.is_adaptive(),
        )
    }

    pub(super) fn new(
        project: &'a Project,
        viewport: Viewport,
        pixel_width: f32,
        target_spacing: f32,
        enabled: bool,
    ) -> Self {
        let mut resolution = Self {
            project,
            viewport,
            pixel_width,
            target_spacing,
            visible: None,
        };
        if !enabled
            || viewport.length == 0
            || !pixel_width.is_finite()
            || pixel_width <= 0.0
            || !target_spacing.is_finite()
        {
            return resolution;
        }
        let tempo = project.tempo_map();
        let (Ok(first), Some(end)) = (tempo.frame_to_beat(viewport.start), viewport.end()) else {
            return resolution;
        };
        let last = tempo
            .frame_to_beat(end.saturating_sub(1))
            .unwrap_or_else(|_| BeatTime::from_raw(u64::MAX));
        let points = project.meter_map().points();
        let first_section = points
            .partition_point(|point| point.beat() <= first)
            .saturating_sub(1);
        let last_section = points.partition_point(|point| point.beat() <= last);
        let sections = last_section.saturating_sub(first_section).max(1);
        // Internal origins are already regular landmarks. Reserve only the
        // same frame-quantized edge-marker exceptions as the original law.
        let extra_markers = usize::from(points.get(first_section).is_some_and(|point| {
            point.beat() < first
                && tempo
                    .beat_to_frame(point.beat())
                    .is_ok_and(|frame| frame >= viewport.start)
        })) + usize::from(points.get(last_section).is_some_and(|point| {
            tempo
                .beat_to_frame(point.beat())
                .is_ok_and(|frame| frame < end)
        }));
        let available = MAX_GRID_LINES.saturating_sub(extra_markers);
        let mut quotas = [0_u16; MAX_GRID_LINES];
        if sections <= available {
            for (relative, quota) in quotas.iter_mut().take(sections).enumerate() {
                #[cfg(test)]
                record_grid_work(3);
                let index = first_section + relative;
                let Some(point) = points.get(index) else {
                    continue;
                };
                let start = first.raw().max(point.beat().raw());
                let end = points
                    .get(index + 1)
                    .and_then(|next| next.beat().raw().checked_sub(1))
                    .unwrap_or(last.raw())
                    .min(last.raw());
                if start > end {
                    continue;
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
                *quota = ((u128::from(end - start) / u128::from(base) + 1)
                    .min((available + 1) as u128)) as u16;
            }
            let (level, mut remainder) =
                capped_fair_share(sections, available, |index| usize::from(quotas[index]));
            for quota in quotas.iter_mut().take(sections) {
                let wanted = usize::from(*quota);
                let extra = usize::from(wanted > level && remainder > 0);
                remainder -= extra;
                *quota = (wanted.min(level) + extra) as u16;
            }
        }
        resolution.visible = Some(VisibleGridQuota {
            first,
            last,
            first_section,
            last_section,
            sections,
            available,
            quotas,
        });
        resolution
    }

    pub(super) fn step(&self, beat: BeatTime, meter_beat_length: u64) -> u64 {
        #[cfg(test)]
        record_grid_work(0);
        let Self {
            project,
            viewport,
            pixel_width,
            target_spacing,
            ..
        } = *self;
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
        if let Some(visible) = &self.visible {
            let points = project.meter_map().points();
            let section = points
                .partition_point(|point| point.beat() <= beat)
                .saturating_sub(1);
            let quota = if visible.sections <= visible.available
                && (visible.first_section..visible.last_section).contains(&section)
            {
                usize::from(visible.quotas[section - visible.first_section])
            } else {
                visible.available / visible.sections
                    + usize::from(
                        section.saturating_sub(visible.first_section)
                            < visible.available % visible.sections,
                    )
            }
            .max(1) as u64;
            let start = visible.first.raw().max(beat.raw());
            let end = points
                .get(section + 1)
                .and_then(|point| point.beat().raw().checked_sub(1))
                .unwrap_or(visible.last.raw())
                .min(visible.last.raw());
            if start <= end {
                loop {
                    // Conservative across ANY phase: a one-frame pan must not
                    // halve the subdivision at a 255/256 landmark transition.
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
}
