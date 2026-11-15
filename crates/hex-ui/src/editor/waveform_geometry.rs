//! Bounded retained waveform paths for the visible editor working set.
//! Compact bodies combine signed channel extrema; proven duplicated samples use
//! one full-height lane at every height. Source summaries remain unchanged.
use gpui::{Bounds, Pixels, Point, point, px};
use hex_project::Clip;
use hex_session::{CachedPeak, CachedWaveform};
use hex_widgets::{
    MAX_WAVEFORM_COLUMNS, MAX_WAVEFORM_ENVELOPE_POINTS, WaveformChannel, WaveformEnvelope,
    WaveformEnvelopeMode, WaveformStroke,
};
#[cfg(test)]
use std::{cell::Cell, hint::black_box};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use super::waveform::{
    WaveformAmplitudeScale, aggregate_peaks, aggregate_ranges, aggregate_ranges_with_detail,
    fraction_u64, peak_vertical_bounds, timeline_source_ranges_at,
};

#[cfg(test)]
thread_local! {
    static REFERENCE_STEREO: Cell<bool> = const { Cell::new(false) };
}

#[cfg(test)]
pub(super) struct ReferenceStereo(bool);

#[cfg(test)]
pub(super) fn use_reference_stereo(reference: bool) -> ReferenceStereo {
    ReferenceStereo(REFERENCE_STEREO.with(|state| state.replace(reference)))
}

#[cfg(test)]
fn reference_stereo() -> bool {
    REFERENCE_STEREO.with(Cell::get)
}

#[cfg(test)]
impl Drop for ReferenceStereo {
    fn drop(&mut self) {
        REFERENCE_STEREO.with(|state| state.set(self.0));
    }
}

// A fitted dense viewport has 350 small audio clips while using only a few
// thousand columns. Retain that working set without lifting the geometry budget.
const MAX_CACHED_WAVEFORM_ENTRIES: usize = 512;
const MAX_CACHED_WAVEFORM_COLUMNS: usize = 65_536;

/// A view's scale and its shared, bounded geometry store.
#[derive(Default)]
pub(super) struct WaveformViewCache {
    geometry: std::rc::Rc<std::cell::RefCell<WaveformGeometryCache>>,
    amplitude_scale: std::cell::Cell<WaveformAmplitudeScale>,
}

impl WaveformViewCache {
    pub(super) fn new(geometry: std::rc::Rc<std::cell::RefCell<WaveformGeometryCache>>) -> Self {
        Self {
            geometry,
            amplitude_scale: std::cell::Cell::new(WaveformAmplitudeScale::default()),
        }
    }

    pub(super) fn amplitude_scale(&self) -> WaveformAmplitudeScale {
        self.amplitude_scale.get()
    }

    pub(super) fn set_amplitude_scale(&self, scale: WaveformAmplitudeScale) {
        self.amplitude_scale.set(scale);
    }
}

impl std::ops::Deref for WaveformViewCache {
    type Target = std::cell::RefCell<WaveformGeometryCache>;

    fn deref(&self) -> &Self::Target {
        &self.geometry
    }
}

