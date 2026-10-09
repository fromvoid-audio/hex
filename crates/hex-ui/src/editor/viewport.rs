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
}
