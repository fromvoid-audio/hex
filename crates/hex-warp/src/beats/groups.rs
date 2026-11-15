//! Chosen-grid timing: one native first pass, then eligible sustain only.
use super::*;
fn smooth(x: f64) -> f64 {
    x * x * (3.0 - 2.0 * x)
}
fn output_background(acc: &Accumulator, rate: u32, work: &mut Work<'_>) -> Result<[f64; 2], Error> {
    let count = (rate as usize / 2000).max(2).min(acc.cursor);
    if count == 0 {
        return Ok([0.0; 2]);
    }
    work.check(count as u64 * 16)?;
    let center = (count - 1) as f64 * 0.5;
    let mut sum = [0.0; 2];
    let mut square = [0.0; 2];
    let mut slope = [0.0; 2];
    let mut denominator = 0.0;
    for (i, x) in acc.output[acc.cursor - count..acc.cursor]
        .iter()
        .enumerate()
    {
        let t = i as f64 - center;
        denominator += t * t;
        for c in 0..2 {
            let sample = f64::from(x[c]);
            sum[c] += sample;
            square[c] += sample * sample;
            slope[c] += t * sample;
        }
    }
    let variance: f64 = (0..2)
        .map(|c| (square[c] - sum[c] * sum[c] / count as f64).max(0.0))
        .sum();
    let fitted = if denominator > 0.0 {
        slope.iter().map(|x| x * x / denominator).sum()
    } else {
        0.0
    };
    Ok(if variance - fitted <= variance * 0.05 + 1e-16 {
        acc.previous()
    } else {
        sum.map(|x| x / count as f64)
    })
}

