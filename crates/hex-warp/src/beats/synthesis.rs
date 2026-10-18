use super::*;
use plan::{Schedule, Span};
#[derive(Clone, Copy)]
struct Match {
    position: f64,
    score: f64,
    correlation: f64,
    previous_mean: [f64; 2],
    next_mean: [f64; 2],
}
fn linear(input: Input<'_>, position: f64, lower: i128, upper: i128) -> [f64; 2] {
    let i = position.floor() as i128;
    let a = input.frame(i.clamp(lower, upper));
    let b = input.frame((i + 1).clamp(lower, upper));
    let f = position - position.floor();
    std::array::from_fn(|c| a[c] * (1.0 - f) + b[c] * f)
}
struct Search<'r, 'a> {
    request: &'r Request<'a>,
    acc: &'r Accumulator,
    out: i128,
    step: f64,
    overlap: usize,
    range: f64,
    lower: i128,
    upper: i128,
}
impl Search<'_, '_> {
    fn score(&self, position: f64, nominal: f64, work: &mut Work<'_>) -> Result<Match, Error> {
        let Self {
            request,
            acc,
            out,
            step,
            overlap,
            range,
            lower,
            upper,
        } = *self;
        let points = overlap.clamp(1, 64);
        work.check(points as u64 * 16)?;
        let mut count = 0.0;
        let mut sx = [0.0; 2];
        let mut sy = [0.0; 2];
        let mut xx = 0.0;
        let mut yy = 0.0;
        let mut xy = 0.0;
        for i in 0..points {
            let at = i * overlap / points;
            if let Some(x) = acc.preview(out + at as i128) {
                let y = linear(request.input, position + at as f64 * step, lower, upper);
                count += 1.0;
                for c in 0..2 {
                    sx[c] += x[c];
                    sy[c] += y[c];
                    xx += x[c] * x[c];
                    yy += y[c] * y[c];
                    xy += x[c] * y[c];
                }
            }
        }
        if count == 0.0 {
            return Ok(Match {
                position,
                score: 0.0,
                correlation: 1.0,
                previous_mean: [0.0; 2],
                next_mean: [0.0; 2],
            });
        }
        for c in 0..2 {
            xx -= sx[c] * sx[c] / count;
            yy -= sy[c] * sy[c] / count;
            xy -= sx[c] * sy[c] / count;
        }
        let correlation = if xx.max(0.0) * yy.max(0.0) > 1e-20 {
            (xy / (xx.max(0.0) * yy.max(0.0)).sqrt()).clamp(-1.0, 1.0)
        } else if xx.abs() < 1e-12 && yy.abs() < 1e-12 {
            1.0
        } else {
            0.0
        };
        let energy = if xx > 1e-20 && yy > 1e-20 {
            ((yy / xx).ln()).abs().min(8.0)
        } else {
            0.0
        };
        let displacement = (position - nominal) / range.max(1.0);
        Ok(Match {
            position,
            score: correlation - 0.002 * displacement * displacement - 0.02 * energy,
            correlation,
            previous_mean: sx.map(|s| s / count),
            next_mean: sy.map(|s| s / count),
        })
    }
}
pub(super) fn bed(
    request: &Request<'_>,
    bank: &Bandlimited,
    work: &mut Work<'_>,
    acc: &mut Accumulator,
    span: Span,
    pitch: f64,
    schedule: &Schedule<'_>,
) -> Result<(), Error> {
    let Span {
        start,
        end,
        source_start,
        source_end,
        locked_end,
    } = span;
    if start >= end {
        return Ok(());
    }
    if source_end - source_start < 4.0 {
        return short_splice(request, bank, work, acc, span, pitch);
    }
    let lower = source_start.floor() as i128;
    let upper = source_end.ceil() as i128 - 1;
    let available = ((source_end - source_start) / pitch).floor().max(0.0) as usize;
    let grain = (request.input.sample_rate as usize / 25)
        .min(available)
        .min((available / 4).max(4))
        .min((end - start) * 2);
    if grain < 4 {
        return short_splice(request, bank, work, acc, span, pitch);
    }
    let grain = grain - grain % 2;
    let hop = grain / 2;
    let first_map = request.map.position(start as i128)?;
    let last_map = request.map.position(end as i128)?;
    let mut out = start as i128 - hop as i128;
    while out < end as i128 {
        work.check(16)?;
        let center = out + hop as i128;
        let fraction = ((center - start as i128) as f64 / (end - start) as f64).clamp(0.0, 1.0);
        let mapped = request.map.position(center)?;
        let mut nominal = mapped
            + (source_start - first_map) * (1.0 - fraction)
            + (source_end - last_map) * fraction
            - hop as f64 * pitch;
        let mut step = pitch;
        if matches!(
            request.mode,
            Mode::Beats {
                tail: Tail::Alternating,
                ..
            }
        ) && let Some((position, direction)) = schedule.reflected(span, center, pitch)
        {
            step = pitch * direction;
            nominal = position - hop as f64 * step;
        }
        let mut forced = false;
        if out <= start as i128 {
            step = pitch;
            nominal = source_start + (out - start as i128) as f64 * pitch;
            forced = true;
        }
        if locked_end && out >= end as i128 - grain as i128 {
            step = pitch;
            nominal = source_end - (end as i128 - out) as f64 * pitch;
            forced = true;
        }
        let (minimum, maximum) = if step > 0.0 {
            (
                source_start,
                (source_end - grain as f64 * pitch).max(source_start),
            )
        } else {
            (
                (source_start + (grain - 1) as f64 * pitch).min(source_end - 1.0),
                source_end - 1.0,
            )
        };
        if !forced {
            nominal = nominal.clamp(minimum, maximum);
        }
        let range = (f64::from(request.input.sample_rate) * 0.012 * pitch)
            .min((source_end - source_start) * 0.25)
            .max(1.0);
        let search = Search {
            request,
            acc,
            out,
            step,
            overlap: hop,
            range,
            lower,
            upper,
        };
        let mut selected = search.score(nominal, nominal, work)?;
        if !forced && acc.preview(out + hop as i128 / 2).is_some() {
            let step = f64::from((request.input.sample_rate / 8_000).max(1));
            // At a physical endpoint keep the bounded search WIDTH by moving
            // it inward; halving it can remove the only phase-coherent bass
            // candidate and manufacture end-of-region modulation.
            let lo = (nominal - range).clamp(minimum, (maximum - 2.0 * range).max(minimum));
            let hi = (lo + 2.0 * range).min(maximum);
            let mut candidate = lo;
            while candidate <= hi {
                let next = search.score(candidate, nominal, work)?;
                if next.score > selected.score {
                    selected = next;
                }
                candidate += step;
            }
            let best = selected.position;
            let mut candidate = (best - step).max(lo);
            while candidate <= (best + step).min(hi) {
                let next = search.score(candidate, nominal, work)?;
                if next.score > selected.score {
                    selected = next;
                }
                candidate += 0.125;
            }
            if selected.correlation < 0.5 {
                // With no useful alignment evidence, unpitched stochastic
                // material needs no fractional reconstruction. A nearest
                // source-sample start keeps its original noise distribution
                // instead of adding sinc interpolation peaks for no timing
                // benefit. Drift is bounded by half a source sample; attack
                // anchors remain separate and exact. Pitched grains retain
                // their required bandlimited reconstruction.
                let position = if pitch == 1.0 {
                    nominal.round().clamp(minimum, maximum)
                } else {
                    nominal
                };
                selected = search.score(position, nominal, work)?;
            }
        }
        work.check(grain as u64 * 64)?;
        for i in 0..grain {
            let at = out + i as i128;
            if at < start as i128 || at >= end as i128 {
                continue;
            }
            let weight = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / grain as f64).cos();
            let sample = bank.sample_segment(
                request.input,
                selected.position + i as f64 * step,
                lower,
                upper,
            );
            acc.add_correlated(
                at,
                sample,
                weight,
                selected.correlation,
                selected.previous_mean,
                selected.next_mean,
            )?;
        }
        out += hop as i128;
        acc.finish_until((out.max(start as i128) as usize).min(end))?;
    }
    acc.finish_until(end)
}

