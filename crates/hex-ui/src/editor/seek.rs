use super::viewport::Viewport;

pub(super) fn pointer_to_frame(
    pointer_x: f32,
    timeline_left: f32,
    timeline_width: f32,
    viewport: Viewport,
) -> Option<u64> {
    if !pointer_x.is_finite()
        || !timeline_left.is_finite()
        || !timeline_width.is_finite()
        || timeline_width <= 0.0
    {
        return None;
    }

    let end = viewport.end()?;
    if viewport.length == 0 {
        return Some(viewport.start);
    }
    let fraction = (f64::from(pointer_x) - f64::from(timeline_left)) / f64::from(timeline_width);
    let offset = (fraction.clamp(0.0, 1.0) * viewport.length as f64).round();
    let offset = if offset <= 0.0 {
        0
    } else if offset >= viewport.length as f64 {
        viewport.length
    } else {
        offset as u64
    };
    Some(viewport.start.saturating_add(offset).min(end))
}

pub(super) fn seek_is_pending(ticket: Option<u64>, consumed: u64, queued: bool) -> bool {
    queued || ticket.is_some_and(|ticket| ticket > consumed)
}

#[cfg(test)]
mod tests {
    use super::Viewport;
    use super::{pointer_to_frame, seek_is_pending};

    #[test]
    fn maps_actual_ruler_bounds_and_rounds_to_nearest_frame() {
        let viewport = Viewport {
            start: 100,
            length: 11,
        };
        assert_eq!(pointer_to_frame(20.0, 10.0, 20.0, viewport), Some(106));
        assert_eq!(pointer_to_frame(10.0, 10.0, 20.0, viewport), Some(100));
        assert_eq!(pointer_to_frame(30.0, 10.0, 20.0, viewport), Some(111));
    }

    #[test]
    fn clamps_pointer_to_visible_horizon() {
        let viewport = Viewport {
            start: 50,
            length: 1_000,
        };
        assert_eq!(pointer_to_frame(-50.0, 10.0, 20.0, viewport), Some(50));
        assert_eq!(pointer_to_frame(200.0, 10.0, 20.0, viewport), Some(1_050));
    }

    #[test]
    fn handles_large_horizons_without_overflow() {
        let viewport = Viewport {
            start: 0,
            length: u64::MAX,
        };
        assert_eq!(pointer_to_frame(10.0, 10.0, 20.0, viewport), Some(0));
        assert_eq!(pointer_to_frame(30.0, 10.0, 20.0, viewport), Some(u64::MAX));
        assert!(pointer_to_frame(20.0, 10.0, 20.0, viewport).is_some());
    }

    #[test]
    fn rejects_non_finite_or_empty_geometry() {
        let viewport = Viewport {
            start: 0,
            length: 100,
        };
        assert_eq!(pointer_to_frame(f32::NAN, 0.0, 10.0, viewport), None);
        assert_eq!(pointer_to_frame(1.0, f32::INFINITY, 10.0, viewport), None);
        assert_eq!(pointer_to_frame(1.0, 0.0, f32::NAN, viewport), None);
        assert_eq!(pointer_to_frame(1.0, 0.0, 0.0, viewport), None);
        assert_eq!(pointer_to_frame(1.0, 0.0, -1.0, viewport), None);
    }

    #[test]
    fn seek_gate_waits_for_command_acknowledgment() {
        assert!(seek_is_pending(Some(7), 6, false));
        assert!(!seek_is_pending(Some(7), 7, false));
        assert!(seek_is_pending(None, 0, true));
        assert!(!seek_is_pending(None, 0, false));
    }
}
