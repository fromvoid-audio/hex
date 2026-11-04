use crate::theme;
use gpui::{Context, Div, Window, prelude::*, px};
use hex_project::{Clip, ClipFade};
use hex_session::{CachedPeak, CachedWaveform};
use hex_widgets::{MAX_WAVEFORM_COLUMNS, WaveformColors, WaveformEmphasis, waveform_plot};
use std::{ops::Range, rc::Rc, sync::Arc};

use super::waveform_geometry::{
    WaveformGeometryCache, WaveformGeometryRequest, WaveformMapping, cached_waveform_geometry,
    prepare_waveform_geometry,
};

pub(super) use super::waveform_geometry::WaveformGrid;

const FADE_ENVELOPE_SAMPLES: usize = 32;
const FADE_ENVELOPE_BREAKPOINTS: usize = 72;

/// Same-revision accepted peaks and the original/prospective metadata view.
/// Trim retains its processing root or proves a silent frame prefix before
/// borrowing unchanged output. No decoding or DSP belongs here.
#[derive(Clone, Copy)]
pub(super) struct WaveformPreview<'a> {
    pub(super) original: &'a Clip,
    pub(super) preview: &'a Clip,
    pub(super) tempo: &'a hex_project::TempoMap,
    pub(super) asset: Option<&'a Arc<CachedWaveform>>,
    pub(super) prepared: Option<&'a hex_session::service::AudioClipWaveform>,
}

/// Keep inactive clip peaks readable but subordinate to active material.
pub(super) fn clip_waveform_activity(body: Div, active: bool) -> Div {
    body.opacity(if active { 1.0 } else { 0.4 })
}

pub(super) fn preview_waveform_view_cached(
    request: WaveformPreview<'_>,
    visible_start: u64,
    visible_end: u64,
    cache: Rc<std::cell::RefCell<WaveformGeometryCache>>,
    grid: Option<WaveformGrid>,
) -> Option<Div> {
    preview_waveform_view_cached_inner(request, visible_start, visible_end, cache, grid)
        .map(|body| clip_waveform_activity(body, request.preview.active()))
}

/// A canonical silent prefix translates the accepted frame output unchanged.
/// Prove the exact model operation before borrowing peaks; a different root
/// cannot use this path. Work is bounded by one clip's event/point limits.
fn prefix_waveform_view(
    request: WaveformPreview<'_>,
    visible_start: u64,
    visible_end: u64,
    cache: Rc<std::cell::RefCell<WaveformGeometryCache>>,
    grid: Option<WaveformGrid>,
) -> Option<Div> {
    let WaveformPreview {
        original,
        preview,
        tempo,
        asset,
        prepared,
    } = request;
    let source_end = original
        .source_offset()
        .checked_add(original.source_length())?;
    let asset_frames = asset.map_or(source_end, |wave| wave.source_frames as u64);
    let (origin, source, mapping) = if let Some(old) = original.audio_edit() {
        let (_, length, offset, _) = old.projected_domain(tempo).ok()?;
        let origin = original.start().checked_sub(offset)?;
        let accepted = prepared?;
        if accepted.offset != offset
            || accepted.length != original.duration()
            || accepted.waveform.source_frames as u64 != length
            || accepted.preview_root.is_some()
        {
            return None;
        }
        (
            origin,
            Arc::clone(&accepted.waveform),
            WaveformMapping::Prepared {
                timeline_origin: origin,
                root_offset: 0,
                gain: preview.gain(),
            },
        )
    } else {
        let origin = original.start().checked_sub(original.source_offset())?;
        let source = Arc::clone(asset?);
        let rewound = original
            .preview_edge_trim(hex_project::TrimEdge::Left, origin, asset_frames)
            .ok()?;
        (
            origin,
            source,
            WaveformMapping::Clip {
                clip: rewound,
                timeline_origin: origin,
            },
        )
    };
    if preview.start() >= origin {
        return None;
    }
    let expected = original
        .preview_edge_trim_with_tempo(
            hex_project::TrimEdge::Left,
            preview.start(),
            asset_frames,
            tempo,
        )
        .ok()?;
    // Plain cached peaks are raw source samples, with fades painted separately.
    // Their existing lower-handle operation can borrow the same raw mapping.
    if &expected != preview {
        let lower_handle = original.audio_edit().is_none()
            && original
                .preview_fade_edge_trim(
                    hex_project::TrimEdge::Left,
                    preview.start(),
                    asset_frames,
                    tempo,
                )
                .ok()
                .as_ref()
                == Some(preview);
        if !lower_handle {
            return None;
        }
    }
    Some(timeline_waveform_view_cached(
        source,
        mapping,
        visible_start,
        visible_end,
        cache,
        grid,
    ))
}

