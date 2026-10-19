#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Viewport {
    pub(super) start: u64,
    pub(super) length: u64,
}

impl Viewport {
    pub(super) fn fit(horizon: u64) -> Self {
        Self {
            start: 0,
            length: horizon,
        }
    }

    pub(super) fn end(self) -> Option<u64> {
        self.start.checked_add(self.length)
    }

    pub(super) fn clamp(self, horizon: u64) -> Self {
        let length = self.length.min(horizon);
        Self {
            start: self.start.min(horizon.saturating_sub(length)),
            length,
        }
    }

    pub(super) fn zoom(self, horizon: u64, playhead: u64, inward: bool) -> Self {
        if horizon == 0 {
            return Self::fit(0);
        }
        let current = self.clamp(horizon);
        if current.length == 0 {
            return Self::fit(horizon);
        }
        let length = if inward {
            (current.length / 2).max(1)
        } else {
            current.length.saturating_mul(2).min(horizon)
        };
        let end = current.end().unwrap_or(horizon);
        let anchor = if playhead >= current.start && playhead < end {
            playhead
        } else {
            current.start + current.length / 2
        };
        let anchor_offset = anchor.saturating_sub(current.start);
        let scaled_offset = ((anchor_offset as u128 * length as u128)
            .saturating_add((current.length / 2) as u128)
            / current.length as u128) as u64;
        Self {
            start: anchor.saturating_sub(scaled_offset),
            length,
        }
        .clamp(horizon)
    }

    /// Zoom around a frame at the pointer, using a bounded wheel-friendly step.
    pub(super) fn zoom_at(self, horizon: u64, anchor_frame: u64, inward: bool) -> Self {
        if horizon == 0 {
            return Self::fit(0);
        }
        let current = self.clamp(horizon);
        if current.length == 0 {
            return Self::fit(horizon);
        }
        let length = if inward {
            ((u128::from(current.length) * 4) / 5).max(1) as u64
        } else {
            ((u128::from(current.length) * 5).div_ceil(4)).min(u128::from(horizon)) as u64
        };
        let anchor = anchor_frame.clamp(current.start, current.end().unwrap_or(horizon));
        let anchor_offset = anchor.saturating_sub(current.start);
        let scaled_offset =
            (u128::from(anchor_offset) * u128::from(length) / u128::from(current.length)) as u64;
        Self {
            start: anchor.saturating_sub(scaled_offset),
            length,
        }
        .clamp(horizon)
    }

    pub(super) fn pan_by_frames(self, horizon: u64, delta: i128) -> Self {
        let current = self.clamp(horizon);
        if current.length == 0 || horizon == 0 {
            return Self::fit(horizon);
        }
        let start = if delta < 0 {
            current
                .start
                .saturating_sub(delta.unsigned_abs().min(u128::from(u64::MAX)) as u64)
        } else {
            current
                .start
                .saturating_add(delta.min(i128::from(u64::MAX)) as u64)
        };
        Self { start, ..current }.clamp(horizon)
    }

    pub(super) fn pan_half(self, horizon: u64, right: bool) -> Self {
        let current = self.clamp(horizon);
        if current.length == 0 {
            return current;
        }
        let distance = (current.length / 2).max(1);
        let start = if right {
            current.start.saturating_add(distance)
        } else {
            current.start.saturating_sub(distance)
        };
        Self { start, ..current }.clamp(horizon)
    }

    pub(super) fn fraction(self, frame: u64) -> Option<f32> {
        let end = self.end()?;
        if self.length == 0 || frame < self.start || frame > end {
            return None;
        }
        Some((frame.saturating_sub(self.start) as f64 / self.length as f64) as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::Viewport;

    #[test]
    fn fit_and_end_are_checked() {
        assert_eq!(
            Viewport::fit(100),
            Viewport {
                start: 0,
                length: 100
            }
        );
        assert_eq!(
            Viewport {
                start: u64::MAX,
                length: 1
            }
            .end(),
            None
        );
    }

    #[test]
    fn zoom_anchors_at_playhead_or_center_and_clamps() {
        let viewport = Viewport {
            start: 100,
            length: 400,
        };
        assert_eq!(
            viewport.zoom(1_000, 200, true),
            Viewport {
                start: 150,
                length: 200
            }
        );
        assert_eq!(
            viewport.zoom(1_000, 900, true),
            Viewport {
                start: 200,
                length: 200
            }
        );
        assert_eq!(
            Viewport {
                start: 900,
                length: 100
            }
            .zoom(2_000, 950, false),
            Viewport {
                start: 850,
                length: 200
            }
        );
    }

    #[test]
    fn pan_moves_by_half_and_stays_inside_horizon() {
        let viewport = Viewport {
            start: 100,
            length: 200,
        };
        assert_eq!(
            viewport.pan_half(1_000, true),
            Viewport {
                start: 200,
                length: 200
            }
        );
        assert_eq!(
            viewport.pan_half(1_000, false),
            Viewport {
                start: 0,
                length: 200
            }
        );
        assert_eq!(
            Viewport {
                start: 990,
                length: 30
            }
            .pan_half(1_000, true),
            Viewport {
                start: 970,
                length: 30
            }
        );
    }

    #[test]
    fn zoom_and_pan_handle_large_horizons_without_overflow() {
        let viewport = Viewport {
            start: u64::MAX / 2,
            length: u64::MAX / 4,
        };
        assert!(viewport.zoom(u64::MAX, u64::MAX / 2, false).end().is_some());
        assert!(viewport.pan_half(u64::MAX, true).end().is_some());
        assert!(viewport.zoom(10, 5, false).end().is_some());
    }

    #[test]
    fn wheel_zoom_keeps_pointer_frame_at_the_same_fraction_and_clamps() {
        let viewport = Viewport {
            start: 100,
            length: 800,
        };
        let zoomed = viewport.zoom_at(2_000, 300, true);
        assert_eq!(zoomed.length, 640);
        let old_fraction = (300 - viewport.start) as f64 / viewport.length as f64;
        let new_fraction = (300 - zoomed.start) as f64 / zoomed.length as f64;
        assert!((old_fraction - new_fraction).abs() < 0.002);
        assert_eq!(zoomed.zoom_at(2_000, 300, false), viewport);
        assert_eq!(
            Viewport {
                start: 1_800,
                length: 200
            }
            .zoom_at(2_000, 1_999, false),
            Viewport {
                start: 1_750,
                length: 250
            }
        );
    }

    #[test]
    fn frame_pan_preserves_scale_and_respects_both_horizon_edges() {
        let viewport = Viewport {
            start: 100,
            length: 200,
        };
        assert_eq!(
            viewport.pan_by_frames(1_000, 50),
            Viewport {
                start: 150,
                length: 200
            }
        );
        assert_eq!(
            viewport.pan_by_frames(1_000, i128::MAX),
            Viewport {
                start: 800,
                length: 200
            }
        );
        assert_eq!(
            viewport.pan_by_frames(1_000, i128::MIN),
            Viewport {
                start: 0,
                length: 200
            }
        );
    }
}
