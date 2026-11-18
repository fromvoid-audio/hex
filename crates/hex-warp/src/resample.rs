//! Windowed-sinc reconstruction with rate-dependent anti-alias cutoff.
//!
//! **v1** ([`Resampler::V1`]): 64 Blackman-windowed taps at every rate,
//! 256 nearest phase rows, cutoff `1/rate`. Frozen bit-for-bit.
//!
//! **v2** ([`Resampler::V2`]): Kaiser window (90 dB design), length
//! `128 * max(rate, 1)` taps capped at [`V2_MAX_TAPS`], so the transition
//! keeps its relative width when decimating; 256 + 1 phase rows linearly
//! interpolated by the exact Q32 fraction. Above unity rate the stopband edge
//! sits at the output Nyquist (`cutoff = 1/rate - transition/2`); at or below
//! unity it reconstructs at full band like v1, so integer positions remain
//! exact. Beyond the tap cap (rate > 4) the transition widens proportionally.
use super::*;
const TAPS: usize = 64;
const PHASES: usize = 256;
const V2_BASE_TAPS: usize = 128;
/// Largest v2 filter length; reached at rate 4.
pub const V2_MAX_TAPS: usize = 512;
const V2_STOPBAND_DB: f64 = 90.0;
const BANK_BYTES_V1: usize = TAPS * PHASES * 8;
const BANK_BYTES_V2: usize = V2_MAX_TAPS * (PHASES + 1) * 8;

/// Versioned reconstruction law. Each version's output is fixed once shipped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resampler {
    V1,
    V2,
}

/// Prepared unwarped, non-looping pitch reconstruction for bounded worker
/// windows. Positions are relative to the complete source, not each window.
/// Source PCM must already be at the project rate. This owns a filter bank and
/// is intended for control/worker use, never construction in an audio callback.
pub struct PitchWindow {
    source_frames: u64,
    pitch_q32: i128,
    bank: Bandlimited,
}

/// Bounded, non-looping Re-Pitch reconstruction at complete-map coordinates.
/// The caller owns source identity and supplies the requested source halo.
/// Construction and rendering are control/worker operations, not callback work.
pub struct RepitchWindow {
    source_frames: u64,
    map: TimeMap,
    bank: Bandlimited,
}