fn preview_waveform_view_cached_inner(
    request: WaveformPreview<'_>,
    visible_start: u64,
    visible_end: u64,
    cache: Rc<std::cell::RefCell<WaveformGeometryCache>>,
    grid: Option<WaveformGrid>,
) -> Option<Div> {
    let WaveformPreview {
        original,
        preview,
        tempo,
        asset,
        prepared,
    } = request;
    if original.id() != preview.id() || original.asset() != preview.asset() {
        return None;
    }
    if let Some(edit) = preview.audio_edit() {
        // Placement sampling owns repeat wraps for both ordinary crop/loop
        // changes and canonical Shift-Warp root retiming.
        if edit.placement.is_some() {
            return super::waveform_loop::loop_waveform_view(
                request,
                visible_start,
                visible_end,
                cache,
                grid,
            );
        }
        if original
            .audio_edit()
            .is_none_or(|old| !Arc::ptr_eq(&old.domain, &edit.domain))
        {
            if let Some(prefix) =
                prefix_waveform_view(request, visible_start, visible_end, Rc::clone(&cache), grid)
            {
                return Some(prefix);
            }
            return super::waveform_stretch::stretch_waveform_view(
                request,
                visible_start,
                visible_end,
                cache,
                grid,
            );
        }
        let old = original.audio_edit()?;
        // A crop shares the immutable full processing root. A changed root must
        // supply its own validated preview projection, never raw-asset peaks.
        if !Arc::ptr_eq(&old.domain, &edit.domain) {
            return None;
        }
        if let (Some(before), Some(after)) = (old.content_view, edit.content_view) {
            let prepared = prepared?;
            if prepared.offset != 0
                || prepared.length != original.duration()
                || u64::try_from(prepared.waveform.source_frames).ok()? != original.duration()
                || before.window.reversed != after.window.reversed
            {
                return None;
            }
            let a = hex_project::AudioViewProjection::new(original, tempo).ok()?;
            let b = hex_project::AudioViewProjection::new(preview, tempo).ok()?;
            // Retain accepted finite output. Never treat its already-directed
            // PCM peaks as the full forward processing root a second time.
            let same_base = before.base_view_start == after.base_view_start
                && before.base_view_end == after.base_view_end
                && before.base_placement == after.base_placement;
            let linear = before.base_placement.is_none() && after.base_placement.is_none();
            if !(same_base || linear) || a.root_origin() != b.root_origin() {
                return None;
            }
            let displacement = b.base_reference_start() - a.base_reference_start();
            let elapsed = if before.window.reversed {
                i128::from(before.window.end) - i128::from(after.window.end) - displacement
            } else {
                i128::from(after.window.start) + displacement - i128::from(before.window.start)
            };
            let current = |clip: &Clip, edit: &hex_project::AudioClipEdit| {
                edit.reference_start_raw()
                    .unwrap_or(i128::from(clip.start()))
            };
            if current(preview, edit) - current(original, old) != elapsed {
                return None;
            }
            let (timeline_origin, root_offset) = if elapsed >= 0 {
                let elapsed = u64::try_from(elapsed).ok()?;
                if elapsed >= before.window.length().ok()? {
                    return None;
                }
                (
                    preview.start(),
                    a.frame_at_elapsed(elapsed)
                        .ok()?
                        .checked_sub(original.start())?,
                )
            } else {
                // Outward extension has no accepted peaks outside this range.
                // Keep the known finite PCM visible at its existing position.
                (original.start(), 0)
            };
            return Some(timeline_waveform_view_cached(
                Arc::clone(&prepared.waveform),
                WaveformMapping::Prepared {
                    timeline_origin,
                    root_offset,
                    gain: preview.gain(),
                },
                visible_start,
                visible_end,
                cache,
                grid,
            ));
        }
        if old.content_view.is_some() || edit.content_view.is_some() {
            return None;
        }
        let (old_origin, old_length, old_offset, _) = old.projected_domain(tempo).ok()?;
        let (origin, length, offset, end) = edit.projected_domain(tempo).ok()?;
        let prepared = prepared?;
        if origin != old_origin
            || length != old_length
            || u64::try_from(prepared.waveform.source_frames).ok()? != length
            || prepared.offset != old_offset
            || prepared.length != original.duration()
            || offset >= end
            || end > length
            || end - offset != preview.duration()
        {
            return None;
        }
        Some(timeline_waveform_view_cached(
            Arc::clone(&prepared.waveform),
            WaveformMapping::Prepared {
                timeline_origin: preview.start(),
                root_offset: offset,
                gain: preview.gain(),
            },
            visible_start,
            visible_end,
            cache,
            grid,
        ))
    } else {
        if original.audio_edit().is_some() {
            return None;
        }
        Some(timeline_waveform_view_cached(
            Arc::clone(asset?),
            WaveformMapping::Clip {
                clip: preview.clone(),
                timeline_origin: preview.start(),
            },
            visible_start,
            visible_end,
            cache,
            grid,
        ))
    }
}

