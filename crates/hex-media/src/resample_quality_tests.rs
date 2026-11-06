//! Measured conversion quality limits for the v2 kernel.
//!
//! Signals are 0.5-amplitude sines.
//! A least-squares sine fit at the test frequency gives the gain; the residual
//! after removing that fitted sine is the combined noise, aliasing, imaging and
//! distortion. Kernel-support edges are excluded. v1 output stays frozen by
//! `resample_plan_tests::v1_golden_digests_freeze_edge_impulse_and_binary_outputs`.
use crate::{ResampleKernel, ResampleLimits, ResamplePlan, ResampleVersion, resample_with_version};
use hex_engine::Audio;

const LIMITS: ResampleLimits = ResampleLimits {
    max_source_frames: 1_000_000,
    max_output_frames: 1_000_000,
};
const EDGE: usize = 1_000;

fn sine(rate: u32, hz: f64, frames: usize) -> Audio {
    let data = (0..frames)
        .map(|n| {
            let v = (std::f64::consts::TAU * hz * n as f64 / f64::from(rate)).sin() as f32 * 0.5;
            [v, -v]
        })
        .collect();
    Audio::new(rate, data).expect("valid sine")
}

/// Least-squares sine at `hz` over `samples`: (amplitude, residual rms).
fn fit(samples: &[f64], rate: u32, hz: f64) -> (f64, f64) {
    let w = std::f64::consts::TAU * hz / f64::from(rate);
    let (mut ss, mut cc, mut sc, mut ys, mut yc) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (n, y) in samples.iter().enumerate() {
        let (s, c) = (w * n as f64).sin_cos();
        ss += s * s;
        cc += c * c;
        sc += s * c;
        ys += y * s;
        yc += y * c;
    }
    let det = ss * cc - sc * sc;
    let a = (ys * cc - yc * sc) / det;
    let b = (yc * ss - ys * sc) / det;
    let residual = samples
        .iter()
        .enumerate()
        .map(|(n, y)| {
            let (s, c) = (w * n as f64).sin_cos();
            (y - a * s - b * c).powi(2)
        })
        .sum::<f64>();
    (
        (a * a + b * b).sqrt(),
        (residual / samples.len() as f64).sqrt(),
    )
}

fn db(value: f64) -> f64 {
    20.0 * value.max(1.0e-300).log10()
}

fn convert(version: ResampleVersion, from: u32, to: u32, hz: f64) -> Vec<f64> {
    let output = resample_with_version(&sine(from, hz, from as usize / 4), to, LIMITS, version)
        .expect("conversion");
    let left: Vec<f64> = output.frames().iter().map(|f| f64::from(f[0])).collect();
    left[EDGE..left.len() - EDGE].to_vec()
}

/// (gain dB, residual dB relative to the fitted tone).
fn passband(version: ResampleVersion, from: u32, to: u32, hz: f64) -> (f64, f64) {
    let (amplitude, residual) = fit(&convert(version, from, to, hz), to, hz);
    (db(amplitude / 0.5), db(residual * 2f64.sqrt() / amplitude))
}

/// Output level of a tone that must be removed, relative to the input tone.
fn stopband(version: ResampleVersion, from: u32, to: u32, hz: f64) -> f64 {
    let output = convert(version, from, to, hz);
    let rms = (output.iter().map(|v| v * v).sum::<f64>() / output.len() as f64).sqrt();
    db(rms * 2f64.sqrt() / 0.5)
}

#[test]
fn v2_rejects_sampled_aliasing_tones_below_minus_90_db() {
    for hz in [22_100.0, 22_500.0, 23_000.0, 23_900.0] {
        let level = stopband(ResampleVersion::V2, 48_000, 44_100, hz);
        assert!(level <= -90.0, "48k->44.1k {hz} Hz leaks {level:.1} dB");
    }
    for hz in [22_100.0, 32_100.0, 43_900.0, 47_000.0] {
        let level = stopband(ResampleVersion::V2, 96_000, 44_100, hz);
        assert!(level <= -90.0, "96k->44.1k {hz} Hz leaks {level:.1} dB");
    }
}

