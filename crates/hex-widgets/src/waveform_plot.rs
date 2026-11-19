//! Bounded, read-only waveform geometry and painting. Hosts supply projected
//! pixel envelopes, retained geometry and colors; source units, peak queries,
//! sampling, processing and cache identity stay with the caller.
use gpui::{
    Bounds, ContentMask, Div, Path, PathBuilder, Pixels, Point, Window, canvas, div, prelude::*,
    px, rgb,
};
use std::ops::Deref;

#[cfg(test)]
#[path = "waveform_band_tests.rs"]
mod band_tests;

/// Visible peak-column budget, shared with the host's bounded projection.
pub const MAX_WAVEFORM_COLUMNS: usize = 4096;
/// A full-source envelope may add its two exact boundary points.
pub const MAX_WAVEFORM_ENVELOPE_POINTS: usize = MAX_WAVEFORM_COLUMNS + 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveformEnvelopeMode {
    Filled,
    Trace,
}

#[derive(Clone, Copy, Debug)]
pub struct WaveformStroke {
    pub points: [Point<Pixels>; 2],
    pub width: Pixels,
}

/// Already projected upper/lower envelopes in window-local logical pixels.
/// The host retains its explicit fill/trace threshold and endpoint policy.
pub struct WaveformEnvelope<'a> {
    pub upper: &'a [Point<Pixels>],
    pub lower: &'a [Point<Pixels>],
    pub mode: WaveformEnvelopeMode,
    pub trace_width: Pixels,
    pub centerline: Option<WaveformStroke>,
}

/// Checked paths for one channel. Construction is bounded and fallible; the
/// painter cannot receive malformed point arrays or an unbounded iterator.
pub struct WaveformChannel {
    envelope: Path<Pixels>,
    centerline: Option<Path<Pixels>>,
    zero_ranges: Option<Path<Pixels>>,
    overrange: Option<Path<Pixels>>,
    filled_envelope: bool,
}
impl WaveformChannel {
    /// Preserve flat/DC portions inside an otherwise filled envelope through
    /// optional horizontal spans. No sample values or signal analysis live here.
    pub fn new(
        envelope: WaveformEnvelope<'_>,
        zero_ranges: impl IntoIterator<Item = [Point<Pixels>; 2]>,
    ) -> Option<Self> {
        let WaveformEnvelope {
            upper,
            lower,
            mode,
            trace_width,
            centerline,
        } = envelope;
        if upper.len() != lower.len()
            || upper.len() > MAX_WAVEFORM_ENVELOPE_POINTS
            || !valid_width(trace_width)
            || upper.iter().zip(lower).any(|(top, bottom)| {
                !valid_point(*top) || !valid_point(*bottom) || top.x != bottom.x || top.y > bottom.y
            })
            || upper.windows(2).any(|pair| pair[0].x > pair[1].x)
            || upper
                .first()
                .zip(upper.last())
                .is_some_and(|(first, last)| !f32::from(last.x - first.x).is_finite())
            || upper
                .iter()
                .zip(lower)
                .any(|(top, bottom)| !f32::from(bottom.y - top.y).is_finite())
            || !valid_vertical_extent(upper, lower)
        {
            return None;
        }
        let centerline = if let Some(line) = centerline {
            if !valid_width(line.width) || !valid_span(line.points) {
                return None;
            }
            let mut path = PathBuilder::stroke(line.width);
            path.move_to(line.points[0]);
            path.line_to(line.points[1]);
            Some((path.build().ok()?, line.points[0]))
        } else {
            None
        };
        let (path, filled_envelope) = if mode == WaveformEnvelopeMode::Filled && !upper.is_empty() {
            filled_band(upper, lower)
        } else if let Some(first) = upper.first().copied() {
            let mut path = PathBuilder::stroke(trace_width);
            path.move_to(first);
            for point in upper.iter().skip(1) {
                path.line_to(*point);
            }
            (path.build().ok()?, false)
        } else {
            if mode != WaveformEnvelopeMode::Trace {
                return None;
            }
            (Path::new(centerline.as_ref()?.1), false)
        };
        let mut flat = PathBuilder::stroke(trace_width);
        let mut spans = 0;
        for points in zero_ranges {
            if spans == MAX_WAVEFORM_ENVELOPE_POINTS || !valid_span(points) {
                return None;
            }
            spans += 1;
            flat.move_to(points[0]);
            flat.line_to(points[1]);
        }
        Some(Self {
            envelope: path,
            centerline: centerline.map(|(path, _)| path),
            zero_ranges: if spans > 0 {
                Some(flat.build().ok()?)
            } else {
                None
            },
            overrange: None,
            filled_envelope,
        })
    }

