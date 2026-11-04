//! Sample-bounded crossfade between plugin output and its aligned dry history.

#[derive(Default)]
pub(super) struct BypassMix {
    target_bypassed: Option<bool>,
    dry_gain: f32,
    wet_gain: f32,
    target_wet_gain: f32,
    remaining: u32,
}

impl BypassMix {
    pub(super) fn process(
        &mut self,
        wet: &mut [[f32; 2]],
        dry: &[[f32; 2]],
        bypassed: bool,
        rate: u32,
        output_gain: f32,
    ) {
        let target_dry = if bypassed { 1.0 } else { 0.0 };
        let target_wet = if bypassed { 0.0 } else { output_gain };
        match self.target_bypassed {
            None => {
                self.target_bypassed = Some(bypassed);
                self.dry_gain = target_dry;
                self.wet_gain = target_wet;
                self.target_wet_gain = target_wet;
            }
            Some(previous) if previous != bypassed => {
                self.target_bypassed = Some(bypassed);
                self.target_wet_gain = target_wet;
                self.remaining = rate.div_ceil(200).max(1);
            }
            _ => {}
        }
        if !bypassed && self.target_wet_gain != target_wet {
            self.target_wet_gain = target_wet;
            if self.remaining > 0 {
                self.remaining = rate.div_ceil(200).max(1);
            } else {
                self.wet_gain = target_wet;
            }
        }
        if self.remaining == 0 {
            if bypassed {
                wet.copy_from_slice(dry);
            } else if output_gain != 1.0 {
                for frame in wet.iter_mut() {
                    frame[0] *= output_gain;
                    frame[1] *= output_gain;
                }
            }
            return;
        }
        for (wet, dry) in wet.iter_mut().zip(dry) {
            if self.remaining > 0 {
                self.dry_gain += (target_dry - self.dry_gain) / self.remaining as f32;
                self.wet_gain += (target_wet - self.wet_gain) / self.remaining as f32;
                self.remaining -= 1;
                if self.remaining == 0 {
                    self.dry_gain = target_dry;
                    self.wet_gain = target_wet;
                }
            }
            for channel in 0..2 {
                wet[channel] = (f64::from(dry[channel]) * f64::from(self.dry_gain)
                    + f64::from(wet[channel]) * f64::from(self.wet_gain))
                    as f32;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BypassMix;

    #[test]
    fn fresh_bypass_is_dry_and_does_not_fade_from_an_invented_history() {
        let mut mix = BypassMix::default();
        let mut wet = [[0.5, -0.5]; 4];
        let dry = [[0.25, -0.25]; 4];
        mix.process(&mut wet, &dry, true, 48_000, 2.0);
        assert_eq!(wet, dry);
    }

    #[test]
    fn bypass_starts_from_the_previous_audible_gain_when_trim_also_changes() {
        let mut mix = BypassMix::default();
        let dry = [[0.25, -0.25]; 1];
        let mut wet = [[0.5, -0.5]; 1];
        mix.process(&mut wet, &dry, false, 48_000, 2.0);
        let before = wet[0][0];
        let mut next = [[0.5, -0.5]; 1];
        mix.process(&mut next, &dry, true, 48_000, 0.5);
        let expected = before + (0.25 - before) / 240.0;
        assert!((next[0][0] - expected).abs() < 1e-6);
    }
}
