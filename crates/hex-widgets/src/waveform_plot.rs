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
/// Horizontal guide projected to one device pixel at the current paint scale.
pub struct WaveformZeroLine {
    pub points: [Point<Pixels>; 2],
}

/// Already projected upper/lower envelopes in window-local logical pixels.
/// The host retains its explicit fill/trace threshold and endpoint policy.
pub struct WaveformEnvelope<'a> {
    pub upper: &'a [Point<Pixels>],
    pub lower: &'a [Point<Pixels>],
    pub mode: WaveformEnvelopeMode,
    pub trace_width: Pixels,
    pub centerline: Option<WaveformZeroLine>,
}

/// Checked envelope and guide for one channel. Construction is bounded and fallible; the
/// painter cannot receive malformed point arrays or an unbounded iterator.
pub struct WaveformChannel {
    envelope: Path<Pixels>,
    envelope_bounds: Option<Bounds<Pixels>>,
    centerline: Option<WaveformZeroLine>,
    zero_ranges: Option<Path<Pixels>>,
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
        {
            return None;
        }
        // Reuse the validation extrema for conservative paint-layer bounds;
        // retaining them adds no extra envelope traversal.
        let envelope_bounds = envelope_bounds(upper, lower);
        if upper.iter().zip(lower).any(|(top, bottom)| {
            !valid_point(*top) || !valid_point(*bottom) || top.x != bottom.x || top.y > bottom.y
        }) || upper.windows(2).any(|pair| pair[0].x > pair[1].x)
            || upper
                .first()
                .zip(upper.last())
                .is_some_and(|(first, last)| !f32::from(last.x - first.x).is_finite())
            || upper
                .iter()
                .zip(lower)
                .any(|(top, bottom)| !f32::from(bottom.y - top.y).is_finite())
            || envelope_bounds.is_some_and(|bounds| !finite_bounds(bounds))
        {
            return None;
        }
        if let Some(line) = centerline
            && !valid_span(line.points)
        {
            return None;
        }
        let path = if mode == WaveformEnvelopeMode::Filled && !upper.is_empty() {
            filled_band(upper, lower)
        } else if let Some(first) = upper.first().copied() {
            let mut path = PathBuilder::stroke(trace_width);
            path.move_to(first);
            for point in upper.iter().skip(1) {
                path.line_to(*point);
            }
            path.build().ok()?
        } else {
            if mode != WaveformEnvelopeMode::Trace {
                return None;
            }
            Path::new(centerline.as_ref()?.points[0])
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
            envelope_bounds: envelope_bounds.filter(|_| mode == WaveformEnvelopeMode::Filled),
            centerline,
            zero_ranges: if spans > 0 {
                Some(flat.build().ok()?)
            } else {
                None
            },
        })
    }

    /// Device-aligned guide bounds for the retained horizontal span. The paint
    /// scale is deliberately evaluated here, not baked into cached paths.
    pub fn zero_line_bounds(&self, scale: f32) -> Option<Bounds<Pixels>> {
        let line = self.centerline?;
        if !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        let left = (f32::from(line.points[0].x) * scale).ceil();
        let right = (f32::from(line.points[1].x) * scale).floor();
        let top = (f32::from(line.points[0].y) * scale).floor();
        let bounds = Bounds::new(
            gpui::point(px(left / scale), px(top / scale)),
            gpui::size(px((right - left) / scale), px(1.0 / scale)),
        );
        (valid_point(bounds.origin)
            && valid_point(bounds.bottom_right())
            && valid_width(bounds.size.width)
            && valid_width(bounds.size.height))
        .then_some(bounds)
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

fn filled_band(upper: &[Point<Pixels>], lower: &[Point<Pixels>]) -> Path<Pixels> {
    let mut path = None;
    let solid = gpui::point(0.0, 1.0);
    band_triangles(upper, lower, |[a, b, c]| {
        // Seeding at the first triangle preserves tight bounds even when the
        // envelope begins with a distant zero-area tail.
        path.get_or_insert_with(|| Path::new(a))
            .push_triangle((a, b, c), (solid, solid, solid));
    });
    path.unwrap_or_else(|| Path::new(Point::default()))
}
fn valid_point(point: Point<Pixels>) -> bool {
    f32::from(point.x).is_finite() && f32::from(point.y).is_finite()
}
fn envelope_bounds(upper: &[Point<Pixels>], lower: &[Point<Pixels>]) -> Option<Bounds<Pixels>> {
    let min = upper.iter().map(|p| f32::from(p.y)).reduce(f32::min)?;
    let max = lower.iter().map(|p| f32::from(p.y)).reduce(f32::max)?;
    Some(Bounds::new(
        gpui::point(upper.first()?.x, px(min)),
        gpui::size(upper.last()?.x - upper.first()?.x, px(max - min)),
    ))
}
fn finite_bounds(bounds: Bounds<Pixels>) -> bool {
    valid_point(bounds.origin)
        && valid_point(bounds.bottom_right())
        && f32::from(bounds.size.width).is_finite()
        && f32::from(bounds.size.height).is_finite()
}
fn disjoint_layer(a: Bounds<Pixels>, b: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
    if !finite_bounds(a) || !finite_bounds(b) || a.is_empty() || b.is_empty() || a.intersects(&b) {
        return None;
    }
    let union = a.union(&b);
    finite_bounds(union).then_some(union)
}
// GPUI layers require non-overlapping geometry at one draw order. Include a
// conservative physical AA pixel; uncertain/overlapping spans retain their
// original independent submission. Stroked paths have join extents beyond their
// point envelope, and separate DC traces may overlap a fill, so neither batches.
fn envelope_layer(geometry: &WaveformGeometry, scale: f32) -> Option<Bounds<Pixels>> {
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let [a, b] = geometry.channels() else {
        return None;
    };
    if a.zero_ranges.is_some() || b.zero_ranges.is_some() {
        return None;
    }
    let padded = |channel: &WaveformChannel| {
        channel
            .envelope_bounds
            .map(|bounds| bounds.dilate(px(1.0 / scale)))
    };
    disjoint_layer(padded(a)?, padded(b)?)
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
// Guides are shared by every emphasis region and submitted only once per lane.
fn zero_line_quads(
    geometry: &WaveformGeometry,
    scale: f32,
) -> impl Iterator<Item = Bounds<Pixels>> {
    geometry
        .channels()
        .iter()
        .filter_map(move |channel| channel.zero_line_bounds(scale))
}
fn paint_guides(geometry: &WaveformGeometry, centerline: u32, scale: f32, window: &mut Window) {
    let mut quads = zero_line_quads(geometry, scale);
    let quads = [quads.next(), quads.next()];
    let paint = |window: &mut Window| {
        for bounds in quads.iter().flatten() {
            window.paint_quad(gpui::fill(*bounds, rgb(centerline)));
        }
    };
    if let Some(layer) = quads[0]
        .zip(quads[1])
        .and_then(|(a, b)| disjoint_layer(a, b))
    {
        window.paint_layer(layer, paint);
    } else {
        paint(window);
    }
}
fn paint_channels(geometry: &WaveformGeometry, color: u32, window: &mut Window) {
    let paint = |window: &mut Window| {
        for channel in geometry.channels() {
            window.paint_path(channel.envelope.clone(), rgb(color));
            if let Some(path) = &channel.zero_ranges {
                window.paint_path(path.clone(), rgb(color));
            }
        }
    };
    if let Some(layer) = envelope_layer(geometry, window.scale_factor()) {
        window.paint_layer(layer, paint);
    } else {
        paint(window);
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
                let Some(data) = prepare(WaveformViewport {
                    bounds,
                    scale,
                    mask: window.content_mask().bounds,
                }) else {
                    return;
                };
                let geometry = data.deref().as_ref();
                // Timeline geometry deliberately includes neighboring columns. Its
                // existing ancestor mask, not another canvas-bound mask, crops them.
                paint_guides(geometry, colors.centerline, scale, window);
                if emphasis == WaveformEmphasis::Uniform {
                    paint_channels(geometry, colors.waveform, window);
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
                        paint_channels(geometry, color, window);
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
