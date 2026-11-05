//! Checked directed finite read intervals. Coordinates belong to the caller's
//! clip clock (frames or raw quarter-note beats); no source or DSP is owned here.
use crate::Invalid;
use std::ops::Range;

/// Constant-space finite loop seam plan, in elapsed clip-clock units. Only
/// internal seams are returned; an introduction becomes a nonrepeating tail
/// when reflected. This is display geometry, never a note scheduler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepeatBoundaryPlan {
    first: u64,
    period: u64,
    count: u64,
}
impl RepeatBoundaryPlan {
    pub fn count(self) -> u64 {
        self.count
    }
    pub fn boundary(self, index: u64) -> Option<u64> {
        (index < self.count).then_some(())?;
        self.first.checked_add(index.checked_mul(self.period)?)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentWindow {
    pub start: u64,
    pub end: u64,
    pub reversed: bool,
}
impl ContentWindow {
    pub fn new(start: u64, end: u64, reversed: bool) -> Result<Self, Invalid> {
        if start >= end {
            return Err(Invalid("content read interval must be positive"));
        }
        Ok(Self {
            start,
            end,
            reversed,
        })
    }
    pub fn length(self) -> Result<u64, Invalid> {
        self.end
            .checked_sub(self.start)
            .filter(|n| *n > 0)
            .ok_or(Invalid("content read interval must be positive"))
    }
    /// Project the forward seams `first + n * period` through this finite read
    /// window. Work and storage are constant even for extremely dense loops.
    pub fn repeat_boundaries(
        self,
        first: u64,
        period: u64,
    ) -> Result<Option<RepeatBoundaryPlan>, Invalid> {
        self.length()?;
        if first == 0 || period == 0 {
            return Err(Invalid("repeat boundary and period must be positive"));
        }
        if self.end <= first {
            return Ok(None);
        }
        let last = (self.end - 1 - first) / period;
        let initial = if self.start < first {
            0
        } else {
            (self.start - first) / period + 1
        };
        if initial > last {
            return Ok(None);
        }
        let index = if self.reversed { last } else { initial };
        let source = first
            .checked_add(
                index
                    .checked_mul(period)
                    .ok_or(Invalid("repeat boundary overflow"))?,
            )
            .ok_or(Invalid("repeat boundary overflow"))?;
        Ok(Some(RepeatBoundaryPlan {
            first: if self.reversed {
                self.end - source
            } else {
                source - self.start
            },
            period,
            count: last - initial + 1,
        }))
    }
    /// Map a half-open elapsed interval, retaining the same finite gate length.
    pub fn read_range(self, range: Range<u64>) -> Result<Range<u64>, Invalid> {
        let length = self.length()?;
        if range.start > range.end || range.end > length {
            return Err(Invalid(
                "content selection exceeds its finite read interval",
            ));
        }
        Ok(if self.reversed {
            self.end - range.end..self.end - range.start
        } else {
            self.start + range.start..self.start + range.end
        })
    }
    /// A sample or raw beat quantum occupies [elapsed,elapsed+1).
    pub fn read_position(self, elapsed: u64) -> Result<u64, Invalid> {
        if elapsed >= self.length()? {
            return Err(Invalid("content position exceeds its finite read interval"));
        }
        Ok(if self.reversed {
            self.end - 1 - elapsed
        } else {
            self.start + elapsed
        })
    }
    /// Retain or extend edges in current audible order. Hidden content remains
    /// addressable, but reads before source elapsed zero are explicitly refused.
    pub fn resized(self, left_delta: i128, length: u64) -> Result<Self, Invalid> {
        self.length()?;
        let (start, end) = if self.reversed {
            let end = i128::from(self.end)
                .checked_sub(left_delta)
                .ok_or(Invalid("content edge overflow"))?;
            (
                end.checked_sub(i128::from(length))
                    .ok_or(Invalid("content edge overflow"))?,
                end,
            )
        } else {
            let start = i128::from(self.start)
                .checked_add(left_delta)
                .ok_or(Invalid("content edge overflow"))?;
            (
                start,
                start
                    .checked_add(i128::from(length))
                    .ok_or(Invalid("content edge overflow"))?,
            )
        };
        Self::new(
            u64::try_from(start).map_err(|_| Invalid("content edge precedes retained source"))?,
            u64::try_from(end).map_err(|_| Invalid("content edge exceeds retained source"))?,
            self.reversed,
        )
    }
    /// Select a retained interval in current audible order, preserving direction.
    pub fn cropped(self, range: Range<u64>) -> Result<Self, Invalid> {
        let read = self.read_range(range)?;
        Self::new(read.start, read.end, self.reversed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finite_reflection_preserves_gates_and_composes_with_crops() {
        let view = ContentWindow::new(7, 19, true).unwrap();
        assert_eq!(view.read_range(2..5).unwrap(), 14..17);
        assert_eq!(
            view.cropped(2..10).unwrap().read_range(1..3).unwrap(),
            view.read_range(3..5).unwrap()
        );
        assert_eq!(
            (0..12)
                .map(|i| view.read_position(i).unwrap())
                .collect::<Vec<_>>(),
            (7..19).rev().collect::<Vec<_>>()
        );
        assert!(view.read_range(0..13).is_err());
        assert!(ContentWindow::new(3, 3, false).is_err());
        let whole = ContentWindow::new(0, u64::MAX, true).unwrap();
        assert_eq!(whole.read_range(0..u64::MAX).unwrap(), 0..u64::MAX);
    }
    #[test]
    fn repeat_seams_reflect_only_internal_boundaries_and_bound_dense_windows() {
        for (reversed, expected) in [(false, [3, 8]), (true, [2, 7])] {
            let plan = ContentWindow::new(3, 13, reversed)
                .unwrap()
                .repeat_boundaries(6, 5)
                .unwrap()
                .unwrap();
            assert_eq!(plan.count(), 2);
            assert_eq!(
                [plan.boundary(0).unwrap(), plan.boundary(1).unwrap()],
                expected
            );
            assert_eq!(plan.boundary(2), None);
        }
        let plan = ContentWindow::new(3, 11, true)
            .unwrap()
            .repeat_boundaries(6, 5)
            .unwrap()
            .unwrap();
        assert_eq!((plan.count(), plan.boundary(0)), (1, Some(5)));
        assert_eq!(
            ContentWindow::new(0, 6, true)
                .unwrap()
                .repeat_boundaries(6, 5)
                .unwrap(),
            None
        );
        let window = ContentWindow::new(0, u64::MAX, true).unwrap();
        let dense = window.repeat_boundaries(1, 1).unwrap().unwrap();
        assert_eq!(dense.count(), u64::MAX - 1);
        assert_eq!(dense.boundary(dense.count() - 1), Some(u64::MAX - 1));
        assert!(window.repeat_boundaries(1, 0).is_err());
    }
}