#[test]
fn v2_passband_is_flat_and_clean_to_its_edge() {
    for (from, to) in [(48_000, 44_100), (44_100, 48_000)] {
        for hz in [1_000.0, 10_000.0, 19_000.0] {
            let (gain, residual) = passband(ResampleVersion::V2, from, to, hz);
            assert!(
                gain.abs() <= 0.001,
                "{from}->{to} {hz} Hz gain {gain:.5} dB"
            );
            assert!(
                residual <= -100.0,
                "{from}->{to} {hz} Hz residual {residual:.1} dB"
            );
        }
    }
}

#[test]
fn v2_windows_match_whole_conversion_bits_at_any_partition() {
    let source = sine(44_100, 3_000.0, 3_000);
    let whole = resample_with_version(&source, 48_000, LIMITS, ResampleVersion::V2)
        .expect("whole conversion");
    let kernel =
        ResampleKernel::with_version(44_100, 48_000, ResampleVersion::V2).expect("v2 kernel");
    let plan = ResamplePlan::with_kernel(kernel, 3_000, 3_000, 10_000, 10_000).expect("plan");
    let mut output = vec![[0.0; 2]; whole.frames().len()];
    let mut start = 0;
    for count in [1, 7, 333, 64].into_iter().cycle() {
        if start == output.len() {
            break;
        }
        let end = (start + count).min(output.len());
        let (from, to) = plan
            .required_source_range(start as u64, end - start)
            .expect("halo");
        plan.render_window(
            from,
            &source.frames()[from as usize..to as usize],
            start as u64,
            &mut output[start..end],
        )
        .expect("window");
        start = end;
    }
    assert_eq!(output, whole.frames());
}

#[test]
fn v2_extreme_rate_pairs_fit_the_table_ceiling_and_stay_finite() {
    for (from, to) in [(192_000, 8_000), (8_000, 192_000), (176_400, 11_025)] {
        let kernel = ResampleKernel::with_version(from, to, ResampleVersion::V2).expect("kernel");
        assert!(kernel.table_payload_bytes() <= ResampleKernel::max_table_payload_bytes());
        let output =
            resample_with_version(&sine(from, 440.0, 4_000), to, LIMITS, ResampleVersion::V2)
                .expect("conversion");
        assert!(output.frames().iter().flatten().all(|v| v.is_finite()));
    }
}

/// Release-only quality and throughput report for the PR; run with
/// `cargo test --release -p hex-media --lib resample_quality_report -- --ignored --nocapture`.
#[test]
#[ignore]
fn resample_quality_report() {
    for version in [ResampleVersion::V1, ResampleVersion::V2] {
        for (from, to) in [(48_000, 44_100), (44_100, 48_000)] {
            for hz in [1_000.0, 10_000.0, 19_000.0, 20_000.0, 21_000.0] {
                let (gain, residual) = passband(version, from, to, hz);
                eprintln!(
                    "{version:?} {from}->{to} {hz} Hz: gain {gain:.4} dB, residual {residual:.1} dB"
                );
            }
        }
        for hz in [22_100.0, 22_500.0, 23_000.0, 23_900.0] {
            eprintln!(
                "{version:?} 48000->44100 {hz} Hz stopband: {:.1} dB",
                stopband(version, 48_000, 44_100, hz)
            );
        }
        let source = sine(48_000, 1_000.0, 480_000);
        let mut best = f64::MAX;
        for _ in 0..5 {
            let start = std::time::Instant::now();
            std::hint::black_box(
                resample_with_version(&source, 44_100, LIMITS, version).expect("conversion"),
            );
            best = best.min(start.elapsed().as_secs_f64());
        }
        eprintln!(
            "{version:?} throughput 48000->44100 stereo: {:.1} ms per 10 s (best of 5)",
            best * 1e3
        );
    }
}

#[test]
fn saved_conversion_versions_select_their_law_and_unknown_versions_refuse() {
    assert_eq!(
        ResampleVersion::for_conversion(0).unwrap(),
        ResampleVersion::V1
    );
    assert_eq!(
        ResampleVersion::for_conversion(1).unwrap(),
        ResampleVersion::V1
    );
    assert_eq!(
        ResampleVersion::for_conversion(2).unwrap(),
        ResampleVersion::V2
    );
    assert!(ResampleVersion::for_conversion(3).is_err());
}
