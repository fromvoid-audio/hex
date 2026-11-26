//! Output-only bridge across a verified-page hold. The transport and recording
//! taps advance solely for real rendered frames.
use hex_engine::Frame;

const RAMP_FRAMES: usize = 64;

#[derive(Default)]
pub(super) struct PageFade {
    last: Option<Frame>,
    origin: Frame,
    remaining: usize,
    holding: bool,
}

impl PageFade {
    /// The caller supplies committed PCM when playing, or silence when held.
    /// Retain the last *emitted* sample so a reversal midway through either
    /// ramp is continuous, including across short callback slices.
    pub(super) fn apply(&mut self, output: &mut [Frame], holding: bool) {
        if output.is_empty() {
            return;
        }
        if self.holding != holding {
            if let Some(last) = self.last {
                self.origin = last;
                self.remaining = RAMP_FRAMES;
            }
            self.holding = holding;
        }
        for frame in output.iter_mut().take(self.remaining) {
            self.remaining -= 1;
            let from = self.remaining as f32 / RAMP_FRAMES as f32;
            for (sample, origin) in frame.iter_mut().zip(self.origin) {
                *sample = *sample * (1.0 - from) + origin * from;
            }
        }
        // Initial pre-roll has no last audible sample from which to resume.
        if !holding || self.last.is_some() {
            self.last = output.last().copied();
        }
    }

    /// Keep the last emitted sample aligned after an outer seek fade-out
    /// scales the committed slice, so an unexpected mid-fade hold bridges
    /// smoothly from the faded level.
    pub(super) fn sync_last(&mut self, frame: Frame) {
        self.last = Some(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_stereo_ramps_reverse_from_the_last_emitted_sample() {
        let mut fade = PageFade::default();
        let source = [0.75, -0.25];
        fade.apply(&mut [source], false);
        let mut gap = [[0.0; 2]; 8];
        fade.apply(&mut gap, true);
        let from = gap[7];
        let next = [-0.25, 0.75];
        let mut resumed = [next; 1];
        fade.apply(&mut resumed, false);
        for channel in 0..2 {
            assert_eq!(
                resumed[0][channel],
                from[channel] + (next[channel] - from[channel]) / RAMP_FRAMES as f32
            );
        }
        gap.fill([0.0; 2]);
        fade.apply(&mut gap, true);
        assert_eq!(
            gap[0],
            resumed[0].map(|sample| sample * (1.0 - 1.0 / RAMP_FRAMES as f32))
        );
    }
}
