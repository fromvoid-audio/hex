//! One bounded measured-spectrum geometry and painter for every plot owner.
//! Domain mapping, freshness and signal processing remain caller responsibilities.
//!
//! Owners paint three passes in order: a vertical-gradient [`Pass::Fill`] under
//! each curve, then (after their grid) a thin translucent [`Pass::Peak`] line for
//! any held-peak curve, then the crisp [`Pass::Trace`] line. Geometry is in
//! normalized plot coordinates, so curves stay stable at any plot size.
use gpui::{
    Bounds, ContentMask, Hsla, PathBuilder, Pixels, Point, Window, linear_color_stop,
    linear_gradient, point, px, rgb,
};

pub(super) const MAX_POINTS: usize = 256;
const MAX_LAYERS: usize = 257;
/// Stroke for the most prominent spectrum (EQ output, project master).
pub(super) const PRIMARY_TRACE_WIDTH: Pixels = px(1.25);
/// Stroke for supporting spectra (EQ input, project track layers).
pub(super) const SECONDARY_TRACE_WIDTH: Pixels = px(1.0);
const PEAK_WIDTH: Pixels = px(1.0);
const PEAK_OPACITY: f32 = 0.45;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pass {
    Fill,
    Peak,
    Trace,
}

#[derive(Clone, Copy)]
pub(super) struct Layer<'a> {
    pub points: &'a [[f32; 2]],
    /// Optional held-peak curve, painted only by [`Pass::Peak`].
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

#[derive(Clone, Copy, Debug)]
enum Command {
    Move([f32; 2]),
    Cubic(Segment),
    Line([f32; 2]),
    Close,
}
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

fn fill_alpha(layers: usize) -> f32 {
    // A bounded combined ink budget keeps even 257 overlapping layers subtle.
    // Float alpha retains small nonzero contributions without u8 quantization.
    (0.32 / layers.max(1) as f32).min(0.2)
}

pub(super) fn paint<'a, I>(plot: Bounds<Pixels>, layers: I, pass: Pass, window: &mut Window)
where
    I: Iterator<Item = Layer<'a>> + Clone,
{
    let count = layers.clone().take(MAX_LAYERS + 1).count();
    if count == 0
        || count > MAX_LAYERS
        || ![
            f32::from(plot.origin.x),
            f32::from(plot.origin.y),
            f32::from(plot.size.width),
            f32::from(plot.size.height),
            f32::from(plot.right()),
            f32::from(plot.bottom()),
        ]
        .iter()
        .all(|v| v.is_finite())
        || plot.size.width <= px(0.0)
        || plot.size.height <= px(0.0)
    {
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
    window.with_content_mask(Some(ContentMask { bounds: plot }), |window| {
        for layer in layers {
            let Some(points) = pass_points(&layer, pass).filter(|_| paintable(&layer, pass)) else {
                continue;
            };
            let mut path = match pass {
                Pass::Fill => PathBuilder::fill(),
                Pass::Peak => PathBuilder::stroke(PEAK_WIDTH),
                Pass::Trace => PathBuilder::stroke(layer.stroke_width),
            };
            #[cfg(test)]
            let mut submitted = Vec::new();
            commands(points, pass, |command| {
                #[cfg(test)]
                submitted.push(command);
                match command {
                    Command::Move(p) => path.move_to(map_point(plot, p)),
                    Command::Line(p) => path.line_to(map_point(plot, p)),
                    Command::Cubic(s) => path.cubic_bezier_to(
                        map_point(plot, s.end),
                        map_point(plot, s.a),
                        map_point(plot, s.b),
                    ),
                    Command::Close => path.close(),
                }
            });
            if let Ok(path) = path.build() {
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
