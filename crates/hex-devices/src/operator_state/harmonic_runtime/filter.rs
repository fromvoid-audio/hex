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
    pub fn inactive(rate: u32) -> Self {
        let frame = Frame::dry();
        Self {
            from: frame,
            target: frame,
            elapsed: 0,
            frames: (rate * 5 + 500) / 1000,
            armed: false,
        }
    }

    pub fn new(
        settings: Settings,
        rate: u32,
    ) -> Result<Self, hex_lattice_contract::DeviceContractError> {
        let frame = settings.prepare(rate)?.frame;
        Ok(Self {
            from: frame,
            target: frame,
            elapsed: 0,
            frames: (rate * 5 + 500) / 1000,
            armed: settings.armed,
        })
    }
    pub fn oversampled(
        settings: Settings,
        host_rate: u32,
    ) -> Result<Self, hex_lattice_contract::DeviceContractError> {
        let frame = settings.prepare_oversampled(host_rate)?.frame;
        Ok(Self {
            from: frame,
            target: frame,
            elapsed: 0,
            frames: (host_rate * 5 + 500) / 1000,
            armed: settings.armed,
        })
    }
    pub fn frame(self) -> Frame {
        self.from.interpolate(
            self.target,
            f64::from(self.elapsed) / f64::from(self.frames),
        )
    }
    pub fn retarget(&mut self, target: Self) {
        if !self.armed && target.armed {
            *self = target;
            self.from.wet = 0.0;
            self.elapsed = 0;
            return;
        }
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
