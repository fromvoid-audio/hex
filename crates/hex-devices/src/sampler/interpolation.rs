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
//! The kernel belongs to the voice's rate class: the next semitone step
//! `r_j = 2^(j/12)` at or above `r`, up to `r_j = 32`. Rounding up only lowers
//! the cutoff, by at most a semitone. Per class, the full-span cutoff scale is
//! `c = 1 / (r_j (1 + TRANSITION))`, so the Kaiser stopband begins at the output
//! Nyquist, with half-span `H = ZERO_CROSSINGS / c` source frames. Above rate
//! 3.08 (about +19.5 semitones at equal source/output rates) the span is
//! capped at `MAX_HALF_TAPS` per side. The wider transition then lowers the
//! cutoff to `1/r_j - TRANSITION * ZERO_CROSSINGS / MAX_HALF_TAPS` (floored at
//! `0.25 / r_j`), keeping the stopband at the output Nyquist while narrowing
//! the passband at high pitches. Above rate 32, alias suppression degrades.
//!
//! Weights come from a static polyphase bank: `PHASES + 1` rows per class at
//! fractions `p / PHASES`, linearly interpolated between the two rows around
//! the exact fraction. The output is computed as two dot products, one per
//! row, in fixed-width chunks the compiler vectorizes. Normalizing by the
//! interpolated row sum keeps DC gain exactly one at every phase.
//!
//! Taps follow playback history. Inside a Classic loop, taps past the loop
//! end continue at the loop start, and once playback has wrapped, taps before
//! the loop start come from the loop end (or, with a loop seam crossfade, from
//! the pre-roll the seam fades into, #4088). Other frames outside the voice region
//! read as silence. The kernel is symmetric and reads source frames by random
//! access, so it adds no output latency or per-voice state.
//!
//! [`warm`] builds the bank (about 0.5 MB, once per process) on control during
//! plan preparation. Render-time access is one acquire load and bounds-checked
//! slices, with no allocation, lock or wait.
//!
//! Saved projects: the authored sampler state and its schema are unchanged.
//! Pitched-up voices in existing projects play with less aliasing; unity and
//! slower voices are bit-identical. No rendered sampler output is persisted,
//! so no versioned interpolation law is retained (decision recorded on #4089).
use std::sync::OnceLock;

/// Sinc lobes per side at full span.
pub(super) const ZERO_CROSSINGS: usize = 8;
/// Kaiser transition width relative to the cutoff (stopband at output Nyquist).
const TRANSITION: f64 = 0.3;
/// Kaiser shape for roughly 65 dB stopband attenuation.
const KAISER_BETA: f64 = 6.2;
/// Largest half-span in source frames; bounds per-frame work.
pub(super) const MAX_HALF_TAPS: usize = 32;
/// Rate classes per octave; a voice uses the next class at or above its rate.
const CLASSES_PER_OCTAVE: f64 = 12.0;
/// Highest class: rate 2^(CLASSES / CLASSES_PER_OCTAVE) = 32 (+60 semitones).
const CLASSES: usize = 60;
/// Phase rows per class (plus one), linearly interpolated.
const PHASES: usize = 32;

/// Precomputed polyphase rows for every rate class.
struct Bank {
    /// Rows of `taps` weights, `PHASES + 1` rows per class, classes in order.
    weights: Box<[f32]>,
    /// Sum of each row, for normalization.
    sums: Box<[f32]>,
    /// Per class: offset of its first row in `weights` and its tap count.
    classes: [(usize, usize); CLASSES + 1],
}

static BANK: OnceLock<Bank> = OnceLock::new();

/// Build the static bank. Call on control before any voice renders; the
/// bank is built once per process and never freed.
pub(super) fn warm() {
    bank();
}

fn class_rate(class: usize) -> f64 {
    2_f64.powf(class as f64 / CLASSES_PER_OCTAVE)
}

/// Cutoff (sinc argument scale) and half-span for a playback rate above one.
fn shape(rate: f64) -> (f64, f64) {
    let full = 1.0 / (rate * (1.0 + TRANSITION));
    if ZERO_CROSSINGS as f64 / full <= MAX_HALF_TAPS as f64 {
        (full, ZERO_CROSSINGS as f64 / full)
    } else {
        // The capped window's transition is TRANSITION * ZERO_CROSSINGS /
        // MAX_HALF_TAPS cycles wide; lower the cutoff so its stopband still
        // starts at the output Nyquist, with a floor for extremes.
        let transition = TRANSITION * ZERO_CROSSINGS as f64 / MAX_HALF_TAPS as f64;
        (
            (rate.recip() - transition).max(0.25 / rate),
            MAX_HALF_TAPS as f64,
        )
    }
}