impl RepitchWindow {
    /// Prepare the same versioned filter and segment rates as resident Re-Pitch.
    pub fn new(
        source_frames: u64,
        map: TimeMap,
        resampler: Resampler,
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<Self, Error> {
        if source_frames == 0
            || source_frames > (1_u64 << 36)
            || map.segments.capacity() > MAX_MAP_SEGMENTS
        {
            return Err(Error::Invalid("invalid repitch source extent"));
        }
        map.validate_stretch(Mode::RePitch, 0)?;
        let mut work = Work {
            cancel,
            used: 0,
            limit: MAX_WORK_UNITS,
            scratch: 0,
            base_scratch: 0,
            resampler,
        };
        work.check(0)?;
        let bank = Bandlimited::new(map.rate_at(0), &mut work)?;
        Ok(Self {
            source_frames,
            map,
            bank,
        })
    }

    /// Maximum filter payload and actual retained backing capacities for a worker reservation.
    pub const fn filter_reservation_bytes(resampler: Resampler) -> usize {
        bank_bytes(resampler)
    }
    pub fn filter_backing_bytes(&self) -> Option<usize> {
        self.bank
            .coefficients
            .capacity()
            .checked_mul(std::mem::size_of::<f64>())
    }
    pub fn map_backing_bytes(&self) -> Option<usize> {
        self.map
            .segments
            .capacity()
            .checked_mul(std::mem::size_of::<MapSegment>())
    }

    /// Return a conservative halo for at most 4096 output frames. Positions
    /// outside the complete source are zero, just as in resident rendering.
    pub fn required_source_range(
        &self,
        output_start: u64,
        count: usize,
    ) -> Result<std::ops::Range<u64>, Error> {
        if count == 0 || count > 4096 {
            return Err(Error::Invalid("invalid repitch window length"));
        }
        let output_extent = self
            .map
            .segments
            .last()
            .ok_or(Error::Invalid("empty audio map"))?
            .output_end;
        let end = output_start
            .checked_add(count as u64)
            .filter(|end| *end <= output_extent)
            .ok_or(Error::Invalid("repitch window exceeds output extent"))?;
        let first = self
            .map
            .position_q32(i128::from(output_start))?
            .div_euclid(Q32);
        let last = self.map.position_q32(i128::from(end - 1))?.div_euclid(Q32);
        let half = match self.bank.resampler {
            Resampler::V1 => TAPS / 2,
            Resampler::V2 => V2_MAX_TAPS / 2,
        } as i128;
        let limit = i128::from(self.source_frames);
        Ok((first - (half - 1)).clamp(0, limit) as u64..(last + half + 1).clamp(0, limit) as u64)
    }

    /// Render an exact output window from its requested halo. Arbitrary window
    /// order is supported; cancellation may leave output partially written.
    pub fn render(
        &mut self,
        output_start: u64,
        source_start: u64,
        source: &[Frame],
        output: &mut [Frame],
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let range = self.required_source_range(output_start, output.len())?;
        if source_start != range.start || source.len() as u64 != range.end - range.start {
            return Err(Error::Invalid("incomplete repitch source window"));
        }
        let mut work = Work {
            cancel,
            used: 0,
            limit: MAX_WORK_UNITS,
            scratch: 0,
            base_scratch: 0,
            resampler: self.bank.resampler,
        };
        for (index, target) in output.iter_mut().enumerate() {
            if index % 1024 == 0 {
                work.check(1024 * V2_MAX_TAPS as u64)?;
            }
            let frame = output_start + index as u64;
            let position = self.map.position_q32(i128::from(frame))?;
            self.bank.set_rate(self.map.rate_at(frame), &mut work)?;
            *target = finite(self.bank.sample_with(position, |at| {
                if at < i128::from(range.start) || at >= i128::from(range.end) {
                    return [0.0; 2];
                }
                let sample = source[(at - i128::from(range.start)) as usize];
                [f64::from(sample[0]), f64::from(sample[1])]
            }))?;
        }
        Ok(())
    }
}

impl PitchWindow {
    /// Maximum coefficient payload requested during construction for this law.
    /// The caller must compare actual retained capacity against its reservation.
    pub const fn filter_reservation_bytes(resampler: Resampler) -> usize {
        bank_bytes(resampler)
    }

    /// Actual retained coefficient capacity, excluding this inline value.
    pub fn filter_backing_bytes(&self) -> Option<usize> {
        self.bank
            .coefficients
            .capacity()
            .checked_mul(std::mem::size_of::<f64>())
    }

