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
