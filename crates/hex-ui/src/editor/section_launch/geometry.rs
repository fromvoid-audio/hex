//! Pure bounds shared by the launch affordance and pointer exclusion.
use gpui::{Bounds, Pixels, point, px, size};

pub(crate) const ACTION_SIZE: f32 = 12.0;
pub(crate) const ACTION_LEFT: f32 = 6.0;
pub(crate) const ACTION_TOP: f32 = 1.0;
/// One geometry contract for the painted action and the raw capture exclusion.
pub(crate) fn action_bounds(left: f32, top: f32, width: f32) -> Option<Bounds<Pixels>> {
    if !left.is_finite() || !top.is_finite() || !width.is_finite() || width < 26.0 {
        return None;
    }
    let x = left + ACTION_LEFT;
    let y = top + ACTION_TOP;
    (x.is_finite() && y.is_finite())
        .then(|| Bounds::new(point(px(x), px(y)), size(px(ACTION_SIZE), px(ACTION_SIZE))))
}
#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    #[test]
    fn action_bounds_keep_narrow_sections_and_header_edges_exact() {
        assert!(action_bounds(10.0, 20.0, 25.99).is_none());
        assert_eq!(
            action_bounds(10.0, 20.0, 26.0),
            Some(Bounds::new(
                point(px(16.0), px(21.0)),
                size(px(12.0), px(12.0))
            ))
        );
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(action_bounds(bad, 0.0, 100.0).is_none());
            assert!(action_bounds(0.0, bad, 100.0).is_none());
            assert!(action_bounds(0.0, 0.0, bad).is_none());
        }
    }
}
