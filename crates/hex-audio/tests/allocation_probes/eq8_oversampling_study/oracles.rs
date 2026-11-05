use super::kernel::Frame;
/// Direct mathematical generator; no production odd table/cursor reuse.
pub(super) fn taps() -> [f64; 33] {
    let mut h = std::array::from_fn(|k| {
        let m = k as i32 - 16;
        if m == 0 {
            0.5
        } else if m % 2 == 0 {
            0.0
        } else {
            let x = f64::from(m);
            (std::f64::consts::PI * x / 2.0).sin() / (std::f64::consts::PI * x)
                * (0.5 + 0.5 * (std::f64::consts::PI * x / 16.0).cos())
        }
    });
    let sum = h
        .iter()
        .enumerate()
        .filter(|(k, _)| *k != 16)
        .map(|(_, v)| v)
        .sum::<f64>();
    for (k, v) in h.iter_mut().enumerate() {
        if k != 16 {
            *v *= 0.5 / sum;
        }
    }
    h
}
/// Full zero insertion/convolution/EQ-test-map/convolution/even-phase sampling.
pub(super) fn direct(input: &[Frame], map: impl Fn(usize, Frame) -> Frame) -> Vec<Frame> {
    let h = taps();
    let mut high = vec![[0.0; 2]; input.len() * 2];
    for (k, out) in high.iter_mut().enumerate() {
        for (t, w) in h.iter().enumerate() {
            if k >= t && (k - t) % 2 == 0 {
                for c in 0..2 {
                    out[c] += 2.0 * w * input[(k - t) / 2][c];
                }
            }
        }
        *out = map(k, *out);
    }
    (0..input.len())
        .map(|n| {
            let mut out = [0.0; 2];
            for (t, w) in h.iter().enumerate() {
                if 2 * n >= t {
                    for c in 0..2 {
                        out[c] += w * high[2 * n - t][c];
                    }
                }
            }
            out
        })
        .collect()
}
pub(super) fn close(a: f64, b: f64) {
    assert!(
        (a - b).abs() <= 2e-12 * (1.0 + a.abs().max(b.abs())),
        "{a} versus {b}"
    );
}
