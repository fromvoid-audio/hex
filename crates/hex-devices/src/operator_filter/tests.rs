use super::*;
#[test]
fn fixed_filter_has_dc_and_nyquist_limits_and_stable_extremes() {
    for rate in [8000, 44100, 48000, 192000] {
        for hz in [20.0, 1000.0, 20000.0] {
            for res in [0.0, 50.0, 100.0] {
                for mode in 0..5 {
                    for slope in 0..2 {
                        let mut s = Settings::default();
                        for (i, v) in [
                            (0, ParameterValue::Bool(true)),
                            (1, ParameterValue::Choice(mode)),
                            (2, ParameterValue::Choice(slope)),
                            (4, ParameterValue::Float(hz)),
                            (5, ParameterValue::Float(res)),
                        ] {
                            s.patch(ParameterId::from_static(FIRST + i), v).unwrap();
                        }
                        let p = s.prepare(rate).unwrap();
                        let mut c = Carrier::default();
                        for i in 0..20000 {
                            let y = c.process(if i == 0 { 1.0 } else { 0.0 }, p.frame);
                            assert!(y.is_finite() && y.abs() < 100.0);
                        }
                    }
                }
            }
        }
    }
    let mut s = Settings::default();
    s.patch(ParameterId::from_static(FIRST), ParameterValue::Bool(true))
        .unwrap();
    let p = s.prepare(48000).unwrap();
    let mut c = Carrier::default();
    let mut y = 0.0;
    for _ in 0..10000 {
        y = c.process(1.0, p.frame);
    }
    assert!((y - 1.0).abs() < 1e-12);
    let mut x = Carrier::default();
    for _ in 0..10000 {
        y = x.process(0.0, p.frame);
    }
    assert_eq!(y, 0.0);
}
#[test]
fn codec_rejects_reserved_fields_and_malformed_inputs() {
    let s = Settings::default();
    let mut bytes = Vec::new();
    s.append(&mut bytes);
    assert_eq!(Settings::decode(&bytes).unwrap(), s);
    for offset in [0, 4, 8, 12, 16, 20, 24, 28, 32] {
        let mut b = bytes.clone();
        b[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Settings::decode(&b).is_err());
    }
    assert!(Settings::decode(&bytes[..bytes.len() - 1]).is_err());
}

#[test]
fn impulse_dft_matches_independent_prewarped_analog_transfer() {
    let rate = 48000;
    let cutoff = 2000.0_f32;
    for mode in 0..5 {
        for slope in 0..2 {
            for resonance in [0.0_f32, 75.0, 100.0] {
                let mut s = Settings::default();
                for (i, v) in [
                    (0, ParameterValue::Bool(true)),
                    (1, ParameterValue::Choice(mode)),
                    (2, ParameterValue::Choice(slope)),
                    (4, ParameterValue::Float(cutoff)),
                    (5, ParameterValue::Float(resonance)),
                    (6, ParameterValue::Float(37.0)),
                ] {
                    s.patch(ParameterId::from_static(FIRST + i), v).unwrap();
                }
                let p = s.prepare(rate).unwrap();
                let mut filter = Carrier::default();
                let impulse: Vec<_> = (0..8192)
                    .map(|i| filter.process(if i == 0 { 1.0 } else { 0.0 }, p.frame))
                    .collect();
                let q = std::f64::consts::FRAC_1_SQRT_2
                    + (20.0 - std::f64::consts::FRAC_1_SQRT_2)
                        * (f64::from(resonance) / 100.0).powi(2);
                for hz in [0.0, 1000.0, 2000.0, 4000.0, 24000.0] {
                    let w = (std::f64::consts::PI * hz / f64::from(rate)).tan()
                        / (std::f64::consts::PI * f64::from(cutoff) / f64::from(rate)).tan();
                    let (nr, ni) = match mode {
                        0 => (1.0, 0.0),
                        1 => (-w * w, 0.0),
                        2 => (0.0, w / q),
                        3 => (1.0 - w * w, 0.0),
                        _ => (0.26, 0.74 * w / q),
                    };
                    let amplitude = ((nr * nr + ni * ni)
                        / ((1.0 - w * w).powi(2) + (w / q).powi(2)))
                    .sqrt()
                    .powi(if slope == 0 { 1 } else { 2 });
                    let (mut re, mut im) = (0.0, 0.0);
                    for (n, x) in impulse.iter().enumerate() {
                        let phase = std::f64::consts::TAU * hz * n as f64 / f64::from(rate);
                        re += x * phase.cos();
                        im -= x * phase.sin();
                    }
                    let measured = re.hypot(im);
                    assert!(
                        (measured - amplitude).abs() < 1e-6 * (1.0 + amplitude),
                        "mode={mode} slope={slope} res={resonance} hz={hz} measured={measured} oracle={amplitude}"
                    );
                }
            }
        }
    }
}

#[test]
fn rapid_parameter_motion_remains_finite_without_clipping_or_silence_fallback() {
    let mut a = Settings::default();
    let mut b = a;
    a.patch(ParameterId::from_static(FIRST), ParameterValue::Bool(true))
        .unwrap();
    b.patch(ParameterId::from_static(FIRST), ParameterValue::Bool(true))
        .unwrap();
    a.patch(
        ParameterId::from_static(FIRST + 4),
        ParameterValue::Float(20.0),
    )
    .unwrap();
    b.patch(
        ParameterId::from_static(FIRST + 4),
        ParameterValue::Float(20000.0),
    )
    .unwrap();
    b.patch(
        ParameterId::from_static(FIRST + 5),
        ParameterValue::Float(100.0),
    )
    .unwrap();
    let (a, b) = (a.prepare(48000).unwrap(), b.prepare(48000).unwrap());
    let mut c = Carrier::default();
    let mut seed = 1_u64;
    for n in 0..100000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let t = (seed >> 32) as f64 / u32::MAX as f64;
        let y = c.process(
            if n % 2 == 0 { 1.0 } else { -1.0 },
            a.frame.interpolate(b.frame, t),
        );
        assert!(y.is_finite() && y.abs() < 10000.0);
    }
}
