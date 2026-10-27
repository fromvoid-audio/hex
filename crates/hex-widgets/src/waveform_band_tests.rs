use super::*;
use lyon::tessellation::{BuffersBuilder, FillOptions, FillTessellator, FillVertex, VertexBuffers};

type Triangle = [[f64; 2]; 3];

fn direct(upper: &[Point<Pixels>], lower: &[Point<Pixels>]) -> Vec<Triangle> {
    let mut triangles = Vec::new();
    band_triangles(upper, lower, |points| {
        triangles.push(points.map(|p| [f64::from(f32::from(p.x)), f64::from(f32::from(p.y))]));
    });
    triangles
}

// Same polygon, fill options and pinned tessellator used by the previous GPUI
// PathBuilder::fill. Its triangulation is independent of the band producer.
fn legacy(upper: &[Point<Pixels>], lower: &[Point<Pixels>]) -> Vec<Triangle> {
    legacy_with_options(upper, lower, FillOptions::default())
}

fn legacy_with_options(
    upper: &[Point<Pixels>],
    lower: &[Point<Pixels>],
    options: FillOptions,
) -> Vec<Triangle> {
    let mut polygon = lyon::path::Path::builder();
    polygon.begin(upper[0].into());
    for p in upper.iter().skip(1).chain(lower.iter().rev()) {
        polygon.line_to((*p).into());
    }
    polygon.close();
    let mut buffers: VertexBuffers<lyon::math::Point, u16> = VertexBuffers::new();
    FillTessellator::new()
        .tessellate_path(
            &polygon.build(),
            &options,
            &mut BuffersBuilder::new(&mut buffers, |v: FillVertex| v.position()),
        )
        .unwrap();
    buffers
        .indices
        .chunks_exact(3)
        .map(|indices| {
            std::array::from_fn(|i| {
                let p = buffers.vertices[usize::from(indices[i])];
                [f64::from(p.x), f64::from(p.y)]
            })
        })
        .collect()
}

fn area(polygon: &[[f64; 2]]) -> f64 {
    polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .take(polygon.len())
        .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
        .sum::<f64>()
        .abs()
        * 0.5
}

// Clip to each cell before integrating: equal total area alone would miss
// displaced regions, gaps and overlapping triangles that compensate elsewhere.
fn clipped_area<const N: usize>(triangles: &[[[f64; 2]; N]], rect: [f64; 4]) -> f64 {
    triangles
        .iter()
        .map(|triangle| {
            let mut polygon = triangle.to_vec();
            for (axis, boundary, sign) in [
                (0, rect[0], 1.0),
                (1, rect[1], 1.0),
                (0, rect[2], -1.0),
                (1, rect[3], -1.0),
            ] {
                let mut clipped = Vec::new();
                for (a, b) in polygon
                    .iter()
                    .zip(polygon.iter().cycle().skip(1))
                    .take(polygon.len())
                {
                    let inside_a = (a[axis] - boundary) * sign >= 0.0;
                    let inside_b = (b[axis] - boundary) * sign >= 0.0;
                    if inside_a {
                        clipped.push(*a);
                    }
                    if inside_a != inside_b {
                        let t = (boundary - a[axis]) / (b[axis] - a[axis]);
                        clipped.push([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]);
                    }
                }
                polygon = clipped;
            }
            area(&polygon)
        })
        .sum()
}

fn bounds(triangles: &[Triangle]) -> Option<[f64; 4]> {
    triangles.iter().flatten().copied().fold(None, |bounds, p| {
        Some(bounds.map_or([p[0], p[1], p[0], p[1]], |b: [f64; 4]| {
            [
                b[0].min(p[0]),
                b[1].min(p[1]),
                b[2].max(p[0]),
                b[3].max(p[1]),
            ]
        }))
    })
}

