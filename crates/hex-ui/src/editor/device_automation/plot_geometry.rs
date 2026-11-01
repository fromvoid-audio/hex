//! Pixel-column reduction for automation lane polylines.
//!
//! Dense lanes can hold many more points than the plot has pixel columns.
//! Painting keeps, per column, the first, lowest, highest and last vertex in
//! their original order. Lines between consecutive kept vertices then cover
//! the same vertical extent in every column as the unreduced polyline, so the
//! painted shape is unchanged at pixel resolution while the vertex count is
//! bounded by four per column. Points must arrive ordered by `x`.

/// Indices of the vertices to paint, ascending, at most four per column.
/// A column is `floor(x)` in logical pixels relative to the plot origin;
/// non-finite positions are kept as their own vertices.
pub(super) fn column_vertices(positions: &[(f32, f32)]) -> Vec<usize> {
    let mut kept = Vec::with_capacity(positions.len().min(1024));
    let mut start = 0;
    while start < positions.len() {
        let column = positions[start].0.floor();
        let mut end = start + 1;
        while end < positions.len() && column.is_finite() && positions[end].0.floor() == column {
            end += 1;
        }
        let range = start..end;
        let lowest = range
            .clone()
            .min_by(|a, b| positions[*a].1.total_cmp(&positions[*b].1))
            .unwrap_or(start);
        let highest = range
            .clone()
            .max_by(|a, b| positions[*a].1.total_cmp(&positions[*b].1))
            .unwrap_or(start);
        let mut column_kept = [start, lowest, highest, end - 1];
        column_kept.sort_unstable();
        for index in column_kept {
            if kept.last() != Some(&index) {
                kept.push(index);
            }
        }
        start = end;
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::column_vertices;
    use std::collections::BTreeMap;

    /// Vertical extent of a polyline in each pixel column, including the
    /// interpolated values where segments cross column boundaries.
    fn raster(polyline: &[(f32, f32)]) -> BTreeMap<i64, (f32, f32)> {
        let mut columns: BTreeMap<i64, (f32, f32)> = BTreeMap::new();
        let mut cover = |x: f32, y: f32| {
            let entry = columns.entry(x.floor() as i64).or_insert((y, y));
            entry.0 = entry.0.min(y);
            entry.1 = entry.1.max(y);
        };
        for pair in polyline.windows(2) {
            let [(x0, y0), (x1, y1)] = [pair[0], pair[1]];
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

    #[test]
    fn dense_lane_is_bounded_by_width_and_keeps_its_pixel_shape() {
        const WIDTH: f32 = 200.0;
        // 1,024 points over a 200 px plot plus one edge neighbour each side.
        let mut positions = vec![(-35.0, 10.0)];
        positions.extend((0..1024).map(|index| {
            let x = index as f32 * WIDTH / 1024.0;
            let y = 20.0 + 15.0 * (index as f32 * 0.37).sin() + (index % 7) as f32;
            (x, y)
        }));
        positions.push((WIDTH + 40.0, 30.0));
        let kept = column_vertices(&positions);
        assert!(kept.len() <= 4 * (WIDTH as usize + 2), "{}", kept.len());
        assert!(kept.len() < positions.len());
        assert_eq!(kept.first(), Some(&0));
        assert_eq!(kept.last(), Some(&(positions.len() - 1)));
        assert!(kept.windows(2).all(|pair| pair[0] < pair[1]));
        let reduced: Vec<_> = kept.iter().map(|index| positions[*index]).collect();
        let (full, reduced) = (raster(&positions), raster(&reduced));
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

    #[test]
    fn sparse_and_degenerate_lanes_keep_every_vertex() {
        assert!(column_vertices(&[]).is_empty());
        let sparse = [(0.5, 1.0), (10.0, 2.0), (20.5, 0.0)];
        assert_eq!(column_vertices(&sparse), [0, 1, 2]);
        let non_finite = [(f32::NAN, 1.0), (f32::NAN, 2.0), (3.0, 0.0)];
        assert_eq!(column_vertices(&non_finite), [0, 1, 2]);
    }
}
