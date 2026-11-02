//! Shared dial artwork. Parameter laws remain in NumericSpec; this draws only
//! the validated normalized position supplied by the numeric control.
use crate::{ControlStyle, NumericScale};
use gpui::{Bounds, PathBuilder, Pixels, Window, point, px, rgb};

/// Micro dial tile metrics shared with native device layout owners.
pub const MICRO_DIAL_WIDTH: f32 = 58.0;
/// A micro dial with space for full frequency values and units.
pub const WIDE_MICRO_DIAL_WIDTH: f32 = 74.0;
pub const MICRO_DIAL_HEIGHT: f32 = 40.0;
pub const MICRO_DIAL_DIAMETER: f32 = 18.0;

/// Semantic rail orientation, independent of knob density or device identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DialLayout {
    /// Lower-right opening for the readout (observed Operator shell class).
    #[default]
    ReadoutGap,
    /// Bottom-centered opening, for callers with a centered mixer-style rail.
    BottomGap,
    /// Bottom opening, center-up needle and a reset marker above the rail.
    /// The caller must supply a bipolar spec and its canonical neutral default.
    Pan,
}

impl DialLayout {
    fn start(self) -> f32 {
        match self {
            Self::ReadoutGap => std::f32::consts::FRAC_PI_2,
            Self::BottomGap | Self::Pan => 3.0 * std::f32::consts::FRAC_PI_4,
        }
    }
}

// The installed Live reference starts at six o'clock and ends at three
// o'clock after a clockwise 270-degree sweep. The lower-right gap holds text.
#[cfg(test)]
const START: f32 = std::f32::consts::FRAC_PI_2;
const SWEEP: f32 = 3.0 * std::f32::consts::FRAC_PI_2;

#[cfg(test)]
fn angle(position: f32) -> f32 {
    START + position * SWEEP
}

#[derive(Clone, Copy)]
pub(crate) struct DialValue {
    position: f32,
    origin: f32,
    layout: DialLayout,
}

impl DialValue {
    pub(crate) fn new(position: f32, scale: NumericScale, layout: DialLayout) -> Self {
        Self {
            position,
            layout,
            origin: if scale == NumericScale::Bipolar {
                0.5
            } else {
                0.0
            },
        }
    }

    fn angle(self, position: f32) -> f32 {
        self.layout.start() + position * SWEEP
    }

    fn fill_angles(self) -> Option<(f32, f32)> {
        (self.position != self.origin).then(|| {
            (
                self.angle(self.origin.min(self.position)),
                self.angle(self.origin.max(self.position)),
            )
        })
    }
}