    /// Add bounded horizontal markers supplied by the host's signal projection.
    /// Empty input retains the original fast path and creates no marker path.
    pub fn with_overrange_markers(
        mut self,
        spans: impl IntoIterator<Item = [Point<Pixels>; 2]>,
        width: Pixels,
    ) -> Option<Self> {
        if !valid_width(width) {
            return None;
        }
        let mut spans = spans.into_iter().peekable();
        if spans.peek().is_none() {
            return Some(self);
        }
        let mut path = PathBuilder::stroke(width);
        let mut count = 0;
        for span in spans {
            if count == MAX_WAVEFORM_COLUMNS * 2 || !valid_span(span) {
                return None;
            }
            count += 1;
            path.move_to(span[0]);
            path.line_to(span[1]);
        }
        if count > 0 {
            self.overrange = Some(path.build().ok()?);
        }
        Some(self)
    }

    pub fn has_zero_range_trace(&self) -> bool {
        self.zero_ranges.is_some()
    }
}

// A validated envelope is an x-monotone band. Adjacent columns form an exact
// trapezoid, so no general polygon tessellation is needed. Keep the original
// vertices, including fractional positions; omit only zero-area triangles.
fn band_triangles(
    upper: &[Point<Pixels>],
    lower: &[Point<Pixels>],
    mut emit: impl FnMut([Point<Pixels>; 3]),
) {
    for (top, bottom) in upper.windows(2).zip(lower.windows(2)) {
        if top[0].x == top[1].x {
            continue;
        }
        if top[0].y < bottom[0].y {
            emit([top[0], top[1], bottom[0]]);
        }
        if top[1].y < bottom[1].y {
            emit([top[1], bottom[1], bottom[0]]);
        }
    }
}

fn filled_band(upper: &[Point<Pixels>], lower: &[Point<Pixels>]) -> (Path<Pixels>, bool) {
    let mut path = None;
    let solid = gpui::point(0.0, 1.0);
    band_triangles(upper, lower, |[a, b, c]| {
        // Seeding at the first triangle preserves tight bounds even when the
        // envelope begins with a distant zero-area tail.
        path.get_or_insert_with(|| Path::new(a))
            .push_triangle((a, b, c), (solid, solid, solid));
    });
    let filled = path.is_some();
    (path.unwrap_or_else(|| Path::new(Point::default())), filled)
}
fn valid_point(point: Point<Pixels>) -> bool {
    f32::from(point.x).is_finite() && f32::from(point.y).is_finite()
}
fn valid_vertical_extent(upper: &[Point<Pixels>], lower: &[Point<Pixels>]) -> bool {
    let min = upper.iter().map(|p| f32::from(p.y)).reduce(f32::min);
    let max = lower.iter().map(|p| f32::from(p.y)).reduce(f32::max);
    min.zip(max)
        .is_none_or(|(min, max)| (max - min).is_finite())
}
fn valid_width(width: Pixels) -> bool {
    f32::from(width).is_finite() && width > px(0.0)
}
fn valid_span(points: [Point<Pixels>; 2]) -> bool {
    points.iter().all(|point| valid_point(*point))
        && points[0].x <= points[1].x
        && points[0].y == points[1].y
        && f32::from(points[1].x - points[0].x).is_finite()
}

