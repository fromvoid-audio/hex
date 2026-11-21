//! One bounded measured-spectrum geometry and painter for every plot owner.
//! Domain mapping, freshness and signal processing remain caller responsibilities.
//!
//! Owners paint three passes in order: a vertical-gradient [`Pass::Fill`] under
//! each curve, then (after their grid) a thin translucent [`Pass::Peak`] line for
//! any held-peak curve, then the crisp [`Pass::Trace`] line. Optional
//! [`paint_peak_ribbon`] renders a translucent fill between held-peak and live
//! curves. Geometry is built directly into [`gpui::Path`] triangle meshes from
//! conservative monotone Hermite splines so live 60+ fps updates avoid per-frame
//! Lyon tessellation overhead while staying calibrated at every plot size.
use gpui::{
    Bounds, ContentMask, Hsla, Path, Pixels, Point, Window, linear_color_stop, linear_gradient,
    point, px, rgb,
};

pub(super) const MAX_POINTS: usize = 256;
const MAX_LAYERS: usize = 257;
/// Stroke for the most prominent spectrum (EQ output, project master).
pub(super) const PRIMARY_TRACE_WIDTH: Pixels = px(1.5);
/// Stroke for supporting spectra (EQ input, project track layers).
pub(super) const SECONDARY_TRACE_WIDTH: Pixels = px(1.0);
const PEAK_WIDTH: Pixels = px(1.0);
const PEAK_OPACITY: f32 = 0.55;
const RIBBON_TOP_OPACITY: f32 = 0.14;
const RIBBON_BOTTOM_OPACITY: f32 = 0.02;
const SOLID_CONTENT_UV: Point<f32> = Point { x: 0.0, y: 1.0 };

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pass {
    Fill,
    Peak,
    Trace,
}

#[derive(Clone, Copy)]
pub(super) struct Layer<'a> {
    pub points: &'a [[f32; 2]],
    /// Optional held-peak curve, painted only by [`Pass::Peak`] and peak ribbons.
    pub peak: Option<&'a [[f32; 2]]>,
    pub color: u32,
    pub stroke_width: Pixels,
}

#[derive(Clone, Copy, Debug)]
struct Segment {
    end: [f32; 2],
    a: [f32; 2],
    b: [f32; 2],
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
enum Command {
    Move([f32; 2]),
    Cubic(Segment),
    Line([f32; 2]),
    Close,
}

#[cfg(test)]
fn commands(points: &[[f32; 2]], pass: Pass, mut emit: impl FnMut(Command)) {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return;
    };
    emit(Command::Move(*first));
    for i in 0..points.len() - 1 {
        emit(Command::Cubic(segment(points, i)));
    }
    if pass == Pass::Fill {
        emit(Command::Line([last[0], 0.0]));
        emit(Command::Line([first[0], 0.0]));
        emit(Command::Close);
    }
}

fn map_point(plot: Bounds<Pixels>, p: [f32; 2]) -> Point<Pixels> {
    point(
        plot.left() + plot.size.width * p[0],
        plot.top() + plot.size.height * (1.0 - p[1]),
    )
}

fn valid_points(points: &[[f32; 2]]) -> bool {
    (2..=MAX_POINTS).contains(&points.len())
        && points
            .iter()
            .all(|p| p.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)))
        && points.windows(2).all(|p| p[0][0] <= p[1][0])
}

/// The curve a pass strokes or fills for one layer.
fn pass_points<'a>(layer: &Layer<'a>, pass: Pass) -> Option<&'a [[f32; 2]]> {
    match pass {
        Pass::Fill | Pass::Trace => Some(layer.points),
        Pass::Peak => layer.peak,
    }
}

fn paintable(layer: &Layer<'_>, pass: Pass) -> bool {
    pass_points(layer, pass).is_some_and(valid_points)
        && layer.color <= 0xffffff
        && f32::from(layer.stroke_width).is_finite()
        && (0.0..=4.0).contains(&f32::from(layer.stroke_width))
        && (pass != Pass::Fill || layer.points.iter().any(|p| p[1] != 0.0))
}

fn secant(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = b[0] - a[0];
    if dx <= f32::EPSILON {
        return 0.0;
    }
    let slope = (b[1] - a[1]) / dx;
    if slope.is_finite() { slope } else { 0.0 }
}

