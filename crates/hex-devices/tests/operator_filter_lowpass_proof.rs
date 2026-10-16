//! Original mathematical proof only; no registered Operator/filter admission.
#[path = "../src/operator_filter_lowpass.rs"]
mod filter;

use filter::{Error, Lowpass, PreparedLowpass};

fn prepare(cutoff: f64, rate: u32) -> PreparedLowpass {
    PreparedLowpass::prepare(cutoff, rate).unwrap()
}

#[test]
fn domain_endpoints_and_poles_are_strictly_inside_the_unit_circle() {
    for rate in [8_000, 11_025, 44_100, 48_000, 96_000, 192_000] {
        let maximum = 20_000.0_f64.min(0.45 * f64::from(rate));
        for cutoff in [20.0, 1000.0, maximum] {
            let [b0, b1, b2, a1, a2] = prepare(cutoff, rate).coefficients();
            assert!([b0, b1, b2, a1, a2].iter().all(|x| x.is_finite()));
            // Independent denominator Jury conditions and conjugate pole norm.
            assert!(1.0 + a1 + a2 > 0.0);
            assert!(1.0 - a1 + a2 > 0.0);
            assert!(1.0 - a2 > 0.0);
            assert!(a1 * a1 - 4.0 * a2 < 0.0);
            assert!(a2.sqrt() < 1.0);
            assert!((b0 + b1 + b2 - (1.0 + a1 + a2)).abs() < 1e-15);
            assert_eq!(b0 - b1 + b2, 0.0);
        }
        for cutoff in [
            19.999,
            maximum + 0.001,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            assert_eq!(
                PreparedLowpass::prepare(cutoff, rate),
                Err(Error::InvalidCutoff)
            );
        }
    }
    for rate in [0, 7999, 192001, u32::MAX] {
        assert_eq!(
            PreparedLowpass::prepare(1000.0, rate),
            Err(Error::InvalidRate)
        );
    }
}

#[test]
fn quarter_rate_impulse_matches_closed_hand_values_and_channel_isolation() {
    let mut state = Lowpass::new(prepare(12_000.0, 48_000));
    // Prewarp k=1 gives a1=0, b0=1-1/sqrt(2), a2=3-2sqrt(2).
    // Values below are independent closed impulse terms, not implementation calls.
    let expected = [
        0.2928932188134525,
        0.585786437626905,
        0.242640687119285,
        -0.10050506338833465,
        -0.04163056034261584,
    ];
    for (age, expected) in expected.into_iter().enumerate() {
        let out = state
            .process_frame([if age == 0 { 1.0 } else { 0.0 }, 0.0])
            .unwrap();
        assert!((f64::from(out[0]) - expected).abs() < 3e-8, "age={age}");
        assert_eq!(out[1], 0.0);
    }
}

#[test]
fn impulse_dft_matches_prewarped_analog_butterworth_transfer_dc_and_nyquist() {
    let cutoff = 1000.0;
    let rate = 48_000;
    let mut state = Lowpass::new(prepare(cutoff, rate));
    let impulse: Vec<_> = (0..8192)
        .map(|age| {
            f64::from(
                state
                    .process_frame([if age == 0 { 1.0 } else { 0.0 }; 2])
                    .unwrap()[0],
            )
        })
        .collect();
    for frequency in [0.0, 100.0, cutoff, 3000.0, 12_000.0, 24_000.0] {
        let omega = std::f64::consts::TAU * frequency / f64::from(rate);
        let (mut re, mut im) = (0.0, 0.0);
        for (age, sample) in impulse.iter().enumerate() {
            let (sin, cos) = (omega * age as f64).sin_cos();
            re += sample * cos;
            im -= sample * sin;
        }
        let expected = if frequency == 24_000.0 {
            0.0
        } else {
            let ratio =
                (omega / 2.0).tan() / (std::f64::consts::PI * cutoff / f64::from(rate)).tan();
            (1.0 + ratio.powi(4)).sqrt().recip()
        };
        assert!((re.hypot(im) - expected).abs() < 3e-8, "hz={frequency}");
    }
    assert!(
        impulse.iter().any(|sample| *sample < 0.0),
        "negative control: not a one-pole smoother"
    );
}

