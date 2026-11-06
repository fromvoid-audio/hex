//! Half-open selection and viewport geometry in original take frames.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Range {
    pub(super) start: u64,
    pub(super) end: u64,
}
impl Range {
    pub(super) fn full(frames: u64) -> Self {
        Self {
            start: 0,
            end: frames,
        }
    }
    pub(super) fn frames(self) -> u64 {
        self.end - self.start
    }
    pub(super) fn between(a: u64, b: u64, frames: u64) -> Self {
        let start = a.min(b).min(frames - 1);
        let end = a.max(b).clamp(start + 1, frames);
        Self { start, end }
    }
    pub(super) fn zoom(self, frame: u64, frames: u64, inward: bool) -> Self {
        let len = if inward {
            (self.frames() / 2).max(2).min(frames)
        } else {
            self.frames().saturating_mul(2).min(frames)
        };
        let start = frame.saturating_sub(len / 2).min(frames - len);
        Self {
            start,
            end: start + len,
        }
    }
    pub(super) fn pan(self, frame: u64, frames: u64) -> Self {
        let len = self.frames();
        let start = frame.saturating_sub(len / 2).min(frames - len);
        Self {
            start,
            end: start + len,
        }
    }
    pub(super) fn slide(self, anchor: u64, to: u64, frames: u64, stepped: bool) -> Self {
        let len = self.frames();
        let delta = i128::from(to) - i128::from(anchor);
        let step = i128::from(len);
        let delta = if stepped {
            ((delta.abs() + step / 2) / step) * step * delta.signum()
        } else {
            delta
        };
        let requested = i128::from(self.start) + delta;
        let start = requested.clamp(0, i128::from(frames - len)) as u64;
        if stepped && i128::from(start) != requested {
            return self;
        }
        Self {
            start,
            end: start + len,
        }
    }
}
