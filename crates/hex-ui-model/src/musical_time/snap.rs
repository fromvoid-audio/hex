//! Musical snap grid choices and meter-landmark snapping.
use super::*;

pub(super) fn snap_to_meter_landmark(
    project: &Project,
    beat: BeatTime,
    grid: SnapGrid,
    resolution: &AdaptiveGridResolution<'_>,
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
        let section_end = points.get(index + 1).map(|next| next.beat().raw());
        let meter_beat_length =
            (4_u64 * BeatTime::ONE.raw()) / u64::from(point.meter().denominator());
        let step = match grid {
            SnapGrid::Adaptive | SnapGrid::AdaptiveDensity(_) => {
                resolution.step(point.beat(), meter_beat_length)
            }
            SnapGrid::Off | SnapGrid::OffDensity(_) => return Some(beat),
            #[cfg(any(test, feature = "test-support"))]
            fixed => fixed.step_raw()?,
        }
        .max(1);
        let candidate = hex_project::MusicalGridSegment::new(
            point.beat(),
            section_end.map(BeatTime::from_raw),
            BeatTime::from_raw(step),
        )
        .ok()?
        .nearest(beat);
        if let Some(candidate) = candidate {
            let raw = candidate.raw();
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
pub(super) fn adjacent_landmark(
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
    let resolution = AdaptiveGridResolution::for_grid(project, viewport, width, grid);
    let mut best: Option<(u64, BeatTime)> = None;
    let points = project.meter_map().points();
    for (index, point) in points.iter().take(MAX_METER_MAP_POINTS).enumerate() {
        let origin = point.beat().raw();
        let end = points.get(index + 1).map(|point| point.beat().raw());
        let step = match grid {
            SnapGrid::Adaptive | SnapGrid::AdaptiveDensity(_) => resolution.step(
                point.beat(),
                4 * BeatTime::ONE.raw() / u64::from(point.meter().denominator()),
            ),
            SnapGrid::Off | SnapGrid::OffDensity(_) => return None,
            #[cfg(any(test, feature = "test-support"))]
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
            let lines = grid_lines_resolved(project, viewport, width, grid, &resolution);
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SnapGrid {
    Off,
    #[default]
    Adaptive,
    AdaptiveDensity(AdaptiveGridDensity),
    OffDensity(AdaptiveGridDensity),
    // Fixed resolutions are retained only as musical-time regression fixtures.
    #[cfg(any(test, feature = "test-support"))]
    #[allow(dead_code)] // Used by feature-gated window and timeline fixtures.
    Quarter,
    #[cfg(any(test, feature = "test-support"))]
    #[allow(dead_code)] // Used by feature-gated window and timeline fixtures.
    Eighth,
    #[cfg(any(test, feature = "test-support"))]
    #[allow(dead_code)] // Used by feature-gated window and timeline fixtures.
    Sixteenth,
}

impl SnapGrid {
    pub(crate) const fn is_adaptive(self) -> bool {
        matches!(self, Self::Adaptive | Self::AdaptiveDensity(_))
    }

    pub const fn density(self) -> AdaptiveGridDensity {
        match self {
            Self::AdaptiveDensity(density) | Self::OffDensity(density) => density,
            _ => AdaptiveGridDensity::Medium,
        }
    }

    pub const fn is_off(self) -> bool {
        matches!(self, Self::Off | Self::OffDensity(_))
    }

    pub const fn with_density(self, density: AdaptiveGridDensity) -> Self {
        match (self.is_off(), density) {
            (true, AdaptiveGridDensity::Medium) => Self::Off,
            (false, AdaptiveGridDensity::Medium) => Self::Adaptive,
            (true, density) => Self::OffDensity(density),
            (false, density) => Self::AdaptiveDensity(density),
        }
    }

    pub const fn toggled(self) -> Self {
        let density = self.density();
        if self.is_off() {
            Self::Adaptive.with_density(density)
        } else {
            Self::Off.with_density(density)
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Off | Self::OffDensity(_) => "off",
            Self::Adaptive | Self::AdaptiveDensity(_) => "adaptive",
            #[cfg(any(test, feature = "test-support"))]
            Self::Quarter => "1/4",
            #[cfg(any(test, feature = "test-support"))]
            Self::Eighth => "1/8",
            #[cfg(any(test, feature = "test-support"))]
            Self::Sixteenth => "1/16",
        }
    }

    /// Q32.32 quarter-note interval; the canonical project map performs all
    /// frame conversion and nearest-grid tie handling.
    #[cfg(any(test, feature = "test-support"))]
    pub const fn step_raw(self) -> Option<u64> {
        const QUARTER: u64 = 1_u64 << 32;
        match self {
            Self::Off | Self::OffDensity(_) | Self::Adaptive | Self::AdaptiveDensity(_) => None,
            Self::Quarter => Some(QUARTER),
            Self::Eighth => Some(QUARTER / 2),
            Self::Sixteenth => Some(QUARTER / 4),
        }
    }

    pub(super) fn landmark_density<'a>(
        self,
        resolution: &'a AdaptiveGridResolution<'a>,
    ) -> LandmarkDensity<'a> {
        match self {
            Self::Off | Self::OffDensity(_) => LandmarkDensity::Meter,
            Self::Adaptive | Self::AdaptiveDensity(_) => LandmarkDensity::Adaptive(resolution),
            #[cfg(any(test, feature = "test-support"))]
            fixed => LandmarkDensity::Fixed(fixed.step_raw().unwrap_or(1)),
        }
    }

    pub const fn with_platform_modifier(self, held: bool) -> Self {
        if held { self.toggled() } else { self }
    }

    pub fn adjacent_beat(
        self,
        project: &Project,
        beat: BeatTime,
        viewport: Viewport,
        width: f32,
        right: bool,
    ) -> Option<BeatTime> {
        adjacent_landmark(project, beat, None, viewport, width, self, right)
    }
    pub fn adjacent_frame(
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

    /// Freeze the same resolved steps and dense-meter painted fallback used by
    /// snapping. This bounded control-side copy does not change ordinary drags.
    pub fn freeze_input_grid(
        self,
        project: &Project,
        viewport: Viewport,
        width: f32,
    ) -> Result<hex_project::ResolvedInputGrid, &'static str> {
        if self.is_off() || !width.is_finite() || width <= 0.0 || viewport.length == 0 {
            return Err("enable the arrangement grid and wait for its layout");
        }
        let resolution = AdaptiveGridResolution::for_grid(project, viewport, width, self);
        let points = project.meter_map().points();
        let mut segments = Vec::new();
        segments
            .try_reserve_exact(points.len())
            .map_err(|_| "could not reserve input grid")?;
        for (index, point) in points.iter().enumerate() {
            let beat_length =
                (4_u64 * BeatTime::ONE.raw()) / u64::from(point.meter().denominator());
            let step = match self {
                Self::Adaptive | Self::AdaptiveDensity(_) => {
                    resolution.step(point.beat(), beat_length)
                }
                Self::Off | Self::OffDensity(_) => return Err("enable the arrangement grid"),
                #[cfg(any(test, feature = "test-support"))]
                fixed => fixed.step_raw().ok_or("invalid fixed input grid")?,
            };
            segments.push(
                hex_project::MusicalGridSegment::new(
                    point.beat(),
                    points.get(index + 1).map(|next| next.beat()),
                    BeatTime::from_raw(step),
                )
                .map_err(|_| "cannot resolve input grid segment")?,
            );
        }
        let mut frozen = hex_project::ResolvedInputGrid::new(segments)
            .map_err(|_| "cannot freeze input grid")?;
        if self.is_adaptive()
            && visible_meter_sections(project, viewport)
                .is_some_and(|count| count > MAX_GRID_LINES / 2)
        {
            let beats = grid_lines_resolved(project, viewport, width, self, &resolution)
                .into_iter()
                .map(|line| line.beat)
                .collect();
            frozen = frozen
                .with_visible_landmarks(
                    viewport.start,
                    viewport.end().ok_or("input viewport overflow")?,
                    beats,
                )
                .map_err(|_| "cannot freeze visible input grid")?;
        }
        Ok(frozen)
    }

    pub fn snap_frame(
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

    pub fn snap_beat(
        self,
        project: &Project,
        beat: BeatTime,
        viewport: Viewport,
        pixel_width: f32,
    ) -> Option<BeatTime> {
        if self.is_off() {
            return Some(beat);
        }
        let resolution = AdaptiveGridResolution::for_grid(project, viewport, pixel_width, self);
        let candidate = snap_to_meter_landmark(project, beat, self, &resolution)?;
        if self.is_adaptive()
            && visible_meter_sections(project, viewport)
                .is_some_and(|count| count > MAX_GRID_LINES / 2)
        {
            let frame = project.tempo_map().beat_to_frame(candidate).ok()?;
            if viewport
                .end()
                .is_some_and(|end| (viewport.start..end).contains(&frame))
            {
                return grid_lines_resolved(project, viewport, pixel_width, self, &resolution)
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
