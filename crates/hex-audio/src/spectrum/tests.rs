use super::*;
use std::time::Duration;

const FLAT: SpectrumSettings = SpectrumSettings {
    slope_db_per_octave: 0.0,
    smoothing_octaves: 1.0 / 12.0,
};
/// Enough frames to fill the bass ring at every supported decimation.
const FILL_FRAMES: usize = BASS_FFT_SIZE * 8 + 64;

#[test]
fn tone_on_noise_does_not_jump_at_tonality_threshold() {
    let rate = 48_000;
    let frequency = 4_000.0;
    let mut seed = 1_u64;
    let noise: Vec<[f32; 2]> = (0..FILL_FRAMES)
        .map(|_| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let x = (((seed >> 40) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0) * 0.05;
            [x, x]
        })
        .collect();
    let index = point_index(rate, frequency).unwrap();
    let levels = [0.00708, 0.00710].map(|amplitude| {
        let mut frames = tone(rate, frequency, amplitude, 1.0, FILL_FRAMES);
        for (frame, n) in frames.iter_mut().zip(&noise) {
            frame[0] += n[0];
            frame[1] += n[1];
        }
        analyze(rate, FLAT, &frames).bins_dbfs[index]
    });
    let jump = (levels[1] - levels[0]).abs();
    assert!(
        jump < 0.5,
        "tone level changed {:.3} db for a tiny amplitude step: {levels:?}",
        jump
    );
    let mut previous: Option<f32> = None;
    let mut largest = 0.0f32;
    for step in 0..=64 {
        let amplitude = 0.006 + f64::from(step) * 0.00005;
        let mut frames = tone(rate, frequency, amplitude, 1.0, FILL_FRAMES);
        for (frame, n) in frames.iter_mut().zip(&noise) {
            frame[0] += n[0];
            frame[1] += n[1];
        }
        let level = analyze(rate, FLAT, &frames).bins_dbfs[index];
        if let Some(last) = previous {
            let change = (level - last).abs();
            largest = largest.max(change);
            assert!(change < 1.0, "{amplitude:.5}: {last:.3} -> {level:.3} dbfs");
        }
        previous = Some(level);
    }
    println!("worst step through tone/noise transition: {largest:.3} db");
}

#[test]
fn mixed_bass_mid_and_high_tones_remain_calibrated_over_noise() {
    let rate = 48_000;
    let components = [(70.0, 0.2), (1_000.0, 0.1), (9_000.0, 0.05)];
    let mut seed = 9_u64;
    let mut frames = vec![[0.0f32; 2]; FILL_FRAMES];
    for frame in &mut frames {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let noise = (((seed >> 40) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0) * 0.002;
        *frame = [noise, noise];
    }
    for (frequency, amplitude) in components {
        for (frame, signal) in
            frames
                .iter_mut()
                .zip(tone(rate, frequency, amplitude, 1.0, FILL_FRAMES))
        {
            frame[0] += signal[0];
            frame[1] += signal[1];
        }
    }
    let result = analyze(rate, FLAT, &frames);
    for (frequency, amplitude) in components {
        let point = point_index(rate, frequency).unwrap();
        let expected = (20.0 * amplitude.log10()) as f32;
        let actual = result.bins_dbfs[point];
        assert!(
            (actual - expected).abs() < 0.6,
            "{frequency} hz: expected {expected:.2}, got {actual:.2} dbfs"
        );
    }
}

#[test]
fn calibrated_tone_moves_continuously_across_display_band_boundary() {
    let mut worst = 0.0f32;
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for target in [500.0, 1_000.0, 5_000.0] {
            let point = point_index(rate, target).unwrap();
            let edge = band_bounds(rate, point).1;
            let lower = analyze(rate, FLAT, &tone(rate, edge - 0.1, 0.5, 1.0, FILL_FRAMES));
            let upper = analyze(rate, FLAT, &tone(rate, edge + 0.1, 0.5, 1.0, FILL_FRAMES));
            for index in [point, point + 1] {
                let change = (lower.bins_dbfs[index] - upper.bins_dbfs[index]).abs();
                worst = worst.max(change);
                assert!(
                    change < 1.0,
                    "{rate} hz, {edge:.1} hz, point {index}: {change:.2} db jump"
                );
            }
            assert!((peak(&lower).1 + 6.0206).abs() < 0.5);
            assert!((peak(&upper).1 + 6.0206).abs() < 0.5);
        }
    }
    println!("worst point change across a 0.2 hz tone step: {worst:.3} db");
}