pub(super) fn prepared_waveform_view_cached(
    prepared: &hex_session::service::AudioClipWaveform,
    clip: &Clip,
    visible_start: u64,
    visible_end: u64,
    cache: Rc<std::cell::RefCell<WaveformGeometryCache>>,
) -> Div {
    clip_waveform_activity(
        waveform_canvas(
            Arc::clone(&prepared.waveform),
            WaveformMapping::Prepared {
                timeline_origin: clip.start(),
                root_offset: prepared.offset,
                gain: clip.gain(),
            },
            visible_start,
            visible_end,
            Some(cache),
            None,
        ),
        clip.active(),
    )
}

/// Arrangement clips and both resize previews use the lane's stable lattice.
/// Other waveform consumers keep their own fitted source view.
pub(super) fn timeline_waveform_view_cached(
    waveform: Arc<CachedWaveform>,
    mapping: WaveformMapping,
    visible_start: u64,
    visible_end: u64,
    cache: Rc<std::cell::RefCell<WaveformGeometryCache>>,
    grid: Option<WaveformGrid>,
) -> Div {
    waveform_canvas(
        waveform,
        mapping,
        visible_start,
        visible_end,
        Some(cache),
        grid,
    )
}

pub(super) fn waveform_view_cached(
    waveform: &Arc<CachedWaveform>,
    clip: &Clip,
    visible_start: u64,
    visible_end: u64,
    cache: Rc<std::cell::RefCell<WaveformGeometryCache>>,
) -> Div {
    clip_waveform_activity(
        waveform_canvas(
            Arc::clone(waveform),
            WaveformMapping::Clip {
                clip: clip.clone(),
                timeline_origin: clip.start(),
            },
            visible_start,
            visible_end,
            Some(cache),
            None,
        ),
        clip.active(),
    )
}

pub(super) fn waveform_source_segment_view(
    waveform: Arc<CachedWaveform>,
    timeline_origin: u64,
    visible_start: u64,
    visible_end: u64,
) -> Div {
    waveform_canvas(
        waveform,
        WaveformMapping::Source { timeline_origin },
        visible_start,
        visible_end,
        None,
        None,
    )
}

pub(super) fn waveform_source_segment_gain_view_cached(
    waveform: Arc<CachedWaveform>,
    timeline_origin: u64,
    visible_start: u64,
    visible_end: u64,
    gain: hex_project::ClipGain,
    detail: bool,
    cache: Rc<std::cell::RefCell<WaveformGeometryCache>>,
) -> Div {
    waveform_canvas(
        waveform,
        WaveformMapping::GainedSource {
            timeline_origin,
            gain,
            detail,
        },
        visible_start,
        visible_end,
        Some(cache),
        None,
    )
}

pub(super) fn waveform_summary_view(waveform: Arc<CachedWaveform>) -> Div {
    let end = u64::try_from(waveform.source_frames).unwrap_or(u64::MAX);
    waveform_source_segment_view(waveform, 0, 0, end)
}

pub(super) fn waveform_canvas(
    waveform: Arc<CachedWaveform>,
    mapping: WaveformMapping,
    visible_start: u64,
    visible_end: u64,
    cache: Option<Rc<std::cell::RefCell<WaveformGeometryCache>>>,
    grid: Option<WaveformGrid>,
) -> Div {
    let mapping = mapping.clone();
    waveform_plot(
        move |paint| {
            #[cfg(all(test, feature = "ui-test-support"))]
            if grid.is_some() && super::navigation_metrics::waveforms_suppressed() {
                // Keep the actual canvas/layout/clip controls in the diagnostic
                // profile while excluding waveform preparation and submission.
                return None;
            }
            #[cfg(all(test, feature = "ui-test-support"))]
            let _navigation_timer = super::navigation_metrics::Timer::start(
                super::navigation_metrics::Stage::WaveformProvider,
            );
            let bounds = paint.bounds;
            let width = f32::from(bounds.size.width);
            let height = f32::from(bounds.size.height);
            if visible_start >= visible_end || width <= 0.0 || height <= 0.0 {
                return None;
            }
            let columns = waveform_column_count(width, paint.scale);
            let geometry = cache.as_ref().and_then(|cache| {
                let request = WaveformGeometryRequest {
                    mapping: &mapping,
                    visible_start,
                    visible_end,
                    bounds,
                    scale: paint.scale,
                    columns,
                    grid,
                };
                cached_waveform_geometry(cache, &waveform, &request)
            });
            let geometry = geometry.or_else(|| {
                prepare_waveform_geometry(
                    &waveform,
                    &mapping,
                    visible_start,
                    visible_end,
                    bounds,
                    columns,
                    grid,
                )
                .map(Arc::new)
            });
            #[cfg(all(test, feature = "ui-test-support"))]
            if let Some(geometry) = &geometry
                && let Some(cache) = &cache
                && let Ok(mut cache) = cache.try_borrow_mut()
            {
                cache.record_paint(bounds, paint.mask, geometry);
            }
            geometry
        },
        WaveformColors {
            waveform: 0x000000,
            inactive: 0x000000,
            centerline: theme::color(theme::PaletteRole::Border),
        },
        WaveformEmphasis::Uniform,
    )
}

