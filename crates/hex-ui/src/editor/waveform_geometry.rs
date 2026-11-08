//! Bounded retained waveform paths for the visible editor working set.
use gpui::{Bounds, Pixels, point, px};
use hex_project::Clip;
use hex_session::{CachedPeak, CachedWaveform};
use hex_widgets::{
    MAX_WAVEFORM_COLUMNS, MAX_WAVEFORM_ENVELOPE_POINTS, WaveformChannel, WaveformEnvelope,
    WaveformEnvelopeMode, WaveformStroke,
};
#[cfg(test)]
use std::hint::black_box;
use std::{collections::VecDeque, sync::Arc};

use super::waveform::{
    aggregate_peaks, aggregate_ranges, fraction_u64, peak_vertical_bounds,
    timeline_source_ranges_at,
};

// A fitted dense viewport has 350 small audio clips while using only a few
// thousand columns. Retain that working set without lifting the geometry budget.
const MAX_CACHED_WAVEFORM_ENTRIES: usize = 512;
const MAX_CACHED_WAVEFORM_COLUMNS: usize = 65_536;

/// UI-owned LRU of ready-to-paint waveform geometry. Entries retain their source
/// Arc, so pointer identity cannot be reused while an entry is resident.
#[derive(Default)]
pub(super) struct WaveformGeometryCache {
    entries: VecDeque<WaveformGeometryEntry>,
    columns: usize,
    #[cfg(all(test, feature = "ui-test-support"))]
    painted: VecDeque<WaveformPaintSample>,
}

struct WaveformGeometryEntry {
    key: WaveformGeometryKey,
    source: Arc<CachedWaveform>,
    geometry: Arc<WaveformGeometry>,
    columns: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WaveformGeometryKey {
    mapping: WaveformMappingKey,
    visible_start: u64,
    visible_end: u64,
    origin_x: u32,
    origin_y: u32,
    width: u32,
    height: u32,
    scale: u32,
    grid: Option<[u64; 6]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum WaveformMappingKey {
    MusicalEvent(super::audio_event_editor::MusicalEventWaveform),
    Stretch(Arc<super::waveform_stretch::StretchProjection>),
    Loop(Arc<super::waveform_loop::LoopProjection>),
    Prepared {
        timeline_origin: u64,
        root_offset: u64,
        gain: i32,
    },
    GainedSource {
        timeline_origin: u64,
        gain: i32,
        detail: bool,
    },
    Clip {
        id: u64,
        start: u64,
        duration: u64,
        source_offset: u64,
        source_length: u64,
        source_phase: u64,
        looping: bool,
        timeline_origin: u64,
        gain: i32,
    },
    Source {
        timeline_origin: u64,
    },
}

pub(super) struct WaveformGeometry {
    display: hex_widgets::WaveformGeometry,
    #[cfg(test)]
    pub(super) channels: [WaveformChannelGeometry; 2],
    // X positions per channel, including both edges of coarse bins. Charge
    // these actual display positions to the existing cache's column budget.
    columns: usize,
}

#[cfg(test)]
pub(super) struct WaveformChannelGeometry {
    outline: Vec<gpui::Point<Pixels>>,
}
#[cfg(test)]
impl WaveformGeometry {
    fn channels(&self) -> &[WaveformChannelGeometry] {
        &self.channels[..self.display.channels().len()]
    }
}

impl AsRef<hex_widgets::WaveformGeometry> for WaveformGeometry {
    fn as_ref(&self) -> &hex_widgets::WaveformGeometry {
        &self.display
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) struct RenderedWaveformSample {
    pub(super) source: Arc<CachedWaveform>,
    pub(super) prepared: Option<(u64, u64)>,
    pub(super) spans: [f32; 2],
}

#[cfg(all(test, feature = "ui-test-support"))]
#[derive(Clone, Copy, Debug)]
pub(super) struct WaveformPaintSample {
    pub(super) colors: hex_widgets::WaveformColors,
    pub(super) bounds: Bounds<Pixels>,
    pub(super) mask: Bounds<Pixels>,
    pub(super) ordinates: [(f32, f32); 2],
    pub(super) envelope_points: usize,
}

impl WaveformGeometryCache {
    fn get(
        &mut self,
        key: WaveformGeometryKey,
        source: &Arc<CachedWaveform>,
    ) -> Option<Arc<WaveformGeometry>> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.key == key && Arc::ptr_eq(&entry.source, source))?;
        let entry = self.entries.remove(index)?;
        let geometry = Arc::clone(&entry.geometry);
        self.entries.push_back(entry);
        Some(geometry)
    }

    fn insert(
        &mut self,
        key: WaveformGeometryKey,
        source: Arc<CachedWaveform>,
        geometry: Arc<WaveformGeometry>,
        columns: usize,
    ) {
        if columns == 0 || columns > MAX_CACHED_WAVEFORM_COLUMNS {
            return;
        }
        // Only one transient stretch snapshot is retained. Ordinary entries keep
        // their existing budget; pointer motion cannot retain a history of
        // copied marker/event metadata for every previous preview position.
        if matches!(key.mapping, WaveformMappingKey::Stretch(_)) {
            self.entries.retain(|entry| {
                if matches!(entry.key.mapping, WaveformMappingKey::Stretch(_)) {
                    self.columns = self.columns.saturating_sub(entry.columns);
                    false
                } else {
                    true
                }
            });
        }
        while self.entries.len() >= MAX_CACHED_WAVEFORM_ENTRIES
            || self.columns.saturating_add(columns) > MAX_CACHED_WAVEFORM_COLUMNS
        {
            let Some(removed) = self.entries.pop_front() else {
                break;
            };
            self.columns = self.columns.saturating_sub(removed.columns);
        }
        self.columns = self.columns.saturating_add(columns);
        self.entries.push_back(WaveformGeometryEntry {
            key,
            source,
            geometry,
            columns,
        });
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.columns = 0;
        #[cfg(all(test, feature = "ui-test-support"))]
        self.painted.clear();
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn record_paint(
        &mut self,
        bounds: Bounds<Pixels>,
        mask: Bounds<Pixels>,
        geometry: &WaveformGeometry,
        colors: hex_widgets::WaveformColors,
    ) {
        let _navigation_timer = super::navigation_metrics::Timer::start(
            super::navigation_metrics::Stage::PaintObservation,
        );
        if self.painted.len() == MAX_CACHED_WAVEFORM_ENTRIES {
            self.painted.pop_front();
        }
        self.painted.push_back(WaveformPaintSample {
            colors,
            bounds,
            mask,
            ordinates: std::array::from_fn(|channel| {
                geometry.channels[channel]
                    .outline
                    .iter()
                    .map(|p| f32::from(p.y))
                    .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), y| {
                        (lo.min(y), hi.max(y))
                    })
            }),
            envelope_points: geometry
                .channels
                .iter()
                .map(|channel| channel.outline.len())
                .sum(),
        });
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn painted_waveforms(&self) -> &VecDeque<WaveformPaintSample> {
        &self.painted
    }

    /// Reset diagnostic observations without invalidating retained geometry.
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn clear_paint_samples(&mut self) {
        self.painted.clear();
    }

    /// Actual immutable source, mapping and path-builder ordinates painted by the cache.
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn rendered_waveforms(&self) -> Vec<RenderedWaveformSample> {
        self.entries
            .iter()
            .map(|entry| RenderedWaveformSample {
                source: Arc::clone(&entry.source),
                prepared: match entry.key.mapping {
                    WaveformMappingKey::Prepared {
                        timeline_origin,
                        root_offset,
                        ..
                    } => Some((timeline_origin, root_offset)),
                    _ => None,
                },
                spans: std::array::from_fn(|channel| {
                    let (low, high) = entry.geometry.channels[channel]
                        .outline
                        .iter()
                        .map(|point| f32::from(point.y))
                        .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), y| {
                            (low.min(y), high.max(y))
                        });
                    high - low
                }),
            })
            .collect()
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

#[derive(Clone)]
pub(super) enum WaveformMapping {
    MusicalEvent(super::audio_event_editor::MusicalEventWaveform),
    Stretch(Arc<super::waveform_stretch::StretchProjection>),
    Loop(Arc<super::waveform_loop::LoopProjection>),
    Prepared {
        timeline_origin: u64,
        root_offset: u64,
        gain: hex_project::ClipGain,
    },
    GainedSource {
        timeline_origin: u64,
        gain: hex_project::ClipGain,
        detail: bool,
    },
    Clip {
        clip: Clip,
        timeline_origin: u64,
    },
    Source {
        timeline_origin: u64,
    },
}

/// A sampling lattice shared by all clips in a timeline viewport. Clip edges
/// select columns from it; they never change its density or horizontal origin.
#[derive(Clone, Copy, Debug)]
pub(super) struct WaveformGrid {
    pub(super) start: u64,
    pub(super) end: u64,
    pub(super) left: f32,
    pub(super) width: f32,
    pub(super) columns: usize,
    /// Coarse bins hold their extrema across the whole interval, so masking a
    /// tiny clip cannot hide a peak whose bin center falls outside the clip.
    pub(super) binned: bool,
}

impl WaveformGrid {
    /// Static arranger detail, measured in seconds per physical pixel. Coarse
    /// views aggregate extrema over wider bins; detailed editors keep their
    /// supplied grids. This never reads capture, playback, idle time or frame
    /// timing, so releasing a gesture cannot change waveform quality.
    pub(super) fn arrangement(
        viewport: super::viewport::Viewport,
        left: f32,
        width: f32,
        scale: f32,
        sample_rate: u32,
    ) -> Option<Self> {
        let end = viewport.end()?;
        if viewport.length == 0
            || sample_rate == 0
            || !left.is_finite()
            || !width.is_finite()
            || width <= 0.0
            || !(left + width).is_finite()
            || !scale.is_finite()
            || scale <= 0.0
        {
            return None;
        }
        let seconds_per_pixel =
            viewport.length as f64 / f64::from(sample_rate) / (f64::from(width) * f64::from(scale));
        let spacing = if seconds_per_pixel >= 0.05 {
            8
        } else if seconds_per_pixel >= 0.005 {
            4
        } else if seconds_per_pixel >= 0.001 {
            2
        } else {
            1
        };
        let columns = super::waveform::waveform_column_count(width, scale).div_ceil(spacing);
        // Two envelope points per coarse bin, including three padding bins,
        // must fit the existing shared painter's point budget.
        let columns = if spacing > 1 {
            columns.min(MAX_WAVEFORM_ENVELOPE_POINTS / 2 - 3)
        } else {
            columns
        };
        let mut grid = Self {
            start: viewport.start,
            end,
            left,
            width,
            columns,
            binned: spacing > 1,
        };
        if spacing == 1 {
            return Some(grid);
        }
        // An integer frame step anchors bins to the timeline, rather than the
        // moving viewport. Pad with neighboring bins for edge interpolation.
        // At the u64 boundary, keep the same coarse density and full coverage;
        // a span that cannot be padded uses the valid unpadded grid above.
        let step = viewport.length.div_ceil(columns as u64);
        let padded_columns = columns.checked_add(3)?;
        if let Some(span) = step.checked_mul(padded_columns as u64) {
            let aligned = (viewport.start / step).checked_mul(step)?;
            let start = aligned.saturating_sub(step).min(u64::MAX - span);
            let padded_end = start.checked_add(span)?;
            if start <= viewport.start && padded_end >= end {
                let padded_left = f64::from(left)
                    - (viewport.start - start) as f64 / viewport.length as f64 * f64::from(width);
                let padded_width = span as f64 / viewport.length as f64 * f64::from(width);
                let (padded_left, padded_width) = (padded_left as f32, padded_width as f32);
                if padded_left.is_finite() && padded_width.is_finite() && padded_width > 0.0 {
                    grid = Self {
                        start,
                        end: padded_end,
                        left: padded_left,
                        width: padded_width,
                        columns: padded_columns,
                        binned: true,
                    };
                }
            }
        }
        Some(grid)
    }