#[test]
fn partitions_exact_history_copy_and_reset_preserve_observable_pcm() {
    let prepared = prepare(1300.0, 44_100);
    let input: Vec<_> = (0..1024)
        .map(|i| [(i as f32 * 0.13).sin(), (i as f32 * 0.07).cos()])
        .collect();
    let mut scalar = Lowpass::new(prepared);
    let expected: Vec<_> = input
        .iter()
        .map(|x| scalar.process_frame(*x).unwrap())
        .collect();
    for block in [1, 7, 64, 128, 257] {
        let mut actual = Lowpass::new(prepared);
        let mut output = Vec::new();
        for chunk in input.chunks(block) {
            output.extend(chunk.iter().map(|x| actual.process_frame(*x).unwrap()));
        }
        assert_eq!(output, expected);
        assert_eq!(actual, scalar);
    }
    let mut source = Lowpass::new(prepared);
    for input in &input[..313] {
        source.process_frame(*input).unwrap();
    }
    let mut copy = Lowpass::new(prepared);
    copy.copy_history_from(&source).unwrap();
    for input in &input[313..] {
        assert_eq!(source.process_frame(*input), copy.process_frame(*input));
    }
    copy.reset();
    assert_eq!(copy, Lowpass::new(prepared));
    assert_eq!(copy.process_frame([0.0; 2]), Ok([0.0; 2]));
    for other in [prepare(1400.0, 44_100), prepare(1300.0, 48_000)] {
        let mut incompatible = Lowpass::new(other);
        incompatible.process_frame([0.3; 2]).unwrap();
        let before = incompatible;
        assert_eq!(
            incompatible.copy_history_from(&source),
            Err(Error::IncompatibleHistory)
        );
        assert_eq!(incompatible, before);
    }
}

#[test]
fn finite_extrema_sign_symmetry_and_failed_stereo_frame_leave_history_untouched() {
    let prepared = prepare(12_000.0, 48_000);
    let mut state = Lowpass::new(prepared);
    for input in [[f32::MAX, -f32::MAX], [-f32::MAX, f32::MAX], [0.0; 2]] {
        let output = state.process_frame(input).unwrap();
        assert_eq!(output[0], -output[1]);
        assert!(output.iter().all(|x| x.is_finite()));
    }
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for input in [[bad, 0.2], [0.2, bad]] {
            let before = state;
            assert_eq!(state.process_frame(input), Err(Error::InvalidSample));
            assert_eq!(state, before);
        }
    }
    let mut overloaded = Lowpass::new(prepared);
    for _ in 0..2 {
        overloaded.process_frame([0.1, f32::MAX]).unwrap();
    }
    let before = overloaded;
    assert_eq!(
        overloaded.process_frame([0.1, f32::MAX]),
        Err(Error::OutputOverflow)
    );
    assert_eq!(
        overloaded, before,
        "right-lane refusal must not commit the left lane"
    );
    let mut reference = before;
    assert_eq!(
        overloaded.process_frame([0.0; 2]),
        reference.process_frame([0.0; 2])
    );
}

#[test]
fn actual_fixed_storage_is_reported_without_host_admission_claims() {
    assert_eq!(std::mem::size_of::<PreparedLowpass>(), 48);
    assert_eq!(std::mem::size_of::<Lowpass>(), 112);
    assert_eq!(std::mem::align_of::<Lowpass>(), 8);
    println!(
        "private filter prepared=48 runtime=112 stereo-history=64 hypothetical-two-copies=224 bytes; no host accounting/admission"
    );
}

#[test]
#[ignore = "explicit optimized timing probe; no native deadline claim"]
fn optimized_paired_timing_retains_all_outliers() {
    use std::{hint::black_box, time::Instant};
    let input: [[f32; 2]; 128] = std::array::from_fn(|i| [(i as f32 * 0.07).sin() * 0.25; 2]);
    for trial in 0..5 {
        for filtered in if trial % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let mut state = Lowpass::new(prepare(1000.0, 48_000));
            let mut output = [[0.0; 2]; 128];
            let mut samples = Vec::with_capacity(1000);
            let mut checksum = 0.0_f64;
            for block in 0..1200 {
                let start = Instant::now();
                for (input, output) in black_box(&input).iter().zip(&mut output) {
                    *output = if filtered {
                        state.process_frame(*input).unwrap()
                    } else {
                        *input
                    };
                }
                black_box(&output);
                let elapsed = start.elapsed().as_nanos();
                checksum += f64::from(output[31][0]);
                if block >= 200 {
                    samples.push(elapsed);
                }
            }
            samples.sort_unstable();
            println!(
                "trial={trial} filtered={filtered} rate=48000 frames=128 warmups=200 samples=1000 p50_ns={} p99_ns={} max_ns={} checksum={checksum}",
                samples[500], samples[990], samples[999]
            );
        }
    }
}
