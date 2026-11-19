//! Deterministic original oscillator/morph laws; state is transaction-local.
//! Morph phase-warps sine/triangle, bends ramp exponents, changes square duty,
//! slews S&H over a fraction of its cycle, and bends Wander interpolation.
#[derive(Clone, Copy, Debug)]
pub(super) struct Lfo {
    phase: f64,
    random: u64,
    previous: f64,
    next: f64,
}
impl Default for Lfo {
    fn default() -> Self {
        Self {
            phase: 0.0,
            random: 0x4d595df4d0f33173,
            previous: 0.0,
            next: 0.5,
        }
    }
}
impl Lfo {
    pub(super) fn process(&mut self, wave: u32, morph: f64, increment: f64) -> f64 {
        let p = self.phase;
        let skew = 0.01 + 0.98 * morph;
        let warped = if p < skew {
            p / skew * 0.5
        } else {
            0.5 + (p - skew) / (1.0 - skew) * 0.5
        };
        let v = match wave {
            0 => (std::f64::consts::TAU * warped).sin(),
            1 => 1.0 - 4.0 * (warped - 0.5).abs(),
            2 => 2.0 * p.powf(2.0_f64.powf((morph - 0.5) * 4.0)) - 1.0,
            3 => 1.0 - 2.0 * p.powf(2.0_f64.powf((morph - 0.5) * 4.0)),
            4 => {
                if p < skew {
                    1.0
                } else {
                    -1.0
                }
            }
            5 => {
                let blend = if morph <= 0.0 {
                    1.0
                } else {
                    (p / morph).min(1.0)
                };
                self.previous + (self.next - self.previous) * blend
            }
            _ => {
                let blend = p * p * (3.0 - 2.0 * p);
                self.previous + (self.next - self.previous) * ((1.0 - morph) * p + morph * blend)
            }
        };
        self.phase += increment.clamp(0.0, 0.49);
        if self.phase >= 1.0 {
            self.phase -= 1.0;
            self.previous = self.next;
            self.random ^= self.random << 13;
            self.random ^= self.random >> 7;
            self.random ^= self.random << 17;
            self.next = (self.random >> 11) as f64 / ((1_u64 << 53) - 1) as f64 * 2.0 - 1.0;
        }
        v
    }
}
