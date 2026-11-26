//! Output-aware ITU-R BS.1770-4 4x true-peak post-guard.
//!
//! Runs after the primary lookahead stage (and after the M/S decode + safety
//! stage in M/S routing) solely in True Peak mode. Because the primary stage
//! already keeps steady-state audio at or below the ceiling and bounds
//! modulation-induced overshoots to a few tenths of a dB, this inline 12-tap
//! post-guard only attenuates when the actual post-gain signal exceeds the
//! ceiling in the BS.1770-4 4x polyphase filter bank.
//!
//! All storage is fixed-size and inline (`Copy`), with zero heap allocations
//! and exact declared latency `GUARD_DELAY = 11` frames.

use super::{Config, Parameters, meter_factor};

pub(super) const GUARD_TAPS: usize = 12;
pub(super) const GUARD_DELAY: usize = GUARD_TAPS - 1;

/// ITU-R BS.1770-4 Annex 2 4x polyphase 12-tap coefficients in chronological
/// sample order (oldest to newest). The four phases form two palindromic pairs,
/// so forward and reverse order span the exact same 4x interpolation grid.
const BS1770_PHASES: [[f64; GUARD_TAPS]; 4] = [
    [
        0.001_708_984_375_0,
        0.010_986_328_125_0,
        -0.019_653_320_312_5,
        0.033_203_125_000_0,
        -0.059_448_242_187_5,
        0.137_329_101_562_5,
        0.972_167_968_750_0,
        -0.102_294_921_875_0,
        0.047_607_421_875_0,
        -0.026_611_328_125_0,
        0.014_892_578_125_0,
        -0.008_300_781_250_0,
    ],
    [
        -0.029_174_804_687_5,
        0.029_296_875_000_0,
        -0.051_757_812_500_0,
        0.089_111_328_125_0,
        -0.166_503_906_250_0,
        0.465_087_890_625_0,
        0.779_785_156_250_0,
        -0.200_317_382_812_5,
        0.101_562_500_000_0,
        -0.058_227_539_062_5,
        0.033_081_054_687_5,
        -0.018_920_898_437_5,
    ],
    [
        -0.018_920_898_437_5,
        0.033_081_054_687_5,
        -0.058_227_539_062_5,
        0.101_562_500_000_0,
        -0.200_317_382_812_5,
        0.779_785_156_250_0,
        0.465_087_890_625_0,
        -0.166_503_906_250_0,
        0.089_111_328_125_0,
        -0.051_757_812_500_0,
        0.029_296_875_000_0,
        -0.029_174_804_687_5,
    ],
    [
        -0.008_300_781_250_0,
        0.014_892_578_125_0,
        -0.026_611_328_125_0,
        0.047_607_421_875_0,
        -0.102_294_921_875_0,
        0.972_167_968_750_0,
        0.137_329_101_562_5,
        -0.059_448_242_187_5,
        0.033_203_125_000_0,
        -0.019_653_320_312_5,
        0.010_986_328_125_0,
        0.001_708_984_375_0,
    ],
];

#[inline(always)]
fn bs1770_window_peak(window: &[f64]) -> f64 {
    let mut peak = 0.0_f64;
    for phase in &BS1770_PHASES {
        let mut acc = 0.0_f64;
        for (&sample, &coeff) in window.iter().zip(phase.iter()) {
            acc += sample * coeff;
        }
        peak = peak.max(acc.abs());
    }
    peak
}

#[derive(Clone, Copy, Default)]
struct GuardSlot {
    signal: [f64; 2],
    attenuation: [f64; 2],
    peak: [f64; 2],
}

/// Inline, allocation-free BS.1770-4 output-aware post-guard.
#[derive(Clone, Copy)]
pub(super) struct OutputGuard {
    ring: [GuardSlot; GUARD_TAPS],
    // Doubled planar ring buffer so `[input_next..input_next + GUARD_TAPS]` is
    // contiguous with zero modulo indexing.
    input: [[f64; GUARD_TAPS * 2]; 2],
    next: usize,
    input_next: usize,
    nonzero: usize,
    gain: [f64; 2],
}

