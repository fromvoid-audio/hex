//! Map piano-roll wheel input into view navigation without acquiring a window.
use gpui::{Modifiers, ScrollDelta, px};

/// Pixels per wheel line for scrolling; one mouse notch moves a few pitch rows.
const SCROLL_LINE_PX: f32 = 40.0;
/// Pixels per wheel line for zoom, matching the Arrange zoom accumulator.
const ZOOM_LINE_PX: f32 = 80.0;

/// One resolved wheel batch. Deltas are finite and at least one is nonzero.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum WheelNav {
    /// Scroll in pixels: positive `pitch` reveals higher rows, positive `time`
    /// reveals earlier beats (content follows the scroll direction).
    Scroll { pitch: f32, time: f32 },
    /// Time zoom delta in pixels; positive zooms in.
    ZoomTime(f32),
    /// Pitch zoom delta in pixels; positive zooms in.
    ZoomPitch(f32),
}

/// Map modifiers and both wheel axes to a navigation; `None` ignores the event.
pub(super) fn resolve(
    modifiers: Modifiers,
    delta: ScrollDelta,
    physical: ScrollDelta,
) -> Option<WheelNav> {
    let finite_nonzero = |value: f32| value.is_finite() && value != 0.0;
    if modifiers.platform || modifiers.alt {
        let zoom = -f32::from(physical.pixel_delta(px(ZOOM_LINE_PX)).y);
        if !finite_nonzero(zoom) {
            return None;
        }
        return Some(if modifiers.platform {
            WheelNav::ZoomTime(zoom)
        } else {
            WheelNav::ZoomPitch(zoom)
        });
    }
    let scroll = delta.pixel_delta(px(SCROLL_LINE_PX));
    let (x, y) = (f32::from(scroll.x), f32::from(scroll.y));
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    let (pitch, time) = if modifiers.shift {
        // A mouse wheel has one axis; Shift redirects it to time.
        (0.0, if x != 0.0 { x } else { y })
    } else {
        (y, x)
    };
    (pitch != 0.0 || time != 0.0).then_some(WheelNav::Scroll { pitch, time })
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    use gpui::point;

    fn pixels(x: f32, y: f32) -> ScrollDelta {
        ScrollDelta::Pixels(point(px(x), px(y)))
    }

    fn mods(platform: bool, alt: bool, shift: bool) -> Modifiers {
        Modifiers {
            platform,
            alt,
            shift,
            ..Modifiers::default()
        }
    }

    #[test]
    fn plain_wheel_scrolls_pitch_and_horizontal_swipe_scrolls_time() {
        let plain = mods(false, false, false);
        assert_eq!(
            resolve(plain, pixels(0.0, 12.0), pixels(0.0, 12.0)),
            Some(WheelNav::Scroll {
                pitch: 12.0,
                time: 0.0
            })
        );
        assert_eq!(
            resolve(plain, pixels(-30.0, 0.0), pixels(-30.0, 0.0)),
            Some(WheelNav::Scroll {
                pitch: 0.0,
                time: -30.0
            })
        );
        assert_eq!(
            resolve(plain, pixels(5.0, -7.0), pixels(5.0, -7.0)),
            Some(WheelNav::Scroll {
                pitch: -7.0,
                time: 5.0
            })
        );
        let lines = ScrollDelta::Lines(point(0.0, 1.0));
        assert_eq!(
            resolve(plain, lines, lines),
            Some(WheelNav::Scroll {
                pitch: SCROLL_LINE_PX,
                time: 0.0
            })
        );
    }

    #[test]
    fn shift_redirects_vertical_wheel_to_time() {
        let shift = mods(false, false, true);
        assert_eq!(
            resolve(shift, pixels(0.0, 9.0), pixels(0.0, 9.0)),
            Some(WheelNav::Scroll {
                pitch: 0.0,
                time: 9.0
            })
        );
        assert_eq!(
            resolve(shift, pixels(-4.0, 9.0), pixels(-4.0, 9.0)),
            Some(WheelNav::Scroll {
                pitch: 0.0,
                time: -4.0
            })
        );
    }

    #[test]
    fn cmd_zooms_time_and_alt_zooms_pitch_in_device_direction() {
        // Device direction is inverted relative to the preference delta here.
        let delta = pixels(3.0, 20.0);
        let physical = pixels(-3.0, -20.0);
        assert_eq!(
            resolve(mods(true, false, false), delta, physical),
            Some(WheelNav::ZoomTime(20.0))
        );
        assert_eq!(
            resolve(mods(true, true, true), delta, physical),
            Some(WheelNav::ZoomTime(20.0)),
            "cmd takes precedence"
        );
        assert_eq!(
            resolve(mods(false, true, false), delta, physical),
            Some(WheelNav::ZoomPitch(20.0))
        );
        // A horizontal-only swipe does not zoom.
        assert_eq!(
            resolve(mods(true, false, false), pixels(9.0, 0.0), pixels(9.0, 0.0)),
            None
        );
    }

    #[test]
    fn zero_and_non_finite_deltas_are_ignored() {
        for modifiers in [
            mods(false, false, false),
            mods(false, false, true),
            mods(true, false, false),
            mods(false, true, false),
        ] {
            assert_eq!(resolve(modifiers, pixels(0.0, 0.0), pixels(0.0, 0.0)), None);
            assert_eq!(
                resolve(modifiers, pixels(f32::NAN, 1.0), pixels(f32::NAN, f32::NAN)),
                None
            );
            assert_eq!(
                resolve(
                    modifiers,
                    pixels(0.0, f32::INFINITY),
                    pixels(0.0, f32::INFINITY)
                ),
                None
            );
        }
    }
}