    fn key(self) -> [u64; 6] {
        [
            self.start,
            self.end,
            u64::from(self.left.to_bits()),
            u64::from(self.width.to_bits()),
            self.columns as u64,
            self.binned as u64,
        ]
    }
}

pub(super) struct WaveformGeometryRequest<'a> {
    pub(super) mapping: &'a WaveformMapping,
    pub(super) visible_start: u64,
    pub(super) visible_end: u64,
    pub(super) bounds: Bounds<Pixels>,
    pub(super) scale: f32,
    pub(super) columns: usize,
    pub(super) grid: Option<WaveformGrid>,
}

pub(super) fn cached_waveform_geometry(
    cache: &std::cell::RefCell<WaveformGeometryCache>,
    waveform: &Arc<CachedWaveform>,
    request: &WaveformGeometryRequest<'_>,
) -> Option<Arc<WaveformGeometry>> {
    #[cfg(all(test, feature = "ui-test-support"))]
    let _navigation_timer =
        super::navigation_metrics::Timer::start(super::navigation_metrics::Stage::WaveformLookup);
    let mut key = waveform_geometry_key(
        request.mapping,
        request.visible_start,
        request.visible_end,
        request.bounds,
        request.scale,
    );
    key.grid = request.grid.map(WaveformGrid::key);
    if let Some(geometry) = cache
        .try_borrow_mut()
        .ok()
        .and_then(|mut cache| cache.get(key.clone(), waveform))
    {
        return Some(geometry);
    }
    let geometry = Arc::new(prepare_waveform_geometry(
        waveform,
        request.mapping,
        request.visible_start,
        request.visible_end,
        request.bounds,
        request.columns,
        request.grid,
    )?);
    if let Ok(mut cache) = cache.try_borrow_mut() {
        cache.insert(
            key,
            Arc::clone(waveform),
            Arc::clone(&geometry),
            geometry.columns,
        );
    }
    Some(geometry)
}

