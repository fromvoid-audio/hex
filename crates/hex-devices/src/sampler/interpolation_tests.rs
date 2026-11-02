//! Alias and passband measurements for pitched-up reconstruction (#4089).
use super::*;

const RATE: u32 = 48_000;
const FRAMES: usize = 1_024;
const START_AGE: u64 = 128;

/// Worst stopband alias and passband residual over a +24 semitone tone sweep.
#[derive(Debug)]
struct Sweep {
    /// Max output/input energy for tones whose pitched frequency exceeds the
    /// output Nyquist (pure alias), in dB.
    alias_db: f64,
    /// Max residual (output minus best-fit sinusoid) energy relative to input
    /// for tones pitched below the output Nyquist, in dB.
    residual_db: f64,
    /// Fraction of output Nyquist below which every swept tone keeps its gain
    /// within 0.1 dB.
    passband_edge: f64,
}

/// Minimum 0.1 dB passband edge, as a fraction of output Nyquist, for
/// kernels within the tap cap.
const PASSBAND_EDGE: f64 = 0.45;

fn db(ratio: f64) -> f64 {
    10.0 * ratio.max(1e-30).log10()
}

fn sweep(transpose: f64, detune_cents: f64, linear: bool) -> Sweep {
    let rate = 2_f64.powf((transpose + detune_cents / 100.0) / 12.0);
    let mut alias_db = f64::NEG_INFINITY;
    let mut residual_db = f64::NEG_INFINITY;
    let mut passband_edge: Option<f64> = None;
    let nyquist = f64::from(RATE) / 2.0;
    let mut frequency = 100.0;
    while frequency < nyquist * 0.98 {
        let omega = std::f64::consts::TAU * frequency / f64::from(RATE);
        // Just enough source for the measured window plus the kernel span.
        let length = ((START_AGE as usize + FRAMES) as f64 * rate) as usize + 256;
        let pcm: Vec<StereoFrame> = (0..length)
            .map(|frame| {
                let value = (omega * frame as f64).sin() as f32 * 0.5;
                [value, value]
            })
            .collect();
        let mut settings = Settings::new(Region {
            start: 0,
            end: pcm.len(),
        });
        settings.transpose_semitones = transpose;
        settings.detune_cents = detune_cents;
        let sampler =
            PreparedSampler::new(Sample::new(&pcm, RATE).unwrap(), RATE, settings).unwrap();
        let mut voice = sampler.prepare_voice(60, 127).unwrap().unwrap();
        if linear {
            voice.coefficients.kernel = None;
        }
        let mut output = vec![[0.0_f32; 2]; FRAMES];
        voice.render(START_AGE, u64::MAX, &mut output);
        let input_energy = 0.125; // mean square of 0.5 sin
        let out_frequency = frequency * rate;
        let energy = output
            .iter()
            .map(|frame| f64::from(frame[0]).powi(2))
            .sum::<f64>()
            / FRAMES as f64;
        if out_frequency > nyquist {
            alias_db = alias_db.max(db(energy / input_energy));
        } else {
            // Least-squares fit of a sinusoid at the pitched frequency.
            let w = std::f64::consts::TAU * out_frequency / f64::from(RATE);
            let (mut ss, mut sc, mut cc, mut ys, mut yc) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for (n, frame) in output.iter().enumerate() {
                let (s, c) = (w * n as f64).sin_cos();
                let y = f64::from(frame[0]);
                ss += s * s;
                sc += s * c;
                cc += c * c;
                ys += y * s;
                yc += y * c;
            }
            let det = ss * cc - sc * sc;
            let a = (ys * cc - yc * sc) / det;
            let b = (yc * ss - ys * sc) / det;
            let residual = output
                .iter()
                .enumerate()
                .map(|(n, frame)| {
                    let (s, c) = (w * n as f64).sin_cos();
                    (f64::from(frame[0]) - a * s - b * c).powi(2)
                })
                .sum::<f64>()
                / FRAMES as f64;
            residual_db = residual_db.max(db(residual / input_energy));
            let gain = (a * a + b * b).sqrt() / 0.5;
            let error = (20.0 * gain.log10()).abs();
            if error > 0.1 && passband_edge.is_none() {
                passband_edge = Some(out_frequency / nyquist);
            }
        }
        frequency += 200.0;
    }
    Sweep {
        alias_db,
        residual_db,
        passband_edge: passband_edge.unwrap_or(1.0),
    }
}