/// UI-owned LRU of ready-to-paint waveform geometry. Entries retain their source
/// Arc, so pointer identity cannot be reused while an entry is resident.
#[derive(Default)]
pub(super) struct WaveformGeometryCache {
    entries: VecDeque<WaveformGeometryEntry>,
    columns: usize,
    coarse_entry_counts: HashMap<CoarseEntryKey, usize>,
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
    #[cfg(test)]
    reference_stereo: bool,
    mapping: WaveformMappingKey,
    amplitude_scale: WaveformAmplitudeScale,
    height: u32,
    scale: u32,
    placement: WaveformPlacementKey,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum WaveformPlacementKey {
    Window {
        visible_start: u64,
        visible_end: u64,
        origin_x: u32,
        origin_y: u32,
        width: u32,
        grid: Option<[u64; 6]>,
    },
    Coarse {
        first_frame: u64,
        frames_per_bin: u64,
        bins: usize,
        pixel_pitch: u32,
        detail_budget: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CoarseEntryKey {
    source_address: usize,
    clip_id: Option<u64>,
    timeline_origin: u64,
    frames_per_bin: u64,
    pixel_pitch: u32,
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
        id: Option<u64>,
        transformed: bool,
        start: u64,
        duration: u64,
        source_offset: u64,
        source_length: u64,
        source_phase: u64,
        looping: bool,
        timeline_origin: u64,
        gain: i32,
        fade: Option<hex_project::ClipFade>,
    },
    Source {
        timeline_origin: u64,
    },
}

pub(super) struct WaveformGeometry {
    display: hex_widgets::WaveformGeometry,
    #[cfg(test)]
    channel_storage: [WaveformChannelGeometry; 2],
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
        &self.channel_storage[..self.display.channels().len()]
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
    spans: [f32; 2],
    channel_count: usize,
}

#[cfg(all(test, feature = "ui-test-support"))]
#[derive(Clone, Copy, Debug)]
pub(super) struct WaveformPaintSample {
    pub(super) colors: hex_widgets::WaveformColors,
    pub(super) bounds: Bounds<Pixels>,
    pub(super) mask: Bounds<Pixels>,
    ordinates: [(f32, f32); 2],
    channel_count: usize,
    pub(super) envelope_points: usize,
    pub(super) outline_fingerprint: [u64; 2],
}

#[cfg(all(test, feature = "ui-test-support"))]
impl RenderedWaveformSample {
    pub(super) fn spans(&self) -> &[f32] {
        &self.spans[..self.channel_count]
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
impl WaveformPaintSample {
    pub(super) fn ordinates(&self) -> &[(f32, f32)] {
        &self.ordinates[..self.channel_count]
    }
}

impl WaveformGeometryCache {
    fn get(
        &mut self,
        key: &WaveformGeometryKey,
        source: &Arc<CachedWaveform>,
    ) -> Option<Arc<WaveformGeometry>> {
        // The index can reject absent signatures only; resident hits still
        // require exact source and complete geometry-key equality below.
        if coarse_entry_key(key, source)
            .is_some_and(|coarse_key| !self.coarse_entry_counts.contains_key(&coarse_key))
        {
            return None;
        }
        let index = self
            .entries
            .iter()
            .position(|entry| Arc::ptr_eq(&entry.source, source) && &entry.key == key)?;
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
            self.remove_coarse_entry(coarse_entry_key(&removed.key, &removed.source));
        }
        self.columns = self.columns.saturating_add(columns);
        let coarse_entry_key = coarse_entry_key(&key, &source);
        if let Some(coarse_key) = coarse_entry_key {
            *self.coarse_entry_counts.entry(coarse_key).or_default() += 1;
        }
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
        self.coarse_entry_counts.clear();
        #[cfg(all(test, feature = "ui-test-support"))]
        self.painted.clear();
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn record_paint(
        &mut self,
        bounds: Bounds<Pixels>,
        mask: Bounds<Pixels>,
        geometry: &WaveformGeometry,
        offset: Point<Pixels>,
        colors: hex_widgets::WaveformColors,
    ) {
        let _navigation_timer = super::navigation_metrics::Timer::start(
            super::navigation_metrics::Stage::PaintObservation,
        );
        if self.painted.len() == MAX_CACHED_WAVEFORM_ENTRIES {
            self.painted.pop_front();
        }
        let observations: [(f32, f32, u64); 2] = std::array::from_fn(|channel| {
            geometry
                .channels()
                .get(channel)
                .map_or((0.0, 0.0, 0), |channel| {
                    channel.outline.iter().fold(
                        (f32::INFINITY, f32::NEG_INFINITY, 0xcbf29ce484222325_u64),
                        |(lo, hi, hash), point| {
                            let y = f32::from(point.y + offset.y);
                            (
                                lo.min(y),
                                hi.max(y),
                                (hash ^ u64::from(y.to_bits())).wrapping_mul(0x100000001b3),
                            )
                        },
                    )
                })
        });
        self.painted.push_back(WaveformPaintSample {
            colors,
            bounds,
            mask,
            channel_count: geometry.channels().len(),
            ordinates: observations.map(|(lo, hi, _)| (lo, hi)),
            envelope_points: geometry
                .channels()
                .iter()
                .map(|channel| channel.outline.len())
                .sum(),
            outline_fingerprint: observations.map(|(_, _, fingerprint)| fingerprint),
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
                channel_count: entry.geometry.channels().len(),
                spans: std::array::from_fn(|channel| {
                    entry
                        .geometry
                        .channels()
                        .get(channel)
                        .map_or(0.0, |channel| {
                            let (low, high) = channel
                                .outline
                                .iter()
                                .map(|p| f32::from(p.y))
                                .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), y| {
                                    (low.min(y), high.max(y))
                                });
                            high - low
                        })
                }),
            })
            .collect()
    }

    #[cfg(test)]
    pub(super) fn entry_count(&self) -> usize {
        self.entries.len()
    }

    fn remove_coarse_entry(&mut self, key: Option<CoarseEntryKey>) {
        let Some(key) = key else {
            return;
        };
        match self.coarse_entry_counts.get_mut(&key) {
            Some(count) if *count > 1 => *count -= 1,
            Some(_) => {
                self.coarse_entry_counts.remove(&key);
            }
            None => debug_assert!(false, "coarse geometry index count missing"),
        }
    }
}

fn coarse_entry_key(
    key: &WaveformGeometryKey,
    source: &Arc<CachedWaveform>,
) -> Option<CoarseEntryKey> {
    let WaveformMappingKey::Clip {
        id,
        timeline_origin,
        ..
    } = &key.mapping
    else {
        return None;
    };
    let WaveformPlacementKey::Coarse {
        frames_per_bin,
        pixel_pitch,
        ..
    } = &key.placement
    else {
        return None;
    };
    Some(CoarseEntryKey {
        source_address: Arc::as_ptr(source) as usize,
        clip_id: *id,
        timeline_origin: *timeline_origin,
        frames_per_bin: *frames_per_bin,
        pixel_pitch: *pixel_pitch,
    })
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
        /// Arrangement fade to display, including a held or pending preview.
        /// Source-editing waveforms may omit this independent clip envelope.
        fade: Option<hex_project::ClipFade>,
    },
    Source {
        timeline_origin: u64,
    },
}

mod grid;
pub(super) use grid::WaveformGrid;

pub(super) struct WaveformGeometryRequest<'a> {
    pub(super) mapping: &'a WaveformMapping,
    pub(super) amplitude_scale: WaveformAmplitudeScale,
    pub(super) visible_start: u64,
    pub(super) visible_end: u64,
    pub(super) bounds: Bounds<Pixels>,
    pub(super) scale: f32,
    pub(super) columns: usize,
    pub(super) grid: Option<WaveformGrid>,
}

#[derive(Clone, Copy)]
struct WaveformLayout {
    grid: WaveformGrid,
    frames: u64,
    first: usize,
    last: usize,
    point_count: usize,
}

