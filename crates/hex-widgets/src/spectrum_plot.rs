//! Bounded read-only overlaid spectrum plot with calibrated logarithmic Hz and
//! dBFS grids, Prism-style peak ribbons, and interactive crosshair readout.
//! Product owns identities, units, signal processing and colors.
use crate::ControlStyle;
use crate::spectrum_painter::{self, Layer as SpectrumLayer, Pass as SpectrumPass};
use gpui::{
    App, Bounds, ContentMask, Corners, Edges, Hsla, Pixels, SharedString, TextRun, Window, canvas,
    div, fill, point, prelude::*, px, quad, rgb, size,
};
use std::{ops::Deref, sync::Arc};

pub const MAX_SPECTRUM_PLOT_LAYERS: usize = 257;
const MAX_POINTS: usize = 256;
const MAX_LEGEND_ITEMS: usize = 8;

const AXIS_LEFT_INSET: f32 = 6.0;
const AXIS_RIGHT_INSET: f32 = 34.0;
const AXIS_TOP_INSET: f32 = 6.0;
const AXIS_BOTTOM_INSET: f32 = 18.0;
const COMPACT_INSET: f32 = 4.0;
const MIN_AXES_HEIGHT: f32 = 48.0;

#[derive(Clone, Debug, PartialEq)]
pub struct SpectrumPlotLayer {
    pub points: Vec<[f32; 2]>,
    /// Optional held-peak curve drawn as a thin line above the fill.
    pub peak: Option<Vec<[f32; 2]>>,
    pub color: u32,
}

/// Validated immutable curves. Cloning shares all point storage, so an
/// unchanged render does not allocate or revalidate every member's points.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpectrumPlotLayers(Arc<[SpectrumPlotLayer]>);

impl SpectrumPlotLayers {
    pub fn new(layers: Vec<SpectrumPlotLayer>) -> Option<Self> {
        let invalid = |points: &[[f32; 2]]| {
            !(2..=MAX_POINTS).contains(&points.len())
                || points.iter().any(|[x, y]| {
                    !x.is_finite()
                        || !y.is_finite()
                        || !(0.0..=1.0).contains(x)
                        || !(0.0..=1.0).contains(y)
                })
                || !points.windows(2).all(|p| p[0][0] <= p[1][0])
        };
        if layers.len() > MAX_SPECTRUM_PLOT_LAYERS
            || layers
                .iter()
                .any(|layer| invalid(&layer.points) || layer.peak.as_deref().is_some_and(invalid))
        {
            return None;
        }
        Some(Self(layers.into()))
    }
}