/// Acceptance: +24 semitones (plus 7 cents so positions are fractional).
/// Thresholds: alias at most -60 dB, in-band residual at most -60 dB, and a
/// 0.1 dB passband reaching at least 45% of output Nyquist (10.8 kHz at
/// 48 kHz). +36 semitones is far past the tap cap: alias stays bounded while
/// the passband narrows to about 10% of Nyquist. The linear baseline is the
/// ignored record below.
#[test]
fn plus_24_semitone_sweep_alias_energy_stays_below_threshold() {
    for (transpose, cents, check_passband) in [
        (24.0, 7.0, true),
        (24.0, 0.0, true),
        (12.0, 31.0, true),
        (2.0, 0.0, true),
        (36.0, 0.0, false),
    ] {
        let after = sweep(transpose, cents, false);
        println!("transpose={transpose} cents={cents} bandlimited={after:?}");
        assert!(after.alias_db <= -60.0, "{after:?}");
        assert!(after.residual_db <= -60.0, "{after:?}");
        if check_passband {
            assert!(after.passband_edge >= PASSBAND_EDGE, "{after:?}");
        }
    }
}

/// Before/after record on the same harness: linear interpolation aliases at
/// roughly full level when pitched up; run with `--ignored --nocapture`.
#[test]
#[ignore = "measurement record, not a gate"]
fn linear_baseline_alias_energy_record() {
    for (transpose, cents) in [
        (24.0, 7.0),
        (24.0, 0.0),
        (12.0, 31.0),
        (2.0, 0.0),
        (36.0, 0.0),
    ] {
        let before = sweep(transpose, cents, true);
        println!("transpose={transpose} cents={cents} linear={before:?}");
        assert!(before.alias_db > -10.0);
    }
}

#[test]
fn unity_and_slower_rates_keep_linear_interpolation() {
    assert!(interpolation::Kernel::for_rate(1.0).is_none());
    assert!(interpolation::Kernel::for_rate(0.5).is_none());
    assert!(interpolation::Kernel::for_rate(f64::NAN).is_none());
    assert!(interpolation::Kernel::for_rate(1.000_001).is_some());
}

#[test]
fn dc_is_exact_and_region_edges_fade_to_silence() {
    let pcm = vec![[0.25_f32, -0.5]; 4_000];
    let mut settings = Settings::new(Region {
        start: 1_000,
        end: 3_000,
    });
    settings.transpose_semitones = 19.0;
    let sampler = PreparedSampler::new(Sample::new(&pcm, RATE).unwrap(), RATE, settings).unwrap();
    let voice = sampler.prepare_voice(60, 127).unwrap().unwrap();
    // Mid-region: normalized weights reproduce DC exactly at any phase.
    for age in 300..340 {
        let value = voice.sample(age, u64::MAX);
        assert!((value[0] - 0.25).abs() < 1e-6 && (value[1] + 0.5).abs() < 1e-6);
    }
    // Onset: frames before the region read silence, not the outside PCM.
    let onset = voice.sample(0, u64::MAX);
    assert!(onset[0] < 0.25 && onset[0] > 0.0, "{onset:?}");
}

#[test]
fn looped_taps_continue_at_loop_start() {
    // A constant loop body between a distinct attack and tail: once playback
    // has wrapped, taps on either side of the loop seam must read the loop
    // body (its end before the start, its start after the end), never the
    // attack or the tail.
    let mut pcm = vec![[1.0_f32; 2]; 3_000];
    pcm[..1_000].fill([-1.0; 2]);
    pcm[2_000..].fill([-1.0; 2]);
    let mut settings = Settings::new(Region {
        start: 0,
        end: 3_000,
    });
    settings.mode = Mode::Classic {
        loop_region: Some(Region {
            start: 1_000,
            end: 2_000,
        }),
    };
    settings.transpose_semitones = 12.0;
    settings.detune_cents = 13.0;
    let sampler = PreparedSampler::new(Sample::new(&pcm, RATE).unwrap(), RATE, settings).unwrap();
    let voice = sampler.prepare_voice(60, 127).unwrap().unwrap();
    // The rate is about 2.015, so the first wrap is near age 993.
    for age in 1_000..3_000 {
        let value = voice.sample(age, u64::MAX);
        assert!((value[0] - 1.0).abs() < 1e-6, "age {age}: {value:?}");
    }
}

#[test]
fn non_finite_raw_pcm_reads_silence() {
    let pcm = vec![[0.5_f32; 2]; 1_000];
    let settings = Settings::new(Region {
        start: 0,
        end: 1_000,
    });
    let sampler = PreparedSampler::new(Sample::new(&pcm, RATE).unwrap(), RATE, settings).unwrap();
    let coefficients = sampler
        .prepare_voice(72, 127)
        .unwrap()
        .unwrap()
        .coefficients();
    let mut raw = pcm.clone();
    raw[200] = [f32::NAN, 0.0];
    let pcm = hex_lattice::DeviceResourcePcm::Stereo(&raw);
    assert_eq!(coefficients.sample(pcm, 100, u64::MAX), [0.0; 2]);
    assert!(coefficients.sample(pcm, 300, u64::MAX)[0] > 0.49);
}