pub(crate) fn paint_dial(
    bounds: Bounds<Pixels>,
    size: f32,
    value: DialValue,
    enabled: bool,
    style: ControlStyle,
    window: &mut Window,
) {
    let pan = value.layout == DialLayout::Pan;
    let center =
        bounds.origin + point(px(size / 2.0), px(size / 2.0 + if pan { 1.5 } else { 0.0 }));
    let radius = size / 2.0 - if pan { 3.5 } else { 1.5 };
    let polar = |angle: f32, radius: f32| {
        center + point(px(angle.cos() * radius), px(angle.sin() * radius))
    };
    let stroke_arc = |start: f32, end: f32, width: f32, window: &mut Window, color| {
        let mut arc = PathBuilder::stroke(px(width));
        arc.move_to(polar(start, radius));
        for step in 1..=48 {
            arc.line_to(polar(start + (end - start) * step as f32 / 48.0, radius));
        }
        if let Ok(path) = arc.build() {
            window.paint_path(path, rgb(color));
        }
    };
    stroke_arc(
        value.layout.start(),
        value.angle(1.0),
        1.8,
        window,
        if enabled {
            style.dial_rail
        } else {
            style.muted
        },
    );
    if enabled && let Some((start, end)) = value.fill_angles() {
        stroke_arc(start, end, 1.8, window, style.accent);
    }
    if pan {
        let mut marker = PathBuilder::fill();
        marker.move_to(bounds.origin + point(px(size / 2.0 - 2.5), px(0.5)));
        marker.line_to(bounds.origin + point(px(size / 2.0 + 2.5), px(0.5)));
        marker.line_to(bounds.origin + point(px(size / 2.0), px(3.5)));
        marker.close();
        if let Ok(path) = marker.build() {
            window.paint_path(path, rgb(if enabled { style.text } else { style.muted }));
        }
    }
    let mut needle = PathBuilder::stroke(px(1.5));
    needle.move_to(polar(value.angle(value.position), radius * 0.12));
    needle.line_to(polar(value.angle(value.position), radius * 0.85));
    if let Ok(path) = needle.build() {
        window.paint_path(path, rgb(if enabled { style.text } else { style.muted }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NumericScale, NumericSpec};

    #[test]
    fn dial_amount_uses_parameter_normalization_and_lower_right_gap() {
        let log = NumericSpec::new(
            20.0,
            20_000.0,
            200.0,
            NumericScale::Logarithmic,
            0,
            None::<String>,
        )
        .expect("spec");
        let bipolar =
            NumericSpec::new(-100.0, 100.0, 0.0, NumericScale::Bipolar, 0, None::<String>)
                .expect("spec");
        assert_eq!(angle(log.to_normalized(20.0).expect("min") as f32), START);
        assert_eq!(
            angle(log.to_normalized(20_000.0).expect("max") as f32),
            START + SWEEP
        );
        let middle = log
            .to_normalized((20.0_f64 * 20_000.0).sqrt())
            .expect("mid");
        assert!((angle(middle as f32) - (START + SWEEP / 2.0)).abs() < 1e-6);
        assert_eq!(
            angle(bipolar.to_normalized(0.0).expect("center") as f32),
            angle(0.5)
        );
        assert_eq!(
            DialValue::new(0.0, NumericScale::Linear, DialLayout::ReadoutGap).fill_angles(),
            None
        );
        assert_eq!(
            DialValue::new(1.0, NumericScale::Linear, DialLayout::ReadoutGap).fill_angles(),
            Some((START, START + SWEEP))
        );
        assert_eq!(
            DialValue::new(0.5, NumericScale::Logarithmic, DialLayout::ReadoutGap).fill_angles(),
            Some((START, angle(0.5)))
        );
        assert_eq!(
            DialValue::new(0.5, NumericScale::Bipolar, DialLayout::ReadoutGap).fill_angles(),
            None
        );
        assert_eq!(
            DialValue::new(0.0, NumericScale::Bipolar, DialLayout::ReadoutGap).fill_angles(),
            Some((START, angle(0.5)))
        );
        assert_eq!(
            DialValue::new(1.0, NumericScale::Bipolar, DialLayout::ReadoutGap).fill_angles(),
            Some((angle(0.5), angle(1.0)))
        );
        // Screen y grows downward: minimum points down, maximum points right.
        assert!(angle(0.0).cos().abs() < 1e-6);
        assert!((angle(0.0).sin() - 1.0).abs() < 1e-6);
        assert!((angle(1.0).cos() - 1.0).abs() < 1e-6);
        assert!(angle(1.0).sin().abs() < 1e-6);
    }
    #[test]
    fn pan_center_points_up_and_bottom_gap_stays_density_independent() {
        let pan = DialValue::new(0.5, NumericScale::Bipolar, DialLayout::Pan);
        assert!(pan.angle(0.5).cos().abs() < 1e-6);
        assert!((pan.angle(0.5).sin() + 1.0).abs() < 1e-6);
        assert!(pan.angle(0.0).cos() < 0.0 && pan.angle(0.0).sin() > 0.0);
        assert!(pan.angle(1.0).cos() > 0.0 && pan.angle(1.0).sin() > 0.0);
        assert_eq!(pan.fill_angles(), None);
        let send = DialValue::new(0.5, NumericScale::Linear, DialLayout::BottomGap);
        assert_eq!(send.angle(0.5), pan.angle(0.5));
        assert_eq!(
            send.fill_angles(),
            Some((DialLayout::BottomGap.start(), pan.angle(0.5)))
        );
    }
}
