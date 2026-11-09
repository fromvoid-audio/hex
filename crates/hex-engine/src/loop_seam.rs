//! Arrangement transport loop end envelope and regression coverage.
//!
//! The envelope is applied only to Arrangement audio before MIDI and effect
//! tails are mixed. It is addressed by the Arrangement clock, so block splits
//! do not alter it. Loop starts retain their authored level.

use crate::Frame;
use hex_project::LoopRegion;

#[derive(Clone, Copy)]
pub(crate) struct LoopEndFade {
    end: u64,
    frames: u64,
}

impl LoopEndFade {
    pub(crate) fn new(region: LoopRegion, sample_rate: u32) -> Option<Self> {
        // Keep short-loop attenuation to at most one quarter of the cycle,
        // and retain the first sample of every iteration at full level.
        let length = region.end() - region.start();
        let frames = length
            .saturating_sub(1)
            .min((length / 4).max(1))
            .min(u64::from(crate::playback::fade_frames(sample_rate)));
        (frames > 0).then_some(Self {
            end: region.end(),
            frames,
        })
    }

    pub(crate) fn intersects(self, start: u64, len: usize) -> bool {
        start < self.end && start.saturating_add(len as u64) > self.end - self.frames
    }

    pub(crate) fn apply(self, start: u64, output: &mut [Frame]) {
        for (offset, frame) in output.iter_mut().enumerate() {
            let at = start.saturating_add(offset as u64);
            if at < self.end && at >= self.end - self.frames {
                let remaining = self.end - at - 1;
                let gain = if self.frames == 1 {
                    0.0
                } else {
                    remaining as f32 / (self.frames - 1) as f32
                };
                frame[0] *= gain;
                frame[1] *= gain;
            }
        }
    }
}

#[cfg(test)]
mod tests;