    /// Prepare the same filter and fixed-point pitch used by resident rendering.
    pub fn new(
        source_frames: u64,
        pitch_cents: i16,
        resampler: Resampler,
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<Self, Error> {
        if source_frames == 0 || source_frames > (1_u64 << 36) {
            return Err(Error::Invalid("invalid pitch window extent"));
        }
        validate_stretch_ratio(1.0, Mode::Resample, pitch_cents)?;
        let pitch = 2.0_f64.powf(f64::from(pitch_cents) / 1_200.0);
        let mut work = Work {
            cancel,
            used: 0,
            limit: MAX_WORK_UNITS,
            scratch: 0,
            base_scratch: 0,
            resampler,
        };
        work.check(0)?;
        let bank = Bandlimited::new(pitch, &mut work)?;
        Ok(Self {
            source_frames,
            pitch_q32: (pitch * Q32 as f64).round() as i128,
            bank,
        })
    }

    /// Return the complete, clipped source halo needed for an output window.
    /// Output windows are limited to 4096 frames and must lie in the clip.
    pub fn required_source_range(
        &self,
        output_start: u64,
        count: usize,
    ) -> Result<std::ops::Range<u64>, Error> {
        if count == 0 || count > 4096 {
            return Err(Error::Invalid("invalid pitch window length"));
        }
        let end = output_start
            .checked_add(count as u64)
            .filter(|end| *end <= self.source_frames)
            .ok_or(Error::Invalid("pitch window exceeds output extent"))?;
        let first = i128::from(output_start) * self.pitch_q32 / Q32;
        let last = i128::from(end - 1) * self.pitch_q32 / Q32;
        let half = (self.bank.taps / 2) as i128;
        let limit = i128::from(self.source_frames);
        let start = (first - (half - 1)).clamp(0, limit) as u64;
        let end = (last + half + 1).clamp(0, limit) as u64;
        Ok(start..end)
    }

    /// Render from exactly the requested halo. Samples outside the full source
    /// are zero, as in the resident renderer; missing in-range PCM is an error.
    /// An error or cancellation can leave the output partially written.
    pub fn render(
        &self,
        output_start: u64,
        source_start: u64,
        source: &[Frame],
        output: &mut [Frame],
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let range = self.required_source_range(output_start, output.len())?;
        if source_start != range.start || source.len() as u64 != range.end - range.start {
            return Err(Error::Invalid("incomplete pitch source window"));
        }
        for (index, target) in output.iter_mut().enumerate() {
            if index % 1024 == 0 && cancel() {
                return Err(Error::Cancelled);
            }
            let position = i128::from(output_start + index as u64) * self.pitch_q32;
            *target = finite(self.bank.sample_with(position, |at| {
                if at < i128::from(range.start) || at >= i128::from(range.end) {
                    return [0.0; 2];
                }
                let frame = source[(at - i128::from(range.start)) as usize];
                [f64::from(frame[0]), f64::from(frame[1])]
            }))?;
        }
        Ok(())
    }
}

/// Coefficient bank size a render with `resampler` accounts as scratch.
pub(super) const fn bank_bytes(resampler: Resampler) -> usize {
    match resampler {
        Resampler::V1 => BANK_BYTES_V1,
        Resampler::V2 => BANK_BYTES_V2,
    }
}

pub(super) struct Bandlimited {
    coefficients: Vec<f64>,
    rate: f64,
    resampler: Resampler,
    taps: usize,
}

/// Render-local direct-mapped memoization of already evaluated input frames.
/// The key is the full unwrapped source position; looping and gain still use
/// `Input::frame` exactly once per miss. It never outlives this render.
pub(super) struct FrameCache<'a> {
    input: Input<'a>,
    slots: Vec<Option<(i128, [f64; 2])>>,
}

impl<'a> FrameCache<'a> {
    const SLOTS: usize = 128;

    pub(super) fn minimum_bytes() -> usize {
        std::mem::size_of::<Self>() + Self::SLOTS * std::mem::size_of::<Option<(i128, [f64; 2])>>()
    }

    pub(super) fn try_new(input: Input<'a>) -> Option<Self> {
        let mut slots = Vec::new();
        slots.try_reserve_exact(Self::SLOTS).ok()?;
        slots.resize(Self::SLOTS, None);
        Some(Self { input, slots })
    }

    pub(super) fn allocated_bytes(&self) -> Option<usize> {
        self.slots
            .capacity()
            .checked_mul(std::mem::size_of::<Option<(i128, [f64; 2])>>())?
            .checked_add(std::mem::size_of::<Self>())
    }