fn waveform_geometry_key(
    mapping: &WaveformMapping,
    visible_start: u64,
    visible_end: u64,
    bounds: Bounds<Pixels>,
    scale: f32,
) -> WaveformGeometryKey {
    let mapping = match mapping {
        WaveformMapping::MusicalEvent(projection) => {
            WaveformMappingKey::MusicalEvent(projection.clone())
        }
        WaveformMapping::Stretch(projection) => WaveformMappingKey::Stretch(Arc::clone(projection)),
        WaveformMapping::Loop(projection) => WaveformMappingKey::Loop(Arc::clone(projection)),
        WaveformMapping::Prepared {
            timeline_origin,
            root_offset,
            gain,
        } => WaveformMappingKey::Prepared {
            timeline_origin: *timeline_origin,
            root_offset: *root_offset,
            gain: gain.millidb(),
        },
        WaveformMapping::GainedSource {
            timeline_origin,
            gain,
            detail,
        } => WaveformMappingKey::GainedSource {
            timeline_origin: *timeline_origin,
            gain: gain.millidb(),
            detail: *detail,
        },
        WaveformMapping::Clip {
            clip,
            timeline_origin,
        } => WaveformMappingKey::Clip {
            id: clip.id().value(),
            gain: clip.gain().millidb(),
            start: clip.start(),
            duration: clip.duration(),
            source_offset: clip.source_offset(),
            source_length: clip.source_length(),
            source_phase: clip.source_phase(),
            looping: clip.looping(),
            timeline_origin: *timeline_origin,
        },
        WaveformMapping::Source { timeline_origin } => WaveformMappingKey::Source {
            timeline_origin: *timeline_origin,
        },
    };
    WaveformGeometryKey {
        mapping,
        visible_start,
        visible_end,
        origin_x: f32::from(bounds.origin.x).to_bits(),
        origin_y: f32::from(bounds.origin.y).to_bits(),
        width: f32::from(bounds.size.width).to_bits(),
        height: f32::from(bounds.size.height).to_bits(),
        scale: scale.to_bits(),
        grid: None,
    }
}

