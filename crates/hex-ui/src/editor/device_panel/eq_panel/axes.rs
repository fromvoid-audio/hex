//! Calibrated UI axes; frequency normalization matches the authored curve and nodes.
use hex_widgets::ResponseGraphAxes;

pub(super) fn upper(rate: u32) -> f32 {
    (rate as f32 * 0.49).min(20_000.0)
}
pub(super) fn x(frequency: f32, rate: u32) -> f32 {
    x_with_upper(frequency, upper(rate))
}
pub(super) fn x_with_upper(frequency: f32, upper: f32) -> f32 {
    ((frequency / 20.0).ln() / (upper / 20.0).ln()).clamp(0.0, 1.0)
}
pub(super) fn frequency_axes(rate: u32) -> ResponseGraphAxes {
    let ticks = [
        (20.0, "20 hz"),
        (30.0, ""),
        (40.0, ""),
        (50.0, "50"),
        (60.0, ""),
        (80.0, ""),
        (100.0, "100"),
        (200.0, "200"),
        (300.0, ""),
        (400.0, ""),
        (500.0, "500"),
        (600.0, ""),
        (800.0, ""),
        (1000.0, "1 khz"),
        (2000.0, "2k"),
        (3000.0, ""),
        (4000.0, ""),
        (5000.0, "5k"),
        (6000.0, ""),
        (8000.0, ""),
        (10000.0, "10k"),
        (20000.0, "20 khz"),
    ];
    ResponseGraphAxes {
        horizontal: ticks
            .into_iter()
            .filter(|(f, _)| *f <= upper(rate))
            .map(|(f, label)| (x(f, rate), crate::theme::text(label)))
            .collect(),
        vertical: [
            (0.0, "-24"),
            (0.25, "-12"),
            (0.5, "0 db"),
            (0.75, "+12"),
            (1.0, "+24"),
        ]
        .map(|(y, t)| (y, crate::theme::text(t)))
        .into(),
        right: [
            (0.0, "-96"),
            (0.25, "-72"),
            (0.5, "-48"),
            (0.75, "-24"),
            (1.0, "0 dbfs"),
        ]
        .map(|(y, t)| (y, crate::theme::text(t)))
        .into(),
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    #[test]
    fn eq_log_axes_follow_rate_and_separate_gain_from_analyzer_scale() {
        for rate in [8000, 44100, 48000, 192000] {
            let axes = frequency_axes(rate);
            assert!(axes.horizontal.windows(2).all(|p| p[0].0 < p[1].0));
            assert_eq!(x(20.0, rate), 0.0);
            assert_eq!(x(upper(rate), rate), 1.0);
            let middle = (20.0 * upper(rate)).sqrt();
            assert!((x(middle, rate) - 0.5).abs() < 0.000001);
            assert_eq!(axes.vertical[2].1.as_ref(), "0 db");
            assert_eq!(axes.right[2].1.as_ref(), "-48");
        }
    }
}
