from pathlib import Path
p=Path('crates/hex-ui/src/editor/musical_time.rs');s=p.read_text()
a=s.index('fn adaptive_grid_step(');b=s.index('\nfn snap_to_meter_landmark(',a)
old=s[a:b];Path('/tmp/310-old-step-instrumented.rs').write_text(old)
new='''// Ephemeral metadata for ONE paint/snap/nudge/band operation. Visible natural
// demands are evaluated once, then the existing max-min allocation is resolved
// once. This fixed scratch never outlives the immutable operation or changes
// the dense-map/outside-neighbor quotient/remainder fallback.
struct AdaptiveGridResolution<'a> {
    project: &'a Project,
    viewport: Viewport,
    pixel_width: f32,
    target_spacing: f32,
    visible: Option<VisibleGridQuota>,
}

struct VisibleGridQuota {
    first: BeatTime,
    last: BeatTime,
    first_section: usize,
    last_section: usize,
    sections: usize,
    available: usize,
    quotas: [u16; MAX_GRID_LINES],
}

impl<'a> AdaptiveGridResolution<'a> {
    fn for_grid(project: &'a Project, viewport: Viewport, width: f32, grid: SnapGrid) -> Self {
        Self::new(project, viewport, width, adaptive_grid_spacing(width, grid.density()), grid.is_adaptive())
    }

    fn new(project: &'a Project, viewport: Viewport, pixel_width: f32, target_spacing: f32, enabled: bool) -> Self {
        let mut resolution = Self { project, viewport, pixel_width, target_spacing, visible: None };
        if !enabled || viewport.length == 0 || !pixel_width.is_finite() || pixel_width <= 0.0 || !target_spacing.is_finite() {
            return resolution;
        }
        let tempo = project.tempo_map();
        let (Ok(first), Some(end)) = (tempo.frame_to_beat(viewport.start), viewport.end()) else { return resolution; };
        let last = tempo.frame_to_beat(end.saturating_sub(1)).unwrap_or_else(|_| BeatTime::from_raw(u64::MAX));
        let points = project.meter_map().points();
        let first_section = points.partition_point(|point| point.beat() <= first).saturating_sub(1);
        let last_section = points.partition_point(|point| point.beat() <= last);
        let sections = last_section.saturating_sub(first_section).max(1);
        let extra_markers = usize::from(points.get(first_section).is_some_and(|point| {
            point.beat() < first && tempo.beat_to_frame(point.beat()).is_ok_and(|frame| frame >= viewport.start)
        })) + usize::from(points.get(last_section).is_some_and(|point| {
            tempo.beat_to_frame(point.beat()).is_ok_and(|frame| frame < end)
        }));
        let available = MAX_GRID_LINES.saturating_sub(extra_markers);
        let mut quotas = [0_u16; MAX_GRID_LINES];
        if sections <= available {
            for (relative, quota) in quotas.iter_mut().take(sections).enumerate() {
                #[cfg(test)]
                record_grid_work(3);
                let index = first_section + relative;
                let Some(point) = points.get(index) else { continue; };
                let start = first.raw().max(point.beat().raw());
                let end = points.get(index + 1).and_then(|next| next.beat().raw().checked_sub(1)).unwrap_or(last.raw()).min(last.raw());
                if start > end { continue; }
                let meter_length = (4_u64 * BeatTime::ONE.raw()) / u64::from(point.meter().denominator());
                let base = adaptive_base_step(project, viewport, point.beat(), meter_length, pixel_width, target_spacing);
                *quota = ((u128::from(end - start) / u128::from(base) + 1).min((available + 1) as u128)) as u16;
            }
            let (level, mut remainder) = capped_fair_share(sections, available, |index| usize::from(quotas[index]));
            for quota in quotas.iter_mut().take(sections) {
                let wanted = usize::from(*quota);
                let extra = usize::from(wanted > level && remainder > 0);
                remainder -= extra;
                *quota = (wanted.min(level) + extra) as u16;
            }
        }
        resolution.visible = Some(VisibleGridQuota { first, last, first_section, last_section, sections, available, quotas });
        resolution
    }

    fn step(&self, beat: BeatTime, meter_beat_length: u64) -> u64 {
        #[cfg(test)]
        record_grid_work(0);
        let Self { project, viewport, pixel_width, target_spacing, .. } = *self;
        if meter_beat_length == 0 || viewport.length == 0 || !pixel_width.is_finite() || pixel_width <= 0.0 || !target_spacing.is_finite() {
            return meter_beat_length.max(1);
        }
        let mut step = adaptive_base_step(project, viewport, beat, meter_beat_length, pixel_width, target_spacing);
        if let Some(visible) = &self.visible {
            let points = project.meter_map().points();
            let section = points.partition_point(|point| point.beat() <= beat).saturating_sub(1);
            let quota = if visible.sections <= visible.available && (visible.first_section..visible.last_section).contains(&section) {
                usize::from(visible.quotas[section - visible.first_section])
            } else {
                visible.available / visible.sections + usize::from(section.saturating_sub(visible.first_section) < visible.available % visible.sections)
            }.max(1) as u64;
            let start = visible.first.raw().max(beat.raw());
            let end = points.get(section + 1).and_then(|point| point.beat().raw().checked_sub(1)).unwrap_or(visible.last.raw()).min(visible.last.raw());
            if start <= end {
                loop {
                    let count = u128::from(end - start) / u128::from(step.max(1)) + 1;
                    if count <= u128::from(quota) || step == u64::MAX { break; }
                    step = step.saturating_mul(2);
                }
            }
        }
        step.max(1)
    }
}
'''
s=s[:a]+new+s[b:]
# Adaptive density carries the operation's one context through candidate paint.
s=s.replace('enum LandmarkDensity {','enum LandmarkDensity<\'a> {').replace('    Adaptive(f32, f32),','    Adaptive(&\'a AdaptiveGridResolution<\'a>),')
s=s.replace('    target_grid: LandmarkDensity,','    target_grid: LandmarkDensity<\'_>,')
s=s.replace('''LandmarkDensity::Adaptive(pixel_width, spacing) => adaptive_grid_step(
                        project,
                        viewport,
                        point.beat(),
                        meter_beat_length,
                        pixel_width,
                        spacing,
                    ),''','''LandmarkDensity::Adaptive(resolution) => resolution.step(point.beat(), meter_beat_length),''')