fn tone(
    rate: u32,
    frequency: f64,
    amplitude: f64,
    right_polarity: f32,
    len: usize,
) -> Vec<[f32; 2]> {
    (0..len)
        .map(|frame| {
            let phase = std::f64::consts::TAU * frequency * frame as f64 / f64::from(rate);
            let sample = (phase.sin() * amplitude) as f32;
            [sample, sample * right_polarity]
        })
        .collect()
}

fn analyze(rate: u32, settings: SpectrumSettings, frames: &[[f32; 2]]) -> SpectrumFrame {
    let mut window = SpectrumWindow::new(rate).unwrap();
    window.push(frames).unwrap();
    SpectrumKernel::with_settings(rate, settings)
        .unwrap()
        .analyze(&window)
        .unwrap()
}

fn peak(frame: &SpectrumFrame) -> (usize, f32) {
    frame
        .bins_dbfs
        .iter()
        .copied()
        .enumerate()
        .max_by(|left, right| left.1.total_cmp(&right.1))
        .unwrap()
}

#[test]
fn sines_land_on_their_frequency_and_calibrated_level_at_every_rate() {
    let mut worst = 0.0f32;
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for frequency in [
            40.0, 63.5, 100.0, 440.0, 1_000.0, 1_234.5, 5_000.0, 12_000.0,
        ] {
            let frame = analyze(rate, FLAT, &tone(rate, frequency, 0.5, 1.0, FILL_FRAMES));
            let (point, level) = peak(&frame);
            let (low, high) = band_bounds_hz(rate, point).unwrap();
            let error = level + 6.0206;
            worst = worst.max(error.abs());
            assert!(
                (low..high).contains(&(frequency as f32)),
                "{frequency} hz at {rate} hz peaked in {low}..{high} hz"
            );
            assert!(
                error.abs() < 0.5,
                "{frequency} hz at {rate} hz read {level} dbfs"
            );
        }
    }
    println!("worst calibrated sine level error: {worst:.3} db");
}

#[test]
fn half_bin_tones_are_corrected_for_hann_scalloping() {
    let resolution = 48_000.0 / FFT_SIZE as f64;
    for bin in [180.5, 300.5, 777.5] {
        let frame = analyze(
            48_000,
            FLAT,
            &tone(48_000, bin * resolution, 0.5, -1.0, FILL_FRAMES),
        );
        let error = peak(&frame).1 + 6.0206;
        assert!(
            error.abs() < 0.3,
            "bin {bin} read {error} db from calibration"
        );
    }
}

/// Independent stereo pink noise by random-phase synthesis: per-bin power
/// density proportional to 1/f, -3 dB per octave.
fn pink_noise(len: usize) -> Vec<[f32; 2]> {
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut uniform = || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut real = vec![0.0; len];
    let mut imaginary = vec![0.0; len];
    for bin in 1..len {
        let signed = if bin <= len / 2 { bin } else { len - bin } as f64;
        let magnitude = 1.0 / signed.sqrt();
        let phase = std::f64::consts::TAU * uniform();
        real[bin] = magnitude * phase.cos();
        imaginary[bin] = -magnitude * phase.sin();
    }
    let twiddles: Vec<_> = (0..len / 2)
        .map(|index| {
            let angle = -std::f64::consts::TAU * index as f64 / len as f64;
            (angle.cos(), angle.sin())
        })
        .collect();
    fft(&mut real, &mut imaginary, &twiddles);
    let peak = real
        .iter()
        .chain(&imaginary)
        .fold(0.0f64, |peak, value| peak.max(value.abs()));
    real.iter()
        .zip(&imaginary)
        .map(|(left, right)| [(left / peak * 0.5) as f32, (-right / peak * 0.5) as f32])
        .collect()
}

#[test]
fn pink_noise_reads_flat_with_a_three_db_per_octave_slope() {
    let rate = 48_000;
    let settings = SpectrumSettings {
        slope_db_per_octave: 3.0,
        smoothing_octaves: 1.0 / 6.0,
    };
    let noise = pink_noise(1 << 17);
    let mut window = SpectrumWindow::new(rate).unwrap();
    let mut kernel = SpectrumKernel::with_settings(rate, settings).unwrap();
    window.push(&noise[..FILL_FRAMES]).unwrap();
    let mut mean_power = [0.0f64; SPECTRUM_BANDS];
    let mut frames = 0;
    for hop in noise[FILL_FRAMES..].chunks_exact(1_600) {
        window.push(hop).unwrap();
        let frame = kernel.analyze(&window).unwrap();
        for (sum, db) in mean_power.iter_mut().zip(frame.bins_dbfs) {
            *sum += 10f64.powf(f64::from(db) / 10.0);
        }
        frames += 1;
    }
    let levels: Vec<(f32, f64)> = (0..SPECTRUM_BANDS)
        .map(|point| {
            (
                band_center_hz(rate, point).unwrap(),
                10.0 * (mean_power[point] / f64::from(frames)).log10(),
            )
        })
        .filter(|(center, _)| (40.0..16_000.0).contains(center))
        .collect();
    let (low, high) = levels
        .iter()
        .fold((f64::MAX, f64::MIN), |(low, high), (_, db)| {
            (low.min(*db), high.max(*db))
        });
    println!(
        "pink noise over {frames} frames spans {:.2} db (40 hz..16 khz)",
        high - low
    );
    assert!(
        high - low < 2.0,
        "pink noise spread {low}..{high} dbfs: {levels:?}"
    );
}

