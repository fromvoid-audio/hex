//! Scalar scrollbar track and thumb geometry shared by the UI and unit harness.
use gpui::{Bounds, Pixels, point, px, size};

#[derive(Clone, Copy)]
pub(super) struct Geometry {
    pub(super) generation: u64,
    pub(super) rows: usize,
    pub(super) track: Bounds<Pixels>,
    pub(super) thumb: Bounds<Pixels>,
    pub(super) maximum: f32,
}
impl Geometry {
    pub(super) fn new(
        bounds: Bounds<Pixels>,
        viewport: f32,
        maximum: f32,
        offset: f32,
        rows: usize,
        generation: u64,
    ) -> Option<Self> {
        let x = f32::from(bounds.left());
        let y = f32::from(bounds.top());
        let width = f32::from(bounds.size.width) - 4.0;
        let height = f32::from(bounds.size.height) - 4.0;
        if rows == 0
            || [x, y, width, height, viewport, maximum, offset]
                .iter()
                .any(|n| !n.is_finite())
            || width <= 0.0
            || height <= 0.0
            || viewport <= 0.0
            || maximum < 0.0
        {
            return None;
        }
        let total = viewport + maximum;
        if !total.is_finite() {
            return None;
        }
        if !(x + 2.0 + width).is_finite() || !(y + 2.0 + height).is_finite() {
            return None;
        }
        let length = (height * (viewport / total)).clamp(20.0_f32.min(height), height);
        let travel = height - length;
        let ratio = if maximum > 0.0 {
            (-offset / maximum).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let track = Bounds::new(point(px(x + 2.0), px(y + 2.0)), size(px(width), px(height)));
        let thumb = Bounds::new(
            point(track.left(), track.top() + px(travel * ratio)),
            size(track.size.width, px(length)),
        );
        Some(Self {
            generation,
            rows,
            track,
            thumb,
            maximum,
        })
    }

    pub(super) fn position(self, pointer: f32, grab: f32) -> Option<f32> {
        let travel = f32::from(self.track.size.height - self.thumb.size.height);
        if !pointer.is_finite() || !grab.is_finite() || self.maximum <= 0.0 || travel <= 0.0 {
            return None;
        }
        let top = pointer
            - f32::from(self.track.top())
            - grab.clamp(0.0, 1.0) * f32::from(self.thumb.size.height);
        Some((top / travel).clamp(0.0, 1.0) * self.maximum)
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::Geometry;
    use gpui::{Bounds, point, px, size};

    #[test]
    fn thumb_geometry_clamps_large_lists_and_refuses_invalid_or_empty_extents() {
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(12.0), px(204.0)));
        assert!(Geometry::new(bounds, 200.0, 0.0, 0.0, 0, 1).is_none());
        for maximum in [f32::NAN, f32::INFINITY, -1.0] {
            assert!(Geometry::new(bounds, 200.0, maximum, 0.0, 1000, 1).is_none());
        }
        let geometry = Geometry::new(bounds, 200.0, 2_000_000.0, -2_000_000.0, 100_000, 1).unwrap();
        assert_eq!(geometry.thumb.size.height, px(20.0));
        assert_eq!(geometry.thumb.bottom(), geometry.track.bottom());
        assert_eq!(geometry.position(-1000.0, 0.5), Some(0.0));
        assert_eq!(geometry.position(1000.0, 0.5), Some(geometry.maximum));
        assert!(geometry.position(f32::NAN, 0.5).is_none());
        let fitting = Geometry::new(bounds, 200.0, 0.0, 0.0, 3, 1).unwrap();
        assert!(fitting.position(100.0, 0.5).is_none());
    }
}
