//! Shared original fixed bipolar modulation shapes; no device ownership/state.
/// Fixed deterministic bipolar hold value: SplitMix64 finalizer of the cycle.
pub(crate) fn waveform(choice: u32, phase: f64, cycle: u64) -> f64 {
    match choice {
        0 => (std::f64::consts::TAU * phase).sin(),
        1 => {
            if phase < 0.25 {
                4.0 * phase
            } else if phase < 0.75 {
                2.0 - 4.0 * phase
            } else {
                4.0 * phase - 4.0
            }
        }
        2 => 2.0 * phase - 1.0,
        3 => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        4 => {
            let mut x = cycle.wrapping_add(0x9e3779b97f4a7c15);
            x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
            x ^= x >> 31;
            (x >> 11) as f64 * (2.0 / 9_007_199_254_740_992.0) - 1.0
        }
        _ => 0.0,
    }
}