fn tangent(points: &[[f32; 2]], i: usize) -> f32 {
    if i == 0 {
        return secant(points[0], points[1]);
    }
    if i + 1 == points.len() {
        return secant(points[i - 1], points[i]);
    }
    let left = secant(points[i - 1], points[i]);
    let right = secant(points[i], points[i + 1]);
    if left.signum() != right.signum() {
        0.0
    } else {
        left.signum() * left.abs().min(right.abs())
    }
}

/// Conservative monotone Hermite controls: exact samples, continuous tangents,
/// and every control inside its segment's endpoint envelope. The convex hull
/// therefore cannot invent a peak, overshoot the floor or cross back in X.
fn segment(points: &[[f32; 2]], i: usize) -> Segment {
    let start = points[i];
    let end = points[i + 1];
    let third = (end[0] - start[0]) / 3.0;
    let low = start[1].min(end[1]);
    let high = start[1].max(end[1]);
    Segment {
        end,
        a: [
            start[0] + third,
            (start[1] + tangent(points, i) * third).clamp(low, high),
        ],
        b: [
            end[0] - third,
            (end[1] - tangent(points, i + 1) * third).clamp(low, high),
        ],
    }
}

#[inline]
fn eval_cubic(start: [f32; 2], seg: Segment, t: f32) -> [f32; 2] {
    let u = 1.0 - t;
    let u2 = u * u;
    let t2 = t * t;
    let w0 = u2 * u;
    let w1 = 3.0 * u2 * t;
    let w2 = 3.0 * u * t2;
    let w3 = t2 * t;
    [
        w0 * start[0] + w1 * seg.a[0] + w2 * seg.b[0] + w3 * seg.end[0],
        w0 * start[1] + w1 * seg.a[1] + w2 * seg.b[1] + w3 * seg.end[1],
    ]
}

/// Adaptive screen-space sampling of the monotone Hermite curve into `out`.
/// Keeps smooth spline fidelity on wide segments while avoiding oversampling
/// sub-pixel bands.
fn sample_curve(points: &[[f32; 2]], plot: Bounds<Pixels>, out: &mut Vec<Point<Pixels>>) {
    out.clear();
    let Some(&first) = points.first() else {
        return;
    };
    let plot_w = f32::from(plot.size.width);
    out.push(map_point(plot, first));
    for i in 0..points.len() - 1 {
        let start = points[i];
        let seg = segment(points, i);
        let dx_px = (seg.end[0] - start[0]) * plot_w;
        let steps: usize = if dx_px <= 3.0 {
            1
        } else if dx_px <= 10.0 {
            2
        } else {
            4
        };
        if steps == 1 {
            out.push(map_point(plot, seg.end));
        } else {
            let inv = 1.0 / steps as f32;
            for step in 1..steps {
                let p = eval_cubic(start, seg, step as f32 * inv);
                out.push(map_point(plot, p));
            }
            out.push(map_point(plot, seg.end));
        }
    }
}

/// Direct O(N) heightfield triangulation from curve down to `baseline_y`.
fn build_fill_path(samples: &[Point<Pixels>], baseline_y: Pixels) -> Option<Path<Pixels>> {
    let mut path: Option<Path<Pixels>> = None;
    let uvs = (SOLID_CONTENT_UV, SOLID_CONTENT_UV, SOLID_CONTENT_UV);
    for pair in samples.windows(2) {
        let p0 = pair[0];
        let p1 = pair[1];
        if p0.x == p1.x || (p0.y >= baseline_y && p1.y >= baseline_y) {
            continue;
        }
        let b0 = point(p0.x, baseline_y);
        let b1 = point(p1.x, baseline_y);
        let target = path.get_or_insert_with(|| Path::new(p0));
        if p0.y < baseline_y {
            target.push_triangle((p0, p1, b0), uvs);
        }
        if p1.y < baseline_y {
            target.push_triangle((p1, b1, b0), uvs);
        }
    }
    path
}

