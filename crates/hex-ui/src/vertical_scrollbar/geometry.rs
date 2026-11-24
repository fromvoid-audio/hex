//! Pure vertical scrollbar geometry and drag-lifecycle state shared by virtualized lists.
use gpui::{Bounds, Pixels, Point, point, px, size};

/// Track and thumb bounds computed from one scrollbar layout pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Geometry {
    pub(crate) generation: u64,
    pub(crate) rows: usize,
    pub(crate) track: Bounds<Pixels>,
    pub(crate) thumb: Bounds<Pixels>,
    pub(crate) maximum: f32,
}

/// Active pointer-capture drag state anchored to one generation and track layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Drag {
    pub(crate) generation: u64,
    pub(crate) track: Bounds<Pixels>,
    pub(crate) grab: f32,
}

impl Geometry {
    pub(crate) fn new(
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
        if !total.is_finite() || !(x + 2.0 + width).is_finite() || !(y + 2.0 + height).is_finite() {
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

    pub(crate) fn position(self, pointer: f32, grab: f32) -> Option<f32> {
        let travel = f32::from(self.track.size.height - self.thumb.size.height);
        if !pointer.is_finite() || !grab.is_finite() || self.maximum <= 0.0 || travel <= 0.0 {
            return None;
        }
        let top = pointer
            - f32::from(self.track.top())
            - grab.clamp(0.0, 1.0) * f32::from(self.thumb.size.height);
        Some((top / travel).clamp(0.0, 1.0) * self.maximum)
    }

    /// Compute the normalized grab anchor within the thumb (`0.5` when clicking the track).
    pub(crate) fn grab_anchor(self, position: Point<Pixels>) -> Option<f32> {
        let pointer = f32::from(position.y);
        if !pointer.is_finite() || !f32::from(position.x).is_finite() {
            return None;
        }
        let grab = if self.thumb.contains(&position) {
            ((pointer - f32::from(self.thumb.top())) / f32::from(self.thumb.size.height))
                .clamp(0.0, 1.0)
        } else {
            0.5
        };
        Some(grab)
    }

    /// True when this cached layout still belongs to `generation`, `rows`, and has scrollable range.
    pub(crate) fn is_scrollable_for(self, generation: u64, rows: usize) -> bool {
        self.generation == generation && self.rows == rows && self.maximum > 0.0
    }
}

impl Drag {
    /// Start a drag from a validated geometry and pointer position, returning the new drag anchor.
    pub(crate) fn begin(
        geometry: Option<Geometry>,
        position: Point<Pixels>,
        generation: u64,
        rows: usize,
    ) -> Option<Self> {
        let geometry = geometry.filter(|g| g.is_scrollable_for(generation, rows))?;
        let grab = geometry.grab_anchor(position)?;
        Some(Self {
            generation,
            track: geometry.track,
            grab,
        })
    }

    /// Resolve a drag move against the latest cached geometry.
    /// Returns `Ok(Some(offset))` for a valid scroll offset, `Ok(None)` for a non-finite pointer
    /// (keeping the drag alive), or `Err(())` when the generation/rows/track changed and the drag
    /// must be cancelled.
    pub(crate) fn resolve_move(
        self,
        geometry: Option<Geometry>,
        pointer: f32,
        generation: u64,
        rows: usize,
    ) -> Result<Option<f32>, ()> {
        let Some(geometry) = geometry.filter(|geometry| {
            self.generation == generation
                && geometry.generation == generation
                && geometry.rows == rows
                && geometry.track == self.track
        }) else {
            return Err(());
        };
        Ok(geometry.position(pointer, self.grab))
    }

    pub(crate) fn is_active_for(self, generation: u64, track: Bounds<Pixels>) -> bool {
        self.generation == generation && self.track == track
    }
}

#[cfg(all(test, any(feature = "ui-unit-tier", not(feature = "ui-test-support"))))]
mod tests {
    use super::*;

    #[test]
    fn thumb_sizing_and_clamping_handle_empty_fitting_and_large_lists() {
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(12.0), px(204.0)));
        assert!(Geometry::new(bounds, 200.0, 0.0, 0.0, 0, 1).is_none());
        for invalid in [f32::NAN, f32::INFINITY, -1.0] {
            assert!(Geometry::new(bounds, 200.0, invalid, 0.0, 100_000, 1).is_none());
        }
        let short = Geometry::new(bounds, 200.0, 0.0, 0.0, 3, 1).unwrap();
        assert_eq!((short.generation, short.rows), (1, 3));
        assert_eq!(short.thumb, short.track);
        assert!(short.position(100.0, 0.5).is_none());

        let long = Geometry::new(bounds, 200.0, 2_000_000.0, -2_000_000.0, 100_000, 1).unwrap();
        assert_eq!(long.thumb.size.height, px(20.0));
        assert_eq!(long.thumb.bottom(), long.track.bottom());
        assert_eq!(long.position(-1000.0, 0.5), Some(0.0));
        assert_eq!(long.position(1000.0, 0.5), Some(long.maximum));
        assert!(long.position(f32::NAN, 0.5).is_none());
    }