impl WaveformLayout {
    fn new(request: &WaveformGeometryRequest<'_>) -> Option<Self> {
        let width = f32::from(request.bounds.size.width);
        let height = f32::from(request.bounds.size.height);
        let grid = request.grid.unwrap_or(WaveformGrid {
            start: request.visible_start,
            end: request.visible_end,
            left: f32::from(request.bounds.origin.x),
            width,
            columns: request.columns,
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
        // Keep a neighbor on each side for the clip's overflow mask to crop.
        let first = ((u128::from(request.visible_start.saturating_sub(grid.start))
            * grid.columns as u128)
            / u128::from(frames))
        .min(grid.columns as u128) as usize;
        let last = (u128::from(request.visible_end.saturating_sub(grid.start))
            * grid.columns as u128)
            .div_ceil(u128::from(frames))
            .min(grid.columns as u128) as usize;
        let first = first.saturating_sub(1);
        let last = last.saturating_add(1).min(grid.columns);
        let point_count =
            last.saturating_sub(first)
                .checked_mul(if grid.binned { 2 } else { 1 })?;
        if point_count > MAX_WAVEFORM_ENVELOPE_POINTS {
            return None;
        }
        Some(Self {
            grid,
            frames,
            first,
            last,
            point_count,
        })
    }

    fn detail_budget(self) -> usize {
        (4_194_304 / self.grid.columns).min(4_096)
    }

    /// Exact bins survive a viewport's padded grid moving by one or more steps.
    /// The nonintegral fallback keeps its window-specific geometry key.
    fn coarse_placement(self, bounds: Bounds<Pixels>) -> Option<CoarsePlacement> {
        let frames_per_bin = self.frames / self.grid.columns as u64;
        if frames_per_bin == 0
            || !self.frames.is_multiple_of(self.grid.columns as u64)
            || self.first >= self.last
        {
            return None;
        }
        let first_frame = self
            .grid
            .start
            .checked_add(frames_per_bin.checked_mul(self.first as u64)?)?;
        let pitch = self.grid.width / self.grid.columns as f32;
        let left = self.grid.left + self.first as f32 * self.grid.width / self.grid.columns as f32;
        let bins = self.last - self.first;
        if pitch <= 0.0
            || !pitch.is_finite()
            || !left.is_finite()
            || !(left + bins as f32 * pitch).is_finite()
            || !f32::from(bounds.origin.y).is_finite()
            || !f32::from(bounds.bottom()).is_finite()
        {
            return None;
        }
        Some(CoarsePlacement {
            key: WaveformPlacementKey::Coarse {
                first_frame,
                frames_per_bin,
                bins,
                pixel_pitch: pitch.to_bits(),
                detail_budget: self.detail_budget(),
            },
            pitch,
            offset: point(px(left), bounds.origin.y),
        })
    }
}

struct CoarsePlacement {
    key: WaveformPlacementKey,
    pitch: f32,
    offset: Point<Pixels>,
}

pub(super) fn cached_waveform_geometry(
    cache: &std::cell::RefCell<WaveformGeometryCache>,
    waveform: &Arc<CachedWaveform>,
    request: &WaveformGeometryRequest<'_>,
) -> Option<(Arc<WaveformGeometry>, Point<Pixels>)> {
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
    key.amplitude_scale = request.amplitude_scale;
    let mut layout = None;
    let mut pitch = None;
    let mut offset = Point::default();
    if matches!(request.mapping, WaveformMapping::Clip { .. })
        && request.grid.is_some_and(|grid| grid.binned)
    {
        let columns = WaveformLayout::new(request)?;
        if let Some(local) = columns.coarse_placement(request.bounds) {
            // Plain clips with the same complete source and timeline projection
            // can share the retained local path across tracks. Placement still
            // owns the exact absolute bins, scale, height and sampling budget.
            if let WaveformMappingKey::Clip {
                id,
                transformed: false,
                ..
            } = &mut key.mapping
            {
                *id = None;
            }
            key.placement = local.key;
            pitch = Some(local.pitch);
            offset = local.offset;
        }
        layout = Some(columns);
    }
    if let WaveformPlacementKey::Window { grid, .. } = &mut key.placement {
        *grid = request.grid.map(WaveformGrid::key);
    }
    if let Some(geometry) = cache
        .try_borrow_mut()
        .ok()
        .and_then(|mut cache| cache.get(&key, waveform))
    {
        return Some((geometry, offset));
    }
    let layout = layout.or_else(|| WaveformLayout::new(request))?;
    let geometry = Arc::new(build_waveform_geometry(waveform, request, layout, pitch)?);
    if let Ok(mut cache) = cache.try_borrow_mut() {
        cache.insert(
            key,
            Arc::clone(waveform),
            Arc::clone(&geometry),
            geometry.columns,
        );
    }
    Some((geometry, offset))
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
            fade,
        } => WaveformMappingKey::Clip {
            id: Some(clip.id().value()),
            transformed: clip.audio_edit().is_some(),
            gain: clip.gain().millidb(),
            start: clip.start(),
            duration: clip.duration(),
            source_offset: clip.source_offset(),
            source_length: clip.source_length(),
            source_phase: clip.source_phase(),
            looping: clip.looping(),
            timeline_origin: *timeline_origin,
            fade: *fade,
        },
        WaveformMapping::Source { timeline_origin } => WaveformMappingKey::Source {
            timeline_origin: *timeline_origin,
        },
    };
    WaveformGeometryKey {
        #[cfg(test)]
        reference_stereo: reference_stereo(),
        mapping,
        amplitude_scale: WaveformAmplitudeScale::default(),
        height: f32::from(bounds.size.height).to_bits(),
        scale: scale.to_bits(),
        placement: WaveformPlacementKey::Window {
            visible_start,
            visible_end,
            origin_x: f32::from(bounds.origin.x).to_bits(),
            origin_y: f32::from(bounds.origin.y).to_bits(),
            width: f32::from(bounds.size.width).to_bits(),
            grid: None,
        },
    }
}