/// Direct O(N) miter-clamped stroke mesh along a sampled polyline.
fn build_stroke_path(samples: &[Point<Pixels>], width: Pixels) -> Option<Path<Pixels>> {
    let half = f32::from(width) * 0.5;
    if samples.len() < 2 || !half.is_finite() || half <= 0.0 {
        return None;
    }
    // Filter consecutive duplicate points so segment normals stay well-defined.
    let mut pts: Vec<(f32, f32)> = Vec::with_capacity(samples.len());
    for &p in samples {
        let xy = (f32::from(p.x), f32::from(p.y));
        if let Some(&prev) = pts.last() {
            let dx = xy.0 - prev.0;
            let dy = xy.1 - prev.1;
            if dx * dx + dy * dy < 1e-6 {
                continue;
            }
        }
        pts.push(xy);
    }
    if pts.len() < 2 {
        return None;
    }
    let seg_count = pts.len() - 1;
    let mut normals: Vec<(f32, f32)> = Vec::with_capacity(seg_count);
    for i in 0..seg_count {
        let dx = pts[i + 1].0 - pts[i].0;
        let dy = pts[i + 1].1 - pts[i].1;
        let len = (dx * dx + dy * dy).sqrt();
        if len > 1e-6 {
            normals.push((-dy / len, dx / len));
        } else {
            normals.push((0.0, -1.0));
        }
    }
    let max_miter = half * 2.0;
    let mut upper: Vec<Point<Pixels>> = Vec::with_capacity(pts.len());
    let mut lower: Vec<Point<Pixels>> = Vec::with_capacity(pts.len());
    for i in 0..pts.len() {
        let (ox, oy) = if i == 0 {
            let n = normals[0];
            (n.0 * half, n.1 * half)
        } else if i == seg_count {
            let n = normals[seg_count - 1];
            (n.0 * half, n.1 * half)
        } else {
            let n0 = normals[i - 1];
            let n1 = normals[i];
            let mx = n0.0 + n1.0;
            let my = n0.1 + n1.1;
            let mlen = (mx * mx + my * my).sqrt();
            if mlen < 1e-4 {
                (n1.0 * half, n1.1 * half)
            } else {
                let ux = mx / mlen;
                let uy = my / mlen;
                let denom = (ux * n1.0 + uy * n1.1).max(0.5);
                let scale = (half / denom).min(max_miter);
                (ux * scale, uy * scale)
            }
        };
        let (x, y) = pts[i];
        upper.push(point(px(x + ox), px(y + oy)));
        lower.push(point(px(x - ox), px(y - oy)));
    }
    let uvs = (SOLID_CONTENT_UV, SOLID_CONTENT_UV, SOLID_CONTENT_UV);
    let mut path = Path::new(upper[0]);
    for i in 0..seg_count {
        let u0 = upper[i];
        let u1 = upper[i + 1];
        let l0 = lower[i];
        let l1 = lower[i + 1];
        path.push_triangle((u0, u1, l0), uvs);
        path.push_triangle((u1, l1, l0), uvs);
    }
    Some(path)
}

