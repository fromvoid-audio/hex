//! Virtual editor time before project zero extends the initial tempo/meter.
//! This only paints/snaps display coordinates: transport and audio reads remain
//! unsigned. Each side retains its existing bounded grid/ruler work (at most 256
//! negative grid lines and 64 labels, in addition to the positive resolver).
use super::super::signed_frame_viewport::SignedFrameViewport;
use super::*;
use hex_project::{audio_projection_beat, audio_projection_frame};

pub(in super::super) struct SignedGridLine {
    pub(in super::super) frame: i128,
    pub(in super::super) bar_start: bool,
}
pub(in super::super) struct SignedRulerTick {
    pub(in super::super) frame: i128,
    pub(in super::super) label: String,
    pub(in super::super) bar_start: bool,
}
fn negative_step(
    project: &Project,
    window: SignedFrameViewport,
    width: f32,
    grid: SnapGrid,
    ruler: bool,
) -> Option<u64> {
    if window.length == 0 || !width.is_finite() || width <= 0.0 {
        return None;
    }
    let point = project.meter_map().points().first()?;
    let beat_length = 4 * BeatTime::ONE.raw() / u64::from(point.meter().denominator());
    let spacing = if ruler {
        RULER_LABEL_SPACING_PX
    } else {
        adaptive_grid_spacing(width, grid.density())
    };
    let mut step = if ruler || grid.is_adaptive() || grid.is_off() {
        adaptive_base_step(
            project,
            Viewport {
                start: 0,
                length: window.length,
            },
            BeatTime::ZERO,
            beat_length,
            width,
            spacing,
        )
    } else {
        #[cfg(test)]
        {
            grid.step_raw()?
        }
        #[cfg(not(test))]
        {
            return None;
        }
    }
    .max(1);
    // Use the complete frame width, independent of pan/landmark phase. The
    // initial-tempo projection is linear, and two extra points cover rounding.
    let span = audio_projection_beat(project.tempo_map(), -i128::from(window.length))
        .ok()?
        .unsigned_abs();
    let maximum = if ruler {
        MAX_RULER_TICKS
    } else {
        MAX_GRID_LINES
    };
    while span / u128::from(step) + 2 > maximum as u128 {
        step = step.checked_mul(2)?;
    }
    Some(step)
}
fn negative_landmarks(
    project: &Project,
    window: SignedFrameViewport,
    width: f32,
    grid: SnapGrid,
    ruler: bool,
) -> Vec<(i128, i128)> {
    let Some(end) = window.end().map(|end| end.min(0)) else {
        return Vec::new();
    };
    if window.start >= end {
        return Vec::new();
    }
    let Some(step) = negative_step(project, window, width, grid, ruler).map(i128::from) else {
        return Vec::new();
    };
    let Ok(first) = audio_projection_beat(project.tempo_map(), window.start) else {
        return Vec::new();
    };
    let Some(mut raw) = first.div_euclid(step).checked_mul(step) else {
        return Vec::new();
    };
    let maximum = if ruler {
        MAX_RULER_TICKS
    } else {
        MAX_GRID_LINES
    };
    let mut points = Vec::with_capacity(maximum.min(32));
    for _ in 0..maximum + 2 {
        let Ok(frame) = audio_projection_frame(project.tempo_map(), raw) else {
            break;
        };
        if frame >= end {
            break;
        }
        if frame >= window.start && points.last().is_none_or(|(_, prior)| *prior < frame) {
            points.push((raw, frame));
            if points.len() == maximum {
                break;
            }
        }
        let Some(next) = raw.checked_add(step) else {
            break;
        };
        raw = next;
    }
    points
}
pub(in super::super) fn negative_grid_lines(
    project: &Project,
    window: SignedFrameViewport,
    width: f32,
    grid: SnapGrid,
) -> Vec<SignedGridLine> {
    if grid.is_off() {
        return Vec::new();
    }
    let Some(point) = project.meter_map().points().first() else {
        return Vec::new();
    };
    let unit = 4 * BeatTime::ONE.raw() / u64::from(point.meter().denominator());
    let bar = i128::from(unit) * i128::from(point.meter().numerator());
    negative_landmarks(project, window, width, grid, false)
        .into_iter()
        .map(|(raw, frame)| SignedGridLine {
            frame,
            bar_start: raw.rem_euclid(bar) == 0,
        })
        .collect()
}
pub(in super::super) fn negative_ruler_ticks(
    project: &Project,
    window: SignedFrameViewport,
    width: f32,
) -> Vec<SignedRulerTick> {
    let Some(point) = project.meter_map().points().first() else {
        return Vec::new();
    };
    let unit = i128::from(4 * BeatTime::ONE.raw() / u64::from(point.meter().denominator()));
    let bar_length = unit * i128::from(point.meter().numerator());
    negative_landmarks(project, window, width, SnapGrid::Adaptive, true)
        .into_iter()
        .map(|(raw, frame)| {
            let bar = raw.div_euclid(bar_length) + 1;
            let inside = raw.rem_euclid(bar_length);
            let beat = inside / unit + 1;
            let fraction = inside % unit;
            let label = if inside == 0 {
                bar.to_string()
            } else if fraction == 0 {
                format!("{bar}.{beat}")
            } else {
                format!("{bar}.{beat}.{}", fraction * 10 / unit)
            };
            SignedRulerTick {
                frame,
                label,
                bar_start: inside == 0,
            }
        })
        .collect()
}
impl SnapGrid {
    pub(in super::super) fn snap_signed_frame(
        self,
        project: &Project,
        frame: i128,
        window: SignedFrameViewport,
        width: f32,
    ) -> Option<i128> {
        if self.is_off() {
            return Some(frame);
        }
        if frame >= 0 {
            let viewport = window.visible_unsigned().unwrap_or(Viewport {
                start: 0,
                length: window.length,
            });
            let width = width * (viewport.length as f64 / window.length.max(1) as f64) as f32;
            return self
                .snap_frame(project, u64::try_from(frame).ok()?, viewport, width)
                .map(i128::from);
        }
        let raw = audio_projection_beat(project.tempo_map(), frame).ok()?;
        let step = i128::from(negative_step(project, window, width, self, false)?);
        let quotient = raw.div_euclid(step);
        let remainder = raw.rem_euclid(step);
        let rounded = quotient
            .checked_add(i128::from(remainder >= step - remainder))?
            .checked_mul(step)?;
        audio_projection_frame(project.tempo_map(), rounded).ok()
    }
}

