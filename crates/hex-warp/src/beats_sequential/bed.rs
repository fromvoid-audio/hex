//! Bounded grain-by-grain WSOLA bed and short-splice synthesis for `SequentialBeats`.
//!
//! Matches `beats/synthesis.rs` candidate screening, pruned correlation scoring,
//! correlation-compensated overlap accumulation, alternating tail reflection,
//! and short-span fallback bit-for-bit without requiring a full-file PCM buffer.
use crate::{
    Error, Frame, GranularOverlap, GranularSource, Mode, Tail, TimeMap, resample::Bandlimited,
};

#[derive(Clone, Copy, Debug)]
pub(super) struct Boundary {
    pub(super) source: f64,
    pub(super) output: u64,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct BedSpan {
    pub(super) start: u64,
    pub(super) end: u64,
    pub(super) source_start: f64,
    pub(super) source_end: f64,
    pub(super) locked_end: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct BedContinuation {
    pub(super) source: f64,
    pub(super) target: f64,
    pub(super) native_end: u64,
    pub(super) fade: usize,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum BedState {
    ShortSplice {
        span: BedSpan,
        age: u64,
        native: u64,
        release: u64,
        last: [f64; 2],
    },
    Wsola {
        span: BedSpan,
        continuation: Option<BedContinuation>,
        lower: i128,
        upper: i128,
        grain: usize,
        hop: usize,
        first_map: f64,
        last_map: f64,
        out: i128,
        previous_position: Option<(f64, f64)>,
    },
}

/// Tiny ring buffer of recently drained pre-decay frames for `acc.previous()`,
/// `acc.reference()`, and `groups::output_background()`.
#[derive(Clone)]
pub(super) struct DrainedHistory {
    frames: Vec<Frame>,
    cursor: u64,
}

impl DrainedHistory {
    pub(super) fn new(sample_rate: u32) -> Result<Self, Error> {
        let len = (sample_rate as usize / 50).max(64) + 16;
        let frames = crate::buffer(len, [0.0; 2])?;
        Ok(Self { frames, cursor: 0 })
    }

    pub(super) fn backing_bytes(&self) -> Option<usize> {
        self.frames
            .capacity()
            .checked_mul(std::mem::size_of::<Frame>())
    }

    pub(super) fn push(&mut self, frame: Frame) {
        let len = self.frames.len() as u64;
        let idx = (self.cursor % len) as usize;
        self.frames[idx] = frame;
        self.cursor += 1;
    }

    pub(super) fn previous(&self) -> [f64; 2] {
        if self.cursor == 0 {
            return [0.0; 2];
        }
        let len = self.frames.len() as u64;
        let idx = ((self.cursor - 1) % len) as usize;
        self.frames[idx].map(f64::from)
    }

    pub(super) fn get(&self, frame: u64) -> Option<[f64; 2]> {
        if frame >= self.cursor {
            return None;
        }
        let len = self.frames.len() as u64;
        if self.cursor - frame > len {
            return None;
        }
        let idx = (frame % len) as usize;
        Some(self.frames[idx].map(f64::from))
    }

    pub(super) fn reference(&self, overlap: &GranularOverlap, frame: i128) -> Option<[f64; 2]> {
        let pos = u64::try_from(frame).ok()?;
        if pos < overlap.position() {
            self.get(pos)
        } else {
            overlap.preview(frame)
        }
    }

    pub(super) fn output_background(&self, rate: u32) -> [f64; 2] {
        let count = ((rate as usize / 2000).max(2) as u64)
            .min(self.cursor)
            .min(self.frames.len() as u64) as usize;
        if count == 0 {
            return [0.0; 2];
        }
        let center = (count - 1) as f64 * 0.5;
        let mut sum = [0.0; 2];
        let mut square = [0.0; 2];
        let mut slope = [0.0; 2];
        let mut denominator = 0.0;
        let start = self.cursor - count as u64;
        for i in 0..count {
            let x = self.get(start + i as u64).unwrap_or([0.0; 2]);
            let t = i as f64 - center;
            denominator += t * t;
            for c in 0..2 {
                let sample = x[c];
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
        if variance - fitted <= variance * 0.05 + 1e-16 {
            self.previous()
        } else {
            sum.map(|x| x / count as f64)
        }
    }

    pub(super) fn checkpoint(&self, budget: usize) -> Result<Self, Error> {
        let needed = self
            .frames
            .len()
            .checked_mul(std::mem::size_of::<Frame>())
            .ok_or(Error::ScratchLimit)?;
        if needed > budget {
            return Err(Error::ScratchLimit);
        }
        let mut frames = crate::buffer(self.frames.len(), [0.0; 2])?;
        frames.copy_from_slice(&self.frames);
        let cp = Self {
            frames,
            cursor: self.cursor,
        };
        if cp.backing_bytes().is_none_or(|b| b > budget) {
            return Err(Error::ScratchLimit);
        }
        Ok(cp)
    }

    pub(super) fn refresh_checkpoint(&self, cp: &mut Self) -> Result<(), Error> {
        if cp.frames.len() != self.frames.len() {
            return Err(Error::Invalid("incompatible beats history checkpoint"));
        }
        cp.frames.copy_from_slice(&self.frames);
        cp.cursor = self.cursor;
        Ok(())
    }

    pub(super) fn restore(&mut self, cp: &Self) -> Result<(), Error> {
        if cp.frames.len() != self.frames.len() {
            return Err(Error::Invalid("incompatible beats history checkpoint"));
        }
        self.frames.copy_from_slice(&cp.frames);
        self.cursor = cp.cursor;
        Ok(())
    }
}

pub(super) fn init_bed(
    span: BedSpan,
    continuation: Option<BedContinuation>,
    sample_rate: u32,
    pitch: f64,
    map: &TimeMap,
    history: &DrainedHistory,
) -> Result<Option<BedState>, Error> {
    if span.start >= span.end {
        return Ok(None);
    }
    let duration = span.end - span.start;
    let available = ((span.source_end - span.source_start) / pitch)
        .floor()
        .max(0.0) as u64;
    let max_g = (sample_rate as u64 / 25)
        .min(available)
        .min((available / 4).max(4))
        .min(duration.saturating_mul(2));
    if span.source_end - span.source_start < 4.0 || max_g < 4 {
        let native =
            (((span.source_end - span.source_start).max(0.0) / pitch).ceil() as u64).min(duration);
        let release = u64::from(sample_rate / 2000).max(1).min(duration - native);
        return Ok(Some(BedState::ShortSplice {
            span,
            age: 0,
            native,
            release,
            last: history.previous(),
        }));
    }
    let grain = (max_g as usize) - (max_g as usize) % 2;
    let hop = grain / 2;
    let lower = span.source_start.floor() as i128;
    let upper = span.source_end.ceil() as i128 - 1;
    let first_map = map.position(i128::from(span.start))?;
    let last_map = map.position(i128::from(span.end))?;
    Ok(Some(BedState::Wsola {
        span,
        continuation,
        lower,
        upper,
        grain,
        hop,
        first_map,
        last_map,
        out: i128::from(span.start) - hop as i128,
        previous_position: None,
    }))
}

#[derive(Clone, Copy)]
struct Match {
    position: f64,
    score: f64,
    correlation: f64,
    previous_mean: [f64; 2],
    next_mean: [f64; 2],
}

#[derive(Clone, Copy)]
struct Screen {
    sum: [f64; 2],
    square: f64,
    count: f64,
}

fn reflected_in_window(
    boundaries: &[Boundary],
    span: BedSpan,
    output: i128,
    pitch: f64,
) -> Option<(f64, f64)> {
    if boundaries.len() < 2 {
        return None;
    }
    let out = output.max(0) as u64;
    let mut lo = 0;
    let mut hi = boundaries.len() - 1;
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if boundaries[mid].output <= out {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let a = boundaries[lo];
    let b = boundaries[lo + 1];
    let start = a.output.max(span.start);
    let end = b.output.min(span.end);
    let lower = a.source.max(span.source_start);
    let upper = b.source.min(span.source_end) - 1.0;
    let width = upper - lower;
    if width < 2.0 || (end.saturating_sub(start)) as f64 * pitch <= width + 1.0 {
        return None;
    }
    let position = lower + (output - i128::from(start)) as f64 * pitch;
    if position <= upper {
        return Some((position, 1.0));
    }
    let phase = (position - upper).rem_euclid(width * 2.0);
    if phase < width {
        Some((upper - phase, -1.0))
    } else {
        Some((lower + phase - width, 1.0))
    }
}

struct WindowedLookup<'a, S> {
    source: &'a mut S,
    cached_start: i128,
    cached: &'a [[f64; 2]],
}

impl<S: GranularSource> WindowedLookup<'_, S> {
    fn frame(&mut self, at: i128) -> Result<[f64; 2], S::Error> {
        if !self.cached.is_empty()
            && let Some(off) = at.checked_sub(self.cached_start)
            && let Ok(idx) = usize::try_from(off)
            && let Some(val) = self.cached.get(idx)
        {
            return Ok(*val);
        }
        self.source.frame(at)
    }

    fn linear(&mut self, position: f64, lower: i128, upper: i128) -> Result<[f64; 2], S::Error> {
        let upper = upper.max(lower);
        let i = position.floor() as i128;
        let a = self.frame(i.clamp(lower, upper))?;
        let b = self.frame((i + 1).clamp(lower, upper))?;
        let f = position - position.floor();
        Ok(std::array::from_fn(|c| a[c] * (1.0 - f) + b[c] * f))
    }
}

struct Search {
    sample_rate: u32,
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

impl Search {
    fn can_improve(&self, position: f64, nominal: f64, selected_score: f64) -> bool {
        let displacement = (position - nominal) / self.range.max(1.0);
        1.0 - 0.002 * displacement * displacement > selected_score
    }

    fn score<S: GranularSource>(
        &self,
        lookup: &mut WindowedLookup<'_, S>,
        position: f64,
        nominal: f64,
    ) -> Result<Match, S::Error> {
        self.score_sampled(lookup, position, nominal, None)
    }

    fn score_sampled<S: GranularSource>(
        &self,
        lookup: &mut WindowedLookup<'_, S>,
        position: f64,
        nominal: f64,
        screen: Option<Screen>,
    ) -> Result<Match, S::Error> {
        let full_points = self.overlap.clamp(1, 64);
        let points = if screen.is_some() {
            full_points.min(16)
        } else {
            full_points
        };
        let sx = screen.map_or(self.previous_sum, |s| s.sum);
        let count = screen.map_or(self.count, |s| s.count);
        let mut sy = [0.0; 2];
        let mut xx = screen.map_or(self.previous_square, |s| s.square);
        let mut yy = 0.0;
        let mut xy = 0.0;
        for i in 0..points {
            let index = i * full_points / points;
            let at = index * self.overlap / full_points;
            if let Some(x) = self.previous[index] {
                let y = lookup.linear(position + at as f64 * self.step, self.lower, self.upper)?;
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
        let displacement = (position - nominal) / self.range.max(1.0);
        Ok(Match {
            position,
            score: correlation - 0.002 * displacement * displacement - 0.02 * energy,
            correlation,
            previous_mean: sx.map(|s| s / count),
            next_mean: sy.map(|s| s / count),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn scan<S: GranularSource>(
        &self,
        lookup: &mut WindowedLookup<'_, S>,
        lo: f64,
        hi: f64,
        step: f64,
        nominal: f64,
        mut selected: Match,
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<Match, S::Error> {
        let mut candidate = lo;
        let mut iter = 0_usize;
        while candidate <= hi {
            if iter > 0 && iter.is_multiple_of(64) && cancel() {
                return Err(Error::Cancelled.into());
            }
            if self.can_improve(candidate, nominal, selected.score) {
                let next = self.score(lookup, candidate, nominal)?;
                if next.score > selected.score {
                    selected = next;
                }
            }
            candidate += step;
            iter += 1;
        }
        Ok(selected)
    }

    #[allow(clippy::too_many_arguments)]
    fn screened<S: GranularSource>(
        &self,
        lookup: &mut WindowedLookup<'_, S>,
        lo: f64,
        hi: f64,
        nominal: f64,
        predicted: f64,
        mut selected: Match,
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<Match, S::Error> {
        let step = f64::from((self.sample_rate / 4_000).max(1));
        let full_points = self.overlap.clamp(1, 64);
        let points = full_points.min(16);
        let mut screen = Screen {
            sum: [0.0; 2],
            square: 0.0,
            count: 0.0,
        };
        for i in 0..points {
            if let Some(x) = self.previous[i * full_points / points] {
                screen.count += 1.0;
                for (c, value) in x.iter().enumerate() {
                    screen.sum[c] += value;
                    screen.square += value * value;
                }
            }
        }
        let mut peaks = [(0.0_f64, f64::NEG_INFINITY); 8];
        let mut position = lo;
        let mut iter = 0_usize;
        while position <= hi {
            if iter > 0 && iter.is_multiple_of(64) && cancel() {
                return Err(Error::Cancelled.into());
            }
            let score = self
                .score_sampled(lookup, position, nominal, Some(screen))?
                .score;
            let nearby = peaks
                .iter()
                .position(|p| p.1.is_finite() && (p.0 - position).abs() < step * 2.0);
            let index = nearby.unwrap_or_else(|| {
                let mut lowest = 0;
                for (candidate, peak) in peaks.iter().enumerate().skip(1) {
                    if peak.1.total_cmp(&peaks[lowest].1).is_lt() {
                        lowest = candidate;
                    }
                }
                lowest
            });
            if score > peaks[index].1 {
                peaks[index] = (position, score);
            }
            position += step;
            iter += 1;
        }
        let mut full = [selected; 10];
        let mut count = 0;
        for pos in std::iter::once(nominal)
            .chain(std::iter::once(predicted.clamp(lo, hi)))
            .chain(peaks.iter().filter(|p| p.1.is_finite()).map(|p| p.0))
        {
            let next = self.score(lookup, pos, nominal)?;
            full[count] = next;
            count += 1;
            if next.score > selected.score {
                selected = next;
            }
        }
        full[..count].sort_unstable_by(|a, b| b.score.total_cmp(&a.score));
        for seed in full.iter().take(count.min(3)) {
            selected = self.scan(
                lookup,
                (seed.position - step).max(lo),
                (seed.position + step).min(hi),
                0.5,
                nominal,
                selected,
                cancel,
            )?;
        }
        let best = selected.position;
        self.scan(
            lookup,
            (best - 0.5).max(lo),
            (best + 0.5).min(hi),
            0.125,
            nominal,
            selected,
            cancel,
        )
    }
}

/// Step one WSOLA grain or one short-splice chunk (`<= 512` frames).
/// Returns `Ok((new_ready_end, finished))`.
#[allow(clippy::too_many_arguments)]
pub(super) fn step_bed<S: GranularSource>(
    state: &mut BedState,
    sample_rate: u32,
    mode: Mode,
    pitch: f64,
    map: &TimeMap,
    bank: &Bandlimited,
    boundaries: &[Boundary],
    overlap: &mut GranularOverlap,
    history: &DrainedHistory,
    source: &mut S,
    scratch: &mut [[f64; 2]],
    cancel: &mut dyn FnMut() -> bool,
) -> Result<(u64, bool), S::Error> {
    match state {
        BedState::ShortSplice {
            span,
            age,
            native,
            release,
            last,
        } => {
            let duration = span.end - span.start;
            let chunk_end = (*age + 512).min(duration);
            let lower = span.source_start.floor() as i128;
            let upper = span.source_end.ceil() as i128 - 1;
            while *age < chunk_end {
                if (*age).is_multiple_of(256) && cancel() {
                    return Err(Error::Cancelled.into());
                }
                let sample = if *age < *native {
                    let s = bank.try_sample_segment(
                        span.source_start + *age as f64 * pitch,
                        lower,
                        upper,
                        |at| source.frame(at),
                    )?;
                    *last = s;
                    s
                } else if *age - *native < *release {
                    let x = (*age - *native + 1) as f64 / *release as f64;
                    last.map(|s| s * (1.0 - x * x * (3.0 - 2.0 * x)))
                } else {
                    [0.0; 2]
                };
                overlap
                    .add(i128::from(span.start + *age), sample, 1.0)
                    .map_err(S::Error::from)?;
                *age += 1;
            }
            let ready = span.start + *age;
            Ok((ready, *age >= duration))
        }
        BedState::Wsola {
            span,
            continuation,
            lower,
            upper,
            grain,
            hop,
            first_map,
            last_map,
            out,
            previous_position,
        } => {
            if *out >= i128::from(span.end) {
                return Ok((span.end, true));
            }
            let center = *out + *hop as i128;
            let fraction = ((center - i128::from(span.start)) as f64
                / (span.end - span.start) as f64)
                .clamp(0.0, 1.0);
            let mapped = map.position(center).map_err(S::Error::from)?;
            let mut nominal = mapped
                + (span.source_start - *first_map) * (1.0 - fraction)
                + (span.source_end - *last_map) * fraction
                - *hop as f64 * pitch;
            if let Some(c) = *continuation {
                nominal = c.source + (c.target - c.source) * fraction - *hop as f64 * pitch;
            }
            let mut step = pitch;
            if matches!(
                mode,
                Mode::Beats {
                    tail: Tail::Alternating,
                    ..
                }
            ) && let Some((position, direction)) = if let Some(c) = *continuation {
                let u = c.target.min(span.source_end) - 1.0;
                let width = u - span.source_start;
                if width >= 2.0 && center >= i128::from(c.native_end) {
                    let phase = ((center - i128::from(c.native_end) + 1) as f64 * pitch)
                        .rem_euclid(width * 2.0);
                    Some(if phase < width {
                        (u - phase, -1.0)
                    } else {
                        (span.source_start + phase - width, 1.0)
                    })
                } else {
                    None
                }
            } else {
                reflected_in_window(boundaries, *span, center, pitch)
            } {
                step = pitch * direction;
                nominal = position - *hop as f64 * step;
            }
            let mut forced = false;
            if continuation.is_none() && *out <= i128::from(span.start) {
                step = pitch;
                nominal = span.source_start + (*out - i128::from(span.start)) as f64 * pitch;
                forced = true;
            }
            if span.locked_end && *out >= i128::from(span.end) - *grain as i128 {
                step = pitch;
                nominal = continuation.map_or(span.source_end, |c| c.target)
                    - (i128::from(span.end) - *out) as f64 * pitch;
                forced = true;
            }
            let (minimum, maximum) = if step > 0.0 {
                (
                    span.source_start,
                    (span.source_end - *grain as f64 * pitch).max(span.source_start),
                )
            } else {
                (
                    (span.source_start + (*grain - 1) as f64 * pitch).min(span.source_end - 1.0),
                    span.source_end - 1.0,
                )
            };
            if !forced {
                nominal = nominal.clamp(minimum, maximum);
            }
            let range = (f64::from(sample_rate) * 0.040 * pitch)
                .min((span.source_end - span.source_start) * 0.25)
                .max(1.0);
            let points = (*hop).clamp(1, 64);
            let mut previous = [None; 64];
            let mut previous_sum = [0.0; 2];
            let mut previous_square = 0.0;
            let mut count = 0.0;
            for (i, value) in previous.iter_mut().enumerate().take(points) {
                let at = *out + (i * *hop / points) as i128;
                *value = if continuation.is_some() {
                    history.reference(overlap, at)
                } else {
                    overlap.preview(at)
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
                sample_rate,
                step,
                overlap: *hop,
                range,
                lower: *lower,
                upper: *upper,
                previous,
                previous_sum,
                previous_square,
                count,
            };
            let lo = (nominal - range).clamp(minimum, (maximum - 2.0 * range).max(minimum));
            let hi = (lo + 2.0 * range).min(maximum);
            // Preload the search window into caller scratch when it fits so scoring
            // stays memory-speed instead of repeating source cache lookups.
            let (cache_start, cache_len) = {
                let lo_end = lo + *hop as f64 * step;
                let hi_end = hi + *hop as f64 * step;
                let nom_end = nominal + *hop as f64 * step;
                let min_pos = lo.min(hi).min(nominal).min(lo_end).min(hi_end).min(nom_end);
                let max_pos = lo.max(hi).max(nominal).max(lo_end).max(hi_end).max(nom_end);
                let s = (min_pos.floor() as i128).clamp(*lower, (*upper).max(*lower));
                let e = ((max_pos.ceil() as i128) + 2).clamp(s, (*upper).max(*lower) + 1);
                let len = usize::try_from(e - s).unwrap_or(usize::MAX);
                if len > 0 && len <= scratch.len() {
                    source.window(s, &mut scratch[..len])?;
                    (s, len)
                } else {
                    (0, 0)
                }
            };
            let selected = {
                let mut lookup = WindowedLookup {
                    source,
                    cached_start: cache_start,
                    cached: &scratch[..cache_len],
                };
                let mut selected = search.score(&mut lookup, nominal, nominal)?;
                let reference = if continuation.is_some() {
                    history.reference(overlap, *out + *hop as i128 / 2)
                } else {
                    overlap.preview(*out + *hop as i128 / 2)
                };
                if !forced && reference.is_some() {
                    let predicted = previous_position
                        .filter(|(_, direction)| *direction == step)
                        .map_or(nominal, |(position, _)| position + *hop as f64 * step);
                    selected = search.screened(
                        &mut lookup,
                        lo,
                        hi,
                        nominal,
                        predicted,
                        selected,
                        cancel,
                    )?;
                    if selected.correlation < 0.5 {
                        let position = if pitch == 1.0 {
                            nominal.round().clamp(minimum, maximum)
                        } else {
                            nominal
                        };
                        selected = search.score(&mut lookup, position, nominal)?;
                    }
                }
                selected
            };
            *previous_position = Some((selected.position, step));
            // Preload synthesis source interval with filter halo into scratch if it fits.
            let (syn_start, syn_len) = {
                let p0 = selected.position;
                let p1 = selected.position + (*grain - 1) as f64 * step;
                let min_p = p0.min(p1).floor() as i128;
                let max_p = p0.max(p1).ceil() as i128;
                let halo = (crate::V2_MAX_TAPS / 2 + 2) as i128;
                let s = (min_p - halo).clamp(*lower, (*upper).max(*lower));
                let e = (max_p + halo + 1).clamp(s, (*upper).max(*lower) + 1);
                let len = usize::try_from(e - s).unwrap_or(usize::MAX);
                if len > 0 && len <= scratch.len() {
                    source.window(s, &mut scratch[..len])?;
                    (s, len)
                } else {
                    (0, 0)
                }
            };
            let mut lookup = WindowedLookup {
                source,
                cached_start: syn_start,
                cached: &scratch[..syn_len],
            };
            for i in 0..*grain {
                if i > 0 && i % 512 == 0 && cancel() {
                    return Err(Error::Cancelled.into());
                }
                let at = *out + i as i128;
                if at < i128::from(span.start) || at >= i128::from(span.end) {
                    continue;
                }
                let mut weight =
                    0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / *grain as f64).cos();
                if let Some(c) = *continuation {
                    let x = ((at - i128::from(span.start)) as f64 / c.fade.max(1) as f64)
                        .clamp(0.0, 1.0);
                    weight *= x * x * (3.0 - 2.0 * x);
                }
                let sample = bank.try_sample_segment(
                    selected.position + i as f64 * step,
                    *lower,
                    *upper,
                    |pos| lookup.frame(pos),
                )?;
                overlap
                    .add_correlated(
                        at,
                        sample,
                        weight,
                        selected.correlation,
                        selected.previous_mean,
                        selected.next_mean,
                    )
                    .map_err(S::Error::from)?;
            }
            *out += *hop as i128;
            let finished = *out >= i128::from(span.end);
            let ready = if finished {
                span.end
            } else {
                ((*out).max(i128::from(span.start)) as u64).min(span.end)
            };
            Ok((ready, finished))
        }
    }
}
