//! Visible elapsed-time ruler labels on the project's frame clock.
//! Intervals are anchored at frame zero and projected to the nearest sample.

use super::viewport::Viewport;

const SECOND: u128 = 1_000_000;
const MINUTE: u128 = 60 * SECOND;
const HOUR: u128 = 60 * MINUTE;
const MAX_TICKS: usize = 64;
const LABEL_SPACING_PX: f32 = 96.0;

const INTERVALS: &[u128] = &[
    1,
    2,
    5,
    10,
    20,
    50,
    100,
    200,
    500,
    1_000,
    2_000,
    5_000,
    10_000,
    20_000,
    50_000,
    100_000,
    200_000,
    500_000,
    SECOND,
    2 * SECOND,
    5 * SECOND,
    10 * SECOND,
    15 * SECOND,
    30 * SECOND,
    MINUTE,
    2 * MINUTE,
    5 * MINUTE,
    10 * MINUTE,
    15 * MINUTE,
    30 * MINUTE,
    HOUR,
];

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Tick {
    pub(super) frame: u64,
    pub(super) label: String,
}

pub(super) fn ruler_ticks(sample_rate: u32, viewport: Viewport, width: f32) -> Vec<Tick> {
    if sample_rate == 0 || viewport.length == 0 || !width.is_finite() || width <= 0.0 {
        return Vec::new();
    }
    let Some(end) = viewport.end() else {
        return Vec::new();
    };
    let rate = u128::from(sample_rate);
    let intervals = (width / LABEL_SPACING_PX).clamp(1.0, (MAX_TICKS - 1) as f32) as u128;
    let minimum_frames = u128::from(viewport.length).div_ceil(intervals);
    let minimum_micros = (minimum_frames * SECOND).div_ceil(rate);
    let interval = INTERVALS
        .iter()
        .copied()
        .find(|step| *step >= minimum_micros)
        .unwrap_or_else(|| minimum_micros.div_ceil(HOUR).next_power_of_two() * HOUR);
    let decimals = (0..=6)
        .find(|digits| interval.is_multiple_of(10_u128.pow(6 - digits)))
        .unwrap_or(6);

    // Include a tick rounding to the first visible frame.
    let first_micros = (u128::from(viewport.start) * SECOND)
        .saturating_sub(SECOND / 2)
        .div_ceil(rate);
    let first_tick = first_micros.div_ceil(interval) * interval;
    (0..MAX_TICKS as u128)
        .map(|index| first_tick + index * interval)
        .map_while(|micros| {
            let frame = u64::try_from((micros * rate + SECOND / 2) / SECOND).ok()?;
            (frame < end).then(|| Tick {
                frame,
                label: label(micros, decimals),
            })
        })
        .collect()
}

fn label(micros: u128, decimals: u32) -> String {
    let seconds = micros / SECOND;
    let minutes = seconds / 60;
    let clock = if minutes < 60 {
        format!("{minutes}:{:02}", seconds % 60)
    } else {
        format!("{}:{:02}:{:02}", minutes / 60, minutes % 60, seconds % 60)
    };
    if decimals == 0 {
        clock
    } else {
        let fraction = (micros % SECOND) / 10_u128.pow(6 - decimals);
        format!("{clock}.{fraction:0width$}", width = decimals as usize)
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;

    #[test]
    fn elapsed_ticks_follow_absolute_time_at_each_zoom_and_sample_rate() {
        let ticks = ruler_ticks(
            48_000,
            Viewport {
                start: 24_000,
                length: 480_000,
            },
            960.0,
        );
        assert_eq!(ticks.len(), 10);
        assert_eq!(
            ticks[0],
            Tick {
                frame: 48_000,
                label: "0:01".into()
            }
        );
        assert_eq!(
            ticks[9],
            Tick {
                frame: 480_000,
                label: "0:10".into()
            }
        );

        let ticks = ruler_ticks(
            44_100,
            Viewport {
                start: 4_410,
                length: 44_100,
            },
            960.0,
        );
        assert_eq!(ticks.len(), 10);
        assert_eq!(
            ticks[0],
            Tick {
                frame: 4_410,
                label: "0:00.1".into()
            }
        );
        assert_eq!(
            ticks[9],
            Tick {
                frame: 44_100,
                label: "0:01.0".into()
            }
        );

        let ticks = ruler_ticks(
            44_100,
            Viewport {
                start: 4,
                length: 40,
            },
            960.0,
        );
        assert_eq!(
            ticks[0],
            Tick {
                frame: 4,
                label: "0:00.0001".into()
            }
        );
        assert!(ticks.windows(2).all(|pair| pair[0].frame < pair[1].frame));
    }

    #[test]
    fn long_timelines_are_bounded_and_invalid_geometry_has_no_ticks() {
        let long = ruler_ticks(48_000, Viewport::fit(u64::MAX), f32::MAX);
        assert!(long.len() > 1 && long.len() <= MAX_TICKS);
        assert!(long.windows(2).all(|pair| pair[0].frame < pair[1].frame));

        let hour = 3_600 * 48_000;
        let ticks = ruler_ticks(
            48_000,
            Viewport {
                start: hour,
                length: 48_000,
            },
            96.0,
        );
        assert_eq!(
            ticks,
            [Tick {
                frame: hour,
                label: "1:00:00".into()
            }]
        );
        assert!(ruler_ticks(0, Viewport::fit(1_000), 600.0).is_empty());
        assert!(ruler_ticks(48_000, Viewport::fit(1_000), f32::NAN).is_empty());
        assert!(ruler_ticks(48_000, Viewport::fit(0), 600.0).is_empty());
        assert!(
            ruler_ticks(
                48_000,
                Viewport {
                    start: u64::MAX,
                    length: 2
                },
                600.0
            )
            .is_empty()
        );
    }
}
