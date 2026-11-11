use crate::theme;
use gpui::{Context, Div, Window, prelude::*, px};
use hex_project::Clip;
use hex_session::CachedWaveform;
use hex_widgets::{WaveformColors, WaveformEmphasis, waveform_plot};
use std::{ops::Range, rc::Rc, sync::Arc};

use super::waveform_geometry::{
    WaveformGeometryCache, WaveformGeometryRequest, WaveformMapping, cached_waveform_geometry,
    prepare_waveform_geometry,
};

pub(super) use super::waveform_geometry::WaveformGrid;
pub(super) use super::waveform_source::{
    SourceRanges, aggregate_peaks, aggregate_ranges, aggregate_ranges_with_detail,
    push_source_range, timeline_source_ranges_at,
};

mod paint_geometry;
pub(super) use paint_geometry::{
    WaveformAmplitudeScale, fade_envelope_samples, fraction_u64, peak_vertical_bounds,
    projected_fade_envelope_samples,
};

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
    pub(super) ink: u32,
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
        ink,
    } = request;
    let source_end = original
        .source_offset()
        .checked_add(original.source_length())?;
    let asset_frames = asset.map_or(source_end, |wave| wave.source_frames() as u64);
    let origin = if let Some(old) = original.audio_edit() {
        if !matches!(old.domain.clock, hex_project::AudioDomainClock::Frames(_)) {
            return None;
        }
        let projection = hex_project::AudioViewProjection::new(original, tempo).ok()?;
        u64::try_from(projection.root_origin()).ok()?
    } else {
        original.start().checked_sub(original.source_offset())?
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
    if &expected != preview
        && (original.audio_edit().is_some()
            || original
                .preview_fade_edge_trim(
                    hex_project::TrimEdge::Left,
                    preview.start(),
                    asset_frames,
                    tempo,
                )
                .ok()
                .as_ref()
                != Some(preview))
    {
        return None;
    }
    let rewound = original
        .preview_edge_trim_with_tempo(hex_project::TrimEdge::Left, origin, asset_frames, tempo)
        .ok()?;
    if rewound
        .audio_edit()
        .is_some_and(|edit| edit.placement.is_some())
    {
        // The original root, rewound to zero, already has the exact sounding
        // interval of the prefixed clip. Its finite gate excludes hidden PCM
        // after the Once cycle end even if the clip has a silent tail.
        return super::waveform_loop::loop_waveform_view(
            WaveformPreview {
                preview: &rewound,
                ..request
            },
            visible_start,
            visible_end,
            cache,
            grid,
        );
    }
    let (source, mapping) = if let Some(old) = original.audio_edit() {
        let (_, length, offset, _) = old.projected_domain(tempo).ok()?;
        let accepted = prepared?;
        if accepted.offset != offset
            || accepted.length != original.duration()
            || accepted.waveform.source_frames() as u64 != length
            || accepted.preview_root.is_some()
        {
            return None;
        }
        (
            Arc::clone(&accepted.waveform),
            WaveformMapping::Prepared {
                timeline_origin: origin,
                root_offset: 0,
                gain: preview.gain(),
            },
        )
    } else {
        let source = Arc::clone(asset?);
        (
            source,
            WaveformMapping::Clip {
                fade: Some(rewound.retained_fade()),
                clip: rewound,
                timeline_origin: origin,
            },
        )
    };
    Some(timeline_waveform_view_cached(
        source,
        mapping,
        visible_start,
        visible_end,
        cache,
        grid,
        ink,
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
        ink,
    } = request;
    if original.id() != preview.id() || original.asset() != preview.asset() {
        return None;
    }
    if let Some(edit) = preview.audio_edit() {
        let changed_root = original
            .audio_edit()
            .is_none_or(|old| !Arc::ptr_eq(&old.domain, &edit.domain));
        if changed_root
            && let Some(prefix) =
                prefix_waveform_view(request, visible_start, visible_end, Rc::clone(&cache), grid)
        {
            return Some(prefix);
        }
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
        if changed_root {
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
                || u64::try_from(prepared.waveform.source_frames()).ok()? != original.duration()
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
                ink,
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
            || u64::try_from(prepared.waveform.source_frames()).ok()? != length
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
            ink,
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
                fade: Some(preview.retained_fade()),
            },
            visible_start,
            visible_end,
            cache,
            grid,
            ink,
        ))
    }
}

