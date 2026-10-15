//! Carrier integrators and their independent coefficient clock travel with each voice.
use crate::operator_filter::{Frame, Settings};
#[derive(Clone, Copy)]
pub(in crate::operator_state) struct Trajectory {
    pub from: Frame,
    pub target: Frame,
    pub elapsed: u32,
    pub frames: u32,
    pub armed: bool,
}
impl Trajectory {
    pub fn new(settings: Settings, rate: u32) -> Self {
        let frame = settings
            .prepare(rate)
            .expect("validated filter settings")
            .frame;
        Self {
            from: frame,
            target: frame,
            elapsed: 0,
            frames: (rate * 5 + 500) / 1000,
            armed: settings.armed,
        }
    }
    pub fn frame(self) -> Frame {
        self.from.interpolate(
            self.target,
            f64::from(self.elapsed) / f64::from(self.frames),
        )
    }
    pub fn retarget(&mut self, target: Self) {
        self.armed |= target.armed;
        if self.target != target.target {
            self.from = self.frame();
            self.target = target.target;
            self.elapsed = 0;
            self.frames = target.frames;
        }
    }
    pub fn advance(&mut self) {
        self.elapsed = self.elapsed.saturating_add(1).min(self.frames);
    }
}