pub(super) fn waveform_column_count(width: f32, scale: f32) -> usize {
    if !width.is_finite() || width <= 0.0 || !scale.is_finite() || scale <= 0.0 {
        return 1;
    }
    ((width * scale).ceil() as usize).clamp(1, MAX_WAVEFORM_COLUMNS)
}

pub(super) fn peak_vertical_bounds(peak: CachedPeak, center: f32, amplitude: f32) -> (f32, f32) {
    let top = center - peak.max.clamp(-1.0, 1.0) * amplitude;
    let bottom = center - peak.min.clamp(-1.0, 1.0) * amplitude;
    (top.min(bottom), top.max(bottom))
}

// Sample inside each fade as well as across the viewport, so a short curved fade
// on a long clip never collapses to a straight line between its endpoint knots.
fn fade_domain_positions(fade: ClipFade) -> impl Iterator<Item = u64> {
    // Invert the canonical two-axis law for drawing coordinates only. Uniform
    // gain spacing resolves steep bends; playback still uses gain_at directly.
    let at_gain = move |length: u64, index: usize, out: bool| {
        let fraction = fade
            .position_at_gain(out, index as f64 / 32.0)
            .unwrap_or(0.5);
        ((fraction * length.saturating_sub(1) as f64).round() as u64).min(length.saturating_sub(1))
    };
    let out_end = fade.span() - fade.out_end_offset();
    let midpoints = [
        (fade.in_curve() != 0 || fade.in_position() != 500)
            .then_some(fade.in_start() + fade.fade_in().saturating_sub(1) / 2),
        (fade.out_curve() != 0 || fade.out_position() != 500)
            .then_some(out_end - 1 - fade.fade_out().saturating_sub(1) / 2),
    ];
    fade.breakpoints()
        .into_iter()
        .flatten()
        .chain(midpoints.into_iter().flatten())
        .chain(
            (1..32)
                .flat_map(move |index| {
                    [
                        ((fade.in_curve() != 0 || fade.in_position() != 500) && fade.fade_in() > 1)
                            .then(|| fade.in_start() + at_gain(fade.fade_in(), index, false)),
                        ((fade.out_curve() != 0 || fade.out_position() != 500)
                            && fade.fade_out() > 1)
                            .then(|| {
                                out_end - fade.fade_out() + at_gain(fade.fade_out(), index, true)
                            }),
                    ]
                })
                .flatten(),
        )
}

/// Return a viewport-clipped, bounded preview of the canonical fade gain law.
/// X and gain are normalized to [0, 1]; sample phases stay in clip timeline
/// frames, independent of looping source-media phase.
pub(super) fn projected_fade_envelope_samples(
    clip: &Clip,
    tempo: &hex_project::TempoMap,
    visible_start: u64,
    visible_end: u64,
) -> Result<Vec<(f32, f32)>, hex_project::Invalid> {
    let edit = clip
        .audio_edit()
        .ok_or(hex_project::Invalid("audio clip has no projected fade"))?;
    let fade = edit.projected_fade(tempo)?;
    let (_, _, view_start, _) = edit.projected_domain(tempo)?;
    if visible_start < clip.start()
        || visible_start >= visible_end
        || visible_end > clip.start().saturating_add(clip.duration())
    {
        return Ok(Vec::new());
    }
    let length = visible_end - visible_start;
    let count = length.min(FADE_ENVELOPE_SAMPLES as u64) as usize;
    let root_start = view_start
        .checked_add(visible_start - clip.start())
        .ok_or(hex_project::Invalid("fade geometry frame overflow"))?;
    let root_end = root_start
        .checked_add(length - 1)
        .ok_or(hex_project::Invalid("fade geometry frame overflow"))?;
    let capacity = count + 3 * FADE_ENVELOPE_BREAKPOINTS;
    let mut positions = Vec::new();
    positions
        .try_reserve_exact(capacity)
        .map_err(|_| hex_project::Invalid("fade geometry allocation failed"))?;
    for index in 0..count {
        positions.push(root_start + fraction_u64(length - 1, index, count.saturating_sub(1)));
    }
    for position in fade_domain_positions(clip.retained_fade()) {
        let Some(frame) = fade.frame_at_domain_position(position)? else {
            continue;
        };
        for frame in [frame.saturating_sub(1), frame, frame.saturating_add(1)] {
            if frame >= root_start && frame <= root_end {
                positions.push(frame);
            }
        }
    }
    positions.sort_unstable();
    positions.dedup();
    let mut result = Vec::new();
    result
        .try_reserve_exact(positions.len())
        .map_err(|_| hex_project::Invalid("fade geometry allocation failed"))?;
    for frame in positions {
        let x = if length == 1 {
            0.0
        } else {
            (frame - root_start) as f32 / (length - 1) as f32
        };
        result.push((x, fade.gain_at(frame)?));
    }
    Ok(result)
}

