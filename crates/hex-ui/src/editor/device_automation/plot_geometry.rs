//! Visible-range selection and pixel-column reduction for automation lane
//! polylines.
//!
//! Painting borrows the displayed lane and binary-searches its visible frame
//! range, keeping one neighbour at each edge so clipped edge segments still
//! interpolate. Dense lanes can hold many more points than the plot has
//! physical pixel columns: [`reduce`] expands held (Step/discrete) segments
//! into explicit vertices and then keeps, per column, the first, lowest,
//! highest and last vertex in their original order. Lines between
//! consecutive kept vertices cover the same vertical extent in every column
//! as the unreduced polyline, so the painted line is unchanged at pixel
//! resolution while its vertex count is bounded by four per column.
//!
//! Markers are kept for retained source points plus every selected point, so
//! selection feedback survives the reduction. Unselected markers of dropped
//! points lie inside their column's retained extent and are not repainted.
//! This module owns no GPUI state; it takes positions in window pixels.

use std::ops::Range;

/// Indices of the points whose frames intersect `start..=end`, plus one
/// neighbour on each side. Points must be ordered by frame. A point whose
/// frame cannot be resolved counts as neither before `start` nor within
/// `end`, as the former paint search did. Each bound is a binary search:
/// only `O(log n)` points are converted, never the whole lane.
pub(super) fn visible_range<T>(
    points: &[T],
    start: u64,
    end: u64,
    frame: impl Fn(&T) -> Option<u64>,
) -> Range<usize> {
    let first = points
        .partition_point(|point| frame(point).is_some_and(|frame| frame < start))
        .saturating_sub(1);
    let last = points
        .partition_point(|point| frame(point).is_some_and(|frame| frame <= end))
        .saturating_add(1)
        .min(points.len());
    first..last.max(first)
}

/// One displayed automation point in window logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PlotPoint {
    pub(super) x: f32,
    pub(super) y: f32,
    /// The segment leaving this point holds its value (Step interpolation).
    pub(super) step: bool,
    pub(super) selected: bool,
}

/// A reduced lane: the polyline vertices and the markers to paint, both in
/// window logical pixels. `markers` carries whether each point is selected.
#[derive(Debug, Default, PartialEq)]
pub(super) struct PlotPaths {
    pub(super) line: Vec<(f32, f32)>,
    pub(super) markers: Vec<(f32, f32, bool)>,
}

#[derive(Clone, Copy)]
struct Vertex {
    order: usize,
    x: f32,
    y: f32,
    /// `Some(selected)` for a source point, `None` for a step corner.
    marker: Option<bool>,
}

struct Column {
    key: f32,
    first: Vertex,
    low: Vertex,
    high: Vertex,
    last: Vertex,
}

struct Reducer {
    scale: f32,
    order: usize,
    column: Option<Column>,
    paths: PlotPaths,
}

impl Reducer {
    fn push(&mut self, x: f32, y: f32, marker: Option<bool>) {
        let vertex = Vertex {
            order: self.order,
            x,
            y,
            marker,
        };
        self.order += 1;
        if marker == Some(true) {
            self.paths.markers.push((x, y, true));
        }
        // Columns are physical pixels in window space; non-finite keys never
        // compare equal, so such vertices stay their own column.
        let key = (x * self.scale).floor();
        match self.column.as_mut() {
            Some(column) if column.key == key => {
                if y < column.low.y {
                    column.low = vertex;
                }
                if y > column.high.y {
                    column.high = vertex;
                }
                column.last = vertex;
            }
            _ => {
                self.flush();
                self.column = Some(Column {
                    key,
                    first: vertex,
                    low: vertex,
                    high: vertex,
                    last: vertex,
                });
            }
        }
    }

    fn flush(&mut self) {
        let Some(column) = self.column.take() else {
            return;
        };
        let mut kept = [column.first, column.low, column.high, column.last];
        kept.sort_unstable_by_key(|vertex| vertex.order);
        let mut previous = None;
        for vertex in kept {
            if previous == Some(vertex.order) {
                continue;
            }
            previous = Some(vertex.order);
            self.paths.line.push((vertex.x, vertex.y));
            if vertex.marker == Some(false) {
                self.paths.markers.push((vertex.x, vertex.y, false));
            }
        }
    }
}

/// Reduce ordered points to at most four line vertices per physical pixel
/// column at `scale` (window scale factor). `discrete` holds every segment.
/// Makes one pass with no intermediate buffers; the outputs are bounded by
/// the visible columns plus the selected markers.
pub(super) fn reduce(
    points: impl IntoIterator<Item = PlotPoint>,
    discrete: bool,
    scale: f32,
) -> PlotPaths {
    let points = points.into_iter();
    let capacity = points.size_hint().0.min(4096);
    let mut reducer = Reducer {
        scale,
        order: 0,
        column: None,
        paths: PlotPaths {
            line: Vec::with_capacity(capacity),
            markers: Vec::with_capacity(capacity),
        },
    };
    let mut held: Option<f32> = None;
    for point in points {
        if let Some(y) = held {
            reducer.push(point.x, y, None);
        }
        reducer.push(point.x, point.y, Some(point.selected));
        held = (point.step || discrete).then_some(point.y);
    }
    reducer.flush();
    reducer.paths
}

