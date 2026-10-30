//! One native traversal per logical segment, then exact intentional silence.
use super::*;
pub(super) struct Baseline {
    pub(super) fitted: [f64; 2],
    pub(super) endpoint: [f64; 2],
}
/// One-sided endpoint estimate, never sampling the arriving attack. A locally
/// predictable background uses its endpoint; incoherent noise uses its local
/// mean so a random last sample does not become a new decaying low-frequency
/// pulse. The predictability decision and time window are shared by stereo.
pub(super) fn restart_baseline(
    input: Input<'_>,
    bank: &Bandlimited,
    start: i128,
    end: i128,
    pitch: f64,
    work: &mut Work<'_>,
) -> Result<Baseline, Error> {
    if end <= start {
        return Ok(Baseline {
            fitted: [0.0; 2],
            endpoint: [0.0; 2],
        });
    }
    let available = (((end - start) as f64 / pitch).floor() as usize).max(1);
    let count = ((input.sample_rate / 2_000).max(2) as usize).min(available);
    work.check(count as u64 * 64)?;
    let center = (count - 1) as f64 * 0.5;
    let mut sum = [0.0; 2];
    let mut square = [0.0; 2];
    let mut slope = [0.0; 2];
    let mut denominator = 0.0;
    let mut minimum = [f64::INFINITY; 2];
    let mut maximum = [f64::NEG_INFINITY; 2];
    let mut endpoint = [0.0; 2];
    for i in 0..count {
        let t = i as f64 - center;
        let sample = bank.sample_segment(
            input,
            end as f64 - (count - i) as f64 * pitch,
            start,
            end - 1,
        );
        endpoint = sample;
        denominator += t * t;
        for c in 0..2 {
            sum[c] += sample[c];
            square[c] += sample[c] * sample[c];
            slope[c] += t * sample[c];
            minimum[c] = minimum[c].min(sample[c]);
            maximum[c] = maximum[c].max(sample[c]);
        }
    }
    let variance: f64 = (0..2)
        .map(|c| (square[c] - sum[c] * sum[c] / count as f64).max(0.0))
        .sum();
    let fitted = if denominator > 0.0 {
        slope.iter().map(|s| s * s / denominator).sum()
    } else {
        0.0
    };
    let predictable = variance - fitted <= variance * 0.05 + 1e-16;
    let fitted = std::array::from_fn(|c| {
        let mean = sum[c] / count as f64;
        if predictable && denominator > 0.0 {
            (mean + slope[c] / denominator * center).clamp(minimum[c], maximum[c])
        } else {
            mean
        }
    });
    Ok(Baseline {
        fitted,
        endpoint: if predictable {
            endpoint
        } else {
            sum.map(|s| s / count as f64)
        },
    })
}

struct Bounds<'a> {
    physical: &'a [Anchor],
    previous: usize,
    next: usize,
}
impl<'a> Bounds<'a> {
    fn new(physical: &'a [Anchor]) -> Self {
        let next = (1..physical.len())
            .find(|i| physical[*i].attack || *i == physical.len() - 1)
            .expect("the final anchor always terminates the search");
        Self {
            physical,
            previous: 0,
            next,
        }
    }
    fn sample(&mut self, input: Input<'_>, bank: &Bandlimited, position: f64) -> [f64; 2] {
        while self.next < self.physical.len() - 1 && position >= self.physical[self.next].source {
            self.previous = self.next;
            self.next = (self.next + 1..self.physical.len())
                .find(|i| self.physical[*i].attack || *i == self.physical.len() - 1)
                .expect("the final anchor always terminates the search");
        }
        let lower = self.physical[self.previous].source.floor() as i128;
        let upper = self.physical[self.next].source.ceil() as i128 - 1;
        bank.sample_segment(input, position, lower, upper.max(lower))
    }
}
fn smooth(x: f64) -> f64 {
    x * x * x * (10.0 + x * (-15.0 + 6.0 * x))
}
pub(super) fn render(
    request: &Request<'_>,
    physical: &[Anchor],
    schedule: &plan::Schedule<'_>,
    bank: &Bandlimited,
    pitch: f64,
    work: &mut Work<'_>,
) -> Result<Vec<Frame>, Error> {
    let Mode::Beats { decay, .. } = request.mode else {
        return Err(Error::Invalid("stop mode mismatch"));
    };
    let mut output = buffer(request.output_frames, [0.0; 2])?;
    work.check(physical.len() as u64 * 4)?;
    // Native and join positions each advance monotonically; interleaving them
    // in one cursor would accidentally apply future-attack bounds to the body.
    let mut native_bounds = Bounds::new(physical);
    let mut join_bounds = Bounds::new(physical);
    let mut previous_gap = false;
    for index in 0..schedule.len() - 1 {
        let a = schedule.point(index);
        let b = schedule.point(index + 1);
        let duration = b.output - a.output;
        let native_duration = (b.source - a.source) / pitch;
        let sounding = duration.min(native_duration.ceil() as usize);
        let changed = (duration as f64 - native_duration).abs() > 1e-6;
        let gap = sounding < duration;
        let onset = if previous_gap {
            (request.input.sample_rate as usize / 500).min(sounding)
        } else {
            0
        };
        let baseline = if onset >= 2 {
            restart_baseline(
                request.input,
                bank,
                schedule.point(index - 1).source.floor() as i128,
                a.source.ceil() as i128,
                pitch,
                work,
            )?
            .fitted
        } else {
            [0.0; 2]
        };
        let join = if changed && decay == 100 && index + 2 < schedule.len() && !gap {
            (request.input.sample_rate as usize / 500).min(duration / 4)
        } else {
            0
        };
        let release = if changed && join == 0 && (gap || index + 2 == schedule.len()) {
            (request.input.sample_rate as usize / 1000).min(sounding / 4)
        } else {
            0
        };
        for age in 0..sounding {
            if age % 256 == 0 {
                work.check(256 * 64)?;
            }
            let position = a.source + age as f64 * pitch;
            if position >= b.source {
                continue;
            }
            let mut sample = native_bounds.sample(request.input, bank, position);
            let restart = if onset >= 2 && age < onset {
                1.0 - smooth(age as f64 / (onset - 1) as f64)
            } else {
                0.0
            };
            let remaining = sounding - age - 1;
            let safety = if release > 0 && remaining < release {
                let x = remaining as f64 / release as f64;
                x * x * (3.0 - 2.0 * x)
            } else {
                1.0
            };
            for c in 0..2 {
                sample[c] = (sample[c] - baseline[c] * restart) * safety;
            }
            let until_next = duration - age;
            if join > 0 && until_next <= join {
                work.check(64)?;
                let incoming =
                    join_bounds.sample(request.input, bank, b.source - until_next as f64 * pitch);
                let weight = smooth((join - until_next + 1) as f64 / join as f64);
                sample = std::array::from_fn(|c| sample[c] * (1.0 - weight) + incoming[c] * weight);
            }
            output[a.output + age] = finite(sample)?;
        }
        previous_gap = gap;
    }
    Ok(output)
}