pub(super) fn prepared_waveform_view_cached(
    prepared: &hex_session::service::AudioClipWaveform,
    clip: &Clip,
    visible_start: u64,
    visible_end: u64,
    cache: Rc<std::cell::RefCell<WaveformGeometryCache>>,
    ink: u32,
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
            ink,
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
    ink: u32,
) -> Div {
    waveform_canvas(
        waveform,
        mapping,
        visible_start,
        visible_end,
        Some(cache),
        grid,
        ink,
    )
}

pub(super) fn waveform_view_cached(
    waveform: &Arc<CachedWaveform>,
    clip: &Clip,
    visible_start: u64,
    visible_end: u64,
    cache: Rc<std::cell::RefCell<WaveformGeometryCache>>,
    ink: u32,
) -> Div {
    clip_waveform_activity(
        waveform_canvas(
            Arc::clone(waveform),
            WaveformMapping::Clip {
                clip: clip.clone(),
                timeline_origin: clip.start(),
                fade: None,
            },
            visible_start,
            visible_end,
            Some(cache),
            None,
            ink,
        ),
        clip.active(),
    )
}

pub(super) fn waveform_source_segment_view(
    waveform: Arc<CachedWaveform>,
    timeline_origin: u64,
    visible_start: u64,
    visible_end: u64,
    ink: u32,
) -> Div {
    waveform_canvas(
        waveform,
        WaveformMapping::Source { timeline_origin },
        visible_start,
        visible_end,
        None,
        None,
        ink,
    )
}

pub(super) fn waveform_source_segment_gain_view_cached(
    waveform: Arc<CachedWaveform>,
    timeline_origin: u64,
    visible: Range<u64>,
    gain: hex_project::ClipGain,
    detail: bool,
    cache: Rc<std::cell::RefCell<WaveformGeometryCache>>,
    ink: u32,
) -> Div {
    waveform_canvas(
        waveform,
        WaveformMapping::GainedSource {
            timeline_origin,
            gain,
            detail,
        },
        visible.start,
        visible.end,
        Some(cache),
        None,
        ink,
    )
}

pub(super) fn waveform_summary_view(waveform: Arc<CachedWaveform>, ink: u32) -> Div {
    let end = u64::try_from(waveform.source_frames()).unwrap_or(u64::MAX);
    waveform_source_segment_view(waveform, 0, 0, end, ink)
}

pub(super) fn waveform_canvas(
    waveform: Arc<CachedWaveform>,
    mapping: WaveformMapping,
    visible_start: u64,
    visible_end: u64,
    cache: Option<Rc<std::cell::RefCell<WaveformGeometryCache>>>,
    grid: Option<WaveformGrid>,
    ink: u32,
) -> Div {
    let mapping = mapping.clone();
    let colors = WaveformColors {
        waveform: ink,
        inactive: ink,
        centerline: theme::color(theme::PaletteRole::Border),
    };
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
                    amplitude_scale: crate::editor::waveform::WaveformAmplitudeScale::from_decibels(
                        false,
                    ),
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
                cache.record_paint(bounds, paint.mask, geometry, colors);
            }
            geometry
        },
        colors,
        WaveformEmphasis::Uniform,
    )
}

pub(super) use super::waveform_columns::waveform_column_count;

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

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;

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
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "waveform_resize_ui_test.rs"]
mod resize_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "waveform_header_ui_test.rs"]
mod waveform_header_ui_test;