#[test]
fn exact_band_matches_legacy_coverage_in_clipped_cells_at_both_scales() {
    // Irregular positions, duplicate x, touching bands, DC, negative/fractional
    // coordinates, and the maximum host budget. No random test seed or timing.
    for count in [2, 9, MAX_WAVEFORM_ENVELOPE_POINTS] {
        for scale in [1.0, 2.0] {
            let mut x = -4.25;
            let mut upper = Vec::new();
            let mut lower = Vec::new();
            for i in 0..count {
                x += if i % 7 == 0 {
                    0.0
                } else {
                    (i % 3 + 1) as f32 * 0.25
                };
                let y = (i % 11) as f32 * 0.375 - 3.0;
                let height = if i % 5 == 0 {
                    0.0
                } else {
                    (i % 4 + 1) as f32 * 0.5
                };
                upper.push(gpui::point(px(x * scale), px(y * scale)));
                lower.push(gpui::point(px(x * scale), px((y + height) * scale)));
            }
            let actual = direct(&upper, &lower);
            let quadrilaterals: Vec<[[f64; 2]; 4]> = upper
                .windows(2)
                .zip(lower.windows(2))
                .map(|(u, l)| {
                    [u[0], u[1], l[1], l[0]]
                        .map(|p| [f64::from(f32::from(p.x)), f64::from(f32::from(p.y))])
                })
                .collect();
            // Default Lyon can merge near intersection events within 0.05px.
            // Use a tight independent oracle for the adversarial maximum band;
            // small fixtures additionally establish default-path equivalence.
            let expected = legacy_with_options(&upper, &lower, FillOptions::tolerance(0.000001));
            if count < 10 {
                assert_eq!(legacy(&upper, &lower), expected);
            }
            assert_eq!(bounds(&actual), bounds(&expected));
            for t in &actual {
                assert!(
                    (t[1][0] - t[0][0]) * (t[2][1] - t[0][1])
                        - (t[1][1] - t[0][1]) * (t[2][0] - t[0][0])
                        > 0.0
                );
            }
            let b = bounds(&actual).unwrap();
            for row in 0..8 {
                for column in 0..16 {
                    let x = b[0] + (b[2] - b[0]) * f64::from(column) / 16.0;
                    let y = b[1] + (b[3] - b[1]) * f64::from(row) / 8.0;
                    let rect = [x, y, x + (b[2] - b[0]) / 16.0, y + (b[3] - b[1]) / 8.0];
                    let a = clipped_area(&actual, rect);
                    let e = clipped_area(&expected, rect);
                    let band = clipped_area(&quadrilaterals, rect);
                    assert!(
                        (a - band).abs() < 1e-8 * band.max(1.0),
                        "original band coverage"
                    );
                    assert!(
                        (a - e).abs() < 1e-5 * e.max(1.0),
                        "{count} {scale} {rect:?}: {a} != {e}"
                    );
                    assert!(
                        a <= (rect[2] - rect[0]) * (rect[3] - rect[1]) + 1e-6,
                        "overlapping interiors"
                    );
                }
            }
        }
    }
}

#[test]
fn finite_band_bounds_and_subpixel_area_are_preserved() {
    let make = |x, y| gpui::point(px(x), px(y));
    let upper = [make(0.0, -f32::MAX), make(1.0, f32::MAX)];
    assert!(
        WaveformChannel::new(
            WaveformEnvelope {
                upper: &upper,
                lower: &upper,
                mode: WaveformEnvelopeMode::Filled,
                trace_width: px(1.0),
                centerline: None,
            },
            []
        )
        .is_none()
    );
    let upper = [make(0.0, 0.0), make(f32::MIN_POSITIVE, 0.0)];
    let lower = [
        make(0.0, f32::MIN_POSITIVE),
        make(f32::MIN_POSITIVE, f32::MIN_POSITIVE),
    ];
    assert!(direct(&upper, &lower).iter().map(|t| area(t)).sum::<f64>() > 0.0);
    let repeated = [make(1.0, 0.0), make(1.0, 0.0)];
    assert!(direct(&repeated, &[make(1.0, 2.0), make(1.0, 3.0)]).is_empty());
}

#[test]
fn zero_area_tails_do_not_expand_filled_geometry() {
    let upper = [
        (-100.0, 2.0),
        (-90.0, 2.0),
        (1.0, 2.0),
        (2.0, 2.0),
        (3.0, 2.0),
        (90.0, 2.0),
        (100.0, 2.0),
    ]
    .map(|(x, y)| gpui::point(px(x), px(y)));
    let mut lower = upper;
    lower[3].y = px(5.0);
    assert_eq!(bounds(&direct(&upper, &lower)), Some([1.0, 2.0, 3.0, 5.0]));
    assert_eq!(
        bounds(&direct(&upper, &lower)),
        bounds(&legacy(&upper, &lower))
    );
    assert!(direct(&upper, &upper).is_empty());
    assert!(legacy(&upper, &upper).is_empty());
}
