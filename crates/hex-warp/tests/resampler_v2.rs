//! Measured reconstruction quality of the v2 warp resampler against v1.
//! Unwarped pitch resampling (`Mode::Resample`) reads the source at `rate`
//! = 2^(cents/1200); a tone above `1/rate` of source Nyquist must be removed,
//! not folded back. Kernel-support edges are excluded from every measurement.
use hex_warp::*;
const RATE: u32 = 48_000;
const FRAMES: usize = 48_000;
const EDGE: usize = 600;

fn tone(hz: f64) -> Vec<Frame> {
    (0..FRAMES)
        .map(|n| {
            let v = (std::f64::consts::TAU * hz * n as f64 / f64::from(RATE)).sin() as f32 * 0.5;
            [v, -v]
        })
        .collect()
}

fn resample(source: &[Frame], cents: i16, resampler: Resampler) -> Vec<f64> {
    let rate = 2f64.powf(f64::from(cents) / 1_200.0);
    let output = ((source.len() as f64 / rate) as usize).min(source.len());
    let map = TimeMap::new(
        vec![MapSegment {
            output_start: 0,
            output_end: output as u64,
            source_start_q32: 0,
            source_end_q32: (output as i128) << 32,
        }],
        output,
    )
    .unwrap();
    let rendered = render_with_resampler(
        &Request {
            attacks: &[],
            input: Input {
                frames: source,
                sample_rate: RATE,
                region_start: 0,
                region_length: source.len() as u64,
                phase: 0,
                looping: false,
                envelope: &[],
            },
            map: &map,
            output_frames: output,
            pitch_cents: cents,
            mode: Mode::Resample,
        },
        resampler,
        &mut || false,
    )
    .unwrap();
    let left: Vec<f64> = rendered.frames.iter().map(|f| f64::from(f[0])).collect();
    left[EDGE..left.len() - EDGE].to_vec()
}

fn db(v: f64) -> f64 {
    20.0 * v.max(1e-300).log10()
}

/// Output level of a tone that must be removed, relative to the input tone.
fn alias(hz: f64, cents: i16, resampler: Resampler) -> f64 {
    let out = resample(&tone(hz), cents, resampler);
    let rms = (out.iter().map(|v| v * v).sum::<f64>() / out.len() as f64).sqrt();
    db(rms * 2f64.sqrt() / 0.5)
}

/// (gain dB, residual dB) of a passband tone after resampling.
fn passband(hz: f64, cents: i16, resampler: Resampler) -> (f64, f64) {
    let rate = 2f64.powf(f64::from(cents) / 1_200.0);
    let out = resample(&tone(hz), cents, resampler);
    let w = std::f64::consts::TAU * hz * rate / f64::from(RATE);
    let (mut ss, mut cc, mut sc, mut ys, mut yc) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (n, y) in out.iter().enumerate() {
        let (s, c) = (w * (n + EDGE) as f64).sin_cos();
        ss += s * s;
        cc += c * c;
        sc += s * c;
        ys += y * s;
        yc += y * c;
    }
    let det = ss * cc - sc * sc;
    let (a, b) = ((ys * cc - yc * sc) / det, (yc * ss - ys * sc) / det);
    let amplitude = (a * a + b * b).sqrt();
    let residual = out
        .iter()
        .enumerate()
        .map(|(n, y)| {
            let (s, c) = (w * (n + EDGE) as f64).sin_cos();
            (y - a * s - b * c).powi(2)
        })
        .sum::<f64>()
        / out.len() as f64;
    (
        db(amplitude / 0.5),
        db(residual.sqrt() * 2f64.sqrt() / amplitude),
    )
}

#[test]
fn v2_rejects_aliasing_at_rate_four_and_one_and_a_half() {
    // Rate 4: source tones above 6 kHz exceed the 24 kHz output band.
    for hz in [6_300.0, 7_000.0, 9_000.0, 11_500.0] {
        let level = alias(hz, 2_400, Resampler::V2);
        assert!(
            level <= -85.0,
            "rate 4, {hz} Hz folds back at {level:.1} dB"
        );
    }
    // Rate 1.5 (702 cents): tones above 16 kHz alias.
    for hz in [16_600.0, 20_000.0] {
        let level = alias(hz, 702, Resampler::V2);
        assert!(
            level <= -85.0,
            "rate 1.5, {hz} Hz folds back at {level:.1} dB"
        );
    }
}