struct SourceBounds<'a> {
    physical: &'a [Anchor],
    previous: usize,
    next: usize,
}
#[derive(Clone, Copy)]
struct FirstPass {
    start: f64,
    end: f64,
    pitch: f64,
    excluded_before: Option<f64>,
}
impl<'a> SourceBounds<'a> {
    fn new(physical: &'a [Anchor]) -> Self {
        Self {
            physical,
            previous: 0,
            next: 1,
        }
    }
    fn advance(&mut self, pass: FirstPass, age: usize) {
        let position = pass.start + age as f64 * pass.pitch;
        while self.next < self.physical.len() - 1
            && (self.physical[self.next].source <= position
                || self.physical[self.next].source < pass.end
                    && ((self.physical[self.next].source - pass.start) / pass.pitch).floor()
                        <= age as f64)
        {
            if self.physical[self.next].attack {
                self.previous = self.next;
            }
            self.next += 1;
        }
        while self.next < self.physical.len() - 1 && !self.physical[self.next].attack {
            self.next += 1;
        }
    }
    fn sample(
        &mut self,
        request: &Request<'_>,
        bank: &Bandlimited,
        pass: FirstPass,
        age: usize,
    ) -> [f64; 2] {
        self.advance(pass, age);
        let position = pass.start + age as f64 * pass.pitch;
        let onset = self.physical[self.previous].source;
        let mut lower = onset.floor() as i128;
        if pass
            .excluded_before
            .is_some_and(|cut| onset >= cut && onset < pass.start)
        {
            lower = lower.max(pass.start.ceil() as i128);
        }
        let upper = self.physical[self.next].source.ceil() as i128 - 1;
        bank.sample_segment(request.input, position, lower, upper.max(lower))
    }
    fn protected(&self, position: f64, guard: f64) -> bool {
        let a = self.physical[self.previous];
        a.attack && position - a.source < guard
    }
}
pub(super) fn render(
    request: &Request<'_>,
    physical: &[Anchor],
    schedule: &plan::Schedule<'_>,
    bank: &Bandlimited,
    pitch: f64,
    work: &mut Work<'_>,
) -> Result<Vec<Frame>, Error> {
    let Mode::Beats { tail, .. } = request.mode else {
        return Err(Error::Invalid("grouped mode mismatch"));
    };
    let ring = (request.input.sample_rate as usize / 20).max(4) * 2 + 4;
    let mut acc = Accumulator::new(
        request.output_frames,
        ring,
        work,
        plan_bytes(request) + bank_bytes(work.resampler) + synthesis::SEARCH_BYTES,
    )?;
    let mut bounds = SourceBounds::new(physical);
    let guard = f64::from(request.input.sample_rate) / 125.0;
    let mut previous_cut_attack = None;
    let mut previous_aligned = false;
    let mut previous_source_cut = None;
    let mut previous_gap = false;
    let mut future = 0;
    let mut last_attack = None;
    for group in 0..schedule.len() - 1 {
        let a = schedule.point(group);
        let b = schedule.point(group + 1);
        let duration = b.output - a.output;
        let sounding = (((b.source - a.source) / pitch).ceil() as usize).min(duration);
        let native_end = a.output + sounding;
        let pass = FirstPass {
            start: a.source,
            end: b.source,
            pitch,
            excluded_before: previous_source_cut,
        };
        // Physical events, including omitted late events, remain source
        // exclusion barriers. A chosen grid is not itself a physical attack.
        work.check(4)?;
        while future < physical.len() {
            work.check(4)?;
            if !physical[future].attack {
                future += 1;
                continue;
            }
            if physical[future].source >= b.source {
                break;
            }
            last_attack = Some(future);
            future += 1;
        }
        let next_attack = physical.get(future).map(|x| x.source);
        let safe_start = if let Some(index) = last_attack {
            let onset = physical[index].source;
            let next = next_attack.unwrap_or(physical.last().expect("physical[0] exists").source);
            onset + guard.min((next - onset).max(0.0) / 3.0)
        } else {
            physical[0].source
        };
        let safe_end = next_attack.map_or(
            physical.last().expect("physical[0] exists").source,
            |onset| {
                onset
                    - (f64::from(request.input.sample_rate) / 1000.0)
                        .min((onset - safe_start).max(0.0) / 3.0)
            },
        );
        let sustain = tail != Tail::Off
            && native_end < b.output
            && b.source - safe_start >= 4.0
            && safe_end - safe_start >= 4.0;
        let align_end = group + 2 < schedule.len() && b.source <= safe_end;
        let overlap = if sustain {
            (request.input.sample_rate as usize / 500)
                .max(1)
                .min(sounding / 4)
        } else {
            0
        };
        let blend_start = native_end - overlap;
        let join = if group > 0 {
            (request.input.sample_rate as usize / if previous_gap { 500 } else { 200 })
                .min(sounding)
        } else {
            0
        };
        let correction = if join >= 2 && !previous_aligned {
            let prior = physical[0].source.floor() as i128;
            let prior_onset = physical
                .iter()
                .rev()
                .find(|x| x.attack && x.source < a.source)
                .filter(|x| previous_source_cut.is_some_and(|cut| x.source >= cut));
            let baseline_end = prior_onset.map_or(a.source, |x| x.source);
            let baseline = stop::restart_baseline(
                request.input,
                bank,
                prior,
                baseline_end.ceil() as i128,
                pitch,
                work,
            )?
            .endpoint;
            let outgoing = if let Some(onset) = previous_cut_attack {
                stop::restart_baseline(request.input, bank, prior, onset, pitch, work)?.endpoint
            } else {
                output_background(&acc, request.input.sample_rate, work)?
            };
            std::array::from_fn(|c| baseline[c] - outgoing[c])
        } else {
            [0.0; 2]
        };
        let release = if tail == Tail::Off && native_end < b.output {
            (request.input.sample_rate as usize / 1000).min(sounding / 4)
        } else {
            0
        };
        for age in 0..sounding {
            if age % 256 == 0 {
                work.check(256 * 64)?;
            }
            let at = a.output + age;
            let position = a.source + age as f64 * pitch;
            let mut sample = bounds.sample(request, bank, pass, age);
            let correction_gain = if join >= 2 && age < join {
                1.0 - smooth(age as f64 / (join - 1) as f64)
            } else {
                0.0
            };
            let remaining = sounding - age - 1;
            // A release may shorten background, never erase an arriving onset
            // core merely because it lies just before a selected boundary.
            let gain = if release > 0 && remaining < release && !bounds.protected(position, guard) {
                smooth(remaining as f64 / release as f64)
            } else {
                1.0
            };
            for c in 0..2 {
                sample[c] = (sample[c] - correction[c] * correction_gain) * gain;
            }
            let weight = if overlap > 0 && at >= blend_start {
                1.0 - smooth((at - blend_start) as f64 / overlap as f64)
            } else {
                1.0
            };
            acc.add(at as i128, sample, weight)?;
            acc.finish_until((at + 1).saturating_sub(overlap).max(a.output))?;
        }
        if duration > 0 {
            previous_cut_attack = if native_end == b.output
                && sounding > 0
                && bounds.protected(a.source + (sounding - 1) as f64 * pitch, guard)
            {
                Some(physical[bounds.previous].source.ceil() as i128)
            } else {
                None
            };
        }
        let cut = a.source + duration as f64 * pitch;
        previous_source_cut = if duration == 0 {
            Some(previous_source_cut.map_or(cut, |previous| previous.min(cut)))
        } else {
            (cut < b.source).then_some(cut)
        };
        if sustain {
            acc.finish_until(blend_start)?;
            synthesis::planned_bed(
                request,
                bank,
                work,
                &mut acc,
                synthesis::Body {
                    span: plan::Span {
                        start: blend_start,
                        end: b.output,
                        source_start: safe_start,
                        source_end: safe_end,
                        locked_end: align_end,
                    },
                    continuation: Some(synthesis::Continuation {
                        source: a.source + (blend_start - a.output) as f64 * pitch,
                        target: b.source,
                        native_end,
                        fade: overlap,
                    }),
                },
                pitch,
                schedule,
            )?;
            previous_aligned = align_end;
        } else {
            previous_aligned = native_end == b.output
                && (a.source + duration as f64 * pitch - b.source).abs() < 1e-9;
            acc.finish_until(native_end)?;
            // No eligible safe tail exists for a pulse-only/empty interval.
            // Do not repeat its protected attack or hold its final sample.
            for at in native_end..b.output {
                if at % 1024 == 0 {
                    work.check(1024)?;
                }
                acc.add(at as i128, [0.0; 2], 1.0)?;
                acc.finish_until(at + 1)?;
            }
        }
        if duration > 0 {
            previous_gap = !sustain && native_end < b.output;
        }
        acc.finish_until(b.output)?;
    }
    acc.finish()
}
