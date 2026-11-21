//! View-only project analyzer with Prism-inspired calibrated display,
//! interactive crosshair/note readout, display-side slope/smoothing/range/peak
//! controls, freeze capture, and an isolated canvas entity so 60 Hz spectrum
//! animation and crosshair movement repaint without invalidating `EditorView`.
//! Canonical selection/color stay in the project; audio owns real capture,
//! freshness, worker storage and retirement.
use super::EditorView;
use crate::theme;
use gpui::{
    Bounds, Context, CursorStyle, Entity, Hsla, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, SharedString, WeakEntity,
    Window, canvas, div, prelude::*, px, rgb,
};
use hex_audio::spectrum::{
    FLOOR_DBFS, SPECTRUM_BANDS, SpectrumFrame, SpectrumSettings, band_center_hz,
};
use hex_audio::spectrum_worker::PublishedProjectSpectrum;
use hex_widgets::{
    ControlStyle, SpectrumPlotConfig, SpectrumPlotCrosshair, SpectrumPlotLayer, SpectrumPlotLayers,
    SpectrumPlotLegendItem, SplitterAxis, SplitterEvent, SplitterGesture, SplitterHandleConfig,
    SplitterInput, format_hz_and_note, layered_spectrum_plot, layered_spectrum_plot_with_config,
    spectrum_plot_bounds,
};
use std::{cell::Cell, rc::Rc, sync::Arc};

const MIN_PLOT_HZ: f32 = 20.0;
const SLOPE_PIVOT_HZ: f32 = 1_000.0;
const KERNEL_SLOPE_DB_PER_OCT: f32 = SpectrumSettings::DEFAULT.slope_db_per_octave;
const MAX_LEGEND_TRACKS: usize = 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SlopePreset {
    Flat,
    Pink,
    #[default]
    Prism,
    Steep,
}

