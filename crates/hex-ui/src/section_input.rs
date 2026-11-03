//! Section header/body routing; hosts retain capture, selection and edit custody.
use hex_widgets::RangeSelectionZones;

/// An edge wins anywhere in the lane. With no edge, the lower body belongs to
/// the time-range owner; extended section selection is handled before routing.
pub(crate) fn routes_to_time_range(
    edge: bool,
    y: f32,
    top: f32,
    height: f32,
    minimum_header: f32,
) -> bool {
    if edge || !y.is_finite() || !top.is_finite() {
        return false;
    }
    RangeSelectionZones::new(height, minimum_header)
        .is_some_and(|zones| y >= top + zones.handle_height())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_starts_at_the_header_boundary_and_edges_win() {
        assert!(!routes_to_time_range(false, 33.9, 10.0, 72.0, 14.0));
        assert!(routes_to_time_range(false, 34.0, 10.0, 72.0, 14.0));
        assert!(!routes_to_time_range(true, 60.0, 10.0, 72.0, 14.0));
    }

    #[test]
    fn compact_minimum_and_invalid_geometry_are_explicit() {
        assert!(!routes_to_time_range(false, 19.9, 5.0, 30.0, 15.0));
        assert!(routes_to_time_range(false, 20.0, 5.0, 30.0, 15.0));
        assert!(!routes_to_time_range(false, f32::NAN, 0.0, 30.0, 15.0));
        assert!(!routes_to_time_range(false, 20.0, 0.0, 0.0, 15.0));
    }
}
