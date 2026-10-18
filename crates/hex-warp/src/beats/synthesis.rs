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
    step: f64,
    overlap: usize,
    range: f64,
    lower: i128,
    upper: i128,
    previous: [Option<[f64; 2]>; 64],
    previous_sum: [f64; 2],
    previous_square: f64,
    count: f64,
}
// The existing 4096-byte search quote covers both construction storage and
// the live search plus comparison records, including the new overlap array.
const _: () = assert!(
    std::mem::size_of::<Search<'static, 'static>>() * 2 + std::mem::size_of::<Match>() * 3 <= 4096
);
impl Search<'_, '_> {
    fn scan<const PRUNED: bool>(
        &self,
        lo: f64,
        hi: f64,
        step: f64,
        nominal: f64,
        mut selected: Match,
        work: &mut Work<'_>,
    ) -> Result<Match, Error> {
        let mut candidate = lo;
        while candidate <= hi {
            if !PRUNED || self.can_improve(candidate, nominal, selected.score, work)? {
                let next = self.score(candidate, nominal, work)?;
                if next.score > selected.score {
                    selected = next;
                }
            }
            candidate += step;
        }
        Ok(selected)
    }
    fn can_improve(
        &self,
        position: f64,
        nominal: f64,
        selected_score: f64,
        work: &mut Work<'_>,
    ) -> Result<bool, Error> {
        work.check(16)?;
        let displacement = (position - nominal) / self.range.max(1.0);
        // Correlation is clamped to <=1 and the energy penalty is >=0.
        // Use the same operation order as score(). If even this upper bound
        // cannot win its strict comparison, no waveform read can change the
        // selected candidate. This does not approximate or thin the search.
        Ok(1.0 - 0.002 * displacement * displacement > selected_score)
    }
    fn score(&self, position: f64, nominal: f64, work: &mut Work<'_>) -> Result<Match, Error> {
        let Self {
            request,
            step,
            overlap,
            range,
            lower,
            upper,
            ref previous,
            previous_sum,
            previous_square,
            count,
        } = *self;
        let points = overlap.clamp(1, 64);
        // The previous overlap's six units per point are computed once when
        // constructing Search. Only the ten candidate-dependent units repeat.
        work.check(points as u64 * 10)?;
        let sx = previous_sum;
        let mut sy = [0.0; 2];
        let mut xx = previous_square;
        let mut yy = 0.0;
        let mut xy = 0.0;
        for (i, previous) in previous.iter().enumerate().take(points) {
            let at = i * overlap / points;
            if let Some(x) = previous {
                let y = linear(request.input, position + at as f64 * step, lower, upper);
                for c in 0..2 {
                    sy[c] += y[c];
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
    planned_bed(
        request,
        bank,
        work,
        acc,
        Body {
            span,
            continuation: None,
        },
        pitch,
        schedule,
    )
}
#[derive(Clone, Copy)]
pub(super) struct Continuation {
    pub(super) source: f64,
    pub(super) target: f64,
    pub(super) native_end: usize,
    pub(super) fade: usize,
}
pub(super) struct Body {
    pub(super) span: Span,
    pub(super) continuation: Option<Continuation>,
}
pub(super) fn planned_bed(
    request: &Request<'_>,
    bank: &Bandlimited,
    work: &mut Work<'_>,
    acc: &mut Accumulator,
    body: Body,
    pitch: f64,
    schedule: &Schedule<'_>,
) -> Result<(), Error> {
    let Body { span, continuation } = body;
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
        if let Some(c) = continuation {
            nominal = c.source + (c.target - c.source) * fraction - hop as f64 * pitch;
        }
        let mut step = pitch;
        if matches!(
            request.mode,
            Mode::Beats {
                tail: Tail::Alternating,
                ..
            }
        ) && let Some((position, direction)) = if let Some(c) = continuation {
            let upper = c.target.min(source_end) - 1.0;
            let width = upper - source_start;
            if width >= 2.0 && center >= c.native_end as i128 {
                let phase =
                    ((center - c.native_end as i128 + 1) as f64 * pitch).rem_euclid(width * 2.0);
                Some(if phase < width {
                    (upper - phase, -1.0)
                } else {
                    (source_start + phase - width, 1.0)
                })
            } else {
                None
            }
        } else {
            schedule.reflected(span, center, pitch)
        } {
            step = pitch * direction;
            nominal = position - hop as f64 * step;
        }
        let mut forced = false;
        if continuation.is_none() && out <= start as i128 {
            step = pitch;
            nominal = source_start + (out - start as i128) as f64 * pitch;
            forced = true;
        }
        if locked_end && out >= end as i128 - grain as i128 {
            step = pitch;
            nominal =
                continuation.map_or(source_end, |c| c.target) - (end as i128 - out) as f64 * pitch;
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
        // Every candidate compares against the same immutable overlap. Cache
        // its actual samples and statistics on bounded stack scratch; preserve
        // the original summation order and candidate scoring exactly.
        let points = hop.clamp(1, 64);
        work.check(points as u64 * 6)?;
        let mut previous = [None; 64];
        let mut previous_sum = [0.0; 2];
        let mut previous_square = 0.0;
        let mut count = 0.0;
        for (i, value) in previous.iter_mut().enumerate().take(points) {
            let at = out + (i * hop / points) as i128;
            *value = if continuation.is_some() {
                acc.reference(at)
            } else {
                acc.preview(at)
            };
            if let Some(x) = value {
                count += 1.0;
                for c in 0..2 {
                    previous_sum[c] += x[c];
                    previous_square += x[c] * x[c];
                }
            }
        }
        let search = Search {
            request,
            step,
            overlap: hop,
            range,
            lower,
            upper,
            previous,
            previous_sum,
            previous_square,
            count,
        };
        let mut selected = search.score(nominal, nominal, work)?;
        let reference = if continuation.is_some() {
            acc.reference(out + hop as i128 / 2)
        } else {
            acc.preview(out + hop as i128 / 2)
        };
        if !forced && reference.is_some() {
            let step = f64::from((request.input.sample_rate / 8_000).max(1));
            // At a physical endpoint keep the bounded search WIDTH by moving
            // it inward; halving it can remove the only phase-coherent bass
            // candidate and manufacture end-of-region modulation.
            let lo = (nominal - range).clamp(minimum, (maximum - 2.0 * range).max(minimum));
            let hi = (lo + 2.0 * range).min(maximum);
            selected = search.scan::<true>(lo, hi, step, nominal, selected, work)?;
            let best = selected.position;
            selected = search.scan::<true>(
                (best - step).max(lo),
                (best + step).min(hi),
                0.125,
                nominal,
                selected,
                work,
            )?;
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
            let mut weight = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / grain as f64).cos();
            if let Some(c) = continuation {
                let x = ((at - start as i128) as f64 / c.fade.max(1) as f64).clamp(0.0, 1.0);
                weight *= x * x * (3.0 - 2.0 * x);
            }
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

#[cfg(test)]
mod search_tests {
    use super::*;
    fn bits(m: Match) -> [u64; 7] {
        [
            m.position.to_bits(),
            m.score.to_bits(),
            m.correlation.to_bits(),
            m.previous_mean[0].to_bits(),
            m.previous_mean[1].to_bits(),
            m.next_mean[0].to_bits(),
            m.next_mean[1].to_bits(),
        ]
    }
    #[test]
    fn exhaustive_and_pruned_search_select_identical_complete_matches() {
        let mut comparisons = 0;
        let mut overshoot_cases = 0;
        let mut nan_score_cases = 0;
        for pattern in 0..5 {
            let source: Vec<Frame> = (0..512)
                .map(|i| match pattern {
                    0 => [0.4, -0.2],
                    1 => {
                        if i % 2 == 0 {
                            [0.25, -0.5]
                        } else {
                            [-0.25, 0.5]
                        }
                    }
                    2 => [
                        (i as f32 * 0.13).sin() * 1e-12,
                        (i as f32 * 0.17).cos() * 1e-13,
                    ],
                    3 => [(i as f32 * 0.13).sin() * 0.4, (i as f32 * 0.31).cos() * 0.2],
                    _ => [0.0, (i as f32 * 0.07).sin() * 0.3],
                })
                .collect();
            let map = TimeMap::new(
                vec![MapSegment {
                    output_start: 0,
                    output_end: 512,
                    source_start_q32: 0,
                    source_end_q32: 512_i128 << 32,
                }],
                512,
            )
            .unwrap();
            let request = Request {
                input: Input {
                    frames: &source,
                    sample_rate: 16000,
                    region_start: 0,
                    region_length: 512,
                    phase: 0,
                    looping: false,
                    envelope: &[],
                },
                attacks: &[],
                map: &map,
                output_frames: 512,
                pitch_cents: 0,
                mode: Mode::Beats {
                    division_frames: None,
                    tail: Tail::Forward,
                    decay: 100,
                },
            };
            for direction in [-1.0, 1.0] {
                let nominal = 192.0;
                let previous = std::array::from_fn(|i| {
                    Some(source[(nominal + i as f64 * direction) as usize].map(f64::from))
                });
                let mut sum = [0.0; 2];
                let mut square = 0.0;
                for x in previous.iter().flatten() {
                    for c in 0..2 {
                        sum[c] += x[c];
                        square += x[c] * x[c];
                    }
                }
                for statistics in 0..6 {
                    let mut search = Search {
                        request: &request,
                        step: direction,
                        overlap: 64,
                        range: 32.0,
                        lower: 0,
                        upper: 511,
                        previous,
                        previous_sum: sum,
                        previous_square: square,
                        count: 64.0,
                    };
                    if statistics == 1 && square > 0.0 {
                        search.previous_square = f64::from_bits(square.to_bits() - 4);
                    }
                    if statistics == 2 {
                        search.previous_square = f64::NAN;
                    }
                    if statistics == 3 {
                        search.previous_sum[0] = f64::from_bits(0x7ff8000000000042);
                    }
                    if statistics == 4 {
                        search.previous[0].as_mut().unwrap()[0] = f64::NAN;
                    }
                    if statistics == 5 {
                        search.previous = [None; 64];
                        search.previous[32] = Some([0.25, -0.2]);
                        search.previous_sum = [0.25, -0.2];
                        search.previous_square = 0.25_f64.powi(2) + 0.2_f64.powi(2);
                        search.count = 1.0;
                    }
                    // The production search predicate's midpoint is present:
                    // this oracle intentionally excludes the count==0 branch.
                    assert!(search.count > 0.0 && search.previous[32].is_some());
                    let mut cancel = || false;
                    let mut work = Work {
                        cancel: &mut cancel,
                        used: 0,
                        scratch: 0,
                        base_scratch: 0,
                    };
                    let initial = search.score(nominal, nominal, &mut work).unwrap();
                    if pattern == 1 && statistics == 4 {
                        assert!(initial.score.is_nan());
                        nan_score_cases += 1;
                    }
                    if pattern == 1 && statistics == 1 {
                        let raw = square / (search.previous_square * square).sqrt();
                        assert!(raw > 1.0, "the fixture must exercise pre-clamp overshoot");
                        assert_eq!(initial.correlation, 1.0);
                        overshoot_cases += 1;
                    }
                    let score = initial.score;
                    let adjacent = if score > 0.0 {
                        f64::from_bits(score.to_bits() + 1)
                    } else {
                        f64::from_bits(1)
                    };
                    let below = if score > 0.0 {
                        f64::from_bits(score.to_bits() - 1)
                    } else {
                        -f64::from_bits(1)
                    };
                    for incumbent in [score, adjacent, below, f64::NAN] {
                        let mut initial = initial;
                        initial.score = incumbent;
                        initial.previous_mean[0] = f64::from_bits(0x7ff8000000000017);
                        work.used = 0;
                        let exhaustive = search
                            .scan::<false>(160.0, 224.0, 4.0, nominal, initial, &mut work)
                            .unwrap();
                        work.used = 0;
                        let pruned = search
                            .scan::<true>(160.0, 224.0, 4.0, nominal, initial, &mut work)
                            .unwrap();
                        assert_eq!(
                            bits(exhaustive),
                            bits(pruned),
                            "coarse {pattern}/{direction}/{statistics}"
                        );
                        // Same strict winner means exactly the same local
                        // fractional sequence, including ties/adjacent scores.
                        let lo = (exhaustive.position - 4.0).max(160.0);
                        let hi = (exhaustive.position + 4.0).min(224.0);
                        work.used = 0;
                        let exhaustive = search
                            .scan::<false>(lo, hi, 0.125, nominal, exhaustive, &mut work)
                            .unwrap();
                        work.used = 0;
                        let pruned = search
                            .scan::<true>(lo, hi, 0.125, nominal, pruned, &mut work)
                            .unwrap();
                        assert_eq!(
                            bits(exhaustive),
                            bits(pruned),
                            "fine {pattern}/{direction}/{statistics}"
                        );
                        comparisons += 2;
                    }
                }
            }
        }
        assert_eq!(overshoot_cases, 2);
        assert_eq!(nan_score_cases, 2);
        assert_eq!(comparisons, 480);
        println!(
            "exhaustive/pruned complete-Match bit identity: {comparisons} coarse/fine comparisons, positive overlap; overshoot/zero/ties/adjacent/NaNs/stereo directions"
        );
    }
}