impl SlopePreset {
    fn db_per_octave(self) -> f32 {
        match self {
            Self::Flat => 0.0,
            Self::Pink => 3.0,
            Self::Prism => 4.5,
            Self::Steep => 6.0,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Flat => "0 db",
            Self::Pink => "3 db",
            Self::Prism => "4.5 db",
            Self::Steep => "6 db",
        }
    }
    fn next(self) -> Self {
        match self {
            Self::Flat => Self::Pink,
            Self::Pink => Self::Prism,
            Self::Prism => Self::Steep,
            Self::Steep => Self::Flat,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SmoothingPreset {
    #[default]
    Human,
    Fine,
    Sixth,
    Third,
}

impl SmoothingPreset {
    fn label(self) -> &'static str {
        match self {
            Self::Human => "erb",
            Self::Fine => "1/12",
            Self::Sixth => "1/6",
            Self::Third => "1/3",
        }
    }
    fn next(self) -> Self {
        match self {
            Self::Human => Self::Fine,
            Self::Fine => Self::Sixth,
            Self::Sixth => Self::Third,
            Self::Third => Self::Human,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum RangePreset {
    Tight60,
    #[default]
    Mix78,
    Wide96,
}

impl RangePreset {
    pub(super) fn bounds_dbfs(self) -> (f32, f32) {
        match self {
            Self::Tight60 => (-54.0, 6.0),
            Self::Mix78 => (-72.0, 6.0),
            Self::Wide96 => (-96.0, 0.0),
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Tight60 => "60 db",
            Self::Mix78 => "78 db",
            Self::Wide96 => "96 db",
        }
    }
    fn next(self) -> Self {
        match self {
            Self::Tight60 => Self::Mix78,
            Self::Mix78 => Self::Wide96,
            Self::Wide96 => Self::Tight60,
        }
    }
    fn zoom_in(self) -> Self {
        match self {
            Self::Wide96 => Self::Mix78,
            Self::Mix78 | Self::Tight60 => Self::Tight60,
        }
    }
    fn zoom_out(self) -> Self {
        match self {
            Self::Tight60 => Self::Mix78,
            Self::Mix78 | Self::Wide96 => Self::Wide96,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum PeakMode {
    #[default]
    Decay,
    Hold,
    Off,
}

impl PeakMode {
    fn label(self) -> &'static str {
        match self {
            Self::Decay => "pk",
            Self::Hold => "max",
            Self::Off => "off",
        }
    }
    fn next(self) -> Self {
        match self {
            Self::Decay => Self::Hold,
            Self::Hold => Self::Off,
            Self::Off => Self::Decay,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DisplayOptions {
    slope: SlopePreset,
    smoothing: SmoothingPreset,
    range: RangePreset,
    peak_mode: PeakMode,
    frozen: bool,
}

pub(super) struct ProjectSpectrumState {
    pub(super) expanded: bool,
    height: f32,
    pub(super) detail_height: Option<f32>,
    resize: Option<Resize>,
    result: Option<PublishedProjectSpectrum>,
    output_identity: Option<u64>,
    plot_cache: Option<PlotCache>,
    options: DisplayOptions,
    max_peak_dbfs: Option<[f32; SPECTRUM_BANDS]>,
    max_peak_epoch: u64,
    canvas: Option<Entity<ProjectSpectrumCanvas>>,
    tick_activity: bool,
}

impl ProjectSpectrumState {
    pub(super) fn resize_is_active(&self) -> bool {
        self.resize.is_some()
    }
    pub(super) fn take_tick_activity(&mut self) -> bool {
        std::mem::take(&mut self.tick_activity)
    }
    fn clear_max_peak(&mut self) {
        if self.max_peak_dbfs.take().is_some() {
            self.max_peak_epoch = self.max_peak_epoch.wrapping_add(1);
            self.plot_cache = None;
        }
    }
}

impl Default for ProjectSpectrumState {
    fn default() -> Self {
        Self {
            expanded: true,
            height: 120.0,
            detail_height: None,
            resize: None,
            result: None,
            output_identity: None,
            plot_cache: None,
            options: DisplayOptions::default(),
            max_peak_dbfs: None,
            max_peak_epoch: 0,
            canvas: None,
            tick_activity: false,
        }
    }
}

struct PlotCache {
    generation: u64,
    revision: u64,
    theme: hex_preferences::ThemePreferences,
    options: DisplayOptions,
    max_peak_epoch: u64,
    source: (
        std::time::Instant,
        std::time::Instant,
        u64,
        hex_audio::spectrum_worker::ProjectSelection,
    ),
    layers: SpectrumPlotLayers,
    legend: Vec<SpectrumPlotLegendItem>,
}

struct Resize {
    gesture: SplitterGesture,
    preferred: f32,
    original: f32,
    generation: u64,
}

/// Isolated live spectrum canvas entity so 60 Hz spectrum frames and 120 Hz
/// crosshair hover repaint only this small panel instead of `EditorView`.
pub(super) struct ProjectSpectrumCanvas {
    editor: WeakEntity<EditorView>,
    layers: SpectrumPlotLayers,
    config: SpectrumPlotConfig,
    style: ControlStyle,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    hover_pos: Option<Point<Pixels>>,
}

impl ProjectSpectrumCanvas {
    fn new(editor: WeakEntity<EditorView>, style: ControlStyle) -> Self {
        Self {
            editor,
            layers: SpectrumPlotLayers::default(),
            config: SpectrumPlotConfig {
                show_axes: true,
                peak_ribbon: true,
                ..Default::default()
            },
            style,
            bounds: Rc::new(Cell::new(None)),
            hover_pos: None,
        }
    }

    fn sync(
        &mut self,
        layers: SpectrumPlotLayers,
        config: SpectrumPlotConfig,
        style: ControlStyle,
    ) -> bool {
        let same_layers = self.layers.len() == layers.len()
            && std::ptr::eq(self.layers.as_ptr(), layers.as_ptr());
        if same_layers && self.config == config && self.style == style {
            return false;
        }
        self.layers = layers;
        self.config = config;
        self.style = style;
        true
    }

    fn build_crosshair(&self) -> Option<SpectrumPlotCrosshair> {
        let pos = self.hover_pos?;
        let bounds = self.bounds.get()?;
        let plot = spectrum_plot_bounds(bounds, self.config.show_axes);
        let plot_w = f32::from(plot.size.width);
        let plot_h = f32::from(plot.size.height);
        if plot_w <= 0.0 || plot_h <= 0.0 {
            return None;
        }
        let rel_x = f32::from(pos.x - plot.left());
        let rel_y = f32::from(pos.y - plot.top());
        if rel_x < 0.0 || rel_x > plot_w || rel_y < 0.0 || rel_y > plot_h {
            return None;
        }
        let norm_x = (rel_x / plot_w).clamp(0.0, 1.0);
        let norm_y = (1.0 - rel_y / plot_h).clamp(0.0, 1.0);
        let min_hz = self.config.min_hz.max(1.0);
        let max_hz = self.config.max_hz.max(min_hz + 1.0);
        let hz = min_hz * (max_hz / min_hz).powf(norm_x);
        let (freq_text, note_text) = format_hz_and_note(hz)?;
        let primary_label: SharedString = format!("{freq_text} · {note_text}").into();

        let min_db = self.config.min_dbfs;
        let max_db = self.config.max_dbfs;
        let span_db = max_db - min_db;
        let cursor_db = min_db + norm_y * span_db;

        let primary_layer = self.layers.last();
        let trace_y = primary_layer
            .map(|layer| sample_normalized_curve(&layer.points, norm_x))
            .filter(|v| v.is_finite() && *v > 0.0);
        let peak_y = primary_layer
            .and_then(|layer| layer.peak.as_deref())
            .map(|peak| sample_normalized_curve(peak, norm_x))
            .filter(|v| v.is_finite() && *v > 0.0);

        let secondary_label: Option<SharedString> = match (trace_y, peak_y) {
            (Some(ty), Some(py)) if py > ty + 0.008 => {
                let live_db = min_db + ty * span_db;
                let pk_db = min_db + py * span_db;
                Some(format!("{live_db:.1} db · pk {pk_db:.1} db").into())
            }
            (Some(ty), _) => {
                let live_db = min_db + ty * span_db;
                Some(format!("{live_db:.1} db").into())
            }
            (None, Some(py)) => {
                let pk_db = min_db + py * span_db;
                Some(format!("{cursor_db:.1} db · pk {pk_db:.1} db").into())
            }
            (None, None) => Some(format!("{cursor_db:.1} db").into()),
        };

        let dot_color = primary_layer.map_or(self.style.accent, |layer| layer.color);
        Some(SpectrumPlotCrosshair {
            x: norm_x,
            y: norm_y,
            trace_y,
            peak_y: peak_y.filter(|py| trace_y.is_none_or(|ty| *py > ty + 0.015)),
            color: dot_color,
            primary_label,
            secondary_label,
        })
    }
}

impl Render for ProjectSpectrumCanvas {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut config = self.config.clone();
        config.crosshair = self.build_crosshair();
        let bounds_cell = Rc::clone(&self.bounds);
        let plot = layered_spectrum_plot_with_config(self.layers.clone(), config, self.style);
        let editor_click = self.editor.clone();
        let editor_right = self.editor.clone();
        let editor_wheel = self.editor.clone();

        div()
            .id("project-spectrum-canvas")
            .relative()
            .size_full()
            .cursor(CursorStyle::Crosshair)
            .child(plot)
            .child(
                canvas(
                    move |bounds, _, _| {
                        bounds_cell.set(Some(bounds));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.hover_pos != Some(event.position) {
                    this.hover_pos = Some(event.position);
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered && this.hover_pos.take().is_some() {
                    cx.notify();
                }
            }))
            .on_mouse_down(MouseButton::Left, move |_: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                let _ = editor_click.update(cx, |view, cx| {
                    view.toggle_project_spectrum_freeze(cx);
                });
            })
            .on_mouse_down(MouseButton::Right, move |_: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                let _ = editor_right.update(cx, |view, cx| {
                    view.reset_project_spectrum_peaks(cx);
                });
            })
            .on_scroll_wheel(move |event: &ScrollWheelEvent, _, cx| {
                let dy = match event.delta {
                    ScrollDelta::Lines(delta) => delta.y,
                    ScrollDelta::Pixels(delta) => f32::from(delta.y) / 16.0,
                };
                if dy.abs() < 0.05 {
                    return;
                }
                cx.stop_propagation();
                let _ = editor_wheel.update(cx, |view, cx| {
                    view.zoom_project_spectrum_range(dy > 0.0, cx);
                });
            })
    }
}

fn sample_normalized_curve(points: &[[f32; 2]], x: f32) -> f32 {
    if points.is_empty() {
        return 0.0;
    }
    if x <= points[0][0] {
        return points[0][1];
    }
    let last = points.len() - 1;
    if x >= points[last][0] {
        return points[last][1];
    }
    let idx = points.partition_point(|p| p[0] < x);
    if idx == 0 {
        return points[0][1];
    }
    let a = points[idx - 1];
    let b = points[idx];
    let dx = b[0] - a[0];
    if dx <= f32::EPSILON {
        b[1]
    } else {
        let t = ((x - a[0]) / dx).clamp(0.0, 1.0);
        a[1] + (b[1] - a[1]) * t
    }
}

/// Apply display-side slope tilt (around 1 kHz) and optional power-domain
/// octave / ERB psychoacoustic smoothing over the 192 calibrated log-spaced bands.
fn process_display_bands(
    raw_dbfs: &[f32; SPECTRUM_BANDS],
    sample_rate_hz: u32,
    slope: SlopePreset,
    smoothing: SmoothingPreset,
) -> [f32; SPECTRUM_BANDS] {
    let slope_delta = slope.db_per_octave() - KERNEL_SLOPE_DB_PER_OCT;
    let mut tilted = [FLOOR_DBFS; SPECTRUM_BANDS];
    let mut centers = [MIN_PLOT_HZ; SPECTRUM_BANDS];
    for i in 0..SPECTRUM_BANDS {
        let hz = band_center_hz(sample_rate_hz, i).unwrap_or(MIN_PLOT_HZ);
        centers[i] = hz;
        let db = raw_dbfs[i];
        if db <= FLOOR_DBFS + 0.01 || !db.is_finite() {
            tilted[i] = FLOOR_DBFS;
        } else if slope_delta.abs() < 1e-4 {
            tilted[i] = db;
        } else {
            let octaves = (hz / SLOPE_PIVOT_HZ).log2();
            tilted[i] = (db + slope_delta * octaves).max(FLOOR_DBFS);
        }
    }
    if smoothing == SmoothingPreset::Fine {
        return tilted;
    }

    // Smooth power-domain Gaussian smoothing (avoiding rectangular boxcar flat
    // tops) followed by a gentle dB-domain corner polish so peaks roll smoothly
    // into neighboring troughs without sharp V-notches or cliffs.
    let max_hz = 20_000.0_f32.min(sample_rate_hz as f32 * 0.49);
    let total_octaves = (max_hz / MIN_PLOT_HZ).log2().max(1.0);
    let bands_per_octave = SPECTRUM_BANDS as f32 / total_octaves;
    let mut power = [0.0_f64; SPECTRUM_BANDS];
    let mut any_signal = false;
    for i in 0..SPECTRUM_BANDS {
        if tilted[i] > FLOOR_DBFS + 0.01 {
            power[i] = 10.0_f64.powf(f64::from(tilted[i]) * 0.1);
            any_signal = true;
        }
    }
    if !any_signal {
        return [FLOOR_DBFS; SPECTRUM_BANDS];
    }

    let mut gaussian_db = [FLOOR_DBFS; SPECTRUM_BANDS];
    for i in 0..SPECTRUM_BANDS {
        let oct_width = match smoothing {
            SmoothingPreset::Fine => 0.0,
            SmoothingPreset::Sixth => 1.0 / 6.0,
            SmoothingPreset::Third => 1.0 / 3.0,
            SmoothingPreset::Human => {
                // Prism-inspired ERB/Bark-like perceptual curve: keep sub-bass
                // tight (<100 Hz) and widen smoothly through mids and highs.
                let hz = centers[i];
                let t =
                    ((hz / 100.0).max(1.0).log2() / (8_000.0_f32 / 100.0).log2()).clamp(0.0, 1.0);
                (1.0 / 14.0) + t * (1.0 / 3.0 - 1.0 / 14.0)
            }
        };
        // Full-width at half-maximum ≈ oct_width => sigma ≈ oct_width / 2.355.
        let sigma = (oct_width * bands_per_octave / 2.355).max(0.65);
        let radius = ((sigma * 3.5).ceil() as usize).clamp(2, 18);
        let inv_two_sigma2 = 0.5 / (f64::from(sigma) * f64::from(sigma));
        let lo = i.saturating_sub(radius);
        let hi = (i + radius).min(SPECTRUM_BANDS - 1);
        let mut weighted_sum = 0.0_f64;
        let mut weight_total = 0.0_f64;
        for (j, &pj) in power[lo..=hi].iter().enumerate() {
            let d = (lo + j) as f64 - i as f64;
            let w = (-d * d * inv_two_sigma2).exp();
            weighted_sum += pj * w;
            weight_total += w;
        }
        let mean_power = if weight_total > 0.0 {
            weighted_sum / weight_total
        } else {
            0.0
        };
        if mean_power > 1.0e-12 {
            gaussian_db[i] = (10.0 * mean_power.log10() as f32).max(FLOOR_DBFS);
        }
    }

    // Gentle 5-tap dB-domain corner polish: preserves flat levels exactly while
    // rounding sharp shoulder transitions where peaks meet troughs.
    const POLISH: [f32; 5] = [0.08, 0.22, 0.40, 0.22, 0.08];
    let mut out = [FLOOR_DBFS; SPECTRUM_BANDS];
    for i in 0..SPECTRUM_BANDS {
        let mut sum = 0.0_f32;
        let mut w_sum = 0.0_f32;
        for (tap, &w) in POLISH.iter().enumerate() {
            let offset = tap as isize - 2;
            let idx = (i as isize + offset).clamp(0, (SPECTRUM_BANDS - 1) as isize) as usize;
            let v = gaussian_db[idx];
            if v > FLOOR_DBFS + 0.01 {
                sum += v * w;
                w_sum += w;
            }
        }
        if w_sum > 0.0 && gaussian_db[i] > FLOOR_DBFS + 0.01 {
            out[i] = (sum / w_sum).max(FLOOR_DBFS);
        }
    }
    out
}

fn map_bands_to_curve(
    bands_dbfs: &[f32; SPECTRUM_BANDS],
    sample_rate_hz: u32,
    range: RangePreset,
) -> Vec<[f32; 2]> {
    let upper = 20_000.0_f32.min(sample_rate_hz as f32 * 0.5);
    let span_x = (upper / MIN_PLOT_HZ).ln();
    let (min_db, max_db) = range.bounds_dbfs();
    let span_y = max_db - min_db;
    let mut curve = Vec::with_capacity(SPECTRUM_BANDS);
    for (index, &dbfs) in bands_dbfs.iter().enumerate() {
        let Some(hz) = band_center_hz(sample_rate_hz, index) else {
            continue;
        };
        let x = (hz / MIN_PLOT_HZ).ln() / span_x;
        if x.is_finite() {
            let y = ((dbfs - min_db) / span_y).clamp(0.0, 1.0);
            curve.push([x.clamp(0.0, 1.0), y]);
        }
    }
    curve
}

fn truncate_label(name: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in name.chars().enumerate() {
        if idx >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

fn bounded_height(requested: f32, viewport: gpui::Size<Pixels>) -> f32 {
    // Supported minimum editor is 540px. Reserve toolbar/status, detail/mixer/
    // routing rows and usable workspace; the graph itself retains 72px or more.
    let extra_toolbar =
        super::header::toolbar_height(f32::from(viewport.width)) - super::header::TOOLBAR_HEIGHT;
    let panel_chrome =
        2.0 * (2.0 * hex_widgets::PANEL_FRAME_INSET + super::workspace_view::PANEL_GUTTER);
    let max =
        (f32::from(viewport.height) - 394.0 - panel_chrome - extra_toolbar).clamp(72.0, 300.0);
    if requested.is_finite() {
        requested.clamp(72.0, max)
    } else {
        120.0_f32.min(max)
    }
}

impl EditorView {
    fn project_spectrum_chrome(&self, window: &Window) -> f32 {
        let returns = self
            .state
            .as_ref()
            .map_or(0, |s| s.project.return_tracks().len());
        // Responsive transport, status22, arrangement header42,
        // main track44 and mixer26. Routing owns its own height.
        134.0
            + super::header::toolbar_height(f32::from(window.viewport_size().width))
            + self.workspace_panel_chrome()
            + if self.clip_view_open && !self.device_panel_active {
                4.0
            } else {
                0.0
            }
            + super::return_tracks::section_height(returns)
    }

    fn project_spectrum_compact(&self, window: &Window) -> bool {
        self.project_spectrum.expanded
            && self.project_spectrum.detail_height.is_some_and(|detail| {
                f32::from(window.viewport_size().height)
                    - self.project_spectrum_chrome(window)
                    - detail
                    - 60.0
                    < 72.0
            })
    }

    fn project_spectrum_height(&self, window: &Window) -> f32 {
        if !self.project_spectrum.expanded || self.project_spectrum_compact(window) {
            return 22.0 + 2.0 * hex_widgets::PANEL_FRAME_INSET;
        }
        let viewport = f32::from(window.viewport_size().height);
        let desired = bounded_height(self.project_spectrum.height, window.viewport_size());
        // Keep complete populated details whenever a useful graph fits. At the
        // minimum window, the detail viewport scrolls instead of crushing tracks.
        self.project_spectrum
            .detail_height
            .map_or(desired, |detail| {
                desired.min(
                    (viewport - self.project_spectrum_chrome(window) - detail - 60.0).max(72.0),
                )
            })
    }

    /// Empty space yields to the timeline; real controls remain scrollable.
    pub(super) fn project_spectrum_detail_height(&self, window: &Window, preferred: f32) -> f32 {
        let viewport = f32::from(window.viewport_size().height);
        let workspace = if self.project_spectrum.detail_height.is_some()
            && (!self.project_spectrum.expanded || self.project_spectrum_compact(window))
        {
            44.0
        } else {
            60.0
        };
        preferred.min(
            (viewport
                - self.project_spectrum_chrome(window)
                - self.project_spectrum_height(window)
                - workspace)
                .max(48.0),
        )
    }

    fn project_spectrum_ids(&self) -> Vec<u64> {
        let mut ids: Vec<_> = self
            .selected_track_ids()
            .iter()
            .map(|id| id.value())
            .collect();
        ids.sort_unstable();
        ids
    }

    fn update_max_peak_from(&mut self, master: &SpectrumFrame) {
        if self.project_spectrum.options.peak_mode != PeakMode::Hold {
            return;
        }
        let peaks = self
            .project_spectrum
            .max_peak_dbfs
            .get_or_insert([FLOOR_DBFS; SPECTRUM_BANDS]);
        let mut changed = false;
        for (slot, &candidate) in peaks.iter_mut().zip(master.peak_dbfs.iter()) {
            if candidate.is_finite() && candidate > *slot {
                *slot = candidate;
                changed = true;
            }
        }
        if changed {
            self.project_spectrum.max_peak_epoch =
                self.project_spectrum.max_peak_epoch.wrapping_add(1);
        }
    }

    pub(super) fn tick_project_spectrum(&mut self, cx: &mut Context<Self>) -> bool {
        let ids = self.project_spectrum_ids();
        let generation = self.project_generation;
        let floor = self.state.as_ref().and_then(|state| {
            self.audio_admission
                .published_audio_revision(generation, state.revision)
        });
        let expanded = self.project_spectrum.expanded
            && !self.opening_project
            && !self.sample_recorder.visible;
        let mut result = None;
        let mut output_identity = None;
        if let Some(audio) = self.audio.as_mut() {
            let desired = audio.control.project_spectrum_desired();
            let same = desired.is_some_and(|s| {
                s.generation == generation
                    && s.tracks() == ids
                    && s.sample_rate_hz == audio.sample_rate
            });
            if !expanded {
                let _ = audio.control.project_spectrum_clear();
            } else {
                if !same {
                    // Clear old identity immediately while a new canonical
                    // revision is still waiting for its real audio publication.
                    let _ = audio.control.project_spectrum_clear();
                    self.project_spectrum.clear_max_peak();
                    if let Some(floor) = floor {
                        let _ = audio.control.project_spectrum_select(
                            &ids,
                            generation,
                            floor,
                            audio.sample_rate,
                        );
                    }
                }
                result = audio.control.project_spectrum_latest().filter(|r| {
                    r.selection.generation == generation
                        && r.selection.tracks() == ids
                        && r.selection.sample_rate_hz == audio.sample_rate
                });
            }
        }
        if result.is_some() {
            output_identity = self
                .audio
                .as_ref()
                .and_then(|a| a.control.output_identity());
        }

        // Preserve frozen snapshot while project/output identity remain valid.
        if self.project_spectrum.options.frozen {
            let context_valid = self.project_spectrum.result.as_ref().is_some_and(|saved| {
                saved.selection.generation == generation
                    && saved.selection.tracks() == ids
                    && (output_identity.is_none()
                        || self.project_spectrum.output_identity == output_identity)
            });
            if context_valid {
                return false;
            }
            self.project_spectrum.options.frozen = false;
        }

        let same_result = match (&self.project_spectrum.result, &result) {
            (Some(a), Some(b)) => {
                Arc::ptr_eq(&a.frames, &b.frames)
                    && a.processed_at == b.processed_at
                    && a.captured_at == b.captured_at
                    && a.selection == b.selection
                    && a.applied_revision == b.applied_revision
            }
            (None, None) => true,
            _ => false,
        };
        if !same_result || self.project_spectrum.output_identity != output_identity {
            self.project_spectrum.plot_cache = None;
            if let Some(master) = result.as_ref().and_then(|r| r.frames().first()) {
                self.update_max_peak_from(master);
            }
            self.project_spectrum.result = result;
            self.project_spectrum.output_identity = output_identity;
            self.project_spectrum.tick_activity = true;
            // Repaint ONLY the isolated spectrum canvas when already mounted,
            // avoiding full-window EditorView::render() at 60 Hz!
            if self.project_spectrum.canvas.is_some() {
                self.sync_project_spectrum_canvas(std::time::Instant::now(), cx);
                return false;
            }
            true
        } else {
            false
        }
    }

    fn sync_project_spectrum_canvas(&mut self, now: std::time::Instant, cx: &mut Context<Self>) {
        let Some(canvas) = self.project_spectrum.canvas.clone() else {
            return;
        };
        let (layers, legend) = self.project_spectrum_layers_and_legend(now);
        let config = self.project_spectrum_config(legend);
        let style = theme::device_style(theme::DeviceStyleRole::Display);
        canvas.update(cx, |view, cx| {
            if view.sync(layers, config, style) {
                cx.notify();
            }
        });
    }

    fn notify_project_spectrum_options(&mut self, cx: &mut Context<Self>) {
        self.project_spectrum.plot_cache = None;
        self.sync_project_spectrum_canvas(std::time::Instant::now(), cx);
        cx.notify();
    }

    fn cycle_project_spectrum_slope(&mut self, cx: &mut Context<Self>) {
        self.project_spectrum.options.slope = self.project_spectrum.options.slope.next();
        self.notify_project_spectrum_options(cx);
    }

    fn cycle_project_spectrum_smoothing(&mut self, cx: &mut Context<Self>) {
        self.project_spectrum.options.smoothing = self.project_spectrum.options.smoothing.next();
        self.notify_project_spectrum_options(cx);
    }

    fn cycle_project_spectrum_range(&mut self, cx: &mut Context<Self>) {
        self.project_spectrum.options.range = self.project_spectrum.options.range.next();
        self.notify_project_spectrum_options(cx);
    }

    fn zoom_project_spectrum_range(&mut self, zoom_in: bool, cx: &mut Context<Self>) {
        let next = if zoom_in {
            self.project_spectrum.options.range.zoom_in()
        } else {
            self.project_spectrum.options.range.zoom_out()
        };
        if next != self.project_spectrum.options.range {
            self.project_spectrum.options.range = next;
            self.notify_project_spectrum_options(cx);
        }
    }

    fn cycle_project_spectrum_peak_mode(&mut self, cx: &mut Context<Self>) {
        let next = self.project_spectrum.options.peak_mode.next();
        self.project_spectrum.options.peak_mode = next;
        if next == PeakMode::Hold {
            self.project_spectrum.max_peak_dbfs = self
                .project_spectrum
                .result
                .as_ref()
                .and_then(|r| r.frames().first())
                .map(|m| m.peak_dbfs);
            self.project_spectrum.max_peak_epoch =
                self.project_spectrum.max_peak_epoch.wrapping_add(1);
        } else {
            self.project_spectrum.clear_max_peak();
        }
        self.notify_project_spectrum_options(cx);
    }

    fn toggle_project_spectrum_freeze(&mut self, cx: &mut Context<Self>) {
        self.project_spectrum.options.frozen = !self.project_spectrum.options.frozen;
        if !self.project_spectrum.options.frozen {
            self.tick_project_spectrum(cx);
        }
        self.notify_project_spectrum_options(cx);
    }

    fn reset_project_spectrum_peaks(&mut self, cx: &mut Context<Self>) {
        self.project_spectrum.clear_max_peak();
        self.notify_project_spectrum_options(cx);
    }

    fn toggle_project_spectrum(&mut self, cx: &mut Context<Self>) {
        self.cancel_project_spectrum_resize(cx);
        self.project_spectrum.expanded = !self.project_spectrum.expanded;
        self.project_spectrum.result = None;
        self.project_spectrum.options.frozen = false;
        self.project_spectrum.clear_max_peak();
        if !self.project_spectrum.expanded
            && let Some(audio) = self.audio.as_ref()
        {
            let _ = audio.control.project_spectrum_clear();
        }
        self.tick_project_spectrum(cx);
        cx.notify();
    }

    pub(super) fn cancel_project_spectrum_resize(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(resize) = self.project_spectrum.resize.take() {
            self.project_spectrum.height = resize.preferred;
            cx.notify();
            true
        } else {
            false
        }
    }

    fn resize_project_spectrum(
        &mut self,
        delta: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(resize) = self.project_spectrum.resize.as_ref() else {
            return false;
        };
        if resize.generation != self.project_generation
            || !resize.gesture.matches_window(window)
            || !self.focus.is_focused(window)
        {
            return self.cancel_project_spectrum_resize(cx);
        }
        self.project_spectrum.height =
            bounded_height(resize.original + delta, window.viewport_size());
        cx.notify();
        true
    }

    pub(super) fn project_spectrum_splitter_input(
        &self,
        cx: &Context<Self>,
    ) -> SplitterInput<Self> {
        SplitterInput::new(
            cx.entity(),
            |view| view.project_spectrum.resize.as_ref().map(|r| r.gesture),
            Self::project_spectrum_splitter_event,
        )
    }

    pub(super) fn project_spectrum_splitter_event(
        &mut self,
        event: SplitterEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match event {
            SplitterEvent::Begin(gesture) => {
                self.cancel_clip_detail_resize(cx);
                window.focus(&self.focus);
                self.project_spectrum.resize = Some(Resize {
                    gesture,
                    // Restore the preference even when responsive layout clamps paint.
                    preferred: self.project_spectrum.height,
                    original: bounded_height(self.project_spectrum.height, window.viewport_size()),
                    generation: self.project_generation,
                });
                cx.notify();
                true
            }
            SplitterEvent::Change(delta) | SplitterEvent::Commit(delta) => {
                if self.project_spectrum.resize.is_none() {
                    return false;
                }
                self.resize_project_spectrum(delta, window, cx);
                if matches!(event, SplitterEvent::Commit(_)) {
                    self.project_spectrum.resize = None;
                    cx.notify();
                }
                true
            }
            SplitterEvent::Cancel(_) => self.cancel_project_spectrum_resize(cx),
        }
    }

    pub(super) fn project_spectrum_pointer_events(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        self.project_spectrum_splitter_input(cx).pointer_layer()
    }

    fn project_spectrum_config(&self, legend: Vec<SpectrumPlotLegendItem>) -> SpectrumPlotConfig {
        let sample_rate = self
            .state
            .as_ref()
            .map_or(48_000, |s| s.project.sample_rate());
        let max_hz = 20_000.0_f32.min(sample_rate as f32 * 0.5);
        let (min_dbfs, max_dbfs) = self.project_spectrum.options.range.bounds_dbfs();
        SpectrumPlotConfig {
            min_hz: MIN_PLOT_HZ,
            max_hz,
            min_dbfs,
            max_dbfs,
            show_axes: true,
            peak_ribbon: self.project_spectrum.options.peak_mode != PeakMode::Off,
            crosshair: None,
            badge: self
                .project_spectrum
                .options
                .frozen
                .then(|| SharedString::from("frozen")),
            legend,
        }
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    fn project_spectrum_layers(&mut self, now: std::time::Instant) -> SpectrumPlotLayers {
        self.project_spectrum_layers_and_legend(now).0
    }

    fn project_spectrum_layers_and_legend(
        &mut self,
        now: std::time::Instant,
    ) -> (SpectrumPlotLayers, Vec<SpectrumPlotLegendItem>) {
        if self.project_spectrum.output_identity
            != self
                .audio
                .as_ref()
                .and_then(|a| a.control.output_identity())
        {
            return (SpectrumPlotLayers::default(), Vec::new());
        }
        if !self.project_spectrum.expanded {
            return (SpectrumPlotLayers::default(), Vec::new());
        }
        let ids = self.project_spectrum_ids();
        let frozen = self.project_spectrum.options.frozen;
        let Some(result) = self.project_spectrum.result.as_ref().filter(|r| {
            r.selection.generation == self.project_generation
                && r.selection.tracks() == ids
                && (frozen || r.is_fresh_at(now))
        }) else {
            return (SpectrumPlotLayers::default(), Vec::new());
        };
        let Some(state) = self.state.as_ref() else {
            return (SpectrumPlotLayers::default(), Vec::new());
        };
        if result.selection.sample_rate_hz != state.project.sample_rate() {
            return (SpectrumPlotLayers::default(), Vec::new());
        }
        let source = (
            result.captured_at,
            result.processed_at,
            result.applied_revision,
            result.selection,
        );
        let palette = theme::preferences();
        let options = self.project_spectrum.options;
        let max_peak_epoch = self.project_spectrum.max_peak_epoch;
        if let Some(cache) = self.project_spectrum.plot_cache.as_ref()
            && cache.generation == self.project_generation
            && cache.source == source
            && cache.revision == state.revision
            && cache.theme == palette
            && cache.options == options
            && cache.max_peak_epoch == max_peak_epoch
        {
            return (cache.layers.clone(), cache.legend.clone());
        }
        // One canonical traversal per revision/theme/result change, with bounded
        // binary lookup. Never a separate full project scan for each layer.
        let mut track_meta: Vec<Option<(u32, SharedString)>> = vec![None; ids.len()];
        for track in state.project.tracks() {
            if let Ok(index) = ids.binary_search(&track.id().value()) {
                track_meta[index] = Some((
                    theme::element_colors(track.color()).accent,
                    theme::text(track.name()),
                ));
            }
        }
        let Some(track_meta) = track_meta.into_iter().collect::<Option<Vec<_>>>() else {
            return (SpectrumPlotLayers::default(), Vec::new());
        };
        let master_color = theme::device_analyzer_style().output;
        let mut colors: Vec<u32> = track_meta.iter().map(|(c, _)| *c).collect();
        colors.push(master_color);

        let mut legend = Vec::new();
        if !track_meta.is_empty() {
            legend.push(SpectrumPlotLegendItem {
                label: "master".into(),
                color: master_color,
            });
            for (color, name) in track_meta.iter().take(MAX_LEGEND_TRACKS) {
                legend.push(SpectrumPlotLegendItem {
                    label: truncate_label(name, 12).into(),
                    color: *color,
                });
            }
            if track_meta.len() > MAX_LEGEND_TRACKS {
                legend.push(SpectrumPlotLegendItem {
                    label: format!("+{}", track_meta.len() - MAX_LEGEND_TRACKS).into(),
                    color: theme::color(theme::PaletteRole::Muted),
                });
            }
        }

        // Unrelated edits update the lookup key without converting points again.
        if let Some(cache) = self.project_spectrum.plot_cache.as_mut()
            && cache.generation == self.project_generation
            && cache.source == source
            && cache.options == options
            && cache.max_peak_epoch == max_peak_epoch
            && cache
                .layers
                .iter()
                .map(|l| l.color)
                .eq(colors.iter().copied())
            && cache.legend == legend
        {
            cache.revision = state.revision;
            cache.theme = palette;
            return (cache.layers.clone(), cache.legend.clone());
        }

        let rate = result.selection.sample_rate_hz;
        let mut layers = Vec::with_capacity(result.selection.layer_count());
        for (index, &(color, _)) in track_meta.iter().enumerate() {
            let frame = &result.frames()[index + 1];
            let bands =
                process_display_bands(&frame.bins_dbfs, rate, options.slope, options.smoothing);
            layers.push(SpectrumPlotLayer {
                points: map_bands_to_curve(&bands, rate, options.range),
                peak: None,
                color,
            });
        }
        // Real master stays contrasting and above every independent track
        // layer, and alone carries a held-peak line to keep dense views legible.
        let master_frame = &result.frames()[0];
        let master_live = process_display_bands(
            &master_frame.bins_dbfs,
            rate,
            options.slope,
            options.smoothing,
        );
        let master_peak = match options.peak_mode {
            PeakMode::Off => None,
            PeakMode::Decay => {
                let bands = process_display_bands(
                    &master_frame.peak_dbfs,
                    rate,
                    options.slope,
                    options.smoothing,
                );
                Some(map_bands_to_curve(&bands, rate, options.range))
            }
            PeakMode::Hold => {
                let raw_hold = self
                    .project_spectrum
                    .max_peak_dbfs
                    .as_ref()
                    .unwrap_or(&master_frame.peak_dbfs);
                let bands = process_display_bands(raw_hold, rate, options.slope, options.smoothing);
                Some(map_bands_to_curve(&bands, rate, options.range))
            }
        };
        layers.push(SpectrumPlotLayer {
            points: map_bands_to_curve(&master_live, rate, options.range),
            peak: master_peak,
            color: master_color,
        });
        let layers = SpectrumPlotLayers::new(layers).unwrap_or_default();
        self.project_spectrum.plot_cache = Some(PlotCache {
            generation: self.project_generation,
            revision: state.revision,
            theme: palette,
            options,
            max_peak_epoch,
            source,
            layers: layers.clone(),
            legend: legend.clone(),
        });
        (layers, legend)
    }

    fn control_button(
        id: &'static str,
        label: &'static str,
        tooltip_desc: &'static str,
        active: bool,
        style: ControlStyle,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let bg: Hsla = if active {
            rgb(style.accent).into()
        } else {
            Hsla::from(rgb(style.raised)).opacity(0.65)
        };
        let fg = if active {
            rgb(style.accent_text)
        } else {
            rgb(style.muted)
        };
        div()
            .id(id)
            .debug_selector(move || id.into())
            .h(px(16.0))
            .px(px(4.0))
            .rounded(px(2.0))
            .bg(bg)
            .text_color(fg)
            .text_size(px(8.5))
            .line_height(px(16.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(move |s| {
                if active {
                    s
                } else {
                    s.bg(rgb(style.hover)).text_color(rgb(style.text))
                }
            })
            .child(theme::text(label))
            .tooltip(move |_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: tooltip_desc.into(),
                })
                .into()
            })
            .on_click(cx.listener(move |view, _, window, cx| {
                cx.stop_propagation();
                on_click(view, window, cx);
            }))
    }

    pub(super) fn project_spectrum_panel(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        if self.project_spectrum.resize.as_ref().is_some_and(|r| {
            r.generation != self.project_generation
                || !r.gesture.matches_window(window)
                || !self.focus.is_focused(window)
        }) {
            self.cancel_project_spectrum_resize(cx);
        }
        let expanded = self.project_spectrum.expanded;
        let height = self.project_spectrum_height(window);
        if self.sample_recorder.visible {
            return self.sample_recorder_panel(height, cx);
        }
        let style = theme::device_style(theme::DeviceStyleRole::Display);
        let compact = self.project_spectrum_compact(window);
        let options = self.project_spectrum.options;
        let show_analyzer_pills =
            expanded && !compact && self.workspace_body_width(window) >= 480.0;

        let mut header = div()
            .debug_selector(|| "project-spectrum-controls".into())
            .h(px(22.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(2.0))
            .px_1();

        if show_analyzer_pills {
            let backdrop: Hsla = rgb(style.background).into();
            let slope_btn = Self::control_button(
                "project-spectrum-slope",
                options.slope.label(),
                "spectral tilt around 1 khz (0 / 3 / 4.5 / 6 db/oct)",
                false,
                style,
                |view, _, cx| view.cycle_project_spectrum_slope(cx),
                cx,
            );
            let smooth_btn = Self::control_button(
                "project-spectrum-smoothing",
                options.smoothing.label(),
                "frequency smoothing (1/12, 1/6, 1/3 oct, or erb auditory)",
                false,
                style,
                |view, _, cx| view.cycle_project_spectrum_smoothing(cx),
                cx,
            );
            let range_btn = Self::control_button(
                "project-spectrum-range",
                options.range.label(),
                "vertical db range (60 / 78 / 96 db; scroll plot to zoom)",
                false,
                style,
                |view, _, cx| view.cycle_project_spectrum_range(cx),
                cx,
            );
            let peak_btn = Self::control_button(
                "project-spectrum-peak",
                options.peak_mode.label(),
                "peak trace (decay / max hold / off; right-click plot resets)",
                options.peak_mode == PeakMode::Hold,
                style,
                |view, _, cx| view.cycle_project_spectrum_peak_mode(cx),
                cx,
            );
            let freeze_btn = Self::control_button(
                "project-spectrum-freeze",
                "frz",
                "freeze spectrum snapshot (or click plot)",
                options.frozen,
                style,
                |view, _, cx| view.toggle_project_spectrum_freeze(cx),
                cx,
            );
            let controls = div()
                .flex()
                .items_center()
                .gap(px(2.0))
                .children([slope_btn, smooth_btn, range_btn, peak_btn, freeze_btn]);
            header = header
                .bg(backdrop.opacity(0.78))
                .rounded_bl(px(3.0))
                .child(controls);
        }

        header = header
            .child(
                div()
                    .id("project-spectrum-toggle")
                    .debug_selector(|| "project-spectrum-toggle".into())
                    .w(px(20.0))
                    .h(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(theme::text(if expanded { "−" } else { "+" }))
                    .tooltip(move |_, cx| {
                        cx.new(|_| super::transport::MiniTooltip {
                            kind: crate::tooltips::Kind::Identification,
                            description: if expanded {
                                "collapse spectrum".into()
                            } else {
                                "expand spectrum".into()
                            },
                        })
                        .into()
                    })
                    .on_click(cx.listener(|view, _, _, cx| {
                        cx.stop_propagation();
                        view.toggle_project_spectrum(cx);
                    })),
            )
            .child(self.sample_recorder_mode_button(cx));

        if compact {
            let (layers, _) = self.project_spectrum_layers_and_legend(std::time::Instant::now());
            let plot = layered_spectrum_plot(layers, style);
            header = header.child(
                div().flex_1().min_w_0().h(px(18.0)).child(
                    plot.id("project-spectrum-plot")
                        .debug_selector(|| "project-spectrum-plot".into()),
                ),
            );
        }
        let mut panel = div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .text_xs();
        if expanded && !compact {
            let now = std::time::Instant::now();
            let entity = match self.project_spectrum.canvas.clone() {
                Some(existing) => existing,
                None => {
                    let weak = cx.entity().downgrade();
                    let created = cx.new(|_| ProjectSpectrumCanvas::new(weak, style));
                    self.project_spectrum.canvas = Some(created.clone());
                    created
                }
            };
            self.sync_project_spectrum_canvas(now, cx);
            // Analyzer controls float over the quiet upper-right plot area;
            // they do not reserve a permanent title row above the data, and
            // occlude pointer input so clicks never bleed into the plot canvas.
            let header = header
                .absolute()
                .top_0()
                .right_0()
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation());
            panel = panel
                .child(
                    div()
                        .id("project-spectrum-plot")
                        .debug_selector(|| "project-spectrum-plot".into())
                        .flex_1()
                        .min_h_0()
                        .child(entity.clone()),
                )
                .child(self.project_spectrum_splitter_input(cx).handle(
                    "project-spectrum-resize".into(),
                    SplitterHandleConfig::thin(SplitterAxis::Vertical, style.border),
                ))
                .child(header);
        } else {
            panel = panel.child(header);
        }
        hex_widgets::panel_frame(theme::workspace_panel_style(), panel)
            .id("project-spectrum")
            .debug_selector(|| "project-spectrum".into())
            .h(px(height))
            .flex_shrink_0()
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