impl SnapGrid {
    pub(in super::super) fn adjacent_signed_frame(
        self,
        project: &Project,
        frame: i128,
        window: SignedFrameViewport,
        width: f32,
        right: bool,
    ) -> Option<i128> {
        if self.is_off() {
            return frame.checked_add(if right { 1 } else { -1 });
        }
        if frame > 0 || frame == 0 && right {
            let viewport = window.visible_unsigned().unwrap_or(Viewport {
                start: 0,
                length: window.length,
            });
            let width = width * (viewport.length as f64 / window.length.max(1) as f64) as f32;
            return self
                .adjacent_frame(project, u64::try_from(frame).ok()?, viewport, width, right)
                .map(i128::from);
        }
        let raw = audio_projection_beat(project.tempo_map(), frame).ok()?;
        let step = i128::from(negative_step(project, window, width, self, false)?);
        let mut index = raw.div_euclid(step);
        if right {
            index = index.checked_add(1)?;
        } else if raw.rem_euclid(step) == 0 {
            index = index.checked_sub(1)?;
        }
        for _ in 0..4 {
            let candidate =
                audio_projection_frame(project.tempo_map(), index.checked_mul(step)?).ok()?;
            if if right {
                candidate > frame
            } else {
                candidate < frame
            } {
                return Some(candidate);
            }
            index = index.checked_add(if right { 1 } else { -1 })?;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn negative_grid_snap_ties_move_later_and_zero_stays_the_shared_anchor() {
        let project = Project::new(48_000).unwrap();
        let window = SignedFrameViewport {
            start: -24_000,
            length: 48_000,
        };
        assert_eq!(
            SnapGrid::Sixteenth.snap_signed_frame(&project, -9000, window, 1024.0),
            Some(-6000)
        );
        assert_eq!(
            SnapGrid::Sixteenth.snap_signed_frame(&project, -9001, window, 1024.0),
            Some(-12000)
        );
        assert_eq!(
            SnapGrid::Sixteenth.snap_signed_frame(&project, 9000, window, 1024.0),
            Some(12000)
        );
        assert_eq!(
            SnapGrid::Off.snap_signed_frame(&project, -9001, window, 1024.0),
            Some(-9001)
        );
        assert!(negative_grid_lines(&project, window, 1024.0, SnapGrid::Off).is_empty());
    }
    #[test]
    fn virtual_grid_and_labels_remain_bounded_ordered_and_in_the_negative_window() {
        for rate in [8000, 44100, 48000, 192000] {
            let project = Project::new(rate).unwrap();
            for window in [
                SignedFrameViewport {
                    start: -16_777_216,
                    length: 33_554_432,
                },
                SignedFrameViewport {
                    start: -24001,
                    length: 24000,
                },
            ] {
                let lines = negative_grid_lines(&project, window, 640.0, SnapGrid::Adaptive);
                assert!(lines.len() <= MAX_GRID_LINES);
                assert!(
                    lines
                        .iter()
                        .all(|line| line.frame < 0 && window.contains(line.frame))
                );
                assert!(lines.windows(2).all(|pair| pair[0].frame < pair[1].frame));
                let labels = negative_ruler_ticks(&project, window, 640.0);
                assert!(labels.len() <= MAX_RULER_TICKS);
                assert!(
                    labels
                        .iter()
                        .all(|label| label.frame < 0 && window.contains(label.frame))
                );
            }
        }
    }
}
