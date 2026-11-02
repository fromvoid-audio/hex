//! Band-limited source reconstruction for pitched-up sampler voices.
//!
//! Law (sampler interpolation v2, #4089): a voice whose source step `r`
//! (source frames per output frame) is at most 1 keeps the original two-point
//! linear interpolation exactly. Above unity, each output frame is a normalized
//! Kaiser-windowed sinc sum over the source frames around the exact Q32 source
//! position:
//!
//! `y = Σ x[i+k] h(k - f) / Σ h(k - f)`, `h(t) = sinc(c t) · w(|t| / H)`
//!
//! with full-span cutoff scale `c = 1 / (r (1 + TRANSITION))`, so the window's
//! stopband begins at the output Nyquist, and half-span `H = ZERO_CROSSINGS / c`
//! source frames. Up to rate 4.44 (about +26 semitones at equal source/output
//! rates) the kernel spans all `ZERO_CROSSINGS` sinc lobes. Beyond it the span
//! is capped at `MAX_HALF_TAPS`, so cost per output frame stays bounded at
//! `2 * MAX_HALF_TAPS` taps; the wider transition then lowers the cutoff to
//! `1/r - TRANSITION * ZERO_CROSSINGS / MAX_HALF_TAPS` (floored at `0.25 / r`)
//! to keep the stopband at the output Nyquist, which narrows the passband at
//! extreme pitches. Normalizing by the weight sum keeps DC gain exactly one at
//! every fractional phase.
//!
//! Taps follow playback history: inside a Classic loop, taps past the loop end
//! continue at the loop start, and once playback has wrapped, taps before the
//! loop start come from the loop end. Other frames outside the voice region
//! read as silence. The kernel is symmetric and reads source frames by random
//! access, so it adds no output latency or per-voice state.
//!
//! The sinc and window are read from fixed static tables with linear
//! interpolation. [`warm`] builds them on control during plan preparation;
//! render-time access is one acquire load, with no allocation, lock or wait.
//!
//! Saved projects: the authored sampler state and its schema are unchanged.
//! Pitched-up voices in existing projects play with less aliasing; unity and
//! slower voices are bit-identical. No rendered sampler output is persisted,
//! so no versioned interpolation law is retained (decision recorded on #4089).
use std::sync::OnceLock;

/// Sinc lobes per side at full span.
pub(super) const ZERO_CROSSINGS: usize = 12;
/// Kaiser transition width relative to the cutoff (stopband at output Nyquist).
const TRANSITION: f64 = 0.2;
/// Kaiser shape for roughly 65 dB stopband attenuation.
const KAISER_BETA: f64 = 6.2;
/// Largest half-span in source frames; bounds per-frame work.
pub(super) const MAX_HALF_TAPS: usize = 64;
/// Table resolution per unit argument.
const RESOLUTION: usize = 512;
const SINC_LEN: usize = ZERO_CROSSINGS * RESOLUTION + 2;
const WINDOW_LEN: usize = RESOLUTION + 2;

struct Tables {
    /// Full-span prototype `sinc(u) * w(u / ZERO_CROSSINGS)`: one lookup.
    windowed: [f32; SINC_LEN],
    sinc: [f32; SINC_LEN],
    window: [f32; WINDOW_LEN],
}

static TABLES: OnceLock<Tables> = OnceLock::new();

/// Build the static tables. Call on control before any voice renders.
pub(super) fn warm() {
    tables();
}

