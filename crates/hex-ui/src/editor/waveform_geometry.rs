//! Bounded retained waveform paths for the visible editor working set.
use gpui::{Bounds, Path, PathBuilder, Pixels, point, px};
use hex_project::Clip;
use hex_session::{CachedPeak, CachedWaveform};
#[cfg(test)]
use std::hint::black_box;
use std::{collections::VecDeque, sync::Arc};

use super::waveform::{
    aggregate_peaks, aggregate_ranges, fraction_u64, peak_vertical_bounds,
    timeline_source_ranges_at,
};

const MAX_CACHED_WAVEFORM_ENTRIES: usize = 128;
const MAX_CACHED_WAVEFORM_COLUMNS: usize = 65_536;

/// UI-owned LRU of ready-to-paint waveform geometry. Entries retain their source
/// Arc, so pointer identity cannot be reused while an entry is resident.
#[derive(Default)]
pub(super) struct WaveformGeometryCache {
    entries: VecDeque<WaveformGeometryEntry>,
    columns: usize,
}

struct WaveformGeometryEntry {
    key: WaveformGeometryKey,
    source: Arc<CachedWaveform>,
    geometry: Arc<WaveformGeometry>,
    columns: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WaveformGeometryKey {
    mapping: WaveformMappingKey,
    visible_start: u64,
    visible_end: u64,
    origin_x: u32,
    origin_y: u32,
    width: u32,
    height: u32,
    scale: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WaveformMappingKey {
    Prepared {
        timeline_origin: u64,
        root_offset: u64,
        gain: i32,
    },
    GainedSource {
        timeline_origin: u64,
        gain: i32,
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
    pub(super) channels: [WaveformChannelGeometry; 2],
}

pub(super) struct WaveformChannelGeometry {
    pub(super) centerline: Path<Pixels>,
    pub(super) waveform: Path<Pixels>,
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
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

#[derive(Clone)]
pub(super) enum WaveformMapping {
    Prepared {
        timeline_origin: u64,
        root_offset: u64,
        gain: hex_project::ClipGain,
    },
    GainedSource {
        timeline_origin: u64,
        gain: hex_project::ClipGain,
    },
    Clip {
        clip: Clip,
        timeline_origin: u64,
    },
    Source {
        timeline_origin: u64,
    },
}

pub(super) struct WaveformGeometryRequest<'a> {
    pub(super) mapping: &'a WaveformMapping,
    pub(super) visible_start: u64,
    pub(super) visible_end: u64,
    pub(super) bounds: Bounds<Pixels>,
    pub(super) scale: f32,
    pub(super) columns: usize,
}

pub(super) fn cached_waveform_geometry(
    cache: &std::cell::RefCell<WaveformGeometryCache>,
    waveform: &Arc<CachedWaveform>,
    request: &WaveformGeometryRequest<'_>,
) -> Option<Arc<WaveformGeometry>> {
    let key = waveform_geometry_key(
        request.mapping,
        request.visible_start,
        request.visible_end,
        request.bounds,
        request.scale,
    );
    if let Some(geometry) = cache
        .try_borrow_mut()
        .ok()
        .and_then(|mut cache| cache.get(key, waveform))
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
    )?);
    if let Ok(mut cache) = cache.try_borrow_mut() {
        cache.insert(
            key,
            Arc::clone(waveform),
            Arc::clone(&geometry),
            request.columns,
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
        } => WaveformMappingKey::GainedSource {
            timeline_origin: *timeline_origin,
            gain: gain.millidb(),
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
    }
}

pub(super) fn prepare_waveform_geometry(
    waveform: &CachedWaveform,
    mapping: &WaveformMapping,
    visible_start: u64,
    visible_end: u64,
    bounds: Bounds<Pixels>,
    columns: usize,
) -> Option<WaveformGeometry> {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let frames = visible_end.saturating_sub(visible_start);
    let gain = match mapping {
        WaveformMapping::Clip { clip, .. } => clip.gain().linear(),
        WaveformMapping::GainedSource { gain, .. } | WaveformMapping::Prepared { gain, .. } => {
            gain.linear()
        }
        WaveformMapping::Source { .. } => 1.0,
    };
    let mut stereo_peaks = Vec::with_capacity(columns);
    for column in 0..columns {
        let start = visible_start.saturating_add(fraction_u64(frames, column, columns));
        let end = visible_start
            .saturating_add(fraction_u64(frames, column + 1, columns))
            .max(start.saturating_add(1));
        let peaks = match mapping {
            WaveformMapping::Prepared {
                timeline_origin,
                root_offset,
                ..
            } => start
                .checked_sub(*timeline_origin)
                .and_then(|start| start.checked_add(*root_offset))
                .zip(
                    end.checked_sub(*timeline_origin)
                        .and_then(|end| end.checked_add(*root_offset)),
                )
                .and_then(|(start, end)| usize::try_from(start).ok().zip(usize::try_from(end).ok()))
                .and_then(|(start, end)| aggregate_peaks(waveform, start, end)),
            WaveformMapping::Clip {
                clip,
                timeline_origin,
            } => {
                let ranges = timeline_source_ranges_at(
                    clip,
                    *timeline_origin,
                    start..end,
                    waveform.source_frames,
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
                .and_then(|(start, end)| aggregate_peaks(waveform, start, end)),
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

    let mut channels = Vec::with_capacity(2);
    for channel in 0..2 {
        let band_top = height * channel as f32 * 0.5;
        let band_height = height * 0.5;
        let center = band_top + band_height * 0.5;
        let amplitude = (band_height * 0.44).max(0.5);
        let mut maxima = Vec::with_capacity(columns);
        let mut minima = Vec::with_capacity(columns);
        for (column, peaks) in stereo_peaks.iter().enumerate() {
            let x = (column as f32 + 0.5) * width / columns as f32;
            let (top, bottom) = peak_vertical_bounds(peaks[channel], center, amplitude);
            maxima.push(point(bounds.origin.x + px(x), bounds.origin.y + px(top)));
            minima.push(point(bounds.origin.x + px(x), bounds.origin.y + px(bottom)));
        }

        let mut centerline = PathBuilder::stroke(px(0.5));
        centerline.move_to(point(bounds.origin.x, bounds.origin.y + px(center)));
        centerline.line_to(point(
            bounds.origin.x + bounds.size.width,
            bounds.origin.y + px(center),
        ));
        let centerline = centerline.build().ok()?;

        let has_area = maxima
            .iter()
            .zip(&minima)
            .any(|(maximum, minimum)| (f32::from(maximum.y - minimum.y)).abs() >= 0.5);
        let waveform = if has_area && let Some(first) = maxima.first().copied() {
            let mut silhouette = PathBuilder::fill();
            silhouette.move_to(first);
            for point in maxima.iter().skip(1) {
                silhouette.line_to(*point);
            }
            for point in minima.iter().rev() {
                silhouette.line_to(*point);
            }
            silhouette.close();
            silhouette.build().ok()?
        } else if let Some(first) = maxima.first().copied() {
            // A constant sample has zero fill area; stroke its real level so
            // deep zoom never turns a non-silent signal into empty space.
            let mut trace = PathBuilder::stroke(px(1.0));
            trace.move_to(first);
            for point in maxima.iter().skip(1) {
                trace.line_to(*point);
            }
            trace.build().ok()?
        } else {
            Path::new(point(bounds.origin.x, bounds.origin.y + px(center)))
        };
        channels.push(WaveformChannelGeometry {
            centerline,
            waveform,
        });
    }
    Some(WaveformGeometry {
        channels: channels.try_into().ok()?,
    })
}

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
            )
            .expect("waveform geometry"),
        )
    }

    #[test]
    fn geometry_cache_reuses_only_matching_source_mapping_and_bounds() {
        let source = waveform();
        let extent = bounds();
        let cache_key = key(0, extent);
        let geometry = geometry(&source, 0, extent);
        let mut cache = WaveformGeometryCache::default();
        cache.insert(cache_key, Arc::clone(&source), Arc::clone(&geometry), 32);
        assert!(Arc::ptr_eq(
            &cache.get(cache_key, &source).expect("cache hit"),
            &geometry,
        ));
        assert!(
            Arc::strong_count(&source) >= 2,
            "cache retains source identity"
        );

        let resized = Bounds::new(extent.origin, size(px(1201.0), extent.size.height));
        assert!(cache.get(key(0, resized), &source).is_none());
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
        };
        let quiet = WaveformMapping::GainedSource {
            timeline_origin: 0,
            gain: hex_project::ClipGain::new(-6_000).unwrap(),
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
                    columns: 32,
                },
            )
            .unwrap()
        };
        let first = geometry(&neutral);
        let changed = geometry(&quiet);
        assert!(!Arc::ptr_eq(&first, &changed));
        assert!(Arc::ptr_eq(&first, &geometry(&neutral)));
        assert_eq!(cache.borrow().entries.len(), 2);
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
        let visible_end = source.source_frames as u64;
        let columns = waveform_column_count(f32::from(extent.size.width), 2.0);
        let cache = RefCell::new(WaveformGeometryCache::default());
        let request = WaveformGeometryRequest {
            mapping: &mapping,
            visible_start,
            visible_end,
            bounds: extent,
            scale: 2.0,
            columns,
        };
        let rounds = 20;

        let sampled = aggregate_peaks(&source, 0, source.source_frames)
            .expect("fixture extrema survive cache preparation");
        assert!(sampled.iter().any(|peak| peak.min < -0.9 && peak.max > 0.7));
        let digest = source.bins.iter().step_by(127).fold(0_u64, |digest, pair| {
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