    fn frame(&mut self, position: i128) -> [f64; 2] {
        let index = (position as u128 & (Self::SLOTS as u128 - 1)) as usize;
        if let Some((key, value)) = self.slots[index]
            && key == position
        {
            return value;
        }
        let value = self.input.frame(position);
        self.slots[index] = Some((position, value));
        value
    }
}
impl Bandlimited {
    pub(super) fn backing_bytes(&self) -> Option<usize> {
        self.coefficients
            .capacity()
            .checked_mul(std::mem::size_of::<f64>())
    }
    pub(super) fn new(rate: f64, work: &mut Work<'_>) -> Result<Self, Error> {
        let resampler = work.resampler;
        work.scratch(bank_bytes(resampler))?;
        let capacity = match resampler {
            Resampler::V1 => TAPS * PHASES,
            Resampler::V2 => V2_MAX_TAPS * (PHASES + 1),
        };
        let mut result = Self {
            coefficients: buffer(capacity, 0.0)?,
            rate: 0.0,
            resampler,
            taps: TAPS,
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
        // Both reconstruction laws use the same full-band coefficients for
        // every positive rate at or below unity. Keep the cancellation check
        // at a changed segment, but do not charge work for a rebuild we skip.
        if self.rate > 0.0 && self.rate <= 1.0 && rate <= 1.0 {
            work.check(0)?;
            self.rate = rate;
            return Ok(());
        }
        match self.resampler {
            Resampler::V1 => self.set_rate_v1(rate, work),
            Resampler::V2 => self.set_rate_v2(rate, work),
        }
    }
    fn set_rate_v1(&mut self, rate: f64, work: &mut Work<'_>) -> Result<(), Error> {
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
    fn set_rate_v2(&mut self, rate: f64, work: &mut Work<'_>) -> Result<(), Error> {
        // Even length keeps the (taps/2 - 1) center convention of v1.
        let scaled = (V2_BASE_TAPS as f64 * rate.max(1.0)).ceil() as usize;
        let taps = (scaled.div_ceil(2) * 2).min(V2_MAX_TAPS);
        work.check((taps * (PHASES + 1)) as u64)?;
        let beta = 0.1102 * (V2_STOPBAND_DB - 8.7);
        let transition = (V2_STOPBAND_DB - 7.95) / (2.285 * taps as f64 * std::f64::consts::PI);
        let cutoff = if rate <= 1.0 {
            1.0
        } else {
            rate.recip() - transition / 2.0
        };
        if cutoff.is_nan() || cutoff <= 0.0 {
            return Err(Error::Invalid("invalid resampling filter cutoff"));
        }
        let half = taps as f64 * 0.5;
        let norm = bessel_i0(beta);
        for phase in 0..=PHASES {
            let fraction = phase as f64 / PHASES as f64;
            let row = &mut self.coefficients[phase * taps..(phase + 1) * taps];
            let mut sum = 0.0;
            for (tap, value) in row.iter_mut().enumerate() {
                let x = tap as f64 - (taps / 2 - 1) as f64 - fraction;
                let sinc = if x.abs() < 1e-12 {
                    cutoff
                } else {
                    (std::f64::consts::PI * x * cutoff).sin() / (std::f64::consts::PI * x)
                };
                let r = (x / half).clamp(-1.0, 1.0);
                let window = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / norm;
                *value = sinc * window;
                sum += *value;
            }
            if !sum.is_finite() || sum.abs() < 1e-12 {
                return Err(Error::Invalid("invalid resampling filter normalization"));
            }
            for value in row {
                *value /= sum;
            }
        }
        self.rate = rate;
        self.taps = taps;
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
        self.sample_with(position_q32, |at| {
            input.frame(interval.map_or(at, |(start, end)| at.clamp(start, end)))
        })
    }
    pub(super) fn sample_with(
        &self,
        position_q32: i128,
        mut frame: impl FnMut(i128) -> [f64; 2],
    ) -> [f64; 2] {
        let integer = position_q32.div_euclid(Q32);
        let fraction = position_q32.rem_euclid(Q32);
        if fraction == 0 && self.rate <= 1.0 {
            return frame(integer);
        }
        let scaled = fraction * PHASES as i128;
        let phase = (scaled / Q32) as usize;
        let mut result = [0.0; 2];
        match self.resampler {
            Resampler::V1 => {
                for tap in 0..TAPS {
                    let sample = frame(integer + tap as i128 - (TAPS / 2 - 1) as i128);
                    let coefficient = self.coefficients[phase * TAPS + tap];
                    result[0] += sample[0] * coefficient;
                    result[1] += sample[1] * coefficient;
                }
            }
            Resampler::V2 => {
                let taps = self.taps;
                let blend = (scaled % Q32) as f64 / Q32 as f64;
                let low = &self.coefficients[phase * taps..(phase + 1) * taps];
                let high = &self.coefficients[(phase + 1) * taps..(phase + 2) * taps];
                let first = integer - (taps / 2 - 1) as i128;
                for (tap, (&a, &b)) in low.iter().zip(high).enumerate() {
                    let sample = frame(first + tap as i128);
                    let coefficient = a + (b - a) * blend;
                    result[0] += sample[0] * coefficient;
                    result[1] += sample[1] * coefficient;
                }
            }
        }
        result
    }
    pub(super) fn sample_float(&self, input: Input<'_>, position: f64) -> [f64; 2] {
        self.sample(input, (position * Q32 as f64).round() as i128)
    }
    pub(super) fn sample_float_cached(
        &self,
        position: f64,
        cache: &mut FrameCache<'_>,
    ) -> [f64; 2] {
        self.sample_with((position * Q32 as f64).round() as i128, |at| {
            cache.frame(at)
        })
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
            work.check(1_024 * bank.taps as u64)?;
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

/// Modified Bessel function of the first kind, order zero (power series).
fn bessel_i0(x: f64) -> f64 {
    let quarter = x * x / 4.0;
    let mut term = 1.0;
    let mut sum = 1.0;
    for k in 1..64 {
        term *= quarter / (k * k) as f64;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use crate::GainPoint;

    fn segmented_map(rates: &[(u64, u64)], frames_per_segment: u64) -> TimeMap {
        let mut source = 0_i128;
        let segments = rates
            .iter()
            .enumerate()
            .map(|(i, &(numerator, denominator))| {
                let start = source;
                source += i128::from(frames_per_segment) * i128::from(numerator) * Q32
                    / i128::from(denominator);
                MapSegment {
                    output_start: i as u64 * frames_per_segment,
                    output_end: (i as u64 + 1) * frames_per_segment,
                    source_start_q32: start,
                    source_end_q32: source,
                }
            })
            .collect();
        TimeMap::new_streamed(segments, rates.len() as u64 * frames_per_segment).unwrap()
    }

    #[test]
    fn repitch_rate_changes_match_independently_rebuilt_filters() {
        let map = segmented_map(&[(1, 2), (3, 4), (1, 1), (3, 2), (1, 2), (3, 4)], 32);
        let source: Vec<_> = (0..512)
            .map(|i| [(i as f32 * 0.031).sin(), (i as f32 * 0.047).cos()])
            .collect();
        for law in [Resampler::V1, Resampler::V2] {
            let mut window =
                RepitchWindow::new(source.len() as u64, map.clone(), law, &mut || false).unwrap();
            let range = window.required_source_range(0, 192).unwrap();
            let mut actual = [[0.0; 2]; 192];
            window
                .render(
                    0,
                    range.start,
                    &source[range.start as usize..range.end as usize],
                    &mut actual,
                    &mut || false,
                )
                .unwrap();
            let mut bank = None;
            for (i, actual) in actual.iter().enumerate() {
                if i % 32 == 0 {
                    let mut cancel = || false;
                    let mut work = Work {
                        cancel: &mut cancel,
                        used: 0,
                        limit: MAX_WORK_UNITS,
                        scratch: 0,
                        base_scratch: 0,
                        resampler: law,
                    };
                    bank = Some(Bandlimited::new(map.rate_at(i as u64), &mut work).unwrap());
                }
                let bank = bank.as_ref().unwrap();
                let expected = finite(bank.sample_with(
                    map.position_q32(i as i128).unwrap(),
                    |at| {
                        usize::try_from(at)
                            .ok()
                            .and_then(|at| source.get(at))
                            .map_or([0.0; 2], |frame| [f64::from(frame[0]), f64::from(frame[1])])
                    },
                ))
                .unwrap();
                assert_eq!(*actual, expected, "law {law:?}, frame {i}");
            }
            // Cancellation on a changed segment must leave that segment
            // untouched, and the same prepared window must remain reusable.
            let mut interrupted =
                RepitchWindow::new(source.len() as u64, map.clone(), law, &mut || false).unwrap();
            let mut partial = [[-9.0; 2]; 192];
            let mut checks = 0;
            let error = interrupted
                .render(
                    0,
                    range.start,
                    &source[range.start as usize..range.end as usize],
                    &mut partial,
                    &mut || {
                        checks += 1;
                        checks == 2
                    },
                )
                .unwrap_err();
            assert_eq!(error, Error::Cancelled);
            assert!(partial[32..].iter().all(|frame| *frame == [-9.0; 2]));
            interrupted
                .render(
                    0,
                    range.start,
                    &source[range.start as usize..range.end as usize],
                    &mut partial,
                    &mut || false,
                )
                .unwrap();
            assert_eq!(partial, actual);
        }
    }

    #[test]
    #[ignore = "explicit optimized window timing profile"]
    fn segmented_repitch_window_profile() {
        use std::time::Instant;
        let rates: Vec<_> = (0..32)
            .map(|i| if i % 2 == 0 { (1, 2) } else { (3, 4) })
            .collect();
        let map = segmented_map(&rates, 128);
        let source: Vec<_> = (0..4096)
            .map(|i| [(i as f32 * 0.013).sin(), (i as f32 * 0.017).cos()])
            .collect();
        let mut window =
            RepitchWindow::new(source.len() as u64, map, Resampler::V2, &mut || false).unwrap();
        let range = window.required_source_range(0, 4096).unwrap();
        let source_window = &source[range.start as usize..range.end as usize];
        let mut output = [[0.0; 2]; 4096];
        let mut samples = Vec::new();
        for iteration in 0..8 {
            let start = Instant::now();
            window
                .render(0, range.start, source_window, &mut output, &mut || false)
                .unwrap();
            if iteration >= 3 {
                samples.push(start.elapsed().as_nanos());
            }
        }
        samples.sort_unstable();
        let checksum = output
            .iter()
            .flatten()
            .fold(0u64, |n, value| n.wrapping_add(u64::from(value.to_bits())));
        eprintln!(
            "repitch_window rate=48000 output=4096 segments=32 slopes=0.5,0.75 law=v2 warmup=3 samples=5 p50_ns={} checksum={checksum}",
            samples[2]
        );
    }

    #[test]
    fn cached_samples_match_uncached_for_loops_edges_collisions_and_envelopes() {
        let frames: Vec<_> = (0..300)
            .map(|i| [i as f32 / 300.0, -(i as f32) / 500.0])
            .collect();
        let envelope = [
            GainPoint {
                source_frame: 0,
                gain_q16: 12000,
            },
            GainPoint {
                source_frame: 200,
                gain_q16: 90000,
            },
        ];
        for resampler in [Resampler::V1, Resampler::V2] {
            let mut cancel = || false;
            let mut work = Work {
                cancel: &mut cancel,
                used: 0,
                limit: MAX_WORK_UNITS,
                scratch: 0,
                base_scratch: 0,
                resampler,
            };
            let bank = Bandlimited::new(1.3, &mut work).unwrap();
            for looping in [false, true] {
                let input = Input {
                    frames: &frames,
                    sample_rate: 16000,
                    region_start: 11,
                    region_length: 270,
                    phase: if looping { 17 } else { 0 },
                    looping,
                    envelope: &envelope,
                };
                let mut cache = FrameCache::try_new(input).unwrap();
                for position in [-129.5, -1.25, 0.0, 1.75, 127.5, 255.5, 127.5, 269.75, 300.0] {
                    let expected = bank.sample_float(input, position).map(f64::to_bits);
                    assert_eq!(
                        bank.sample_float_cached(position, &mut cache)
                            .map(f64::to_bits),
                        expected
                    );
                }
            }
        }
    }
}