#[test]
fn bass_stage_resolves_tones_ten_hz_apart_without_stair_steps() {
    let rate = 48_000;
    let mut frames = tone(rate, 50.0, 0.25, 1.0, FILL_FRAMES);
    for (frame, other) in frames
        .iter_mut()
        .zip(tone(rate, 60.0, 0.25, 1.0, FILL_FRAMES))
    {
        frame[0] += other[0];
        frame[1] += other[1];
    }
    let frame = analyze(rate, FLAT, &frames);
    let point = |hz: f32| point_index(rate, f64::from(hz)).unwrap();
    let dip = frame.bins_dbfs[point(54.8)];
    let lobes = frame.bins_dbfs[point(50.0)].min(frame.bins_dbfs[point(60.0)]);
    assert!(
        lobes - dip > 6.0,
        "50/60 hz lobes {lobes} dbfs, dip {dip} dbfs"
    );

    let low = analyze(rate, FLAT, &tone(rate, 58.6, 0.5, -1.0, FILL_FRAMES));
    let repeated = low.bins_dbfs[..48]
        .windows(2)
        .filter(|pair| pair[0] > FLOOR_DBFS && pair[0].to_bits() == pair[1].to_bits())
        .count();
    assert_eq!(
        repeated, 0,
        "sparse bass bins must interpolate continuously"
    );
}

#[test]
fn slope_tilts_around_one_kilohertz() {
    let frames = tone(48_000, 4_000.0, 0.5, 1.0, FILL_FRAMES);
    let flat = peak(&analyze(48_000, FLAT, &frames)).1;
    let tilted = peak(&analyze(48_000, SpectrumSettings::DEFAULT, &frames)).1;
    // 4 khz is two octaves above the pivot; the point center is within the band.
    assert!(
        (tilted - flat - 9.0).abs() < 0.3,
        "flat {flat}, tilted {tilted}"
    );
}

#[test]
fn stereo_power_avoids_antiphase_cancellation_and_silence_has_a_floor() {
    let aligned = analyze(48_000, FLAT, &tone(48_000, 1_500.0, 0.5, 1.0, FILL_FRAMES));
    let opposite = analyze(48_000, FLAT, &tone(48_000, 1_500.0, 0.5, -1.0, FILL_FRAMES));
    for (left, right) in aligned.bins_dbfs.iter().zip(opposite.bins_dbfs) {
        assert!((left - right).abs() < 1.0e-3);
    }
    let silent = analyze(
        48_000,
        SpectrumSettings::DEFAULT,
        &vec![[0.0; 2]; FILL_FRAMES],
    );
    assert_eq!(silent, SpectrumFrame::silent(48_000));
}

#[test]
fn geometry_follows_the_sample_rate() {
    for (rate, stages) in [
        (8_000, 1),
        (16_000, 2),
        (22_050, 2),
        (44_100, 3),
        (48_000, 3),
        (96_000, 3),
        (192_000, 3),
    ] {
        assert_eq!(bass_decimation_stages(rate), stages, "{rate} hz");
        let maximum = 20_000.0f64.min(f64::from(rate) * 0.49) as f32;
        assert_eq!(maximum_frequency_hz(rate), Some(maximum));
        assert_eq!(band_bounds_hz(rate, 0).map(|bounds| bounds.0), Some(20.0));
        assert_eq!(
            band_bounds_hz(rate, SPECTRUM_BANDS - 1).map(|bounds| bounds.1),
            Some(maximum)
        );
        assert_eq!(band_bounds_hz(rate, SPECTRUM_BANDS), None);
        for band in 0..SPECTRUM_BANDS {
            let (low, high) = band_bounds_hz(rate, band).unwrap();
            let center = band_center_hz(rate, band).unwrap();
            assert!(low < center && center < high);
            assert_eq!(point_index(rate, f64::from(center)), Some(band));
        }
        let kernel = SpectrumKernel::new(rate).unwrap();
        let crossover = kernel.high.resolution_hz * CROSSOVER_BINS * 2.0;
        // The bass stage is used only well below its own decimated Nyquist.
        assert!(crossover <= kernel.bass.resolution_hz * BASS_FFT_SIZE as f64 * 0.125 + 1.0e-9);
    }
    assert_eq!(band_bounds_hz(7_999, 0), None);
    assert_eq!(band_center_hz(192_001, 0), None);
}