#[cfg(test)]
pub(super) fn prepare_waveform_geometry(
    waveform: &CachedWaveform,
    mapping: &WaveformMapping,
    visible_start: u64,
    visible_end: u64,
    bounds: Bounds<Pixels>,
    columns: usize,
    grid: Option<WaveformGrid>,
) -> Option<WaveformGeometry> {
    prepare_waveform_geometry_with_scale(
        waveform,
        &WaveformGeometryRequest {
            mapping,
            amplitude_scale: WaveformAmplitudeScale::default(),
            visible_start,
            visible_end,
            bounds,
            scale: 1.0,
            columns,
            grid,
        },
    )
}

pub(super) fn prepare_waveform_geometry_with_scale(
    waveform: &CachedWaveform,
    request: &WaveformGeometryRequest<'_>,
) -> Option<WaveformGeometry> {
    build_waveform_geometry(waveform, request, WaveformLayout::new(request)?, None)
}

fn build_waveform_geometry(
    waveform: &CachedWaveform,
    request: &WaveformGeometryRequest<'_>,
    layout: WaveformLayout,
    local_pitch: Option<f32>,
) -> Option<WaveformGeometry> {
    let WaveformGeometryRequest {
        mapping,
        bounds,
        amplitude_scale,
        ..
    } = *request;
    let WaveformLayout {
        grid,
        frames,
        first,
        last,
        point_count,
    } = layout;
    #[cfg(all(test, feature = "ui-test-support"))]
    let _navigation_timer =
        super::navigation_metrics::Timer::start(super::navigation_metrics::Stage::WaveformBuild);
    let height = f32::from(bounds.size.height);
    let y = if local_pitch.is_some() {
        px(0.0)
    } else {
        bounds.origin.y
    };
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
    let clip_fade = match mapping {
        WaveformMapping::Clip {
            clip,
            fade: Some(fade),
            ..
        } if !fade.is_neutral() => Some((
            clip.start(),
            clip.start().saturating_add(clip.duration()),
            *fade,
        )),
        _ => None,
    };
    #[cfg(all(test, feature = "ui-test-support"))]
    let sampling_timer =
        super::navigation_metrics::Timer::start(super::navigation_metrics::Stage::WaveformSampling);
    let mut stereo_peaks = Vec::with_capacity(last.saturating_sub(first));
    // A fixed total bound keeps close-zoom refinement proportional to the
    // viewport, including columns that straddle a loop boundary.
    let detail_budget = layout.detail_budget();
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
            WaveformMapping::Loop(projection) => {
                projection.peaks_with_detail(waveform, start..end, detail_budget)
            }
            WaveformMapping::Stretch(projection) => {
                let ranges = projection.ranges(start..end, waveform.source_frames());
                aggregate_ranges_with_detail(waveform, ranges.as_slice(), detail_budget)
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
                    waveform
                        .bounded_exact_peaks_for_frames(start, end, detail_budget)
                        .or_else(|| aggregate_peaks(waveform, start, end))
                } else {
                    aggregate_ranges_with_detail(
                        waveform,
                        std::slice::from_ref(&(start..end)),
                        detail_budget,
                    )
                }
            }
            WaveformMapping::Clip {
                clip,
                timeline_origin,
                ..
            } => {
                let ranges = timeline_source_ranges_at(
                    clip,
                    *timeline_origin,
                    start..end,
                    waveform.source_frames(),
                );
                aggregate_ranges_with_detail(waveform, ranges.as_slice(), detail_budget)
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
        let fade_gain = match clip_fade {
            Some((clip_start, clip_end, fade)) => {
                // A cached peak represents an interval; use its timeline
                // midpoint within the clip to sample the envelope without PCM.
                let first = start.max(clip_start);
                let last = end.min(clip_end);
                if first < last {
                    let midpoint = first + (last - first - 1) / 2;
                    fade.gain_at(midpoint - clip_start)
                } else {
                    0.0
                }
            }
            _ => 1.0,
        };
        let gain = gain * fade_gain;
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
    let combined =
        height < crate::theme::WAVEFORM_STEREO_MIN_BODY_HEIGHT || waveform.has_identical_channels();
    // Compare legacy stereo in the same test binary/window; no shipped override.
    #[cfg(all(test, feature = "ui-test-support"))]
    let combined = combined && !reference_stereo();
    let channel_count = if combined { 1 } else { 2 };
    let mut channels = Vec::with_capacity(channel_count);
    #[cfg(test)]
    let mut outlines = std::array::from_fn(|_| WaveformChannelGeometry {
        outline: Vec::new(),
    });
    for channel in 0..channel_count {
        let band_height = height / channel_count as f32;
        let band_top = band_height * channel as f32;
        let center = band_top + band_height * 0.5;
        let amplitude = (band_height * 0.44).max(0.5);
        let mut maxima = Vec::with_capacity(point_count);
        let mut minima = Vec::with_capacity(point_count);
        for (column, peaks) in stereo_peaks.iter().enumerate() {
            let peak = if combined {
                CachedPeak {
                    min: peaks[0].min.min(peaks[1].min),
                    max: peaks[0].max.max(peaks[1].max),
                }
            } else {
                peaks[channel]
            };
            let (top, bottom) = peak_vertical_bounds(peak, center, amplitude, amplitude_scale);
            if grid.binned {
                for edge in [column, column + 1] {
                    let x = if let Some(pitch) = local_pitch {
                        edge as f32 * pitch
                    } else {
                        grid.left + (first + edge) as f32 * grid.width / grid.columns as f32
                    };
                    maxima.push(point(px(x), y + px(top)));
                    minima.push(point(px(x), y + px(bottom)));
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
    let display = if combined {
        hex_widgets::WaveformGeometry::mono(channels.pop()?)
    } else {
        hex_widgets::WaveformGeometry::stereo(channels.try_into().ok()?)
    };
    Some(WaveformGeometry {
        display,
        #[cfg(test)]
        channel_storage: outlines,
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

    #[cfg(all(test, not(feature = "ui-test-support")))]
    #[test]
    fn plain_clip_fade_shapes_peaks_and_changes_cache_identity() {
        use hex_project::{Command, Created, Editor, Project};

        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "fade".into(),
                path: "fade.wav".into(),
                sample_rate: 48_000,
                frames: 100,
            })
            .unwrap()
        else {
            panic!("imported clip")
        };
        let neutral = editor.project().clip(clip).unwrap().clone();
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 40,
                fade_out: 40,
            })
            .unwrap();
        let faded = editor.project().clip(clip).unwrap().clone();
        let peak = CachedPeak {
            min: -1.0,
            max: 1.0,
        };
        let waveform = Arc::new(CachedWaveform::from_peaks(100, vec![[peak, peak]; 100]).unwrap());
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(100.0)));
        let mapping = |clip: &Clip| WaveformMapping::Clip {
            clip: clip.clone(),
            timeline_origin: 0,
            fade: Some(clip.retained_fade()),
        };
        let held = WaveformMapping::Clip {
            clip: neutral.clone(),
            timeline_origin: 0,
            fade: Some(faded.retained_fade()),
        };
        let cache = RefCell::new(WaveformGeometryCache::default());
        let get = |mapping: &WaveformMapping| {
            cached_waveform_geometry(
                &cache,
                &waveform,
                &WaveformGeometryRequest {
                    amplitude_scale: crate::editor::waveform::WaveformAmplitudeScale::from_decibels(
                        false,
                    ),
                    mapping,
                    visible_start: 0,
                    visible_end: 100,
                    bounds,
                    scale: 1.0,
                    columns: 100,
                    grid: None,
                },
            )
            .unwrap()
            .0
        };
        let neutral = get(&mapping(&neutral));
        let faded = get(&mapping(&faded));
        assert!(
            !Arc::ptr_eq(&neutral, &faded),
            "fade edits invalidate cached geometry"
        );
        assert!(
            Arc::ptr_eq(&faded, &get(&held)),
            "held and canonical fades share the same geometry"
        );
        let neutral_top = &neutral.channels()[0].outline[..100];
        let faded_top = &faded.channels()[0].outline[..100];
        assert!(faded_top[0].y > neutral_top[0].y);
        assert_eq!(faded_top[50].y, neutral_top[50].y);
        assert!(faded_top[99].y > neutral_top[99].y);
        let neutral_bottom = &neutral.channels()[0].outline[100..];
        let faded_bottom = &faded.channels()[0].outline[100..];
        assert!(faded_bottom[0].y < neutral_bottom[0].y);
        assert_eq!(faded_bottom[50].y, neutral_bottom[50].y);
        assert!(faded_bottom[99].y < neutral_bottom[99].y);
    }

    #[cfg(all(test, not(feature = "ui-test-support")))]
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

    #[cfg(all(test, not(feature = "ui-test-support")))]
    fn coarse_key(clip_id: u64, first_frame: u64) -> WaveformGeometryKey {
        WaveformGeometryKey {
            reference_stereo: reference_stereo(),
            mapping: WaveformMappingKey::Clip {
                id: Some(clip_id),
                transformed: false,
                start: first_frame,
                duration: 256,
                source_offset: 0,
                source_length: 256,
                source_phase: 0,
                looping: false,
                timeline_origin: 0,
                gain: 0,
                fade: None,
            },
            amplitude_scale: WaveformAmplitudeScale::default(),
            height: 180.0_f32.to_bits(),
            scale: 1.0_f32.to_bits(),
            placement: WaveformPlacementKey::Coarse {
                first_frame,
                frames_per_bin: 128,
                bins: 2,
                pixel_pitch: 2.0_f32.to_bits(),
                detail_budget: 4_096,
            },
        }
    }

    #[cfg(all(test, not(feature = "ui-test-support")))]
    #[test]
    fn comparison_cache_identity_is_scoped_and_restored() {
        assert!(!reference_stereo());
        let current = key(0, bounds());
        {
            let _reference = use_reference_stereo(true);
            assert!(reference_stereo());
            assert_ne!(current, key(0, bounds()));
            {
                let _nested = use_reference_stereo(false);
                assert_eq!(current, key(0, bounds()));
            }
            assert!(reference_stereo());
        }
        assert!(!reference_stereo());
        assert_eq!(current, key(0, bounds()));
    }

    #[cfg(all(test, not(feature = "ui-test-support")))]
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

    #[cfg(all(test, not(feature = "ui-test-support")))]
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
            let peaks: Vec<_> = geometry.channels()[0].outline[..100]
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

    #[cfg(all(test, not(feature = "ui-test-support")))]
    #[test]
    fn arrangement_clip_keeps_a_transient_in_its_visible_column() {
        use hex_project::{Command, Created, Editor, Project};

        let mut samples = vec![[0.0, 0.0]; 1_024_000];
        samples[2_010] = [1.0, 1.0];
        let audio = hex_engine::Audio::new(48_000, samples).unwrap();
        let waveform = hex_media::waveform::summarize(&audio, 1_024).unwrap();
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "track".into(),
                path: "audio.wav".into(),
                sample_rate: 48_000,
                frames: 1_024_000,
            })
            .unwrap()
        else {
            panic!("imported audio")
        };
        let Created::Clip(id) = editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 0,
                duration: 6_500,
                source_offset: 0,
                source_length: 1_024_000,
                looping: false,
            })
            .unwrap()
        else {
            panic!("placed clip")
        };
        let extent = Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(100.0)));
        let geometry = prepare_waveform_geometry(
            &waveform,
            &WaveformMapping::Clip {
                clip: editor.project().clip(id).unwrap().clone(),
                timeline_origin: 0,
                fade: None,
            },
            0,
            6_500,
            extent,
            100,
            None,
        )
        .unwrap();
        let peaks: Vec<_> = geometry.channels()[0].outline[..100]
            .iter()
            .enumerate()
            .filter(|(_, point)| point.y < px(25.0))
            .map(|(index, _)| index)
            .collect();
        assert_eq!(peaks, vec![2_010 / 65]);
    }

    #[cfg(all(test, not(feature = "ui-test-support")))]
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
            let top = &geometry.channels()[0].outline[..100];
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
        let peaks: Vec<_> = geometry.channels()[0].outline[..100]
            .iter()
            .enumerate()
            .filter(|(_, p)| p.y < px(25.0))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(peaks, vec![6_010 / 2_050]);
    }

    #[cfg(all(test, not(feature = "ui-test-support")))]
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
                    let outline = &geometry.channels()[channel].outline;
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

    #[cfg(all(test, not(feature = "ui-test-support")))]
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
        let outline = &geometry.channels()[0].outline;
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

    #[cfg(all(test, not(feature = "ui-test-support")))]
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
                let outline = &geometry.channels()[channel].outline;
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

    #[cfg(all(test, not(feature = "ui-test-support")))]
    #[test]
    fn coarse_aligned_clips_share_only_equivalent_geometry() {
        use hex_project::{Command, Created, Editor, Project};

        let start = 96_000;
        let duration = 48_000;
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: start,
            })
            .unwrap()
        else {
            panic!("imported source")
        };
        let place = |editor: &mut Editor, track, source_offset| {
            let Created::Clip(id) = editor
                .execute(Command::PlaceClip {
                    track,
                    asset,
                    start,
                    duration,
                    source_offset,
                    source_length: duration,
                    looping: false,
                })
                .unwrap()
            else {
                panic!("placed clip")
            };
            id
        };
        let first = place(&mut editor, track, 0);
        let others = [0, 1_024].map(|source_offset| {
            let Created::Track(track) = editor
                .execute(Command::AddTrack {
                    name: "copy".into(),
                })
                .unwrap()
            else {
                panic!("added track")
            };
            place(&mut editor, track, source_offset)
        });
        let source = waveform();
        let cache = RefCell::new(WaveformGeometryCache::default());
        let cached = |id, timeline_origin, y| {
            let mapping = WaveformMapping::Clip {
                clip: editor.project().clip(id).unwrap().clone(),
                timeline_origin,
                fade: None,
            };
            cached_waveform_geometry(
                &cache,
                &source,
                &WaveformGeometryRequest {
                    mapping: &mapping,
                    amplitude_scale: WaveformAmplitudeScale::default(),
                    visible_start: start,
                    visible_end: start + duration,
                    bounds: Bounds::new(point(px(202.25), px(y)), size(px(96.0), px(80.0))),
                    scale: 2.0,
                    columns: 144,
                    grid: Some(WaveformGrid {
                        start: 0,
                        end: start + duration,
                        left: 10.25,
                        width: 288.0,
                        columns: 144,
                        binned: true,
                    }),
                },
            )
            .unwrap()
        };
        let (first_geometry, first_offset) = cached(first, start, 34.25);
        let (second_geometry, second_offset) = cached(others[0], start, 134.25);
        let outline = &first_geometry.channels()[0].outline;
        assert!(outline.iter().any(|point| point.y != outline[0].y));
        assert!(Arc::ptr_eq(&first_geometry, &second_geometry));
        assert_eq!(first_offset.x, second_offset.x);
        assert_eq!(second_offset.y - first_offset.y, px(100.0));
        assert_eq!(cache.borrow().entry_count(), 1);
        for (id, origin) in [(others[1], start), (others[0], start + 1)] {
            assert!(!Arc::ptr_eq(&first_geometry, &cached(id, origin, 134.25).0));
        }
    }

    #[cfg(all(test, not(feature = "ui-test-support")))]
    #[test]
    fn coarse_clip_cache_reuses_paths_across_aligned_grid_shifts() {
        use crate::editor::viewport::Viewport;
        use hex_project::{Command, Created, Editor, Project};

        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "coarse".into(),
                path: "coarse.wav".into(),
                sample_rate: 48_000,
                frames: 24_000,
            })
            .unwrap()
        else {
            panic!("imported clip")
        };
        let mapping = WaveformMapping::Clip {
            clip: editor.project().clip(clip).unwrap().clone(),
            timeline_origin: 40_000,
            fade: None,
        };
        let source = waveform();
        let cache = RefCell::new(WaveformGeometryCache::default());
        let grid = |start, length| {
            WaveformGrid::arrangement(Viewport { start, length }, 12.25, 400.25, 2.0, 48_000)
                .unwrap()
        };
        let request = WaveformGeometryRequest {
            mapping: &mapping,
            amplitude_scale: WaveformAmplitudeScale::default(),
            visible_start: 40_000,
            visible_end: 64_000,
            bounds: Bounds::new(point(px(112.5), px(34.25)), size(px(120.0), px(80.0))),
            scale: 2.0,
            columns: 240,
            grid: Some(grid(20_000, 80_000)),
        };
        let (initial, initial_offset) =
            cached_waveform_geometry(&cache, &source, &request).unwrap();
        let panned = WaveformGeometryRequest {
            bounds: Bounds::new(point(px(109.5), px(18.75)), request.bounds.size),
            grid: Some(grid(21_203, 80_000)),
            ..request
        };
        assert_ne!(request.grid.unwrap().start, panned.grid.unwrap().start);
        let (retained, offset) = cached_waveform_geometry(&cache, &source, &panned).unwrap();
        assert!(Arc::ptr_eq(&initial, &retained));
        assert_ne!(initial_offset, offset);
        let fresh = prepare_waveform_geometry_with_scale(&source, &panned).unwrap();
        assert_eq!(retained.channels().len(), fresh.channels().len());
        for (local, fresh) in retained.channels().iter().zip(fresh.channels()) {
            assert_eq!(local.outline.len(), fresh.outline.len());
            for (local, fresh) in local.outline.iter().zip(&fresh.outline) {
                assert!((f32::from(local.x + offset.x - fresh.x)).abs() < 0.001);
                assert_eq!(local.y + offset.y, fresh.y);
            }
        }

        for changed in [
            WaveformGeometryRequest {
                visible_start: 42_000,
                ..panned
            },
            WaveformGeometryRequest {
                grid: Some(grid(21_203, 72_000)),
                ..panned
            },
            WaveformGeometryRequest {
                bounds: Bounds::new(panned.bounds.origin, size(px(120.0), px(90.0))),
                ..panned
            },
            WaveformGeometryRequest {
                scale: 1.0,
                ..panned
            },
        ] {
            let (new_path, _) = cached_waveform_geometry(&cache, &source, &changed).unwrap();
            assert!(!Arc::ptr_eq(&initial, &new_path));
        }
        let (new_source, _) = cached_waveform_geometry(&cache, &waveform(), &panned).unwrap();
        assert!(!Arc::ptr_eq(&initial, &new_source));
    }

    #[cfg(all(test, not(feature = "ui-test-support")))]
    #[test]
    fn coarse_cache_uses_exact_bin_identity_at_the_timeline_limit() {
        let request = WaveformGeometryRequest {
            mapping: &WaveformMapping::Source { timeline_origin: 0 },
            amplitude_scale: WaveformAmplitudeScale::default(),
            visible_start: u64::MAX - 700,
            visible_end: u64::MAX - 300,
            bounds: bounds(),
            scale: 2.0,
            columns: 10,
            grid: None,
        };
        let placement = |start, end| {
            WaveformLayout::new(&WaveformGeometryRequest {
                grid: Some(WaveformGrid {
                    start,
                    end,
                    left: 8.0,
                    width: 100.0,
                    columns: 10,
                    binned: true,
                }),
                ..request
            })
            .unwrap()
            .coarse_placement(request.bounds)
        };
        let first = placement(u64::MAX - 1_100, u64::MAX - 100).unwrap();
        let pinned = placement(u64::MAX - 1_000, u64::MAX).unwrap();
        assert_eq!(first.key, pinned.key);
        assert_ne!(first.offset, pinned.offset);
        assert!(placement(u64::MAX - 1_000, u64::MAX - 1).is_none());
    }

    #[cfg(all(test, not(feature = "ui-test-support")))]
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
                            geometry.channels()[channel]
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
                    revealed.channels()[0]
                        .outline
                        .iter()
                        .any(|p| f32::from(p.x) < grid.left + width * 4_900.0 / 96_000.0)
                );
                assert!(
                    revealed.channels()[0]
                        .outline
                        .iter()
                        .any(|p| f32::from(p.x) > grid.left + width * 91_100.0 / 96_000.0)
                );
            }
        }
    }

    #[cfg(all(test, not(feature = "ui-test-support")))]
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
                retained_crop.channels()[channel].outline == full_root.channels()[channel].outline,
                "same accepted root/time positions retain edge-bin peaks after a crop"
            );
        }
    }

    #[cfg(all(test, not(feature = "ui-test-support")))]
    #[test]
    fn canonical_trim_and_lower_fade_resize_keep_loop_and_one_shot_outline_still() {
        use hex_project::{Command, Created, Editor, Project, TrimEdge};
        let source = waveform();
        let mut fixture = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported {
            track,
            clip: imported,
            ..
        } = fixture
            .execute(Command::ImportAudioTrack {
                name: "audio source".into(),
                path: "missing.wav".into(),
                sample_rate: 48_000,
                frames: 96_000,
            })
            .unwrap()
        else {
            panic!("import result")
        };
        let project = fixture.project().clone();
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
                        fade: Some(clip.retained_fade()),
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
                            g.channels()[channel]
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

    #[cfg(all(test, not(feature = "ui-test-support")))]
    #[test]
    fn geometry_cache_separates_vertical_amplitude_scales() {
        let peak = CachedPeak {
            min: -0.1,
            max: 0.1,
        };
        let source = Arc::new(CachedWaveform::from_peaks(100, vec![[peak, peak]; 100]).unwrap());
        let extent = bounds();
        let mapping = WaveformMapping::Source { timeline_origin: 0 };
        let cache = RefCell::new(WaveformGeometryCache::default());
        let get = |decibels| {
            cached_waveform_geometry(
                &cache,
                &source,
                &WaveformGeometryRequest {
                    mapping: &mapping,
                    amplitude_scale: WaveformAmplitudeScale::from_decibels(decibels),
                    visible_start: 0,
                    visible_end: 100,
                    bounds: extent,
                    scale: 1.0,
                    columns: 100,
                    grid: None,
                },
            )
            .unwrap()
            .0
        };
        let linear = get(false);
        let decibel = get(true);
        assert!(!Arc::ptr_eq(&linear, &decibel));
        assert!(Arc::ptr_eq(&linear, &get(false)));
        assert!(Arc::ptr_eq(&decibel, &get(true)));
        assert!(decibel.channels()[0].outline[0].y < linear.channels()[0].outline[0].y);
    }

    #[cfg(all(test, not(feature = "ui-test-support")))]
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
            &cache.get(&cache_key, &source).expect("cache hit"),
            &geometry,
        ));
        assert!(
            Arc::strong_count(&source) >= 2,
            "cache retains source identity"
        );

        let resized = Bounds::new(extent.origin, size(px(1201.0), extent.size.height));
        assert!(cache.get(&key(0, resized), &source).is_none());
        let scrolled = Bounds::new(
            point(extent.origin.x, extent.origin.y + px(44.0)),
            extent.size,
        );
        assert!(cache.get(&key(0, scrolled), &source).is_none());
        let scale_changed = waveform_geometry_key(
            &WaveformMapping::Source { timeline_origin: 0 },
            0,
            96_000,
            extent,
            1.0,
        );
        assert!(cache.get(&scale_changed, &source).is_none());
        assert!(cache.get(&key(1, extent), &source).is_none());
        let different_source = waveform();
        assert!(cache.get(&cache_key, &different_source).is_none());
    }

    #[cfg(all(test, not(feature = "ui-test-support")))]
    #[test]
    fn coarse_absence_counts_survive_shared_signature_eviction_and_clear() {
        let source = waveform();
        let geometry = geometry(&source, 0, bounds());
        let first = coarse_key(1, 0);
        let second = coarse_key(1, 256);
        let index_key = coarse_entry_key(&first, &source).unwrap();
        let mut cache = WaveformGeometryCache::default();
        for key in [first.clone(), second.clone()] {
            cache.insert(key, Arc::clone(&source), Arc::clone(&geometry), 1);
        }
        assert_eq!(cache.coarse_entry_counts.get(&index_key), Some(&2));

        for clip_id in 2..MAX_CACHED_WAVEFORM_ENTRIES as u64 {
            cache.insert(
                coarse_key(clip_id, clip_id * 256),
                Arc::clone(&source),
                Arc::clone(&geometry),
                1,
            );
        }
        cache.insert(
            coarse_key(10_000, 0),
            Arc::clone(&source),
            Arc::clone(&geometry),
            1,
        );
        assert_eq!(cache.coarse_entry_counts.get(&index_key), Some(&1));
        assert!(cache.get(&second, &source).is_some());
        assert_eq!(cache.coarse_entry_counts.get(&index_key), Some(&1));

        for clip_id in 20_000..20_000 + MAX_CACHED_WAVEFORM_ENTRIES as u64 {
            cache.insert(
                coarse_key(clip_id, clip_id * 256),
                Arc::clone(&source),
                Arc::clone(&geometry),
                1,
            );
        }
        assert!(!cache.coarse_entry_counts.contains_key(&index_key));
        assert!(cache.get(&second, &source).is_none());

        cache.clear();
        assert!(cache.coarse_entry_counts.is_empty());
        assert!(cache.entries.is_empty());
        assert_eq!(cache.columns, 0);
    }

    #[cfg(all(test, not(feature = "ui-test-support")))]
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
                    amplitude_scale: crate::editor::waveform::WaveformAmplitudeScale::from_decibels(
                        false,
                    ),
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
            .0
        };
        let first = geometry(&neutral);
        let changed = geometry(&quiet);
        assert!(!Arc::ptr_eq(&first, &changed));
        assert!(Arc::ptr_eq(&first, &geometry(&neutral)));
        assert!(!Arc::ptr_eq(&first, &geometry(&detailed)));
        assert_eq!(cache.borrow().entries.len(), 3);
    }

    #[cfg(all(test, not(feature = "ui-test-support")))]
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
            assert!(cache.get(&key(index * 96_000, extent), &source).is_some());
        }
    }

    #[cfg(all(test, not(feature = "ui-test-support")))]
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
                amplitude_scale: crate::editor::waveform::WaveformAmplitudeScale::from_decibels(
                    false,
                ),
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
                cached_waveform_geometry(&cache, &source, &request)
                    .unwrap()
                    .0,
            ));
        }
        for (origin, extent, original) in retained {
            let mapping = WaveformMapping::Source {
                timeline_origin: origin,
            };
            let request = WaveformGeometryRequest {
                amplitude_scale: crate::editor::waveform::WaveformAmplitudeScale::from_decibels(
                    false,
                ),
                mapping: &mapping,
                visible_start: origin,
                visible_end: origin + 96_000,
                bounds: extent,
                scale: 2.0,
                grid: None,
                columns: 19,
            };
            let repeated = cached_waveform_geometry(&cache, &source, &request)
                .unwrap()
                .0;
            assert!(
                Arc::ptr_eq(&original, &repeated),
                "visible clip geometry was rebuilt at {origin}"
            );
        }
        assert_eq!(cache.borrow().entries.len(), 350);
        assert_eq!(cache.borrow().columns, 350 * 19);
    }

    #[cfg(all(test, not(feature = "ui-test-support")))]
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
        assert!(entry_bounded.get(&key(0, extent), &source).is_none());
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
        assert!(column_bounded.get(&key(0, extent), &source).is_none());
    }

    #[cfg(all(test, not(feature = "ui-test-support")))]
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
            amplitude_scale: crate::editor::waveform::WaveformAmplitudeScale::from_decibels(false),
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
            .expect("initial cached preparation")
            .0;
        let cached_started = Instant::now();
        for _ in 0..rounds {
            let geometry = cached_waveform_geometry(&cache, &source, &request)
                .expect("warm cache hit")
                .0;
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
