//! Windowed-sinc reconstruction with rate-dependent anti-alias cutoff.
use super::*;
const TAPS: usize = 64;
const PHASES: usize = 256;
pub(super) const BANK_BYTES: usize = TAPS * PHASES * 8;
pub(super) struct Bandlimited {
    coefficients: Vec<f64>,
    rate: f64,
}
impl Bandlimited {
    pub(super) fn new(rate: f64, work: &mut Work<'_>) -> Result<Self, Error> {
        work.scratch(BANK_BYTES)?;
        let mut result = Self {
            coefficients: buffer(TAPS * PHASES, 0.0)?,
            rate: 0.0,
        };
        result.set_rate(rate, work)?;
        Ok(result)
    }
    pub(super) fn set_rate(&mut self, rate: f64, work: &mut Work<'_>) -> Result<(), Error> {
        if !rate.is_finite() || rate <= 0.0 || rate > 128.0 {
            return Err(Error::Invalid("invalid resampling rate"));
        }
        if self.rate.to_bits() == rate.to_bits() {
            return Ok(());
        }
        work.check((TAPS * PHASES) as u64)?;
        self.rate = rate;
        let cutoff = rate.recip().min(1.0);
        for phase in 0..PHASES {
            let fraction = phase as f64 / PHASES as f64;
            let mut sum = 0.0;
            for tap in 0..TAPS {
                let x = tap as f64 - (TAPS / 2 - 1) as f64 - fraction;
                let sinc = if x.abs() < 1e-12 {
                    cutoff
                } else {
                    (std::f64::consts::PI * x * cutoff).sin() / (std::f64::consts::PI * x)
                };
                let radius = (x / (TAPS as f64 * 0.5)).clamp(-1.0, 1.0);
                let window = 0.42
                    + 0.5 * (std::f64::consts::PI * radius).cos()
                    + 0.08 * (2.0 * std::f64::consts::PI * radius).cos();
                let coefficient = sinc * window;
                self.coefficients[phase * TAPS + tap] = coefficient;
                sum += coefficient;
            }
            if !sum.is_finite() || sum.abs() < 1e-12 {
                return Err(Error::Invalid("invalid resampling filter normalization"));
            }
            for value in &mut self.coefficients[phase * TAPS..(phase + 1) * TAPS] {
                *value /= sum;
            }
        }
        Ok(())
    }
    pub(super) fn sample(&self, input: Input<'_>, position_q32: i128) -> [f64; 2] {
        self.sample_interval(input, position_q32, None)
    }
    fn sample_interval(
        &self,
        input: Input<'_>,
        position_q32: i128,
        interval: Option<(i128, i128)>,
    ) -> [f64; 2] {
        // A transient segment may not read the following attack through its
        // reconstruction filter, including during a tail wrap or reversal.
        let frame =
            |at: i128| input.frame(interval.map_or(at, |(start, end)| at.clamp(start, end)));
        let integer = position_q32.div_euclid(Q32);
        let fraction = position_q32.rem_euclid(Q32);
        if fraction == 0 && self.rate <= 1.0 {
            return frame(integer);
        }
        let phase = (fraction * PHASES as i128 / Q32) as usize;
        let mut result = [0.0; 2];
        for tap in 0..TAPS {
            let sample = frame(integer + tap as i128 - (TAPS / 2 - 1) as i128);
            let coefficient = self.coefficients[phase * TAPS + tap];
            result[0] += sample[0] * coefficient;
            result[1] += sample[1] * coefficient;
        }
        result
    }
    pub(super) fn sample_float(&self, input: Input<'_>, position: f64) -> [f64; 2] {
        self.sample(input, (position * Q32 as f64).round() as i128)
    }
    pub(super) fn sample_segment(
        &self,
        input: Input<'_>,
        position: f64,
        start: i128,
        end: i128,
    ) -> [f64; 2] {
        self.sample_interval(
            input,
            (position * Q32 as f64).round() as i128,
            Some((start, end)),
        )
    }
}
pub(super) fn render(
    request: &Request<'_>,
    pitch: f64,
    work: &mut Work<'_>,
) -> Result<Vec<Frame>, Error> {
    let mut output = buffer(request.output_frames, [0.0; 2])?;
    let pitch_q32 = (pitch * Q32 as f64).round() as i128;
    let first_rate = if request.mode == Mode::Resample {
        pitch
    } else {
        request.map.rate_at(0)
    };
    let mut bank = Bandlimited::new(first_rate, work)?;
    for (index, frame) in output.iter_mut().enumerate() {
        if index % 1_024 == 0 {
            work.check(1_024 * TAPS as u64)?;
        }
        let mut position = request.map.position_q32(index as i128)?;
        if request.mode == Mode::Resample {
            position = position
                .checked_mul(pitch_q32)
                .ok_or(Error::Invalid("pitch position overflow"))?
                / Q32;
        } else {
            bank.set_rate(request.map.rate_at(index as u64), work)?;
        }
        *frame = finite(bank.sample(request.input, position))?;
    }
    Ok(output)
}