# bands once
anchor='    let mut bands = Vec::with_capacity(32);';s=s.replace(anchor,'    let resolution = AdaptiveGridResolution::new(project, viewport, pixel_width, adaptive_grid_spacing(pixel_width, grid.density()), true);\n'+anchor)
s=s.replace('''adaptive_grid_step(
            project,
            viewport,
            point.beat(),
            beat_length,
            pixel_width,
            adaptive_grid_spacing(pixel_width, grid.density()),
        )''','resolution.step(point.beat(), beat_length)')
# grid_lines wrapper for default caller; fallback can borrow same resolution.
anchor='''    #[cfg(test)]
    GRID_LINE_BUILDS.with''';pos=s.index(anchor,s.index('pub(super) fn grid_lines('));s=s[:pos]+'''    let resolution = AdaptiveGridResolution::for_grid(project, viewport, pixel_width, grid);
    grid_lines_resolved(project, viewport, pixel_width, grid, &resolution)
}

fn grid_lines_resolved(project: &Project, viewport: Viewport, pixel_width: f32, grid: SnapGrid, resolution: &AdaptiveGridResolution<'_>) -> Vec<GridLine> {
'''+s[pos:]
s=s.replace('grid.landmark_density(pixel_width)','grid.landmark_density(resolution)')
# ruler context
s=s.replace('    let ruler_candidates = ruler_candidates(','    let resolution = AdaptiveGridResolution::new(project, viewport, pixel_width, RULER_LABEL_SPACING_PX, true);\n    let ruler_candidates = ruler_candidates(',1)
s=s.replace('LandmarkDensity::Adaptive(pixel_width, RULER_LABEL_SPACING_PX)','LandmarkDensity::Adaptive(&resolution)')
# density method
s=s.replace('fn landmark_density(self, pixel_width: f32) -> LandmarkDensity {',"fn landmark_density<'a>(self, resolution: &'a AdaptiveGridResolution<'a>) -> LandmarkDensity<'a> {")
s=s.replace('''LandmarkDensity::Adaptive(
                pixel_width,
                adaptive_grid_spacing(pixel_width, self.density()),
            )''','LandmarkDensity::Adaptive(resolution)')
# snap helper receive resolution, nudge construct its own once
s=s.replace('''    grid: SnapGrid,
) -> Option<BeatTime> {''','''    grid: SnapGrid,
    resolution: &AdaptiveGridResolution<'_>,
) -> Option<BeatTime> {''',1)
s=s.replace('''adaptive_grid_step(
                project,
                viewport,
                point.beat(),
                meter_beat_length,
                pixel_width,
                adaptive_grid_spacing(pixel_width, grid.density()),
            )''','resolution.step(point.beat(), meter_beat_length)')
anchor='    let mut best: Option<(u64, BeatTime)> = None;';s=s.replace(anchor,'    let resolution = AdaptiveGridResolution::for_grid(project, viewport, width, grid);\n'+anchor)
s=s.replace('''adaptive_grid_step(
                project,
                viewport,
                point.beat(),
                4 * BeatTime::ONE.raw() / u64::from(point.meter().denominator()),
                width,
                adaptive_grid_spacing(width, grid.density()),
            )''','resolution.step(point.beat(), 4 * BeatTime::ONE.raw() / u64::from(point.meter().denominator()))')
s=s.replace('let lines = grid_lines(project, viewport, width, grid);','let lines = grid_lines_resolved(project, viewport, width, grid, &resolution);')
s=s.replace('''        let candidate = snap_to_meter_landmark(project, beat, viewport, pixel_width, self)?;''','''        let resolution = AdaptiveGridResolution::for_grid(project, viewport, pixel_width, self);
        let candidate = snap_to_meter_landmark(project, beat, viewport, pixel_width, self, &resolution)?;''')
s=s.replace('return grid_lines(project, viewport, pixel_width, self)','return grid_lines_resolved(project, viewport, pixel_width, self, &resolution)')
# snap helper no longer needs viewport/width. retain signature? Remove those now unused arguments.
s=s.replace('''    beat: BeatTime,
    viewport: Viewport,
    pixel_width: f32,
    grid: SnapGrid,
    resolution:''','''    beat: BeatTime,
    grid: SnapGrid,
    resolution:''')
s=s.replace('snap_to_meter_landmark(project, beat, viewport, pixel_width, self, &resolution)','snap_to_meter_landmark(project, beat, self, &resolution)')
p.write_text(s)