pub(super) fn prepare_waveform_geometry(
    waveform: &CachedWaveform,
    mapping: &WaveformMapping,
    visible_start: u64,
    visible_end: u64,
    bounds: Bounds<Pixels>,
    columns: usize,
    grid: Option<WaveformGrid>,
) -> Option<WaveformGeometry> {
    #[cfg(all(test, feature = "ui-test-support"))]
    let _navigation_timer =
        super::navigation_metrics::Timer::start(super::navigation_metrics::Stage::WaveformBuild);
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let grid = grid.unwrap_or(WaveformGrid {
        start: visible_start,
        end: visible_end,
        left: f32::from(bounds.origin.x),
        width,
        columns,
        binned: false,
    });
    let frames = grid.end.checked_sub(grid.start)?;
    if frames == 0
        || grid.columns == 0
        || grid.columns > MAX_WAVEFORM_COLUMNS
        || !grid.left.is_finite()
        || !grid.width.is_finite()
        || grid.width <= 0.0
        || !height.is_finite()
        || height <= 0.0
    {
        return None;
    }
    // Include a neighbor on either side so cropping does not reshape the
    // silhouette segment at the visible edge. The clip's overflow mask crops it.
    let first = ((u128::from(visible_start.saturating_sub(grid.start)) * grid.columns as u128)
        / u128::from(frames))
    .min(grid.columns as u128) as usize;
    let last = (u128::from(visible_end.saturating_sub(grid.start)) * grid.columns as u128)
        .div_ceil(u128::from(frames))
        .min(grid.columns as u128) as usize;
    let first = first.saturating_sub(1).min(grid.columns);
    let last = last.saturating_add(1).min(grid.columns);
    let columns = last.saturating_sub(first);
    let point_count = columns.checked_mul(if grid.binned { 2 } else { 1 })?;
    if point_count > MAX_WAVEFORM_ENVELOPE_POINTS {
        return None;
    }
    let gain = match mapping {
        WaveformMapping::MusicalEvent(projection) => projection.gain().linear(),
        WaveformMapping::Stretch(projection) => projection.gain().linear(),
        WaveformMapping::Loop(projection) => projection.gain().linear(),
        WaveformMapping::Clip { clip, .. } => clip.gain().linear(),
        WaveformMapping::GainedSource { gain, .. } | WaveformMapping::Prepared { gain, .. } => {
            gain.linear()
        }
        WaveformMapping::Source { .. } => 1.0,
    };
    #[cfg(all(test, feature = "ui-test-support"))]
    let sampling_timer =
        super::navigation_metrics::Timer::start(super::navigation_metrics::Stage::WaveformSampling);
    let mut stereo_peaks = Vec::with_capacity(columns);
    for column in first..last {
        let start = grid
            .start
            .saturating_add(fraction_u64(frames, column, grid.columns));
        let end = grid
            .start
            .saturating_add(fraction_u64(frames, column + 1, grid.columns))
            .max(start.saturating_add(1));
        let peaks = match mapping {
            WaveformMapping::MusicalEvent(projection) => {
                let ranges = projection.ranges(start..end, waveform.source_frames());
                aggregate_ranges(waveform, ranges.as_slice())
            }
            WaveformMapping::Loop(projection) => projection.peaks(waveform, start..end),
            WaveformMapping::Stretch(projection) => {
                let ranges = projection.ranges(start..end, waveform.source_frames());
                aggregate_ranges(waveform, ranges.as_slice())
            }
            WaveformMapping::Prepared {
                timeline_origin,
                root_offset,
                ..
            } => {
                // A viewport bin may begin before the cropped clip origin while
                // still addressing the retained full root. Combine the signed
                // translation before clamping; subtracting origin first erases
                // valid edge bins and changes their shape during a left trim.
                let translation = i128::from(*root_offset) - i128::from(*timeline_origin);
                let available = waveform.source_frames() as i128;
                let start = (i128::from(start) + translation).clamp(0, available) as usize;
                let end = (i128::from(end) + translation).clamp(0, available) as usize;
                if waveform.is_repeated() {
                    let budget = (4_194_304 / grid.columns).min(4_096);
                    waveform
                        .bounded_exact_peaks_for_frames(start, end, budget)
                        .or_else(|| aggregate_peaks(waveform, start, end))
                } else {
                    aggregate_peaks(waveform, start, end)
                }
            }
            WaveformMapping::Clip {
                clip,
                timeline_origin,
            } => {
                let ranges = timeline_source_ranges_at(
                    clip,
                    *timeline_origin,
                    start..end,
                    waveform.source_frames(),
                );
                aggregate_ranges(waveform, ranges.as_slice())
            }
            WaveformMapping::Source { timeline_origin }
            | WaveformMapping::GainedSource {
                timeline_origin, ..
            } => start
                .checked_sub(*timeline_origin)
                .zip(end.checked_sub(*timeline_origin))
                .and_then(|(start, end)| usize::try_from(start).ok().zip(usize::try_from(end).ok()))
                .and_then(|(start, end)| {
                    // Limit total PCM reads per event geometry, even for a large
                    // viewport. Peak-only sources and wider views use the
                    // existing bounded summary path.
                    const DETAIL_FRAME_BUDGET: usize = 4_194_304;
                    const DETAIL_COLUMN_BUDGET: usize = 4_096;
                    let detail_budget =
                        (DETAIL_FRAME_BUDGET / grid.columns).min(DETAIL_COLUMN_BUDGET);
                    if matches!(mapping, WaveformMapping::GainedSource { detail: true, .. }) {
                        waveform
                            .bounded_exact_peaks_for_frames(start, end, detail_budget)
                            .or_else(|| aggregate_peaks(waveform, start, end))
                    } else {
                        aggregate_peaks(waveform, start, end)
                    }
                }),
        };
        let mut peaks = peaks.unwrap_or([CachedPeak { min: 0.0, max: 0.0 }; 2]);
        if gain != 1.0 {
            for peak in &mut peaks {
                peak.min *= gain;
                peak.max *= gain;
            }
        }
        stereo_peaks.push(peaks);
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    drop(sampling_timer);
    #[cfg(all(test, feature = "ui-test-support"))]
    let _paths_timer =
        super::navigation_metrics::Timer::start(super::navigation_metrics::Stage::WaveformPaths);
    let mut channels = Vec::with_capacity(2);
    #[cfg(test)]
    let mut outlines = std::array::from_fn(|_| WaveformChannelGeometry {
        outline: Vec::new(),
    });
    for channel in 0..2 {
        let band_top = height * channel as f32 * 0.5;
        let band_height = height * 0.5;
        let center = band_top + band_height * 0.5;
        let amplitude = (band_height * 0.44).max(0.5);
        let mut maxima = Vec::with_capacity(point_count);
        let mut minima = Vec::with_capacity(point_count);
        for (column, peaks) in stereo_peaks.iter().enumerate() {
            let (top, bottom) = peak_vertical_bounds(peaks[channel], center, amplitude);
            if grid.binned {
                for edge in [first + column, first + column + 1] {
                    let x = grid.left + edge as f32 * grid.width / grid.columns as f32;
                    maxima.push(point(px(x), bounds.origin.y + px(top)));
                    minima.push(point(px(x), bounds.origin.y + px(bottom)));
                }
            } else {
                let x =
                    grid.left + ((first + column) as f32 + 0.5) * grid.width / grid.columns as f32;
                maxima.push(point(px(x), bounds.origin.y + px(top)));
                minima.push(point(px(x), bounds.origin.y + px(bottom)));
            }
        }

        let has_area = maxima
            .iter()
            .zip(&minima)
            .any(|(maximum, minimum)| (f32::from(maximum.y - minimum.y)).abs() >= 0.5);
        channels.push(WaveformChannel::new(
            WaveformEnvelope {
                upper: &maxima,
                lower: &minima,
                mode: if has_area {
                    WaveformEnvelopeMode::Filled
                } else {
                    WaveformEnvelopeMode::Trace
                },
                trace_width: px(1.0),
                // Coarse overview bins already show the signed envelope (or
                // the DC/silence trace). Avoid another path per channel just
                // for its zero-line guide; fine/detail views keep that guide.
                centerline: (!grid.binned).then_some(WaveformStroke {
                    points: [
                        point(bounds.origin.x, bounds.origin.y + px(center)),
                        point(
                            bounds.origin.x + bounds.size.width,
                            bounds.origin.y + px(center),
                        ),
                    ],
                    width: px(0.5),
                }),
            },
            std::iter::empty(),
        )?);
        #[cfg(test)]
        {
            outlines[channel] = WaveformChannelGeometry {
                outline: maxima.into_iter().chain(minima).collect(),
            };
        }
    }
    Some(WaveformGeometry {
        display: hex_widgets::WaveformGeometry::stereo(channels.try_into().ok()?),
        #[cfg(test)]
        channels: outlines,
        columns: point_count,
    })
}

#[cfg(test)]
#[path = "waveform_geometry/lane_tests.rs"]
mod lane_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, size};
    use std::cell::RefCell;

    use crate::editor::waveform::waveform_column_count;
    use std::time::Instant;

    fn waveform() -> Arc<CachedWaveform> {
        Arc::new(
            CachedWaveform::from_peaks(
                96_000,
                (0..4096)
                    .map(|index| {
                        let phase = (index % 128) as f32 / 127.0;
                        [
                            CachedPeak {
                                min: -phase,
                                max: phase * 0.8,
                            },
                            CachedPeak {
                                min: -phase * 0.7,
                                max: phase,
                            },
                        ]
                    })
                    .collect(),
            )
            .expect("valid waveform fixture"),
        )
    }

    fn bounds() -> Bounds<Pixels> {
        Bounds::new(point(px(12.0), px(34.0)), size(px(1200.0), px(180.0)))
    }

    fn key(origin: u64, bounds: Bounds<Pixels>) -> WaveformGeometryKey {
        waveform_geometry_key(
            &WaveformMapping::Source {
                timeline_origin: origin,
            },
            origin,
            origin + 96_000,
            bounds,
            2.0,
        )
    }

    fn geometry(
        source: &CachedWaveform,
        origin: u64,
        bounds: Bounds<Pixels>,
    ) -> Arc<WaveformGeometry> {
        Arc::new(
            prepare_waveform_geometry(
                source,
                &WaveformMapping::Source {
                    timeline_origin: origin,
                },
                origin,
                origin + 96_000,
                bounds,
                32,
                None,
            )
            .expect("waveform geometry"),
        )
    }

    #[test]
    #[ignore = "manual release profile of repeated waveform geometry"]
    fn profile_repeated_prepared_waveform() {
        use std::time::Instant;
        let mut samples = vec![[0.0, 0.0]; 16_000];
        samples[2_010] = [1.0, 1.0];
        let audio = hex_engine::Audio::new(48_000, samples).unwrap();
        let waveform = hex_media::waveform::summarize_repeated(
            &audio,
            hex_media::waveform::Repeat {
                offset: 0,
                period: 16_000,
                phase: 0,
            },
            1_600_000,
            1_024,
        )
        .unwrap();
        let mapping = WaveformMapping::Prepared {
            timeline_origin: 0,
            root_offset: 0,
            gain: hex_project::ClipGain::UNITY,
        };
        let extent = Bounds::new(point(px(0.0), px(0.0)), size(px(1000.0), px(100.0)));
        let mut times = Vec::with_capacity(100);
        for pass in 0..110 {
            let start = Instant::now();
            std::hint::black_box(
                prepare_waveform_geometry(&waveform, &mapping, 0, 80_000, extent, 1000, None)
                    .unwrap(),
            );
            let elapsed = start.elapsed().as_nanos();
            if pass >= 10 {
                times.push(elapsed);
            }
        }
        times.sort_unstable();
        eprintln!(
            "repeated waveform 1000 columns/5 cycles: p50={}ns p95={}ns",
            times[50], times[95]
        );
    }

    #[test]
    fn repeated_prepared_waveform_keeps_transients_in_their_visible_columns() {
        let mut samples = vec![[0.0, 0.0]; 16_000];
        samples[2_010] = [1.0, 1.0];
        let audio = hex_engine::Audio::new(48_000, samples).unwrap();
        let repeated = hex_media::waveform::summarize_repeated(
            &audio,
            hex_media::waveform::Repeat {
                offset: 0,
                period: 16_000,
                phase: 0,
            },
            1_600_000,
            1_024,
        )
        .unwrap();
        let extent = Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(100.0)));
        for (start, frames_per_column) in [
            (0_u64, 65_u64),
            (0, 160),
            (16_000 * 50, 160),
            (16_000 * 50, 1_000),
        ] {
            let geometry = prepare_waveform_geometry(
                &repeated,
                &WaveformMapping::Prepared {
                    timeline_origin: 0,
                    root_offset: 0,
                    gain: hex_project::ClipGain::UNITY,
                },
                start,
                start + 100 * frames_per_column,
                extent,
                100,
                None,
            )
            .unwrap();
            let peaks: Vec<_> = geometry.channels[0].outline[..100]
                .iter()
                .enumerate()
                .filter(|(_, point)| point.y < px(25.0))
                .map(|(i, _)| i)
                .collect();
            let expected: Vec<_> = (0..100)
                .filter(|column| {
                    let a = start + *column * frames_per_column;
                    let b = a + frames_per_column;
                    (a..b).any(|frame| frame % 16_000 == 2_010)
                })
                .map(|column| column as usize)
                .collect();
            assert_eq!(
                peaks, expected,
                "start {start}, frames per column {frames_per_column}"
            );
        }
    }

    #[test]
    fn event_detail_keeps_a_transient_in_its_visible_column() {
        // A long prepared root can have 1,000 source frames per cached bin.
        // At 65 frames per visible column, one impulse must not fill the
        // surrounding columns merely because they share a cached bin.
        let mut samples = vec![[0.0, 0.0]; 1_024_000];
        samples[2_010] = [1.0, 1.0];
        let audio = hex_engine::Audio::new(48_000, samples).unwrap();
        let waveform = hex_media::waveform::summarize(&audio, 1_024).unwrap();
        let extent = Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(100.0)));
        for (start, frames_per_column) in [
            (0_u64, 65_u64),
            (1_300, 65),
            (1_950, 65),
            (0, 1_000),
            (0, 1_025),
        ] {
            let geometry = prepare_waveform_geometry(
                &waveform,
                &WaveformMapping::GainedSource {
                    timeline_origin: 0,
                    gain: hex_project::ClipGain::UNITY,
                    detail: true,
                },
                start,
                start + 100 * frames_per_column,
                extent,
                100,
                None,
            )
            .unwrap();
            let top = &geometry.channels[0].outline[..100];
            let peaks: Vec<_> = top
                .iter()
                .enumerate()
                .filter(|(_, p)| p.y < px(25.0))
                .map(|(i, _)| i)
                .collect();
            assert_eq!(peaks, vec![((2_010 - start) / frames_per_column) as usize]);
        }

        // A longer root has wider cached bins. A narrow visible event may use
        // its share of the same total work budget rather than inheriting the
        // per-column limit of a full-width event.
        let mut long_samples = vec![[0.0, 0.0]; 3_072_000];
        long_samples[6_010] = [1.0, 1.0];
        let long_audio = hex_engine::Audio::new(48_000, long_samples).unwrap();
        let long_waveform = hex_media::waveform::summarize(&long_audio, 1_024).unwrap();
        let geometry = prepare_waveform_geometry(
            &long_waveform,
            &WaveformMapping::GainedSource {
                timeline_origin: 0,
                gain: hex_project::ClipGain::UNITY,
                detail: true,
            },
            0,
            205_000,
            extent,
            100,
            None,
        )
        .unwrap();
        let peaks: Vec<_> = geometry.channels[0].outline[..100]
            .iter()
            .enumerate()
            .filter(|(_, p)| p.y < px(25.0))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(peaks, vec![6_010 / 2_050]);
    }

    #[test]
    fn arranger_detail_preserves_close_zoom_and_anchors_coarse_bins() {
        use crate::editor::viewport::Viewport;
        for sample_rate in [8_000_u32, 44_100, 48_000, 192_000] {
            let close = Viewport {
                start: 123,
                length: u64::from(sample_rate) / 4,
            };
            let grid = WaveformGrid::arrangement(close, 12.0, 1000.0, 2.0, sample_rate).unwrap();
            assert_eq!((grid.start, grid.end), (close.start, close.end().unwrap()));
            assert_eq!((grid.left, grid.width, grid.columns), (12.0, 1000.0, 2000));
            let length = u64::from(sample_rate) * 120;
            let mut step = None;
            for start in [0, 17_123, length / 2] {
                let view = Viewport { start, length };
                let grid = WaveformGrid::arrangement(view, 12.0, 1000.0, 2.0, sample_rate).unwrap();
                assert!(grid.start <= start && grid.end >= view.end().unwrap());
                // The dense overview needs at most one column per eight
                // physical pixels, while retaining neighbor bins at the edges.
                assert!(grid.columns as f32 / (grid.width * 2.0) <= 0.126);
                let current_step = (grid.end - grid.start) / grid.columns as u64;
                assert_eq!(grid.start % current_step, 0);
                assert_eq!(grid.end - grid.start, current_step * grid.columns as u64);
                if let Some(previous) = step {
                    assert_eq!(previous, current_step, "pan changed the sampling density");
                }
                step = Some(current_step);
            }
        }
    }

    #[test]
    fn arranger_grid_refuses_invalid_numbers_and_covers_u64_edges() {
        use crate::editor::viewport::Viewport;
        let view = Viewport {
            start: 0,
            length: 1_000_000,
        };
        for (left, width, scale, rate) in [
            (0.0, 0.0, 2.0, 44_100),
            (0.0, f32::NAN, 2.0, 44_100),
            (f32::INFINITY, 1000.0, 2.0, 44_100),
            (f32::MAX, f32::MAX, 2.0, 44_100),
            (0.0, 1000.0, -1.0, 44_100),
            (0.0, 1000.0, 2.0, 0),
        ] {
            assert!(WaveformGrid::arrangement(view, left, width, scale, rate).is_none());
        }
        for view in [
            Viewport {
                start: u64::MAX - 1_000_000,
                length: 1_000_000,
            },
            Viewport {
                start: 0,
                length: u64::MAX,
            },
        ] {
            let grid = WaveformGrid::arrangement(view, 12.0, 1000.0, 2.0, 44_100).unwrap();
            assert!(grid.start <= view.start && grid.end >= view.end().unwrap());
            assert!((1..=MAX_WAVEFORM_COLUMNS).contains(&grid.columns));
            assert!(grid.left.is_finite() && grid.width.is_finite() && grid.width > 0.0);
        }
        assert!(
            WaveformGrid::arrangement(
                Viewport {
                    start: u64::MAX,
                    length: 1
                },
                0.0,
                1000.0,
                2.0,
                44_100
            )
            .is_none()
        );
    }

    #[test]
    fn coarse_arranger_bins_keep_narrow_stereo_peaks_silence_and_dc() {
        use crate::editor::viewport::Viewport;
        let frames = 4_410_000_u64;
        let view = Viewport {
            start: 0,
            length: frames,
        };
        let grid = WaveformGrid::arrangement(view, 12.0, 200.0, 2.0, 44_100).unwrap();
        let step = (grid.end - grid.start) / grid.columns as u64;
        let extent = Bounds::new(point(px(12.0), px(34.0)), size(px(200.0), px(80.0)));
        for kind in ["needle", "silence", "dc"] {
            let mut peaks = vec![[CachedPeak { min: 0.0, max: 0.0 }; 2]; 4096];
            if kind == "needle" {
                peaks[2000][0] = CachedPeak {
                    min: -0.7,
                    max: 0.8,
                };
                peaks[15][1] = CachedPeak {
                    min: -1.0,
                    max: 0.35,
                };
            } else if kind == "dc" {
                peaks.fill([
                    CachedPeak {
                        min: 0.25,
                        max: 0.25,
                    },
                    CachedPeak {
                        min: -0.375,
                        max: -0.375,
                    },
                ]);
            }
            let source = CachedWaveform::from_peaks(frames as usize, peaks.clone()).unwrap();
            let geometry = prepare_waveform_geometry(
                &source,
                &WaveformMapping::Source { timeline_origin: 0 },
                0,
                frames,
                extent,
                400,
                Some(grid),
            )
            .unwrap();
            assert!(
                geometry.columns < 200,
                "overview still builds pixel-dense geometry"
            );
            for column in 0..geometry.columns / 2 {
                let start = grid.start + column as u64 * step;
                let end = start.saturating_add(step).min(frames);
                // Independent linear scan of the documented disjoint source
                // bins (the first remainder bins are one frame wider). No
                // production range aggregation establishes the answer.
                let mut expected: Option<[CachedPeak; 2]> = None;
                if start < end {
                    let width = frames / peaks.len() as u64;
                    let extra = frames % peaks.len() as u64;
                    let mut bin_start = 0;
                    for (index, peak) in peaks.iter().enumerate() {
                        let bin_end =
                            bin_start + width + if (index as u64) < extra { 1 } else { 0 };
                        if start < bin_end && bin_start < end {
                            if let Some(old) = &mut expected {
                                for channel in 0..2 {
                                    old[channel].min = old[channel].min.min(peak[channel].min);
                                    old[channel].max = old[channel].max.max(peak[channel].max);
                                }
                            } else {
                                expected = Some(*peak);
                            }
                        }
                        bin_start = bin_end;
                    }
                }
                let expected = expected.unwrap_or([CachedPeak { min: 0.0, max: 0.0 }; 2]);
                for (channel, peak) in expected.iter().enumerate() {
                    let outline = &geometry.channels[channel].outline;
                    let center = 34.0 + channel as f32 * 40.0 + 20.0;
                    for edge in [column * 2, column * 2 + 1] {
                        assert!(
                            (f32::from(outline[edge].y) - (center - peak.max * 17.6)).abs()
                                < 0.0001,
                            "{kind} upper peak lost"
                        );
                        assert!(
                            (f32::from(outline[edge + geometry.columns].y)
                                - (center - peak.min * 17.6))
                                .abs()
                                < 0.0001,
                            "{kind} lower peak lost"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn coarse_peak_remains_visible_inside_a_clip_narrower_than_a_bin() {
        use crate::editor::viewport::Viewport;
        let view = Viewport {
            start: 0,
            length: 4_410_000,
        };
        let grid = WaveformGrid::arrangement(view, 12.0, 1000.0, 2.0, 44_100).unwrap();
        let origin = 1_000_000;
        let duration = 8_000;
        let mut peaks = vec![[CachedPeak { min: 0.0, max: 0.0 }; 2]; duration];
        peaks[duration / 2] = [CachedPeak {
            min: -1.0,
            max: 1.0,
        }; 2];
        let source = CachedWaveform::from_peaks(duration, peaks).unwrap();
        let left = 12.0 + origin as f32 / view.length as f32 * 1000.0;
        let width = duration as f32 / view.length as f32 * 1000.0;
        let extent = Bounds::new(point(px(left), px(34.0)), size(px(width), px(80.0)));
        let geometry = prepare_waveform_geometry(
            &source,
            &WaveformMapping::Prepared {
                timeline_origin: origin,
                root_offset: 0,
                gain: hex_project::ClipGain::UNITY,
            },
            origin,
            origin + duration as u64,
            extent,
            waveform_column_count(width, 2.0),
            Some(grid),
        )
        .unwrap();
        let middle = left + width * 0.5;
        let outline = &geometry.channels[0].outline;
        let visible_peak = (0..geometry.columns.saturating_sub(1)).any(|i| {
            let a = outline[i];
            let b = outline[i + 1];
            f32::from(a.x) <= middle
                && middle <= f32::from(b.x)
                && f32::from(b.x) > f32::from(a.x)
                && (f32::from(a.y) - 36.4).abs() < 0.0001
                && (f32::from(b.y) - 36.4).abs() < 0.0001
                && (f32::from(outline[i + geometry.columns].y) - 71.6).abs() < 0.0001
                && (f32::from(outline[i + geometry.columns + 1].y) - 71.6).abs() < 0.0001
        });
        assert!(
            visible_peak,
            "clip masking hid a narrow transient between bin centers"
        );
    }

    #[test]
    fn coarse_waveforms_translate_without_resampling_during_pan() {
        use crate::editor::viewport::Viewport;
        let source = waveform();
        let length = 24_000;
        let first_start = 20_000;
        let extent = Bounds::new(point(px(12.0), px(34.0)), size(px(100.25), px(80.0)));
        let grid = WaveformGrid::arrangement(
            Viewport {
                start: first_start,
                length,
            },
            12.0,
            100.25,
            2.0,
            44_100,
        )
        .unwrap();
        let step = (grid.end - grid.start) / grid.columns as u64;
        let samples = |start| {
            let grid =
                WaveformGrid::arrangement(Viewport { start, length }, 12.0, 100.25, 2.0, 44_100)
                    .unwrap();
            let geometry = prepare_waveform_geometry(
                &source,
                &WaveformMapping::Source { timeline_origin: 0 },
                start,
                start + length,
                extent,
                201,
                Some(grid),
            )
            .unwrap();
            let first = ((u128::from(start - grid.start) * grid.columns as u128)
                / u128::from(grid.end - grid.start)) as usize;
            let first = first.saturating_sub(1);
            std::array::from_fn::<_, 2, _>(|channel| {
                let outline = &geometry.channels[channel].outline;
                (0..geometry.columns / 2)
                    .map(|i| {
                        (
                            grid.start + (first + i) as u64 * step,
                            (
                                f32::from(outline[i * 2].x),
                                f32::from(outline[i * 2].y).to_bits(),
                                f32::from(outline[i * 2 + geometry.columns].y).to_bits(),
                            ),
                        )
                    })
                    .collect::<std::collections::BTreeMap<_, _>>()
            })
        };
        let before = samples(first_start);
        for start in [first_start + step / 3, first_start + step * 2 + 7] {
            let after = samples(start);
            let displacement = (start - first_start) as f32 / length as f32 * 100.25;
            for channel in 0..2 {
                let mut compared = 0;
                for (bin, &(x, upper, lower)) in &before[channel] {
                    if let Some(&(new_x, new_upper, new_lower)) = after[channel].get(bin) {
                        assert_eq!(
                            (upper, lower),
                            (new_upper, new_lower),
                            "pan resampled a fixed source bin"
                        );
                        assert!(
                            (new_x - x + displacement).abs() < 0.002,
                            "waveform did not translate with the viewport"
                        );
                        compared += 1;
                    }
                }
                assert!(compared > 20);
            }
        }
    }

    #[test]
    fn resize_preserves_interior_stereo_outline_at_fractional_edges_and_column_cap() {
        let source = waveform();
        for (width, scale) in [(783.25, 1.0), (783.25, 2.0), (5000.25, 2.0)] {
            let grid = WaveformGrid {
                start: 0,
                end: 96_000,
                left: 125.25,
                width,
                columns: waveform_column_count(width, scale),
                binned: false,
            };
            for prepared in [false, true] {
                let draw = |start: u64, end: u64| {
                    let mapping = if prepared {
                        // A retained prepared root's offset follows a left crop.
                        WaveformMapping::Prepared {
                            timeline_origin: start,
                            root_offset: start,
                            gain: hex_project::ClipGain::UNITY,
                        }
                    } else {
                        WaveformMapping::Source { timeline_origin: 0 }
                    };
                    let extent = Bounds::new(
                        point(px(grid.left + start as f32 / 96_000.0 * width), px(34.0)),
                        size(px((end - start) as f32 / 96_000.0 * width), px(44.0)),
                    );
                    prepare_waveform_geometry(
                        &source,
                        &mapping,
                        start,
                        end,
                        extent,
                        waveform_column_count(f32::from(extent.size.width), scale),
                        Some(grid),
                    )
                    .unwrap()
                };
                let baseline = draw(4_900, 91_100);
                // Both trim directions, subpixel motion, crop and newly exposed
                // audio. Compare the actual outline points fed to PathBuilder.
                for (start, end) in [
                    (5_001, 91_100),
                    (4_900, 90_999),
                    (6_103, 89_087),
                    (4_003, 92_017),
                ] {
                    let resized = draw(start, end);
                    for channel in 0..2 {
                        let interior = |geometry: &WaveformGeometry| {
                            geometry.channels[channel]
                                .outline
                                .iter()
                                .filter(|p| {
                                    let x = f32::from(p.x);
                                    x > grid.left + width * 0.2 && x < grid.left + width * 0.8
                                })
                                .map(|p| (f32::from(p.x).to_bits(), f32::from(p.y).to_bits()))
                                .collect::<Vec<_>>()
                        };
                        let expected = interior(&baseline);
                        assert!(expected.len() > 100);
                        assert!(
                            interior(&resized) == expected,
                            "unchanged outline must not shift: width={width} scale={scale} prepared={prepared} crop={start}..{end}"
                        );
                    }
                    assert!(resized.columns <= 4096);
                }
                let revealed = draw(4_003, 92_017);
                assert!(
                    revealed.channels[0]
                        .outline
                        .iter()
                        .any(|p| f32::from(p.x) < grid.left + width * 4_900.0 / 96_000.0)
                );
                assert!(
                    revealed.channels[0]
                        .outline
                        .iter()
                        .any(|p| f32::from(p.x) > grid.left + width * 91_100.0 / 96_000.0)
                );
            }
        }
    }

    #[test]
    fn prepared_crop_sampling_is_invariant_across_grid_bins_before_clip_origin() {
        let source = waveform();
        let grid = WaveformGrid {
            start: 0,
            end: 96_000,
            left: 125.25,
            width: 783.25,
            columns: 784,
            binned: false,
        };
        let extent = Bounds::new(
            point(px(125.25 + 5_001.0 / 96_000.0 * 783.25), px(34.0)),
            size(px(700.0), px(44.0)),
        );
        let draw = |origin, offset| {
            prepare_waveform_geometry(
                &source,
                &WaveformMapping::Prepared {
                    timeline_origin: origin,
                    root_offset: offset,
                    gain: hex_project::ClipGain::UNITY,
                },
                5_001,
                91_100,
                extent,
                700,
                Some(grid),
            )
            .unwrap()
        };
        let full_root = draw(0, 0);
        let retained_crop = draw(5_001, 5_001);
        for channel in 0..2 {
            assert!(
                retained_crop.channels[channel].outline == full_root.channels[channel].outline,
                "same accepted root/time positions retain edge-bin peaks after a crop"
            );
        }
    }

    #[cfg(feature = "ui-test-support")]
    #[test]
    fn canonical_trim_and_lower_fade_resize_keep_loop_and_one_shot_outline_still() {
        use hex_project::{Command, Created, Editor, TrimEdge};
        let source = waveform();
        let (project, track, _, imported) = super::super::clip_drag_ui_test::audio_project();
        let asset = project.clip(imported).unwrap().asset();
        let mut editor = Editor::new(project, 8).unwrap();
        for looping in [false, true] {
            let Created::Clip(id) = editor
                .execute(Command::PlaceClip {
                    track,
                    asset,
                    start: 4_000,
                    duration: 84_000,
                    source_offset: 100,
                    source_length: if looping { 1701 } else { 84_000 },
                    looping,
                })
                .unwrap()
            else {
                panic!("placed clip")
            };
            let clip = editor.project().clip(id).unwrap();
            let grid = WaveformGrid {
                start: 0,
                end: 96_000,
                left: 125.25,
                width: 783.25,
                columns: 1567,
                binned: false,
            };
            let draw = |clip: &Clip| {
                let start = clip.start();
                let end = start + clip.duration();
                let extent = Bounds::new(
                    point(
                        px(grid.left + start as f32 / 96_000.0 * grid.width),
                        px(34.0),
                    ),
                    size(px(clip.duration() as f32 / 96_000.0 * grid.width), px(44.0)),
                );
                prepare_waveform_geometry(
                    &source,
                    &WaveformMapping::Clip {
                        clip: clip.clone(),
                        timeline_origin: start,
                    },
                    start,
                    end,
                    extent,
                    32,
                    Some(grid),
                )
                .unwrap()
            };
            let baseline = draw(clip);
            for lower_fade in [false, true] {
                for (edge, frame) in [(TrimEdge::Left, 5_001), (TrimEdge::Right, 86_117)] {
                    let cropped = if lower_fade {
                        clip.preview_fade_edge_trim(
                            edge,
                            frame,
                            96_000,
                            editor.project().tempo_map(),
                        )
                        .unwrap()
                    } else {
                        clip.preview_edge_trim(edge, frame, 96_000).unwrap()
                    };
                    let after = draw(&cropped);
                    for channel in 0..2 {
                        let interior = |g: &WaveformGeometry| {
                            g.channels[channel]
                                .outline
                                .iter()
                                .filter(|p| f32::from(p.x) > 300.0 && f32::from(p.x) < 700.0)
                                .copied()
                                .collect::<Vec<_>>()
                        };
                        assert_eq!(interior(&after), interior(&baseline));
                    }
                }
            }
        }
    }

    #[test]
    fn geometry_cache_reuses_only_matching_source_mapping_and_bounds() {
        let source = waveform();
        let extent = bounds();
        let cache_key = key(0, extent);
        let geometry = geometry(&source, 0, extent);
        let mut cache = WaveformGeometryCache::default();
        cache.insert(
            cache_key.clone(),
            Arc::clone(&source),
            Arc::clone(&geometry),
            32,
        );
        assert!(Arc::ptr_eq(
            &cache.get(cache_key.clone(), &source).expect("cache hit"),
            &geometry,
        ));
        assert!(
            Arc::strong_count(&source) >= 2,
            "cache retains source identity"
        );

        let resized = Bounds::new(extent.origin, size(px(1201.0), extent.size.height));
        assert!(cache.get(key(0, resized), &source).is_none());
        let scrolled = Bounds::new(
            point(extent.origin.x, extent.origin.y + px(44.0)),
            extent.size,
        );
        assert!(cache.get(key(0, scrolled), &source).is_none());
        let scale_changed = waveform_geometry_key(
            &WaveformMapping::Source { timeline_origin: 0 },
            0,
            96_000,
            extent,
            1.0,
        );
        assert!(cache.get(scale_changed, &source).is_none());
        assert!(cache.get(key(1, extent), &source).is_none());
        let different_source = waveform();
        assert!(cache.get(cache_key, &different_source).is_none());
    }

    #[test]
    fn clip_detail_gain_changes_cached_geometry_and_unity_reuses_it() {
        let source = waveform();
        let cache = RefCell::new(WaveformGeometryCache::default());
        let neutral = WaveformMapping::GainedSource {
            timeline_origin: 0,
            gain: hex_project::ClipGain::UNITY,
            detail: false,
        };
        let quiet = WaveformMapping::GainedSource {
            timeline_origin: 0,
            gain: hex_project::ClipGain::new(-6_000).unwrap(),
            detail: false,
        };
        let detailed = WaveformMapping::GainedSource {
            timeline_origin: 0,
            gain: hex_project::ClipGain::UNITY,
            detail: true,
        };
        let geometry = |mapping| {
            cached_waveform_geometry(
                &cache,
                &source,
                &WaveformGeometryRequest {
                    mapping,
                    visible_start: 0,
                    visible_end: 96_000,
                    bounds: bounds(),
                    scale: 2.0,
                    grid: None,
                    columns: 32,
                },
            )
            .unwrap()
        };
        let first = geometry(&neutral);
        let changed = geometry(&quiet);
        assert!(!Arc::ptr_eq(&first, &changed));
        assert!(Arc::ptr_eq(&first, &geometry(&neutral)));
        assert!(!Arc::ptr_eq(&first, &geometry(&detailed)));
        assert_eq!(cache.borrow().entries.len(), 3);
    }

    #[test]
    fn geometry_cache_holds_twelve_visible_clips_and_clip_detail() {
        let source = waveform();
        let extent = bounds();
        let mut cache = WaveformGeometryCache::default();
        for index in 0..13_u64 {
            let origin = index * 96_000;
            let cache_key = key(origin, extent);
            cache.insert(
                cache_key,
                Arc::clone(&source),
                geometry(&source, origin, extent),
                4_000,
            );
        }
        assert_eq!(cache.entries.len(), 13);
        assert_eq!(cache.columns, 52_000);
        for index in 0..13_u64 {
            assert!(cache.get(key(index * 96_000, extent), &source).is_some());
        }
    }

    #[test]
    fn dense_visible_working_set_survives_repeated_selection_paints() {
        let source = waveform();
        let cache = RefCell::new(WaveformGeometryCache::default());
        // The unchanged native fixture paints seven rows of 50 small audio
        // clips: 350 entries, far below the existing aggregate column budget.
        let mut retained = Vec::new();
        for index in 0..350_u64 {
            let origin = index * 96_000;
            let mapping = WaveformMapping::Source {
                timeline_origin: origin,
            };
            let extent = Bounds::new(
                point(
                    px((index % 50) as f32 * 14.0),
                    px((index / 50) as f32 * 44.0),
                ),
                size(px(9.3), px(44.0)),
            );
            let request = WaveformGeometryRequest {
                mapping: &mapping,
                visible_start: origin,
                visible_end: origin + 96_000,
                bounds: extent,
                scale: 2.0,
                grid: None,
                columns: 19,
            };
            retained.push((
                origin,
                extent,
                cached_waveform_geometry(&cache, &source, &request).unwrap(),
            ));
        }
        for (origin, extent, original) in retained {
            let mapping = WaveformMapping::Source {
                timeline_origin: origin,
            };
            let request = WaveformGeometryRequest {
                mapping: &mapping,
                visible_start: origin,
                visible_end: origin + 96_000,
                bounds: extent,
                scale: 2.0,
                grid: None,
                columns: 19,
            };
            let repeated = cached_waveform_geometry(&cache, &source, &request).unwrap();
            assert!(
                Arc::ptr_eq(&original, &repeated),
                "visible clip geometry was rebuilt at {origin}"
            );
        }
        assert_eq!(cache.borrow().entries.len(), 350);
        assert_eq!(cache.borrow().columns, 350 * 19);
    }

    #[test]
    fn geometry_cache_evicts_by_entry_and_column_limits() {
        let source = waveform();
        let extent = bounds();
        let path_geometry = geometry(&source, 0, extent);
        let mut entry_bounded = WaveformGeometryCache::default();
        for index in 0..(MAX_CACHED_WAVEFORM_ENTRIES + 1) as u64 {
            entry_bounded.insert(
                key(index, extent),
                Arc::clone(&source),
                Arc::clone(&path_geometry),
                1,
            );
        }
        assert_eq!(entry_bounded.entries.len(), MAX_CACHED_WAVEFORM_ENTRIES);
        assert!(entry_bounded.get(key(0, extent), &source).is_none());
        assert_eq!(entry_bounded.columns, MAX_CACHED_WAVEFORM_ENTRIES);

        let mut column_bounded = WaveformGeometryCache::default();
        for index in 0..17_u64 {
            column_bounded.insert(
                key(index, extent),
                Arc::clone(&source),
                Arc::clone(&path_geometry),
                MAX_WAVEFORM_COLUMNS_FOR_TEST,
            );
        }
        assert_eq!(column_bounded.entries.len(), 16);
        assert_eq!(column_bounded.columns, MAX_CACHED_WAVEFORM_COLUMNS);
        assert!(column_bounded.get(key(0, extent), &source).is_none());
    }

    const MAX_WAVEFORM_COLUMNS_FOR_TEST: usize = MAX_CACHED_WAVEFORM_COLUMNS / 16;

    #[test]
    #[ignore = "manual release profile of waveform geometry preparation reuse"]
    fn waveform_geometry_preparation_profile() {
        let source = waveform();
        let extent = bounds();
        let mapping = WaveformMapping::Source { timeline_origin: 0 };
        let visible_start = 0;
        let visible_end = source.source_frames() as u64;
        let columns = waveform_column_count(f32::from(extent.size.width), 2.0);
        let cache = RefCell::new(WaveformGeometryCache::default());
        let request = WaveformGeometryRequest {
            mapping: &mapping,
            visible_start,
            visible_end,
            bounds: extent,
            scale: 2.0,
            columns,
            grid: None,
        };
        let rounds = 20;

        let sampled = aggregate_peaks(&source, 0, source.source_frames())
            .expect("fixture extrema survive cache preparation");
        assert!(sampled.iter().any(|peak| peak.min < -0.9 && peak.max > 0.7));
        let digest = source
            .bins()
            .iter()
            .step_by(127)
            .fold(0_u64, |digest, pair| {
                digest.rotate_left(7)
                    ^ u64::from(pair[0].min.to_bits())
                    ^ u64::from(pair[0].max.to_bits()).rotate_left(17)
            });

        for _ in 0..2 {
            black_box(prepare_waveform_geometry(
                &source,
                &mapping,
                visible_start,
                visible_end,
                extent,
                columns,
                None,
            ));
        }
        let uncached_started = Instant::now();
        for _ in 0..rounds {
            black_box(prepare_waveform_geometry(
                &source,
                &mapping,
                visible_start,
                visible_end,
                extent,
                columns,
                None,
            ));
        }
        let uncached_elapsed = uncached_started.elapsed();

        let warmed = cached_waveform_geometry(&cache, &source, &request)
            .expect("initial cached preparation");
        let cached_started = Instant::now();
        for _ in 0..rounds {
            let geometry =
                cached_waveform_geometry(&cache, &source, &request).expect("warm cache hit");
            black_box(Arc::clone(&geometry));
            assert!(Arc::ptr_eq(&geometry, &warmed));
        }
        let cached_elapsed = cached_started.elapsed();
        eprintln!(
            "waveform geometry prep: source=96000 frames/4096 bins, width=1200px, scale=2, columns={columns}, rounds={rounds}, uncached={uncached_elapsed:?}, warm_lookup={cached_elapsed:?}, extrema={sampled:?}, digest={digest:016x}"
        );
    }
}

// The canvas has no intrinsic size. Pin it to all four edges so its paint
// bounds cover the clip, rather than collapsing to a zero-height child.
