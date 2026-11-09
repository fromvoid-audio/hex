//! Section header/body routing; hosts retain capture, selection and edit custody.

/// Section edit keys keep copy and marker intent distinct from live snap inversion.
/// Control remains an optional alias for snap inversion, including marker edits.
pub(crate) struct EditModifiers {
    pub marker_only: bool,
    pub copy_content: bool,
    pub invert_snap: bool,
}
impl From<gpui::Modifiers> for EditModifiers {
    fn from(modifiers: gpui::Modifiers) -> Self {
        Self {
            marker_only: modifiers.alt && modifiers.platform,
            copy_content: modifiers.alt && !modifiers.platform,
            invert_snap: modifiers.control || (modifiers.platform && !modifiers.alt),
        }
    }
}

/// An edge wins anywhere in the lane. With no edge, the lower body belongs to
/// the time-range owner, except a double press in empty space starts painting.
/// Extended section selection is handled before routing.
pub(crate) fn routes_to_time_range(
    edge: bool,
    paint_empty: bool,
    y: f32,
    top: f32,
    height: f32,
    minimum_header: f32,
) -> Option<bool> {
    if !y.is_finite()
        || !top.is_finite()
        || !height.is_finite()
        || height <= 0.0
        || !minimum_header.is_finite()
        || minimum_header < 0.0
    {
        return None;
    }
    // Match the compact fixed-height painted header; increasing Main's
    // height grows the selection body rather than its reorder hit target.
    let boundary = top + minimum_header.min(height);
    boundary
        .is_finite()
        .then_some(!edge && !paint_empty && y >= boundary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_edit_keys_have_distinct_modes_and_a_control_snap_alias() {
        for control in [false, true] {
            for shift in [false, true] {
                for (platform, alt, marker_only, copy_content, invert_snap) in [
                    (false, false, false, false, false),
                    (true, false, false, false, true),
                    (false, true, false, true, false),
                    (true, true, true, false, false),
                ] {
                    let actual = EditModifiers::from(gpui::Modifiers {
                        platform,
                        alt,
                        control,
                        shift,
                        ..gpui::Modifiers::default()
                    });
                    assert_eq!(actual.marker_only, marker_only);
                    assert_eq!(actual.copy_content, copy_content);
                    assert_eq!(actual.invert_snap, invert_snap || control);
                }
            }
        }
    }

    #[test]
    fn body_starts_at_the_header_boundary_and_edges_win() {
        assert_eq!(
            routes_to_time_range(false, false, 23.9, 10.0, 72.0, 14.0),
            Some(false)
        );
        assert_eq!(
            routes_to_time_range(false, false, 24.0, 10.0, 72.0, 14.0),
            Some(true)
        );
        assert_eq!(
            routes_to_time_range(true, false, 60.0, 10.0, 72.0, 14.0),
            Some(false)
        );
    }

    #[test]
    fn empty_double_press_paints_the_full_height_but_preserves_geometry_validation() {
        for y in [12.0, 46.0, 80.0] {
            assert_eq!(
                routes_to_time_range(false, true, y, 10.0, 72.0, 14.0),
                Some(false)
            );
        }
        assert_eq!(
            routes_to_time_range(false, true, f32::NAN, 0.0, 72.0, 14.0),
            None
        );
        assert_eq!(
            routes_to_time_range(false, true, 50.0, 0.0, 0.0, 14.0),
            None
        );
    }

    #[test]
    fn compact_minimum_and_invalid_geometry_are_explicit() {
        assert_eq!(
            routes_to_time_range(false, false, 19.9, 5.0, 30.0, 15.0),
            Some(false)
        );
        assert_eq!(
            routes_to_time_range(false, false, 20.0, 5.0, 30.0, 15.0),
            Some(true)
        );
        assert_eq!(
            routes_to_time_range(false, false, f32::NAN, 0.0, 30.0, 15.0),
            None
        );
        assert_eq!(
            routes_to_time_range(false, false, 20.0, 0.0, 0.0, 15.0),
            None
        );
    }
}