fn tables() -> &'static Tables {
    TABLES.get_or_init(|| {
        let mut sinc = [0.0_f32; SINC_LEN];
        for (index, value) in sinc.iter_mut().enumerate() {
            let x = index as f64 / RESOLUTION as f64;
            *value = if index == 0 {
                1.0
            } else {
                ((std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)) as f32
            };
        }
        let mut window = [0.0_f32; WINDOW_LEN];
        let norm = bessel_i0(KAISER_BETA);
        for (index, value) in window.iter_mut().enumerate() {
            let v = (index as f64 / RESOLUTION as f64).min(1.0);
            *value = (bessel_i0(KAISER_BETA * (1.0 - v * v).sqrt()) / norm) as f32;
        }
        let mut windowed = [0.0_f32; SINC_LEN];
        for (index, value) in windowed.iter_mut().enumerate() {
            let u = index as f64 / RESOLUTION as f64;
            let v = (u / ZERO_CROSSINGS as f64).min(1.0);
            let window = bessel_i0(KAISER_BETA * (1.0 - v * v).sqrt()) / norm;
            *value = (f64::from(sinc[index]) * window) as f32;
        }
        Tables {
            windowed,
            sinc,
            window,
        }
    })
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for k in 1..64 {
        term *= (half / k as f64) * (half / k as f64);
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

#[inline]
fn lookup(table: &[f32], argument: f64) -> f64 {
    let scaled = argument * RESOLUTION as f64;
    let index = scaled as usize;
    let (Some(a), Some(b)) = (table.get(index), table.get(index + 1)) else {
        return 0.0;
    };
    let fraction = scaled - index as f64;
    f64::from(*a) + (f64::from(*b) - f64::from(*a)) * fraction
}

/// Per-voice kernel shape derived from the fixed-point step on control.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Kernel {
    /// Sinc argument scale per source frame.
    cutoff: f64,
    /// Reciprocal half-span, mapping |t| to the window argument.
    inverse_half_span: f64,
    /// Taps on each side of the source position.
    half_taps: usize,
    /// Full-span kernels read the combined prototype table once per tap.
    full_span: bool,
}

impl Kernel {
    /// `None` for unity or slower playback, which keeps linear interpolation.
    pub(super) fn for_rate(rate: f64) -> Option<Self> {
        if !rate.is_finite() || rate <= 1.0 {
            return None;
        }
        let full = 1.0 / (rate * (1.0 + TRANSITION));
        let full_span = ZERO_CROSSINGS as f64 / full <= MAX_HALF_TAPS as f64;
        let (cutoff, half_span) = if full_span {
            (full, ZERO_CROSSINGS as f64 / full)
        } else {
            // The capped window's transition is TRANSITION * ZERO_CROSSINGS /
            // MAX_HALF_TAPS cycles wide; lower the cutoff so its stopband
            // still starts at the output Nyquist, with a floor for extremes.
            let transition = TRANSITION * ZERO_CROSSINGS as f64 / MAX_HALF_TAPS as f64;
            (
                (rate.recip() - transition).max(0.25 / rate),
                MAX_HALF_TAPS as f64,
            )
        };
        Some(Self {
            cutoff,
            inverse_half_span: half_span.recip(),
            half_taps: (half_span.ceil() as usize).clamp(1, MAX_HALF_TAPS),
            full_span,
        })
    }

    /// Weighted sum around integer source frame `index` plus `fraction`.
    /// `frame(offset)` returns the source frame at a signed playback offset,
    /// or `None` for silence. Returns `None` if a read frame is not finite and
    /// `CHECK_FINITE` is set.
    #[inline]
    pub(super) fn reconstruct<const CHECK_FINITE: bool>(
        &self,
        index: i64,
        fraction: f64,
        mut frame: impl FnMut(i64) -> Option<[f32; 2]>,
    ) -> Option<[f64; 2]> {
        let tables = tables();
        let half = self.half_taps as i64;
        let mut sum = [0.0_f64; 2];
        let mut weights = 0.0_f64;
        for tap in (1 - half)..=half {
            let distance = (tap as f64 - fraction).abs();
            let window_argument = distance * self.inverse_half_span;
            if window_argument >= 1.0 {
                continue;
            }
            let weight = if self.full_span {
                // Same law: here the window argument equals u / ZERO_CROSSINGS.
                lookup(&tables.windowed, distance * self.cutoff)
            } else {
                lookup(&tables.sinc, distance * self.cutoff)
                    * lookup(&tables.window, window_argument)
            };
            weights += weight;
            if let Some(value) = frame(index + tap) {
                if CHECK_FINITE && value.iter().any(|sample| !sample.is_finite()) {
                    return None;
                }
                sum[0] += f64::from(value[0]) * weight;
                sum[1] += f64::from(value[1]) * weight;
            }
        }
        if weights.abs() < f64::EPSILON {
            return Some([0.0; 2]);
        }
        Some([sum[0] / weights, sum[1] / weights])
    }
}