#[cfg(test)]
mod tests {
    use super::{PlotPoint, reduce, visible_range};
    use std::{cell::Cell, collections::BTreeMap};

    /// Unreduced reference polyline: every point, with held segments drawn as
    /// a horizontal run then a vertical jump, exactly as painting did before
    /// reduction.
    fn full_line(points: &[PlotPoint], discrete: bool) -> Vec<(f32, f32)> {
        let mut line = Vec::new();
        let mut held = None;
        for point in points {
            if let Some(y) = held {
                line.push((point.x, y));
            }
            line.push((point.x, point.y));
            held = (point.step || discrete).then_some(point.y);
        }
        line
    }

    /// Vertical extent of a polyline in each physical pixel column at
    /// `scale`, including the values where segments cross column boundaries.
    fn raster(polyline: &[(f32, f32)], scale: f32) -> BTreeMap<i64, (f32, f32)> {
        let mut columns: BTreeMap<i64, (f32, f32)> = BTreeMap::new();
        let mut cover = |x: f32, y: f32| {
            let entry = columns.entry(x.floor() as i64).or_insert((y, y));
            entry.0 = entry.0.min(y);
            entry.1 = entry.1.max(y);
        };
        for pair in polyline.windows(2) {
            let [(x0, y0), (x1, y1)] = [pair[0], pair[1]];
            let (x0, x1) = (x0 * scale, x1 * scale);
            cover(x0, y0);
            let mut boundary = x0.floor() + 1.0;
            while boundary <= x1 {
                let y = y0 + (y1 - y0) * (boundary - x0) / (x1 - x0);
                // A crossing closes the left column and opens the right one.
                cover(boundary - 1.0e-3, y);
                cover(boundary, y);
                boundary += 1.0;
            }
            cover(x1, y1);
        }
        columns
    }

    fn assert_same_pixels(full: &[(f32, f32)], reduced: &[(f32, f32)], scale: f32) {
        let (full, reduced) = (raster(full, scale), raster(reduced, scale));
        assert_eq!(
            full.keys().collect::<Vec<_>>(),
            reduced.keys().collect::<Vec<_>>()
        );
        for (column, (low, high)) in &full {
            let (reduced_low, reduced_high) = reduced[column];
            assert!((low - reduced_low).abs() < 1.0e-3, "column {column}");
            assert!((high - reduced_high).abs() < 1.0e-3, "column {column}");
        }
    }

    /// 1,024 mixed linear/Step points over `width` logical pixels from `left`.
    fn dense(left: f32, width: f32) -> Vec<PlotPoint> {
        (0..1024)
            .map(|index| PlotPoint {
                x: left + index as f32 * width / 1024.0,
                y: 20.0 + 15.0 * (index as f32 * 0.37).sin() + ((index * 37) % 11) as f32,
                step: index % 3 == 0,
                selected: index == 500,
            })
            .collect()
    }

    #[test]
    fn dense_lane_is_bounded_by_width_and_keeps_its_pixel_shape() {
        const WIDTH: f32 = 200.0;
        for discrete in [false, true] {
            for (scale, left) in [(1.0_f32, 0.0_f32), (2.0, 0.37), (1.5, 13.25)] {
                let mut points = vec![PlotPoint {
                    x: left - 35.0,
                    y: 10.0,
                    step: false,
                    selected: false,
                }];
                points.extend(dense(left, WIDTH));
                points.push(PlotPoint {
                    x: left + WIDTH + 40.0,
                    y: 30.0,
                    step: true,
                    selected: false,
                });
                let full = full_line(&points, discrete);
                let paths = reduce(points.iter().copied(), discrete, scale);
                // Visible physical columns plus the two edge neighbours' own.
                let columns = (WIDTH * scale).ceil() as usize + 3;
                assert!(paths.line.len() <= 4 * columns, "{}", paths.line.len());
                assert!(paths.line.len() < full.len());
                assert_eq!(paths.line.first(), full.first());
                assert_eq!(paths.line.last(), full.last());
                assert!(paths.line.windows(2).all(|pair| pair[0].0 <= pair[1].0));
                assert_same_pixels(&full, &paths.line, scale);
                // The selected point survives reduction with its emphasis.
                let selected = points[501];
                assert!(paths.markers.contains(&(selected.x, selected.y, true)));
                assert_eq!(paths.markers.iter().filter(|marker| marker.2).count(), 1);
                assert!(paths.markers.len() <= paths.line.len() + 1);
            }
        }
    }