#[test]
fn malformed_input_rates_and_settings_are_refused_without_corrupting_state() {
    assert_eq!(
        SpectrumKernel::new(7_999).err(),
        Some(SpectrumError::InvalidSampleRate(7_999))
    );
    assert_eq!(
        SpectrumWindow::new(192_001).err(),
        Some(SpectrumError::InvalidSampleRate(192_001))
    );
    for settings in [
        SpectrumSettings {
            slope_db_per_octave: f32::NAN,
            ..FLAT
        },
        SpectrumSettings {
            slope_db_per_octave: 13.0,
            ..FLAT
        },
        SpectrumSettings {
            smoothing_octaves: -0.1,
            ..FLAT
        },
    ] {
        assert_eq!(
            SpectrumKernel::with_settings(48_000, settings).err(),
            Some(SpectrumError::InvalidSettings)
        );
    }

    let mut kernel = SpectrumKernel::with_settings(48_000, FLAT).unwrap();
    let mut window = SpectrumWindow::new(48_000).unwrap();
    assert_eq!(kernel.analyze(&window), Err(SpectrumError::WindowNotReady));
    let valid = tone(48_000, 1_000.0, 0.25, 1.0, FILL_FRAMES);
    window.push(&valid).unwrap();
    let expected = kernel.analyze(&window).unwrap();
    let mut invalid = valid[..32].to_vec();
    invalid[23][1] = f32::NAN;
    assert_eq!(
        window.push(&invalid),
        Err(SpectrumError::NonFiniteSample {
            frame: 23,
            channel: 1
        })
    );
    kernel.reset();
    assert_eq!(kernel.analyze(&window).unwrap(), expected);

    let other = SpectrumWindow::new(44_100).unwrap();
    assert_eq!(
        kernel.analyze(&other),
        Err(SpectrumError::SampleRateMismatch {
            kernel: 48_000,
            window: 44_100
        })
    );
    window.reconfigure(96_000).unwrap();
    assert!(!window.is_ready() && !window.bass_ready());
}

fn step_input(at: Duration) -> SpectrumFrame {
    let mut frame = SpectrumFrame::silent(48_000);
    let level = if at < Duration::from_millis(500) {
        -20.0
    } else {
        -90.0
    };
    frame.bins_dbfs = [level; SPECTRUM_BANDS];
    frame.peak_dbfs = frame.bins_dbfs;
    frame
}

fn run_ballistics(rate_hz: u32, until: Duration) -> SpectrumFrame {
    let mut ballistics = SpectrumBallistics::default();
    let step = Duration::from_secs(1) / rate_hz;
    let mut at = Duration::ZERO;
    let mut frame = SpectrumFrame::silent(48_000);
    ballistics.apply(&mut frame, Duration::ZERO);
    while at < until {
        frame = step_input(at);
        ballistics.apply(&mut frame, step);
        at += step;
    }
    frame
}

#[test]
fn ballistics_and_peak_hold_are_independent_of_the_update_rate() {
    for until in [100, 500, 700, 1_200, 2_000, 3_000].map(Duration::from_millis) {
        let slow = run_ballistics(20, until);
        let fast = run_ballistics(120, until);
        for (slow, fast) in [
            (slow.bins_dbfs[0], fast.bins_dbfs[0]),
            (slow.peak_dbfs[0], fast.peak_dbfs[0]),
        ] {
            assert!((slow - fast).abs() < 0.01, "at {until:?}: {slow} vs {fast}");
        }
    }
    let attacked = run_ballistics(60, Duration::from_millis(100));
    assert!(
        (attacked.bins_dbfs[0] + 20.0).abs() < 0.01,
        "attack is fast"
    );
    let released = run_ballistics(60, Duration::from_millis(600));
    assert!(
        released.bins_dbfs[0] < -40.0 && released.bins_dbfs[0] > -90.0,
        "release is gradual"
    );
    assert_eq!(released.peak_dbfs[0], -20.0, "peak holds for one second");
    let decayed = run_ballistics(60, Duration::from_millis(2_000));
    assert!(
        (decayed.peak_dbfs[0] - (-20.0 - 0.5 * 6.0)).abs() < 0.05,
        "peak then falls slowly"
    );
}