pub(super) fn fade_envelope_samples(
    fade: ClipFade,
    clip_start: u64,
    visible_start: u64,
    visible_end: u64,
) -> Vec<(f32, f32)> {
    if fade.is_neutral() || visible_start < clip_start || visible_start >= visible_end {
        return Vec::new();
    }
    let visible_frames = visible_end - visible_start;
    let sample_count = usize::try_from(visible_frames.min(FADE_ENVELOPE_SAMPLES as u64))
        .unwrap_or(FADE_ENVELOPE_SAMPLES)
        .max(1);
    let Some(local_start) = visible_start.checked_sub(clip_start) else {
        return Vec::new();
    };
    let local_span = visible_frames.saturating_sub(1);
    let Some(local_end) = local_start.checked_add(local_span) else {
        return Vec::new();
    };
    let mut frames = Vec::with_capacity(sample_count + FADE_ENVELOPE_BREAKPOINTS);
    for index in 0..sample_count {
        frames.push(local_start.saturating_add(fraction_u64(
            local_span,
            index,
            sample_count.saturating_sub(1),
        )));
    }
    let mut add_domain_position = |position: u64| {
        if let Some(frame) = position.checked_sub(fade.offset())
            && frame >= local_start
            && frame <= local_end
        {
            frames.push(frame);
        }
    };
    for position in fade_domain_positions(fade) {
        add_domain_position(position);
    }
    frames.sort_unstable();
    frames.dedup();
    let mut points = Vec::with_capacity(frames.len());
    for frame in frames {
        let x = if local_span == 0 {
            0.0
        } else {
            (frame - local_start) as f32 / local_span as f32
        };
        points.push((x, fade.gain_at(frame).clamp(0.0, 1.0)));
    }
    if points.iter().all(|(_, gain)| *gain >= 1.0) {
        return Vec::new();
    }
    points
}

pub(super) fn fraction_u64(value: u64, numerator: usize, denominator: usize) -> u64 {
    if denominator == 0 {
        return 0;
    }
    ((u128::from(value) * numerator as u128) / denominator as u128) as u64
}