// With no meaningful safe sustain, play only the available body once, then a
// bounded endpoint splice. Never turn a future attack into a held sample by
// forcing an empty interval's upper bound up to its lower bound.
fn short_splice(
    request: &Request<'_>,
    bank: &Bandlimited,
    work: &mut Work<'_>,
    acc: &mut Accumulator,
    span: Span,
    pitch: f64,
) -> Result<(), Error> {
    let duration = span.end - span.start;
    let native =
        (((span.source_end - span.source_start).max(0.0) / pitch).ceil() as usize).min(duration);
    let release = (request.input.sample_rate as usize / 2000)
        .max(1)
        .min(duration - native);
    let mut last = acc.previous();
    for age in 0..duration {
        if age % 256 == 0 {
            work.check(256 * 64)?;
        }
        let sample = if age < native {
            let lower = span.source_start.floor() as i128;
            let upper = span.source_end.ceil() as i128 - 1;
            last = bank.sample_segment(
                request.input,
                span.source_start + age as f64 * pitch,
                lower,
                upper,
            );
            last
        } else if age - native < release {
            let x = (age - native + 1) as f64 / release as f64;
            last.map(|s| s * (1.0 - x * x * (3.0 - 2.0 * x)))
        } else {
            [0.0; 2]
        };
        acc.add((span.start + age) as i128, sample, 1.0)?;
    }
    acc.finish_until(span.end)
}
