//! Detect silent windows, not merely a nonzero final digest or whole-run peak.
use super::Fallible;

#[derive(Clone, Copy, Debug)]
pub(super) struct Summary {
    pub windows: usize,
    pub min_peak: f32,
    pub final_peak: f32,
}
pub(super) struct Activity {
    rate: usize,
    frames: usize,
    peak: f32,
    summary: Summary,
}
impl Activity {
    pub fn new(rate: u32) -> Self {
        Self {
            rate: rate as usize,
            frames: 0,
            peak: 0.0,
            summary: Summary {
                windows: 0,
                min_peak: f32::INFINITY,
                final_peak: 0.0,
            },
        }
    }
    pub fn observe(&mut self, frames: &[[f32; 2]]) -> Fallible<()> {
        for frame in frames {
            for sample in frame {
                if !sample.is_finite() {
                    return Err("profile output is not finite".into());
                }
                self.peak = self.peak.max(sample.abs());
            }
            self.frames += 1;
            if self.frames == self.rate {
                self.finish_window()?;
            }
        }
        Ok(())
    }
    fn finish_window(&mut self) -> Fallible<()> {
        if self.peak < 1.0e-6 {
            return Err(
                "profile measured window is silent or below fixture activity threshold".into(),
            );
        }
        self.summary.windows += 1;
        self.summary.min_peak = self.summary.min_peak.min(self.peak);
        self.summary.final_peak = self.peak;
        self.frames = 0;
        self.peak = 0.0;
        Ok(())
    }
    pub fn finish(mut self) -> Fallible<Summary> {
        if self.frames > 0 {
            self.finish_window()?;
        }
        if self.summary.windows == 0 {
            return Err("profile measured no activity windows".into());
        }
        Ok(self.summary)
    }
}