impl Default for OutputGuard {
    fn default() -> Self {
        Self {
            ring: [GuardSlot::default(); GUARD_TAPS],
            input: [[0.0; GUARD_TAPS * 2]; 2],
            next: 0,
            input_next: 0,
            nonzero: 0,
            gain: [1.0; 2],
        }
    }
}

impl OutputGuard {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn quiet(&self) -> bool {
        self.nonzero == 0
    }

    pub(super) fn is_idle_unity(&self) -> bool {
        self.nonzero == 0
            && self.gain == [1.0; 2]
            && self.input[0][..GUARD_TAPS].iter().all(|&v| v == 0.0)
            && self.input[1][..GUARD_TAPS].iter().all(|&v| v == 0.0)
            && self.ring.iter().all(|slot| slot.peak == [0.0; 2])
    }

    pub(super) fn advance_silent(&mut self, count: usize) {
        self.next = (self.next + count) % GUARD_TAPS;
        self.input_next = (self.input_next + count) % GUARD_TAPS;
    }

    #[inline(always)]
    pub(super) fn process(
        &mut self,
        signal: [f64; 2],
        attenuation: [f64; 2],
        config: &Config,
        parameters: &Parameters,
        link: f64,
    ) -> ([f64; 2], [f64; 2]) {
        let w = self.input_next;
        self.input[0][w] = signal[0];
        self.input[0][w + GUARD_TAPS] = signal[0];
        self.input[1][w] = signal[1];
        self.input[1][w + GUARD_TAPS] = signal[1];
        self.input_next = if w + 1 == GUARD_TAPS { 0 } else { w + 1 };

        let start = self.input_next;
        let peak = [
            signal[0].abs().max(bs1770_window_peak(
                &self.input[0][start..start + GUARD_TAPS],
            )),
            signal[1].abs().max(bs1770_window_peak(
                &self.input[1][start..start + GUARD_TAPS],
            )),
        ];

        let old_next = self.next;
        if self.ring[old_next].signal != [0.0; 2] {
            self.nonzero -= 1;
        }
        if signal != [0.0; 2] {
            self.nonzero += 1;
        }
        self.ring[old_next] = GuardSlot {
            signal,
            attenuation,
            peak,
        };
        self.next = if old_next + 1 == GUARD_TAPS {
            0
        } else {
            old_next + 1
        };

        // Every 12-tap BS.1770-4 window covering `self.ring[self.next]` is
        // currently stored across the 12 slots of `self.ring`.
        let mut worst = [0.0_f64; 2];
        for slot in &self.ring {
            worst[0] = worst[0].max(slot.peak[0]);
            worst[1] = worst[1].max(slot.peak[1]);
        }

        let mut delayed = self.ring[self.next];
        self.ring[self.next] = GuardSlot::default();
        if delayed.signal != [0.0; 2] {
            self.nonzero -= 1;
        }

        let worst_both = worst[0].max(worst[1]);
        let ceiling = parameters.output_ceiling;
        let release_coeff = parameters
            .release_coefficient
            .min(config.antimodulation_coefficient);
        let mut applied = [1.0_f64; 2];
        for lane in 0..2 {
            let lane_peak = worst[lane] + link * (worst_both - worst[lane]);
            let target = if lane_peak > ceiling {
                ceiling / lane_peak
            } else {
                1.0
            };
            let gain = if target <= self.gain[lane] {
                target
            } else {
                target.min(self.gain[lane] + release_coeff * (target - self.gain[lane]))
            };
            self.gain[lane] = gain;
            applied[lane] = gain;
            delayed.signal[lane] *= gain;
        }

        let combined_attenuation = std::array::from_fn(|lane| {
            let carried = if delayed.attenuation[lane] == 0.0 {
                1.0
            } else {
                delayed.attenuation[lane]
            };
            carried * meter_factor(applied[lane])
        });
        (delayed.signal, combined_attenuation)
    }
}
