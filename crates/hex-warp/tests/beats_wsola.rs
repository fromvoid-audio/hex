//! Phase-independent sustain checks, not a proprietary-reference comparison.
use hex_warp::*;
fn prepare(source: &[Frame], rate: u32, output: usize, pitch: i16) -> Rendered {
    let map = TimeMap::new(
        vec![MapSegment {
            output_start: 0,
            output_end: output as u64,
            source_start_q32: 0,
            source_end_q32: (source.len() as i128) << 32,
        }],
        output,
    )
    .unwrap();
    render(
        &Request {
            input: Input {
                frames: source,
                sample_rate: rate,
                region_start: 0,
                region_length: source.len() as u64,
                phase: 0,
                looping: false,
                envelope: &[],
            },
            attacks: &[],
            map: &map,
            output_frames: output,
            pitch_cents: pitch,
            mode: Mode::Beats {
                division_frames: None,
                tail: Tail::Forward,
                decay: 100,
            },
        },
        &mut || false,
    )
    .unwrap()
}
// Least-squares fundamental fit includes an independent DC term and free phase.
// A harmless constant phase change is not misclassified as choppy sustain.
fn residual(pcm: &[Frame], rate: u32, hz: f64) -> (f64, f64) {
    let n = pcm.len() as f64;
    let mut sum = [0.0; 3];
    let mut ss = 0.0;
    let mut cc = 0.0;
    let mut sc = 0.0;
    let mut xs = 0.0;
    let mut xc = 0.0;
    let mut xx = 0.0;
    for (i, x) in pcm.iter().enumerate() {
        let (s, c) = (std::f64::consts::TAU * hz * i as f64 / f64::from(rate)).sin_cos();
        let x = f64::from(x[0]);
        sum[0] += x;
        sum[1] += s;
        sum[2] += c;
        ss += s * s;
        cc += c * c;
        sc += s * c;
        xs += x * s;
        xc += x * c;
        xx += x * x;
    }
    ss -= sum[1] * sum[1] / n;
    cc -= sum[2] * sum[2] / n;
    sc -= sum[1] * sum[2] / n;
    xs -= sum[0] * sum[1] / n;
    xc -= sum[0] * sum[2] / n;
    xx -= sum[0] * sum[0] / n;
    let determinant = ss * cc - sc * sc;
    let a = (xs * cc - xc * sc) / determinant;
    let b = (xc * ss - xs * sc) / determinant;
    let error = ((xx - a * xs - b * xc).max(0.0) / n).sqrt();
    (error, (xx / n).sqrt())
}
#[test]
fn wsola_forward_periodic_sustain_has_low_added_residual_and_no_pumping() {
    for rate in [16_000, 44_100, 48_000] {
        for (hz, pitch) in [(55.0, 0), (220.0, 0), (997.0, 0), (220.0, 1200)] {
            let source: Vec<Frame> = (0..rate as usize / 3)
                .map(|i| {
                    let x = (std::f64::consts::TAU * hz * i as f64 / f64::from(rate) + 0.4).sin()
                        as f32
                        * 0.5;
                    [x, -x * 0.7]
                })
                .collect();
            let pcm = prepare(&source, rate, source.len() * 3, pitch);
            let material = &pcm.frames[source.len() / 2..pcm.frames.len() - rate as usize / 100];
            let (error, rms) =
                residual(material, rate, hz * 2.0_f64.powf(f64::from(pitch) / 1200.0));
            println!(
                "WSOLA rate={rate} hz={hz} pitch={pitch} residual={error} rms={rms} work={} scratch={}",
                pcm.work_units, pcm.peak_scratch_bytes
            );
            assert!(
                error < rms * 0.1,
                "added sustain residual {rate}/{hz}/{pitch}: {error}/{rms}"
            );
            assert!((0.30..0.40).contains(&rms), "sustain gain changed {rms}");
            assert!(pcm.frames.iter().all(|s| (s[1] + s[0] * 0.7).abs() < 1e-6));
            let window = (f64::from(rate) * 4.0 / hz).round() as usize;
            let powers: Vec<f64> = material
                .windows(window)
                .step_by((window / 4).max(1))
                .map(|w| {
                    (w.iter().map(|s| f64::from(s[0]).powi(2)).sum::<f64>() / w.len() as f64).sqrt()
                })
                .collect();
            let minimum = powers.iter().copied().fold(f64::INFINITY, f64::min);
            let maximum = powers.iter().copied().fold(0.0, f64::max);
            assert!(
                maximum / minimum < 1.15,
                "sustain pumps {minimum}..{maximum}"
            );
        }
    }
}
#[test]
fn wsola_forward_dc_and_uncorrelated_noise_keep_level_with_one_stereo_schedule() {
    for rate in [16_000, 48_000] {
        let source = vec![[0.4, -0.2]; rate as usize / 3];
        let pcm = prepare(&source, rate, source.len() * 2, 0);
        assert!(pcm.frames.iter().all(|s| *s == [0.4, -0.2]));
        let mut seed = 0x123456789abcdef_u64;
        let noise: Vec<Frame> = (0..rate as usize / 2)
            .map(|_| {
                std::array::from_fn(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    ((seed >> 11) as f64 / (1_u64 << 53) as f64 - 0.5) as f32 * 0.6
                })
            })
            .collect();
        let pcm = prepare(&noise, rate, noise.len() * 3, 0);
        let rms = |p: &[Frame]| {
            (p.iter()
                .flat_map(|s| s.iter())
                .map(|x| f64::from(*x).powi(2))
                .sum::<f64>()
                / (p.len() * 2) as f64)
                .sqrt()
        };
        let ratio = rms(&pcm.frames[rate as usize / 20..]) / rms(&noise);
        println!("WSOLA noise rate={rate} RMSratio={ratio}");
        assert!((0.85..1.15).contains(&ratio), "noise overlap gain {ratio}");
        let peak = pcm
            .frames
            .iter()
            .flat_map(|s| s.iter())
            .map(|v| v.abs())
            .fold(0.0_f32, f32::max);
        println!("WSOLA noise peak rate={rate}: {peak}");
        assert!(
            pcm.frames
                .iter()
                .all(|s| s.iter().all(|v| v.is_finite() && v.abs() < 0.6))
        );
        assert!(pcm.peak_scratch_bytes < 2 * 1024 * 1024);
    }
}
#[test]
fn wsola_forward_keeps_attacks_exact_without_preecho_or_duplicates_and_cancels() {
    let mut source = vec![[0.0; 2]; 4_000];
    for (at, x) in [
        (500, [1.0, -0.5]),
        (1_800, [-0.75, 0.4]),
        (3_200, [0.8, 0.2]),
    ] {
        source[at] = x;
    }
    for ratio in [0.5, 1.375, 2.0] {
        let output = (source.len() as f64 * ratio) as usize;
        let pcm = prepare(&source, 16_000, output, 0);
        let mut expected = vec![[0.0; 2]; output];
        for at in [500, 1_800, 3_200] {
            expected[(at as f64 * ratio) as usize] = source[at];
        }
        assert_eq!(pcm.frames, expected);
        assert_eq!(prepare(&source, 16_000, output, 0).frames, expected);
    }
    let map = TimeMap::new(
        vec![MapSegment {
            output_start: 0,
            output_end: 8_000,
            source_start_q32: 0,
            source_end_q32: 4_000_i128 << 32,
        }],
        8_000,
    )
    .unwrap();
    let request = Request {
        input: Input {
            frames: &source,
            sample_rate: 16_000,
            region_start: 0,
            region_length: 4_000,
            phase: 0,
            looping: false,
            envelope: &[],
        },
        attacks: &[],
        map: &map,
        output_frames: 8_000,
        pitch_cents: 0,
        mode: Mode::Beats {
            division_frames: None,
            tail: Tail::Forward,
            decay: 100,
        },
    };
    let mut polls = 0;
    assert!(matches!(
        render(&request, &mut || {
            polls += 1;
            polls == 5
        }),
        Err(Error::Cancelled)
    ));
}