/// Evaluate a monotone X-ordered curve linearly at normalized `x`.
fn eval_linear_y(points: &[[f32; 2]], x: f32) -> f32 {
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

/// Paint a translucent ribbon strictly between a layer's held-peak curve and
/// its live curve (`peak_y < live_y`), highlighting transient headroom.
pub(super) fn paint_peak_ribbon(plot: Bounds<Pixels>, layer: &Layer<'_>, window: &mut Window) {
    let Some(peak) = layer.peak else {
        return;
    };
    if !valid_points(layer.points)
        || !valid_points(peak)
        || layer.color > 0xffffff
        || !valid_plot(plot)
    {
        return;
    }
    let mut peak_samples = Vec::with_capacity(peak.len() * 2);
    sample_curve(peak, plot, &mut peak_samples);
    if peak_samples.len() < 2 {
        return;
    }
    let plot_left = f32::from(plot.left());
    let inv_w = 1.0 / f32::from(plot.size.width);
    let plot_top = f32::from(plot.top());
    let plot_h = f32::from(plot.size.height);
    let uvs = (SOLID_CONTENT_UV, SOLID_CONTENT_UV, SOLID_CONTENT_UV);
    let mut path: Option<Path<Pixels>> = None;
    let mut prev_top = peak_samples[0];
    let prev_x_norm = ((f32::from(prev_top.x) - plot_left) * inv_w).clamp(0.0, 1.0);
    let mut prev_bot = point(
        prev_top.x,
        px(plot_top + plot_h * (1.0 - eval_linear_y(layer.points, prev_x_norm))).max(prev_top.y),
    );
    for &top in &peak_samples[1..] {
        let x_norm = ((f32::from(top.x) - plot_left) * inv_w).clamp(0.0, 1.0);
        let bot = point(
            top.x,
            px(plot_top + plot_h * (1.0 - eval_linear_y(layer.points, x_norm))).max(top.y),
        );
        if top.x > prev_top.x && (prev_top.y < prev_bot.y || top.y < bot.y) {
            let target = path.get_or_insert_with(|| Path::new(prev_top));
            if prev_top.y < prev_bot.y {
                target.push_triangle((prev_top, top, prev_bot), uvs);
            }
            if top.y < bot.y {
                target.push_triangle((top, bot, prev_bot), uvs);
            }
        }
        prev_top = top;
        prev_bot = bot;
    }
    if let Some(path) = path {
        let color: Hsla = rgb(layer.color).into();
        window.with_content_mask(Some(ContentMask { bounds: plot }), |window| {
            window.paint_path(
                path,
                linear_gradient(
                    180.0,
                    linear_color_stop(color.opacity(RIBBON_TOP_OPACITY), 0.0),
                    linear_color_stop(color.opacity(RIBBON_BOTTOM_OPACITY), 1.0),
                ),
            );
        });
    }
}

fn fill_alpha(layers: usize) -> f32 {
    // A bounded combined ink budget keeps even 257 overlapping layers subtle.
    // Float alpha retains small nonzero contributions without u8 quantization.
    (0.32 / layers.max(1) as f32).min(0.2)
}

fn valid_plot(plot: Bounds<Pixels>) -> bool {
    [
        f32::from(plot.origin.x),
        f32::from(plot.origin.y),
        f32::from(plot.size.width),
        f32::from(plot.size.height),
        f32::from(plot.right()),
        f32::from(plot.bottom()),
    ]
    .iter()
    .all(|v| v.is_finite())
        && plot.size.width > px(0.0)
        && plot.size.height > px(0.0)
}

pub(super) fn paint<'a, I>(plot: Bounds<Pixels>, layers: I, pass: Pass, window: &mut Window)
where
    I: Iterator<Item = Layer<'a>> + Clone,
{
    let count = layers.clone().take(MAX_LAYERS + 1).count();
    if count == 0 || count > MAX_LAYERS || !valid_plot(plot) {
        return;
    }
    // Input admission counts every layer; only visible fills spend the ink budget.
    let alpha = fill_alpha(if pass == Pass::Fill {
        layers
            .clone()
            .filter(|layer| paintable(layer, pass))
            .count()
    } else {
        count
    });
    let baseline_y = plot.bottom();
    let mut samples: Vec<Point<Pixels>> = Vec::new();
    window.with_content_mask(Some(ContentMask { bounds: plot }), |window| {
        for layer in layers {
            let Some(points) = pass_points(&layer, pass).filter(|_| paintable(&layer, pass)) else {
                continue;
            };
            #[cfg(test)]
            let mut submitted = Vec::new();
            #[cfg(test)]
            commands(points, pass, |command| submitted.push(command));

            sample_curve(points, plot, &mut samples);
            let built = match pass {
                Pass::Fill => build_fill_path(&samples, baseline_y),
                Pass::Peak => build_stroke_path(&samples, PEAK_WIDTH),
                Pass::Trace => build_stroke_path(&samples, layer.stroke_width),
            };
            if let Some(path) = built {
                #[cfg(test)]
                tests::record(
                    plot,
                    window.content_mask().bounds,
                    layer,
                    pass,
                    alpha,
                    submitted,
                );
                let color: Hsla = rgb(layer.color).into();
                match pass {
                    Pass::Fill => window.paint_path(
                        path,
                        linear_gradient(
                            180.0,
                            linear_color_stop(color.opacity(alpha), 0.0),
                            linear_color_stop(color.opacity(0.0), 1.0),
                        ),
                    ),
                    Pass::Peak => window.paint_path(path, color.opacity(PEAK_OPACITY)),
                    Pass::Trace => window.paint_path(path, color),
                }
            }
        }
    });
}

#[cfg(test)]
mod tests;
