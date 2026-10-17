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
    grid: Option<[u64; 5]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum WaveformMappingKey {
    Stretch(Arc<super::waveform_stretch::StretchProjection>),
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
    columns: usize,
}

pub(super) struct WaveformChannelGeometry {
    pub(super) centerline: Path<Pixels>,
    pub(super) waveform: Path<Pixels>,
    #[cfg(test)]
    outline: Vec<gpui::Point<Pixels>>,
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) struct RenderedWaveformSample {
    pub(super) source: Arc<CachedWaveform>,
    pub(super) prepared: Option<(u64, u64)>,
    pub(super) spans: [f32; 2],