// Retained production geometry already stores two channels inline in one Rc/Arc.
// Boxing only the stereo variant would add an allocation and indirection to every
// cold production preparation; the smaller mono fixture remains bounded too.
#[allow(clippy::large_enum_variant)]
enum Channels {
    Mono(WaveformChannel),
    Stereo([WaveformChannel; 2]),
}
/// Immutable prepared display. The host keeps its existing Rc/Arc cache owner;
/// no second cache, source handle, or per-repaint path preparation is introduced.
pub struct WaveformGeometry {
    channels: Channels,
}
impl WaveformGeometry {
    pub fn mono(channel: WaveformChannel) -> Self {
        Self {
            channels: Channels::Mono(channel),
        }
    }
    pub fn stereo(channels: [WaveformChannel; 2]) -> Self {
        Self {
            channels: Channels::Stereo(channels),
        }
    }
    pub fn channels(&self) -> &[WaveformChannel] {
        match &self.channels {
            Channels::Mono(channel) => std::slice::from_ref(channel),
            Channels::Stereo(channels) => channels,
        }
    }
}
impl AsRef<WaveformGeometry> for WaveformGeometry {
    fn as_ref(&self) -> &Self {
        self
    }
}

/// Validated normalized emphasis, independent of source-frame or beat units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveformRange {
    start: f64,
    end: f64,
}
impl WaveformRange {
    pub fn new(start: f64, end: f64) -> Option<Self> {
        (start.is_finite() && end.is_finite() && start >= 0.0 && end <= 1.0 && start <= end)
            .then_some(Self { start, end })
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WaveformEmphasis {
    Uniform,
    /// None paints the entire overview in the inactive color.
    Range(Option<WaveformRange>),
}
#[derive(Clone, Copy, Debug)]
pub struct WaveformColors {
    pub waveform: u32,
    pub inactive: u32,
    pub centerline: u32,
    pub overrange: u32,
}
/// Actual paint bounds/scale and ancestor clip, supplied read-only to the host's
/// existing cache lookup. Changing a mask need not invalidate cached geometry.
#[derive(Clone, Copy, Debug)]
pub struct WaveformViewport {
    pub bounds: Bounds<Pixels>,
    pub scale: f32,
    pub mask: Bounds<Pixels>,
}
fn regions(emphasis: WaveformEmphasis, colors: WaveformColors) -> [(f64, f64, u32); 3] {
    match emphasis {
        WaveformEmphasis::Uniform => [(0.0, 1.0, colors.waveform), (0.0, 0.0, 0), (0.0, 0.0, 0)],
        WaveformEmphasis::Range(range) => {
            let (start, end) = range.map_or((0.0, 0.0), |range| (range.start, range.end));
            [
                (0.0, start, colors.inactive),
                (start, end, colors.waveform),
                (end, 1.0, colors.inactive),
            ]
        }
    }
}
fn paint_channels_with_offset(
    geometry: &WaveformGeometry,
    offset: Point<Pixels>,
    color: u32,
    centerline: u32,
    overrange: u32,
    window: &mut Window,
) {
    if let Channels::Stereo(channels) = &geometry.channels {
        let [upper, lower] = channels;
        if [upper, lower].iter().all(|channel| {
            channel.filled_envelope && channel.centerline.is_none() && channel.zero_ranges.is_none()
        }) {
            window.paint_disjoint_path_pair_with_offset(
                [&upper.envelope, &lower.envelope],
                offset,
                rgb(color),
            );
            for channel in channels {
                if let Some(path) = &channel.overrange {
                    window.paint_path_with_offset(path, offset, rgb(overrange));
                }
            }
            return;
        }
    }
    for channel in geometry.channels() {
        paint_channel_with_offset(channel, offset, color, centerline, overrange, window);
    }
}

fn paint_channel_with_offset(
    channel: &WaveformChannel,
    offset: Point<Pixels>,
    color: u32,
    centerline: u32,
    overrange: u32,
    window: &mut Window,
) {
    if let Some(path) = &channel.centerline {
        window.paint_path_with_offset(path, offset, rgb(centerline));
    }
    window.paint_path_with_offset(&channel.envelope, offset, rgb(color));
    if let Some(path) = &channel.zero_ranges {
        window.paint_path_with_offset(path, offset, rgb(color));
    }
    if let Some(path) = &channel.overrange {
        window.paint_path_with_offset(path, offset, rgb(overrange));
    }
}

/// Paint retained channel geometry into the containing view. The query returns
/// an existing smart pointer whose target exposes its checked display via AsRef;
/// host projection/test metadata can remain beside it without a nested allocation.
/// Queries must use bounded prepared peak/geometry data, never PCM, decoding or I/O.
pub fn waveform_plot<D>(
    prepare: impl FnOnce(WaveformViewport) -> Option<D> + 'static,
    colors: WaveformColors,
    emphasis: WaveformEmphasis,
) -> Div
where
    D: Deref + 'static,
    D::Target: AsRef<WaveformGeometry>,
{
    waveform_plot_with_offset(
        move |viewport| prepare(viewport).map(|geometry| (geometry, gpui::point(px(0.0), px(0.0)))),
        colors,
        emphasis,
    )
}

/// Paint retained waveform geometry translated by a logical-pixel offset.
///
/// The offset is applied during scene submission, before scale and clipping,
/// so cached local paths can be reused at a new location without rebuilding or
/// cloning their path meshes. The existing [`waveform_plot`] submits a zero
/// offset and retains its original behavior.
pub fn waveform_plot_with_offset<D>(
    prepare: impl FnOnce(WaveformViewport) -> Option<(D, Point<Pixels>)> + 'static,
    colors: WaveformColors,
    emphasis: WaveformEmphasis,
) -> Div
where
    D: Deref + 'static,
    D::Target: AsRef<WaveformGeometry>,
{
    div().absolute().inset_0().child(
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let width = f32::from(bounds.size.width);
                let height = f32::from(bounds.size.height);
                let scale = window.scale_factor();
                if !valid_point(bounds.origin)
                    || !width.is_finite()
                    || width <= 0.0
                    || !height.is_finite()
                    || height <= 0.0
                    || !scale.is_finite()
                    || scale <= 0.0
                {
                    return;
                }
                let Some((data, offset)) = prepare(WaveformViewport {
                    bounds,
                    scale,
                    mask: window.content_mask().bounds,
                }) else {
                    return;
                };
                if !valid_point(offset) {
                    return;
                }
                let geometry = data.deref().as_ref();
                // Timeline geometry deliberately includes neighboring columns. Its
                // current ancestor mask, not another canvas-bound mask, crops them.
                if emphasis == WaveformEmphasis::Uniform {
                    paint_channels_with_offset(
                        geometry,
                        offset,
                        colors.waveform,
                        colors.centerline,
                        colors.overrange,
                        window,
                    );
                    return;
                }
                for (left, right, color) in regions(emphasis, colors) {
                    if right <= left {
                        continue;
                    }
                    let mask = Bounds::new(
                        bounds.origin + gpui::point(px(left as f32 * width), px(0.0)),
                        gpui::size(px((right - left) as f32 * width), bounds.size.height),
                    );
                    window.with_content_mask(Some(ContentMask { bounds: mask }), |window| {
                        paint_channels_with_offset(
                            geometry,
                            offset,
                            color,
                            colors.centerline,
                            colors.overrange,
                            window,
                        );
                    });
                }
            },
        )
        .absolute()
        .inset_0(),
    )
}

#[cfg(test)]
#[path = "waveform_plot_tests.rs"]
mod tests;