    #[test]
    fn drag_grab_delta_mapping_and_invalidation_cancel_cleanly() {
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(12.0), px(204.0)));
        // track y: 2..202 (height 200), viewport 200, maximum 200 => thumb height 100, travel 100
        let geometry = Geometry::new(bounds, 200.0, 200.0, 0.0, 40, 7).unwrap();
        assert_eq!(geometry.thumb.top(), px(2.0));
        assert_eq!(geometry.thumb.size.height, px(100.0));

        // Grabbing inside the thumb preserves relative offset (top + 25px => grab = 0.25).
        let thumb_drag = Drag::begin(Some(geometry), point(px(6.0), px(27.0)), 7, 40).unwrap();
        assert!((thumb_drag.grab - 0.25).abs() < 1e-5);
        assert!(thumb_drag.is_active_for(7, geometry.track));
        // Dragging down 50px moves scroll offset by 100px (half of maximum 200).
        assert_eq!(
            thumb_drag.resolve_move(Some(geometry), 77.0, 7, 40),
            Ok(Some(100.0))
        );
        // Non-finite pointer keeps drag alive without producing a scroll offset.
        assert_eq!(
            thumb_drag.resolve_move(Some(geometry), f32::NAN, 7, 40),
            Ok(None)
        );

        // Clicking outside the thumb centers the grab at 0.5.
        let track_drag = Drag::begin(Some(geometry), point(px(6.0), px(152.0)), 7, 40).unwrap();
        assert_eq!(track_drag.grab, 0.5);
        assert_eq!(
            track_drag.resolve_move(Some(geometry), 152.0, 7, 40),
            Ok(Some(200.0))
        );

        // Stale generation, changed row count, missing geometry, or resized track cancels drag.
        assert_eq!(
            thumb_drag.resolve_move(Some(geometry), 77.0, 8, 40),
            Err(())
        );
        assert_eq!(
            thumb_drag.resolve_move(Some(geometry), 77.0, 7, 41),
            Err(())
        );
        assert_eq!(thumb_drag.resolve_move(None, 77.0, 7, 40), Err(()));
        let resized = Geometry::new(
            Bounds::new(point(px(0.0), px(0.0)), size(px(12.0), px(164.0))),
            160.0,
            200.0,
            0.0,
            40,
            7,
        )
        .unwrap();
        assert_eq!(thumb_drag.resolve_move(Some(resized), 77.0, 7, 40), Err(()));
        assert!(!thumb_drag.is_active_for(7, resized.track));

        // Refuses starting a drag when unscrollable or non-finite.
        let fitting = Geometry::new(bounds, 200.0, 0.0, 0.0, 3, 7).unwrap();
        assert!(Drag::begin(Some(fitting), point(px(6.0), px(50.0)), 7, 3).is_none());
        assert!(Drag::begin(Some(geometry), point(px(6.0), px(f32::NAN)), 7, 40).is_none());
    }
}
