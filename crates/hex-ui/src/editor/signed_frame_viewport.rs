//! Bounded display windows in an explicitly chosen frame clock. Negative starts
//! reveal virtual space; they never authorize a negative transport or source read.
//! Arithmetic is independent of project/model state and allocates no frame data.
use super::viewport::Viewport;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SignedFrameViewport {
    pub(super) start: i128,
    pub(super) length: u64,
}
impl From<Viewport> for SignedFrameViewport {
    fn from(view: Viewport) -> Self {
        Self {
            start: i128::from(view.start),
            length: view.length,
        }
    }
}
impl SignedFrameViewport {
    pub(super) fn end(self) -> Option<i128> {
        self.start.checked_add(i128::from(self.length))
    }
    pub(super) fn shifted(self, origin: i128) -> Option<Self> {
        Some(Self {
            start: self.start.checked_add(origin)?,
            ..self
        })
    }
    pub(super) fn bounded(self, minimum: i128, maximum: i128) -> Option<Self> {
        let available = maximum.checked_sub(minimum)?;
        if available < 0 {
            return None;
        }
        let length = self
            .length
            .min(u64::try_from(available).unwrap_or(u64::MAX));
        let last = maximum.checked_sub(i128::from(length))?;
        Some(Self {
            start: self.start.clamp(minimum, last),
            length,
        })
    }
    pub(super) fn contains(self, frame: i128) -> bool {
        self.end()
            .is_some_and(|end| frame >= self.start && frame <= end)
    }
    /// Signed fraction for clipping/canvas geometry, including outside points.
    pub(super) fn fraction(self, frame: i128) -> Option<f64> {
        if self.length == 0 || self.end().is_none() {
            return None;
        }
        Some(frame.checked_sub(self.start)? as f64 / self.length as f64)
    }
    /// Only this intersection may reach existing unsigned read/grid APIs.
    pub(super) fn visible_unsigned(self) -> Option<Viewport> {
        let end = self.end()?.clamp(0, i128::from(u64::MAX));
        let first = self.start.clamp(0, i128::from(u64::MAX));
        if first >= end {
            return None;
        }
        Some(Viewport {
            start: u64::try_from(first).ok()?,
            length: u64::try_from(end - first).ok()?,
        })
    }
    pub(super) fn intersect(self, first: u64, end: u64) -> Option<(u64, u64)> {
        let start = self.start.max(i128::from(first));
        let end = self.end()?.min(i128::from(end));
        if start >= end {
            return None;
        }
        Some((u64::try_from(start).ok()?, u64::try_from(end).ok()?))
    }
    pub(super) fn pan(self, minimum: i128, maximum: i128, delta: i128) -> Option<Self> {
        let current = self.bounded(minimum, maximum)?;
        Self {
            start: current.start.saturating_add(delta),
            ..current
        }
        .bounded(minimum, maximum)
    }
    pub(super) fn zoom(
        self,
        minimum: i128,
        maximum: i128,
        anchor: i128,
        inward: bool,
        wheel: bool,
    ) -> Option<Self> {
        let current = self.bounded(minimum, maximum)?;
        if current.length == 0 {
            return Some(current);
        }
        let available = u128::try_from(maximum.checked_sub(minimum)?)
            .ok()?
            .min(u128::from(u64::MAX));
        let next = match (inward, wheel) {
            (true, true) => (u128::from(current.length) * 4 / 5).max(1),
            (false, true) => (u128::from(current.length) * 5).div_ceil(4),
            (true, false) => u128::from((current.length / 2).max(1)),
            (false, false) => u128::from(current.length) * 2,
        }
        .min(available);
        let length = u64::try_from(next).ok()?;
        let end = current.end()?;
        let anchor = if wheel {
            anchor.clamp(current.start, end)
        } else if current.contains(anchor) {
            anchor
        } else {
            current.start.checked_add(i128::from(current.length / 2))?
        };
        let offset = u128::try_from(anchor.checked_sub(current.start)?).ok()?;
        let scaled = offset
            .checked_mul(u128::from(length))?
            .checked_div(u128::from(current.length))?;
        Self {
            start: anchor.checked_sub(i128::try_from(scaled).ok()?)?,
            length,
        }
        .bounded(minimum, maximum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn negative_space_projects_and_intersects_without_authorizing_a_read() {
        let view = SignedFrameViewport {
            start: -50,
            length: 100,
        };
        assert_eq!(
            view.visible_unsigned(),
            Some(Viewport {
                start: 0,
                length: 50
            })
        );
        assert_eq!(view.intersect(20, 70), Some((20, 50)));
        assert_eq!(view.fraction(0), Some(0.5));
        assert_eq!(
            view.shifted(1000).unwrap().visible_unsigned(),
            Some(Viewport {
                start: 950,
                length: 100
            })
        );
        assert!(
            SignedFrameViewport {
                start: -100,
                length: 50
            }
            .visible_unsigned()
            .is_none()
        );
    }
    #[test]
    fn pan_and_zoom_cross_zero_and_keep_the_pointer_anchor() {
        let view = SignedFrameViewport {
            start: -100,
            length: 200,
        };
        assert_eq!(
            view.zoom(-1000, 1000, 0, true, true),
            Some(SignedFrameViewport {
                start: -80,
                length: 160
            })
        );
        assert_eq!(
            view.zoom(-1000, 1000, 400, true, false),
            Some(SignedFrameViewport {
                start: -50,
                length: 100
            })
        );
        assert_eq!(view.pan(-1000, 1000, 150).unwrap().start, 50);
        assert_eq!(view.pan(-1000, 1000, i128::MIN).unwrap().start, -1000);
        assert_eq!(view.pan(-1000, 1000, i128::MAX).unwrap().start, 800);
    }
    #[test]
    fn malformed_extreme_and_empty_windows_remain_bounded() {
        let view = SignedFrameViewport {
            start: i128::MAX,
            length: u64::MAX,
        };
        assert!(view.end().is_none());
        assert!(view.fraction(0).is_none());
        assert!(view.bounded(1, 0).is_none());
        assert!(view.bounded(i128::MIN, i128::MAX).is_none());
        assert_eq!(
            view.bounded(-1000, 1000),
            Some(SignedFrameViewport {
                start: -1000,
                length: 2000
            })
        );
        assert_eq!(
            view.bounded(0, 0),
            Some(SignedFrameViewport {
                start: 0,
                length: 0
            })
        );
        let edge = SignedFrameViewport {
            start: i128::from(u64::MAX) - 1,
            length: 100,
        };
        assert_eq!(
            edge.visible_unsigned(),
            Some(Viewport {
                start: u64::MAX - 1,
                length: 1
            })
        );
    }
}