/// A display column can touch at most two source intervals at a loop wrap.
/// Keep these intervals on the stack rather than allocating a vector per column.
#[derive(Default)]
pub(super) struct SourceRanges {
    ranges: [Range<usize>; 2],
    len: usize,
}
impl SourceRanges {
    pub(super) fn as_slice(&self) -> &[Range<usize>] {
        &self.ranges[..self.len]
    }
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Map a visible timeline interval to its active source region, splitting at a loop wrap.
#[cfg(test)]
fn timeline_source_ranges(
    clip: &Clip,
    visible: Range<u64>,
    available_frames: usize,
) -> SourceRanges {
    timeline_source_ranges_at(clip, clip.start(), visible, available_frames)
}

pub(super) fn timeline_source_ranges_at(
    clip: &Clip,
    timeline_origin: u64,
    visible: Range<u64>,
    available_frames: usize,
) -> SourceRanges {
    // Transformed audio has a prepared root projection. An original asset peak
    // map cannot describe it, including not-yet-prepared drag/trim previews.
    if clip.audio_edit().is_some() {
        return SourceRanges::default();
    }
    let Some(clip_end) = timeline_origin.checked_add(clip.duration()) else {
        return SourceRanges::default();
    };
    let Some(source_end) = clip.source_offset().checked_add(clip.source_length()) else {
        return SourceRanges::default();
    };
    if clip.source_length() == 0
        || source_end > available_frames as u64
        || visible.start >= visible.end
    {
        return SourceRanges::default();
    }
    let start = visible.start.max(timeline_origin);
    let end = visible.end.min(clip_end);
    if start >= end {
        return SourceRanges::default();
    }
    let relative_start = start - timeline_origin;
    let span = end - start;
    let mut ranges = SourceRanges::default();
    if !clip.looping() {
        let active_end = (relative_start.saturating_add(span)).min(clip.source_length());
        let active_start = relative_start.min(clip.source_length());
        if active_start < active_end {
            push_source_range(
                &mut ranges,
                clip.source_offset() + active_start,
                clip.source_offset() + active_end,
                available_frames,
            );
        }
        return ranges;
    }

    if span >= clip.source_length() {
        push_source_range(
            &mut ranges,
            clip.source_offset(),
            source_end,
            available_frames,
        );
        return ranges;
    }
    let phase_start = ((u128::from(clip.source_phase() % clip.source_length())
        + u128::from(relative_start % clip.source_length()))
        % u128::from(clip.source_length())) as u64;
    let phase_end = u128::from(phase_start) + u128::from(span);
    if phase_end <= u128::from(clip.source_length()) {
        push_source_range(
            &mut ranges,
            clip.source_offset() + phase_start,
            clip.source_offset() + phase_end as u64,
            available_frames,
        );
    } else {
        push_source_range(
            &mut ranges,
            clip.source_offset() + phase_start,
            source_end,
            available_frames,
        );
        push_source_range(
            &mut ranges,
            clip.source_offset(),
            clip.source_offset() + (phase_end - u128::from(clip.source_length())) as u64,
            available_frames,
        );
    }
    ranges
}

pub(super) fn push_source_range(
    ranges: &mut SourceRanges,
    start: u64,
    end: u64,
    available_frames: usize,
) {
    let (Ok(start), Ok(end)) = (usize::try_from(start), usize::try_from(end)) else {
        return;
    };
    if start < end
        && end <= available_frames
        && let Some(slot) = ranges.ranges.get_mut(ranges.len)
    {
        *slot = start..end;
        ranges.len += 1;
    }
}

pub(super) fn aggregate_peaks(
    waveform: &CachedWaveform,
    start: usize,
    end: usize,
) -> Option<[CachedPeak; 2]> {
    waveform.peaks_for_frames(start, end)
}

pub(super) fn aggregate_ranges(
    waveform: &CachedWaveform,
    ranges: &[Range<usize>],
) -> Option<[CachedPeak; 2]> {
    let mut merged = None;
    for range in ranges {
        let peaks = aggregate_peaks(waveform, range.start, range.end)?;
        if let Some(current) = merged.as_mut() {
            let current: &mut [CachedPeak; 2] = current;
            for channel in 0..2 {
                current[channel].min = current[channel].min.min(peaks[channel].min);
                current[channel].max = current[channel].max.max(peaks[channel].max);
            }
        } else {
            merged = Some(peaks);
        }
    }
    merged
}

pub(super) fn button(
    label: &'static str,
    tooltip: &'static str,
    id: (&'static str, u64),
) -> gpui::Stateful<Div> {
    button_surface(
        hex_widgets::action(theme::text(label), theme::action_style()),
        tooltip,
        id,
    )
}

/// Tooltip/identity adapter; stateful callers resolve their hover colors before
/// the shared widget is built, rather than installing a second GPUI hover style.
pub(super) fn toggle_button_with_style(
    label: &'static str,
    tooltip: &'static str,
    id: (&'static str, u64),
    style: hex_widgets::ActionStyle,
) -> gpui::Stateful<Div> {
    button_surface(
        hex_widgets::toggle_action(theme::text(label), style),
        tooltip,
        id,
    )
}

fn button_surface(
    surface: Div,
    tooltip: &'static str,
    id: (&'static str, u64),
) -> gpui::Stateful<Div> {
    surface
        .id(id)
        .w(px(19.0))
        .h(px(21.0))
        .text_xs()
        .tooltip(move |_, cx| {
            cx.new(|_| ClipTooltip {
                kind: crate::tooltips::Kind::Identification,
                name: tooltip.to_string(),
            })
            .into()
        })
}

pub(super) struct ClipTooltip {
    pub(super) kind: crate::tooltips::Kind,
    pub(super) name: String,
}

impl Render for ClipTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::tooltips::view(&self.kind, &self.name, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project};

    fn peak(value: f32) -> CachedPeak {
        CachedPeak {
            min: value,
            max: value,
        }
    }

    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn waveform_canvas_paints_full_clip_extent(cx: &mut gpui::TestAppContext) {
        use gpui::{IntoElement, div, point, size};
        use std::{cell::Cell, rc::Rc};

        let visual = cx.add_empty_window();
        for width in [8.0, 260.0] {
            let painted = Rc::new(Cell::new(None));
            let result = Rc::clone(&painted);
            visual.draw(
                point(px(0.0), px(0.0)),
                size(px(width), px(44.0)),
                |_, _| {
                    div()
                        .w(px(width))
                        .h(px(44.0))
                        .relative()
                        .children([waveform_plot(
                            move |paint| {
                                result.set(Some(paint.bounds));
                                None::<Rc<hex_widgets::WaveformGeometry>>
                            },
                            WaveformColors {
                                waveform: 0,
                                inactive: 0,
                                centerline: 0,
                            },
                            WaveformEmphasis::Uniform,
                        )])
                        .into_element()
                },
            );
            let bounds = painted.get().expect("waveform paint callback ran");
            assert_eq!(bounds.size, size(px(width), px(44.0)));
        }
    }

    #[test]
    fn waveform_density_tracks_device_pixels_and_stays_bounded() {
        assert_eq!(waveform_column_count(120.0, 1.0), 120);
        assert_eq!(waveform_column_count(120.0, 2.0), 240);
        assert_eq!(waveform_column_count(100_000.0, 2.0), MAX_WAVEFORM_COLUMNS);
        assert_eq!(waveform_column_count(f32::NAN, 2.0), 1);
    }

    #[test]
    fn waveform_peak_geometry_preserves_signed_channel_extrema() {
        let positive = peak_vertical_bounds(
            CachedPeak {
                min: 0.25,
                max: 0.75,
            },
            10.0,
            8.0,
        );
        assert!(positive.0 < positive.1 && positive.1 < 10.0);
        let negative = peak_vertical_bounds(
            CachedPeak {
                min: -0.75,
                max: -0.25,
            },
            10.0,
            8.0,
        );
        assert!(negative.0 > 10.0 && negative.1 > negative.0);
    }

    fn clip(start: u64, duration: u64, offset: u64, length: u64, looping: bool) -> Clip {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 2).expect("editor");
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "track".into(),
                path: "audio.wav".into(),
                sample_rate: 48_000,
                frames: 32,
            })
            .expect("media import")
        else {
            panic!("unexpected import result");
        };
        let Created::Clip(id) = editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start,
                duration,
                source_offset: offset,
                source_length: length,
                looping,
            })
            .expect("place clip")
        else {
            panic!("unexpected clip result");
        };
        editor.project().clip(id).expect("clip").clone()
    }

    fn trimmed_loop_after_left_trim() -> Clip {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 4).expect("editor");
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "track".into(),
                path: "loop.wav".into(),
                sample_rate: 48_000,
                frames: 32,
            })
            .expect("media import")
        else {
            panic!("unexpected import result");
        };
        let Created::Clip(id) = editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 100,
                duration: 20,
                source_offset: 10,
                source_length: 4,
                looping: true,
            })
            .expect("place loop")
        else {
            panic!("unexpected clip result");
        };
        editor
            .execute(Command::TrimClipEdge {
                clip: id,
                edge: hex_project::TrimEdge::Left,
                frame: 101,
            })
            .expect("trim loop");
        editor.project().clip(id).expect("trimmed loop").clone()
    }

    #[test]
    fn cached_queries_preserve_signed_extrema_across_uneven_bins() {
        let waveform = CachedWaveform::from_peaks(
            11,
            vec![
                [peak(0.0), peak(0.0)],
                [peak(0.2), peak(0.2)],
                [peak(0.8), peak(0.8)],
            ],
        )
        .expect("valid cache");
        assert_eq!(aggregate_peaks(&waveform, 0, 4).unwrap()[0].max, 0.0);
        assert_eq!(aggregate_peaks(&waveform, 4, 7).unwrap()[0].max, 0.2);
        assert_eq!(aggregate_peaks(&waveform, 7, 11).unwrap()[0].max, 0.8);
        assert_eq!(waveform.cached_bin_count(), 6);
    }

    #[test]
    fn display_group_merges_all_bins_to_keep_transient_extrema() {
        let waveform = CachedWaveform::from_peaks(
            12,
            vec![
                [peak(0.0), peak(0.0)],
                [
                    CachedPeak {
                        min: -1.0,
                        max: 0.9,
                    },
                    peak(0.7),
                ],
                [peak(0.0), peak(0.0)],
            ],
        )
        .expect("valid cache");
        let peaks = aggregate_peaks(&waveform, 0, 12).expect("nonempty group");
        assert_eq!(peaks[0].min, -1.0);
        assert_eq!(peaks[0].max, 0.9);
        assert_eq!(peaks[1].max, 0.7);
    }

    #[test]
    fn cropped_one_shot_maps_only_its_visible_active_source_frames() {
        assert_eq!(
            timeline_source_ranges(&clip(100, 20, 10, 8, false), 105..118, 32).as_slice(),
            std::slice::from_ref(&(15..18))
        );
        assert!(timeline_source_ranges(&clip(100, 20, 10, 8, false), 108..116, 32).is_empty());
    }

    #[test]
    fn cropped_loop_maps_phase_and_splits_at_wrap() {
        assert_eq!(
            timeline_source_ranges(&clip(100, 20, 10, 4, true), 102..105, 32).as_slice(),
            &[12..14, 10..11]
        );
        assert_eq!(
            timeline_source_ranges(&clip(100, 20, 10, 4, true), 101..106, 32).as_slice(),
            std::slice::from_ref(&(10..14))
        );
    }

    #[test]
    fn cropped_loop_waveform_uses_trimmed_source_phase_for_transients() {
        let clip = trimmed_loop_after_left_trim();
        assert_eq!(clip.start(), 101);
        assert_eq!(clip.source_phase(), 1);
        let ranges = timeline_source_ranges(&clip, 103..106, 32);
        assert_eq!(ranges.as_slice(), &[13..14, 10..12]);

        let waveform = CachedWaveform::from_peaks(
            32,
            (0..32)
                .map(|frame| [peak(frame as f32 / 32.0), peak(frame as f32 / 32.0)])
                .collect(),
        )
        .expect("valid cache");
        let peaks = aggregate_ranges(&waveform, ranges.as_slice()).expect("visible loop peaks");
        assert_eq!(peaks[0].max, 13.0 / 32.0);
        assert_eq!(peaks[0].min, 10.0 / 32.0);
    }

    #[test]
    fn fade_envelope_samples_are_clipped_and_use_the_canonical_inherited_phase() {
        let fade = hex_project::ClipFade::new(1_000, 400, 600, 500).expect("inherited fade");
        let samples = fade_envelope_samples(fade, 200, 240, 260);
        assert_eq!(samples.len(), 20);
        assert!(samples.len() <= FADE_ENVELOPE_SAMPLES + FADE_ENVELOPE_BREAKPOINTS);
        assert_eq!(samples.first().unwrap().0, 0.0);
        assert_eq!(samples.last().unwrap().0, 1.0);
        assert_eq!(samples.first().unwrap().1, fade.gain_at(40));
        assert_eq!(samples.last().unwrap().1, fade.gain_at(59));
        assert!(fade_envelope_samples(fade, 200, 190, 220).is_empty());
        assert!(fade_envelope_samples(fade, 200, 240, 240).is_empty());
        let neutral = ClipFade::neutral(100).expect("neutral fade");
        assert!(fade_envelope_samples(neutral, 200, 200, 220).is_empty());
        let outside_fade = ClipFade::new(100, 40, 10, 10).expect("off-segment fade");
        assert!(fade_envelope_samples(outside_fade, 200, 220, 240).is_empty());
    }

    #[test]
    fn short_fade_breakpoint_stays_local_on_a_long_visible_clip() {
        let fade = ClipFade::new(1_000_000, 0, 241, 0).expect("short fade");
        let samples = fade_envelope_samples(fade, 0, 0, 1_000_000);
        assert!(samples.len() <= FADE_ENVELOPE_SAMPLES + FADE_ENVELOPE_BREAKPOINTS);
        assert_eq!(samples.first().unwrap().1, 0.0);
        let end = samples
            .iter()
            .find(|(x, _)| (*x - 240.0 / 999_999.0).abs() < f32::EPSILON)
            .expect("exact fade-in end breakpoint");
        assert_eq!(end.1, 1.0);
        let curved = fade.with_shape(0, 0, 800, 0).unwrap();
        let samples = fade_envelope_samples(curved, 0, 0, 1_000_000);
        let midpoint = samples
            .iter()
            .find(|(x, _)| (*x - 120.0 / 999_999.0).abs() < f32::EPSILON)
            .expect("short curve interior survives zoom out");
        assert_eq!(midpoint.1, curved.gain_at(120));
        assert!(midpoint.1 < 0.2);
        for (horizontal, knot) in [
            (fade.with_curve_positions(250, 500).unwrap(), 60),
            (
                ClipFade::new(1_000_000, 0, 0, 241)
                    .unwrap()
                    .with_curve_positions(500, 750)
                    .unwrap(),
                999_939,
            ),
        ] {
            let samples = fade_envelope_samples(horizontal, 0, 0, 1_000_000);
            let anchor = samples
                .iter()
                .find(|(x, _)| (*x - knot as f32 / 999_999.0).abs() < f32::EPSILON)
                .expect("horizontal-only curve anchor survives zoom out");
            assert_eq!(anchor.1, 0.5);
        }
    }

    #[test]
    fn inherited_fade_breakpoints_use_domain_positions_before_offset_translation() {
        let fade = ClipFade::new(1_000, 200, 250, 400).expect("inherited fade");
        let samples = fade_envelope_samples(fade, 1_000, 1_000, 1_800);
        let at_local_frame = |frame: u64| {
            samples
                .iter()
                .find(|(x, _)| (*x - frame as f32 / 799.0).abs() < f32::EPSILON)
                .copied()
                .expect("exact inherited breakpoint")
        };
        assert_eq!(at_local_frame(49).1, 1.0); // fade-in endpoint
        assert_eq!(at_local_frame(399).1, 1.0); // before fade-out
        assert_eq!(at_local_frame(400).1, 1.0); // fade-out start
        assert_eq!(at_local_frame(799).1, 0.0); // original span endpoint

        let fade_in_crop = fade_envelope_samples(fade, 1_000, 1_040, 1_060);
        assert!(
            fade_in_crop
                .iter()
                .any(|(x, gain)| { (*x - 9.0 / 19.0).abs() < f32::EPSILON && *gain == 1.0 })
        );
        let fade_out_crop = fade_envelope_samples(fade, 1_000, 1_390, 1_410);
        assert!(
            fade_out_crop
                .iter()
                .any(|(x, gain)| (*x - 9.0 / 19.0).abs() < f32::EPSILON && *gain == 1.0)
        );
        assert!(
            fade_out_crop
                .iter()
                .any(|(x, gain)| (*x - 10.0 / 19.0).abs() < f32::EPSILON && *gain == 1.0)
        );
    }

    #[test]
    fn loop_waveform_preserves_peaks_across_the_phase_wrap() {
        let waveform = CachedWaveform::from_peaks(
            6,
            vec![
                [peak(0.1), peak(0.1)],
                [peak(0.9), peak(0.2)],
                [peak(0.3), peak(0.3)],
            ],
        )
        .expect("valid cache");
        let ranges = timeline_source_ranges(&clip(0, 8, 1, 4, true), 2..4, 6);
        let peaks = aggregate_ranges(&waveform, ranges.as_slice()).expect("wrapped visible loop");
        assert_eq!(peaks[0].max, 0.9);
        assert_eq!(peaks[1].max, 0.3);
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "waveform_resize_ui_test.rs"]
mod resize_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "waveform_header_ui_test.rs"]
mod waveform_header_ui_test;
