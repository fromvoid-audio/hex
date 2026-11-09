//! Original 2x FM graph evaluation and eighth-order Butterworth decimation.
//! Envelopes and LFO controls are held over the two subframes. The carrier
//! filter is prepared at2Fs. There is no buffering/lookahead latency; IIR phase
//! delay is frequency dependent. A bounded4096-frame silent drain is declared.
use super::*;
pub(super) const TAIL: u64 = 4096;
#[derive(Clone, Copy)]
pub(in crate::operator_state) struct Decimator {
    state: [[f64; 2]; 4],
    silent: u32,
}
impl Default for Decimator {
    fn default() -> Self {
        Self {
            state: [[0.0; 2]; 4],
            silent: TAIL as u32,
        }
    }
}
impl Decimator {
    #[inline(always)]
    pub fn sample(&mut self, input: f32) -> f32 {
        if input == 0.0 {
            self.silent = self.silent.saturating_add(1)
        } else {
            self.silent = 0;
        }
        if self.silent >= 2 * TAIL as u32 {
            self.state = [[0.0; 2]; 4];
            return 0.0;
        }
        // Bilinear-transform Butterworth8 at0.4 outputFs (=0.2 of2Fs).
        let mut value = f64::from(input);
        const K: [f64; 4] = [
            0.3901806440322565,
            1.1111404660392044,
            1.6629392246050905,
            1.9615705608064609,
        ];
        const G: f64 = 0.7265425280053609; // tan(pi*0.2)
        for (state, k) in self.state.iter_mut().zip(K) {
            let a = 1.0 / (1.0 + G * (G + k));
            let v1 = a * (state[0] + G * (value - state[1]));
            let v2 = state[1] + G * v1;
            state[0] = 2.0 * v1 - state[0];
            state[1] = 2.0 * v2 - state[1];
            if state[0].abs() < 1e-30 {
                state[0] = 0.0
            }
            if state[1].abs() < 1e-30 {
                state[1] = 0.0
            }
            value = v2;
        }
        value as f32
    }
    #[cfg(test)]
    pub fn quiet(self) -> bool {
        self.state == [[0.0; 2]; 4]
    }
}
pub(super) fn lfo_table() -> &'static LfoTable {
    static TABLE: std::sync::OnceLock<LfoTable> = std::sync::OnceLock::new();
    TABLE.get_or_init(LfoTable::prepare)
}
pub(super) struct LfoTable([[f64; 1025]; 4]);
impl LfoTable {
    pub fn prepare() -> Self {
        Self(std::array::from_fn(|shape| {
            std::array::from_fn(|i| lfo::waveform(shape as u32, i as f64 / 1024.0, 0))
        }))
    }
    pub fn sample(&self, shape: u32, phase: f64, cycle: u64, interpolate: bool) -> f64 {
        if shape == 4 {
            return lfo::waveform(shape, phase, cycle);
        }
        let Some(table) = self.0.get(shape as usize) else {
            return 0.0;
        };
        let position = phase.clamp(0.0, 1.0) * 1024.0;
        let i = (position as usize).min(1023);
        if interpolate {
            table[i] + (table[i + 1] - table[i]) * (position - i as f64)
        } else {
            table[i]
        }
    }
}
