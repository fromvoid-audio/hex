//! Pure chamfer and miniature-edge geometry for shared section surfaces.

use gpui::{Bounds, Pixels, point, px, size};

/// Shared overview tier: a real cutout, never an opaque background patch.
pub(crate) const CHAMFER: f32 = 6.0;

pub(crate) fn cut(width: f32, height: f32, requested: f32) -> f32 {
    if [width, height, requested]
        .into_iter()
        .any(|v| !v.is_finite())
        || width <= 0.0
        || height <= 0.0
    {
        return 0.0;
    }
    requested.max(0.0).min(width / 2.0).min(height / 2.0)
}
/// Conservative right edge for a rectangular miniature ending at `bottom`.
pub(crate) fn miniature_right(width: f32, height: f32, chamfer: f32, bottom: f32) -> f32 {
    width - (bottom - (height - cut(width, height, chamfer))).max(0.0)
}

#[derive(Clone, Copy)]
pub(super) struct SectionFillRects {
    pub(super) upper: Bounds<Pixels>,
    pub(super) lower_left: Option<Bounds<Pixels>>,
    pub(super) cut: f32,
}

pub(super) fn fill_rects(bounds: Bounds<Pixels>, requested: f32) -> Option<SectionFillRects> {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return None;
    }
    let cut = cut(width, height, requested);
    let upper = Bounds::new(bounds.origin, size(bounds.size.width, px(height - cut)));
    let lower_left = (cut > 0.0).then(|| {
        Bounds::new(
            point(bounds.left(), bounds.top() + px(height - cut)),
            size(px(miniature_right(width, height, cut, height)), px(cut)),
        )
    });
    Some(SectionFillRects {
        upper,
        lower_left,
        cut,
    })
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    #[test]
    fn cuts_are_bounded_and_miniatures_stay_inside_the_polygon() {
        for width in [0.1, 1.0, 4.0, 12.0, 100.0] {
            for height in [1.0, 14.0, 40.0] {
                let c = cut(width, height, CHAMFER);
                assert!(c >= 0.0 && c <= width / 2.0 && c <= height / 2.0);
                assert_eq!(miniature_right(width, height, c, height - c), width);
                assert!((miniature_right(width, height, c, height) - (width - c)).abs() < 0.00001);
            }
        }
        for value in [f32::NAN, f32::INFINITY, -1.0, 0.0] {
            assert_eq!(cut(value, 20.0, CHAMFER), 0.0);
        }
    }
}