/// Saved-project migration relies on this: unpitched, unwarped content reads
/// integer source positions at unity rate, where both laws return the source
/// frames exactly, so such content may take v2 without changing its output.
#[test]
fn unpitched_resample_is_identical_under_both_laws() {
    let source: Vec<Frame> = (0..4_096u32)
        .map(|n| {
            let v = ((n.wrapping_mul(2_654_435_761) >> 8) as f32 / 16_777_216.0) - 0.5;
            [v, v * -0.75]
        })
        .collect();
    let envelope = [
        GainPoint {
            source_frame: 0,
            gain_q16: 65_536,
        },
        GainPoint {
            source_frame: 900,
            gain_q16: 20_000,
        },
    ];
    let output = 5_000usize;
    let map = TimeMap::new(
        vec![MapSegment {
            output_start: 0,
            output_end: output as u64,
            source_start_q32: 0,
            source_end_q32: (output as i128) << 32,
        }],
        output,
    )
    .unwrap();
    let request = Request {
        attacks: &[],
        input: Input {
            frames: &source,
            sample_rate: RATE,
            region_start: 123,
            region_length: 1_777,
            phase: 37,
            looping: true,
            envelope: &envelope,
        },
        map: &map,
        output_frames: output,
        pitch_cents: 0,
        mode: Mode::Resample,
    };
    let v1 = render(&request, &mut || false).unwrap().frames;
    let v2 = render_with_resampler(&request, Resampler::V2, &mut || false)
        .unwrap()
        .frames;
    let bits = |frames: &[Frame]| -> Vec<[u32; 2]> {
        frames
            .iter()
            .map(|f| [f[0].to_bits(), f[1].to_bits()])
            .collect()
    };
    assert_eq!(bits(&v1), bits(&v2));
}

#[test]
fn v2_passband_stays_flat_below_its_transition() {
    for (hz, cents) in [
        (1_000.0, 2_400),
        (4_800.0, 2_400),
        (10_000.0, 702),
        (8_000.0, -1_200),
    ] {
        let (gain, residual) = passband(hz, cents, Resampler::V2);
        assert!(
            gain.abs() <= 0.01,
            "{hz} Hz at {cents} cents: gain {gain:.4} dB"
        );
        assert!(
            residual <= -80.0,
            "{hz} Hz at {cents} cents: residual {residual:.1} dB"
        );
    }
}

/// Release-only report for the PR; run with
/// `cargo test --release -p hex-warp --test resampler_v2 -- --ignored --nocapture`.
#[test]
#[ignore]
fn resampler_report() {
    for resampler in [Resampler::V1, Resampler::V2] {
        for hz in [6_300.0, 7_000.0, 9_000.0, 11_500.0] {
            eprintln!(
                "{resampler:?} rate 4 alias {hz} Hz: {:.1} dB",
                alias(hz, 2_400, resampler)
            );
        }
        for hz in [16_600.0, 20_000.0] {
            eprintln!(
                "{resampler:?} rate 1.5 alias {hz} Hz: {:.1} dB",
                alias(hz, 702, resampler)
            );
        }
        for (hz, cents) in [
            (1_000.0, 2_400),
            (4_800.0, 2_400),
            (5_500.0, 2_400),
            (10_000.0, 702),
            (14_500.0, 702),
            (8_000.0, -1_200),
        ] {
            let (gain, residual) = passband(hz, cents, resampler);
            eprintln!(
                "{resampler:?} {hz} Hz at {cents} cents: gain {gain:.4} dB, residual {residual:.1} dB"
            );
        }
        let source: Vec<Frame> = (0..480_000)
            .map(|n| {
                let v = ((n * 7_919) % 1_013) as f32 / 1_013.0 - 0.5;
                [v, -v]
            })
            .collect();
        for cents in [702_i16, 2_400, -1_200] {
            let mut best = f64::MAX;
            for _ in 0..3 {
                let start = std::time::Instant::now();
                std::hint::black_box(resample(&source, cents, resampler));
                best = best.min(start.elapsed().as_secs_f64());
            }
            let rate = 2f64.powf(f64::from(cents) / 1_200.0);
            let output_seconds = 10.0 / rate.max(1.0);
            eprintln!(
                "{resampler:?} {cents} cents throughput: {:.1} ms for {:.2} s of stereo output (best of 3)",
                best * 1e3,
                output_seconds.min(10.0)
            );
        }
    }
}
