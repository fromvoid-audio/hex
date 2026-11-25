//! Bounded retained waveform paths for the visible editor working set.
use gpui::{Bounds, Pixels, Point, point, px};
use hex_project::Clip;
use hex_session::CachedOverrange as Overrange;
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
use super::{
    waveform_frame, waveform_overrange,
    waveform_prepared_fade::{PreparedFade, PreparedMusicalFade},
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
const MAX_PENDING_OVERRANGE: usize = 8;

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

/// UI-owned O(1) indexed LRU of ready-to-paint waveform geometry. Entries retain
/// their source Arc, so pointer identity cannot be reused while an entry is resident.
#[derive(Default)]
pub(super) struct WaveformGeometryCache {
    entries: GeometryLru,
    columns: usize,
    coarse_entry_counts: HashMap<CoarseEntryKey, usize>,
    overrange: OverrangeCache,
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
    PreparedFade(PreparedFade),
    PreparedMusicalFade(Arc<PreparedMusicalFade>),
    Frame(Arc<waveform_frame::Key>, waveform_frame::Clock),
    MusicalEventFade(
        Arc<super::waveform_musical_event::Projection>,
        Option<hex_project::AudioEvent>,
    ),
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
        gain: u32,
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
        gain: u32,
        fade: Option<hex_project::ClipFade>,
    },
    Source {
        timeline_origin: u64,
    },
}

pub(super) struct WaveformGeometry {
    display: hex_widgets::WaveformGeometry,
    #[cfg(test)]
    pub(super) overrange: WaveformOverrange,
    overrange_input: WaveformOverrange,
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
    pub(super) outline_fingerprint: [u64; 2],
    pub(super) overrange_ready: bool,
}

#[derive(Clone)]
pub(super) enum WaveformMapping {
    PreparedFade(PreparedFade),
    PreparedMusicalFade(Arc<PreparedMusicalFade>),
    Frame {
        projection: Arc<waveform_frame::Projection>,
        clock: waveform_frame::Clock,
        fallback: Option<Arc<WaveformWindow>>,
    },
    MusicalEventFade {
        projection: Arc<super::waveform_musical_event::Projection>,
        clock: super::waveform_musical_event::Clock,
        fallback: Option<Arc<WaveformWindow>>,
    },
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
        gain: f32,
        detail: bool,
    },
    Clip {
        clip: Clip,
        timeline_origin: u64,
        /// Held or pending display gain. The source peaks are unchanged.
        gain: Option<f32>,
        /// Arrangement fade to display, including a held or pending preview.
        /// Source-editing waveforms may omit this independent clip envelope.
        fade: Option<hex_project::ClipFade>,
    },
    Source {
        timeline_origin: u64,
    },
}

impl WaveformMapping {
    fn can_prove_overrange(&self) -> bool {
        match self {
            Self::Frame { projection, .. } => projection.has_exact_pcm(),
            Self::Clip { fade: Some(_), .. } | Self::PreparedFade(_) => true,
            _ => false,
        }
    }
}

/// A waveform with its own visible coordinates. Events can have different
/// origins in the prepared root and original source.
pub(super) struct WaveformWindow {
    pub(super) waveform: Arc<CachedWaveform>,
    pub(super) mapping: WaveformMapping,
    pub(super) visible: std::ops::Range<u64>,
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

mod builder;
mod cache;
mod lru;
mod overrange;

#[cfg(test)]
pub(super) use builder::prepare_waveform_geometry;
pub(super) use builder::prepare_waveform_geometry_with_scale;
use builder::*;
pub(super) use cache::cached_waveform_geometry;
#[cfg(all(test, feature = "ui-test-support"))]
pub(super) use lru::painted_clip_waveform_outline;
use lru::*;
pub(super) use overrange::WaveformOverrange;
use overrange::*;

#[cfg(test)]
mod tests;
