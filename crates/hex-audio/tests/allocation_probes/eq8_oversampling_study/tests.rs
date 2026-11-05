use super::{
    kernel::{Error, Frame, History, weights_bytes},
    oracles::{close, direct},
};
use allocation_counter::measure;
use std::hint::black_box;
fn signal(n: usize) -> Frame {
    [
        (n as f64 * 0.173).sin() * 0.7,
        (n as f64 * 0.117).cos() * 0.4,
    ]
}
#[test]
fn direct_convolution_matches_polyphase_identity_and_nonidentity_transform_at_all_rates() {
    let input: Vec<_> = (0..1024).map(signal).collect();
    for rate in [
        16000, 22050, 32000, 88200, 96000, 176400, 192000, 352800, 384000,
    ] {
        for changed in [false, true] {
            let map = |x: Frame| {
                if changed {
                    x.map(|v| v * v * 0.25 - v * 0.1)
                } else {
                    x
                }
            };
            let expected = direct(&input, |_, x| map(x));
            let mut history = History::new(rate).unwrap();
            for (input, expected) in input.iter().zip(expected) {
                let actual = history.process(*input, |p| p.map(map)).unwrap();
                for c in 0..2 {
                    close(actual[c], expected[c]);
                }
            }
        }
    }
}
#[test]
fn impulse_dc_reset_and_arbitrary_partitions_preserve_exact_history() {
    let mut impulse = vec![[0.0; 2]; 96];
    impulse[0] = [1.0, -0.5];
    let expected = direct(&impulse, |_, x| x);
    let mut history = History::new(96000).unwrap();
    for (x, e) in impulse.iter().zip(expected) {
        let a = history.process(*x, |p| p).unwrap();
        for c in 0..2 {
            close(a[c], e[c]);
        }
    }
    history.reset();
    assert_eq!(history, History::new(96000).unwrap());
    for n in 0..128 {
        let out = history.process([0.75, -0.25], |p| p).unwrap();
        if n >= 32 {
            close(out[0], 0.75);
            close(out[1], -0.25);
        }
    }
    let input: Vec<_> = (0..2049).map(signal).collect();
    let mut whole = History::new(384000).unwrap();
    let all: Vec<_> = input
        .iter()
        .map(|x| whole.process(*x, |p| p).unwrap())
        .collect();
    let mut split = History::new(384000).unwrap();
    let mut n = 0;
    let mut seed = 1u32;
    while n < input.len() {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let end = (n + 1 + seed as usize % 257).min(input.len());
        for i in n..end {
            assert_eq!(
                split.process(input[i], |p| p).unwrap().map(f64::to_bits),
                all[i].map(f64::to_bits)
            );
        }
        n = end;
    }
    assert_eq!(whole, split);
}
#[test]
fn interpolation_images_and_decimation_aliases_are_real_filtered_signals() {
    let mut h = History::new(96000).unwrap();
    let mut samples = Vec::new();
    for n in 0..4096 {
        h.process(
            [(std::f64::consts::TAU * 0.1 * n as f64).sin(); 2],
            |pair| {
                samples.extend([pair[0][0], pair[1][0]]);
                pair
            },
        )
        .unwrap();
    }
    let amplitude = |f: f64| {
        let mut r = 0.0;
        let mut i = 0.0;
        let values = &samples[128..];
        for (n, v) in values.iter().enumerate() {
            r += v * (std::f64::consts::TAU * f * (n + 128) as f64).cos();
            i += v * (std::f64::consts::TAU * f * (n + 128) as f64).sin();
        }
        2.0 * r.hypot(i) / values.len() as f64
    };
    assert!(amplitude(0.05) > 0.99);
    assert!(amplitude(0.45) < 0.01);
    let mut h = History::new(96000).unwrap();
    let mut energy = 0.0;
    for n in 0..4096 {
        let out = h
            .process([0.0; 2], |_| {
                std::array::from_fn(|phase| {
                    [(std::f64::consts::TAU * 0.35 * (2 * n + phase) as f64).sin(); 2]
                })
            })
            .unwrap();
        if n >= 64 {
            energy += out[0] * out[0];
        }
    }
    assert!((2.0 * energy / (4096 - 64) as f64).sqrt() < 0.01);
}
#[test]
fn malformed_rates_nonfinite_maps_and_refused_retry_leave_all_histories_unchanged() {
    for rate in [0, 15998, 16001, 384001, u32::MAX] {
        assert_eq!(History::new(rate), Err(Error::Rate));
    }
    let mut h = History::new(96000).unwrap();
    for n in 0..33 {
        h.process(signal(n), |p| p).unwrap();
    }
    let old = h;
    assert_eq!(h.process([f64::NAN, 0.0], |p| p), Err(Error::Nonfinite));
    assert_eq!(h, old);
    assert_eq!(
        h.process(signal(34), |_| [[f64::INFINITY; 2]; 2]),
        Err(Error::Nonfinite)
    );
    assert_eq!(h, old);
    let mut twin = old;
    assert_eq!(
        h.process(signal(34), |p| p),
        twin.process(signal(34), |p| p)
    );
    assert_eq!(h, twin);
}
#[test]
fn constructor_processing_reset_and_refusal_have_zero_total_and_signed_net_allocations() {
    assert_eq!(weights_bytes(), 128);
    let observed = measure(|| {
        let mut h = black_box(History::new(384000).unwrap());
        for n in 0..4096 {
            black_box(h.process(black_box(signal(n)), |p| p).unwrap());
        }
        assert_eq!(
            black_box(h.process([f64::NAN, 0.0], |p| p)),
            Err(Error::Nonfinite)
        );
        h.reset();
        black_box(h);
    });
    assert_eq!(
        (
            observed.count_total,
            observed.bytes_total,
            observed.count_current,
            observed.bytes_current
        ),
        (0, 0, 0, 0)
    );
    // Instrument sensitivity: balanced allocation cannot masquerade as allocation-free.
    let bad = measure(|| {
        let b = black_box(Box::new(black_box(42u64)));
        black_box(&b);
        drop(b);
    });
    assert_eq!(
        (
            bad.count_total,
            bad.bytes_total,
            bad.count_current,
            bad.bytes_current
        ),
        (1, 8, 0, 0)
    );
    let b = black_box(Box::new(1u64));
    let freed = measure(|| drop(b));
    assert_eq!((freed.count_current, freed.bytes_current), (-1, -8));
}

#[test]
fn every_allowed_internal_rate_and_signed_zero_history_refusal_is_preserved() {
    for rate in (16000..=384000).step_by(2) {
        assert!(History::new(rate).is_ok());
    }
    let mut history = History::new(96000).unwrap();
    for _ in 0..33 {
        history.process([-0.0; 2], |p| p).unwrap();
    }
    let before = history;
    assert_eq!(
        history.process([0.0; 2], |_| [[f64::NAN; 2]; 2]),
        Err(Error::Nonfinite)
    );
    assert_eq!(history, before);
}