impl Deref for SpectrumPlotLayers {
    type Target = [SpectrumPlotLayer];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Interactive hover crosshair and readout painted inside the spectrum canvas.
#[derive(Clone, Debug, PartialEq)]
pub struct SpectrumPlotCrosshair {
    /// Normalized horizontal position in `0.0..=1.0`.
    pub x: f32,
    /// Normalized vertical pointer position in `0.0..=1.0` (`0` = floor, `1` = ceiling).
    pub y: f32,
    /// Optional normalized live curve level at `x` for the curve-snapped dot.
    pub trace_y: Option<f32>,
    /// Optional normalized held-peak level at `x` for the peak ring marker.
    pub peak_y: Option<f32>,
    /// Accent color for the curve-snapped marker.
    pub color: u32,
    /// Primary lowercase readout line (e.g. `440 hz · a4 +0c`).
    pub primary_label: SharedString,
    /// Optional secondary lowercase readout line (e.g. `-14.2 db · pk -8.4 db`).
    pub secondary_label: Option<SharedString>,
}

/// Compact upper-left color swatch + lowercase label for overlaid layers.
#[derive(Clone, Debug, PartialEq)]
pub struct SpectrumPlotLegendItem {
    pub label: SharedString,
    pub color: u32,
}

/// Display-only configuration for [`layered_spectrum_plot_with_config`].
#[derive(Clone, Debug, PartialEq)]
pub struct SpectrumPlotConfig {
    /// Minimum plotted frequency in Hz (`> 0`).
    pub min_hz: f32,
    /// Maximum plotted frequency in Hz (`> min_hz`).
    pub max_hz: f32,
    /// Bottom dBFS boundary (e.g. `-72.0` or `-96.0`).
    pub min_dbfs: f32,
    /// Top dBFS boundary (e.g. `6.0` or `0.0`).
    pub max_dbfs: f32,
    /// Show calibrated frequency/dB margins and labels when panel height permits.
    pub show_axes: bool,
    /// Paint a translucent Prism-style ribbon between the primary peak and live curves.
    pub peak_ribbon: bool,
    /// Optional interactive crosshair and floating frequency/note/dB readout.
    pub crosshair: Option<SpectrumPlotCrosshair>,
    /// Optional subtle status pill in the upper-left (e.g. `frozen`).
    pub badge: Option<SharedString>,
    /// Optional compact colored layer legend chips in the upper-left.
    pub legend: Vec<SpectrumPlotLegendItem>,
}

impl Default for SpectrumPlotConfig {
    fn default() -> Self {
        Self {
            min_hz: 20.0,
            max_hz: 20_000.0,
            min_dbfs: -96.0,
            max_dbfs: 0.0,
            show_axes: false,
            peak_ribbon: false,
            crosshair: None,
            badge: None,
            legend: Vec::new(),
        }
    }
}

/// Exact inner curve rectangle inside the spectrum canvas bounds, shared
/// between rendering and pointer-to-frequency/dB coordinate conversion.
pub fn spectrum_plot_bounds(bounds: Bounds<Pixels>, show_axes: bool) -> Bounds<Pixels> {
    let use_axes = show_axes && f32::from(bounds.size.height) >= MIN_AXES_HEIGHT;
    let (left, right, top, bottom) = if use_axes {
        (
            AXIS_LEFT_INSET,
            AXIS_RIGHT_INSET,
            AXIS_TOP_INSET,
            AXIS_BOTTOM_INSET,
        )
    } else {
        (COMPACT_INSET, COMPACT_INSET, COMPACT_INSET, COMPACT_INSET)
    };
    Bounds::new(
        bounds.origin + point(px(left), px(top)),
        size(
            (bounds.size.width - px(left + right)).max(px(0.0)),
            (bounds.size.height - px(top + bottom)).max(px(0.0)),
        ),
    )
}

/// Format a frequency in Hz into lowercase frequency and equal-temperament
/// musical note + cents strings (A4 = 440 Hz), e.g. `("440 hz", "a4 +0c")`.
pub fn format_hz_and_note(hz: f32) -> Option<(String, String)> {
    if !hz.is_finite() || hz <= 0.0 {
        return None;
    }
    let freq_text = if hz < 100.0 {
        format!("{hz:.1} hz")
    } else if hz < 1_000.0 {
        format!("{hz:.0} hz")
    } else if hz < 10_000.0 {
        format!("{:.2} khz", hz / 1_000.0)
    } else {
        format!("{:.1} khz", hz / 1_000.0)
    };
    const NOTES: [&str; 12] = [
        "c", "c#", "d", "d#", "e", "f", "f#", "g", "g#", "a", "a#", "b",
    ];
    let midi_float = 69.0 + 12.0 * (f64::from(hz) / 440.0).log2();
    let nearest = midi_float.round();
    let cents = ((midi_float - nearest) * 100.0).round() as i32;
    let midi = nearest as i32;
    let note_index = midi.rem_euclid(12) as usize;
    let octave = midi.div_euclid(12) - 1;
    let note_text = if cents >= 0 {
        format!("{}{octave} +{cents}c", NOTES[note_index])
    } else {
        format!("{}{octave} {cents}c", NOTES[note_index])
    };
    Some((freq_text, note_text))
}

#[inline]
fn snap_px(val: f32, scale: f32) -> f32 {
    if scale > 0.0 && val.is_finite() {
        (val * scale).round() / scale
    } else {
        val
    }
}

#[inline]
fn hairline_px(scale: f32) -> Pixels {
    px(1.0 / scale.max(1.0))
}

fn can_reserve_layer(bounds: Bounds<Pixels>, mask: Bounds<Pixels>, scale: f32) -> bool {
    let clipped = bounds.intersect(&mask);
    let reserved = clipped.scale(scale);
    !clipped.is_empty()
        && !reserved.is_empty()
        && [
            reserved.left(),
            reserved.right(),
            reserved.top(),
            reserved.bottom(),
        ]
        .into_iter()
        .all(|edge| f64::from(edge).is_finite())
}

fn with_layer(bounds: Bounds<Pixels>, window: &mut Window, paint: impl FnOnce(&mut Window)) {
    let mask = window.content_mask().bounds;
    let scale = window.scale_factor();
    if can_reserve_layer(bounds, mask, scale) {
        window.paint_layer(bounds, paint);
    } else {
        paint(window);
    }
}

struct FreqTick {
    hz: f32,
    label: &'static str,
    major: bool,
    priority: u8,
}

const FREQ_TICKS: &[FreqTick] = &[
    FreqTick {
        hz: 20.0,
        label: "20",
        major: false,
        priority: 2,
    },
    FreqTick {
        hz: 30.0,
        label: "30",
        major: false,
        priority: 1,
    },
    FreqTick {
        hz: 50.0,
        label: "50",
        major: false,
        priority: 1,
    },
    FreqTick {
        hz: 100.0,
        label: "100",
        major: true,
        priority: 0,
    },
    FreqTick {
        hz: 200.0,
        label: "200",
        major: false,
        priority: 1,
    },
    FreqTick {
        hz: 300.0,
        label: "300",
        major: false,
        priority: 2,
    },
    FreqTick {
        hz: 500.0,
        label: "500",
        major: false,
        priority: 1,
    },
    FreqTick {
        hz: 1_000.0,
        label: "1k",
        major: true,
        priority: 0,
    },
    FreqTick {
        hz: 2_000.0,
        label: "2k",
        major: false,
        priority: 1,
    },
    FreqTick {
        hz: 3_000.0,
        label: "3k",
        major: false,
        priority: 2,
    },
    FreqTick {
        hz: 5_000.0,
        label: "5k",
        major: false,
        priority: 1,
    },
    FreqTick {
        hz: 10_000.0,
        label: "10k",
        major: true,
        priority: 0,
    },
    FreqTick {
        hz: 20_000.0,
        label: "20k",
        major: false,
        priority: 1,
    },
];

fn choose_db_step(span_db: f32, plot_height: f32) -> f32 {
    for step in [6.0, 12.0, 18.0, 24.0] {
        let px_per_step = plot_height * (step / span_db);
        if px_per_step >= 18.0 {
            return step;
        }
    }
    24.0
}

fn db_grid_values(min_dbfs: f32, max_dbfs: f32, plot_height: f32) -> Vec<f32> {
    let span = max_dbfs - min_dbfs;
    if !span.is_finite() || span <= 0.0 || !plot_height.is_finite() || plot_height <= 0.0 {
        return Vec::new();
    }
    let step = choose_db_step(span, plot_height);
    let top_multiple = (max_dbfs / step).floor() as i32;
    let bot_multiple = (min_dbfs / step).ceil() as i32;
    let mut values = Vec::new();
    let count = (top_multiple - bot_multiple + 1).clamp(0, 32);
    for idx in 0..count {
        let db = (top_multiple - idx) as f32 * step;
        if db >= min_dbfs + 0.01 && db <= max_dbfs - 0.01 {
            values.push(db);
        }
    }
    // Always include 0 dBFS reference line when headroom puts it inside the plot.
    if min_dbfs < -0.01 && max_dbfs > 0.01 && !values.iter().any(|db| db.abs() < 0.05) {
        values.push(0.0);
        values.sort_by(|a, b| b.total_cmp(a));
    }
    values
}

fn paint_grid_and_axes(
    bounds: Bounds<Pixels>,
    plot: Bounds<Pixels>,
    config: &SpectrumPlotConfig,
    use_axes: bool,
    style: ControlStyle,
    window: &mut Window,
    cx: &mut App,
) {
    let scale = window.scale_factor();
    let line_px = hairline_px(scale);
    let plot_w = f32::from(plot.size.width);
    let plot_h = f32::from(plot.size.height);
    let min_hz = if config.min_hz.is_finite() && config.min_hz > 0.0 {
        config.min_hz
    } else {
        20.0
    };
    let max_hz = if config.max_hz.is_finite() && config.max_hz > min_hz {
        config.max_hz
    } else {
        20_000.0
    };
    let log_span = (max_hz / min_hz).ln();
    let min_dbfs = if config.min_dbfs.is_finite() {
        config.min_dbfs
    } else {
        -96.0
    };
    let max_dbfs = if config.max_dbfs.is_finite() && config.max_dbfs > min_dbfs {
        config.max_dbfs
    } else {
        0.0
    };
    let db_span = max_dbfs - min_dbfs;

    let rail_hsla: Hsla = rgb(style.rail).into();
    let border_hsla: Hsla = rgb(style.border).into();
    let muted_hsla: Hsla = rgb(style.muted).into();
    let minor_grid = rail_hsla.opacity(0.45);
    let major_grid = rail_hsla.opacity(0.95);
    let zero_db_grid = border_hsla.opacity(0.85);

    let db_lines = db_grid_values(min_dbfs, max_dbfs, plot_h);

    with_layer(bounds, window, |window| {
        // Horizontal calibrated dB gridlines.
        for &db in &db_lines {
            let norm_y = ((db - min_dbfs) / db_span).clamp(0.0, 1.0);
            let y = snap_px(f32::from(plot.top()) + plot_h * (1.0 - norm_y), scale);
            let color = if db.abs() < 0.05 && max_dbfs > 0.01 {
                zero_db_grid
            } else {
                minor_grid
            };
            window.paint_quad(fill(
                Bounds::new(point(plot.left(), px(y)), size(plot.size.width, line_px)),
                color,
            ));
        }
        // Subtle plot baseline & top boundary frame when axes are shown.
        if use_axes {
            let bottom_y = snap_px(f32::from(plot.bottom()), scale);
            window.paint_quad(fill(
                Bounds::new(
                    point(plot.left(), px(bottom_y)),
                    size(plot.size.width, line_px),
                ),
                major_grid,
            ));
            let right_x = snap_px(f32::from(plot.right()), scale);
            window.paint_quad(fill(
                Bounds::new(
                    point(px(right_x), plot.top()),
                    size(line_px, plot.size.height),
                ),
                minor_grid,
            ));
        }

        // Vertical logarithmic frequency gridlines.
        for tick in FREQ_TICKS {
            if tick.hz <= min_hz * 1.001 || tick.hz >= max_hz * 0.999 {
                continue;
            }
            let norm_x = ((tick.hz / min_hz).ln() / log_span).clamp(0.0, 1.0);
            let x = snap_px(f32::from(plot.left()) + plot_w * norm_x, scale);
            let color = if tick.major { major_grid } else { minor_grid };
            window.paint_quad(fill(
                Bounds::new(point(px(x), plot.top()), size(line_px, plot.size.height)),
                color,
            ));
        }

        if !use_axes {
            return;
        }

        // Paint crisp lowercase frequency & dB labels directly in the same layer.
        let font = window.text_style().font();
        let font_size = px(8.5);
        let line_height = px(10.0);
        let label_color = muted_hsla.opacity(0.85);
        let major_label_color = muted_hsla;

        let max_priority = if plot_w >= 760.0 {
            2
        } else if plot_w >= 380.0 {
            1
        } else {
            0
        };
        let mut last_right = f32::NEG_INFINITY;
        let label_y = plot.bottom() + px(4.0);
        for tick in FREQ_TICKS {
            if tick.priority > max_priority || tick.hz < min_hz * 0.99 || tick.hz > max_hz * 1.01 {
                continue;
            }
            let norm_x = ((tick.hz / min_hz).ln() / log_span).clamp(0.0, 1.0);
            let center_x = f32::from(plot.left()) + plot_w * norm_x;
            let text: SharedString = tick.label.into();
            let run = TextRun {
                len: text.len(),
                font: font.clone(),
                color: if tick.major {
                    major_label_color
                } else {
                    label_color
                },
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped = window
                .text_system()
                .shape_line(text, font_size, &[run], None);
            let w = f32::from(shaped.width);
            let left = (center_x - w * 0.5).clamp(
                f32::from(plot.left()),
                (f32::from(plot.right()) - w).max(f32::from(plot.left())),
            );
            if left < last_right + 6.0 {
                continue;
            }
            last_right = left + w;
            let _ = shaped.paint(point(px(left), label_y), line_height, window, cx);
        }

        // Right-margin dBFS labels, skipping labels too close to top/bottom edges.
        let mut last_label_y = f32::NEG_INFINITY;
        for &db in &db_lines {
            let norm_y = ((db - min_dbfs) / db_span).clamp(0.0, 1.0);
            let y = f32::from(plot.top()) + plot_h * (1.0 - norm_y);
            if y < f32::from(plot.top()) + 4.0 || y > f32::from(plot.bottom()) - 4.0 {
                continue;
            }
            if (y - last_label_y).abs() < 12.0 {
                continue;
            }
            last_label_y = y;
            let text: SharedString = if db.abs() < 0.05 {
                "0".into()
            } else {
                format!("{db:.0}").into()
            };
            let run = TextRun {
                len: text.len(),
                font: font.clone(),
                color: if db.abs() < 0.05 && max_dbfs > 0.01 {
                    major_label_color
                } else {
                    label_color
                },
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped = window
                .text_system()
                .shape_line(text, font_size, &[run], None);
            let right_edge = f32::from(bounds.right()) - 4.0;
            let x = (right_edge - f32::from(shaped.width)).max(f32::from(plot.right()) + 3.0);
            let _ = shaped.paint(point(px(x), px(y - 5.0)), line_height, window, cx);
        }
    });
}

fn paint_overlays(
    bounds: Bounds<Pixels>,
    plot: Bounds<Pixels>,
    config: &SpectrumPlotConfig,
    style: ControlStyle,
    window: &mut Window,
    cx: &mut App,
) {
    let has_crosshair = config.crosshair.is_some();
    let has_badge = config.badge.is_some();
    let has_legend = !config.legend.is_empty();
    if !has_crosshair && !has_badge && !has_legend {
        return;
    }
    let scale = window.scale_factor();
    let line_px = hairline_px(scale);
    let font = window.text_style().font();
    let surface_hsla: Hsla = rgb(style.background).into();
    let border_hsla: Hsla = rgb(style.border).into();
    let text_hsla: Hsla = rgb(style.text).into();
    let muted_hsla: Hsla = rgb(style.muted).into();

    with_layer(bounds, window, |window| {
        // Upper-left subtle badge (e.g. "frozen") and compact track legend chips.
        let mut chip_x = f32::from(plot.left()) + 4.0;
        let chip_y = f32::from(plot.top()) + 3.0;
        let max_chip_right = f32::from(plot.left()) + f32::from(plot.size.width) * 0.52;
        if let Some(badge) = config.badge.as_ref()
            && !badge.is_empty()
        {
            let run = TextRun {
                len: badge.len(),
                font: font.clone(),
                color: text_hsla,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped = window
                .text_system()
                .shape_line(badge.clone(), px(8.5), &[run], None);
            let pill_w = f32::from(shaped.width) + 10.0;
            let pill_h = 14.0;
            if chip_x + pill_w <= max_chip_right {
                window.paint_quad(quad(
                    Bounds::new(point(px(chip_x), px(chip_y)), size(px(pill_w), px(pill_h))),
                    Corners::all(px(3.0)),
                    surface_hsla.opacity(0.86),
                    Edges::all(line_px),
                    border_hsla.opacity(0.9),
                    gpui::BorderStyle::Solid,
                ));
                let _ = shaped.paint(
                    point(px(chip_x + 5.0), px(chip_y + 2.0)),
                    px(10.0),
                    window,
                    cx,
                );
                chip_x += pill_w + 4.0;
            }
        }
        for item in config.legend.iter().take(MAX_LEGEND_ITEMS) {
            if item.label.is_empty() || item.color > 0xffffff {
                continue;
            }
            let run = TextRun {
                len: item.label.len(),
                font: font.clone(),
                color: muted_hsla,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped = window
                .text_system()
                .shape_line(item.label.clone(), px(8.5), &[run], None);
            let pill_w = f32::from(shaped.width) + 16.0;
            let pill_h = 14.0;
            if chip_x + pill_w > max_chip_right {
                break;
            }
            window.paint_quad(quad(
                Bounds::new(point(px(chip_x), px(chip_y)), size(px(pill_w), px(pill_h))),
                Corners::all(px(3.0)),
                surface_hsla.opacity(0.76),
                Edges::all(line_px),
                border_hsla.opacity(0.45),
                gpui::BorderStyle::Solid,
            ));
            let swatch = Bounds::new(
                point(px(chip_x + 4.0), px(chip_y + 4.5)),
                size(px(5.0), px(5.0)),
            );
            window.paint_quad(quad(
                swatch,
                Corners::all(px(2.5)),
                rgb(item.color),
                Edges::default(),
                gpui::transparent_black(),
                gpui::BorderStyle::Solid,
            ));
            let _ = shaped.paint(
                point(px(chip_x + 12.0), px(chip_y + 2.0)),
                px(10.0),
                window,
                cx,
            );
            chip_x += pill_w + 4.0;
        }

        // Interactive Prism-style crosshair + live curve dot + floating HUD pill.
        let Some(cross) = config.crosshair.as_ref() else {
            return;
        };
        if !cross.x.is_finite()
            || !cross.y.is_finite()
            || !(0.0..=1.0).contains(&cross.x)
            || !(0.0..=1.0).contains(&cross.y)
        {
            return;
        }
        let plot_w = f32::from(plot.size.width);
        let plot_h = f32::from(plot.size.height);
        let cx_px = snap_px(f32::from(plot.left()) + plot_w * cross.x, scale);
        let cy_px = snap_px(f32::from(plot.top()) + plot_h * (1.0 - cross.y), scale);
        let guide = muted_hsla.opacity(0.42);
        window.with_content_mask(Some(ContentMask { bounds: plot }), |window| {
            // Vertical guide
            window.paint_quad(fill(
                Bounds::new(
                    point(px(cx_px), plot.top()),
                    size(line_px, plot.size.height),
                ),
                guide,
            ));
            // Horizontal guide
            window.paint_quad(fill(
                Bounds::new(
                    point(plot.left(), px(cy_px)),
                    size(plot.size.width, line_px),
                ),
                guide,
            ));
            // Optional peak ring marker when peak sits noticeably above live trace
            if let Some(peak_y) = cross
                .peak_y
                .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
            {
                let py = f32::from(plot.top()) + plot_h * (1.0 - peak_y);
                let accent: Hsla = rgb(cross.color.min(0xffffff)).into();
                let ring =
                    Bounds::new(point(px(cx_px - 3.0), px(py - 3.0)), size(px(6.0), px(6.0)));
                window.paint_quad(quad(
                    ring,
                    Corners::all(px(3.0)),
                    surface_hsla.opacity(0.6),
                    Edges::all(px(1.0)),
                    accent.opacity(0.7),
                    gpui::BorderStyle::Solid,
                ));
            }
            // Luminous curve-snapped marker dot
            if let Some(trace_y) = cross
                .trace_y
                .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
            {
                let ty = f32::from(plot.top()) + plot_h * (1.0 - trace_y);
                let dot = Bounds::new(point(px(cx_px - 3.0), px(ty - 3.0)), size(px(6.0), px(6.0)));
                window.paint_quad(quad(
                    dot,
                    Corners::all(px(3.0)),
                    rgb(cross.color.min(0xffffff)),
                    Edges::all(px(1.0)),
                    surface_hsla.opacity(0.9),
                    gpui::BorderStyle::Solid,
                ));
            }
        });

        // Floating HUD pill showing frequency, musical note + cents, and dBFS.
        if cross.primary_label.is_empty() {
            return;
        }
        let primary_run = TextRun {
            len: cross.primary_label.len(),
            font: font.clone(),
            color: text_hsla,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let primary_shaped = window.text_system().shape_line(
            cross.primary_label.clone(),
            px(9.0),
            &[primary_run],
            None,
        );
        let secondary_shaped = cross
            .secondary_label
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|text| {
                let run = TextRun {
                    len: text.len(),
                    font: font.clone(),
                    color: muted_hsla,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                window
                    .text_system()
                    .shape_line(text.clone(), px(8.5), &[run], None)
            });
        let text_w = f32::from(primary_shaped.width).max(
            secondary_shaped
                .as_ref()
                .map_or(0.0, |s| f32::from(s.width)),
        );
        let pad_x = 6.0;
        let pad_y = 4.0;
        let box_w = text_w + pad_x * 2.0;
        let box_h = if secondary_shaped.is_some() {
            27.0
        } else {
            16.0
        };
        let margin = 10.0;
        let mut bx = if cx_px + margin + box_w <= f32::from(plot.right()) - 4.0 {
            cx_px + margin
        } else {
            cx_px - margin - box_w
        };
        let mut by = cy_px - box_h - 8.0;
        // Keep away from top floating controls strip (top 24px) when possible.
        let min_y = if f32::from(plot.size.height) > box_h + 26.0 {
            f32::from(plot.top()) + 22.0
        } else {
            f32::from(plot.top()) + 2.0
        };
        if by < min_y {
            by = cy_px + 10.0;
        }
        let max_y = (f32::from(plot.bottom()) - box_h - 2.0).max(f32::from(plot.top()) + 2.0);
        by = by.clamp(f32::from(plot.top()) + 2.0, max_y);
        let max_x = (f32::from(plot.right()) - box_w - 2.0).max(f32::from(plot.left()) + 2.0);
        bx = bx.clamp(f32::from(plot.left()) + 2.0, max_x);

        window.paint_quad(quad(
            Bounds::new(point(px(bx), px(by)), size(px(box_w), px(box_h))),
            Corners::all(px(3.0)),
            surface_hsla.opacity(0.92),
            Edges::all(line_px),
            border_hsla.opacity(0.85),
            gpui::BorderStyle::Solid,
        ));
        let _ = primary_shaped.paint(point(px(bx + pad_x), px(by + pad_y)), px(10.0), window, cx);
        if let Some(secondary) = secondary_shaped {
            let _ = secondary.paint(
                point(px(bx + pad_x), px(by + pad_y + 11.0)),
                px(10.0),
                window,
                cx,
            );
        }
    });
}

/// Paint every supplied independent layer in order, with the final layer above
/// the others, using calibrated studio axes, optional Prism-style peak ribbon,
/// and optional interactive crosshair readout.
pub fn layered_spectrum_plot_with_config(
    layers: SpectrumPlotLayers,
    config: SpectrumPlotConfig,
    style: ControlStyle,
) -> gpui::Div {
    div()
        .relative()
        .size_full()
        .bg(rgb(style.background))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    let use_axes =
                        config.show_axes && f32::from(bounds.size.height) >= MIN_AXES_HEIGHT;
                    let plot = spectrum_plot_bounds(bounds, use_axes);
                    if plot.size.width <= px(0.0) || plot.size.height <= px(0.0) {
                        return;
                    }
                    let top = layers.len().saturating_sub(1);
                    let spectra = layers
                        .iter()
                        .enumerate()
                        .map(|(index, layer)| SpectrumLayer {
                            points: &layer.points,
                            peak: layer.peak.as_deref(),
                            color: layer.color,
                            stroke_width: if index == top {
                                spectrum_painter::PRIMARY_TRACE_WIDTH
                            } else {
                                spectrum_painter::SECONDARY_TRACE_WIDTH
                            },
                        });
                    // 1. Gradient fills under every curve.
                    spectrum_painter::paint(plot, spectra.clone(), SpectrumPass::Fill, window);
                    // 2. Calibrated logarithmic Hz + dBFS grid & axis labels (one layer).
                    paint_grid_and_axes(bounds, plot, &config, use_axes, style, window, cx);
                    // 3. Optional translucent Prism-style peak-to-live ribbon for primary layer.
                    if config.peak_ribbon
                        && let Some(primary) = layers.last()
                    {
                        let layer = SpectrumLayer {
                            points: &primary.points,
                            peak: primary.peak.as_deref(),
                            color: primary.color,
                            stroke_width: spectrum_painter::PRIMARY_TRACE_WIDTH,
                        };
                        spectrum_painter::paint_peak_ribbon(plot, &layer, window);
                    }
                    // 4. Held-peak traces above grid.
                    spectrum_painter::paint(plot, spectra.clone(), SpectrumPass::Peak, window);
                    // 5. Crisp live traces on top.
                    spectrum_painter::paint(plot, spectra, SpectrumPass::Trace, window);
                    // 6. Optional frozen badge, layer legend, crosshair & readout pill.
                    paint_overlays(bounds, plot, &config, style, window, cx);
                },
            )
            .size_full(),
        )
}

/// Paint every supplied independent layer in order, with the final layer above
/// the others. No summation, domain conversion, signal processing or selection.
pub fn layered_spectrum_plot(layers: SpectrumPlotLayers, style: ControlStyle) -> gpui::Div {
    layered_spectrum_plot_with_config(layers, SpectrumPlotConfig::default(), style)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_or_excess_layers_are_refused_without_dropping_members() {
        let layer = SpectrumPlotLayer {
            points: vec![[0.0, 0.0], [1.0, 1.0]],
            peak: Some(vec![[0.0, 0.5], [1.0, 1.0]]),
            color: 0x123456,
        };
        assert!(SpectrumPlotLayers::new(vec![layer.clone(); MAX_SPECTRUM_PLOT_LAYERS]).is_some());
        assert!(
            SpectrumPlotLayers::new(vec![layer.clone(); MAX_SPECTRUM_PLOT_LAYERS + 1]).is_none()
        );
        let invalid = SpectrumPlotLayer {
            points: vec![[1.0, 0.0], [0.0, 1.0]],
            ..layer
        };
        assert!(SpectrumPlotLayers::new(vec![invalid.clone()]).is_none());
        let invalid_peak = SpectrumPlotLayer {
            peak: Some(vec![[0.0, f32::NAN], [1.0, 1.0]]),
            points: vec![[0.0, 0.0], [1.0, 1.0]],
            ..invalid
        };
        assert!(SpectrumPlotLayers::new(vec![invalid_peak]).is_none());
    }

    #[test]
    fn format_hz_and_note_maps_concert_pitch_and_rejects_invalid_frequencies() {
        assert_eq!(
            format_hz_and_note(440.0),
            Some(("440 hz".into(), "a4 +0c".into()))
        );
        assert_eq!(
            format_hz_and_note(261.62558),
            Some(("262 hz".into(), "c4 +0c".into()))
        );
        assert_eq!(
            format_hz_and_note(1_000.0),
            Some(("1.00 khz".into(), "b5 +21c".into()))
        );
        assert_eq!(
            format_hz_and_note(55.0),
            Some(("55.0 hz".into(), "a1 +0c".into()))
        );
        assert_eq!(format_hz_and_note(0.0), None);
        assert_eq!(format_hz_and_note(-100.0), None);
        assert_eq!(format_hz_and_note(f32::NAN), None);
        assert_eq!(format_hz_and_note(f32::INFINITY), None);
    }

    #[test]
    fn spectrum_plot_bounds_switches_cleanly_between_compact_and_calibrated_axes() {
        let compact = Bounds::new(point(px(0.0), px(0.0)), size(px(400.0), px(18.0)));
        assert_eq!(
            spectrum_plot_bounds(compact, true),
            Bounds::new(point(px(4.0), px(4.0)), size(px(392.0), px(10.0)))
        );
        let expanded = Bounds::new(point(px(10.0), px(20.0)), size(px(600.0), px(120.0)));
        assert_eq!(
            spectrum_plot_bounds(expanded, true),
            Bounds::new(point(px(16.0), px(26.0)), size(px(560.0), px(96.0)))
        );
        assert_eq!(
            spectrum_plot_bounds(expanded, false),
            Bounds::new(point(px(14.0), px(24.0)), size(px(592.0), px(112.0)))
        );
    }

    #[test]
    fn db_grid_includes_zero_db_headroom_and_stays_bounded() {
        let with_headroom = db_grid_values(-72.0, 6.0, 120.0);
        assert!(with_headroom.iter().any(|db| db.abs() < 0.05));
        assert!(with_headroom.iter().all(|db| *db > -72.0 && *db < 6.0));
        let tight = db_grid_values(-96.0, 0.0, 40.0);
        assert!(!tight.is_empty() && tight.len() <= 8);
        assert!(db_grid_values(0.0, -12.0, 100.0).is_empty());
        assert!(db_grid_values(f32::NAN, 0.0, 100.0).is_empty());
    }
}
