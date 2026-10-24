//! Vertical hit policy for native time-range surfaces. The host supplies local
//! pixels, reserves trim/fade targets and decides whether existing content is hit.
//! Track/frame/beat projection, snapping, capture transport, canonical selection
//! and released edit receipts stay with that host. This policy has no project or
//! audio dependency and never infers musical coordinates from pixels.

pub const RANGE_HANDLE_FRACTION: f32 = 1.0 / 3.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeSelectionTarget {
    Move,
    Paint,
}

/// A proportional move rail with an optional compact title minimum. Rendering
/// and input use the same checked geometry, including the exact third boundary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RangeSelectionZones {
    height: f32,
    handle_height: f32,
}
impl RangeSelectionZones {
    pub fn new(height: f32, minimum_handle_height: f32) -> Option<Self> {
        if !height.is_finite()
            || height <= 0.0
            || !minimum_handle_height.is_finite()
            || minimum_handle_height < 0.0
        {
            return None;
        }
        Some(Self {
            height,
            handle_height: (height * RANGE_HANDLE_FRACTION)
                .max(minimum_handle_height)
                .min(height),
        })
    }

    pub fn handle_height(self) -> f32 {
        self.handle_height
    }

    /// Existing content/ranges move only from the upper rail. Empty space, the
    /// lower body and an explicit extension gesture paint/repaint a range.
    /// `local_y` excludes any separate ruler; the host handles that ruler itself.
    pub fn target(
        self,
        local_y: f32,
        hits_existing: bool,
        extend: bool,
    ) -> Option<RangeSelectionTarget> {
        if !local_y.is_finite() || local_y < 0.0 || local_y >= self.height {
            return None;
        }
        Some(
            if !extend && hits_existing && local_y < self.handle_height {
                RangeSelectionTarget::Move
            } else {
                RangeSelectionTarget::Paint
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_body_repaints_even_inside_an_existing_range() {
        let zones = RangeSelectionZones::new(90.0, 0.0).unwrap();
        assert_eq!(zones.handle_height(), 30.0);
        assert_eq!(
            zones.target(29.999, true, false),
            Some(RangeSelectionTarget::Move)
        );
        for y in [30.0, 60.0, 89.999] {
            assert_eq!(
                zones.target(y, true, false),
                Some(RangeSelectionTarget::Paint)
            );
        }
        assert_eq!(
            zones.target(0.0, false, false),
            Some(RangeSelectionTarget::Paint)
        );
        assert_eq!(
            zones.target(0.0, true, true),
            Some(RangeSelectionTarget::Paint)
        );
        let compact = RangeSelectionZones::new(24.0, 12.0).unwrap();
        assert_eq!(compact.handle_height(), 12.0);
        assert_eq!(
            compact.target(12.0, true, false),
            Some(RangeSelectionTarget::Paint)
        );
        assert_eq!(
            RangeSelectionZones::new(8.0, 12.0).unwrap().handle_height(),
            8.0
        );
    }

    #[test]
    fn invalid_and_outside_geometry_cannot_claim_input() {
        for value in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(RangeSelectionZones::new(value, 0.0).is_none());
        }
        for value in [-1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(RangeSelectionZones::new(90.0, value).is_none());
        }
        let zones = RangeSelectionZones::new(90.0, 0.0).unwrap();
        for y in [-1.0, 90.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(zones.target(y, true, false), None);
        }
    }
}
