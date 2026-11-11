//! Original two-pole Butterworth high/low cascade; not reference coefficients.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}
impl Biquad {
    fn tune(&mut self, frequency: f64, rate: u32, high: bool) {
        let omega =
            std::f64::consts::TAU * frequency.clamp(5.0, f64::from(rate) * 0.45) / f64::from(rate);
        let (sin, cos) = omega.sin_cos();
        let alpha = sin / std::f64::consts::SQRT_2;
        let norm = 1.0 / (1.0 + alpha);
        self.a = [-2.0 * cos * norm, (1.0 - alpha) * norm];
        self.b = if high {
            [
                (1.0 + cos) * 0.5 * norm,
                -(1.0 + cos) * norm,
                (1.0 + cos) * 0.5 * norm,
            ]
        } else {
            [
                (1.0 - cos) * 0.5 * norm,
                (1.0 - cos) * norm,
                (1.0 - cos) * 0.5 * norm,
            ]
        };
    }
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z = [
            self.b[1] * x - self.a[0] * y + self.z[1],
            self.b[2] * x - self.a[1] * y,
        ];
        // Clear the state pair together. Independently snapping one component
        // can create a near-floor limit cycle in a high-pass recurrence.
        if self.z.iter().all(|z| z.abs() < 1e-20) {
            self.z = [0.0; 2];
        }
        y
    }
    fn magnitude(self, omega: f64) -> f64 {
        let (s, c) = omega.sin_cos();
        let (s2, c2) = (2.0 * omega).sin_cos();
        let n = (self.b[0] + self.b[1] * c + self.b[2] * c2).powi(2)
            + (self.b[1] * s + self.b[2] * s2).powi(2);
        let d = (1.0 + self.a[0] * c + self.a[1] * c2).powi(2)
            + (self.a[0] * s + self.a[1] * s2).powi(2);
        (n / d.max(1e-30)).sqrt()
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct BandPass {
    high: Biquad,
    low: Biquad,
}
impl BandPass {
    /// Preserve coefficients, but never duplicate the donor's signal memory.
    pub(super) fn without_signal_memory(mut self) -> Self {
        self.high.z = [0.0; 2];
        self.low.z = [0.0; 2];
        self
    }
    pub(super) fn tune(&mut self, frequency: f64, width: f64, rate: u32) {
        let span = 2.0_f64.powf(width * 0.5);
        self.high.tune(frequency / span, rate, true);
        self.low.tune(frequency * span, rate, false);
    }
    pub(super) fn process(&mut self, x: f64) -> f64 {
        self.low.process(self.high.process(x))
    }
    pub(super) fn quiet(self) -> bool {
        self.high.z == [0.0; 2] && self.low.z == [0.0; 2]
    }
    pub(super) fn magnitude(self, hz: f64, rate: u32) -> f64 {
        let w = std::f64::consts::TAU * hz / f64::from(rate);
        self.high.magnitude(w) * self.low.magnitude(w)
    }
}