    #[test]
    fn mixed_step_and_linear_neighbours_keep_their_interpolation() {
        // A dropped interior point carries a Step; the retained segment after
        // it must still hold rather than becoming linear.
        let points: Vec<_> = [
            (0.1, 2.0, true),
            (0.2, 8.0, false),
            (0.3, 3.0, true),
            (0.4, 9.0, false),
            (0.5, 4.0, false),
            (3.0, 7.0, false),
        ]
        .into_iter()
        .map(|(x, y, step)| PlotPoint {
            x,
            y,
            step,
            selected: false,
        })
        .collect();
        for discrete in [false, true] {
            let full = full_line(&points, discrete);
            let paths = reduce(points.iter().copied(), discrete, 1.0);
            assert_same_pixels(&full, &paths.line, 1.0);
        }
    }

    #[test]
    fn sparse_and_degenerate_lanes_keep_every_vertex_and_marker() {
        assert_eq!(reduce([], false, 1.0), Default::default());
        let sparse: Vec<_> = [(0.5, 1.0), (10.0, 2.0), (20.5, 0.0)]
            .into_iter()
            .map(|(x, y)| PlotPoint {
                x,
                y,
                step: false,
                selected: false,
            })
            .collect();
        let paths = reduce(sparse.iter().copied(), false, 2.0);
        assert_eq!(paths.line, full_line(&sparse, false));
        assert_eq!(
            paths.markers,
            [(0.5, 1.0, false), (10.0, 2.0, false), (20.5, 0.0, false)]
        );
        let non_finite = [f32::NAN, f32::NAN, 3.0].map(|x| PlotPoint {
            x,
            y: 1.0,
            step: false,
            selected: false,
        });
        assert_eq!(reduce(non_finite, false, 1.0).line.len(), 3);
    }

    #[test]
    fn visible_range_binary_searches_and_keeps_one_edge_neighbour() {
        let frames: Vec<u64> = (0..1024).map(|index| index * 100).collect();
        let calls = Cell::new(0_usize);
        let frame = |frame: &u64| {
            calls.set(calls.get() + 1);
            Some(*frame)
        };
        // Frames 20_000..=30_000 cover points 200..=300, plus 199 and 301.
        assert_eq!(visible_range(&frames, 20_000, 30_000, frame), 199..302);
        // Two binary searches over 1,024 points, not a traversal.
        assert!(calls.get() <= 2 * 11, "{} conversions", calls.get());
        assert_eq!(visible_range(&frames, 0, 50, |frame| Some(*frame)), 0..2);
        assert_eq!(
            visible_range(&frames, 200_000, 300_000, |frame| Some(*frame)),
            1023..1024
        );
        assert_eq!(visible_range::<u64>(&[], 0, 10, |frame| Some(*frame)), 0..0);
    }

    /// Manual release profile of paint-path construction for a dense lane:
    /// `cargo test -p hex-ui --release --lib plot_geometry -- --ignored --nocapture`.
    /// "before" builds the unreduced line and one marker per point as painting
    /// did before #4097; "after" includes reduction. Excludes `paint_path`.
    #[test]
    #[ignore = "manual optimized paint-path construction profile"]
    fn dense_lane_paint_path_construction_profile() {
        use gpui::{PathBuilder, point, px};
        use std::{hint::black_box, time::Instant};
        if cfg!(debug_assertions) {
            eprintln!("timing requires --release; skipped");
            return;
        }
        let points = dense(0.0, 200.0);
        let build = |line: &[(f32, f32)], markers: &mut dyn Iterator<Item = (f32, f32, bool)>| {
            let mut path = PathBuilder::stroke(px(1.5));
            path.move_to(point(px(0.0), px(line[0].1)));
            for &(x, y) in line {
                path.line_to(point(px(x), px(y)));
            }
            black_box(path.build().ok());
            for (x, y, selected) in markers {
                let mut marker = PathBuilder::stroke(px(if selected { 3.0 } else { 2.0 }));
                marker.move_to(point(px(x - 2.0), px(y)));
                marker.line_to(point(px(x + 2.0), px(y)));
                black_box(marker.build().ok());
            }
        };
        let measure = |label: &str, run: &dyn Fn()| {
            for _ in 0..20 {
                run();
            }
            let iterations = 200;
            let started = Instant::now();
            for _ in 0..iterations {
                run();
            }
            let micros = started.elapsed().as_secs_f64() * 1.0e6 / f64::from(iterations);
            println!("{label}: {micros:.1} us/paint");
        };
        measure("before (unreduced, marker per point)", &|| {
            let line = full_line(black_box(&points), false);
            build(
                &line,
                &mut points
                    .iter()
                    .map(|point| (point.x, point.y, point.selected)),
            );
        });
        for scale in [1.0, 2.0] {
            measure(&format!("after (reduced, scale {scale})"), &|| {
                let paths = reduce(black_box(&points).iter().copied(), false, scale);
                build(&paths.line, &mut paths.markers.iter().copied());
            });
            let full = full_line(&points, false).len();
            let paths = reduce(points.iter().copied(), false, scale);
            println!(
                "scale {scale}: vertices {full} -> {}; markers {} -> {}",
                paths.line.len(),
                points.len(),
                paths.markers.len()
            );
        }
    }
}