fn half_taps(half_span: f64) -> usize {
    (half_span.ceil() as usize).clamp(1, MAX_HALF_TAPS)
}

fn bank() -> &'static Bank {
    BANK.get_or_init(|| {
        let norm = bessel_i0(KAISER_BETA);
        let mut classes = [(0, 0); CLASSES + 1];
        let mut total = 0;
        for (class, entry) in classes.iter_mut().enumerate().skip(1) {
            let taps = 2 * half_taps(shape(class_rate(class)).1);
            *entry = (total, taps);
            total += taps * (PHASES + 1);
        }
        let mut weights = vec![0.0_f32; total].into_boxed_slice();
        let mut sums = vec![0.0_f32; CLASSES * (PHASES + 1)].into_boxed_slice();
        for class in 1..=CLASSES {
            let (cutoff, half_span) = shape(class_rate(class));
            let (offset, taps) = classes[class];
            let first = 1.0 - (taps / 2) as f64;
            for phase in 0..=PHASES {
                let fraction = phase as f64 / PHASES as f64;
                let row = &mut weights[offset + phase * taps..offset + (phase + 1) * taps];
                let mut sum = 0.0;
                for (tap, weight) in row.iter_mut().enumerate() {
                    let t = first + tap as f64 - fraction;
                    let v = t / half_span;
                    let x = cutoff * t;
                    let sinc = if x == 0.0 {
                        1.0
                    } else {
                        (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
                    };
                    let window = if v.abs() >= 1.0 {
                        0.0
                    } else {
                        bessel_i0(KAISER_BETA * (1.0 - v * v).sqrt()) / norm
                    };
                    *weight = (sinc * window) as f32;
                    sum += sinc * window;
                }
                sums[(class - 1) * (PHASES + 1) + phase] = sum as f32;
            }
        }
        Bank {
            weights,
            sums,
            classes,
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

/// Per-voice kernel: the rate class chosen on control from the voice step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Kernel {
    class: usize,
}

/// One output frame's taps: two adjacent phase rows and their blend.
/// Weight of tap `i` is `(1 - blend) * lower[i] + blend * upper[i]`, over
/// playback offsets `first + i` relative to the integer source position.
pub(super) struct Taps {
    pub(super) first: i64,
    pub(super) lower: &'static [f32],
    pub(super) upper: &'static [f32],
    pub(super) blend: f32,
    /// Sum of all blended weights; the output is normalized by it.
    pub(super) total: f32,
}

impl Kernel {
    /// `None` for unity or slower playback, which keeps linear interpolation.
    /// Otherwise the class at or above `rate` (its cutoff is never higher than
    /// the voice needs), capped at the highest class.
    pub(super) fn for_rate(rate: f64) -> Option<Self> {
        if !rate.is_finite() || rate <= 1.0 {
            return None;
        }
        let class = (rate.log2() * CLASSES_PER_OCTAVE - 1e-9).ceil();
        Some(Self {
            class: (class.max(1.0) as usize).min(CLASSES),
        })
    }

    /// Taps for a source position with `fraction` in [0, 1).
    #[inline]
    pub(super) fn taps(self, fraction: f64) -> Option<Taps> {
        let bank = bank();
        let &(offset, taps) = bank.classes.get(self.class)?;
        let scaled = (fraction * PHASES as f64).clamp(0.0, PHASES as f64);
        let phase = (scaled as usize).min(PHASES - 1);
        let blend = (scaled - phase as f64) as f32;
        let lower_start = offset + phase * taps;
        let lower = bank.weights.get(lower_start..lower_start + taps)?;
        let upper = bank
            .weights
            .get(lower_start + taps..lower_start + 2 * taps)?;
        let sums = bank.sums.get((self.class - 1) * (PHASES + 1) + phase..)?;
        let (Some(low_sum), Some(high_sum)) = (sums.first(), sums.get(1)) else {
            return None;
        };
        Some(Taps {
            first: 1 - (taps / 2) as i64,
            lower,
            upper,
            blend,
            total: low_sum + (high_sum - low_sum) * blend,
        })
    }
}
