use super::{kernel::ODD, oracles::taps};
fn amplitude(h: &[f64; 33], f: f64) -> f64 {
    h.iter()
        .enumerate()
        .map(|(k, v)| v * (std::f64::consts::TAU * f * (k as f64 - 16.0)).cos())
        .sum()
}
fn bounds(h: &[f64; 33], lo: f64, hi: f64) -> (f64, f64) {
    const CELLS: usize = 65536;
    // |dA/df| <= 2pi sum |k-center||h[k]|. Every point is <=half a cell from a sample.
    let lipschitz = std::f64::consts::TAU
        * h.iter()
            .enumerate()
            .map(|(k, v)| (k as f64 - 16.0).abs() * v.abs())
            .sum::<f64>();
    // Explicit numerical allowance, in addition to the analytic between-grid bound.
    let uncertainty = lipschitz * (hi - lo) / (2.0 * CELLS as f64) + 1e-10;
    let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
    for i in 0..=CELLS {
        let a = amplitude(h, lo + (hi - lo) * i as f64 / CELLS as f64);
        low = low.min(a);
        high = high.max(a);
    }
    eprintln!("band {lo}..{hi} A={low}..{high} derivative={lipschitz} uncertainty={uncertainty}");
    (low - uncertainty, high + uncertainty)
}
#[test]
fn independent_math_coefficients_and_continuous_response_bounds() {
    let h = taps();
    assert_eq!(h[16], 0.5);
    for k in 0..33 {
        assert_eq!(h[k], h[32 - k]);
        if k != 16 && k % 2 == 0 {
            assert_eq!(h[k], 0.0);
        }
    }
    for (j, weight) in ODD.into_iter().enumerate() {
        assert!((weight - h[2 * j + 1]).abs() < 1e-15);
        assert_eq!(weight.to_bits(), ODD[15 - j].to_bits());
    }
    assert!((h.iter().sum::<f64>() - 1.0).abs() < 2e-15);
    let (low, high) = bounds(&h, 0.0, 0.2);
    assert!(low >= 10f64.powf(-0.06 / 20.0) && high <= 10f64.powf(0.06 / 20.0));
    let pair = |a: f64| a * a + (1.0 - a).powi(2);
    assert!(
        low > 0.5 && pair(low) >= 10f64.powf(-0.12 / 20.0) && pair(high) <= 10f64.powf(0.12 / 20.0)
    );
    let (low, high) = bounds(&h, 0.3, 0.5);
    assert!(low.abs().max(high.abs()) <= 0.01); // >=40dB single-stage stop attenuation.
    // Independent full complex DFT checks the zero-phase/symmetric shortcut.
    for i in 0..=2048 {
        let f = 0.5 * i as f64 / 2048.0;
        let real = h
            .iter()
            .enumerate()
            .map(|(k, v)| v * (std::f64::consts::TAU * f * k as f64).cos())
            .sum::<f64>();
        let imag = h
            .iter()
            .enumerate()
            .map(|(k, v)| -v * (std::f64::consts::TAU * f * k as f64).sin())
            .sum::<f64>();
        assert!((real.hypot(imag) - amplitude(&h, f).abs()).abs() < 1e-13);
    }
}
#[test]
fn identity_pair_phase_delay_and_nyquist_rolloff_have_independent_impulse_oracle() {
    let h = taps();
    let g: [f64; 33] = std::array::from_fn(|n| {
        2.0 * h
            .iter()
            .enumerate()
            .filter(|(k, _)| 2 * n >= *k && 2 * n - *k < 33)
            .map(|(k, v)| v * h[2 * n - k])
            .sum::<f64>()
    });
    for k in 0..33 {
        assert!((g[k] - g[32 - k]).abs() < 1e-15);
    }
    assert!((g.iter().sum::<f64>() - 1.0).abs() < 2e-15);
    assert!((g.iter().enumerate().map(|(k, v)| k as f64 * v).sum::<f64>() - 16.0).abs() < 1e-13);
    assert_eq!(g[0], 0.0);
    assert!(g[1] > 0.0); // Delay is the linear-phase center, not sixteen initial zero frames.
    for f in [0.01, 0.05, 0.1, 0.2, 0.4] {
        let r = g
            .iter()
            .enumerate()
            .map(|(k, v)| v * (std::f64::consts::TAU * f * (k as f64 - 16.0)).cos())
            .sum::<f64>();
        let a = amplitude(&h, f * 0.5);
        assert!((r - (a * a + (1.0 - a).powi(2))).abs() < 1e-13);
    }
    assert!((amplitude(&h, 0.25) - 0.5).abs() < 1e-14);
}
