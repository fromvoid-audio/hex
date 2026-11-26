//! Adaptive background grid bands for timeline rendering.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridBand {
    pub start_frame: u64,
    pub end_frame: u64,
    pub alternate: bool,
}

/// Build alternating background bands spanning at least four adaptive grid
/// units, widened to the next musical level (see [`stable_band_span`]) so the
/// zebra changes only every few zoom steps. Band indices are anchored to each
/// meter segment's musical origin, so panning changes the visible subset
/// without moving the pattern.
pub fn adaptive_grid_bands(
    project: &Project,
    viewport: Viewport,
    pixel_width: f32,
) -> Vec<GridBand> {
    adaptive_grid_bands_for_grid(project, viewport, pixel_width, SnapGrid::Adaptive)
}

pub fn adaptive_grid_bands_for_grid(
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
    let resolution = AdaptiveGridResolution::new(
        project,
        viewport,
        pixel_width,
        adaptive_grid_spacing(pixel_width, grid.density()),
        true,
    );
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
        let step = resolution.step(point.beat(), beat_length);
        let Some(band_span) = step
            .checked_mul(4)
            .and_then(|span| {
                stable_band_span(span, beat_length, u64::from(point.meter().numerator()))
            })
            .filter(|span| *span > 0)
        else {
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

/// Quantize a zebra band to the smallest musical level that holds `span`:
/// one beat (the minimum, even at extreme zoom), one bar, then four times the
/// previous level (4, 16, 64 … bars). In 4/4 each level is four times the
/// last, so the pattern holds through two adaptive-grid doublings before it
/// changes, instead of changing at every grid step.
pub(super) fn stable_band_span(span: u64, beat_length: u64, beats_per_bar: u64) -> Option<u64> {
    if beat_length == 0 {
        return None;
    }
    if span <= beat_length {
        return Some(beat_length);
    }
    let mut level = beat_length.checked_mul(beats_per_bar.max(1))?;
    while level < span {
        level = level.checked_mul(4)?;
    }
    Some(level)
}

pub(super) fn bounded_band_indices(
    first: u64,
    last: u64,
    budget: usize,
) -> impl Iterator<Item = u64> {
    (first..=last).take(budget)
}
