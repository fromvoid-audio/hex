//! Checked directed finite read intervals. Coordinates belong to the caller's
//! clip clock (frames or raw quarter-note beats); no source or DSP is owned here.
use crate::Invalid;
use std::ops::Range;

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
}
